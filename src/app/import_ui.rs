//! The "Import from OneNote" wizard: choose the notebook folder, choose where to write it, then the
//! import runs on a thread while the window shows its progress and, at the end, a summary.

use super::*;
use crate::onenote::{self, Progress, Report};
use std::sync::atomic::{AtomicBool, Ordering};

/// How long the source path must stay unchanged before its sections are counted again.
const RECOUNT_DELAY: Duration = Duration::from_millis(300);
/// How long closing NoteCraft waits for a running import to stop after its current section.
const EXIT_WAIT: Duration = Duration::from_secs(10);
const ERROR_TEXT: Color32 = Color32::from_rgb(0xC0, 0x20, 0x20);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// Step 1: the folder with the notebook's .one files.
    Source,
    /// Step 2: the folder the notebook is written into.
    Target,
    /// Step 3: the import runs, then the summary.
    Run,
}

/// The wizard window.
pub struct ImportWin {
    step: Step,
    src: String,
    dst: String,
    /// The source text that `found` belongs to.
    counted_for: Option<String>,
    /// When the source text was last typed into; None counts at once.
    changed_at: Option<Instant>,
    /// Sections and folders found in the source, or why Next is disabled.
    found: Result<(usize, usize), String>,
    /// The (source, target) text that `target` belongs to.
    checked_for: Option<(String, String)>,
    /// The notebook folder the import writes, or why Next is disabled.
    target: Result<PathBuf, String>,
    /// The notebook folder in `target` exists and holds anything (so the user is asked first).
    target_not_empty: bool,
    /// The count running on its own thread, for this source text.
    counting: Option<(String, mpsc::Receiver<Result<(usize, usize), String>>)>,
    /// The target check running on its own thread, for this (source, target) text.
    checking: Option<((String, String), mpsc::Receiver<Result<(PathBuf, bool), String>>)>,
    /// The question about a non-empty notebook folder is showing.
    confirm: bool,
    /// The notebook folder the user already agreed to write into (asked once per wizard).
    confirmed_for: Option<PathBuf>,
    /// Messages from the import thread; Some while it runs.
    rx: Option<mpsc::Receiver<Progress>>,
    cancel: Arc<AtomicBool>,
    /// The section being imported: index (0-based), count, place in the notebook.
    last: Option<(usize, usize, String)>,
    report: Option<Report>,
    message: String,
    /// NoteCraft is closing and waits for the import until this moment.
    exit_deadline: Option<Instant>,
}

impl ImportWin {
    pub fn new(src: Option<PathBuf>, dst: Option<PathBuf>) -> Self {
        ImportWin {
            step: Step::Source,
            src: src.map(|p| p.display().to_string()).unwrap_or_default(),
            dst: dst.map(|p| p.display().to_string()).unwrap_or_default(),
            counted_for: None,
            changed_at: None,
            found: Err(String::new()),
            checked_for: None,
            target: Err(String::new()),
            target_not_empty: false,
            counting: None,
            checking: None,
            confirm: false,
            confirmed_for: None,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            last: None,
            report: None,
            message: String::new(),
            exit_deadline: None,
        }
    }

    fn src_path(&self) -> PathBuf {
        PathBuf::from(self.src.trim())
    }

    fn dst_path(&self) -> PathBuf {
        PathBuf::from(self.dst.trim())
    }

    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    /// Counts the sections of the source, on its own thread, when its text has changed and typing
    /// has paused; takes the result when it is there.
    fn refresh_count(&mut self, ctx: &egui::Context) {
        if let Some((key, rx)) = &self.counting
            && let Some(found) = background_result(rx)
        {
            self.found = found.unwrap_or_else(|| Err("Could not read this folder.".into()));
            self.counted_for = Some(key.clone());
            self.counting = None;
        }
        let waited = self.changed_at.map(|t| t.elapsed());
        let pending = self.counting.as_ref().is_some_and(|c| c.0 == self.src);
        if !pending && recount_due(self.counted_for.as_deref(), &self.src, waited) {
            let text = self.src.clone();
            self.counting = Some((self.src.clone(), in_background(ctx, move || count_sections(&text))));
            self.changed_at = None;
        } else if self.counted_for.as_deref() != Some(self.src.as_str())
            && let Some(w) = waited
        {
            ctx.request_repaint_after(RECOUNT_DELAY.saturating_sub(w));
        }
    }

