//! History: old versions of pages and deleted pages/sections, kept in hidden `.history` folders.
//!
//! See docs/superpowers/specs/2026-11-04-history-design.md. Only `fs::copy` and `fs::rename` within
//! one volume are used, so this works on network drives. Nothing is deleted automatically.

use chrono::NaiveDateTime;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Threads reading files in `search_history`.
const SEARCH_THREADS: usize = 8;

pub const HISTORY_DIR: &str = ".history";

/// One saved older text of a page.
pub struct Version {
    pub path: PathBuf,
    pub when: NaiveDateTime,
    pub size: u64,
}

const TIME_FORMAT: &str = "%Y-%m-%dT%H%M%S";
/// Length of a formatted time in a file name (`2026-11-04T110500`).
const TIME_LEN: usize = 17;
const MIN_AGE_MINUTES: i64 = 10;
const BIG_DELETION_CHARS: usize = 200;

/// Where the older versions of a page are kept: `<folder>\.history\pages\<page name>`.
pub fn versions_dir(page: &Path) -> PathBuf {
    let dir = page.parent().unwrap_or(Path::new("."));
    dir.join(HISTORY_DIR).join("pages").join(crate::store::file_stem(page))
}

fn body_of(path: &Path) -> String {
    crate::store::read_doc(path).map(|d| d.body).unwrap_or_default()
}

/// Copies the page as it is on disk into its history when a save is about to overwrite it and
/// the rules say a version is due: the body changes and there is no version yet, or the newest one
/// is at least 10 minutes old, or the new body is shorter by more than 200 characters or 20 %.
/// `Ok(None)` when nothing is due or the page does not exist.
pub fn snapshot_before_save(page: &Path, new_body: &str, now: NaiveDateTime) -> Result<Option<PathBuf>, String> {
    if !page.is_file() {
        return Ok(None);
    }
    let old_body = body_of(page);
    if old_body == new_body {
        return Ok(None);
    }
    let versions = list_versions(page);
    let due = match versions.first() {
        None => true,
        Some(newest) => {
            if body_of(&newest.path) == old_body {
                return Ok(None); // this text is already kept
            }
            let (old, new) = (old_body.chars().count(), new_body.chars().count());
            let shrink = old.saturating_sub(new);
            now - newest.when >= chrono::Duration::minutes(MIN_AGE_MINUTES)
                || shrink > BIG_DELETION_CHARS
                || (shrink as f64) > 0.2 * old as f64
        }
    };
    if due { snapshot_now(page, now).map(Some) } else { Ok(None) }
}

/// Copies the page as it is on disk into its history, always.
pub fn snapshot_now(page: &Path, now: NaiveDateTime) -> Result<PathBuf, String> {
    let dir = versions_dir(page);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("history::snapshot_now: could not create the history folder {}: {e}", dir.display()))?;
    let stamp = now.format(TIME_FORMAT).to_string();
    let mut target = dir.join(format!("{stamp}.md"));
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("{stamp} ({n}).md"));
        n += 1;
    }
    std::fs::copy(page, &target).map_err(|e| {
        format!("history::snapshot_now: could not copy {} to {}: {e}", page.display(), target.display())
    })?;
    Ok(target)
}

/// The saved versions of a page, newest first.
pub fn list_versions(page: &Path) -> Vec<Version> {
    let Ok(rd) = std::fs::read_dir(versions_dir(page)) else { return Vec::new() };
    let mut out: Vec<Version> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let stem = name.strip_suffix(".md")?;
            let when = NaiveDateTime::parse_from_str(stem.get(..TIME_LEN)?, TIME_FORMAT).ok()?;
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            Some(Version { path: e.path(), when, size })
        })
        .collect();
    out.sort_by(|a, b| b.when.cmp(&a.when).then_with(|| b.path.cmp(&a.path)));
    out
}

/// Puts an older version back: the current text is kept as a version first, then the version's body
/// is saved into the page (current title kept, `updated` set to now).
pub fn restore_version(page: &Path, version: &Path) -> Result<(), String> {
    let doc = crate::store::read_doc(version)
        .map_err(|e| format!("history::restore_version: could not read {}: {e}", version.display()))?;
    snapshot_now(page, chrono::Local::now().naive_local())?;
    crate::store::save_body(page, &doc.body)
}

/// After a page was renamed: its versions follow it.
pub fn rename_versions(old_page: &Path, new_page: &Path) -> Result<(), String> {
    let (from, to) = (versions_dir(old_page), versions_dir(new_page));
    if from == to || !from.is_dir() {
        return Ok(());
    }
    // A rename that only changes the case names the same folder on Windows.
    if from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase() {
        return move_to(&from, &to);
    }
    if to.exists() {
        return merge_dir(&from, &to);
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("history::rename_versions: could not create {}: {e}", parent.display()))?;
    }
    move_to(&from, &to)
}

