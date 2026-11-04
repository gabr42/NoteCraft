//! Windows for the history: the earlier versions of a page, the deleted items, and the old texts
//! found by the search.

use super::*;
use crate::history;
use chrono::NaiveDateTime;
use std::sync::atomic::{AtomicBool, Ordering};

/// "4 Nov 2026, 10:15"
pub fn when_text(when: NaiveDateTime) -> String {
    when.format("%-d %b %Y, %H:%M").to_string()
}

fn size_text(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// The first two non-empty lines of a text, for a list row.
fn first_lines(body: &str) -> String {
    let mut lines = body.lines().map(|l| l.trim().trim_start_matches('#').trim()).filter(|l| !l.is_empty());
    let mut s = lines.next().unwrap_or("").to_string();
    if let Some(second) = lines.next() {
        s.push_str("  ⏎  ");
        s.push_str(second);
    }
    if s.chars().count() > 140 {
        s = s.chars().take(140).collect::<String>() + "…";
    }
    s
}

fn body_preview(file: &Path) -> String {
    match store::read_doc(file) {
        Ok(doc) => doc.body.trim_start_matches(['\r', '\n']).to_string(),
        Err(e) => format!("Could not read {}: {e}", file.display()),
    }
}

/// The window with the earlier versions of one page.
pub struct HistoryWin {
    page: PathBuf,
    title: String,
    versions: Vec<history::Version>,
    /// First lines of each version, read when the row first comes into view.
    summaries: Vec<Option<String>>,
    selected: Option<usize>,
    preview: String,
    confirm_purge: bool,
    message: String,
}

impl HistoryWin {
    pub fn new(page: PathBuf, title: String) -> Self {
        let mut w = HistoryWin {
            page,
            title,
            versions: Vec::new(),
            summaries: Vec::new(),
            selected: None,
            preview: String::new(),
            confirm_purge: false,
            message: String::new(),
        };
        w.reload();
        w
    }

    fn reload(&mut self) {
        self.versions = history::list_versions(&self.page);
        self.summaries = vec![None; self.versions.len()];
        self.confirm_purge = false;
        self.select(if self.versions.is_empty() { None } else { Some(0) });
    }

    fn select(&mut self, i: Option<usize>) {
        self.selected = i;
        self.preview = i.and_then(|i| self.versions.get(i)).map(|v| body_preview(&v.path)).unwrap_or_default();
    }
}

impl NoteApp {
    /// Opens the history window of a page (unsaved changes are saved first so the list is current).
    pub(super) fn open_history(&mut self, page: PathBuf) {
        self.save_if_dirty();
        let title = store::read_doc(&page).map(|d| d.title_or(&page)).unwrap_or_else(|_| store::file_stem(&page));
        self.history_win = Some(HistoryWin::new(page, title));
    }

    /// Why the open page cannot take a restore right now, if it cannot.
    fn restore_blocked(&self, page: &Path) -> Option<String> {
        if let Some(p) = self.page.as_ref().filter(|p| p.path == page) {
            if self.mode == Mode::Edit {
                return Some("Finish editing the page first (Done).".into());
            }
            if p.body != p.saved_body {
                return Some("The page has changes that are not saved yet.".into());
            }
            if page.exists() && lock::stamp(page) != p.stamp {
                return Some("The page was changed on another computer. Reload it first (F5).".into());
            }
        }
        lock::held_by_other(page).map(|h| format!("This page is being edited {}. Try again when they are done.", h.describe()))
    }

    /// Puts an old version back into its page and reloads the page when it is the open one.
    fn restore_version_file(&mut self, page: &Path, file: &Path, when: NaiveDateTime) -> Result<String, String> {
        if let Some(why) = self.restore_blocked(page) {
            return Err(why);
        }
        history::restore_version(page, file)?;
        let _ = store::take_history_warning();
        if self.page.as_ref().is_some_and(|p| p.path == page) {
            self.page = None;
            self.open_page(page);
        }
        self.index.add(page);
        self.index_stale = true;
        self.reload_pages();
        let msg = format!("Restored the version from {}", when_text(when));
        self.set_status(msg.clone());
        Ok(msg)
    }

    fn restore_version_now(&mut self, win: &mut HistoryWin) {
        let Some(v) = win.selected.and_then(|i| win.versions.get(i)) else { return };
        let (file, when, page) = (v.path.clone(), v.when, win.page.clone());
        match self.restore_version_file(&page, &file, when) {
            Ok(msg) => {
                win.message = msg;
                win.reload();
            }
            Err(e) => win.message = e,
        }
    }

    pub(super) fn history_window(&mut self, ctx: &egui::Context) {
        let Some(mut win) = self.history_win.take() else { return };
        let mut open = true;
        let mut restore = false;
        let mut purge = false;
        let blocked_now = self.page.as_ref().filter(|p| p.path == win.page).is_some_and(|p| self.mode == Mode::Edit || p.body != p.saved_body);
        let screen = ctx.content_rect();
        let height = (screen.height() - 190.0).clamp(120.0, 360.0);
        egui::Window::new(format!("History of \"{}\"", win.title))
            .open(&mut open)
            .collapsible(false)
            .default_size([780.0, 480.0])
            .show(ctx, |ui| {
                if win.versions.is_empty() {
                    ui.label("There are no earlier versions of this page yet. A version is kept when a save changes the page.");
                    if !win.message.is_empty() {
                        ui.label(RichText::new(&win.message).color(MUTED));
                    }
                    return;
                }
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(Vec2::new(290.0, height), Layout::top_down(Align::Min), |ui| {
                        egui::ScrollArea::vertical().id_salt("history-list").min_scrolled_height(height).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                            ui.set_width(270.0);
                            let mut clicked = None;
                            for i in 0..win.versions.len() {
                                let probe = egui::Rect::from_min_size(ui.cursor().min, Vec2::new(270.0, 70.0));
                                if win.summaries[i].is_none() && ui.is_rect_visible(probe) {
                                    win.summaries[i] = Some(first_lines(&body_preview(&win.versions[i].path)));
                                }
                                let v = &win.versions[i];
                                let summary = win.summaries[i].clone().unwrap_or_default();
                                let resp = ui
                                    .push_id(i, |ui| {
                                        search_row(ui, &when_text(v.when), &size_text(v.size), &summary, 1, win.selected == Some(i))
                                    })
                                    .inner;
                                if resp.clicked() {
                                    clicked = Some(i);
                                }
                            }
                            if let Some(i) = clicked {
                                win.select(Some(i));
                                win.message.clear();
                            }
                        });
                    });
                    // A plain line of the list's height: a separator would stretch to the screen's bottom.
                    let (line, _) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
                    ui.painter().rect_filled(line, 0.0, BORDER);
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical().id_salt("history-preview").min_scrolled_height(height).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(&win.preview).monospace().size(12.5)).wrap());
                        });
                    });
                });
                ui.add_space(6.0);
                if !win.message.is_empty() {
                    ui.label(RichText::new(&win.message).color(MUTED));
                }
                ui.horizontal(|ui| {
                    let enabled = win.selected.is_some() && !blocked_now;
                    let b = ui.add_enabled(enabled, accent_button("Restore this version")).hover_fx();
                    let b = if blocked_now { b.on_disabled_hover_text("Finish editing and saving the page first.") } else { b };
                    if b.on_hover_text("Puts this text back into the page. The current text is kept as a version first.").clicked() {
                        restore = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if win.confirm_purge {
                            if ui.button("Cancel").clicked() {
                                win.confirm_purge = false;
                            }
                            if ui.add(accent_button("Delete all versions")).hover_fx().clicked() {
                                purge = true;
                            }
                            ui.label(RichText::new(format!("Delete all {} versions for good?", win.versions.len())).color(TEXT));
                        } else if ui.button("Delete all versions…").on_hover_text("Removes the saved versions of this page for good").clicked() {
                            win.confirm_purge = true;
                        }
                    });
                });
            });
        if restore {
            self.restore_version_now(&mut win);
        }
        if purge {
            match history::purge_versions(&win.page) {
                Ok(()) => {
                    self.set_status("All versions of the page were deleted".into());
                    win.reload();
                }
                Err(e) => win.message = e,
            }
        }
        if open {
            self.history_win = Some(win);
        }
    }
}

