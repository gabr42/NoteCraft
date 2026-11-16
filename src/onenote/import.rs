//! The import driver: walks a OneNote notebook folder, converts every section and writes the
//! pages as Markdown files. Port of `tools\onenote-import\run_import.py`, plus history snapshots
//! and lock checks for pages that are replaced.

use super::convert::{Converter, NameAllocator, UNTITLED_PAGE, page_markdown, safe};
use super::parse::OneFile;
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// What the import has done so far; also the final result.
#[derive(Default)]
pub struct Report {
    pub sections: usize,
    pub pages_written: usize,
    pub pages_replaced: usize,
    pub pages_unchanged: usize,
    pub resources_written: usize,
    pub problems: Vec<String>,
    pub cancelled: bool,
    /// Sections that could not be imported at all (read or parse failure); also in `problems`.
    pub sections_failed: usize,
}

impl Report {
    /// Multi-line text for the wizard and the command line.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        if self.cancelled {
            s.push_str("The import was cancelled.\n");
        }
        s.push_str(&format!("Sections imported: {}\n", self.sections));
        if self.sections_failed > 0 {
            s.push_str(&format!("Sections that failed: {}\n", self.sections_failed));
        }
        s.push_str(&format!(
            "Pages: {} new, {} replaced, {} unchanged\n",
            self.pages_written, self.pages_replaced, self.pages_unchanged
        ));
        s.push_str(&format!("Pictures and attachments written: {}\n", self.resources_written));
        if !self.problems.is_empty() {
            s.push_str(&format!("Problems ({}):\n", self.problems.len()));
            for p in &self.problems {
                s.push_str("  ");
                s.push_str(p);
                s.push('\n');
            }
        }
        s
    }

    fn copy(&self) -> Report {
        Report {
            sections: self.sections,
            pages_written: self.pages_written,
            pages_replaced: self.pages_replaced,
            pages_unchanged: self.pages_unchanged,
            resources_written: self.resources_written,
            problems: self.problems.clone(),
            cancelled: self.cancelled,
            sections_failed: self.sections_failed,
        }
    }
}

pub enum Progress {
    /// About to import section `index` (0-based) of `count`; `place` is its path in the notebook.
    Section { index: usize, count: usize, place: String },
    Done(Report),
}

fn is_section_file(name: &str) -> bool {
    name.len() > 4 && name.get(name.len() - 4..).is_some_and(|e| e.eq_ignore_ascii_case(".one"))
}

/// The `.one` files below `src`: per folder first its own files (sorted), then the subfolders in
/// sorted order, like `os.walk` + `sorted`; `OneNote_RecycleBin` is skipped.
pub fn find_sections(src: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(src, &mut out);
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut dirs: Vec<(String, PathBuf)> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            if name != "OneNote_RecycleBin" {
                dirs.push((name, e.path()));
            }
        } else if is_section_file(&name) {
            files.push((name, e.path()));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    out.extend(files.into_iter().map(|f| f.1));
    for (_, d) in dirs {
        walk(&d, out);
    }
}

/// `<target>\<name of the source folder>`
pub fn notebook_dir(src: &Path, target: &Path) -> PathBuf {
    let name = src.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "Notebook".into());
    target.join(name)
}

/// Absolute (a relative path is taken relative to `base`) with `.` and `..` removed from the text,
/// then with links resolved as far as the path exists, lower case for comparing. `None` when no
/// absolute form can be made: the caller must refuse such a path.
fn resolved(base: &Path, p: &Path) -> Option<PathBuf> {
    let joined = if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
    let p = std::path::absolute(&joined).ok()?;
    if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return None;
    }
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = p.clone();
    let mut base = loop {
        if let Ok(c) = cur.canonicalize() {
            break c;
        }
        match (cur.file_name().map(|n| n.to_os_string()), cur.parent().map(|x| x.to_path_buf())) {
            (Some(n), Some(parent)) => {
                rest.push(n);
                cur = parent;
            }
            // Nothing of the path exists, not even its root: use the normalised absolute form.
            _ => break cur,
        }
    };
    for n in rest.iter().rev() {
        base.push(n);
    }
    Some(PathBuf::from(base.to_string_lossy().to_lowercase()))
}

