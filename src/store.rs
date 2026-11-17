//! Reading and writing the notes: folders are notebooks/sections, `.md` files are pages.
//! A folder named like a page (`Page.md` + `Page/`) holds that page's subpages.

use crate::op_error::{Kind, OpError};
use std::fs;
use std::path::{Path, PathBuf};

/// Folder names that never show up as sections.
const HIDDEN_FOLDERS: &[&str] = &["resources", "_resources", "attachments", ".git", ".obsidian"];

#[derive(Clone, Debug)]
pub struct Folder {
    pub path: PathBuf,
    pub name: String,
    pub children: Vec<Folder>,
    /// Pages directly inside this folder (not counting subpages).
    pub page_count: usize,
}

#[derive(Clone, Debug)]
pub struct PageEntry {
    pub path: PathBuf,
    pub title: String,
    /// 0 = page, 1 = subpage, ...
    pub level: usize,
    pub created: String,
    pub updated: String,
}

/// A page split into its front matter and its Markdown body.
#[derive(Clone, Debug, Default)]
pub struct PageDoc {
    pub meta: Vec<(String, String)>,
    /// The body always uses `\n` line ends here; `crlf` remembers how the file had them.
    pub body: String,
    pub crlf: bool,
}

impl PageDoc {
    pub fn parse(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let crlf = text.contains("\r\n");
        let mut doc = Self::parse_raw(text);
        doc.crlf = crlf;
        if crlf {
            doc.body = doc.body.replace("\r\n", "\n");
        }
        doc
    }

    fn parse_raw(text: &str) -> Self {
        let first_end = text.find('\n');
        if let Some(fe) = first_end
            && text[..fe].trim_end() == "---"
        {
            let rest = &text[fe + 1..];
            let mut pos = 0;
            let mut meta = Vec::new();
            for line in rest.split_inclusive('\n') {
                pos += line.len();
                let l = line.trim_end();
                if l == "---" {
                    return PageDoc { meta, body: rest[pos..].to_string(), crlf: false };
                }
                if let Some((k, v)) = l.split_once(':') {
                    meta.push((k.trim().to_string(), v.trim().to_string()));
                } else if !l.is_empty() {
                    // Not simple front matter: keep the line in the value of the previous key.
                    if let Some(last) = meta.last_mut() {
                        last.1.push('\n');
                        last.1.push_str(l);
                    }
                }
            }
        }
        PageDoc { meta: Vec::new(), body: text.to_string(), crlf: false }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.meta.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, key: &str, value: &str) {
        if let Some(entry) = self.meta.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            entry.1 = value.to_string();
        } else {
            self.meta.push((key.to_string(), value.to_string()));
        }
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        if !self.meta.is_empty() {
            s.push_str("---\n");
            for (k, v) in &self.meta {
                s.push_str(k);
                s.push_str(": ");
                s.push_str(v);
                s.push('\n');
            }
            s.push_str("---\n");
        }
        s.push_str(&self.body.replace("\r\n", "\n"));
        if self.crlf { s.replace('\n', "\r\n") } else { s }
    }

    pub fn title_or(&self, path: &Path) -> String {
        match self.get("title") {
            Some(t) if !t.trim().is_empty() => unquote(t.trim()),
            _ => file_stem(path),
        }
    }
}

fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 2 && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

pub fn file_stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// The extension of a page file, without the dot (`md` when it has none).
pub fn page_ext(path: &Path) -> String {
    path.extension().map_or_else(|| "md".to_string(), |e| e.to_string_lossy().to_string())
}

pub fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Natural, case-insensitive ordering ("Page 2" before "Page 10").
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                    nb.push(c);
                    bi.next();
                }
                let ord = na.trim_start_matches('0').len().cmp(&nb.trim_start_matches('0').len())
                    .then_with(|| na.trim_start_matches('0').cmp(nb.trim_start_matches('0')));
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let (fx, fy) = (fold_char(x), fold_char(y));
                if fx != fy {
                    return fx.cmp(&fy);
                }
                ai.next();
                bi.next();
            }
        }
    }
}

pub(crate) fn is_md(path: &Path) -> bool {
    path.extension().map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown")).unwrap_or(false)
}

pub(crate) fn is_hidden_folder(name: &str) -> bool {
    name.starts_with('.') || HIDDEN_FOLDERS.iter().any(|h| h.eq_ignore_ascii_case(name))
}

/// Whether a directory entry is a folder. The listing already says so; asking the file system again
/// costs a network round trip per file on a shared folder. Links are followed the slow way.
pub fn entry_is_dir(e: &fs::DirEntry) -> bool {
    match e.file_type() {
        Ok(t) if !t.is_symlink() => t.is_dir(),
        _ => e.path().is_dir(),
    }
}

/// Lower-case names of the `.md` pages in one listing; folder `X` next to `X.md` holds subpages of that page.
fn page_stems(entries: &[(fs::DirEntry, bool)]) -> std::collections::HashSet<String> {
    entries
        .iter()
        .filter(|(e, dir)| !dir && is_md(&e.path()))
        .filter_map(|(e, _)| e.path().file_stem().map(|s| s.to_string_lossy().to_lowercase()))
        .collect()
}

fn list_entries(dir: &Path) -> Vec<(fs::DirEntry, bool)> {
    match fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| {
            let d = entry_is_dir(&e);
            (e, d)
        }).collect(),
        Err(_) => Vec::new(),
    }
}

pub fn scan_tree(root: &Path) -> Folder {
    let mut f = scan_folder(root);
    f.name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| root.display().to_string());
    f
}