/// The window with everything deleted from the active top folder.
pub struct DeletedWin {
    top: PathBuf,
    items: Vec<history::DeletedItem>,
    /// The listing runs on a thread; it can take a while on a network drive.
    loading: Option<mpsc::Receiver<Vec<history::DeletedItem>>>,
    filter: String,
    /// Index into `items`.
    selected: Option<usize>,
    preview: String,
    confirm_forever: bool,
    confirm_purge: bool,
    message: String,
}

impl DeletedWin {
    pub fn new(top: PathBuf, confirm_purge: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let from = top.clone();
        std::thread::spawn(move || {
            let _ = tx.send(history::list_deleted(&from));
        });
        DeletedWin {
            top,
            items: Vec::new(),
            loading: Some(rx),
            filter: String::new(),
            selected: None,
            preview: String::new(),
            confirm_forever: false,
            confirm_purge,
            message: String::new(),
        }
    }

    fn select(&mut self, i: Option<usize>) {
        self.selected = i;
        self.confirm_forever = false;
        self.preview = match i.and_then(|i| self.items.get(i)) {
            Some(item) if item.kind == history::ItemKind::Page => body_preview(&item.entry.join(&item.file)),
            Some(item) => section_summary(&item.entry.join(&item.name)),
            None => String::new(),
        };
    }