/// The import must never write into the source: refuse a target that is the source, lies inside
/// it, or would put the notebook folder there, and a source that lies inside the notebook folder.
pub fn check_target(src: &Path, target: &Path) -> Result<(), String> {
    let base = std::env::current_dir().map_err(|e| format!("onenote::check_target: no current folder: {e}"))?;
    check_target_in(&base, src, target)
}

/// `check_target` with the folder that relative paths are taken from.
fn check_target_in(base: &Path, src: &Path, target: &Path) -> Result<(), String> {
    let refuse = || {
        Err(format!(
            "onenote::check_target: the target folder {} is the source folder or lies inside it",
            target.display()
        ))
    };
    let nb = notebook_dir(src, target);
    let (Some(s), Some(t), Some(n)) = (resolved(base, src), resolved(base, target), resolved(base, &nb)) else {
        return Err(format!(
            "onenote::check_target: cannot tell whether the target folder {} lies inside the source",
            target.display()
        ));
    };
    if t.starts_with(&s) || n.starts_with(&s) || s.starts_with(&n) {
        return refuse();
    }
    Ok(())
}

/// Writes `(file stem, markdown)` pairs into `dir`. A page whose text is unchanged is left alone;
/// a page that is replaced is first kept in the history; a page that is locked by another
/// computer or cannot be kept in the history is skipped and listed, and so is a page whose existing
/// file cannot be read. Returns false (and lists the problem) when the folder cannot be created.
pub fn write_section(dir: &Path, pages: &[(String, String)], report: &mut Report) -> bool {
    if let Err(e) = std::fs::create_dir_all(dir) {
        report.problems.push(format!("{}: could not create the folder: {e}", dir.display()));
        return false;
    }
    for (stem, md) in pages {
        let path = dir.join(format!("{stem}.md"));
        let exists = path.is_file();
        if exists {
            match std::fs::read(&path) {
                Ok(old) if old == md.as_bytes() => {
                    report.pages_unchanged += 1;
                    continue;
                }
                Ok(_) => {}
                Err(e) => {
                    report.problems.push(format!("{}: could not read the existing page: {e}", path.display()));
                    continue;
                }
            }
            if let Some(h) = crate::lock::held_by_other(&path) {
                report.problems.push(format!("{}: skipped, the page is being edited {}", path.display(), h.describe()));
                continue;
            }
            if let Err(e) = crate::history::snapshot_now(&path, chrono::Local::now().naive_local()) {
                report.problems.push(format!("{}: kept the old page, {e}", path.display()));
                continue;
            }
        }
        match std::fs::write(&path, md.as_bytes()) {
            Ok(()) if exists => report.pages_replaced += 1,
            Ok(()) => report.pages_written += 1,
            Err(e) => report.problems.push(format!("{}: could not write the page: {e}", path.display())),
        }
    }
    true
}

/// Only `.` and ASCII letters and digits, at most 10 characters after the dot; empty when nothing
/// usable remains. The text comes from a user's attachment file name.
fn clean_extension(ext: &str) -> String {
    let kept: String = ext.chars().filter(|c| *c == '.' || c.is_ascii_alphanumeric()).collect();
    let kept: String = kept.trim_matches('.').chars().take(10).collect();
    let kept = kept.trim_end_matches('.');
    if kept.is_empty() { String::new() } else { format!(".{kept}") }
}

fn plain_name(name: &str) -> String {
    safe(&name.replace("^J", " ").replace(['\r', '\n'], " "))
}