/// Removes all saved versions of a page.
pub fn purge_versions(page: &Path) -> Result<(), String> {
    let dir = versions_dir(page);
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|e| format!("history::purge_versions: could not remove {}: {e}", dir.display()))
}


#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Page,
    Section,
}

/// Something deleted from a folder and kept in that folder's `.history\deleted`.
#[derive(Clone, Debug)]
pub struct DeletedItem {
    /// The `deleted\<time>` folder that holds the item.
    pub entry: PathBuf,
    pub name: String,
    /// The file name of a deleted page (`X.md`, `X.markdown`); the folder name of a section.
    pub file: String,
    pub kind: ItemKind,
    /// The folder the item was deleted from (the one that holds the `.history`).
    pub original_folder: PathBuf,
    pub deleted_at: NaiveDateTime,
}

const VERSIONS_IN_ENTRY: &str = ".versions";

fn deleted_root(folder: &Path) -> PathBuf {
    folder.join(HISTORY_DIR).join("deleted")
}

/// A new `deleted\<time>` folder for something deleted from `folder`.
fn new_entry(folder: &Path, now: NaiveDateTime) -> Result<PathBuf, String> {
    let stamp = now.format(TIME_FORMAT).to_string();
    let root = deleted_root(folder);
    let mut entry = root.join(&stamp);
    let mut n = 2;
    while entry.exists() {
        entry = root.join(format!("{stamp} ({n})"));
        n += 1;
    }
    std::fs::create_dir_all(&entry)
        .map_err(|e| format!("history: could not create the history folder {}: {e}", entry.display()))?;
    Ok(entry)
}

fn move_to(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::rename(from, to)
        .map_err(|e| format!("history: could not move {} to {}: {e}", from.display(), to.display()))
}

/// Moves each `(from, to)` pair in order; when one fails, the earlier ones are moved back and the
/// (empty) entry folder is removed.
fn move_all(moves: &[(PathBuf, PathBuf)], entry: &Path) -> Result<(), String> {
    for (i, (from, to)) in moves.iter().enumerate() {
        if let Err(e) = move_to(from, to) {
            for (f, t) in moves[..i].iter().rev() {
                let _ = std::fs::rename(t, f);
            }
            let _ = std::fs::remove_dir(entry);
            return Err(e);
        }
    }
    Ok(())
}

/// Moves a page, its subpages folder and its versions into the `.history\deleted` of its folder.
/// Returns the entry folder.
pub fn delete_page(page: &Path, now: NaiveDateTime) -> Result<PathBuf, String> {
    let folder = page.parent().ok_or("history::delete_page: the page has no folder")?;
    let name = page.file_name().ok_or("history::delete_page: the page has no name")?;
    let entry = new_entry(folder, now)?;
    let mut moves = vec![(page.to_path_buf(), entry.join(name))];
    let sub = crate::store::subpage_dir(page);
    if sub.is_dir() {
        moves.push((sub.clone(), entry.join(sub.file_name().unwrap_or_default())));
    }
    let versions = versions_dir(page);
    if versions.is_dir() {
        moves.push((versions, entry.join(VERSIONS_IN_ENTRY)));
    }
    move_all(&moves, &entry)?;
    Ok(entry)
}

/// Moves a folder into the `.history\deleted` of its parent. Returns the entry folder.
pub fn delete_folder(folder: &Path, now: NaiveDateTime) -> Result<PathBuf, String> {
    let parent = folder.parent().ok_or("history::delete_folder: a top-level folder cannot be deleted")?;
    let name = folder.file_name().ok_or("history::delete_folder: the folder has no name")?;
    let entry = new_entry(parent, now)?;
    move_all(&[(folder.to_path_buf(), entry.join(name))], &entry)?;
    Ok(entry)
}

/// What is in one `deleted\<time>` folder, if it looks like something we put there.
fn read_entry(folder: &Path, entry: &Path) -> Option<DeletedItem> {
    let stamp = entry.file_name()?.to_string_lossy().to_string();
    let deleted_at = NaiveDateTime::parse_from_str(stamp.get(..TIME_LEN)?, TIME_FORMAT).ok()?;
    let mut section = None;
    for e in std::fs::read_dir(entry).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let is_dir = crate::store::entry_is_dir(&e);
        if !is_dir && crate::store::is_md(&e.path()) {
            let stem = crate::store::file_stem(&e.path());
            return Some(DeletedItem {
                entry: entry.to_path_buf(),
                name: stem,
                file: name,
                kind: ItemKind::Page,
                original_folder: folder.to_path_buf(),
                deleted_at,
            });
        }
        if is_dir && name != VERSIONS_IN_ENTRY {
            section = Some(name);
        }
    }
    section.map(|name| DeletedItem {
        entry: entry.to_path_buf(),
        file: name.clone(),
        name,
        kind: ItemKind::Section,
        original_folder: folder.to_path_buf(),
        deleted_at,
    })
}