    fn visible(&self) -> Vec<usize> {
        let words: Vec<Vec<char>> = self.filter.split_whitespace().map(store::fold).collect();
        (0..self.items.len())
            .filter(|&i| {
                let name = store::fold(&self.items[i].name);
                words.iter().all(|w| w.is_empty() || name.windows(w.len()).any(|x| x == w.as_slice()))
            })
            .collect()
    }
}

/// The pages inside a deleted section, as a list of names.
fn section_summary(dir: &Path) -> String {
    let mut names = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if store::entry_is_dir(&e) {
                if !store::is_hidden_folder(&e.file_name().to_string_lossy()) {
                    stack.push(e.path());
                }
            } else if store::is_md(&e.path()) {
                names.push(store::file_stem(&e.path()));
            }
        }
    }
    names.sort();
    let mut s = format!("A section with {} page(s):\n\n", names.len());
    for n in names.iter().take(60) {
        s.push_str("  ");
        s.push_str(n);
        s.push('\n');
    }
    if names.len() > 60 {
        s.push_str("  …\n");
    }
    s
}

impl NoteApp {
    /// Opens the window with the deleted items of the active top folder.
    pub(super) fn open_deleted(&mut self, confirm_purge: bool) {
        if let Some(top) = self.roots.get(self.active).cloned() {
            self.deleted_win = Some(DeletedWin::new(top, confirm_purge));
        }
    }

    /// Updates the index, the tree and the page list after a deleted item came back.
    fn after_restore_deleted(&mut self, item: &history::DeletedItem, path: &Path) -> String {
        match item.kind {
            history::ItemKind::Page => {
                self.index.add(path);
                let sub = store::subpage_dir(path);
                if sub.is_dir() {
                    self.index.add_tree(&sub);
                }
            }
            history::ItemKind::Section => self.index.add_tree(path),
        }
        self.index_stale = true;
        self.reload_tree();
        self.reload_pages();
        let msg = format!("Restored \"{}\" to {}", item.name, path.display());
        self.set_status(msg.clone());
        msg
    }

    /// Restores the deleted item kept in a `deleted\<time>` folder (from a search hit).
    fn restore_deleted_entry(&mut self, entry: &Path) -> Result<String, String> {
        let item = history::deleted_item_at(entry).ok_or_else(|| "The deleted item is not there any more.".to_string())?;
        let path = history::restore_deleted(&item)?;
        Ok(self.after_restore_deleted(&item, &path))
    }

