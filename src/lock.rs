//! Edit locks for notes in a shared folder.
//!
//! While a page is being edited, a small lock file sits next to it (`.Page.md.notecraft-lock`).
//! Other computers see it and refuse to edit, rename or delete the page. The editing app touches
//! the file every [`HEARTBEAT`]; a lock that was not touched for [`STALE_AFTER`] (crashed app,
//! computer switched off) is taken over.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

pub const HEARTBEAT: Duration = Duration::from_secs(30);
pub const STALE_AFTER: Duration = Duration::from_secs(180);
const SUFFIX: &str = ".notecraft-lock";

/// Identifies this running copy of the app.
static SESSION: LazyLock<String> = LazyLock::new(|| {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{nanos:x}", std::process::id())
});

pub fn computer() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "this computer".into())
}

fn user() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}

pub fn lock_path(page: &Path) -> PathBuf {
    let name = page.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    page.with_file_name(format!(".{name}{SUFFIX}"))
}

pub fn is_lock_file(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(SUFFIX))
}

/// Who holds a lock.
#[derive(Clone, Debug)]
pub struct Holder {
    pub computer: String,
    pub user: String,
    pub since: String,
    session: String,
}

impl Holder {
    pub fn describe(&self) -> String {
        let who = if self.user.is_empty() { String::new() } else { format!(" by {}", self.user) };
        let since = if self.since.is_empty() { String::new() } else { format!(" since {}", self.since) };
        format!("on computer {}{who}{since}", self.computer)
    }

    fn is_me(&self) -> bool {
        self.session == *SESSION
    }
}

fn read_holder(lock: &Path) -> Option<Holder> {
    let text = fs::read_to_string(lock).ok()?;
    let mut h = Holder { computer: "another computer".into(), user: String::new(), since: String::new(), session: String::new() };
    for line in text.lines() {
        match line.split_once('=') {
            Some(("computer", v)) => h.computer = v.to_string(),
            Some(("user", v)) => h.user = v.to_string(),
            Some(("since", v)) => h.since = v.to_string(),
            Some(("session", v)) => h.session = v.to_string(),
            _ => {}
        }
    }
    Some(h)
}

fn is_stale(lock: &Path) -> bool {
    let Ok(modified) = fs::metadata(lock).and_then(|m| m.modified()) else { return false };
    SystemTime::now().duration_since(modified).is_ok_and(|age| age > STALE_AFTER)
}

/// The holder of the page's lock if it is someone else and the lock is still alive.
pub fn held_by_other(page: &Path) -> Option<Holder> {
    let lock = lock_path(page);
    if !lock.exists() || is_stale(&lock) {
        return None;
    }
    let h = read_holder(&lock).unwrap_or(Holder {
        computer: "another computer".into(),
        user: String::new(),
        since: String::new(),
        session: String::new(),
    });
    if h.is_me() { None } else { Some(h) }
}

/// The first page under `dir` that someone else is editing.
pub fn held_below(dir: &Path) -> Option<(PathBuf, Holder)> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if crate::store::entry_is_dir(&e) {
                stack.push(p);
            } else if is_lock_file(&p) {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                let page = p.with_file_name(name.trim_start_matches('.').trim_end_matches(SUFFIX));
                if let Some(h) = held_by_other(&page) {
                    return Some((page, h));
                }
            }
        }
    }
    None
}

/// An edit lock held by this app; removed when dropped.
pub struct EditLock {
    pub page: PathBuf,
    lock: PathBuf,
    content: String,
    touched: Instant,
}