fn scan_folder(dir: &Path) -> Folder {
    let mut children = Vec::new();
    let mut page_count = 0;
    let entries = list_entries(dir);
    let stems = page_stems(&entries);
    for (e, is_dir) in &entries {
        let p = e.path();
        if *is_dir {
            let name = e.file_name().to_string_lossy().to_string();
            if is_hidden_folder(&name) || stems.contains(&name.to_lowercase()) {
                continue;
            }
            children.push(scan_folder(&p));
        } else if is_md(&p) {
            page_count += 1;
        }
    }
    children.sort_by(|a: &Folder, b: &Folder| natural_cmp(&a.name, &b.name));
    Folder {
        path: dir.to_path_buf(),
        name: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        children,
        page_count,
    }
}

/// Reads only the front matter of a page (cheap enough for page lists).
pub fn read_doc(path: &Path) -> std::io::Result<PageDoc> {
    let bytes = fs::read(path)?;
    let doc = PageDoc::parse(&String::from_utf8_lossy(&bytes));
    Ok(doc)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortOrder {
    Name,
    Created,
    Updated,
}

/// Size and modification time of a file, as the directory listing reports them.
type Stamp = Option<(u64, std::time::SystemTime)>;

/// What a page list needs from a page.
#[derive(Clone)]
struct PageHead {
    title: String,
    created: String,
    updated: String,
}

/// Page heads already read, keyed by path; an entry is valid while the file's size and time are unchanged.
static HEAD_CACHE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<PathBuf, (Stamp, PageHead)>>> =
    std::sync::LazyLock::new(Default::default);

const READ_THREADS: usize = 8;

fn read_head(p: &Path) -> PageHead {
    let doc = read_doc(p).unwrap_or_default();
    PageHead {
        title: doc.title_or(p),
        created: doc.get("created").unwrap_or("").to_string(),
        updated: doc.get("updated").unwrap_or("").to_string(),
    }
}

/// Heads of the given files, in the same order. Files unchanged since the last call come from the cache;
/// the rest are read on several threads, because on a network drive each read is mostly waiting.
fn load_heads(files: Vec<(PathBuf, Stamp)>) -> Vec<(PathBuf, PageHead)> {
    let mut heads: Vec<Option<PageHead>> = vec![None; files.len()];
    {
        let cache = HEAD_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        for (i, (p, stamp)) in files.iter().enumerate() {
            if stamp.is_some()
                && let Some((s, h)) = cache.get(p)
                && s == stamp
            {
                heads[i] = Some(h.clone());
            }
        }
    }
    let misses: Vec<usize> = (0..files.len()).filter(|&i| heads[i].is_none()).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let read: Vec<(usize, PageHead)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..READ_THREADS.min(misses.len().saturating_sub(1)))
            .map(|_| s.spawn(|| read_misses(&files, &misses, &next)))
            .collect();
        let mut all = read_misses(&files, &misses, &next);
        for w in workers {
            all.extend(w.join().unwrap_or_default());
        }
        all
    });
    {
        let mut cache = HEAD_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        for (i, h) in read {
            if files[i].1.is_some() {
                cache.insert(files[i].0.clone(), (files[i].1, h.clone()));
            }
            heads[i] = Some(h);
        }
    }
    files.into_iter().zip(heads).map(|((p, _), h)| (p, h.unwrap_or_else(|| PageHead { title: String::new(), created: String::new(), updated: String::new() }))).collect()
}

/// One worker of `load_heads`: takes the next unread miss until none are left.
fn read_misses(files: &[(PathBuf, Stamp)], misses: &[usize], next: &std::sync::atomic::AtomicUsize) -> Vec<(usize, PageHead)> {
    let mut done = Vec::new();
    loop {
        let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some(&i) = misses.get(k) else { break };
        done.push((i, read_head(&files[i].0)));
    }
    done
}

pub fn list_pages(dir: &Path, order: SortOrder) -> Vec<PageEntry> {
    let mut out = Vec::new();
    list_pages_into(dir, 0, order, &mut out);
    out
}

fn list_pages_into(dir: &Path, level: usize, order: SortOrder, out: &mut Vec<PageEntry>) {
    let entries = list_entries(dir);
    let folders: std::collections::HashSet<String> =
        entries.iter().filter(|(_, d)| *d).map(|(e, _)| e.file_name().to_string_lossy().to_lowercase()).collect();
    let files: Vec<(PathBuf, Stamp)> = entries
        .iter()
        .filter(|(e, d)| !*d && is_md(&e.path()))
        .map(|(e, _)| {
            // On Windows the listing already carries size and time, so this costs no extra round trip.
            let stamp = e.metadata().ok().and_then(|m| m.modified().ok().map(|t| (m.len(), t)));
            (e.path(), stamp)
        })
        .collect();
    let mut pages: Vec<PageEntry> = load_heads(files)
        .into_iter()
        .map(|(path, h)| PageEntry { title: h.title, created: h.created, updated: h.updated, path, level })
        .collect();
    match order {
        SortOrder::Name => pages.sort_by(|a, b| natural_cmp(&a.title, &b.title)),
        SortOrder::Created => pages.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| natural_cmp(&a.title, &b.title))),
        SortOrder::Updated => pages.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| natural_cmp(&a.title, &b.title))),
    }
    for pg in pages {
        let sub = subpage_dir(&pg.path);
        let has_sub = sub.file_name().is_some_and(|n| folders.contains(&n.to_string_lossy().to_lowercase()));
        out.push(pg);
        if has_sub {
            list_pages_into(&sub, level + 1, order, out);
        }
    }
}

/// The folder that holds a page's subpages (it may not exist).
pub fn subpage_dir(page: &Path) -> PathBuf {
    page.with_extension("")
}