    fn restore_deleted_now(&mut self, win: &mut DeletedWin, i: usize) {
        let Some(item) = win.items.get(i).cloned() else { return };
        match history::restore_deleted(&item) {
            Ok(path) => {
                let msg = self.after_restore_deleted(&item, &path);
                win.message = msg;
                win.items.remove(i);
                win.select(None);
            }
            Err(e) => win.message = e,
        }
    }

    pub(super) fn deleted_window(&mut self, ctx: &egui::Context) {
        let Some(mut win) = self.deleted_win.take() else { return };
        if let Some(rx) = &win.loading {
            match rx.try_recv() {
                Ok(items) => {
                    win.items = items;
                    win.loading = None;
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
                Err(mpsc::TryRecvError::Disconnected) => win.loading = None,
            }
        }
        let mut open = true;
        let mut restore = None;
        let mut forever = None;
        let mut purge = false;
        let screen = ctx.content_rect();
        let height = (screen.height() - 230.0).clamp(120.0, 340.0);
        let roots = self.roots.clone();
        egui::Window::new(format!("Deleted items in \"{}\"", folder_name(&win.top)))
            .open(&mut open)
            .collapsible(false)
            .default_width((screen.width() - 60.0).min(860.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name contains:");
                    egui::Frame::new().stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(4)).show(ui, |ui| {
                        ui.add(
                            TextEdit::singleline(&mut win.filter)
                                .hint_text("type part of a name")
                                .frame(egui::Frame::NONE)
                                .desired_width(240.0)
                                .margin(Margin::symmetric(6, 4)),
                        );
                    });
                    if win.loading.is_some() {
                        ui.label(RichText::new("Looking for deleted items…").color(MUTED));
                    } else {
                        ui.label(RichText::new(format!("{} deleted", win.items.len())).color(MUTED));
                    }
                });
                ui.add_space(4.0);
                let shown = win.visible();
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(Vec2::new(380.0, height), Layout::top_down(Align::Min), |ui| {
                        egui::ScrollArea::vertical().id_salt("deleted-list").min_scrolled_height(height).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                            ui.set_width(360.0);
                            let mut clicked = None;
                            for &i in &shown {
                                let item = &win.items[i];
                                let kind = if item.kind == history::ItemKind::Page { "page" } else { "section" };
                                let info = format!(
                                    "{kind} from {}  ·  deleted {}",
                                    history::place_of(&roots, &item.original_folder),
                                    when_text(item.deleted_at)
                                );
                                let resp = ui.push_id(i, |ui| search_row(ui, &item.name, &info, "", 1, win.selected == Some(i))).inner;
                                if resp.clicked() {
                                    clicked = Some(i);
                                }
                            }
                            if let Some(i) = clicked {
                                win.select(Some(i));
                                win.message.clear();
                            }
                            if shown.is_empty() && win.loading.is_none() {
                                ui.label(RichText::new("Nothing deleted matches.").color(MUTED));
                            }
                        });
                    });
                    let (line, _) = ui.allocate_exact_size(Vec2::new(1.0, height), Sense::hover());
                    ui.painter().rect_filled(line, 0.0, BORDER);
                    ui.vertical(|ui| {
                        egui::ScrollArea::vertical().id_salt("deleted-preview").min_scrolled_height(height).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(&win.preview).monospace().size(12.5)).wrap());
                        });
                    });
                });
                ui.add_space(6.0);
                if !win.message.is_empty() {
                    ui.label(RichText::new(&win.message).color(MUTED));
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(win.selected.is_some(), accent_button("Restore")).hover_fx().on_hover_text("Puts the item back where it was").clicked() {
                        restore = win.selected;
                    }
                    if win.confirm_forever {
                        if ui.add(accent_button("Delete forever")).hover_fx().clicked() {
                            forever = win.selected;
                        }
                        if ui.button("Cancel").clicked() {
                            win.confirm_forever = false;
                        }
                    } else if ui.add_enabled(win.selected.is_some(), egui::Button::new("Delete forever…")).clicked() {
                        win.confirm_forever = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if win.confirm_purge {
                            if ui.button("Cancel").clicked() {
                                win.confirm_purge = false;
                            }
                            if ui.add(accent_button("Delete all history")).hover_fx().clicked() {
                                purge = true;
                            }
                            ui.label(RichText::new("This removes all old versions and deleted items for good. Continue?").color(TEXT));
                        } else if ui
                            .button("Purge history…")
                            .on_hover_text("Removes all old versions and deleted items of this top folder for good")
                            .clicked()
                        {
                            win.confirm_purge = true;
                        }
                    });
                });
            });
        if let Some(i) = restore {
            self.restore_deleted_now(&mut win, i);
        }
        if let Some(i) = forever {
            match win.items.get(i).map(history::delete_forever) {
                Some(Ok(())) => {
                    win.items.remove(i);
                    win.select(None);
                    win.message = "Deleted for good.".into();
                }
                Some(Err(e)) => win.message = e,
                None => {}
            }
        }
        if purge {
            match history::purge_all(&win.top) {
                Ok(n) => {
                    self.history_win = None;
                    win.items.clear();
                    win.select(None);
                    win.confirm_purge = false;
                    win.message = format!("Removed {n} history folder(s).");
                    self.set_status(win.message.clone());
                }
                Err(e) => win.message = e,
            }
        }
        if open {
            self.deleted_win = Some(win);
        }
    }
}