    /// Checks the target, on its own thread, when the source or target text has changed; takes
    /// the result when it is there.
    fn refresh_target(&mut self, ctx: &egui::Context) {
        if let Some((key, rx)) = &self.checking
            && let Some(checked) = background_result(rx)
        {
            let checked = checked.unwrap_or_else(|| Err("Could not check this folder.".into()));
            self.target_not_empty = checked.as_ref().is_ok_and(|c| c.1);
            self.target = checked.map(|c| c.0);
            self.checked_for = Some(key.clone());
            self.checking = None;
        }
        let key = (self.src.clone(), self.dst.clone());
        let pending = self.checking.as_ref().is_some_and(|c| c.0 == key);
        if !pending && self.checked_for.as_ref() != Some(&key) {
            let (src, dst) = (self.src_path(), self.dst.clone());
            let rx = in_background(ctx, move || {
                target_check(&src, &dst).map(|nb| {
                    let not_empty = needs_confirmation(&nb);
                    (nb, not_empty)
                })
            });
            self.checking = Some((key, rx));
        }
    }

    /// The target check belongs to the text in the boxes.
    fn target_current(&self) -> bool {
        self.checked_for.as_ref().is_some_and(|(s, d)| *s == self.src && *d == self.dst)
    }

    /// The count belongs to the text in the box and found at least one section.
    fn source_ok(&self) -> bool {
        self.counted_for.as_deref() == Some(self.src.as_str()) && self.found.is_ok()
    }

    /// Next on step 1.
    fn next_from_source(&mut self) -> bool {
        if !self.source_ok() {
            return false;
        }
        self.step = Step::Target;
        self.message.clear();
        // The target folder may have changed since it was last checked.
        self.checked_for = None;
        true
    }

    /// < Back on step 2.
    fn back(&mut self) {
        self.step = Step::Source;
        self.confirm = false;
        self.message.clear();
    }

    /// Next on step 2. `not_empty` tells whether the notebook folder exists and holds anything.
    /// Returns true when the import should start now.
    fn next_from_target(&mut self, not_empty: bool) -> bool {
        let Ok(nb) = self.target.clone() else { return false };
        if not_empty && self.confirmed_for.as_ref() != Some(&nb) {
            self.confirm = true;
            return false;
        }
        true
    }

    /// The answer to the non-empty-folder question. Returns true when the import should start.
    fn answer(&mut self, yes: bool) -> bool {
        self.confirm = false;
        if !yes {
            return false;
        }
        self.confirmed_for = self.target.clone().ok();
        true
    }