/// Converts the notebook folder `src` into `<target>\<notebook>`. Synchronous; the caller puts it
/// on a thread. `progress` gets a line per section and the report at the end.
pub fn import(src: &Path, target: &Path, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Report {
    let mut report = Report::default();
    if let Err(e) = check_target(src, target) {
        report.problems.push(e);
        report.sections_failed += 1;
        progress(Progress::Done(report.copy()));
        return report;
    }
    let base = notebook_dir(src, target);
    let files = find_sections(src);
    let count = files.len();
    // One name allocator per output folder, shared by section and group folders.
    let mut allocators: HashMap<PathBuf, NameAllocator> = HashMap::new();
    // Output folder of every source folder that has been seen.
    let mut out_dirs: HashMap<PathBuf, PathBuf> = HashMap::new();
    out_dirs.insert(src.to_path_buf(), base.clone());

    for (index, file) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let rel = file.strip_prefix(src).unwrap_or(file);
        let place = rel.to_string_lossy().to_string();
        progress(Progress::Section { index, count, place: place.clone() });

        // The output folder of the section's parent folder, allocating group names on the way.
        let mut src_dir = src.to_path_buf();
        let mut out_dir = base.clone();
        let parents: Vec<_> = rel.parent().map(|p| p.components().collect()).unwrap_or_default();
        for c in parents {
            src_dir.push(c);
            out_dir = match out_dirs.get(&src_dir) {
                Some(o) => o.clone(),
                None => {
                    let name = allocators
                        .entry(out_dir.clone())
                        .or_insert_with(NameAllocator::folders)
                        .take(&plain_name(&c.as_os_str().to_string_lossy()));
                    let o = out_dir.join(name);
                    out_dirs.insert(src_dir.clone(), o.clone());
                    o
                }
            };
        }
        let file_name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let stem = &file_name[..file_name.len() - 4];
        let sec_name = allocators.entry(out_dir.clone()).or_insert_with(NameAllocator::folders).take(&plain_name(stem));
        let sec_dir = out_dir.join(sec_name);

        let data = match std::fs::read(file) {
            Ok(d) => d,
            Err(e) => {
                report.problems.push(format!("{place}: {e}"));
                report.sections_failed += 1;
                continue;
            }
        };
        let one = match OneFile::parse(&data) {
            Ok(o) => o,
            Err(e) => {
                report.problems.push(format!("{place}: {e}"));
                report.sections_failed += 1;
                continue;
            }
        };
        drop(data);

        let res_dir = sec_dir.join("resources");
        let mut new_resources = 0usize;
        let mut res_errors: Vec<String> = Vec::new();
        let converted = {
            let mut add = |bytes: &[u8], ext: &str| -> String {
                let name = format!("{:x}{}", Md5::digest(bytes), clean_extension(ext));
                let p = res_dir.join(&name);
                if !p.exists() {
                    match std::fs::create_dir_all(&res_dir).and_then(|_| std::fs::write(&p, bytes)) {
                        Ok(()) => new_resources += 1,
                        Err(e) => res_errors.push(format!("{place}: could not write {name}: {e}")),
                    }
                }
                name
            };
            let mut conv = Converter::new(&one, &mut add);
            conv.section_pages().map(|pages| {
                let mut alloc = NameAllocator::new();
                let list: Vec<(String, String)> = pages
                    .iter()
                    .map(|p| {
                        let title = if p.title.is_empty() { UNTITLED_PAGE } else { p.title.as_str() };
                        (alloc.take(&safe(title)), page_markdown(p))
                    })
                    .collect();
                (list, std::mem::take(&mut conv.warnings))
            })
        };
        report.resources_written += new_resources;
        report.problems.extend(res_errors);
        match converted {
            Ok((list, warnings)) => {
                report.problems.extend(warnings.into_iter().map(|w| format!("{place} › {w}")));
                if write_section(&sec_dir, &list, &mut report) {
                    report.sections += 1;
                } else {
                    report.sections_failed += 1;
                }
            }
            Err(e) => {
                report.problems.push(format!("{place}: {e}"));
                report.sections_failed += 1;
            }
        }
    }
    progress(Progress::Done(report.copy()));
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("notecraft-import-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn pages(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn write_new_pages() {
        let d = temp("new");
        let mut r = Report::default();
        write_section(&d.join("sec"), &pages(&[("A", "one"), ("B", "two")]), &mut r);
        assert_eq!(r.pages_written, 2);
        assert_eq!(fs::read_to_string(d.join("sec").join("B.md")).unwrap(), "two");
        assert!(r.problems.is_empty());
    }

    #[test]
    fn replace_keeps_history() {
        let d = temp("replace");
        let page = d.join("A.md");
        fs::write(&page, "old").unwrap();
        // A very young version exists already: the import keeps the old text anyway.
        crate::history::snapshot_now(&page, chrono::Local::now().naive_local()).unwrap();
        fs::write(&page, "older still").unwrap();
        let before = crate::history::list_versions(&page).len();
        let mut r = Report::default();
        write_section(&d, &pages(&[("A", "new")]), &mut r);
        assert_eq!(r.pages_replaced, 1);
        assert_eq!(fs::read_to_string(&page).unwrap(), "new");
        let v = crate::history::list_versions(&page);
        assert_eq!(v.len(), before + 1);
        assert!(v.iter().any(|x| fs::read_to_string(&x.path).unwrap() == "older still"));
    }

    #[test]
    fn replace_with_no_version_makes_one() {
        let d = temp("replace1");
        let page = d.join("A.md");
        fs::write(&page, "old").unwrap();
        let mut r = Report::default();
        write_section(&d, &pages(&[("A", "new")]), &mut r);
        let v = crate::history::list_versions(&page);
        assert_eq!(v.len(), 1);
        assert_eq!(fs::read_to_string(&v[0].path).unwrap(), "old");
        assert_eq!(r.pages_replaced, 1);
    }

    #[test]
    fn reimport_unchanged_writes_nothing() {
        let d = temp("same");
        let mut r = Report::default();
        write_section(&d, &pages(&[("A", "text")]), &mut r);
        let page = d.join("A.md");
        let m1 = fs::metadata(&page).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let mut r2 = Report::default();
        write_section(&d, &pages(&[("A", "text")]), &mut r2);
        assert_eq!(r2.pages_unchanged, 1);
        assert_eq!(r2.pages_written + r2.pages_replaced, 0);
        assert_eq!(fs::metadata(&page).unwrap().modified().unwrap(), m1);
        assert!(crate::history::list_versions(&page).is_empty());
    }

    #[test]
    fn locked_page_is_skipped() {
        let d = temp("locked");
        let page = d.join("A.md");
        fs::write(&page, "old").unwrap();
        fs::write(
            crate::lock::lock_path(&page),
            "computer=OTHERPC\nuser=somebody\nsince=1.1.2026 10:00\nsession=other-session\n",
        )
        .unwrap();
        let mut r = Report::default();
        write_section(&d, &pages(&[("A", "new")]), &mut r);
        assert_eq!(fs::read_to_string(&page).unwrap(), "old");
        assert_eq!(r.pages_replaced, 0);
        assert_eq!(r.problems.len(), 1);
        assert!(r.problems[0].contains("A.md"));
        assert!(r.problems[0].contains("OTHERPC"));
    }

    #[test]
    fn target_inside_source_is_refused() {
        let d = temp("target");
        let src = d.join("nb");
        fs::create_dir_all(&src).unwrap();
        assert!(check_target(&src, &src).is_err());
        assert!(check_target(&src, &src.join("sub")).is_err());
        assert!(check_target(&src, &d).is_err(), "the notebook folder would be the source itself");
        assert!(check_target(&src, &d.join("out")).is_ok());
    }

    #[test]
    fn target_with_dot_dot_is_normalised() {
        let d = temp("dotdot");
        let src = d.join("nb");
        fs::create_dir_all(&src).unwrap();
        let ends_in = d.join("nb").join("new1").join("new2").join("..");
        assert!(check_target(&src, &ends_in).is_err(), "ends in `..` below the source");
        let detour = d.join("newdir").join("..").join("nb").join("x");
        assert!(check_target(&src, &detour).is_err(), "`..` leads back into the source");
        let sibling = d.join("other").join("..").join("out");
        assert!(check_target(&src, &sibling).is_ok());
        // a path that exists nowhere is still compared in the same form
        assert!(check_target(&src, Path::new(r"Q:\no\such\drive\..\..\x")).is_ok());
    }

    #[test]
    fn source_inside_the_notebook_folder_is_refused() {
        let d = temp("srcinnb");
        // target d\T gives the notebook folder d\T\nb, which holds the source d\T\nb\nb
        let src = d.join("T").join("nb").join("nb");
        fs::create_dir_all(&src).unwrap();
        assert!(check_target(&src, &d.join("T")).is_err());
        assert!(check_target(&src, &d.join("U")).is_ok());
    }

    #[test]
    fn relative_target_is_resolved_against_the_current_folder() {
        let d = temp("relative");
        let src = d.join("nb");
        fs::create_dir_all(&src).unwrap();
        // Current folder is the source: `out` and `.` would be written into it.
        assert!(check_target_in(&src, &src, Path::new("out")).is_err());
        assert!(check_target_in(&src, &src, Path::new(".")).is_err());
        assert!(check_target_in(&src, Path::new("."), Path::new("out")).is_err());
        // Current folder is a sibling: `out` is fine, and so is `../out` from the source.
        assert!(check_target_in(&d, &src, Path::new("out")).is_ok());
        assert!(check_target_in(&src, &src, Path::new("../out")).is_ok());
        // The public function uses the real current folder; a relative nonexistent name is accepted
        // only when the process is not running in the source.
        assert!(check_target(&src, Path::new("notecraft-no-such-target-dir")).is_ok());
    }

    #[test]
    fn section_folder_that_cannot_be_created_is_a_failure() {
        let d = temp("nofolder");
        let mut r = Report::default();
        // `sec` is an existing file, so create_dir_all fails
        let f = d.join("sec");
        fs::write(&f, b"x").unwrap();
        assert!(!write_section(&f, &pages(&[("A", "one")]), &mut r));
        assert_eq!(r.problems.len(), 1);
        assert!(r.problems[0].contains("could not create the folder"));
        assert!(write_section(&d.join("ok"), &pages(&[("A", "one")]), &mut r));
    }

    #[test]
    fn hidden_looking_section_names_land_in_visible_folders() {
        let mut a = NameAllocator::folders();
        let names: Vec<String> = [".NET", "Attachments", "resources", "_resources"].iter().map(|n| a.take(n)).collect();
        assert_eq!(names, ["_.NET", "Attachments-1", "resources-1", "_resources-1"]);
        assert!(names.iter().all(|n| !crate::store::is_hidden_folder(n)));
        // pages keep the dot
        assert_eq!(NameAllocator::new().take(&safe(".NET tips")), ".NET tips");
    }

    #[test]
    fn cancel_stops_after_section() {
        let d = temp("cancel");
        let src = d.join("nb");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.one"), b"not a onenote file").unwrap();
        fs::write(src.join("b.one"), b"not a onenote file").unwrap();
        let cancel = AtomicBool::new(true);
        let r = import(&src, &d.join("out"), &|_| {}, &cancel);
        assert!(r.cancelled);
        assert_eq!(r.sections + r.sections_failed, 0);
        assert!(!d.join("out").exists());
    }

    #[test]
    fn unparsable_section_is_listed_and_import_goes_on() {
        let d = temp("badsec");
        let src = d.join("nb");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.one"), b"not a onenote file").unwrap();
        let r = import(&src, &d.join("out"), &|_| {}, &AtomicBool::new(false));
        assert_eq!(r.sections_failed, 1);
        assert_eq!(r.problems.len(), 1);
        assert!(r.problems[0].starts_with("a.one: "));
    }

    #[test]
    fn sections_found_sorted() {
        let d = temp("find");
        for f in ["b.one", "A.one", "t.onetoc2", "g/c.one", "OneNote_RecycleBin/x.one"] {
            let p = d.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"x").unwrap();
        }
        let found: Vec<PathBuf> = find_sections(&d).iter().map(|p| p.strip_prefix(&d).unwrap().to_path_buf()).collect();
        assert_eq!(found, vec![PathBuf::from("A.one"), PathBuf::from("b.one"), PathBuf::from("g").join("c.one")]);
    }

    #[test]
    fn extension_is_cleaned() {
        assert_eq!(clean_extension(".png"), ".png");
        assert_eq!(clean_extension(".b:c"), ".bc");
        assert_eq!(clean_extension(".\\..\\x"), ".x");
        assert_eq!(clean_extension(".abcdefghijklmnopqrstuvwxyz"), ".abcdefghij");
        assert_eq!(clean_extension(".:*"), "");
        assert_eq!(clean_extension(""), "");
        assert_eq!(clean_extension(".tar.gz"), ".tar.gz");
    }

    #[test]
    fn section_names_are_cleaned() {
        assert_eq!(plain_name("Foo^JBar"), "Foo Bar");
        assert_eq!(plain_name("A:B"), "AB");
    }
}