/// Everything deleted from `top` and the folders below it, newest first. Does not look inside
/// `.history` folders, so deletions made inside an already deleted section are not listed.
pub fn list_deleted(top: &Path) -> Vec<DeletedItem> {
    let mut out = Vec::new();
    let mut stack = vec![top.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(deleted_root(&dir)) {
            for e in rd.flatten() {
                if let Some(item) = read_entry(&dir, &e.path()) {
                    out.push(item);
                }
            }
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if crate::store::entry_is_dir(&e) && !crate::store::is_hidden_folder(&e.file_name().to_string_lossy()) {
                stack.push(e.path());
            }
        }
    }
    out.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at).then_with(|| a.name.cmp(&b.name)));
    out
}

/// The deleted item kept in a `<folder>\.history\deleted\<time>` folder, if that is what `entry` is.
pub fn deleted_item_at(entry: &Path) -> Option<DeletedItem> {
    let deleted = entry.parent()?;
    let history = deleted.parent()?;
    if deleted.file_name()? != "deleted" || history.file_name()? != HISTORY_DIR {
        return None;
    }
    read_entry(history.parent()?, entry)
}

/// A name based on `stem` for which neither `<name>.md` nor a folder `<name>` exists in `folder`.
fn free_name(folder: &Path, stem: &str, page: bool) -> String {
    let taken = |n: &str| folder.join(n).exists() || (page && folder.join(format!("{n}.md")).exists());
    let mut name = stem.to_string();
    let mut n = 2;
    while taken(&name) {
        name = format!("{stem} ({n})");
        n += 1;
    }
    name
}

/// Moves the files of `from` into `to`, giving a numbered name to a file that is already there.
fn merge_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("history: could not create {}: {e}", to.display()))?;
    for e in std::fs::read_dir(from).map_err(|e| format!("history: could not read {}: {e}", from.display()))?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // A number goes before the extension so that the file is still recognised as a version.
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => name.split_at(i),
            _ => (name.as_str(), ""),
        };
        let mut target = to.join(&name);
        let mut n = 2;
        while target.exists() {
            target = to.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        move_to(&e.path(), &target)?;
    }
    let _ = std::fs::remove_dir(from);
    Ok(())
}

/// Puts a deleted item back into the folder it came from (created if it is gone). A name that is
/// taken gets a number. Returns the path of the restored page or folder.
pub fn restore_deleted(item: &DeletedItem) -> Result<PathBuf, String> {
    let folder = &item.original_folder;
    std::fs::create_dir_all(folder)
        .map_err(|e| format!("history::restore_deleted: could not create {}: {e}", folder.display()))?;
    let restored = match item.kind {
        ItemKind::Section => {
            let name = free_name(folder, &item.name, false);
            let target = folder.join(&name);
            move_to(&item.entry.join(&item.name), &target)?;
            target
        }
        ItemKind::Page => {
            let name = free_name(folder, &item.name, true);
            let ext = Path::new(&item.file).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let target = folder.join(format!("{name}{ext}"));
            let mut moves = vec![(item.entry.join(&item.file), target.clone())];
            let sub = item.entry.join(&item.name);
            if sub.is_dir() {
                moves.push((sub, folder.join(&name)));
            }
            // Versions are moved in the same all-or-nothing step when the page has none yet; otherwise
            // they are merged afterwards (a failure there leaves the entry for another try).
            let versions = item.entry.join(VERSIONS_IN_ENTRY);
            let new_versions = versions_dir(&target);
            let mut merge = false;
            if versions.is_dir() {
                if new_versions.exists() {
                    merge = true;
                } else {
                    if let Some(parent) = new_versions.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            format!("history::restore_deleted: could not create {}: {e}", parent.display())
                        })?;
                    }
                    moves.push((versions.clone(), new_versions.clone()));
                }
            }
            move_all(&moves, &item.entry)?;
            if merge {
                merge_dir(&versions, &new_versions)?;
            }
            target
        }
    };
    let _ = std::fs::remove_dir(&item.entry);
    Ok(restored)
}

/// Removes a deleted item from the history for good.
pub fn delete_forever(item: &DeletedItem) -> Result<(), String> {
    std::fs::remove_dir_all(&item.entry)
        .map_err(|e| format!("history::delete_forever: could not remove {}: {e}", item.entry.display()))
}

/// Removes every `.history` folder under `top`. Returns how many were removed.
pub fn purge_all(top: &Path) -> Result<usize, String> {
    let mut count = 0;
    let mut stack = vec![top.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let history = dir.join(HISTORY_DIR);
        if history.exists() {
            std::fs::remove_dir_all(&history)
                .map_err(|e| format!("history::purge_all: could not remove {}: {e}", history.display()))?;
            count += 1;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if crate::store::entry_is_dir(&e) && !crate::store::is_hidden_folder(&e.file_name().to_string_lossy()) {
                stack.push(e.path());
            }
        }
    }
    Ok(count)
}

