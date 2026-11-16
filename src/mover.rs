//! Moving a page (with its subpages, versions and pictures) to another folder.

use std::fs;
use std::path::{Component, Path};
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