    /// The window's close (Esc, a click beside it, or a Cancel/Close button before the import).
    /// While the import runs this only cancels it: the window stays until the thread is done.
    fn request_close(&mut self) -> bool {
        if self.running() {
            self.cancel.store(true, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// Starts the thread. It owns only the two paths, the sender, the cancel flag and a handle to
    /// ask for a repaint; it never touches the app's state.
    fn start(&mut self, src: PathBuf, dst: PathBuf, ctx: egui::Context) {
        let (tx, rx) = mpsc::channel();
        let cancel = self.cancel.clone();
        std::thread::spawn(move || {
            onenote::import(
                &src,
                &dst,
                &|p| {
                    // The window may be gone when NoteCraft is closing; nothing to do then.
                    let _ = tx.send(p);
                    ctx.request_repaint();
                },
                &cancel,
            );
        });
        self.rx = Some(rx);
        self.step = Step::Run;
        self.confirm = false;
        self.message.clear();
    }

    /// Takes the messages from the thread. Returns true once, when the import has just finished.
    fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        loop {
            match rx.try_recv() {
                Ok(Progress::Section { index, count, place }) => self.last = Some((index, count, place)),
                Ok(Progress::Done(r)) => {
                    self.report = Some(r);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let mut r = Report::default();
                    r.problems.push("The import stopped unexpectedly.".into());
                    r.cancelled = self.cancel.load(Ordering::Relaxed);
                    self.report = Some(r);
                    break;
                }
            }
        }
        self.rx = None;
        true
    }

    /// NoteCraft is closing: asks the import to stop. Returns true while NoteCraft should stay
    /// open so the section being written is finished, at most `EXIT_WAIT` from the first call.
    /// The UI keeps running meanwhile; `exit_ready` tells when to close.
    pub fn hold_exit(&mut self) -> bool {
        self.cancel.store(true, Ordering::Relaxed);
        if !self.running() {
            return false;
        }
        let deadline = *self.exit_deadline.get_or_insert_with(|| Instant::now() + EXIT_WAIT);
        Instant::now() < deadline
    }

    /// An exit held by `hold_exit` can go on: the import has stopped or the wait is over.
    pub fn exit_ready(&self) -> bool {
        self.exit_deadline.is_some_and(|d| !self.running() || Instant::now() >= d)
    }

    fn source_step(&mut self, ui: &mut egui::Ui, next: &mut bool, close: &mut bool) {
        ui.label(RichText::new("Step 1 of 3 – OneNote notebook").color(MUTED));
        ui.add_space(6.0);
        ui.label("Folder with the notebook's .one files:");
        ui.horizontal(|ui| {
            let r = ui.add(TextEdit::singleline(&mut self.src).desired_width(400.0).margin(Margin::symmetric(6, 5)));
            if r.changed() {
                self.changed_at = Some(Instant::now());
            }
            if ui.button("Browse…").clicked() {
                let mut dlg = rfd::FileDialog::new().set_title("Choose the OneNote notebook folder");
                let cur = self.src_path();
                if cur.is_dir() {
                    dlg = dlg.set_directory(&cur);
                }
                if let Some(dir) = dlg.pick_folder() {
                    self.src = dir.display().to_string();
                    self.changed_at = None;
                }
            }
        });
        self.refresh_count(ui.ctx());
        let counted = self.counted_for.as_deref() == Some(self.src.as_str());
        match &self.found {
            Ok((n, m)) if counted => {
                ui.label(RichText::new(count_text(*n, *m)).color(TEXT));
            }
            Err(e) if counted && !e.is_empty() => {
                ui.label(RichText::new(e).color(ERROR_TEXT));
            }
            _ if self.counting.as_ref().is_some_and(|c| c.0 == self.src) => {
                ui.label(RichText::new("Looking for sections…").color(MUTED));
            }
            _ => {
                ui.label("");
            }
        }
        ui.add_space(10.0);
        let ok = self.source_ok();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled(ok, accent_button("Next >")).hover_fx().clicked() {
                *next = true;
            }
            if ui.button("Cancel").clicked() {
                *close = true;
            }
        });
    }