#[derive(Clone, Debug, PartialEq)]
pub enum HitKind {
    /// An old version of `page`, which still exists.
    Version { page: PathBuf },
    /// A page (or a version of one) in the `deleted\<time>` folder `entry`.
    Deleted { entry: PathBuf },
}

#[derive(Clone, Debug)]
pub struct HistoryHit {
    /// The file that matched (a version file or a page inside a deleted entry).
    pub file: PathBuf,
    pub kind: HitKind,
    pub title: String,
    pub place: String,
    pub snippet: String,
    pub count: usize,
    pub when: NaiveDateTime,
    /// How many other (older) versions of the same page match too.
    pub older_matches: usize,
}

/// What the search needs to know about a page whose old texts are looked through.
struct Group {
    kind: HitKind,
    /// Title to show when the text has none.
    name: String,
    /// The folder the page lived in, for `place`.
    folder: PathBuf,
    /// Candidate files, newest first, with the time they stand for.
    files: Vec<(PathBuf, NaiveDateTime)>,
}

/// Where a deleted entry came from, for the pages found inside it.
struct EntryCtx {
    entry: PathBuf,
    original_folder: PathBuf,
    deleted_at: NaiveDateTime,
}

fn stamp_of(name: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(name.get(..TIME_LEN)?, TIME_FORMAT).ok()
}

/// The version files (`<time>.md`) in a folder, in no particular order.
fn version_files(dir: &Path) -> Vec<(PathBuf, NaiveDateTime)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            Some((e.path(), stamp_of(name.strip_suffix(".md")?)?))
        })
        .collect()
}

fn newest_first(files: &mut [(PathBuf, NaiveDateTime)]) {
    files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
}

/// Collects the pages with old texts below `dir`. Inside a deleted entry (`ctx`) every page counts,
/// with its versions; outside, only the versions kept in `.history` folders and the deleted entries.
fn collect_groups(dir: &Path, ctx: Option<&EntryCtx>, cancel: &AtomicBool, out: &mut Vec<Group>) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if crate::store::entry_is_dir(&e) {
            if name == HISTORY_DIR {
                if ctx.is_none() {
                    collect_history(dir, &path, cancel, out);
                }
            } else if !crate::store::is_hidden_folder(&name) {
                collect_groups(&path, ctx, cancel, out);
            }
        } else if let Some(c) = ctx
            && crate::store::is_md(&path)
        {
            let stem = crate::store::file_stem(&path);
            let mut files = vec![(path.clone(), c.deleted_at)];
            files.extend(version_files(&dir.join(HISTORY_DIR).join("pages").join(&stem)));
            files.extend(version_files(&dir.join(VERSIONS_IN_ENTRY)));
            newest_first(&mut files);
            let rel = dir.strip_prefix(&c.entry).unwrap_or(Path::new(""));
            out.push(Group {
                kind: HitKind::Deleted { entry: c.entry.clone() },
                name: stem,
                folder: c.original_folder.join(rel),
                files,
            });
        }
    }
}

/// The groups kept in one `.history` folder: versions of pages and deleted entries.
fn collect_history(folder: &Path, history: &Path, cancel: &AtomicBool, out: &mut Vec<Group>) {
    if let Ok(rd) = std::fs::read_dir(history.join("pages")) {
        for e in rd.flatten() {
            let stem = e.file_name().to_string_lossy().to_string();
            let mut files = version_files(&e.path());
            if files.is_empty() {
                continue;
            }
            newest_first(&mut files);
            out.push(Group {
                kind: HitKind::Version { page: folder.join(format!("{stem}.md")) },
                name: stem,
                folder: folder.to_path_buf(),
                files,
            });
        }
    }
    if let Ok(rd) = std::fs::read_dir(deleted_root(folder)) {
        for e in rd.flatten() {
            let Some(deleted_at) = stamp_of(&e.file_name().to_string_lossy()) else { continue };
            let ctx = EntryCtx { entry: e.path(), original_folder: folder.to_path_buf(), deleted_at };
            collect_groups(&e.path(), Some(&ctx), cancel, out);
        }
    }
}

/// "Top-level folder › Section › ..." like the normal search shows it.
pub fn place_of(roots: &[PathBuf], folder: &Path) -> String {
    let root = roots.iter().find(|r| folder.starts_with(r));
    let base = root.map(|r| r.parent().unwrap_or(r.as_path()));
    let rel = base.and_then(|b| folder.strip_prefix(b).ok()).unwrap_or(Path::new(""));
    rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join(" › ")
}

