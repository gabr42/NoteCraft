//! Moving a page (with its subpages, versions and pictures) to another folder.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

/// `](resources/x)`, `src="resources/x"`, `src='resources/x'` and `<resources/x>`.
static LINK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(\]\(|src="|src='|<)resources/([^)"'>\s]+)"#).expect("mover: link pattern"));

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
        let name = decode(&c[2]);
        if !out.contains(&name) {
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

/// Points every link to the picture `old` at `new` instead.
pub(crate) fn rewrite_picture_link(text: &str, old: &str, new: &str) -> String {
    LINK.replace_all(text, |c: &regex::Captures| {
        if decode(&c[2]) == old { format!("{}resources/{}", &c[1], encode_name(new)) } else { c[0].to_string() }
    })
    .into_owned()
}

/// The first of `stem`, `stem (2)`, `stem (3)` … that is free in `dir` as a page, as a subpage
/// folder and as a versions folder.
pub(crate) fn free_stem(dir: &Path, stem: &str) -> String {
    let mut n = 1;
    loop {
        let s = if n == 1 { stem.to_string() } else { format!("{stem} ({n})") };
        let page = dir.join(format!("{s}.md"));
        if !page.exists() && !dir.join(&s).exists() && !crate::history::versions_dir(&page).exists() {
            return s;
        }
        n += 1;
    }
}

fn drive_of(p: &Path) -> String {
    match p.components().next() {
        Some(Component::Prefix(x)) => x.as_os_str().to_string_lossy().to_lowercase(),
        _ => String::new(),
    }
}

/// Copies a file or a whole folder to `to`, then removes the original.
fn copy_then_remove(from: &Path, to: &Path) -> std::io::Result<()> {
    fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
        if from.is_dir() {
            fs::create_dir_all(to)?;
            for e in fs::read_dir(from)? {
                let e = e?;
                copy_tree(&e.path(), &to.join(e.file_name()))?;
            }
        } else {
            if let Some(p) = to.parent() {
                fs::create_dir_all(p)?;
            }
            fs::copy(from, to)?;
        }
        Ok(())
    }
    copy_tree(from, to)?;
    if from.is_dir() { fs::remove_dir_all(from) } else { fs::remove_file(from) }
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

/// Whether `p` is `dir` or lies below it (case-insensitive).
fn is_within(p: &Path, dir: &Path) -> bool {
    let (p, d) = (crate::store::path_key(p), crate::store::path_key(dir));
    p.len() >= d.len() && p[..d.len()] == d[..]
}

/// Whether `page` may be moved into `target_dir`: not its own folder, not its subpage folder or below.
pub fn can_move_to(page: &Path, target_dir: &Path) -> bool {
    let Some(dir) = page.parent() else { return false };
    crate::store::path_key(dir) != crate::store::path_key(target_dir)
        && !is_within(target_dir, &crate::store::subpage_dir(page))
}

/// `x.png` -> first free of `x-2.png`, `x-3.png` … in `dir`.
fn free_picture_name(dir: &Path, name: &str) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut k = 2;
    loop {
        let n = format!("{stem}-{k}{ext}");
        if !dir.join(&n).exists() {
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
pub fn move_page(page: &Path, target_dir: &Path) -> Result<Option<PathBuf>, String> {
    use crate::history::versions_dir;
    use crate::store::{file_stem, subpage_dir};
    let src_dir = page.parent().ok_or("The page has no folder.")?;
    if crate::store::path_key(src_dir) == crate::store::path_key(target_dir) {
        return Ok(None);
    }
    if !can_move_to(page, target_dir) {
        return Err("A page cannot be moved under itself.".into());
    }
    fs::create_dir_all(target_dir).map_err(|e| format!("Could not create {}: {e}", target_dir.display()))?;
    let new = target_dir.join(format!("{}.md", free_stem(target_dir, &file_stem(page))));

    // 1. Copy the pictures the page and its versions link.
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
    let mut copied: Vec<String> = Vec::new();
    let mut renames: Vec<(String, String)> = Vec::new();
    for n in names {
        let src = src_res.join(&n);
        if !src.is_file() {
            continue;
        }
        let mut dst = dst_res.join(&n);
        if dst.exists() {
            if fs::read(&src).ok() == fs::read(&dst).ok() {
                copied.push(n);
                continue;
            }
            let other = free_picture_name(&dst_res, &n);
            dst = dst_res.join(&other);
            renames.push((n.clone(), other));
        }
        fs::create_dir_all(&dst_res)
            .and_then(|_| fs::copy(&src, &dst))
            .map_err(|e| format!("Could not copy the picture {n} to {}: {e}", dst_res.display()))?;
        copied.push(n);
    }

    // 2. Move the page, its subpages and its versions; undo on failure.
    let mut moves = vec![(page.to_path_buf(), new.clone())];
    if subpage_dir(page).is_dir() {
        moves.push((subpage_dir(page), subpage_dir(&new)));
    }
    if versions.is_dir() {
        moves.push((versions, versions_dir(&new)));
    }
    for (i, (from, to)) in moves.iter().enumerate() {
        if let Some(p) = to.parent() {
            let _ = fs::create_dir_all(p);
        }
        if let Err(e) = move_path(from, to) {
            for (f, t) in moves[..i].iter().rev() {
                let _ = move_path(t, f);
            }
            return Err(e);
        }
    }

    // 3. Point links at renamed pictures.
    if !renames.is_empty() {
        for f in std::iter::once(new.clone()).chain(files_under(&versions_dir(&new))) {
            if let Ok(text) = fs::read_to_string(&f) {
                let mut t = text.clone();
                for (o, n) in &renames {
                    t = rewrite_picture_link(&t, o, n);
                }
                if t != text {
                    let _ = fs::write(&f, t);
                }
            }
        }
    }

    // 4. Remove originals nothing else needs.
    if let Some(linked) = names_still_linked(src_dir) {
        for n in copied {
            if !linked.contains(&n) {
                let _ = fs::remove_file(src_res.join(&n));
            }
        }
    }
    Ok(Some(new))
}

/// Moves a file or folder; across drives by copying and removing the original.
pub(crate) fn move_path(from: &Path, to: &Path) -> Result<(), String> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
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
        copy_then_remove(&from, &to).unwrap();
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
        let new = move_page(&p, &b).unwrap().unwrap();
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
        assert_eq!(move_page(&s, &a).unwrap().unwrap(), a.join("S.md"));
        assert!(!s.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn under_other_page() {
        let (d, a, _) = notebook("under");
        let p = page_with(&a, "A1", "x");
        let t = page_with(&a, "B1", "y");
        let new = move_page(&p, &crate::store::subpage_dir(&t)).unwrap().unwrap();
        assert_eq!(new, a.join("B1").join("A1.md"));
        assert!(new.is_file() && !p.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn same_folder_is_noop() {
        let (d, a, _) = notebook("noop");
        let p = page_with(&a, "P", "x");
        let before = fs::metadata(&p).unwrap().modified().unwrap();
        assert_eq!(move_page(&p, &a).unwrap(), None);
        assert_eq!(fs::metadata(&p).unwrap().modified().unwrap(), before);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_under_itself() {
        let (d, a, _) = notebook("itself");
        let p = page_with(&a, "P", "x");
        page_with(&a.join("P"), "S", "");
        let sub = crate::store::subpage_dir(&p);
        assert!(move_page(&p, &sub).is_err());
        assert!(move_page(&p, &sub.join("S")).is_err());
        assert!(!can_move_to(&p, &sub) && !can_move_to(&p, &sub.join("S")) && !can_move_to(&p, &a));
        assert!(p.is_file() && sub.join("S.md").is_file());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn name_clash_gets_suffix() {
        let (d, a, b) = notebook("clash");
        let p = page_with(&a, "P", "mine");
        page_with(&b, "P", "theirs");
        let new = move_page(&p, &b).unwrap().unwrap();
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
        let new = move_page(&p, &b).unwrap().unwrap();
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
        move_page(&p, &b).unwrap();
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
        move_page(&p, &b).unwrap();
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
        move_page(&p, &b).unwrap();
        assert!(orig.exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn identical_picture_not_duplicated() {
        let (d, a, b) = notebook("pic-same");
        let p = page_with(&a, "P", "![](resources/x.png)");
        pic(&a, "x.png", b"X");
        pic(&b, "x.png", b"X");
        let new = move_page(&p, &b).unwrap().unwrap();
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
        let new = move_page(&p, &b).unwrap().unwrap();
        assert_eq!(fs::read(b.join("resources").join("x.png")).unwrap(), b"THEIRS");
        assert_eq!(fs::read(b.join("resources").join("x-2.png")).unwrap(), b"MINE");
        assert!(fs::read_to_string(&new).unwrap().contains("](resources/x-2.png)"));
        let v = &crate::history::list_versions(&new)[0];
        assert!(fs::read_to_string(&v.path).unwrap().contains("](resources/x-2.png)"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn move_pasted_picture_with_spaces() {
        let (d, a, b) = notebook("pic-spaces");
        let p = page_with(&a, "P", "![](resources/P%202026.png)");
        let orig = pic(&a, "P 2026.png", b"X");
        move_page(&p, &b).unwrap();
        assert!(!orig.exists() && b.join("resources").join("P 2026.png").is_file());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_picture_link_left_alone() {
        let (d, a, b) = notebook("pic-missing");
        let p = page_with(&a, "P", "![](resources/gone.png)");
        let new = move_page(&p, &b).unwrap().unwrap();
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
}