    fn target_step(&mut self, ui: &mut egui::Ui, next: &mut bool, close: &mut bool, answer: &mut Option<bool>) {
        ui.label(RichText::new("Step 2 of 3 – Target folder").color(MUTED));
        ui.add_space(6.0);
        ui.label("Folder to write the notebook into:");
        ui.horizontal(|ui| {
            ui.add_enabled(
                !self.confirm,
                TextEdit::singleline(&mut self.dst).desired_width(400.0).margin(Margin::symmetric(6, 5)),
            );
            if ui.add_enabled(!self.confirm, egui::Button::new("Browse…")).clicked() {
                let mut dlg = rfd::FileDialog::new().set_title("Choose the folder to write the notebook into");
                let cur = self.dst_path();
                if cur.is_dir() {
                    dlg = dlg.set_directory(&cur);
                } else if let Some(parent) = self.src_path().parent() {
                    dlg = dlg.set_directory(parent);
                }
                if let Some(dir) = dlg.pick_folder() {
                    self.dst = dir.display().to_string();
                }
            }
        });
        self.refresh_target(ui.ctx());
        let current = self.target_current();
        let dst = self.dst_path();
        if dst.is_absolute() {
            let nb = onenote::notebook_dir(&self.src_path(), &dst);
            ui.label(RichText::new(format!("The notebook will be written to {}", nb.display())).color(TEXT));
        }
        if !current && !self.dst.trim().is_empty() {
            ui.label(RichText::new("Checking the folder…").color(MUTED));
        } else if let Err(e) = &self.target
            && current
            && !e.is_empty()
        {
            ui.label(RichText::new(e).color(ERROR_TEXT));
        }
        if !self.message.is_empty() {
            ui.label(RichText::new(&self.message).color(ERROR_TEXT));
        }
        ui.add_space(10.0);
        if self.confirm {
            let nb = self.target.clone().unwrap_or_default();
            ui.label(RichText::new(confirm_text(&nb)).color(TEXT));
            ui.add_space(6.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("No").clicked() {
                    *answer = Some(false);
                }
                if ui.add(accent_button("Yes")).hover_fx().clicked() {
                    *answer = Some(true);
                }
            });
            return;
        }
        let ok = current && self.target.is_ok();
        ui.horizontal(|ui| {
            if ui.button("< Back").clicked() {
                self.back();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(ok, accent_button("Next >")).hover_fx().clicked() {
                    *next = true;
                }
                if ui.button("Cancel").clicked() {
                    *close = true;
                }
            });
        });
    }

    fn run_step(&mut self, ui: &mut egui::Ui, close: &mut bool) {
        ui.label(RichText::new("Step 3 of 3 – Import").color(MUTED));
        ui.add_space(6.0);
        if let Some(report) = &self.report {
            ui.add(egui::ProgressBar::new(1.0).desired_width(500.0));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(report.summary()).monospace().size(12.5)).wrap());
            });
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(accent_button("Close")).hover_fx().clicked() {
                    *close = true;
                }
            });
            return;
        }
        let (fraction, line) = match &self.last {
            Some((i, n, place)) => (*i as f32 / (*n).max(1) as f32, section_text(*i, *n, place)),
            None => (0.0, "Starting…".to_string()),
        };
        ui.add(egui::ProgressBar::new(fraction).desired_width(500.0));
        ui.label(RichText::new(line).color(TEXT));
        ui.add_space(10.0);
        let cancelling = self.cancel.load(Ordering::Relaxed);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label = if cancelling { "Cancelling…" } else { "Cancel" };
            if ui
                .add_enabled(!cancelling, egui::Button::new(label))
                .on_hover_text("Stops after the section being written; what was written stays.")
                .clicked()
            {
                self.cancel.store(true, Ordering::Relaxed);
            }
        });
    }
}

impl Drop for ImportWin {
    /// A thread that outlives the window stops after its current section.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Runs `f` on its own thread (a slow network share must not freeze the window) and asks for a
/// repaint when its result is there.
fn in_background<T: Send + 'static>(ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) -> mpsc::Receiver<T> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(f());
        ctx.request_repaint();
    });
    rx
}

/// The result of an `in_background` call: None while it runs, Some(None) when its thread died.
fn background_result<T>(rx: &mpsc::Receiver<T>) -> Option<Option<T>> {
    match rx.try_recv() {
        Ok(v) => Some(Some(v)),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(mpsc::TryRecvError::Disconnected) => Some(None),
    }
}

/// The source must stay unchanged for `RECOUNT_DELAY` after typing; `waited` is None when the text
/// was set at once (Browse…, opening the wizard).
fn recount_due(counted_for: Option<&str>, text: &str, waited: Option<Duration>) -> bool {
    counted_for != Some(text) && waited.is_none_or(|w| w >= RECOUNT_DELAY)
}

/// "Found 14 sections in 3 folders."
fn count_text(sections: usize, folders: usize) -> String {
    let s = if sections == 1 { "section" } else { "sections" };
    let f = if folders == 1 { "folder" } else { "folders" };
    format!("Found {sections} {s} in {folders} {f}.")
}

/// The number of different folders the sections lie in.
fn folder_count(sections: &[PathBuf]) -> usize {
    let mut dirs: Vec<&Path> = sections.iter().filter_map(|p| p.parent()).collect();
    dirs.sort();
    dirs.dedup();
    dirs.len()
}