/// The newest matching text of a page, with a count of the older ones that match too.
fn match_group(g: &Group, roots: &[PathBuf], words: &[Vec<char>], cancel: &AtomicBool) -> Option<HistoryHit> {
    let mut hit: Option<HistoryHit> = None;
    let mut older = 0;
    for (file, when) in &g.files {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let Ok(doc) = crate::store::read_doc(file) else { continue };
        let title = match doc.get("title") {
            Some(t) if !t.trim().is_empty() => doc.title_or(file),
            _ => g.name.clone(),
        };
        let text = format!("{title}\n{}", crate::render::plain_text(&doc.body));
        let Some((count, snippet)) = crate::store::match_text(&text, words) else { continue };
        if hit.is_none() {
            hit = Some(HistoryHit {
                file: file.clone(),
                kind: g.kind.clone(),
                title,
                place: place_of(roots, &g.folder),
                snippet,
                count,
                when: *when,
                older_matches: 0,
            });
        } else {
            older += 1;
        }
    }
    hit.map(|h| HistoryHit { older_matches: older, ..h })
}

/// Looks for `query` (all words) in the old versions and deleted pages below `roots` and calls
/// `emit` for each page that matches, with its newest matching text. Reads on several threads;
/// stops early when `cancel` is set. Files that vanish meanwhile are skipped.
pub fn search_history(roots: &[PathBuf], query: &str, cancel: &AtomicBool, emit: &(dyn Fn(HistoryHit) + Sync)) {
    let words: Vec<Vec<char>> = query.split_whitespace().map(crate::store::fold).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return;
    }
    let mut groups = Vec::new();
    for root in roots {
        collect_groups(root, None, cancel, &mut groups);
    }
    let next = AtomicUsize::new(0);
    let work = || {
        while !cancel.load(Ordering::Relaxed) {
            let Some(g) = groups.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
            if let Some(hit) = match_group(g, roots, &words, cancel) {
                emit(hit);
            }
        }
    };
    std::thread::scope(|s| {
        for _ in 0..SEARCH_THREADS.min(groups.len().saturating_sub(1)) {
            s.spawn(work);
        }
        work();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store;
    use chrono::{Duration, NaiveDate};
    use std::fs;

    fn at(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 4).unwrap().and_hms_opt(h, m, 0).unwrap()
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("notecraft-hist-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn page_with(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, format!("---\ntitle: T\nupdated: 2026-01-01T00:00:00\n---\n{body}")).unwrap();
        p
    }

    /// What a save does to the file on disk, without going through `store::save_body`.
    fn set_body(p: &Path, body: &str) {
        fs::write(p, format!("---
title: T
updated: 2026-01-02T00:00:00
---
{body}")).unwrap();
    }

    #[test]
    fn snapshot_rules() {
        let d = temp("rules");
        let p = page_with(&d, "A.md", "\nfirst text\n");
        // No version yet: a changed body makes one.
        assert!(snapshot_before_save(&p, "\nsecond\n", at(10, 0)).unwrap().is_some());
        assert_eq!(list_versions(&p).len(), 1);
        // The save itself happened: the page now holds the second text.
        set_body(&p, "\nsecond\n");
        // The body is unchanged: none.
        assert!(snapshot_before_save(&p, "\nsecond\n", at(10, 30)).unwrap().is_none());
        // A small edit 5 minutes after the last version: none.
        assert!(snapshot_before_save(&p, "\nsecond!\n", at(10, 5)).unwrap().is_none());
        // 11 minutes after it: one.
        assert!(snapshot_before_save(&p, "\nsecond!\n", at(10, 11)).unwrap().is_some());
        assert_eq!(list_versions(&p).len(), 2);
        // A page that does not exist: none.
        assert!(snapshot_before_save(&d.join("Missing.md"), "x", at(11, 0)).unwrap().is_none());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn big_deletions_are_always_kept() {
        let d = temp("big");
        let long = "x".repeat(1000);
        let p = page_with(&d, "A.md", &long);
        assert!(snapshot_before_save(&p, "y", at(10, 0)).unwrap().is_some());
        // 5 minutes later, 201 characters shorter: kept.
        fs::write(&p, format!("---\ntitle: T\n---\n{}", "z".repeat(1000))).unwrap();
        assert!(snapshot_before_save(&p, &"z".repeat(799), at(10, 5)).unwrap().is_some());
        // 100 characters long, more than 20 % shorter: kept; 15 % shorter: not.
        let q = page_with(&d, "B.md", &"q".repeat(100));
        assert!(snapshot_before_save(&q, "first", at(9, 0)).unwrap().is_some());
        fs::write(&q, format!("---\ntitle: T\n---\n{}", "r".repeat(100))).unwrap();
        assert!(snapshot_before_save(&q, &"r".repeat(85), at(9, 2)).unwrap().is_none());
        assert!(snapshot_before_save(&q, &"r".repeat(79), at(9, 3)).unwrap().is_some());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn list_is_newest_first() {
        let d = temp("order");
        let p = page_with(&d, "A.md", "one");
        snapshot_now(&p, at(8, 0)).unwrap();
        snapshot_now(&p, at(9, 0) + Duration::seconds(5)).unwrap();
        snapshot_now(&p, at(7, 0)).unwrap();
        let v = list_versions(&p);
        assert_eq!(v.iter().map(|x| x.when).collect::<Vec<_>>(), vec![at(9, 0) + Duration::seconds(5), at(8, 0), at(7, 0)]);
        assert!(v[0].size > 0);
        // A stray file in the folder is ignored.
        fs::write(versions_dir(&p).join("notes.txt"), "x").unwrap();
        assert_eq!(list_versions(&p).len(), 3);
        // Two snapshots in the same second do not overwrite each other.
        snapshot_now(&p, at(8, 0)).unwrap();
        assert_eq!(list_versions(&p).len(), 4);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unicode_and_dotted_names() {
        let d = temp("names");
        let p = page_with(&d, "01.03. četrtek.md", "\ntext\n");
        snapshot_now(&p, at(8, 0)).unwrap();
        assert_eq!(versions_dir(&p), d.join(HISTORY_DIR).join("pages").join("01.03. četrtek"));
        assert_eq!(list_versions(&p).len(), 1);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn blocked_history_dir_reports_an_error() {
        let d = temp("blocked");
        fs::write(d.join(HISTORY_DIR), "in the way").unwrap();
        let p = page_with(&d, "A.md", "text");
        let e = snapshot_before_save(&p, "other", at(8, 0)).unwrap_err();
        assert!(e.contains("history"), "{e}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn restore_keeps_title_and_crlf() {
        let d = temp("restore");
        let p = d.join("A.md");
        fs::write(&p, "---\r\ntitle: T\r\n---\r\nold body\r\n").unwrap();
        let old = snapshot_now(&p, at(8, 0)).unwrap();
        fs::write(&p, "---\r\ntitle: T2\r\n---\r\nnew body\r\n").unwrap();
        restore_version(&p, &old).unwrap();
        let doc = store::read_doc(&p).unwrap();
        assert_eq!(doc.get("title"), Some("T2"));
        assert_eq!(doc.body, "old body\n");
        assert!(doc.crlf);
        // The text from before the restore is itself in the list.
        let texts: Vec<String> = list_versions(&p).iter().map(|v| store::read_doc(&v.path).unwrap().body).collect();
        assert!(texts.iter().any(|t| t == "new body\n"), "{texts:?}");

        // A page without front matter gets none.
        let q = d.join("B.md");
        fs::write(&q, "plain old\n").unwrap();
        let v = snapshot_now(&q, at(8, 0)).unwrap();
        fs::write(&q, "plain new\n").unwrap();
        restore_version(&q, &v).unwrap();
        assert_eq!(fs::read_to_string(&q).unwrap(), "plain old\n");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn purge_removes_versions() {
        let d = temp("purge");
        let p = page_with(&d, "A.md", "one");
        snapshot_now(&p, at(8, 0)).unwrap();
        purge_versions(&p).unwrap();
        assert!(list_versions(&p).is_empty());
        purge_versions(&p).unwrap(); // nothing to purge is fine
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn delete_and_restore_page() {
        let top = temp("delrest");
        let d = top.join("S").join("T");
        fs::create_dir_all(d.join("A")).unwrap();
        let p = page_with(&d, "A.md", "\nbody\n");
        page_with(&d.join("A"), "Sub.md", "\nsub\n");
        snapshot_now(&p, at(8, 0)).unwrap();
        let before = fs::read(&p).unwrap();
        delete_page(&p, at(9, 0)).unwrap();
        assert!(!p.exists() && !d.join("A").exists());
        assert!(list_versions(&p).is_empty());
        let items = list_deleted(&top);
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].kind, items[0].name.as_str(), items[0].deleted_at), (ItemKind::Page, "A", at(9, 0)));
        assert_eq!(items[0].original_folder, d);
        let back = restore_deleted(&items[0]).unwrap();
        assert_eq!(back, p);
        assert_eq!(fs::read(&p).unwrap(), before);
        assert!(d.join("A").join("Sub.md").is_file());
        assert_eq!(list_versions(&p).len(), 1);
        assert!(list_deleted(&top).is_empty());
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn same_second_deletes_do_not_collide() {
        let top = temp("same");
        let a = page_with(&top, "A.md", "a");
        let b = page_with(&top, "B.md", "b");
        let e1 = delete_page(&a, at(9, 0)).unwrap();
        let e2 = delete_page(&b, at(9, 0)).unwrap();
        assert_ne!(e1, e2);
        let names: Vec<String> = list_deleted(&top).into_iter().map(|i| i.name).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"A".to_string()) && names.contains(&"B".to_string()));
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn restore_with_taken_name_is_numbered() {
        let top = temp("taken");
        let p = page_with(&top, "A.md", "old");
        fs::create_dir_all(top.join("A")).unwrap();
        delete_page(&p, at(9, 0)).unwrap();
        let newer = page_with(&top, "A.md", "newer");
        fs::create_dir_all(top.join("A")).unwrap();
        let back = restore_deleted(&list_deleted(&top)[0]).unwrap();
        assert_eq!(back, top.join("A (2).md"));
        assert!(fs::read_to_string(&newer).unwrap().contains("newer"));
        assert!(fs::read_to_string(&back).unwrap().contains("old"));
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn deleted_section_keeps_pages_and_their_versions() {
        let top = temp("section");
        let s = top.join("Sec");
        fs::create_dir_all(&s).unwrap();
        let p = page_with(&s, "P.md", "text");
        snapshot_now(&p, at(8, 0)).unwrap();
        delete_folder(&s, at(9, 0)).unwrap();
        assert!(!s.exists());
        let items = list_deleted(&top);
        assert_eq!((items[0].kind, items[0].name.as_str()), (ItemKind::Section, "Sec"));
        assert_eq!(items[0].original_folder, top);
        assert_eq!(restore_deleted(&items[0]).unwrap(), s);
        assert!(p.is_file());
        assert_eq!(list_versions(&p).len(), 1);
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn list_does_not_descend_into_history() {
        let top = temp("nested");
        let s = top.join("Sec");
        fs::create_dir_all(&s).unwrap();
        let p = page_with(&s, "Old.md", "x");
        delete_page(&p, at(8, 0)).unwrap();
        delete_folder(&s, at(9, 0)).unwrap();
        let items = list_deleted(&top);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Sec");
        let _ = fs::remove_dir_all(&top);
    }

    #[cfg(windows)]
    #[test]
    fn failed_second_move_rolls_back() {
        let top = temp("rollback");
        fs::create_dir_all(top.join("A")).unwrap();
        let p = page_with(&top, "A.md", "text");
        let sub = page_with(&top.join("A"), "Sub.md", "sub");
        let _open = fs::File::open(&sub).unwrap(); // an open file keeps Windows from moving the folder
        let e = delete_page(&p, at(9, 0)).unwrap_err();
        assert!(e.contains("history"), "{e}");
        assert!(p.is_file() && sub.is_file());
        assert!(list_deleted(&top).is_empty());
        assert!(!top.join(HISTORY_DIR).join("deleted").join("2026-11-04T090000").exists());
        drop(_open);
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn delete_forever_and_purge_all() {
        let top = temp("purgeall");
        let a = page_with(&top, "A.md", "a");
        let sec = top.join("Sec");
        fs::create_dir_all(&sec).unwrap();
        let b = page_with(&sec, "B.md", "b");
        delete_page(&a, at(9, 0)).unwrap();
        snapshot_now(&b, at(9, 0)).unwrap();
        let item = list_deleted(&top).remove(0);
        delete_forever(&item).unwrap();
        assert!(list_deleted(&top).is_empty());
        delete_page(&page_with(&top, "C.md", "c"), at(9, 1)).unwrap();
        assert_eq!(purge_all(&top).unwrap(), 2); // top's and Sec's
        assert!(!top.join(HISTORY_DIR).exists() && !sec.join(HISTORY_DIR).exists());
        let _ = fs::remove_dir_all(&top);
    }

    fn collect(roots: &[PathBuf], q: &str, cancel: bool) -> Vec<HistoryHit> {
        let hits = std::sync::Mutex::new(Vec::new());
        let flag = std::sync::atomic::AtomicBool::new(cancel);
        search_history(roots, q, &flag, &|h| hits.lock().unwrap().push(h));
        let mut v = hits.into_inner().unwrap();
        v.sort_by(|a, b| a.title.cmp(&b.title));
        v
    }

    fn titled(dir: &Path, name: &str, title: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, format!("---\ntitle: {title}\nupdated: 2026-01-01T00:00:00\n---\n{body}")).unwrap();
        p
    }

    #[test]
    fn finds_old_version_and_deleted_page() {
        let top = temp("sh-basic");
        let p = titled(&top, "Live.md", "Live", "\nnothing here\n");
        let old = versions_dir(&p);
        fs::create_dir_all(&old).unwrap();
        titled(&old, "2026-11-01T100000.md", "Live", "\nthe zebra was here\n");
        let d = titled(&top, "Gone.md", "Gone", "\nanother zebra\n");
        delete_page(&d, at(9, 0)).unwrap();
        let hits = collect(&[top.clone()], "ZEBRA", false);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Gone");
        assert!(matches!(hits[0].kind, HitKind::Deleted { .. }));
        assert_eq!(hits[1].title, "Live");
        assert_eq!(hits[1].kind, HitKind::Version { page: p.clone() });
        assert_eq!(hits[1].when, NaiveDate::from_ymd_opt(2026, 11, 1).unwrap().and_hms_opt(10, 0, 0).unwrap());
        assert!(hits[1].snippet.contains("zebra"));
        assert_eq!(hits[1].place, top.file_name().unwrap().to_string_lossy());
        assert!(collect(&[top.clone()], "unicorn", false).is_empty());
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn newest_match_only_with_count() {
        let top = temp("sh-count");
        let p = titled(&top, "P.md", "P", "\nnone\n");
        let vd = versions_dir(&p);
        fs::create_dir_all(&vd).unwrap();
        for t in ["2026-11-01T100000", "2026-11-02T100000", "2026-11-03T100000"] {
            titled(&vd, &format!("{t}.md"), "P", "\nmoose\n");
        }
        titled(&vd, "2026-11-04T100000.md", "P", "\nelk\n");
        let hits = collect(&[top.clone()], "moose", false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].older_matches, 2);
        assert!(hits[0].file.to_string_lossy().contains("2026-11-03T100000"));
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn finds_page_inside_deleted_section() {
        let top = temp("sh-section");
        let s = top.join("Sec");
        fs::create_dir_all(&s).unwrap();
        let p = titled(&s, "Inner.md", "Inner", "\nwalrus\n");
        snapshot_now(&p, at(8, 0)).unwrap();
        delete_folder(&s, at(9, 0)).unwrap();
        let hits = collect(&[top.clone()], "walrus", false);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].title, "Inner");
        assert!(matches!(hits[0].kind, HitKind::Deleted { .. }));
        assert_eq!(hits[0].older_matches, 1);
        assert!(hits[0].place.ends_with("Sec"), "{}", hits[0].place);
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn cancel_stops_early() {
        let top = temp("sh-cancel");
        let p = titled(&top, "P.md", "P", "\nx\n");
        let vd = versions_dir(&p);
        fs::create_dir_all(&vd).unwrap();
        titled(&vd, "2026-11-01T100000.md", "P", "\nmoose\n");
        assert!(collect(&[top.clone()], "moose", true).is_empty());
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn vanishing_files_are_skipped() {
        let top = temp("sh-vanish");
        let p = titled(&top, "P.md", "P", "\nx\n");
        let vd = versions_dir(&p);
        fs::create_dir_all(&vd).unwrap();
        let a = titled(&vd, "2026-11-01T100000.md", "P", "\nmoose\n");
        let b = titled(&vd, "2026-11-02T100000.md", "P", "\nmoose\n");
        let flag = std::sync::atomic::AtomicBool::new(false);
        let n = std::sync::atomic::AtomicUsize::new(0);
        search_history(&[top.clone()], "moose", &flag, &|_| {
            n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = fs::remove_file(&a);
            let _ = fs::remove_file(&b);
        });
        assert!(n.load(std::sync::atomic::Ordering::Relaxed) <= 1);
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn deleted_item_at_finds_the_entry() {
        let top = temp("item-at");
        let p = page_with(&top, "P.md", "text");
        let entry = delete_page(&p, at(9, 0)).unwrap();
        let item = deleted_item_at(&entry).unwrap();
        assert_eq!((item.name.as_str(), item.kind, item.original_folder.as_path()), ("P", ItemKind::Page, top.as_path()));
        assert_eq!(item.deleted_at, at(9, 0));
        assert!(deleted_item_at(&top).is_none());
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn case_only_rename_keeps_versions_visible() {
        let top = temp("case-rename");
        let old = page_with(&top, "notes.md", "one");
        snapshot_now(&old, at(9, 0)).unwrap();
        let new = top.join("Notes.md");
        fs::rename(&old, &new).unwrap();
        rename_versions(&old, &new).unwrap();
        assert_eq!(list_versions(&new).len(), 1);
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn merge_collision_keeps_files_visible() {
        let top = temp("merge-collide");
        let a = page_with(&top, "A.md", "one");
        let b = page_with(&top, "B.md", "two");
        snapshot_now(&a, at(9, 0)).unwrap();
        snapshot_now(&b, at(9, 0)).unwrap();
        fs::remove_file(&b).unwrap();
        rename_versions(&b, &a).unwrap();
        assert_eq!(list_versions(&a).len(), 2);
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn deleted_markdown_page_can_be_restored() {
        let top = temp("del-markdown");
        let p = page_with(&top, "X.markdown", "text");
        let entry = delete_page(&p, at(9, 0)).unwrap();
        let item = deleted_item_at(&entry).unwrap();
        assert_eq!((item.name.as_str(), item.kind), ("X", ItemKind::Page));
        let back = restore_deleted(&item).unwrap();
        assert_eq!(back, p);
        assert!(p.is_file());
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn failed_restore_is_rolled_back() {
        let top = temp("restore-rollback");
        let p = page_with(&top, "P.md", "text");
        snapshot_now(&p, at(9, 0)).unwrap();
        let entry = delete_page(&p, at(9, 5)).unwrap();
        let item = deleted_item_at(&entry).unwrap();
        // `.history\pages` is a plain file now, so the versions cannot be put back.
        let pages = top.join(HISTORY_DIR).join("pages");
        let _ = fs::remove_dir_all(&pages);
        fs::write(&pages, "x").unwrap();
        assert!(restore_deleted(&item).is_err());
        assert!(!p.exists(), "the page must not stay half restored");
        assert_eq!(deleted_item_at(&entry).map(|d| d.kind), Some(ItemKind::Page));
        let _ = fs::remove_dir_all(&top);
    }
}