/// Turns a title into a safe file name.
pub fn safe_name(title: &str) -> String {
    let mut s: String = title
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '-' } else { c })
        .collect();
    s = s.trim().trim_end_matches('.').trim().to_string();
    if s.is_empty() { "Untitled".into() } else { s }
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let make = |n: usize| {
        let name = if n == 1 { stem.to_string() } else { format!("{stem} ({n})") };
        if ext.is_empty() { dir.join(name) } else { dir.join(format!("{name}.{ext}")) }
    };
    let mut n = 1;
    while make(n).exists() {
        n += 1;
    }
    make(n)
}

pub fn create_page(dir: &Path, title: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("Could not create the folder: {e}"))?;
    let path = unique_path(dir, &safe_name(title), "md");
    let now = now_stamp();
    let doc = PageDoc {
        meta: vec![("title".into(), title.trim().to_string()), ("updated".into(), now.clone()), ("created".into(), now)],
        body: "\n".into(),
        crlf: false,
    };
    fs::write(&path, doc.to_text()).map_err(|e| format!("Could not create the page: {e}"))?;
    Ok(path)
}

/// The last problem with keeping history, for the status bar; set when a save could not keep a version.
static HISTORY_WARNING: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Takes the pending history warning, if any.
pub fn take_history_warning() -> Option<String> {
    HISTORY_WARNING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub fn save_body(path: &Path, body: &str) -> Result<(), String> {
    // A failed history copy must not stop the save; it is reported through `take_history_warning`.
    if let Err(e) = crate::history::snapshot_before_save(path, body, chrono::Local::now().naive_local()) {
        *HISTORY_WARNING.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
    }
    let mut doc = read_doc(path).unwrap_or_default();
    let had_meta = !doc.meta.is_empty();
    doc.body = body.to_string();
    if had_meta {
        doc.set("updated", &now_stamp());
    }
    fs::write(path, doc.to_text()).map_err(|e| format!("Could not save the page: {e}"))
}

/// Keeps text that could not be saved over a page changed elsewhere, as a new page next to it.
pub fn save_conflict_copy(path: &Path, body: &str, computer: &str) -> Result<PathBuf, String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let when = chrono::Local::now().format("%Y-%m-%d %H%M");
    let stem = safe_name(&format!("{} (conflict {computer} {when})", file_stem(path)));
    let copy = unique_path(dir, &stem, "md");
    let mut doc = read_doc(path).unwrap_or_default();
    let title = doc.title_or(path);
    if !doc.meta.is_empty() {
        doc.set("title", &format!("{title} (conflict {computer} {when})"));
        doc.set("updated", &now_stamp());
    }
    doc.body = body.to_string();
    fs::write(&copy, doc.to_text()).map_err(|e| format!("Could not write {}: {e}", copy.display()))?;
    Ok(copy)
}

/// Renames a page: new title in the front matter, new file name, subpages folder follows.
/// With `exact` (the path the page must get) the file becomes exactly that name and extension, and
/// the rename is refused (before anything is written) when another page or folder already has that
/// name; without it the name follows the title and the extension becomes `.md`.
/// All or nothing: the subpages folder is renamed first, then the file, and the title is written
/// last; when a step fails, the earlier ones are turned back (`Partial` when that fails too). A
/// folder that already has the new name is refused, so that the subpages are not mixed with it.
pub fn rename_page(path: &Path, new_title: &str, exact: Option<&Path>) -> Result<PathBuf, OpError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = exact.map(file_stem);
    let ext = exact.map_or_else(|| "md".to_string(), page_ext);
    if let Some(why) = exact.and_then(|e| rename_refusal(path, e)) {
        return Err(OpError::refused(why));
    }
    let mut doc = read_doc(path).map_err(|e| OpError::io(format!("Could not read the page: {e}"), &e))?;
    doc.set("title", new_title.trim());
    doc.set("updated", &now_stamp());
    let wanted = stem.clone().unwrap_or_else(|| safe_name(new_title));
    let same_ext = path.extension().is_some_and(|e| e.eq_ignore_ascii_case(&ext));
    let new_path = if wanted == file_stem(path) && (stem.is_none() || same_ext) {
        path.to_path_buf()
    } else if stem.is_some() || wanted.eq_ignore_ascii_case(&file_stem(path)) {
        dir.join(format!("{wanted}.{}", if stem.is_some() { ext.as_str() } else { "md" }))
    } else {
        unique_path(dir, &wanted, "md")
    };
    let old_sub = subpage_dir(path);
    let new_sub = subpage_dir(&new_path);
    let move_sub = new_sub != old_sub && old_sub.is_dir();
    if move_sub && new_sub.exists() && !same_path(&new_sub, &old_sub) {
        return Err(OpError::refused(format!("\"{}\" already has a folder \"{}\".", dir.display(), file_stem(&new_path))));
    }
    if move_sub {
        fs::rename(&old_sub, &new_sub).map_err(|e| OpError::io(format!("Could not rename the subpages folder: {e}"), &e))?;
    }
    // Turns the earlier steps back after `err`: the file name when `file_moved`, then the subpages folder.
    let turn_back = |err: OpError, file_moved: bool| -> OpError {
        if file_moved && let Err(back) = fs::rename(&new_path, path) {
            let text = format!("{err}\n\nIt could not get its old name back: {back}\nIt is now {}.", new_path.display());
            return OpError::new(Kind::Partial, text);
        }
        if move_sub && let Err(back) = fs::rename(&new_sub, &old_sub) {
            let text = format!("{err}\n\nIts subpages could not get their old folder back: {back}\nThey are now in {}.", new_sub.display());
            return OpError::new(Kind::Partial, text);
        }
        err
    };
    if new_path != path
        && let Err(e) = fs::rename(path, &new_path)
    {
        return Err(turn_back(OpError::io(format!("Could not rename the file: {e}"), &e), false));
    }
    if let Err(e) = fs::write(&new_path, doc.to_text()) {
        return Err(turn_back(OpError::io(format!("Could not save the page: {e}"), &e), new_path != path));
    }
    if wanted == file_stem(path) {
        return Ok(new_path);
    }
    if let Err(e) = crate::history::rename_versions(path, &new_path) {
        *HISTORY_WARNING.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
    }
    Ok(new_path)
}