/// (sections, folders) in the source, or why the wizard cannot go on (empty for an empty box).
fn count_sections(text: &str) -> Result<(usize, usize), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(String::new());
    }
    let src = Path::new(text);
    if !src.is_absolute() {
        return Err("Enter the full path of the folder.".into());
    }
    if !src.is_dir() {
        return Err("This folder does not exist.".into());
    }
    let sections = onenote::find_sections(src);
    if sections.is_empty() {
        return Err("No .one files in this folder.".into());
    }
    Ok((sections.len(), folder_count(&sections)))
}

/// The notebook folder for the target text, or why the wizard cannot go on.
fn target_check(src: &Path, text: &str) -> Result<PathBuf, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(String::new());
    }
    let dst = Path::new(text);
    if !dst.is_absolute() {
        return Err("Enter the full path of the folder.".into());
    }
    if let Err(e) = onenote::check_target(src, dst) {
        return Err(user_text(&e));
    }
    Ok(onenote::notebook_dir(src, dst))
}

/// An error from the import code without its "module::function: " prefix, starting upper case.
fn user_text(e: &str) -> String {
    let rest = match e.split_once(": ") {
        Some((head, rest)) if head.contains("::") && !head.contains(' ') => rest,
        _ => e,
    };
    let mut c = rest.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The notebook folder exists and holds anything.
fn needs_confirmation(nb: &Path) -> bool {
    std::fs::read_dir(nb).is_ok_and(|mut d| d.next().is_some())
}

fn confirm_text(nb: &Path) -> String {
    format!(
        "{} is not empty. Pages that already exist will be replaced; their current text is kept in their history. Continue?",
        nb.display()
    )
}

/// "Section 5 of 14: Work › Projects" from a 0-based index and a place such as `Work\Projects.one`.
fn section_text(index: usize, count: usize, place: &str) -> String {
    let mut parts: Vec<String> = Path::new(place).components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    if let Some(last) = parts.last_mut()
        && last.len() > 4
        && last.get(last.len() - 4..).is_some_and(|e| e.eq_ignore_ascii_case(".one"))
    {
        last.truncate(last.len() - 4);
    }
    format!("Section {} of {count}: {}", index + 1, parts.join(" › "))
}

/// The pages the status line reports: new, replaced and unchanged.
fn imported_pages(r: &Report) -> usize {
    r.pages_written + r.pages_replaced + r.pages_unchanged
}

/// The status line after an import: "Imported 1 page from OneNote", or for a cancelled import
/// "OneNote import cancelled; 12 pages imported".
fn status_text(r: &Report) -> String {
    let n = imported_pages(r);
    let pages = if n == 1 { "page" } else { "pages" };
    if r.cancelled {
        format!("OneNote import cancelled; {n} {pages} imported")
    } else {
        format!("Imported {n} {pages} from OneNote")
    }
}

/// The folder in the trees with this path.
fn find_folder<'a>(f: &'a Folder, path: &Path) -> Option<&'a Folder> {
    if f.path == path {
        return Some(f);
    }
    f.children.iter().find_map(|c| find_folder(c, path))
}

impl NoteApp {
    /// Opens the wizard with the folders used last time.
    pub(super) fn open_import(&mut self) {
        if self.import_win.is_none() {
            self.import_win = Some(ImportWin::new(self.import_src.clone(), self.import_dst.clone()));
        }
    }

    /// Saves and leaves the open page, remembers the folders and starts the thread.
    fn start_import(&mut self, win: &mut ImportWin, ctx: &egui::Context) {
        self.save_if_dirty();
        if self.page.as_ref().is_some_and(|p| p.body != p.saved_body) {
            win.message = "The open page could not be saved, so the import did not start.".into();
            return;
        }
        self.leave_edit();
        let (src, dst) = (win.src_path(), win.dst_path());
        self.import_src = Some(src.clone());
        self.import_dst = Some(dst.clone());
        self.remember();
        win.start(src, dst, ctx.clone());
    }

