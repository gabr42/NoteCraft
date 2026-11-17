//! Moving a page (with its subpages, versions and pictures) to another folder.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

/// Picture links: `](<resources/a b.png>)` and `<resources/a b.png>` (the editor writes names with
/// spaces or parentheses this way; the name runs to `>`), and `](resources/x)`, `src="resources/x"`,
/// `src='resources/x'`, each also with `./` before `resources`. Groups: 1+2 = angle form (prefix,
/// name), 3+4 = plain form (prefix, name); the prefix includes the `./`.
static LINK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"((?:\]\(<|<)(?:\./)?)resources/([^>\r\n]+)>|((?:\]\(|src="|src=')(?:\./)?)resources/([^)"'>\s]+)"#)
        .expect("mover: link pattern")
});

/// The prefix, the name as written and whether it is the angle form, of one link match.
fn link_parts<'a>(c: &regex::Captures<'a>) -> (&'a str, &'a str, bool) {
    match (c.get(1), c.get(2)) {
        (Some(p), Some(n)) => (p.as_str(), n.as_str(), true),
        _ => (c.get(3).map_or("", |m| m.as_str()), c.get(4).map_or("", |m| m.as_str()), false),
    }
}

/// Picture names compare without case, like the file system.
fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// Decodes `%XX` sequences (UTF-8); anything else is kept as is.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'%'));
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// File names (decoded, without duplicates, in order) of all `resources/<name>` links in a page.
pub(crate) fn picture_links(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in LINK.captures_iter(text) {
        let name = decode(link_parts(&c).1);
        if !out.iter().any(|o| same_name(o, &name)) {
            out.push(name);
        }
    }
    out
}

/// Percent-encodes the characters that would end or confuse a link: `%`, space, `(` and `)`.
pub(crate) fn encode_name(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            '%' => s.push_str("%25"),
            ' ' => s.push_str("%20"),
            '(' => s.push_str("%28"),
            ')' => s.push_str("%29"),
            _ => s.push(ch),
        }
    }
    s
}

/// Points every link to the picture `old` at `new` instead (a shorthand for tests).
#[cfg(test)]
pub(crate) fn rewrite_picture_link(text: &str, old: &str, new: &str) -> String {
    rewrite_picture_links(text, &[(old.to_string(), new.to_string())])
}

/// Renames pictures in all links in one pass: each link is looked up once in `renames` (old name,
/// new name), so a new name that is also the old name of a later pair is not renamed again.
pub(crate) fn rewrite_picture_links(text: &str, renames: &[(String, String)]) -> String {
    LINK.replace_all(text, |c: &regex::Captures| {
        let (prefix, name, angle) = link_parts(c);
        let decoded = decode(name);
        let Some((_, new)) = renames.iter().find(|(o, _)| same_name(&decoded, o)) else {
            return c[0].to_string();
        };
        if angle && !name.contains('%') {
            format!("{prefix}resources/{}>", new.replace('<', "%3C").replace('>', "%3E"))
        } else if angle {
            format!("{prefix}resources/{}>", encode_name(new))
        } else {
            format!("{prefix}resources/{}", encode_name(new))
        }
    })
    .into_owned()
}

/// The first of `stem`, `stem (2)`, `stem (3)` … that is free in `dir` as a page (`.md` or
/// `.markdown`), as a subpage folder and as a versions folder.
pub(crate) fn free_stem(dir: &Path, stem: &str) -> String {
    let mut n = 1;
    loop {
        let s = if n == 1 { stem.to_string() } else { format!("{stem} ({n})") };
        let page = dir.join(format!("{s}.md"));
        let taken = ["md", "markdown"].iter().any(|e| dir.join(format!("{s}.{e}")).exists())
            || dir.join(&s).exists()
            || crate::history::versions_dir(&page).exists();
        if !taken {
            return s;
        }
        n += 1;
    }
}

/// The name of a folder as messages show it (the whole path for a drive root).
fn display_name(dir: &Path) -> String {
    dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().to_string())
}

fn drive_of(p: &Path) -> String {
    match p.components().next() {
        Some(Component::Prefix(x)) => x.as_os_str().to_string_lossy().to_lowercase(),
        _ => String::new(),
    }
}

/// Where a page or section went, and what of the original could not be removed: a move across
/// drives copies first, and a file that is open elsewhere stays behind.
pub struct Moved {
    pub path: PathBuf,
    pub left_behind: Vec<PathBuf>,
    /// The picture renames this move made, (source name, target name); empty for sections.
    pub pictures: Vec<(String, String)>,
    /// Target names of the pictures that were already in the target with the same bytes, so nothing
    /// was copied; the undo must not remove them from there. Empty for sections.
    pub kept: Vec<String>,
}

/// Names an exact move must give (undo/redo). `pictures`: (name linked in the page now, name it must
/// get in the target `resources`); a linked picture not listed keeps its name.
pub struct Exact {
    pub stem: String,
    /// Extension of the page file, without the dot (`md`, or `markdown` for a page that had it).
    pub ext: String,
    pub pictures: Vec<(String, String)>,
    /// Pictures (names linked in the page now) that stay in the source folder: they were there before
    /// the move this one takes back.
    pub keep: Vec<String>,
}

/// Removes `p` and everything below it as far as possible, also read-only files, and adds what is
/// still there to `left` (a folder only when nothing inside it was listed already).
fn remove_best_effort(p: &Path, left: &mut Vec<PathBuf>) {
    let Ok(meta) = fs::symlink_metadata(p) else { return };
    if meta.is_dir() {
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                remove_best_effort(&e.path(), left);
            }
        }
        if fs::remove_dir(p).is_err() && !left.iter().any(|l| l.starts_with(p)) {
            left.push(p.to_path_buf());
        }
        return;
    }
    let mut removed = fs::remove_file(p).or_else(|_| fs::remove_dir(p)).is_ok();
    if !removed && meta.permissions().readonly() {
        let mut perm = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        if fs::set_permissions(p, perm).is_ok() {
            removed = fs::remove_file(p).is_ok();
            if !removed {
                // It stays behind: keep its protection.
                let _ = fs::set_permissions(p, meta.permissions());
            }
        }
    }
    if !removed {
        left.push(p.to_path_buf());
    }
}