/// Why `rename_page` with `exact` refuses: another page, or a folder that is not the page's own
/// subpage folder, already has the stem of `exact`.
fn rename_refusal(path: &Path, exact: &Path) -> Option<String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let stem = file_stem(exact);
    let own = same_path(&dir.join(&stem), &dir.join(file_stem(path)));
    let page_taken = ["md", "markdown"].iter().any(|ext| {
        let other = dir.join(format!("{stem}.{ext}"));
        other.exists() && !same_path(&other, path)
    });
    (page_taken || (!own && dir.join(&stem).exists())).then(|| format!("\"{}\" already has \"{stem}\".", dir.display()))
}

/// Deletes a page (with its subpages and versions) into the history of its folder.
pub fn delete_page(path: &Path) -> Result<PathBuf, OpError> {
    crate::history::delete_page(path, chrono::Local::now().naive_local())
}

pub fn create_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let path = unique_path(parent, &safe_name(name), "");
    fs::create_dir_all(&path).map_err(|e| format!("Could not create the section: {e}"))?;
    Ok(path)
}

/// Renames a section folder to `name`; refused when something else already has that name (a change
/// of case only, also of a non-ASCII letter, is allowed).
pub fn rename_folder(path: &Path, name: &str) -> Result<PathBuf, OpError> {
    let parent = path.parent().ok_or("This folder cannot be renamed")?;
    let new_path = parent.join(safe_name(name));
    if new_path == path {
        return Ok(new_path);
    }
    if new_path.exists() && !same_path(&new_path, path) {
        return Err(OpError::refused(format!("A section named \"{}\" already exists here.", safe_name(name))));
    }
    fs::rename(path, &new_path).map_err(|e| OpError::io(format!("Could not rename the section: {e}"), &e))?;
    Ok(new_path)
}

/// Deletes a section into the history of its parent folder.
pub fn delete_folder(path: &Path) -> Result<PathBuf, OpError> {
    crate::history::delete_folder(path, chrono::Local::now().naive_local())
}

/// Lower-case path components, for comparing folders on a case-insensitive file system.
pub(crate) fn path_key(p: &Path) -> Vec<String> {
    p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect()
}

/// Whether two paths name the same file or folder (case-insensitive).
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// Whether `p` is `dir` or lies below it (case-insensitive).
pub(crate) fn is_within(p: &Path, dir: &Path) -> bool {
    key_within(&path_key(p), &path_key(dir))
}

/// `is_within` on keys made by `path_key`.
fn key_within(p: &[String], dir: &[String]) -> bool {
    p.len() >= dir.len() && p[..dir.len()] == dir[..]
}

/// `p` with its leading `from` replaced by `to` (case-insensitive), or `None` when `p` is not
/// `from` or below it.
pub(crate) fn rebase(p: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    is_within(p, from).then(|| to.join(p.components().skip(from.components().count()).collect::<PathBuf>()))
}

/// "Top-level folder › Section › ..." for `folder`, which lies in one of `roots`; empty otherwise.
pub(crate) fn place_in(roots: &[PathBuf], folder: &Path) -> String {
    let Some(root) = roots.iter().find(|r| is_within(folder, r)) else { return String::new() };
    // Keep the root's own name in front: skip only the components of its parent.
    let skip = root.parent().map_or(root.components().count(), |b| b.components().count());
    folder.components().skip(skip).map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join(" › ")
}