    /// Shows what the import wrote: adds the target folder (or reloads the tree), reloads the
    /// page list and the open page, rebuilds the search index.
    fn after_import(&mut self, win: &ImportWin) {
        let Some(report) = &win.report else { return };
        let (src, dst) = (win.src_path(), win.dst_path());
        let nb = onenote::notebook_dir(&src, &dst);
        if nb.is_dir() && store::check_new_root(&self.roots, &nb).is_ok() {
            self.add_root(nb.clone());
        } else {
            self.reload_tree();
            self.start_indexing();
        }
        self.reload_pages();
        self.reload_open_page();
        let shown = self.trees.iter().find_map(|t| find_folder(t, &nb)).map(|f| first_folder_with_pages(f).unwrap_or_else(|| f.path.clone()));
        if let Some(dir) = shown {
            self.select_folder(dir, false);
        }
        self.set_status(status_text(report));
    }

    pub(super) fn import_window(&mut self, ctx: &egui::Context) {
        let Some(mut win) = self.import_win.take() else { return };
        if win.poll() {
            self.after_import(&win);
        }
        let (mut next, mut close, mut answer) = (false, false, None);
        let modal = egui::Modal::new(Id::unique("import-onenote")).show(ctx, |ui| {
            ui.set_width(540.0);
            ui.label(RichText::new("Import from OneNote").size(18.0).color(TEXT));
            ui.add_space(4.0);
            match win.step {
                Step::Source => win.source_step(ui, &mut next, &mut close),
                Step::Target => win.target_step(ui, &mut next, &mut close, &mut answer),
                Step::Run => win.run_step(ui, &mut close),
            }
        });
        if outside_close(modal.should_close(), win.running()) {
            close = true;
        }
        let mut start = false;
        if next {
            match win.step {
                Step::Source => {
                    win.next_from_source();
                }
                Step::Target if win.target_current() => {
                    let not_empty = win.target_not_empty;
                    start = win.next_from_target(not_empty);
                }
                Step::Target => {}
                Step::Run => {}
            }
        }
        if let Some(yes) = answer {
            start = win.answer(yes);
        }
        if start {
            self.start_import(&mut win, ctx);
        }
        if win.running() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if close && win.request_close() {
            return;
        }
        self.import_win = Some(win);
    }
}