const HISTORY_DELAY: Duration = Duration::from_millis(400);
const MAX_HISTORY_HITS: usize = 300;

/// The search through old versions and deleted pages, run on a thread while the search box has text
/// and "Include history" is ticked.
pub struct HistorySearch {
    hits: Vec<history::HistoryHit>,
    rx: Option<mpsc::Receiver<history::HistoryHit>>,
    cancel: Option<Arc<AtomicBool>>,
    /// The query the running (or finished) search was started for.
    started_for: String,
    /// The query being typed, and since when it has not changed.
    pending: String,
    pending_since: Instant,
}

impl Default for HistorySearch {
    fn default() -> Self {
        HistorySearch {
            hits: Vec::new(),
            rx: None,
            cancel: None,
            started_for: String::new(),
            pending: String::new(),
            pending_since: Instant::now(),
        }
    }
}

impl HistorySearch {
    /// Stops the running search (its hits so far stay).
    fn stop(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.rx = None;
    }

    /// Stops the search and forgets its hits; the next tick starts a new one.
    pub fn clear(&mut self) {
        self.stop();
        self.hits.clear();
        self.started_for.clear();
    }

    fn running(&self) -> bool {
        self.rx.is_some()
    }
}

/// An old text opened from the search results, read-only.
pub struct HistoryView {
    hit: history::HistoryHit,
    text: String,
    message: String,
}