/// Copies a file or a whole folder to `to`, then removes the original. Returns what of the original
/// could not be removed; the move counts as done then (the copy is complete).
/// What is already at `to` is kept: a file there with the same bytes counts as copied (the undo of a
/// move finds the originals that stayed behind), a file with other bytes is an error. When the copy
/// fails, only what this call created is removed again.
fn copy_then_remove(from: &Path, to: &Path) -> std::io::Result<Vec<PathBuf>> {
    fn copy_tree(from: &Path, to: &Path, created: &mut Vec<PathBuf>) -> std::io::Result<()> {
        if from.is_dir() {
            if !to.is_dir() {
                fs::create_dir(to)?;
                created.push(to.to_path_buf());
            }
            for e in fs::read_dir(from)? {
                let e = e?;
                copy_tree(&e.path(), &to.join(e.file_name()), created)?;
            }
        } else if to.exists() {
            if !same_bytes(from, to) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("{} already exists with other content", to.display()),
                ));
            }
        } else {
            fs::copy(from, to)?;
            created.push(to.to_path_buf());
        }
        Ok(())
    }
    if let Some(p) = to.parent() {
        fs::create_dir_all(p)?;
    }
    let mut created = Vec::new();
    if let Err(e) = copy_tree(from, to, &mut created) {
        // Leave nothing half-copied behind, but never touch what was there before.
        for p in created.iter().rev() {
            let _ = if p.is_dir() { fs::remove_dir(p) } else { fs::remove_file(p) };
        }
        return Err(e);
    }
    // The copy is complete: the move counts as done even when some originals cannot be removed.
    let mut left = Vec::new();
    remove_best_effort(from, &mut left);
    Ok(left)
}

/// All files below `dir` (none when it does not exist).
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { out.extend(files_under(&p)) } else { out.push(p) }
        }
    }
    out
}

fn is_md(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

use crate::store::{is_within, same_path};

/// Whether `page` may be moved into `target_dir`: not its own folder, not its subpage folder or below.
pub fn can_move_to(page: &Path, target_dir: &Path) -> bool {
    let Some(dir) = page.parent() else { return false };
    !same_path(dir, target_dir)
        && !is_within(target_dir, &crate::store::subpage_dir(page))
}

/// Whether two files hold the same bytes.
fn same_bytes(a: &Path, b: &Path) -> bool {
    matches!((fs::read(a), fs::read(b)), (Ok(x), Ok(y)) if x == y)
}

/// Removes `dir` when it is the subpage folder of a page and nothing is left in it but empty
/// `resources` and `.history` folders. A section folder is never removed.
fn remove_if_empty_subpage_folder(dir: &Path) {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else { return };
    if !parent.join(format!("{}.md", name.to_string_lossy())).is_file() {
        return;
    }
    let history = dir.join(".history");
    for d in [dir.join("resources"), history.join("pages"), history.join("deleted"), history] {
        let _ = fs::remove_dir(d);
    }
    let _ = fs::remove_dir(dir);
}

/// `x.png` -> first free of `x-2.png`, `x-3.png` … in `dir` and not in `taken`.
fn free_picture_name(dir: &Path, name: &str, taken: &[String]) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut k = 2;
    loop {
        let n = format!("{stem}-{k}{ext}");
        if !dir.join(&n).exists() && !taken.iter().any(|t| same_name(t, &n)) {
            return n;
        }
        k += 1;
    }
}

/// Picture names still linked by what stays in `dir`: its pages and everything in its `.history`.
/// `None` when one of those files cannot be read (then nothing may be deleted).
fn names_still_linked(dir: &Path) -> Option<Vec<String>> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_md(p))
        .collect();
    files.extend(files_under(&dir.join(".history")).into_iter().filter(|p| is_md(p)));
    let mut out = Vec::new();
    for f in files {
        out.extend(picture_links(&fs::read_to_string(&f).ok()?));
    }
    Some(out)
}

/// Moves `page` into `target_dir` (a section folder, or the subpage folder of another page) with its
/// subpages, versions and pictures. Pictures are copied first; an original is deleted only after the
/// move succeeded and when nothing left in the source folder still links it.
/// Returns `None` when the page already is in `target_dir`. The caller checks edit locks.
/// With `exact` the page gets exactly that stem and the pictures exactly those names, or the move is
/// refused before anything is copied.
pub fn move_page(page: &Path, target_dir: &Path, exact: Option<&Exact>) -> Result<Option<Moved>, String> {
    use crate::history::versions_dir;
    use crate::store::{file_stem, subpage_dir};
    let src_dir = page.parent().ok_or("The page has no folder.")?;
    if same_path(src_dir, target_dir) {
        return Ok(None);
    }
    if !can_move_to(page, target_dir) {
        return Err("A page cannot be moved under itself.".into());
    }
    let target_name = display_name(target_dir);
    let stem = match exact {
        Some(e) => {
            if free_stem(target_dir, &e.stem) != e.stem {
                return Err(format!("\"{target_name}\" already has a page named \"{}\".", e.stem));
            }
            e.stem.clone()
        }
        None => free_stem(target_dir, &file_stem(page)),
    };
    let ext = exact.map_or("md", |e| e.ext.as_str());
    let new = target_dir.join(format!("{stem}.{ext}"));

    // 1. Plan the pictures the page and its versions link (all exact checks happen here), then copy them.
    let versions = versions_dir(page);
    let mut names: Vec<String> = Vec::new();
    for f in std::iter::once(page.to_path_buf()).chain(files_under(&versions)) {
        let text = fs::read_to_string(&f).map_err(|e| format!("Could not read {}: {e}", f.display()))?;
        for n in picture_links(&text) {
            if !names.contains(&n) {
                names.push(n);
            }
        }
    }
    let (src_res, dst_res) = (src_dir.join("resources"), target_dir.join("resources"));
    // (source name, target name, whether the file still has to be copied)
    let mut plan: Vec<(String, String, bool)> = Vec::new();
    let mut taken: Vec<String> = Vec::new();
    for n in names {
        let src = src_res.join(&n);
        if !src.is_file() {
            continue;
        }
        let wanted = exact
            .and_then(|e| e.pictures.iter().find(|(from, _)| same_name(from, &n)))
            .map_or_else(|| n.clone(), |(_, to)| to.clone());
        let dst = dst_res.join(&wanted);
        let clash = taken.iter().any(|t| same_name(t, &wanted));
        if dst.exists() && !clash && same_bytes(&src, &dst) {
            plan.push((n, wanted, false));
            continue;
        }
        let target = if dst.exists() || clash {
            if exact.is_some() {
                return Err(format!("\"{target_name}\" already has a different picture named \"{wanted}\"."));
            }
            free_picture_name(&dst_res, &n, &taken)
        } else {
            wanted
        };
        taken.push(target.clone());
        plan.push((n, target, true));
    }
    for (n, target, copy) in &plan {
        if !copy {
            continue;
        }
        fs::create_dir_all(&dst_res)
            .and_then(|_| fs::copy(src_res.join(n), dst_res.join(target)))
            .map_err(|e| format!("Could not copy the picture {n} to {}: {e}", dst_res.display()))?;
    }
    let renames: Vec<(String, String)> = plan.iter().filter(|(n, t, _)| n != t).map(|(n, t, _)| (n.clone(), t.clone())).collect();
    let kept: Vec<String> = plan.iter().filter(|(_, _, copy)| !copy).map(|(_, t, _)| t.clone()).collect();
    let keep: &[String] = exact.map_or(&[], |e| e.keep.as_slice());
    // The originals to remove once the page is moved.
    let copied: Vec<String> = plan.into_iter().map(|(n, _, _)| n).filter(|n| !keep.iter().any(|k| same_name(k, n))).collect();

    // 2. Move the page, its subpages and its versions; undo on failure.
    let mut moves = vec![(page.to_path_buf(), new.clone())];
    if subpage_dir(page).is_dir() {
        moves.push((subpage_dir(page), subpage_dir(&new)));
    }
    if versions.is_dir() {
        moves.push((versions, versions_dir(&new)));
    }
    let mut left_behind = Vec::new();
    for (i, (from, to)) in moves.iter().enumerate() {
        if let Some(p) = to.parent() {
            let _ = fs::create_dir_all(p);
        }
        match move_path(from, to) {
            Ok(left) => left_behind.extend(left),
            Err(e) => return Err(undo_moves(&moves[..i], e)),
        }
    }

    // 3. Point links at renamed pictures.
    if !renames.is_empty() {
        for f in std::iter::once(new.clone()).chain(files_under(&versions_dir(&new))) {
            if let Ok(text) = fs::read_to_string(&f) {
                let t = rewrite_picture_links(&text, &renames);
                if t != text {
                    let _ = fs::write(&f, t);
                }
            }
        }
    }

    // 4. Remove originals nothing else needs.
    if let Some(linked) = names_still_linked(src_dir) {
        for n in copied {
            if !linked.iter().any(|l| same_name(l, &n)) {
                let orig = src_res.join(&n);
                if fs::remove_file(&orig).is_err() && orig.exists() {
                    left_behind.push(orig);
                }
            }
        }
    }
    remove_if_empty_subpage_folder(src_dir);
    Ok(Some(Moved { path: new, left_behind, pictures: renames, kept }))
}