/// Whether a new top-level folder may be added to the open ones: it must not be one of them,
/// lie inside one of them, or contain one of them.
pub fn check_new_root(existing: &[PathBuf], new: &Path) -> Result<(), String> {
    let n = path_key(new);
    for e in existing {
        let k = path_key(e);
        if n == k {
            return Err(format!("\"{}\" is already open.", new.display()));
        }
        if n.starts_with(&k) {
            return Err(format!("\"{}\" lies inside the open folder \"{}\".", new.display(), e.display()));
        }
        if k.starts_with(&n) {
            return Err(format!("\"{}\" contains the open folder \"{}\"; remove that one first.", new.display(), e.display()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Search

/// Lower-case and drop accents so "crne" finds "črne". Always one char in, one char out.
pub fn fold_char(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'č' | 'ć' | 'ç' => 'c',
        'š' | 'ś' => 's',
        'ž' | 'ź' | 'ż' => 'z',
        'đ' => 'd',
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ą' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'ę' | 'ě' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ő' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'ű' | 'ů' => 'u',
        'ñ' | 'ń' | 'ň' => 'n',
        'ř' => 'r',
        'ť' => 't',
        'ý' | 'ÿ' => 'y',
        'ł' | 'ľ' => 'l',
        '\u{a0}' => ' ',
        c => c,
    }
}

pub fn fold(s: &str) -> Vec<char> {
    s.chars().map(fold_char).collect()
}

pub struct IndexedPage {
    pub path: PathBuf,
    pub title: String,
    /// "Notebook › Section"
    pub place: String,
    pub text: Vec<char>,
    folded_title: Vec<char>,
    folded_text: Vec<char>,
    /// `path_key(path)`, so that finding a page by path does not build a key per page.
    key: Vec<String>,
}

/// Reads a page for the index; `None` when it cannot be read.
fn read_indexed(path: &Path, place: String) -> Option<IndexedPage> {
    let doc = read_doc(path).ok()?;
    let title = doc.title_or(path);
    let text: Vec<char> = crate::render::plain_text(&doc.body).chars().collect();
    let folded_text: Vec<char> = text.iter().map(|c| fold_char(*c)).collect();
    Some(IndexedPage {
        place,
        folded_title: fold(&title),
        key: path_key(path),
        path: path.to_path_buf(),
        title,
        text,
        folded_text,
    })
}

pub struct SearchHit {
    pub path: PathBuf,
    pub title: String,
    pub place: String,
    pub snippet: String,
    pub count: usize,
}

#[derive(Default)]
pub struct Index {
    pub pages: Vec<IndexedPage>,
    /// The open top-level folders; a page's place starts with the name of the one it lies in.
    pub roots: Vec<PathBuf>,
}

impl Index {
    /// Indexes every page below the given folders.
    pub fn build(roots: &[PathBuf]) -> Index {
        let mut idx = Index { pages: Vec::new(), roots: roots.to_vec() };
        for root in roots {
            let mut stack = vec![root.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = fs::read_dir(&dir) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    if entry_is_dir(&e) {
                        let name = e.file_name().to_string_lossy().to_string();
                        if !is_hidden_folder(&name) {
                            stack.push(p);
                        }
                    } else if is_md(&p) {
                        // Every file is listed once: no need to look for an older entry.
                        let place = idx.place_of(&p);
                        idx.pages.extend(read_indexed(&p, place));
                    }
                }
            }
        }
        idx
    }

    pub fn add(&mut self, path: &Path) {
        self.remove(path);
        let place = self.place_of(path);
        self.pages.extend(read_indexed(path, place));
    }

    pub fn remove(&mut self, path: &Path) {
        let key = path_key(path);
        self.pages.retain(|p| p.key != key);
    }

    /// Removes everything at or below `path` (a deleted or renamed folder).
    pub fn remove_tree(&mut self, path: &Path) {
        let dir = path_key(path);
        self.pages.retain(|p| !key_within(&p.key, &dir));
    }

    /// Indexes every page at or below `path` (replacing what was indexed there); reads each file once.
    pub fn add_tree(&mut self, path: &Path) {
        self.remove_tree(path);
        for mut p in Index::build(&[path.to_path_buf()]).pages {
            p.place = self.place_of(&p.path);
            self.pages.push(p);
        }
    }

    /// Follows a file or folder that was moved or renamed from `from` to `to` without reading any
    /// file: the pages at or below `from` get their new path and place. Titles stay as they were.
    pub fn relocate(&mut self, from: &Path, to: &Path) {
        let (roots, dir, skip) = (&self.roots, path_key(from), from.components().count());
        for p in &mut self.pages {
            if key_within(&p.key, &dir) {
                let new = to.join(p.path.components().skip(skip).collect::<PathBuf>());
                p.place = place_in(roots, new.parent().unwrap_or(Path::new("")));
                p.key = path_key(&new);
                p.path = new;
            }
        }
    }

    /// "Top-level folder › Section › ..." for a page.
    pub(crate) fn place_of(&self, path: &Path) -> String {
        place_in(&self.roots, path.parent().unwrap_or(Path::new("")))
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let words: Vec<Vec<char>> = query.split_whitespace().map(fold).filter(|w| !w.is_empty()).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<(usize, usize, SearchHit)> = Vec::new();
        for p in &self.pages {
            let Some((title_score, count, snippet)) = match_folded(&p.folded_title, &p.text, &p.folded_text, &words) else { continue };
            hits.push((
                title_score,
                count,
                SearchHit { path: p.path.clone(), title: p.title.clone(), place: p.place.clone(), snippet, count },
            ));
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then_with(|| natural_cmp(&a.2.title, &b.2.title)));
        hits.into_iter().take(limit).map(|h| h.2).collect()
    }
}

/// Whether a page matches all `words` (folded) in its title or text. Returns the number of title
/// words found, the number of occurrences in the text and a snippet around the first one.
fn match_folded(folded_title: &[char], text: &[char], folded_text: &[char], words: &[Vec<char>]) -> Option<(usize, usize, String)> {
    let mut title_score = 0;
    let mut count = 0;
    let mut first_pos: Option<usize> = None;
    for w in words {
        let in_title = find_chars(folded_title, w, 0).is_some();
        let body_hits = count_chars(folded_text, w);
        if !in_title && body_hits == 0 {
            return None;
        }
        if in_title {
            title_score += 1;
        }
        count += body_hits;
        if first_pos.is_none() {
            first_pos = find_chars(folded_text, w, 0);
        }
    }
    let snippet = match first_pos {
        Some(pos) => make_snippet(text, pos, words[0].len()),
        None => make_snippet(text, 0, 0),
    };
    Some((title_score, count, snippet))
}

/// Matches `text` (title and plain body) against folded `words`; returns the number of
/// occurrences and a snippet. Used where no prebuilt index exists (the history search).
pub fn match_text(text: &str, words: &[Vec<char>]) -> Option<(usize, String)> {
    if words.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars.iter().map(|c| fold_char(*c)).collect();
    match_folded(&[], &chars, &folded, words).map(|(_, count, snippet)| (count, snippet))
}

fn find_chars(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i] == needle[0] && &hay[i..i + needle.len()] == needle)
}

fn count_chars(hay: &[char], needle: &[char]) -> usize {
    let mut n = 0;
    let mut from = 0;
    while let Some(i) = find_chars(hay, needle, from) {
        n += 1;
        from = i + needle.len();
    }
    n
}

fn make_snippet(text: &[char], pos: usize, len: usize) -> String {
    let start = pos.saturating_sub(50);
    let end = (pos + len + 90).min(text.len());
    let mut s: String = text[start..end].iter().map(|c| if c.is_whitespace() { ' ' } else { *c }).collect();
    // Collapse runs of spaces.
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(s.trim());
    if end < text.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebase_ignores_case_and_rejects_outside_paths() {
        let (from, to) = (Path::new(r"C:\Notes\Work"), Path::new(r"D:\Other\Work (2)"));
        assert_eq!(rebase(Path::new(r"c:\notes\WORK\a\b.md"), from, to), Some(PathBuf::from(r"D:\Other\Work (2)\a\b.md")));
        assert_eq!(rebase(Path::new(r"C:\Notes\work"), from, to), Some(to.to_path_buf()));
        assert_eq!(rebase(Path::new(r"C:\Notes\Workshop\a.md"), from, to), None);
        assert_eq!(rebase(Path::new(r"C:\Notes"), from, to), None);
    }

    #[test]
    fn place_in_starts_with_the_root_name_and_ignores_case() {
        let roots = vec![PathBuf::from(r"C:\Data\Notes")];
        assert_eq!(place_in(&roots, Path::new(r"c:\data\notes\Work\Plans")), "notes › Work › Plans");
        assert_eq!(place_in(&roots, Path::new(r"C:\Data\Notes")), "Notes");
        assert_eq!(place_in(&roots, Path::new(r"C:\Elsewhere")), "");
    }

    #[test]
    fn front_matter_round_trip() {
        let src = "---\ntitle: Hello\nupdated: 2010-12-31T20:53:13\n---\n\nBody line\n";
        let doc = PageDoc::parse(src);
        assert_eq!(doc.get("title"), Some("Hello"));
        assert_eq!(doc.body, "\nBody line\n");
        assert_eq!(doc.to_text(), src);
    }

    #[test]
    fn no_front_matter() {
        let doc = PageDoc::parse("# Just text\n");
        assert!(doc.meta.is_empty());
        assert_eq!(doc.to_text(), "# Just text\n");
    }

    fn temp_root(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("notecraft-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn save_makes_version() {
        let root = temp_root("savever");
        let page = create_page(&root, "A").unwrap();
        save_body(&page, "\nfirst\n").unwrap();
        let v = crate::history::list_versions(&page);
        assert_eq!(v.len(), 1);
        assert_eq!(read_doc(&v[0].path).unwrap().body, "\n");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn blocked_history_dir_does_not_fail_save() {
        let root = temp_root("blocked");
        let page = create_page(&root, "A").unwrap();
        fs::write(root.join(crate::history::HISTORY_DIR), "in the way").unwrap();
        save_body(&page, "\nnew text\n").unwrap();
        assert!(read_doc(&page).unwrap().body.contains("new text"));
        assert!(take_history_warning().is_some_and(|w| w.contains("history")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_page_moves_versions() {
        let root = temp_root("renver");
        let page = create_page(&root, "Old name").unwrap();
        save_body(&page, "\ntext\n").unwrap();
        let renamed = rename_page(&page, "New name", None).unwrap();
        assert_eq!(crate::history::list_versions(&renamed).len(), 1);
        assert!(crate::history::list_versions(&page).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_page_exact_stem() {
        let root = temp_root("renexact");
        let page = root.join("Odd name (2).md");
        fs::write(&page, "---\ntitle: Odd\n---\n\ntext\n").unwrap();
        save_body(&page, "\nmore text\n").unwrap();
        let renamed = rename_page(&page, "New", None).unwrap();
        assert_eq!(renamed.file_name().unwrap(), "New.md");
        let back = rename_page(&renamed, "Odd", Some(Path::new("Odd name (2).md"))).unwrap();
        assert_eq!(back.file_name().unwrap(), "Odd name (2).md");
        assert_eq!(read_doc(&back).unwrap().get("title"), Some("Odd"));
        assert_eq!(crate::history::list_versions(&back).len(), 1);
        assert!(crate::history::list_versions(&renamed).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_page_exact_refuses_taken() {
        let root = temp_root("renrefuse");
        let a = create_page(&root, "A").unwrap();
        create_page(&root, "B").unwrap();
        let before = fs::read(&a).unwrap();
        assert!(rename_page(&a, "B", Some(Path::new("B.md"))).is_err());
        assert!(rename_page(&a, "b", Some(Path::new("b.md"))).is_err());
        assert_eq!(fs::read(&a).unwrap(), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_page_exact_case_only_non_ascii() {
        let root = temp_root("renunicode");
        let page = root.join("Čaj.md");
        fs::write(&page, "---\ntitle: Čaj\n---\n\ntext\n").unwrap();
        let renamed = rename_page(&page, "čaj", Some(Path::new("čaj.md"))).unwrap();
        assert_eq!(renamed.file_name().unwrap(), "čaj.md");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_folder_case_only_non_ascii() {
        let root = temp_root("renfolderunicode");
        let sec = root.join("čaj");
        fs::create_dir_all(&sec).unwrap();
        assert_eq!(rename_folder(&sec, "Čaj").unwrap(), root.join("Čaj"));
        fs::create_dir_all(root.join("Kava")).unwrap();
        assert_eq!(rename_folder(&root.join("Čaj"), "kava").unwrap_err().kind, Kind::Refused);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_page_exact_keeps_markdown_extension() {
        let root = temp_root("renkeepext");
        let page = root.join("X.markdown");
        fs::write(&page, "---\ntitle: X\n---\n\ntext\n").unwrap();
        let renamed = rename_page(&page, "Y", None).unwrap();
        assert_eq!(renamed, root.join("Y.md"));
        let back = rename_page(&renamed, "X", Some(Path::new("X.markdown"))).unwrap();
        assert_eq!(back, root.join("X.markdown"));
        assert!(back.is_file() && !root.join("X.md").exists() && !root.join("Y.md").exists());
        assert_eq!(read_doc(&back).unwrap().get("title"), Some("X"));
        let _ = fs::remove_dir_all(&root);
    }

    /// A rename that fails must not change the title.
    #[cfg(windows)]
    #[test]
    fn rename_page_failure_leaves_title() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = temp_root("renfail");
        let page = create_page(&root, "Old name").unwrap();
        let before = fs::read(&page).unwrap();
        // Readable and writable by others, but not renamable (no FILE_SHARE_DELETE).
        let _lock = fs::OpenOptions::new().read(true).share_mode(3).open(&page).unwrap();
        assert!(rename_page(&page, "New name", None).is_err());
        assert_eq!(fs::read(&page).unwrap(), before);
        drop(_lock);
        let _ = fs::remove_dir_all(&root);
    }

    /// A subpage folder that cannot be renamed (a file in it is open) stops the whole rename:
    /// the page keeps its name and title and the subpages stay with it.
    #[cfg(windows)]
    #[test]
    fn rename_page_subfolder_locked_changes_nothing() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = temp_root("rensublock");
        let page = create_page(&root, "Old name").unwrap();
        let sub = create_page(&subpage_dir(&page), "Child").unwrap();
        let before = fs::read(&page).unwrap();
        let lock = fs::OpenOptions::new().read(true).share_mode(3).open(&sub).unwrap();
        let e = rename_page(&page, "New name", None).unwrap_err();
        drop(lock);
        assert_eq!(e.kind, Kind::Busy, "{e}");
        assert_eq!(fs::read(&page).unwrap(), before);
        assert!(sub.is_file() && !root.join("New name.md").exists() && !root.join("New name").exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// When the file cannot be renamed after its subpages folder was, the folder gets its name back.
    #[cfg(windows)]
    #[test]
    fn rename_page_file_locked_turns_subfolder_back() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = temp_root("renfileback");
        let page = create_page(&root, "Old name").unwrap();
        let sub = create_page(&subpage_dir(&page), "Child").unwrap();
        let lock = fs::OpenOptions::new().read(true).share_mode(3).open(&page).unwrap();
        let e = rename_page(&page, "New name", None).unwrap_err();
        drop(lock);
        assert_eq!(e.kind, Kind::Busy, "{e}");
        assert!(sub.is_file() && !root.join("New name").exists() && !root.join("New name.md").exists());
        let _ = fs::remove_dir_all(&root);
    }

    /// A folder that already has the new name (not a page) is not merged with the subpages.
    #[test]
    fn rename_page_refuses_taken_subfolder() {
        let root = temp_root("rensubtaken");
        let page = create_page(&root, "Old name").unwrap();
        let sub = create_page(&subpage_dir(&page), "Child").unwrap();
        fs::create_dir_all(root.join("New name")).unwrap();
        let before = fs::read(&page).unwrap();
        let e = rename_page(&page, "New name", None).unwrap_err();
        assert_eq!(e.kind, Kind::Refused, "{e}");
        assert_eq!(fs::read(&page).unwrap(), before);
        assert!(sub.is_file() && !root.join("New name.md").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_page_exact_refuses_markdown_extension() {
        let root = temp_root("renmarkdown");
        let a = create_page(&root, "A").unwrap();
        fs::write(root.join("B.markdown"), "---\ntitle: B\n---\n").unwrap();
        let before = fs::read(&a).unwrap();
        assert!(rename_page(&a, "B", Some(Path::new("B.md"))).is_err());
        assert_eq!(fs::read(&a).unwrap(), before);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_returns_entry() {
        let root = temp_root("delentry");
        let sec = create_folder(&root, "Sec").unwrap();
        let page = create_page(&sec, "P").unwrap();
        let item = crate::history::deleted_item_at(&delete_page(&page).unwrap()).unwrap();
        assert_eq!(item.name, "P");
        let item = crate::history::deleted_item_at(&delete_folder(&sec).unwrap()).unwrap();
        assert_eq!(item.name, "Sec");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn deletes_go_to_history() {
        let root = temp_root("dels");
        let sec = create_folder(&root, "Sec").unwrap();
        let page = create_page(&sec, "P").unwrap();
        delete_page(&page).unwrap();
        assert!(!page.exists());
        delete_folder(&sec).unwrap();
        assert!(!sec.exists());
        let items = crate::history::list_deleted(&root);
        assert_eq!(items.len(), 1); // the page was inside the section that was deleted later
        assert_eq!(items[0].name, "Sec");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn history_is_invisible() {
        let root = temp_root("invisible");
        let page = create_page(&root, "Keep").unwrap();
        let gone = create_page(&root, "Gone").unwrap();
        save_body(&gone, "\nzebrafish\n").unwrap();
        save_body(&gone, "\nzebrafish and more\n").unwrap();
        delete_page(&gone).unwrap();
        save_body(&page, "\nplain\n").unwrap();
        save_body(&page, "\nplain text\n").unwrap();
        let tree = scan_tree(&root);
        assert!(tree.children.is_empty());
        assert_eq!(tree.page_count, 1);
        assert_eq!(list_pages(&root, SortOrder::Name).len(), 1);
        let idx = Index::build(&[root.clone()]);
        assert_eq!(idx.pages.len(), 1);
        assert!(idx.search("zebrafish", 10).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn create_save_rename_search() {
        let root = std::env::temp_dir().join(format!("notecraft-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let section = create_folder(&root, "Recipes: soups").unwrap();
        assert_eq!(section.file_name().unwrap(), "Recipes- soups");
        let page = create_page(&section, "Goulash / beef").unwrap();
        assert_eq!(page.file_name().unwrap(), "Goulash - beef.md");
        save_body(&page, "\nPaprika and **onions**\n- [ ] buy beef\n").unwrap();
        let doc = read_doc(&page).unwrap();
        assert_eq!(doc.get("title"), Some("Goulash / beef"));
        assert!(doc.body.contains("onions"));

        // Subpage follows its page on rename.
        let sub = create_page(&subpage_dir(&page), "Variant").unwrap();
        assert!(sub.is_file());
        let renamed = rename_page(&page, "Beef goulash", None).unwrap();
        assert_eq!(renamed.file_name().unwrap(), "Beef goulash.md");
        assert!(subpage_dir(&renamed).join("Variant.md").is_file());
        assert_eq!(read_doc(&renamed).unwrap().get("title"), Some("Beef goulash"));

        let pages = list_pages(&section, SortOrder::Name);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[1].level, 1);

        let tree = scan_tree(&root);
        assert_eq!(tree.children.len(), 1, "subpage folder must not show as a section");

        let idx = Index::build(std::slice::from_ref(&root));
        let hits = idx.search("ONIONS paprika", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Beef goulash");
        assert!(idx.search("zzz", 10).is_empty());

        let moved = rename_folder(&section, "Soups").unwrap();
        assert!(moved.join("Beef goulash.md").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn several_roots() {
        let base = std::env::temp_dir().join(format!("notecraft-roots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (a, b) = (base.join("A"), base.join("B"));
        let soups = create_folder(&a, "Soups").unwrap();
        create_page(&soups, "Goulash").unwrap();
        create_page(&a, "Loose page").unwrap();
        let fish = create_folder(&b, "Fish").unwrap();
        create_page(&fish, "Trout goulash").unwrap();

        // Each folder is a tree of its own, named after the folder.
        let (ta, tb) = (scan_tree(&a), scan_tree(&b));
        assert_eq!((ta.name.as_str(), ta.children.len(), ta.page_count), ("A", 1, 1));
        assert_eq!((tb.name.as_str(), tb.children.len(), tb.page_count), ("B", 1, 0));

        // One index over both; the place starts with the top-level folder.
        let mut idx = Index::build(&[a.clone(), b.clone()]);
        assert_eq!(idx.pages.len(), 3);
        let hits = idx.search("goulash", 10);
        let places: Vec<&str> = hits.iter().map(|h| h.place.as_str()).collect();
        assert!(places.contains(&"A › Soups") && places.contains(&"B › Fish"), "{places:?}");
        assert_eq!(idx.search("loose", 10)[0].place, "A");

        // A page added later gets the same kind of place.
        let late = create_page(&soups, "Late goulash").unwrap();
        idx.add(&late);
        assert!(idx.search("late", 10).iter().any(|h| h.place == "A › Soups"));

        // A section moved to the other folder: the index follows without reading the files.
        let moved = b.join("Soups");
        fs::rename(&soups, &moved).unwrap();
        idx.relocate(&soups, &moved);
        let hits = idx.search("goulash", 10);
        assert_eq!(hits.len(), 3);
        assert!(hits.iter().all(|h| store_place_ok(h, &b)), "{:?}", hits.iter().map(|h| (&h.path, &h.place)).collect::<Vec<_>>());

        // add_tree replaces what was there and gives each page its place in the open folders.
        create_page(&moved, "Fresh goulash").unwrap();
        idx.add_tree(&moved);
        assert_eq!(idx.pages.len(), 5);
        assert!(idx.search("fresh", 10).iter().any(|h| h.place == "B › Soups"));

        // Paths are found without case, also after a relocate (the stored key follows the path).
        let late_now = moved.join(late.file_name().unwrap());
        idx.remove(Path::new(&late_now.to_string_lossy().to_uppercase()));
        assert_eq!(idx.pages.len(), 4);
        idx.remove_tree(Path::new(&moved.to_string_lossy().to_lowercase()));
        // Left: the loose page in A and the trout in B › Fish.
        assert_eq!(idx.pages.len(), 2);
        let _ = fs::remove_dir_all(&base);
    }

    /// A goulash hit after the move: the trout in B › Fish, the soups now in B › Soups.
    fn store_place_ok(h: &SearchHit, b: &Path) -> bool {
        is_within(&h.path, b) && h.path.is_file() && (h.place == "B › Soups" || h.place == "B › Fish")
    }

    #[test]
    fn new_root_checks() {
        let open = vec![PathBuf::from(r"C:\Notes\A"), PathBuf::from(r"C:\Notes\B")];
        assert!(check_new_root(&open, Path::new(r"C:\Notes\C")).is_ok());
        assert!(check_new_root(&open, Path::new(r"C:\Notes\AB")).is_ok(), "a name prefix is not nesting");
        assert!(check_new_root(&open, Path::new(r"c:\notes\a")).is_err(), "same folder, other case");
        assert!(check_new_root(&open, Path::new(r"C:\Notes\A\Sub")).is_err());
        assert!(check_new_root(&open, Path::new(r"C:\Notes")).is_err());
    }

    #[test]
    fn crlf_is_kept() {
        let src = "---\r\ntitle: A\r\n---\r\nline 1\r\nline 2\r\n";
        let doc = PageDoc::parse(src);
        assert_eq!(doc.get("title"), Some("A"));
        assert_eq!(doc.body, "line 1\nline 2\n");
        assert_eq!(doc.to_text(), src);
    }

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("Page 2", "Page 10"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("Čebula", "Dan"), std::cmp::Ordering::Less);
    }

    #[test]
    fn folding_keeps_length() {
        assert_eq!(fold("Črne Šole").len(), "Črne Šole".chars().count());
        assert_eq!(fold("ŠKOTSKA").iter().collect::<String>(), "skotska");
    }
}