/// Esc or a click beside the window closes it, but not while the import runs: only the Cancel
/// button cancels then.
fn outside_close(should_close: bool, running: bool) -> bool {
    should_close && !running
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("notecraft-import-ui-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn count_text_plural_and_singular() {
        assert_eq!(count_text(14, 3), "Found 14 sections in 3 folders.");
        assert_eq!(count_text(1, 1), "Found 1 section in 1 folder.");
    }

    #[test]
    fn counts_sections_and_folders() {
        let d = temp("count");
        assert_eq!(count_sections(""), Err(String::new()));
        assert_eq!(count_sections("relative\\path"), Err("Enter the full path of the folder.".into()));
        assert_eq!(count_sections(&d.join("missing").display().to_string()), Err("This folder does not exist.".into()));
        fs::write(d.join("notes.txt"), "x").unwrap();
        assert_eq!(count_sections(&d.display().to_string()), Err("No .one files in this folder.".into()));
        fs::write(d.join("A.one"), "x").unwrap();
        fs::write(d.join("B.ONE"), "x").unwrap();
        fs::create_dir_all(d.join("Group")).unwrap();
        fs::write(d.join("Group").join("C.one"), "x").unwrap();
        fs::create_dir_all(d.join("OneNote_RecycleBin")).unwrap();
        fs::write(d.join("OneNote_RecycleBin").join("D.one"), "x").unwrap();
        assert_eq!(count_sections(&format!("  {}  ", d.display())), Ok((3, 2)));
        let _ = fs::remove_dir_all(&d);
    }

    /// Calls `step` until `done` holds, at most 5 s (the checks run on their own threads).
    fn wait_for(w: &mut ImportWin, step: impl Fn(&mut ImportWin), done: impl Fn(&ImportWin) -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while !done(w) {
            assert!(Instant::now() < until, "background check did not finish");
            step(w);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn count_and_target_check_run_in_the_background() {
        let d = temp("background");
        let src = d.join("Book");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("A.one"), "x").unwrap();
        let ctx = egui::Context::default();
        let mut w = ImportWin::new(Some(src.clone()), Some(d.join("out")));
        w.refresh_count(&ctx);
        assert!(w.counting.is_some());
        wait_for(&mut w, |w| w.refresh_count(&ctx), |w| w.source_ok());
        assert_eq!(w.found, Ok((1, 1)));
        assert!(w.next_from_source());

        w.refresh_target(&ctx);
        assert!(!w.target_current());
        wait_for(&mut w, |w| w.refresh_target(&ctx), |w| w.target_current());
        assert_eq!(w.target, Ok(d.join("out").join("Book")));
        assert!(!w.target_not_empty);

        // a non-empty notebook folder is noticed; a changed text makes the old check stale
        fs::create_dir_all(d.join("out").join("Book").join("x")).unwrap();
        w.dst = d.join("out").display().to_string() + "\\";
        assert!(!w.target_current());
        wait_for(&mut w, |w| w.refresh_target(&ctx), |w| w.target_current());
        assert!(w.target_not_empty);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn recount_waits_for_typing_to_pause() {
        assert!(recount_due(None, "x", None));
        assert!(!recount_due(Some("x"), "x", None));
        assert!(!recount_due(Some("x"), "xy", Some(Duration::from_millis(100))));
        assert!(recount_due(Some("x"), "xy", Some(RECOUNT_DELAY)));
        assert!(recount_due(Some("x"), "xy", None));
    }

    #[test]
    fn target_inside_source_is_refused() {
        let d = temp("target");
        let src = d.join("Book");
        fs::create_dir_all(&src).unwrap();
        assert_eq!(target_check(&src, ""), Err(String::new()));
        assert_eq!(target_check(&src, "out"), Err("Enter the full path of the folder.".into()));
        let e = target_check(&src, &src.join("out").display().to_string()).unwrap_err();
        assert!(e.starts_with("The target folder "), "{e}");
        assert_eq!(target_check(&src, &d.join("out").display().to_string()), Ok(d.join("out").join("Book")));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn user_text_drops_the_function_prefix() {
        assert_eq!(user_text("onenote::check_target: the target folder x"), "The target folder x");
        assert_eq!(user_text("plain: text"), "Plain: text");
    }

    #[test]
    fn confirmation_only_for_a_non_empty_folder() {
        let d = temp("confirm");
        assert!(!needs_confirmation(&d.join("missing")));
        assert!(!needs_confirmation(&d));
        fs::create_dir_all(d.join("sub")).unwrap();
        assert!(needs_confirmation(&d));
        let _ = fs::remove_dir_all(&d);
        assert_eq!(
            confirm_text(Path::new(r"D:\Notes\Work")),
            r"D:\Notes\Work is not empty. Pages that already exist will be replaced; their current text is kept in their history. Continue?"
        );
    }

    #[test]
    fn section_line() {
        assert_eq!(section_text(4, 14, r"Work\Projects.one"), "Section 5 of 14: Work › Projects");
        assert_eq!(section_text(0, 1, "Ideas.ONE"), "Section 1 of 1: Ideas");
    }

    #[test]
    fn status_counts_all_imported_pages() {
        let r = Report { pages_written: 2, pages_replaced: 3, pages_unchanged: 4, ..Default::default() };
        assert_eq!(imported_pages(&r), 9);
        assert_eq!(status_text(&r), "Imported 9 pages from OneNote");
        let one = Report { pages_unchanged: 1, ..Default::default() };
        assert_eq!(status_text(&one), "Imported 1 page from OneNote");
        let stopped = Report { pages_written: 1, cancelled: true, ..Default::default() };
        assert_eq!(status_text(&stopped), "OneNote import cancelled; 1 page imported");
    }

    fn win_with_target(nb: &str) -> ImportWin {
        let mut w = ImportWin::new(None, None);
        w.step = Step::Target;
        w.target = Ok(PathBuf::from(nb));
        w
    }

    #[test]
    fn step_transitions() {
        let mut w = ImportWin::new(Some(PathBuf::from(r"C:\Book")), None);
        assert_eq!(w.src, r"C:\Book");
        assert!(!w.next_from_source(), "not counted yet");
        w.counted_for = Some(w.src.clone());
        w.found = Err("No .one files in this folder.".into());
        assert!(!w.next_from_source());
        w.found = Ok((2, 1));
        assert!(w.next_from_source());
        assert_eq!(w.step, Step::Target);
        w.target = Err("x".into());
        assert!(!w.next_from_target(false));
        w.back();
        assert_eq!(w.step, Step::Source);
    }

    #[test]
    fn confirmation_is_asked_once_per_notebook_folder() {
        let mut w = win_with_target(r"D:\Out\Book");
        assert!(w.next_from_target(false), "empty folder: start at once");
        assert!(!w.next_from_target(true));
        assert!(w.confirm);
        assert!(!w.answer(false), "No returns to step 2");
        assert!(!w.confirm);
        assert_eq!(w.step, Step::Target);
        assert!(!w.next_from_target(true), "asked again after No");
        assert!(w.answer(true));
        assert!(w.next_from_target(true), "not asked again after Yes");
        w.target = Ok(PathBuf::from(r"D:\Other\Book"));
        assert!(!w.next_from_target(true), "another folder is asked about");
    }

    #[test]
    fn click_outside_or_esc_does_not_cancel_a_running_import() {
        assert!(outside_close(true, false));
        assert!(!outside_close(true, true));
        assert!(!outside_close(false, false));
    }

    #[test]
    fn close_while_running_only_cancels() {
        let mut w = win_with_target(r"D:\Out\Book");
        assert!(w.request_close());
        let (tx, rx) = mpsc::channel();
        w.rx = Some(rx);
        w.step = Step::Run;
        assert!(!w.request_close());
        assert!(w.cancel.load(Ordering::Relaxed));
        assert!(!w.poll());
        tx.send(Progress::Section { index: 1, count: 3, place: "A.one".into() }).unwrap();
        assert!(!w.poll());
        assert_eq!(w.last, Some((1, 3, "A.one".to_string())));
        tx.send(Progress::Done(Report { cancelled: true, ..Default::default() })).unwrap();
        assert!(w.poll());
        assert!(!w.running());
        assert!(w.report.as_ref().is_some_and(|r| r.cancelled));
        assert!(w.request_close());
    }

    #[test]
    fn a_thread_that_dies_still_ends_the_import() {
        let mut w = win_with_target(r"D:\Out\Book");
        let (tx, rx) = mpsc::channel::<Progress>();
        w.rx = Some(rx);
        drop(tx);
        assert!(w.poll());
        assert!(w.report.as_ref().is_some_and(|r| r.problems == ["The import stopped unexpectedly."]));
    }

    #[test]
    fn exit_waits_for_the_import_without_blocking() {
        let mut w = win_with_target(r"D:\Out\Book");
        assert!(!w.hold_exit(), "nothing running: close at once");
        assert!(!w.exit_ready(), "no exit was held");

        let mut w = win_with_target(r"D:\Out\Book");
        let (tx, rx) = mpsc::channel();
        w.rx = Some(rx);
        assert!(w.hold_exit());
        assert!(w.cancel.load(Ordering::Relaxed));
        assert!(!w.exit_ready());
        tx.send(Progress::Done(Report { cancelled: true, ..Default::default() })).unwrap();
        assert!(w.poll());
        assert!(w.exit_ready());
        assert!(!w.hold_exit(), "the second close goes through");

        // an import that never stops: the wait ends at the deadline
        let mut w = win_with_target(r"D:\Out\Book");
        let (_tx, rx) = mpsc::channel::<Progress>();
        w.rx = Some(rx);
        assert!(w.hold_exit());
        w.exit_deadline = Some(Instant::now() - Duration::from_millis(1));
        assert!(w.exit_ready());
        assert!(!w.hold_exit());
    }

    #[test]
    fn sending_after_the_window_is_gone_does_not_panic() {
        let (tx, rx) = mpsc::channel::<Progress>();
        drop(rx);
        let p = |p: Progress| {
            let _ = tx.send(p);
        };
        p(Progress::Done(Report::default()));
    }
}