/// Moves `done` (pairs of from, to) back in reverse order after the move that failed with `error`.
/// Returns `error`, extended with what the undo could not do, so that nothing goes unreported.
fn undo_moves(done: &[(PathBuf, PathBuf)], error: String) -> String {
    let mut msg = error;
    for (from, to) in done.iter().rev() {
        match move_path(to, from) {
            Ok(left) if left.is_empty() => {}
            Ok(left) => {
                msg.push_str("\n\nThe move was undone, but these copies could not be removed from the new place:");
                for p in left {
                    msg.push_str(&format!("\n{}", p.display()));
                }
            }
            Err(e) => msg.push_str(&format!(
                "\n\nThe move could not be undone for {}: {e}\nIt is kept at {}.",
                from.display(),
                to.display()
            )),
        }
    }
    msg
}

/// Whether the section `folder` may be moved into `target`: not its current parent, not itself or below it.
pub fn can_move_section_to(folder: &Path, target: &Path) -> bool {
    let Some(parent) = folder.parent() else { return false };
    !same_path(parent, target) && !is_within(target, folder)
}

/// Moves a section folder (with its pages, pictures and history, which all live inside it) into
/// `target`. Returns `None` when `target` is already its parent. A name clash gives `Name (2)`; with
/// `name` the section gets exactly that name or the move is refused.
pub fn move_section(folder: &Path, target: &Path, name: Option<&str>) -> Result<Option<Moved>, String> {
    let own = folder.file_name().map(|n| n.to_string_lossy().to_string()).ok_or("This folder cannot be moved.")?;
    if is_within(target, folder) {
        return Err("A section cannot be moved into itself.".into());
    }
    if folder.parent().is_none_or(|p| same_path(p, target)) {
        return Ok(None);
    }
    let new = match name {
        Some(n) => {
            if free_stem(target, n) != n {
                return Err(format!("\"{}\" already has a section named \"{n}\".", display_name(target)));
            }
            target.join(n)
        }
        None => target.join(free_stem(target, &own)),
    };
    let left_behind = move_path(folder, &new)?;
    Ok(Some(Moved { path: new, left_behind, pictures: Vec::new(), kept: Vec::new() }))
}