impl NoteApp {
    /// Starts, restarts or stops the history search to match the search box. Called every frame.
    pub(super) fn history_search_tick(&mut self, ctx: &egui::Context) {
        let query = self.search.trim().to_string();
        let hs = &mut self.history_search;
        if !self.include_history || query.is_empty() {
            if hs.running() || !hs.hits.is_empty() || !hs.started_for.is_empty() {
                hs.clear();
            }
            hs.pending.clear();
            return;
        }
        if query != hs.pending {
            hs.pending = query.clone();
            hs.pending_since = Instant::now();
            hs.clear();
        }
        if hs.started_for != query {
            let waited = hs.pending_since.elapsed();
            if waited < HISTORY_DELAY {
                ctx.request_repaint_after(HISTORY_DELAY - waited);
                return;
            }
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let (roots, flag, q) = (self.roots.clone(), cancel.clone(), query.clone());
            std::thread::spawn(move || {
                history::search_history(&roots, &q, &flag, &|h| {
                    let _ = tx.send(h);
                });
            });
            hs.started_for = query;
            hs.rx = Some(rx);
            hs.cancel = Some(cancel);
        }
        let mut got = false;
        let mut finished = false;
        if let Some(rx) = &hs.rx {
            loop {
                match rx.try_recv() {
                    Ok(h) => {
                        hs.hits.push(h);
                        got = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        ctx.request_repaint_after(Duration::from_millis(100));
                        break;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        if got {
            hs.hits.sort_by(|a, b| b.when.cmp(&a.when).then_with(|| a.title.cmp(&b.title)));
        }
        if finished {
            hs.rx = None;
            hs.cancel = None;
        } else if hs.hits.len() >= MAX_HISTORY_HITS {
            hs.stop();
        }
    }

    /// The "In history" part of the search results.
    pub(super) fn history_rows(&mut self, ui: &mut egui::Ui) {
        if !self.include_history || self.search.trim().is_empty() {
            return;
        }
        ui.add_space(10.0);
        let hs = &self.history_search;
        let note = if hs.running() {
            "searching…".to_string()
        } else if hs.hits.is_empty() {
            "nothing found in old versions or deleted pages".to_string()
        } else if hs.hits.len() >= MAX_HISTORY_HITS {
            format!("{}+ found", MAX_HISTORY_HITS)
        } else {
            format!("{} found", hs.hits.len())
        };
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("In history").color(TEXT).size(15.0).strong());
            ui.label(RichText::new(note).color(MUTED).size(12.5));
        });
        ui.add_space(4.0);
        let mut clicked = None;
        for (i, h) in hs.hits.iter().enumerate() {
            let kind = match h.kind {
                history::HitKind::Version { .. } => "old version",
                history::HitKind::Deleted { .. } => "deleted page",
            };
            let mut info = format!("{}  ·  {kind}, {}", h.place, when_text(h.when));
            if h.older_matches > 0 {
                info.push_str(&format!("  ·  +{} older", h.older_matches));
            }
            let resp = ui.push_id(("history-hit", i), |ui| search_row(ui, &h.title, &info, &h.snippet, h.count, false)).inner;
            if resp.clicked() {
                clicked = Some(i);
            }
        }
        if let Some(i) = clicked {
            self.actions.push(Action::OpenHistoryHit(Box::new(hs.hits[i].clone())));
        }
    }

    pub(super) fn open_history_hit(&mut self, hit: history::HistoryHit) {
        let text = body_preview(&hit.file);
        self.history_view = Some(HistoryView { hit, text, message: String::new() });
    }

    pub(super) fn history_view_window(&mut self, ctx: &egui::Context) {
        let Some(mut view) = self.history_view.take() else { return };
        let mut open = true;
        let mut restore = false;
        let screen = ctx.content_rect();
        let height = (screen.height() - 220.0).clamp(120.0, 380.0);
        let (kind, button) = match &view.hit.kind {
            history::HitKind::Version { .. } => ("old version", "Restore this version"),
            history::HitKind::Deleted { entry } => (
                "deleted page",
                match history::deleted_item_at(entry).map(|d| d.kind) {
                    Some(history::ItemKind::Section) => "Restore the deleted section",
                    _ => "Restore the deleted page",
                },
            ),
        };
        egui::Window::new(format!("{} — {kind}", view.hit.title))
            .open(&mut open)
            .collapsible(false)
            .default_width((screen.width() - 60.0).min(760.0))
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("{}  ·  {}", view.hit.place, when_text(view.hit.when))).color(ACCENT).size(12.5));
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("hit-preview").min_scrolled_height(height).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(&view.text).monospace().size(12.5)).wrap());
                });
                ui.add_space(6.0);
                if !view.message.is_empty() {
                    ui.label(RichText::new(&view.message).color(MUTED));
                }
                let hover = match &view.hit.kind {
                    history::HitKind::Version { .. } => "Puts this text back into the page. The current text is kept as a version first.",
                    history::HitKind::Deleted { .. } => "Puts the page back as it was when it was deleted.",
                };
                if ui.add(accent_button(button)).hover_fx().on_hover_text(hover).clicked() {
                    restore = true;
                }
            });
        let mut keep = open;
        if restore {
            let result = match &view.hit.kind {
                history::HitKind::Version { page } => self.restore_version_file(page, &view.hit.file, view.hit.when),
                history::HitKind::Deleted { entry } => self.restore_deleted_entry(entry),
            };
            match result {
                Ok(msg) => {
                    self.history_search.clear();
                    keep = false;
                    self.set_status(msg);
                }
                Err(e) => view.message = e,
            }
        }
        if keep {
            self.history_view = Some(view);
        }
    }
}