impl EditLock {
    /// Takes the lock, or says who is editing the page.
    pub fn acquire(page: &Path) -> Result<EditLock, String> {
        let lock = lock_path(page);
        let content = format!(
            "computer={}\nuser={}\nsince={}\nsession={}\n",
            computer(),
            user(),
            chrono::Local::now().format("%-d.%-m.%Y %H:%M"),
            *SESSION
        );
        for attempt in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&lock) {
                Ok(mut f) => {
                    f.write_all(content.as_bytes()).map_err(|e| format!("Could not write the edit lock: {e}"))?;
                    hide(&lock);
                    return Ok(EditLock { page: page.to_path_buf(), lock, content, touched: Instant::now() });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                    if let Some(h) = held_by_other(page) {
                        return Err(format!("This page is being edited {}.\nYou can edit it when they are done.", h.describe()));
                    }
                    // Our own leftover or an expired lock: take it over.
                    let _ = fs::remove_file(&lock);
                }
                Err(e) => return Err(format!("Could not lock the page for editing: {e}")),
            }
        }
        Err("Could not lock the page for editing.".into())
    }

    /// Keeps the lock alive; returns an error if another computer took it over.
    pub fn heartbeat(&mut self) -> Result<(), String> {
        if self.touched.elapsed() < HEARTBEAT {
            return Ok(());
        }
        self.touched = Instant::now();
        match read_holder(&self.lock) {
            Some(h) if !h.is_me() => Err(format!("The page was taken over for editing {}.", h.describe())),
            Some(_) => {
                // Refresh the time stamp (hidden files cannot be re-created in place on Windows).
                if let Ok(f) = OpenOptions::new().write(true).open(&self.lock) {
                    let _ = f.set_modified(SystemTime::now());
                }
                Ok(())
            }
            None => {
                // Someone removed our lock: put it back.
                if let Ok(mut f) = OpenOptions::new().write(true).create_new(true).open(&self.lock) {
                    let _ = f.write_all(self.content.as_bytes());
                    hide(&self.lock);
                }
                Ok(())
            }
        }
    }

    /// True while the lock file is still ours.
    pub fn still_mine(&self) -> bool {
        read_holder(&self.lock).is_none_or(|h| h.is_me())
    }
}

impl Drop for EditLock {
    fn drop(&mut self) {
        if read_holder(&self.lock).is_some_and(|h| h.is_me()) {
            let _ = fs::remove_file(&self.lock);
        }
    }
}

#[cfg(windows)]
fn hide(path: &Path) {
    use std::os::windows::process::CommandExt as _;
    // FILE_ATTRIBUTE_HIDDEN via attrib keeps the lock out of the way in File Explorer.
    let _ = std::process::Command::new("attrib").arg("+h").arg(path).creation_flags(0x0800_0000).status();
}

#[cfg(not(windows))]
fn hide(_path: &Path) {}

/// Size and time of a file, to notice changes made elsewhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

pub fn stamp(path: &Path) -> Option<Stamp> {
    let m = fs::metadata(path).ok()?;
    Some(Stamp { len: m.len(), modified: m.modified().ok() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_session_is_refused() {
        let dir = std::env::temp_dir().join(format!("notecraft-lock-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let page = dir.join("Page.md");
        fs::write(&page, "x").unwrap();

        let lock = EditLock::acquire(&page).unwrap();
        assert!(lock_path(&page).exists());
        assert!(held_by_other(&page).is_none(), "our own lock does not block us");

        // Pretend another computer holds it.
        drop(lock);
        let _ = std::process::Command::new("attrib").arg("-h").arg(lock_path(&page)).status();
        fs::write(lock_path(&page), "computer=PC-B\nuser=ana\nsince=1.1.2026 10:00\nsession=other\n").unwrap();
        let err = EditLock::acquire(&page).err().unwrap();
        assert!(err.contains("PC-B") && err.contains("ana"), "{err}");
        assert!(held_below(&dir).is_some());

        // An expired lock is taken over.
        let old = SystemTime::now() - Duration::from_secs(600);
        fs::File::options().write(true).open(lock_path(&page)).unwrap().set_modified(old).unwrap();
        let lock = EditLock::acquire(&page).unwrap();
        assert!(lock.still_mine());
        drop(lock);
        assert!(!lock_path(&page).exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