/// Moves a file or folder; across drives by copying and removing the original. Returns what of the
/// original could not be removed after a complete copy (see `copy_then_remove`).
pub(crate) fn move_path(from: &Path, to: &Path) -> Result<Vec<PathBuf>, String> {
    match fs::rename(from, to) {
        Ok(()) => Ok(Vec::new()),
        Err(e) if from.exists() && drive_of(from) != drive_of(to) => copy_then_remove(from, to)
            .map_err(|e2| format!("Could not move {} to {}: {e} / {e2}", from.display(), to.display())),
        Err(e) => Err(format!("Could not move {} to {}: {e}", from.display(), to.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// `move_page` giving only the new path.
    fn move_page_to(page: &Path, target_dir: &Path) -> Result<Option<PathBuf>, String> {
        move_page(page, target_dir, None).map(|m| m.map(|m| m.path))
    }

    /// `move_section` giving only the new path.
    fn move_section_to(folder: &Path, target: &Path) -> Result<Option<PathBuf>, String> {
        move_section(folder, target, None).map(|m| m.map(|m| m.path))
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("notecraft-move-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn links_all_forms() {
        let t = "![a](resources/x.png) <img src=\"resources/y.jpg\"> [f](resources/z.pdf) <resources/w.png> ![a](resources/x.png) ![b](other/q.png) <img src='resources/v.gif'>";
        assert_eq!(picture_links(t), vec!["x.png", "y.jpg", "z.pdf", "w.png", "v.gif"]);
    }

    #[test]
    fn links_decode_percent() {
        assert_eq!(picture_links("![](resources/Plan%202026-11-16%20103000.png)"), vec!["Plan 2026-11-16 103000.png"]);
        assert_eq!(picture_links("![](resources/%C4%8Crka.png)"), vec!["Črka.png"]);
    }

    #[test]
    fn links_angle_form_from_editor() {
        // What editor.js `mdUrl` writes for names with spaces or parentheses.
        let t = "![](<resources/Plan 2026-11-16 103000.png>) [f](<resources/P (2).pdf>) ![](resources/c.png)";
        assert_eq!(picture_links(t), vec!["Plan 2026-11-16 103000.png", "P (2).pdf", "c.png"]);
    }

    #[test]
    fn rewrite_keeps_angle_form() {
        let t = "![](<resources/a b.png>) ![](resources/a%20b.png)";
        assert_eq!(rewrite_picture_link(t, "a b.png", "a b-2.png"), "![](<resources/a b-2.png>) ![](resources/a%20b-2.png)");
    }

    #[test]
    fn links_dot_slash_form() {
        let t = "![](./resources/x.png) <img src=\"./resources/y.png\"> ![](<./resources/a b.png>)";
        assert_eq!(picture_links(t), vec!["x.png", "y.png", "a b.png"]);
        assert_eq!(
            rewrite_picture_link(t, "x.png", "x-2.png"),
            "![](./resources/x-2.png) <img src=\"./resources/y.png\"> ![](<./resources/a b.png>)"
        );
        assert_eq!(rewrite_picture_link("![](<./resources/a b.png>)", "a b.png", "c.png"), "![](<./resources/c.png>)");
    }

    #[test]
    fn unreadable_pictures_are_not_the_same() {
        let d = temp("same-bytes");
        let (x, y, z) = (d.join("x.png"), d.join("y.png"), d.join("z.png"));
        fs::write(&x, b"X").unwrap();
        fs::write(&y, b"X").unwrap();
        assert!(same_bytes(&x, &y));
        assert!(!same_bytes(&x, &z));
        assert!(!same_bytes(&z, &d.join("missing.png")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn last_subpage_out_removes_empty_folder() {
        let (d, a, b) = notebook("empty-sub");
        let parent = page_with(&a, "P", "text");
        let sub_dir = store_sub(&parent);
        let child = page_with(&sub_dir, "C", "![](resources/x.png)");
        pic(&sub_dir, "x.png", b"X");
        version_of(&child, "old");
        move_page_to(&child, &b).unwrap();
        assert!(!sub_dir.exists(), "empty subpage folder left behind");
        assert!(a.exists() && parent.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn last_page_out_keeps_section() {
        let (d, a, b) = notebook("keep-section");
        let p = page_with(&a, "P", "![](resources/x.png)");
        pic(&a, "x.png", b"X");
        move_page_to(&p, &b).unwrap();
        assert!(a.is_dir(), "section folder must stay");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn subpage_folder_with_other_files_stays() {
        let (d, a, b) = notebook("sub-other");
        let parent = page_with(&a, "P", "text");
        let sub_dir = store_sub(&parent);
        let child = page_with(&sub_dir, "C", "text");
        fs::write(sub_dir.join("notes.txt"), "keep").unwrap();
        move_page_to(&child, &b).unwrap();
        assert!(sub_dir.join("notes.txt").exists());
        let _ = fs::remove_dir_all(&d);
    }

    fn store_sub(page: &Path) -> PathBuf {
        crate::store::subpage_dir(page)
    }

    #[test]
    fn rewrite_keeps_other_links() {
        let t = "![](resources/a%20b.png) ![](resources/c.png) <img src=\"resources/a%20b.png\"> <resources/a%20b.png>";
        assert_eq!(
            rewrite_picture_link(t, "a b.png", "a b-2.png"),
            "![](resources/a%20b-2.png) ![](resources/c.png) <img src=\"resources/a%20b-2.png\"> <resources/a%20b-2.png>"
        );
    }

    #[test]
    fn encode_name_only_special() {
        assert_eq!(encode_name("a b(1)%.png"), "a%20b%281%29%25.png");
        assert_eq!(encode_name("Črka.png"), "Črka.png");
    }

    #[test]
    fn free_stem_skips_folders_and_versions() {
        let d = temp("free-stem");
        fs::write(d.join("Plan.md"), "x").unwrap();
        fs::create_dir_all(d.join("Plan (2)")).unwrap();
        fs::create_dir_all(crate::history::versions_dir(&d.join("Plan (3).md"))).unwrap();
        assert_eq!(free_stem(&d, "Plan"), "Plan (4)");
        assert_eq!(free_stem(&d, "New"), "New");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn move_path_copy_fallback() {
        let d = temp("copy-fallback");
        let from = d.join("from");
        fs::create_dir_all(from.join("inner")).unwrap();
        fs::write(from.join("a.md"), "A").unwrap();
        fs::write(from.join("inner").join("b.png"), [1u8, 2, 3]).unwrap();
        let to = d.join("to");
        assert!(copy_then_remove(&from, &to).unwrap().is_empty());
        assert!(!from.exists());
        assert_eq!(fs::read_to_string(to.join("a.md")).unwrap(), "A");
        assert_eq!(fs::read(to.join("inner").join("b.png")).unwrap(), vec![1u8, 2, 3]);
        // A single file, too.
        let f = d.join("f.txt");
        fs::write(&f, "F").unwrap();
        copy_then_remove(&f, &d.join("g.txt")).unwrap();
        assert!(!f.exists());
        assert_eq!(fs::read_to_string(d.join("g.txt")).unwrap(), "F");
        let _ = fs::remove_dir_all(&d);
    }

    fn page_with(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join(format!("{name}.md"));
        fs::write(&p, format!("---\ntitle: {name}\n---\n\n{body}\n")).unwrap();
        p
    }

    fn version_of(page: &Path, body: &str) -> PathBuf {
        let saved = fs::read_to_string(page).unwrap();
        fs::write(page, body).unwrap();
        let v = crate::history::snapshot_now(page, chrono::Local::now().naive_local()).unwrap();
        fs::write(page, saved).unwrap();
        v
    }

    fn pic(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let r = dir.join("resources");
        fs::create_dir_all(&r).unwrap();
        fs::write(r.join(name), bytes).unwrap();
        r.join(name)
    }

    /// Notebook with sections `A` and `B`.
    fn notebook(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let d = temp(name);
        let (a, b) = (d.join("A"), d.join("B"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        (d, a, b)
    }

    #[test]
    fn to_other_section() {
        let (d, a, b) = notebook("to-other");
        let p = page_with(&a, "P", "text");
        page_with(&a.join("P"), "S", "sub");
        version_of(&p, "old");
        let new = move_page_to(&p, &b).unwrap().unwrap();
        assert_eq!(new, b.join("P.md"));
        assert!(new.is_file() && b.join("P").join("S.md").is_file());
        assert_eq!(crate::history::list_versions(&new).len(), 1);
        assert!(!p.exists() && !a.join("P").exists() && !crate::history::versions_dir(&p).exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn subpage_to_top_level() {
        let (d, a, _) = notebook("sub-up");
        page_with(&a, "P", "");
        let s = page_with(&a.join("P"), "S", "sub");
        assert_eq!(move_page_to(&s, &a).unwrap().unwrap(), a.join("S.md"));
        assert!(!s.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn under_other_page() {
        let (d, a, _) = notebook("under");
        let p = page_with(&a, "A1", "x");
        let t = page_with(&a, "B1", "y");
        let new = move_page_to(&p, &crate::store::subpage_dir(&t)).unwrap().unwrap();
        assert_eq!(new, a.join("B1").join("A1.md"));
        assert!(new.is_file() && !p.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn same_folder_is_noop() {
        let (d, a, _) = notebook("noop");
        let p = page_with(&a, "P", "x");
        let before = fs::metadata(&p).unwrap().modified().unwrap();
        assert_eq!(move_page_to(&p, &a).unwrap(), None);
        assert_eq!(fs::metadata(&p).unwrap().modified().unwrap(), before);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_under_itself() {
        let (d, a, _) = notebook("itself");
        let p = page_with(&a, "P", "x");
        page_with(&a.join("P"), "S", "");
        let sub = crate::store::subpage_dir(&p);
        assert!(move_page_to(&p, &sub).is_err());
        assert!(move_page_to(&p, &sub.join("S")).is_err());
        assert!(!can_move_to(&p, &sub) && !can_move_to(&p, &sub.join("S")) && !can_move_to(&p, &a));
        assert!(p.is_file() && sub.join("S.md").is_file());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn name_clash_gets_suffix() {
        let (d, a, b) = notebook("clash");
        let p = page_with(&a, "P", "mine");
        page_with(&b, "P", "theirs");
        let new = move_page_to(&p, &b).unwrap().unwrap();
        assert_eq!(new, b.join("P (2).md"));
        assert!(fs::read_to_string(&new).unwrap().contains("title: P\n"));
        assert!(fs::read_to_string(b.join("P.md")).unwrap().contains("theirs"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn picture_moves_when_unused() {
        let (d, a, b) = notebook("pic-unused");
        let p = page_with(&a, "P", "![](resources/x.png)");
        let orig = pic(&a, "x.png", b"X");
        let new = move_page_to(&p, &b).unwrap().unwrap();
        assert!(!orig.exists());
        assert_eq!(fs::read(b.join("resources").join("x.png")).unwrap(), b"X");
        assert!(fs::read_to_string(&new).unwrap().contains("](resources/x.png)"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn keeps_picture_used_by_sibling() {
        let (d, a, b) = notebook("pic-sibling");
        let p = page_with(&a, "P", "![](resources/x.png)");
        page_with(&a, "Q", "<img src=\"resources/x.png\">");
        let orig = pic(&a, "x.png", b"X");
        move_page_to(&p, &b).unwrap();
        assert!(orig.exists() && b.join("resources").join("x.png").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn keeps_picture_used_by_version_of_sibling() {
        let (d, a, b) = notebook("pic-version");
        let p = page_with(&a, "P", "![](resources/x.png)");
        let q = page_with(&a, "Q", "nothing");
        version_of(&q, "![](resources/x.png)");
        let orig = pic(&a, "x.png", b"X");
        move_page_to(&p, &b).unwrap();
        assert!(orig.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn keeps_picture_used_by_deleted_page() {
        let (d, a, b) = notebook("pic-deleted");
        let p = page_with(&a, "P", "![](resources/x.png)");
        let q = page_with(&a, "Q", "![](resources/x.png)");
        let orig = pic(&a, "x.png", b"X");
        crate::store::delete_page(&q).unwrap();
        move_page_to(&p, &b).unwrap();
        assert!(orig.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn identical_picture_not_duplicated() {
        let (d, a, b) = notebook("pic-same");
        let p = page_with(&a, "P", "![](resources/x.png)");
        pic(&a, "x.png", b"X");
        pic(&b, "x.png", b"X");
        let new = move_page_to(&p, &b).unwrap().unwrap();
        assert_eq!(fs::read_dir(b.join("resources")).unwrap().count(), 1);
        assert!(fs::read_to_string(&new).unwrap().contains("](resources/x.png)"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn different_picture_same_name_renamed() {
        let (d, a, b) = notebook("pic-diff");
        let p = page_with(&a, "P", "![](resources/x.png)");
        version_of(&p, "old ![](resources/x.png)");
        pic(&a, "x.png", b"MINE");
        pic(&b, "x.png", b"THEIRS");
        let new = move_page_to(&p, &b).unwrap().unwrap();
        assert_eq!(fs::read(b.join("resources").join("x.png")).unwrap(), b"THEIRS");
        assert_eq!(fs::read(b.join("resources").join("x-2.png")).unwrap(), b"MINE");
        assert!(fs::read_to_string(&new).unwrap().contains("](resources/x-2.png)"));
        let v = &crate::history::list_versions(&new)[0];
        assert!(fs::read_to_string(&v.path).unwrap().contains("](resources/x-2.png)"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn clash_reports_picture_renames() {
        let (d, a, b) = notebook("pic-diff-report");
        let p = page_with(&a, "P", "![](resources/x.png)");
        pic(&a, "x.png", b"MINE");
        pic(&b, "x.png", b"THEIRS");
        let moved = move_page(&p, &b, None).unwrap().unwrap();
        assert_eq!(moved.pictures, vec![("x.png".to_string(), "x-2.png".to_string())]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn exact_back_restores_picture_names() {
        let (d, a, b) = notebook("exact-back");
        let p = page_with(&a, "P", "![](resources/x.png)");
        version_of(&p, "old ![](resources/x.png)");
        pic(&a, "x.png", b"MINE");
        pic(&b, "x.png", b"THEIRS");
        let new = move_page(&p, &b, None).unwrap().unwrap().path;
        let exact = Exact { stem: "P".into(), ext: "md".into(), pictures: vec![("x-2.png".into(), "x.png".into())], keep: Vec::new() };
        let back = move_page(&new, &a, Some(&exact)).unwrap().unwrap();
        assert_eq!(back.path, a.join("P.md"));
        assert!(fs::read_to_string(&back.path).unwrap().contains("](resources/x.png)"));
        let v = &crate::history::list_versions(&back.path)[0];
        assert!(fs::read_to_string(&v.path).unwrap().contains("](resources/x.png)"));
        assert_eq!(fs::read(a.join("resources").join("x.png")).unwrap(), b"MINE");
        assert!(!b.join("resources").join("x-2.png").exists());
        assert_eq!(fs::read(b.join("resources").join("x.png")).unwrap(), b"THEIRS");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn exact_back_gets_old_stem() {
        let (d, a, b) = notebook("exact-stem");
        let p = page_with(&a, "P", "text");
        page_with(&b, "P", "other");
        let new = move_page(&p, &b, None).unwrap().unwrap().path;
        assert_eq!(new, b.join("P (2).md"));
        let exact = Exact { stem: "P".into(), ext: "md".into(), pictures: Vec::new(), keep: Vec::new() };
        let back = move_page(&new, &a, Some(&exact)).unwrap().unwrap();
        assert_eq!(back.path, a.join("P.md"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn chained_clash_renames_rewrite_once() {
        let (d, a, b) = notebook("chain");
        let p = page_with(&a, "P", "![](resources/x.png) ![](resources/x-2.png)");
        version_of(&p, "old ![](resources/x.png) ![](resources/x-2.png)");
        pic(&a, "x.png", b"MINE");
        pic(&a, "x-2.png", b"MINE2");
        pic(&b, "x.png", b"THEIRS");
        let moved = move_page(&p, &b, None).unwrap().unwrap();
        let text = fs::read_to_string(&moved.path).unwrap();
        assert_eq!(picture_links(&text), vec!["x-2.png".to_string(), "x-2-2.png".to_string()], "{text}");
        let v = &crate::history::list_versions(&moved.path)[0];
        assert_eq!(picture_links(&fs::read_to_string(&v.path).unwrap()), vec!["x-2.png".to_string(), "x-2-2.png".to_string()]);
        let res = b.join("resources");
        assert_eq!(fs::read(res.join("x.png")).unwrap(), b"THEIRS");
        assert_eq!(fs::read(res.join("x-2.png")).unwrap(), b"MINE");
        assert_eq!(fs::read(res.join("x-2-2.png")).unwrap(), b"MINE2");
        // Undo: the exact move back gives the original names.
        let exact = Exact { stem: "P".into(), ext: "md".into(), pictures: crate::undo::invert(&moved.pictures), keep: Vec::new() };
        let back = move_page(&moved.path, &a, Some(&exact)).unwrap().unwrap();
        let text = fs::read_to_string(&back.path).unwrap();
        assert_eq!(picture_links(&text), vec!["x.png".to_string(), "x-2.png".to_string()], "{text}");
        assert_eq!(fs::read(a.join("resources").join("x.png")).unwrap(), b"MINE");
        assert_eq!(fs::read(a.join("resources").join("x-2.png")).unwrap(), b"MINE2");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn exact_move_keeps_markdown_extension() {
        let (d, a, b) = notebook("exact-markdown");
        let p = a.join("X.markdown");
        fs::write(&p, "---
title: X
---

text
").unwrap();
        let new = move_page(&p, &b, None).unwrap().unwrap().path;
        assert_eq!(new, b.join("X.md"));
        let exact = Exact { stem: "X".into(), ext: "markdown".into(), pictures: Vec::new(), keep: Vec::new() };
        let back = move_page(&new, &a, Some(&exact)).unwrap().unwrap();
        assert_eq!(back.path, a.join("X.markdown"));
        assert!(back.path.is_file() && !a.join("X.md").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn markdown_page_takes_the_stem() {
        let (d, a, b) = notebook("md-stem");
        let p = page_with(&a, "P", "text");
        fs::write(b.join("P.markdown"), "---\ntitle: P\n---\n\nother\n").unwrap();
        assert_eq!(free_stem(&b, "P"), "P (2)");
        let exact = Exact { stem: "P".into(), ext: "md".into(), pictures: Vec::new(), keep: Vec::new() };
        let err = move_page(&p, &b, Some(&exact)).err().unwrap();
        assert!(err.contains("already has a page named \"P\""), "{err}");
        assert_eq!(move_page(&p, &b, None).unwrap().unwrap().path, b.join("P (2).md"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn undo_keeps_picture_that_was_there_before() {
        let (d, a, b) = notebook("kept-pic");
        let p = page_with(&a, "P", "![](resources/x.png)");
        pic(&a, "x.png", b"SAME");
        pic(&b, "x.png", b"SAME");
        let moved = move_page(&p, &b, None).unwrap().unwrap();
        assert_eq!(moved.kept, vec!["x.png".to_string()]);
        assert!(!a.join("resources").join("x.png").exists());
        let exact = Exact { stem: "P".into(), ext: "md".into(), pictures: Vec::new(), keep: moved.kept.clone() };
        let back = move_page(&moved.path, &a, Some(&exact)).unwrap().unwrap();
        assert_eq!(back.path, a.join("P.md"));
        assert_eq!(fs::read(a.join("resources").join("x.png")).unwrap(), b"SAME");
        assert_eq!(fs::read(b.join("resources").join("x.png")).unwrap(), b"SAME");
        let _ = fs::remove_dir_all(&d);
    }

    /// All files below `dir`, sorted.
    fn listing(dir: &Path) -> Vec<PathBuf> {
        let mut v = files_under(dir);
        v.sort();
        v
    }

    #[test]
    fn exact_refuses_taken_stem() {
        let (d, a, b) = notebook("exact-taken");
        let p = page_with(&a, "P", "![](resources/x.png)");
        pic(&a, "x.png", b"MINE");
        page_with(&b, "P", "other");
        let (before_a, before_b) = (listing(&a), listing(&b));
        let exact = Exact { stem: "P".into(), ext: "md".into(), pictures: Vec::new(), keep: Vec::new() };
        let err = move_page(&p, &b, Some(&exact)).err().unwrap();
        assert!(err.contains("already has a page named \"P\""), "{err}");
        assert_eq!((listing(&a), listing(&b)), (before_a, before_b));
        assert_eq!(fs::read(a.join("resources").join("x.png")).unwrap(), b"MINE");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn exact_refuses_other_picture_bytes() {
        let (d, a, b) = notebook("exact-pic");
        let p = page_with(&a, "P", "![](resources/x-2.png)");
        pic(&a, "x-2.png", b"MINE");
        pic(&b, "x.png", b"THEIRS");
        let (before_a, before_b) = (listing(&a), listing(&b));
        let exact = Exact { stem: "P".into(), ext: "md".into(), pictures: vec![("x-2.png".into(), "x.png".into())], keep: Vec::new() };
        let err = move_page(&p, &b, Some(&exact)).err().unwrap();
        assert!(err.contains("already has a different picture named \"x.png\""), "{err}");
        assert_eq!((listing(&a), listing(&b)), (before_a, before_b));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn section_exact_name() {
        let (d, a, b) = notebook("sec-exact");
        fs::create_dir_all(b.join("A")).unwrap();
        page_with(&a, "P", "text");
        let err = move_section(&a, &b, Some("A")).err().unwrap();
        assert!(err.contains("already has"), "{err}");
        assert!(a.join("P.md").is_file());
        let moved = move_section(&a, &b, Some("Z")).unwrap().unwrap();
        assert_eq!(moved.path, b.join("Z"));
        assert!(b.join("Z").join("P.md").is_file());
        assert!(moved.pictures.is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn move_pasted_picture_with_spaces() {
        let (d, a, b) = notebook("pic-spaces");
        let p = page_with(&a, "P", "![](resources/P%202026.png)");
        let orig = pic(&a, "P 2026.png", b"X");
        move_page_to(&p, &b).unwrap();
        assert!(!orig.exists() && b.join("resources").join("P 2026.png").is_file());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn move_editor_picture_with_spaces() {
        let (d, a, b) = notebook("pic-angle");
        let p = page_with(&a, "P", "![](<resources/P 2026 (2).png>)");
        let orig = pic(&a, "P 2026 (2).png", b"X");
        let new = move_page_to(&p, &b).unwrap().unwrap();
        assert!(!orig.exists() && b.join("resources").join("P 2026 (2).png").is_file());
        assert!(fs::read_to_string(&new).unwrap().contains("](<resources/P 2026 (2).png>)"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn keeps_picture_linked_in_angle_form_by_sibling() {
        let (d, a, b) = notebook("pic-angle-sibling");
        let p = page_with(&a, "P", "![](resources/P%202026.png)");
        page_with(&a, "Q", "![](<resources/P 2026.png>)");
        let orig = pic(&a, "P 2026.png", b"X");
        move_page_to(&p, &b).unwrap();
        assert!(orig.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn keeps_picture_linked_with_other_case() {
        let (d, a, b) = notebook("pic-case");
        let p = page_with(&a, "P", "![](resources/photo.png)");
        page_with(&a, "Q", "![](resources/Photo.PNG)");
        let orig = pic(&a, "photo.png", b"X");
        move_page_to(&p, &b).unwrap();
        assert!(orig.exists());
        let _ = fs::remove_dir_all(&d);
    }

    /// Opens a file so that nobody else may read or delete it until the handle is dropped.
    #[cfg(windows)]
    fn lock_hard(p: &Path) -> fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new().read(true).share_mode(0).open(p).unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn copy_then_remove_keeps_copy_when_removal_fails() {
        let d = temp("copy-remove-fails");
        let from = d.join("from");
        fs::create_dir_all(&from).unwrap();
        fs::write(from.join("a.md"), "A").unwrap();
        fs::write(from.join("b.md"), "B").unwrap();
        let to = d.join("to");
        // Readable but not deletable: open without delete sharing.
        let guard = {
            use std::os::windows::fs::OpenOptionsExt;
            fs::OpenOptions::new().read(true).share_mode(1).open(from.join("b.md")).unwrap()
        };
        // Only the locked file stays behind (its folder is not listed separately); the rest is gone.
        assert_eq!(copy_then_remove(&from, &to).unwrap(), vec![from.join("b.md")]);
        drop(guard);
        assert!(!from.join("a.md").exists());
        assert_eq!(fs::read_to_string(to.join("a.md")).unwrap(), "A");
        assert_eq!(fs::read_to_string(to.join("b.md")).unwrap(), "B");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn copy_then_remove_removes_read_only_originals() {
        let d = temp("copy-read-only");
        let from = d.join("from");
        fs::create_dir_all(&from).unwrap();
        let f = from.join("a.md");
        fs::write(&f, "A").unwrap();
        let mut perm = fs::metadata(&f).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&f, perm).unwrap();
        assert!(copy_then_remove(&from, &d.join("to")).unwrap().is_empty());
        assert!(!from.exists());
        assert_eq!(fs::read_to_string(d.join("to").join("a.md")).unwrap(), "A");
        // The copy keeps the attribute; clear it so the folder can be cleaned up.
        let c = d.join("to").join("a.md");
        let mut perm = fs::metadata(&c).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        let _ = fs::set_permissions(&c, perm);
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn copy_then_remove_cleans_partial_copy() {
        let d = temp("copy-fails");
        let from = d.join("from");
        fs::create_dir_all(&from).unwrap();
        fs::write(from.join("a.md"), "A").unwrap();
        fs::write(from.join("b.md"), "B").unwrap();
        let to = d.join("to");
        let guard = lock_hard(&from.join("b.md"));
        assert!(copy_then_remove(&from, &to).is_err());
        drop(guard);
        assert!(!to.exists());
        assert!(from.join("a.md").is_file() && from.join("b.md").is_file());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn copy_back_over_leftovers_keeps_them() {
        // The undo of a cross-drive move: the original folder still holds a file that stayed behind.
        let d = temp("copy-back");
        let (moved, orig) = (d.join("moved"), d.join("orig"));
        fs::create_dir_all(&moved).unwrap();
        fs::create_dir_all(&orig).unwrap();
        fs::write(moved.join("a.md"), "A").unwrap();
        fs::write(moved.join("x.md"), "X").unwrap();
        fs::write(orig.join("x.md"), "X").unwrap();
        assert!(copy_then_remove(&moved, &orig).unwrap().is_empty());
        assert!(!moved.exists());
        assert_eq!(fs::read_to_string(orig.join("a.md")).unwrap(), "A");
        assert_eq!(fs::read_to_string(orig.join("x.md")).unwrap(), "X");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn failed_copy_back_never_removes_what_was_there() {
        let d = temp("copy-back-fails");
        let (moved, orig) = (d.join("moved"), d.join("orig"));
        fs::create_dir_all(&moved).unwrap();
        fs::create_dir_all(&orig).unwrap();
        fs::write(moved.join("a.md"), "A").unwrap();
        fs::write(moved.join("x.md"), "NEW").unwrap();
        fs::write(orig.join("x.md"), "OLD").unwrap();
        fs::write(orig.join("keep.md"), "K").unwrap();
        assert!(copy_then_remove(&moved, &orig).is_err());
        // The original folder is as it was; the moved copy is untouched.
        assert_eq!(fs::read_to_string(orig.join("x.md")).unwrap(), "OLD");
        assert_eq!(fs::read_to_string(orig.join("keep.md")).unwrap(), "K");
        assert!(!orig.join("a.md").exists());
        assert!(moved.join("a.md").is_file() && moved.join("x.md").is_file());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn undo_reports_what_it_could_not_do() {
        let d = temp("undo-report");
        // Nothing at the new place to move back.
        let msg = undo_moves(&[(d.join("from.md"), d.join("gone.md"))], "First error".into());
        assert!(msg.starts_with("First error"));
        assert!(msg.contains("could not be undone") && msg.contains("gone.md"));
        // A successful undo adds nothing.
        fs::write(d.join("to.md"), "T").unwrap();
        assert_eq!(undo_moves(&[(d.join("back.md"), d.join("to.md"))], "E".into()), "E");
        assert!(d.join("back.md").is_file());
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn read_only_file_left_behind_stays_read_only() {
        let d = temp("read-only-left");
        let f = d.join("a.md");
        fs::write(&f, "A").unwrap();
        let mut perm = fs::metadata(&f).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&f, perm).unwrap();
        let guard = {
            use std::os::windows::fs::OpenOptionsExt;
            fs::OpenOptions::new().read(true).share_mode(1).open(&f).unwrap()
        };
        let mut left = Vec::new();
        remove_best_effort(&f, &mut left);
        drop(guard);
        assert_eq!(left, vec![f.clone()]);
        assert!(fs::metadata(&f).unwrap().permissions().readonly());
        let mut perm = fs::metadata(&f).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        let _ = fs::set_permissions(&f, perm);
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn picture_that_cannot_be_removed_is_reported() {
        let (d, a, b) = notebook("pic-locked");
        let p = page_with(&a, "P", "![](resources/x.png)");
        let orig = pic(&a, "x.png", b"X");
        let guard = {
            use std::os::windows::fs::OpenOptionsExt;
            fs::OpenOptions::new().read(true).share_mode(1).open(&orig).unwrap()
        };
        let moved = move_page(&p, &b, None).unwrap().unwrap();
        drop(guard);
        assert_eq!(moved.left_behind, vec![orig.clone()]);
        assert!(b.join("resources").join("x.png").is_file());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_picture_link_left_alone() {
        let (d, a, b) = notebook("pic-missing");
        let p = page_with(&a, "P", "![](resources/gone.png)");
        let new = move_page_to(&p, &b).unwrap().unwrap();
        assert!(fs::read_to_string(&new).unwrap().contains("](resources/gone.png)"));
        assert!(!b.join("resources").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn move_path_same_drive_renames() {
        let d = temp("move-path");
        fs::write(d.join("a.md"), "A").unwrap();
        move_path(&d.join("a.md"), &d.join("b.md")).unwrap();
        assert!(!d.join("a.md").exists());
        assert_eq!(fs::read_to_string(d.join("b.md")).unwrap(), "A");
        assert!(move_path(&d.join("missing.md"), &d.join("c.md")).is_err());
        let _ = fs::remove_dir_all(&d);
    }

    // --- sections ---

    #[test]
    fn section_to_other_section() {
        let (d, a, b) = notebook("sec-other");
        let p = page_with(&a, "P", "![](resources/x.png)");
        page_with(&a.join("P"), "S", "sub");
        pic(&a, "x.png", b"X");
        version_of(&p, "old");
        let new = move_section_to(&a, &b).unwrap().unwrap();
        assert_eq!(new, b.join("A"));
        assert!(new.join("P.md").is_file() && new.join("P").join("S.md").is_file());
        assert!(new.join("resources").join("x.png").is_file());
        assert_eq!(crate::history::list_versions(&new.join("P.md")).len(), 1);
        assert!(!a.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn section_to_notebook_root() {
        let d = temp("sec-root");
        let c = d.join("A").join("G").join("C");
        page_with(&c, "P", "x");
        let new = move_section_to(&c, &d).unwrap().unwrap();
        assert_eq!(new, d.join("C"));
        assert!(new.join("P.md").is_file() && !c.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn clash_with_folder_gets_suffix() {
        let (d, a, b) = notebook("sec-clash-folder");
        page_with(&a, "P", "x");
        fs::create_dir_all(b.join("A")).unwrap();
        let new = move_section_to(&a, &b).unwrap().unwrap();
        assert_eq!(new, b.join("A (2)"));
        assert!(new.join("P.md").is_file() && b.join("A").is_dir());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn clash_with_page_gets_suffix() {
        let (d, a, b) = notebook("sec-clash-page");
        page_with(&a, "P", "x");
        let other = page_with(&b, "A", "keep");
        let new = move_section_to(&a, &b).unwrap().unwrap();
        assert_eq!(new, b.join("A (2)"));
        assert!(!b.join("A").exists());
        assert!(fs::read_to_string(&other).unwrap().contains("keep"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn same_parent_is_noop() {
        let (d, a, _) = notebook("sec-noop");
        assert!(!can_move_section_to(&a, &d));
        assert_eq!(move_section_to(&a, &d).unwrap(), None);
        assert!(a.is_dir());
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn same_parent_other_case_is_noop() {
        let (d, a, _) = notebook("sec-noop-case");
        let upper = PathBuf::from(d.to_string_lossy().to_uppercase());
        assert!(!can_move_section_to(&a, &upper));
        assert_eq!(move_section_to(&a, &upper).unwrap(), None);
        assert!(a.is_dir());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_into_itself() {
        let (d, a, _) = notebook("sec-self");
        assert!(!can_move_section_to(&a, &a));
        assert!(move_section_to(&a, &a).is_err());
        assert!(a.is_dir());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_into_descendant() {
        let (d, a, _) = notebook("sec-desc");
        let sub = a.join("Sub");
        fs::create_dir_all(&sub).unwrap();
        assert!(!can_move_section_to(&a, &sub));
        assert!(move_section_to(&a, &sub).is_err());
        assert!(sub.is_dir() && !sub.join("A").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn sibling_with_prefix_name_is_allowed() {
        let (d, a, _) = notebook("sec-prefix");
        let ab = d.join("AB");
        fs::create_dir_all(&ab).unwrap();
        assert!(can_move_section_to(&a, &ab));
        assert_eq!(move_section_to(&a, &ab).unwrap().unwrap(), ab.join("A"));
        let _ = fs::remove_dir_all(&d);
    }
}
