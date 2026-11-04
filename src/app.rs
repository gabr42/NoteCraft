//! The NoteCraft window: notebooks and sections on the left, pages in the middle, the page on the right.

use crate::lock::{self, EditLock};
use crate::render;
use crate::store::{self, Folder, Index, PageEntry, SearchHit, SortOrder};
use crate::web::{self, PageWeb, WebMsg};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Id, Key, KeyboardShortcut, Layout, Margin, Modifiers, RichText,
    Sense, Stroke, TextEdit, Vec2,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

mod history_ui;

const ACCENT: Color32 = Color32::from_rgb(0x6B, 0x2F, 0xA0);
const ACCENT_DARK: Color32 = Color32::from_rgb(0x55, 0x22, 0x82);
const ACCENT_SOFT: Color32 = Color32::from_rgb(0xEC, 0xE2, 0xF6);
const HOVER_SOFT: Color32 = Color32::from_rgb(0xF4, 0xEF, 0xF9);
const NAV_BG: Color32 = Color32::from_rgb(0xF3, 0xF2, 0xF1);
const LIST_BG: Color32 = Color32::from_rgb(0xFA, 0xF9, 0xF8);
const PAGE_BG: Color32 = Color32::WHITE;
const TEXT: Color32 = Color32::from_rgb(0x25, 0x24, 0x23);
const MUTED: Color32 = Color32::from_rgb(0x7A, 0x77, 0x74);
const BORDER: Color32 = Color32::from_rgb(0xE1, 0xDF, 0xDD);
const SECTION_COLORS: [Color32; 8] = [
    Color32::from_rgb(0xD1, 0x34, 0x38),
    Color32::from_rgb(0xE8, 0x7D, 0x0D),
    Color32::from_rgb(0xC1, 0x9C, 0x00),
    Color32::from_rgb(0x49, 0x8C, 0x3A),
    Color32::from_rgb(0x00, 0x83, 0x8F),
    Color32::from_rgb(0x2B, 0x6C, 0xC4),
    Color32::from_rgb(0x88, 0x4F, 0xC4),
    Color32::from_rgb(0xC2, 0x39, 0xB3),
];
const DEFAULT_FOLDER: &str = r"e:\0\onenote-export";

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    View,
    Edit,
}

struct OpenPage {
    path: PathBuf,
    title: String,
    created: String,
    updated: String,
    body: String,
    saved_body: String,
    display: String,
    display_src_len: usize,
    display_src_hash: u64,
    last_edit: Option<Instant>,
    /// Size and time of the file when it was read or saved here.
    stamp: Option<lock::Stamp>,
    /// Who is editing this page on another computer, if anyone.
    other_editor: Option<String>,
    checked_at: Instant,
}

enum Dialog {
    NewPage { dir: PathBuf, title: String, under: Option<String> },
    RenamePage { path: PathBuf, title: String },
    DeletePage { path: PathBuf, title: String, subpages: bool },
    NewSection { parent: PathBuf, name: String },
    RenameSection { path: PathBuf, name: String },
    DeleteSection { path: PathBuf, name: String, pages: usize },
    Message { title: String, text: String },
}

enum Action {
    SelectFolder(PathBuf),
    OpenPage(PathBuf),
    OpenHit(PathBuf),
    NewPage(PathBuf),
    NewSubpage(PathBuf),
    RenamePage(PathBuf),
    DeletePage(PathBuf),
    NewSection(PathBuf),
    RenameSection(PathBuf),
    DeleteSection(PathBuf),
    PrintPage(PathBuf),
    PrintSection(PathBuf),
    ShowInExplorer(PathBuf),
    StartEdit,
    StopEdit,
    Save,
    Refresh,
    /// Pick a folder and add it as another top-level folder.
    AddFolder,
    /// Show another top folder in the left column.
    SelectRoot(usize),
    /// Take a top-level folder out of the view (nothing is deleted on disk).
    RemoveRoot(PathBuf),
    ToggleSource,
    /// Show the earlier versions of a page.
    ShowHistory(PathBuf),
    /// Show what was deleted from the active top folder.
    ShowDeleted,
    /// Ask whether to remove all history of the active top folder.
    PurgeHistory,
    /// Open a hit of the history search (the hit itself: the list may be re-sorted before it is handled).
    OpenHistoryHit(Box<crate::history::HistoryHit>),
}

/// What the command line asks for at start.
#[derive(Default)]
pub struct Start {
    pub folders: Vec<PathBuf>,
    pub page: Option<PathBuf>,
    pub search: Option<String>,
    pub edit: bool,
    /// Open the history window of the start page (for screenshots).
    pub show_history: bool,
    /// Open the deleted items window (for screenshots).
    pub show_deleted: bool,
    /// Start with "Include history" ticked in the search.
    pub history: bool,
    pub screenshot: Option<PathBuf>,
}

pub struct NoteApp {
    /// The open top-level folders, in the order they were added.
    roots: Vec<PathBuf>,
    /// One tree per open top-level folder (same order as `roots`).
    trees: Vec<Folder>,
    /// Index into `roots` / `trees` of the top folder shown in the left column.
    active: usize,
    selected_folder: Option<PathBuf>,
    pages: Vec<PageEntry>,
    sort: SortOrder,
    page: Option<OpenPage>,
    mode: Mode,

    search: String,
    searched_for: String,
    hits: Vec<SearchHit>,
    index: Index,
    index_rx: Option<mpsc::Receiver<Index>>,
    index_stale: bool,
    page_search_for: String,

    md_cache: CommonMarkCache,
    dialog: Option<Dialog>,
    dialog_error: String,
    focus_dialog: bool,
    focus_search: bool,
    focus_editor: bool,
    /// Edit lock of the page being edited here.
    lock: Option<EditLock>,
    status: String,
    status_at: Instant,
    actions: Vec<Action>,

    screenshot: Option<PathBuf>,
    frames: u64,
    shot_requested: bool,
    shot_saved: bool,
    shot_frame: u64,
    shot_ui: Arc<Mutex<Option<image::RgbaImage>>>,
    shot_web: Arc<Mutex<Option<Vec<u8>>>>,
    shot_web_bounds: Option<[i32; 4]>,
    /// The formatted page view (WebView2); None until first needed.
    web: Option<PageWeb>,
    /// Why the formatted page view is not available (the simple view is used then).
    web_error: Option<String>,
    /// Where the page view goes in this frame.
    web_rect: Option<egui::Rect>,
    /// Edit the Markdown text instead of the formatted page.
    source_mode: bool,
    /// The window with the earlier versions of a page.
    history_win: Option<history_ui::HistoryWin>,
    /// The window with the deleted items of the active top folder.
    deleted_win: Option<history_ui::DeletedWin>,
    /// Also search old versions and deleted pages (not remembered).
    include_history: bool,
    history_search: history_ui::HistorySearch,
    /// An old text opened from the search results.
    history_view: Option<history_ui::HistoryView>,
}

impl NoteApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Start) -> Self {
        let Start { folders, page: start_page, search: start_search, edit: start_edit, show_history, show_deleted, history: start_history, screenshot } = start;
        setup_style(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let settings = Settings::load();
        let wanted_roots = if !folders.is_empty() {
            folders
        } else if !settings.roots.is_empty() {
            settings.roots.clone()
        } else {
            vec![PathBuf::from(DEFAULT_FOLDER)]
        };
        let missing: Vec<String> = wanted_roots.iter().filter(|p| !p.is_dir()).map(|p| p.display().to_string()).collect();
        let mut roots: Vec<PathBuf> = Vec::new();
        for r in wanted_roots.into_iter().filter(|p| p.is_dir()) {
            if store::check_new_root(&roots, &r).is_ok() {
                roots.push(r);
            }
        }

        let mut app = NoteApp {
            roots: Vec::new(),
            trees: Vec::new(),
            active: 0,
            selected_folder: None,
            pages: Vec::new(),
            sort: settings.sort,
            page: None,
            mode: Mode::View,
            search: String::new(),
            searched_for: String::new(),
            hits: Vec::new(),
            index: Index::default(),
            index_rx: None,
            index_stale: false,
            page_search_for: String::new(),
            md_cache: CommonMarkCache::default(),
            dialog: None,
            dialog_error: String::new(),
            focus_dialog: false,
            focus_search: false,
            focus_editor: false,
            lock: None,
            status: String::new(),
            status_at: Instant::now(),
            actions: Vec::new(),
            screenshot,
            frames: 0,
            shot_requested: false,
            shot_saved: false,
            shot_frame: 0,
            shot_ui: Arc::new(Mutex::new(None)),
            shot_web: Arc::new(Mutex::new(None)),
            shot_web_bounds: None,
            web: None,
            web_error: None,
            web_rect: None,
            source_mode: false,
            history_win: None,
            deleted_win: None,
            include_history: start_history,
            history_search: history_ui::HistorySearch::default(),
            history_view: None,
        };
        let want_page = start_page.clone().or(settings.page.clone());
        if roots.is_empty()
            && let Some(dir) = start_page.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()))
        {
            roots.push(dir);
        }
        if !roots.is_empty() {
            app.open_roots(roots, want_page.as_deref());
        }
        if !missing.is_empty() {
            app.set_status(format!("Folder not found: {}", missing.join(", ")));
        }
        if let Some(q) = start_search {
            app.search = q;
            if let Some(rx) = app.index_rx.take() {
                if let Ok(idx) = rx.recv() {
                    app.index = idx;
                }
            }
        }
        if start_edit && app.page.is_some() {
            app.start_edit();
        }
        if show_history && let Some(p) = app.page.as_ref().map(|p| p.path.clone()) {
            app.open_history(p);
        }
        if show_deleted {
            app.open_deleted(false);
        }
        app
    }

    // -----------------------------------------------------------------------------------------
    // Data

    /// Replaces the open top-level folders with `roots` and selects a page.
    fn open_roots(&mut self, roots: Vec<PathBuf>, want_page: Option<&Path>) {
        self.save_if_dirty();
        self.page = None;
        self.lock = None;
        self.mode = Mode::View;
        self.trees = roots.iter().map(|r| store::scan_tree(r)).collect();
        self.roots = roots;
        self.active = 0;
        self.selected_folder = None;
        self.pages.clear();
        self.start_indexing();

        let wanted = want_page.filter(|p| p.is_file() && self.roots.iter().any(|r| p.starts_with(r))).map(|p| p.to_path_buf());
        if let Some(p) = wanted {
            let dir = self.folder_of_page(&p);
            self.select_folder(dir, false);
            self.open_page(&p);
        } else if let Some(first) = self.trees.iter().find_map(first_folder_with_pages) {
            self.select_folder(first, true);
        }
        self.remember();
    }

    /// Adds one more top-level folder; the page being viewed stays open.
    fn add_root(&mut self, dir: PathBuf) {
        if let Err(e) = store::check_new_root(&self.roots, &dir) {
            self.set_status(e);
            return;
        }
        self.save_if_dirty();
        self.trees.push(store::scan_tree(&dir));
        self.roots.push(dir);
        self.active = self.roots.len() - 1;
        self.start_indexing();
        self.remember();
    }

    /// Takes a top-level folder out of the view. Nothing is deleted on disk.
    fn remove_root(&mut self, dir: &Path) {
        let Some(i) = self.roots.iter().position(|r| r == dir) else { return };
        self.save_if_dirty();
        self.roots.remove(i);
        self.trees.remove(i);
        if i < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.roots.len().saturating_sub(1));
        if self.selected_folder.as_ref().is_some_and(|s| s.starts_with(dir)) {
            self.selected_folder = None;
            self.pages.clear();
        }
        if self.page.as_ref().is_some_and(|p| p.path.starts_with(dir)) {
            self.page = None;
            self.lock = None;
            self.mode = Mode::View;
        }
        self.search.clear();
        self.start_indexing();
        self.remember();
    }

    fn start_indexing(&mut self) {
        if self.roots.is_empty() {
            self.index = Index::default();
            self.index_rx = None;
            self.index_stale = true;
            return;
        }
        let roots = self.roots.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Index::build(&roots));
        });
        self.index_rx = Some(rx);
    }

    /// The section folder a page belongs to (walks up out of subpage folders).
    fn folder_of_page(&self, page: &Path) -> PathBuf {
        let mut dir = page.parent().unwrap_or(Path::new(".")).to_path_buf();
        while dir.parent().is_some_and(|p| p.join(format!("{}.md", store::file_stem(&dir))).is_file()) {
            dir = dir.parent().unwrap().to_path_buf();
        }
        dir
    }

    /// Shows the top folder that holds `dir` in the left column.
    fn show_root_of(&mut self, dir: &Path) {
        if let Some(i) = self.roots.iter().position(|r| dir.starts_with(r)) {
            self.active = i;
        }
    }

    fn select_folder(&mut self, dir: PathBuf, open_first: bool) {
        self.show_root_of(&dir);
        if self.selected_folder.as_ref() != Some(&dir) {
            self.selected_folder = Some(dir.clone());
        }
        self.pages = store::list_pages(&dir, self.sort);
        if open_first {
            if let Some(first) = self.pages.first().map(|p| p.path.clone()) {
                self.open_page(&first);
            } else {
                self.save_if_dirty();
                self.page = None;
            }
        }
    }

    fn reload_pages(&mut self) {
        if let Some(dir) = self.selected_folder.clone() {
            self.pages = store::list_pages(&dir, self.sort);
        }
    }

    fn reload_tree(&mut self) {
        self.trees = self.roots.iter().map(|r| store::scan_tree(r)).collect();
    }

    fn open_page(&mut self, path: &Path) {
        if self.page.as_ref().is_some_and(|p| p.path == path) {
            return;
        }
        self.save_if_dirty();
        match store::read_doc(path) {
            Ok(doc) => {
                let body = doc.body.clone();
                self.page = Some(OpenPage {
                    path: path.to_path_buf(),
                    title: doc.title_or(path),
                    created: doc.get("created").unwrap_or("").to_string(),
                    updated: doc.get("updated").unwrap_or("").to_string(),
                    saved_body: body.clone(),
                    body,
                    display: String::new(),
                    display_src_len: usize::MAX,
                    display_src_hash: 0,
                    last_edit: None,
                    stamp: lock::stamp(path),
                    other_editor: lock::held_by_other(path).map(|h| h.describe()),
                    checked_at: Instant::now(),
                });
                self.mode = Mode::View;
                self.md_cache = CommonMarkCache::default();
                self.page_search_for.clear();
                self.remember();
            }
            Err(e) => self.set_status(format!("Could not open \"{}\": {e}", path.display())),
        }
    }

    fn save_if_dirty(&mut self) {
        let Some(p) = &mut self.page else { return };
        if p.body == p.saved_body {
            return;
        }
        // Never overwrite work from another computer.
        let editing_here = self.lock.as_ref().is_some_and(|l| l.page == p.path);
        if editing_here && !self.lock.as_ref().is_some_and(|l| l.still_mine()) {
            self.save_conflict("Another computer took over editing of this page.");
            return;
        }
        if !editing_here && let Some(h) = lock::held_by_other(&p.path) {
            p.body = p.saved_body.clone();
            p.last_edit = None;
            p.display_src_len = usize::MAX;
            self.md_cache = CommonMarkCache::default();
            self.show_message("Page in use", format!("Your change was not saved: this page is being edited {}.", h.describe()));
            return;
        }
        if p.path.exists() && lock::stamp(&p.path) != p.stamp {
            self.save_conflict("The page was changed on another computer while you were working on it.");
            return;
        }
        match store::save_body(&p.path, &p.body) {
            Ok(()) => {
                p.saved_body = p.body.clone();
                p.last_edit = None;
                p.stamp = lock::stamp(&p.path);
                p.updated = store::now_stamp();
                let path = p.path.clone();
                self.index.add(&path);
                self.searched_for.clear();
                match store::take_history_warning() {
                    Some(w) => self.set_status(format!("Saved, but the history copy failed: {w}")),
                    None => self.set_status("Saved".into()),
                }
            }
            Err(e) => self.set_status(e),
        }
    }

    /// Keeps the text from here as a separate page instead of overwriting the other version.
    fn save_conflict(&mut self, reason: &str) {
        let Some(p) = self.page.take() else { return };
        self.lock = None;
        self.mode = Mode::View;
        let copy = store::save_conflict_copy(&p.path, &p.body, &lock::computer());
        // Show the version that is on disk now.
        if p.path.exists() {
            self.open_page(&p.path);
            self.index.add(&p.path);
        }
        let text = match copy {
            Ok(c) => {
                self.index.add(&c);
                format!(
                    "{reason}\n\nYour version was kept as the page \"{}\" next to it, so nothing is lost.",
                    store::file_stem(&c)
                )
            }
            Err(e) => format!("{reason}\n\nYour version could not be kept: {e}"),
        };
        self.index_stale = true;
        self.reload_pages();
        self.show_message("Page changed elsewhere", text);
    }

    /// Switches to editing, unless another computer is editing the page.
    fn start_edit(&mut self) {
        let Some(path) = self.page.as_ref().map(|p| p.path.clone()) else { return };
        if self.mode == Mode::Edit && self.lock.as_ref().is_some_and(|l| l.page == path) {
            return;
        }
        match EditLock::acquire(&path) {
            Ok(l) => {
                self.lock = Some(l);
                // Edit the newest version.
                self.reload_if_changed(true);
                if let Some(p) = &mut self.page {
                    p.other_editor = None;
                }
                self.mode = Mode::Edit;
                self.focus_editor = true;
            }
            Err(e) => {
                if let Some(p) = &mut self.page {
                    p.other_editor = lock::held_by_other(&path).map(|h| h.describe());
                }
                self.show_message("Page in use", e);
            }
        }
    }

    /// Picks up changes saved on another computer (only when nothing is unsaved here).
    fn reload_if_changed(&mut self, force: bool) {
        let Some(p) = &mut self.page else { return };
        if !force && p.checked_at.elapsed() < Duration::from_secs(3) {
            return;
        }
        p.checked_at = Instant::now();
        if self.mode == Mode::View {
            p.other_editor = lock::held_by_other(&p.path).map(|h| h.describe());
        }
        if p.body != p.saved_body {
            return;
        }
        let now = lock::stamp(&p.path);
        if now.is_none() || now == p.stamp {
            return;
        }
        if let Ok(doc) = store::read_doc(&p.path) {
            p.title = doc.title_or(&p.path);
            p.created = doc.get("created").unwrap_or("").to_string();
            p.updated = doc.get("updated").unwrap_or("").to_string();
            p.body = doc.body.clone();
            p.saved_body = doc.body;
            p.stamp = now;
            p.display_src_len = usize::MAX;
            let path = p.path.clone();
            self.index.add(&path);
            self.index_stale = true;
            self.md_cache = CommonMarkCache::default();
            self.page_search_for.clear();
        }
    }

    fn show_message(&mut self, title: &str, text: String) {
        self.dialog = Some(Dialog::Message { title: title.into(), text });
        self.dialog_error.clear();
        self.focus_dialog = true;
    }

    fn set_status(&mut self, s: String) {
        self.status = s;
        self.status_at = Instant::now();
    }

    fn remember(&self) {
        if self.screenshot.is_some() {
            return;
        }
        Settings { roots: self.roots.clone(), page: self.page.as_ref().map(|p| p.path.clone()), sort: self.sort }.save();
    }

    fn run_search(&mut self) {
        if self.search.trim() == self.searched_for.trim() && !self.index_stale {
            return;
        }
        self.index_stale = false;
        self.searched_for = self.search.clone();
        self.hits = self.index.search(&self.search, 300);
    }

    // -----------------------------------------------------------------------------------------
    // Actions

    fn apply_actions(&mut self) {
        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            match a {
                Action::SelectFolder(dir) => {
                    self.save_if_dirty();
                    self.select_folder(dir, true);
                }
                Action::OpenPage(p) => self.open_page(&p),
                Action::OpenHit(p) => {
                    let dir = self.folder_of_page(&p);
                    if self.selected_folder.as_ref() != Some(&dir) {
                        self.select_folder(dir, false);
                    }
                    self.open_page(&p);
                }
                Action::NewPage(dir) => self.show_dialog(Dialog::NewPage { dir, title: String::new(), under: None }),
                Action::NewSubpage(page) => {
                    let title = store::read_doc(&page).map(|d| d.title_or(&page)).unwrap_or_default();
                    self.show_dialog(Dialog::NewPage { dir: store::subpage_dir(&page), title: String::new(), under: Some(title) })
                }
                Action::RenamePage(p) => {
                    let title = store::read_doc(&p).map(|d| d.title_or(&p)).unwrap_or_default();
                    self.show_dialog(Dialog::RenamePage { path: p, title })
                }
                Action::DeletePage(p) => {
                    let title = store::read_doc(&p).map(|d| d.title_or(&p)).unwrap_or_default();
                    let subpages = store::subpage_dir(&p).is_dir();
                    self.show_dialog(Dialog::DeletePage { path: p, title, subpages })
                }
                Action::NewSection(parent) => self.show_dialog(Dialog::NewSection { parent, name: String::new() }),
                Action::RenameSection(path) => {
                    let name = folder_name(&path);
                    self.show_dialog(Dialog::RenameSection { path, name })
                }
                Action::DeleteSection(path) => {
                    let pages = Index::build(std::slice::from_ref(&path)).pages.len();
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    self.show_dialog(Dialog::DeleteSection { path, name, pages })
                }
                Action::PrintPage(p) => {
                    self.save_if_dirty();
                    self.print_pages(&[p]);
                }
                Action::PrintSection(dir) => {
                    self.save_if_dirty();
                    let pages: Vec<PathBuf> = store::list_pages(&dir, self.sort).into_iter().map(|p| p.path).collect();
                    if pages.is_empty() {
                        self.set_status("This section has no pages to print.".into());
                    } else {
                        self.print_pages(&pages);
                    }
                }
                Action::ShowInExplorer(p) => {
                    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
                }
                Action::StartEdit => self.start_edit(),
                Action::StopEdit => {
                    self.save_if_dirty();
                    self.mode = Mode::View;
                    self.lock = None;
                    self.md_cache = CommonMarkCache::default();
                    self.page_search_for.clear();
                    if let Some(p) = &mut self.page {
                        p.display_src_len = usize::MAX;
                    }
                }
                Action::Save => self.save_if_dirty(),
                Action::Refresh => {
                    self.save_if_dirty();
                    self.reload_tree();
                    self.reload_pages();
                    if let Some(p) = self.page.take() {
                        if p.path.is_file() {
                            self.open_page(&p.path);
                        }
                    }
                    self.start_indexing();
                    self.set_status("Notes reloaded from disk".into());
                }
                Action::ToggleSource => {
                    self.source_mode = !self.source_mode;
                    self.focus_editor = true;
                    if let Some(w) = &mut self.web {
                        w.shown = None;
                    }
                    if let Some(p) = &mut self.page {
                        p.display_src_len = usize::MAX;
                    }
                }
                Action::AddFolder => {
                    let mut dlg = rfd::FileDialog::new().set_title("Choose a folder of notes to add");
                    if let Some(parent) = self.roots.last().and_then(|r| r.parent()) {
                        dlg = dlg.set_directory(parent);
                    }
                    if let Some(dir) = dlg.pick_folder() {
                        self.add_root(dir);
                    }
                }
                Action::SelectRoot(i) => {
                    if i < self.roots.len() && i != self.active {
                        self.save_if_dirty();
                        self.active = i;
                        let target = self.trees.get(i).and_then(first_folder_with_pages).unwrap_or_else(|| self.roots[i].clone());
                        self.select_folder(target, true);
                    }
                }
                Action::RemoveRoot(dir) => self.remove_root(&dir),
                Action::ShowHistory(p) => self.open_history(p),
                Action::ShowDeleted => self.open_deleted(false),
                Action::PurgeHistory => self.open_deleted(true),
                Action::OpenHistoryHit(hit) => self.open_history_hit(*hit),
            }
        }
    }

    fn show_dialog(&mut self, d: Dialog) {
        self.save_if_dirty();
        if self.mode == Mode::Edit {
            self.mode = Mode::View;
            self.lock = None;
            self.md_cache = CommonMarkCache::default();
            if let Some(p) = &mut self.page {
                p.display_src_len = usize::MAX;
            }
        }
        self.dialog = Some(d);
        self.dialog_error.clear();
        self.focus_dialog = true;
    }

    fn print_pages(&mut self, paths: &[PathBuf]) {
        let docs: Vec<(PathBuf, store::PageDoc)> =
            paths.iter().filter_map(|p| store::read_doc(p).ok().map(|d| (p.clone(), d))).collect();
        let titles: Vec<String> = docs.iter().map(|(p, d)| d.title_or(p)).collect();
        let pages: Vec<render::PrintPage> = docs
            .iter()
            .zip(titles.iter())
            .map(|((p, d), t)| render::PrintPage {
                title: t,
                created: d.get("created").unwrap_or(""),
                body: &d.body,
                dir: p.parent().unwrap_or(Path::new(".")),
            })
            .collect();
        let doc_title = if titles.len() == 1 { titles[0].clone() } else { format!("{} pages", titles.len()) };
        let html = render::print_document(&doc_title, &pages);
        let dir = std::env::temp_dir().join("NoteCraft");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("print-{}.html", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        match std::fs::write(&file, html) {
            Ok(()) => match open::that_detached(&file) {
                Ok(()) => self.set_status("The print view opened in your web browser".into()),
                Err(e) => self.set_status(format!("Could not open the print view: {e}")),
            },
            Err(e) => self.set_status(format!("Could not prepare the printout: {e}")),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Drawing

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(ACCENT).inner_margin(Margin::symmetric(12, 8));
        egui::Panel::top("top").frame(frame).show_separator_line(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("NoteCraft").color(Color32::WHITE).size(18.0).strong());
                ui.add_space(16.0);
                let roots = self.roots.clone();
                let (b, _) = egui::containers::menu::MenuButton::from_button(top_button("📁  Folders ▾")).ui(ui, |ui| {
                    if ui.button("Add folder…").clicked() {
                        self.actions.push(Action::AddFolder);
                        ui.close();
                    }
                    if !roots.is_empty() {
                        ui.separator();
                        if ui.button("Deleted items…").on_hover_text("Restore pages and sections that were deleted").clicked() {
                            self.actions.push(Action::ShowDeleted);
                            ui.close();
                        }
                        if ui.button("Purge history…").on_hover_text("Remove all old versions and deleted items for good").clicked() {
                            self.actions.push(Action::PurgeHistory);
                            ui.close();
                        }
                        ui.separator();
                    }
                    for r in &roots {
                        if ui.button(format!("Remove \"{}\" from view", folder_name(r))).on_hover_text(r.display().to_string()).clicked() {
                            self.actions.push(Action::RemoveRoot(r.clone()));
                            ui.close();
                        }
                    }
                });
                b.hover_fx().on_hover_text(if roots.is_empty() {
                    "Choose the folders that hold your notes".to_string()
                } else {
                    roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join("\n")
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(top_button("⟳")).hover_fx().on_hover_text("Reload notes from disk (F5)").clicked() {
                        self.actions.push(Action::Refresh);
                    }
                    ui.add_space(8.0);
                    ui.add_space(8.0);
                    ui.scope(|ui| {
                        ui.visuals_mut().override_text_color = Some(Color32::WHITE);
                        ui.checkbox(&mut self.include_history, "Include history")
                            .on_hover_text("Also look in old versions of pages and in deleted pages (slower)");
                    });
                    ui.add_space(8.0);
                    let w = (ui.available_width() - 40.0).clamp(160.0, 460.0);
                    let id = Id::unique("search-box");
                    let resp = ui.add(
                        TextEdit::singleline(&mut self.search)
                            .id(id)
                            .hint_text("🔍  Search all notes (Ctrl+F)")
                            .desired_width(w)
                            .margin(Margin::symmetric(8, 5)),
                    );
                    if self.focus_search {
                        resp.request_focus();
                        self.focus_search = false;
                    }
                    if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                        self.search.clear();
                    }
                });
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(NAV_BG).inner_margin(Margin::symmetric(10, 3)).stroke(Stroke::new(1.0, BORDER));
        egui::Panel::bottom("status").frame(frame).show_separator_line(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                let fresh = self.status_at.elapsed() < Duration::from_secs(6);
                if fresh && !self.status.is_empty() {
                    ui.label(RichText::new(&self.status).color(TEXT).size(12.0));
                } else if let Some(p) = &self.page {
                    let dirty = p.body != p.saved_body;
                    let txt = if self.mode == Mode::Edit {
                        if dirty { "Editing — changes are saved automatically" } else { "Editing — all changes saved" }
                    } else {
                        "Ctrl+E edit · Ctrl+N new page · Ctrl+P print · Ctrl+F search"
                    };
                    ui.label(RichText::new(txt).color(MUTED).size(12.0));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let txt = if self.index_rx.is_some() {
                        "Reading notes…".to_string()
                    } else {
                        format!("{} pages", self.index.pages.len())
                    };
                    ui.label(RichText::new(txt).color(MUTED).size(12.0));
                });
            });
        });
    }

    fn nav_panel(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(NAV_BG).inner_margin(Margin::symmetric(6, 8));
        egui::Panel::left("nav").frame(frame).resizable(true).default_size(250.0).size_range(170.0..=480.0).show(ui, |ui| {
            // A fixed row height: with a right-to-left layout inside `horizontal` the row's height is
            // not settled in the first frame and the rows below jump when the pointer hovers a button.
            // The row is allocated first and its content is placed inside that rectangle, so the content
            // cannot change how far the rows below are pushed (the button is about 24 px tall itself).
            let (_, header) = ui.allocate_space(egui::vec2(ui.available_width(), 28.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(header).layout(Layout::left_to_right(Align::Center)), |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("Notebooks").color(MUTED).size(13.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(root) = self.roots.get(self.active).cloned() {
                        let target = self.selected_folder.clone().unwrap_or(root);
                        if ui
                            .add(small_button("+ Section")).hover_fx()
                            .on_hover_text(format!("New section inside \"{}\"", folder_name(&target)))
                            .clicked()
                        {
                            self.actions.push(Action::NewSection(target));
                        }
                    }
                });
            });
            ui.add_space(4.0);
            if self.trees.is_empty() {
                ui.label(RichText::new("No folder open").color(MUTED));
                return;
            }
            // Which top folder is shown.
            let names: Vec<String> = self.roots.iter().map(|r| folder_name(r)).collect();
            let mut chosen = self.active;
            egui::ComboBox::from_id_salt("top-folder")
                .width(ui.available_width() - 8.0)
                .selected_text(RichText::new(names.get(self.active).cloned().unwrap_or_default()).strong())
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut chosen, i, n.as_str()).on_hover_text(self.roots[i].display().to_string());
                    }
                });
            if chosen != self.active {
                self.actions.push(Action::SelectRoot(chosen));
            }
            ui.add_space(4.0);
            let Some(tree) = self.trees.get(self.active).cloned() else { return };
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for child in &tree.children {
                    self.folder_row(ui, child, 0);
                }
                if tree.page_count > 0 {
                    // Pages lying directly in the top folder.
                    self.folder_leaf(ui, &tree, 0, "(top folder)");
                }
            });
        });
    }

    fn folder_row(&mut self, ui: &mut egui::Ui, f: &Folder, depth: usize) {
        if f.children.is_empty() {
            self.folder_leaf(ui, f, depth, &f.name);
            return;
        }
        let id = Id::unique(("folder", &f.path));
        let on_path = self.selected_folder.as_ref().is_some_and(|s| s.starts_with(&f.path) && s != &f.path);
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, depth < 2 || on_path);
        if on_path && !state.is_open() && self.frames < 3 {
            state.set_open(true);
        }
        let header = state.show_header(ui, |ui| {
            self.folder_label(ui, f, depth, &f.name);
        });
        header.body(|ui| {
            for c in &f.children {
                self.folder_row(ui, c, depth + 1);
            }
        });
    }

    fn folder_leaf(&mut self, ui: &mut egui::Ui, f: &Folder, depth: usize, name: &str) {
        ui.horizontal(|ui| {
            ui.add_space(ui.spacing().indent);
            self.folder_label(ui, f, depth, name);
        });
    }

    fn folder_label(&mut self, ui: &mut egui::Ui, f: &Folder, depth: usize, name: &str) {
        let selected = self.selected_folder.as_ref() == Some(&f.path);
        let width = ui.available_width().max(60.0);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
        let painter = ui.painter_at(rect);
        if selected {
            painter.rect_filled(rect, CornerRadius::same(4), ACCENT_SOFT);
        } else if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(4), HOVER_SOFT);
        }
        let is_notebook = depth == 0 && !f.children.is_empty();
        let color = if is_notebook { ACCENT } else { section_color(name) };
        let chip = egui::Rect::from_min_size(rect.left_center() + Vec2::new(4.0, -7.0), Vec2::new(if is_notebook { 12.0 } else { 5.0 }, 14.0));
        painter.rect_filled(chip, CornerRadius::same(2), color);
        let text_x = chip.right() + 7.0;
        let font = FontId::proportional(14.0);
        let mut label = name.to_string();
        if f.page_count > 0 {
            label.push_str(&format!("  ({})", f.page_count));
        }
        let galley = painter.layout_no_wrap(label, font, if selected { ACCENT_DARK } else { TEXT });
        painter.galley(egui::pos2(text_x, rect.center().y - galley.size().y / 2.0), galley, TEXT);
        if resp.clicked() {
            self.actions.push(Action::SelectFolder(f.path.clone()));
        }
        let path = f.path.clone();
        let is_root = self.roots.contains(&path);
        let resp = resp.on_hover_text(name);
        resp.context_menu(|ui| {
            if ui.button("New page here").clicked() {
                self.actions.push(Action::NewPage(path.clone()));
                ui.close();
            }
            if ui.button("New section inside").clicked() {
                self.actions.push(Action::NewSection(path.clone()));
                ui.close();
            }
            ui.separator();
            if is_root {
                if ui.button("Remove from NoteCraft").on_hover_text("Only takes it out of the view; no files are deleted").clicked() {
                    self.actions.push(Action::RemoveRoot(path.clone()));
                    ui.close();
                }
                ui.separator();
            } else {
                if ui.button("Rename…").clicked() {
                    self.actions.push(Action::RenameSection(path.clone()));
                    ui.close();
                }
                if ui.button("Delete…").clicked() {
                    self.actions.push(Action::DeleteSection(path.clone()));
                    ui.close();
                }
                ui.separator();
            }
            if ui.button("Print all pages").clicked() {
                self.actions.push(Action::PrintSection(path.clone()));
                ui.close();
            }
            if ui.button("Show in File Explorer").clicked() {
                self.actions.push(Action::ShowInExplorer(path.clone()));
                ui.close();
            }
        });
    }

    fn list_panel(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(LIST_BG).inner_margin(Margin::symmetric(6, 8)).stroke(Stroke::new(1.0, BORDER));
        let searching = !self.search.trim().is_empty();
        egui::Panel::left("pages").frame(frame).resizable(true).default_size(if searching { 340.0 } else { 260.0 }).size_range(180.0..=560.0).show(ui, |ui| {
            if searching {
                self.search_results(ui);
            } else {
                self.page_list(ui);
            }
        });
    }

    fn page_list(&mut self, ui: &mut egui::Ui) {
        let Some(dir) = self.selected_folder.clone() else {
            ui.label(RichText::new("Pick a section on the left").color(MUTED));
            return;
        };
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(folder_name(&dir)).color(TEXT).size(15.0).strong());
        });
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            if ui.add(accent_button("+ Page")).hover_fx().on_hover_text("New page in this section (Ctrl+N)").clicked() {
                self.actions.push(Action::NewPage(dir.clone()));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let before = self.sort;
                egui::ComboBox::from_id_salt("sort")
                    .width(96.0)
                    .selected_text(match self.sort {
                        SortOrder::Name => "By name",
                        SortOrder::Created => "Newest first",
                        SortOrder::Updated => "Last changed",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.sort, SortOrder::Name, "By name");
                        ui.selectable_value(&mut self.sort, SortOrder::Created, "Newest first");
                        ui.selectable_value(&mut self.sort, SortOrder::Updated, "Last changed");
                    });
                if before != self.sort {
                    self.reload_pages();
                    self.remember();
                }
            });
        });
        ui.add_space(6.0);
        if self.pages.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No pages here yet").color(MUTED));
            });
            return;
        }
        let current = self.page.as_ref().map(|p| p.path.clone());
        let pages = self.pages.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for pg in &pages {
                let selected = current.as_ref() == Some(&pg.path);
                let indent = 14.0 * pg.level as f32;
                let date = if pg.created.len() >= 10 { pg.created[..10].to_string() } else { String::new() };
                let resp = list_row(ui, &pg.title, &date, selected, indent, 40.0);
                if resp.clicked() {
                    self.actions.push(Action::OpenPage(pg.path.clone()));
                }
                self.page_context_menu(&resp, &pg.path);
            }
        });
    }

    fn page_context_menu(&mut self, resp: &egui::Response, path: &Path) {
        let path = path.to_path_buf();
        resp.context_menu(|ui| {
            if ui.button("Rename…").clicked() {
                self.actions.push(Action::RenamePage(path.clone()));
                ui.close();
            }
            if ui.button("New subpage…").clicked() {
                self.actions.push(Action::NewSubpage(path.clone()));
                ui.close();
            }
            if ui.button("Print").clicked() {
                self.actions.push(Action::PrintPage(path.clone()));
                ui.close();
            }
            if ui.button("History…").clicked() {
                self.actions.push(Action::ShowHistory(path.clone()));
                ui.close();
            }
            if ui.button("Show in File Explorer").clicked() {
                self.actions.push(Action::ShowInExplorer(path.clone()));
                ui.close();
            }
            ui.separator();
            if ui.button("Delete…").clicked() {
                self.actions.push(Action::DeletePage(path.clone()));
                ui.close();
            }
        });
    }

    fn search_results(&mut self, ui: &mut egui::Ui) {
        self.run_search();
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let n = self.hits.len();
            let txt = if self.index_rx.is_some() && self.index.pages.is_empty() {
                "Reading your notes…".to_string()
            } else if n == 0 {
                "No pages found".to_string()
            } else if n >= 300 {
                "300+ pages found".to_string()
            } else if n == 1 {
                "1 page found".to_string()
            } else {
                format!("{n} pages found")
            };
            ui.label(RichText::new(txt).color(TEXT).size(15.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(small_button("✕ Clear")).hover_fx().clicked() {
                    self.search.clear();
                }
            });
        });
        ui.add_space(6.0);
        let current = self.page.as_ref().map(|p| p.path.clone());
        let hits: Vec<(PathBuf, String, String, String, usize)> =
            self.hits.iter().map(|h| (h.path.clone(), h.title.clone(), h.place.clone(), h.snippet.clone(), h.count)).collect();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (path, title, place, snippet, count) in &hits {
                let selected = current.as_ref() == Some(path);
                // Two pages may share a title, so each row is told apart by its file.
                let resp = ui.push_id(path, |ui| search_row(ui, title, place, snippet, *count, selected)).inner;
                if resp.clicked() {
                    self.actions.push(Action::OpenHit(path.clone()));
                }
                self.page_context_menu(&resp, path);
            }
            self.history_rows(ui);
        });
    }

    fn page_panel(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(PAGE_BG).inner_margin(Margin { left: 28, right: 20, top: 16, bottom: 8 });
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            if self.roots.is_empty() {
                self.welcome(ui);
                return;
            }
            let Some(page) = &self.page else {
                ui.add_space(60.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No page selected").size(18.0).color(MUTED));
                    if let Some(dir) = self.selected_folder.clone() {
                        ui.add_space(10.0);
                        if ui.add(accent_button("+ New page")).hover_fx().clicked() {
                            self.actions.push(Action::NewPage(dir));
                        }
                    }
                });
                return;
            };
            let path = page.path.clone();
            let title = page.title.clone();
            let created = page.created.clone();
            let updated = page.updated.clone();

            // Title row with the page tools.
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.menu_button(RichText::new("⋯").size(18.0), |ui| {
                        if ui.button("Rename…").clicked() {
                            self.actions.push(Action::RenamePage(path.clone()));
                        }
                        if ui.button("New subpage…").clicked() {
                            self.actions.push(Action::NewSubpage(path.clone()));
                        }
                        if ui.button("History…").on_hover_text("Earlier versions of this page").clicked() {
                            self.actions.push(Action::ShowHistory(path.clone()));
                            ui.close();
                        }
                        if ui.button("Show in File Explorer").clicked() {
                            self.actions.push(Action::ShowInExplorer(path.clone()));
                        }
                        ui.separator();
                        if ui.button("Delete…").clicked() {
                            self.actions.push(Action::DeletePage(path.clone()));
                        }
                    });
                    if ui.add(tool_button("🖶 Print")).hover_fx().on_hover_text("Print this page (Ctrl+P)").clicked() {
                        self.actions.push(Action::PrintPage(path.clone()));
                    }
                    if ui.add(tool_button("Delete")).hover_fx().on_hover_text("Delete this page (restore it later from Folders > Deleted items)").clicked() {
                        self.actions.push(Action::DeletePage(path.clone()));
                    }
                    if ui.add(tool_button("Rename")).hover_fx().clicked() {
                        self.actions.push(Action::RenamePage(path.clone()));
                    }
                    if self.mode == Mode::Edit {
                        if ui.add(accent_button("✔ Done")).hover_fx().on_hover_text("Save and go back to reading (Esc)").clicked() {
                            self.actions.push(Action::StopEdit);
                        }
                        if self.web_error.is_none() {
                            let (label, tip) = if self.source_mode {
                                ("Formatted", "Edit the page as it looks")
                            } else {
                                ("Markdown", "Edit the Markdown text of the page")
                            };
                            if ui.add(tool_button(label)).hover_fx().on_hover_text(tip).clicked() {
                                self.actions.push(Action::ToggleSource);
                            }
                        }
                    } else if ui.add(accent_button("✎ Edit")).hover_fx().on_hover_text("Change this page (Ctrl+E)").clicked() {
                        self.actions.push(Action::StartEdit);
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(&title).size(26.0).color(TEXT)).truncate());
                    });
                });
            });
            let mut meta = Vec::new();
            if !created.is_empty() {
                meta.push(format!("Created {}", render::friendly_date(&created)));
            }
            if !updated.is_empty() && updated != created {
                meta.push(format!("changed {}", render::friendly_date(&updated)));
            }
            if !meta.is_empty() {
                ui.label(RichText::new(meta.join(" · ")).size(12.0).color(MUTED));
            }
            ui.add_space(4.0);
            let r = ui.available_rect_before_wrap();
            ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, BORDER));
            ui.add_space(8.0);

            let formatted = self.web_error.is_none() && !(self.mode == Mode::Edit && self.source_mode);
            match self.mode {
                Mode::View if formatted => {
                    if let Some(who) = self.page.as_ref().and_then(|p| p.other_editor.clone()) {
                        other_editor_banner(ui, &who);
                    }
                    let r = ui.available_rect_before_wrap();
                    ui.allocate_rect(r, Sense::hover());
                    self.web_rect = Some(r);
                }
                Mode::Edit if formatted => {
                    let r = ui.available_rect_before_wrap();
                    ui.allocate_rect(r, Sense::hover());
                    self.web_rect = Some(r);
                }
                Mode::View => self.page_view(ui),
                Mode::Edit => self.page_editor(ui),
            }
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.add_space(80.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Welcome to NoteCraft").size(26.0).color(TEXT));
            ui.add_space(8.0);
            ui.label(RichText::new("Choose the folder that holds your notes (Markdown files in folders).").color(MUTED));
            ui.label(
                RichText::new("A shared network folder works too: several computers can use it, one person edits a page at a time.")
                    .color(MUTED),
            );
            ui.add_space(16.0);
            if ui.add(accent_button("📁 Open notes folder…")).hover_fx().clicked() {
                self.actions.push(Action::AddFolder);
            }
            ui.add_space(10.0);
            ui.label(RichText::new("NoteCraft remembers the open folders on this computer; add more from the Folders menu.").size(12.0).color(MUTED));
        });
    }

    fn refresh_display(page: &mut OpenPage) {
        let hash = quick_hash(&page.body);
        if page.display_src_len == page.body.len() && page.display_src_hash == hash {
            return;
        }
        page.display = render::display_markdown(&page.body).into_owned();
        page.display_src_len = page.body.len();
        page.display_src_hash = hash;
    }

    fn page_view(&mut self, ui: &mut egui::Ui) {
        let search = self.search.trim().to_string();
        let Some(page) = &mut self.page else { return };
        Self::refresh_display(page);
        let dir = page.path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let viewer_id = Id::unique("page-view");

        // Highlight the search words on the page.
        if search != self.page_search_for {
            let words = search.split_whitespace().next().unwrap_or("").to_string();
            self.md_cache.search_cache_mut(&viewer_id).search_query = words;
            self.md_cache.update_search_matches(&viewer_id, &page.display);
            self.page_search_for = search.clone();
        }

        if let Some(who) = &page.other_editor {
            other_editor_banner(ui, who);
        }
        let max_w = (ui.available_width() - 20.0).max(100.0) as usize;
        let mut toggled = false;
        egui::ScrollArea::vertical().id_salt(("page-scroll", &page.path)).auto_shrink([false, false]).show(ui, |ui| {
            ui.style_mut().spacing.item_spacing.y = 6.0;
            let viewer = CommonMarkViewer::new()
                .default_implicit_uri_scheme(file_base_uri(&dir))
                .max_image_width(Some(max_w))
                .show_alt_text_on_hover(true);
            if search.is_empty() {
                // Checkboxes can be ticked right here; the same box is then ticked in the file.
                let mut shown = page.display.clone();
                let resp = viewer.show_mut(ui, &mut self.md_cache, &mut shown);
                if resp.response.changed() && shown != page.display {
                    let before = task_states(&page.display);
                    let after = task_states(&shown);
                    if let Some(n) = before.iter().zip(after.iter()).position(|(a, b)| a != b)
                        && let Some(body) = set_task(&page.body, n, after[n])
                    {
                        page.body = body;
                        toggled = true;
                    }
                }
            } else {
                viewer.show_with_id(viewer_id, ui, &mut self.md_cache, &page.display);
            }
            ui.add_space(40.0);
        });
        if toggled {
            self.save_if_dirty();
        }
    }

    fn page_editor(&mut self, ui: &mut egui::Ui) {
        let focus_editor = std::mem::take(&mut self.focus_editor);
        let Some(page) = &mut self.page else { return };
        let dir = page.path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let height = ui.available_height();
        let mut changed = false;
        ui.columns(2, |cols| {
            let left = &mut cols[0];
            left.label(RichText::new("Text (Markdown)").size(12.0).color(MUTED));
            egui::ScrollArea::vertical().id_salt("editor-scroll").max_height(height - 24.0).auto_shrink([false, false]).show(left, |ui| {
                let resp = ui.add(
                    TextEdit::multiline(&mut page.body)
                        .id(Id::unique(("editor", &page.path)))
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(30)
                        .lock_focus(true)
                        .margin(Margin::same(8)),
                );
                if resp.changed() {
                    changed = true;
                }
                if focus_editor {
                    resp.request_focus();
                }
            });
            let right = &mut cols[1];
            right.label(RichText::new("Preview").size(12.0).color(MUTED));
            Self::refresh_display(page);
            let max_w = (right.available_width() - 20.0).max(100.0) as usize;
            egui::ScrollArea::vertical().id_salt("preview-scroll").max_height(height - 24.0).auto_shrink([false, false]).show(right, |ui| {
                egui::Frame::new().inner_margin(Margin::same(8)).show(ui, |ui| {
                    CommonMarkViewer::new()
                        .default_implicit_uri_scheme(file_base_uri(&dir))
                        .max_image_width(Some(max_w))
                        .show(ui, &mut self.md_cache, &page.display);
                });
            });
        });
        if changed {
            page.last_edit = Some(Instant::now());
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut close = false;
        let mut confirm = false;
        let focus = std::mem::take(&mut self.focus_dialog);
        let modal = egui::Modal::new(Id::unique("dialog")).show(ctx, |ui| {
            ui.set_width(380.0);
            let (heading, prompt): (String, String) = match &dialog {
                Dialog::NewPage { under: Some(t), .. } => ("New subpage".into(), format!("Title of the new subpage under \"{t}\":")),
                Dialog::NewPage { .. } => ("New page".into(), "Title of the new page:".into()),
                Dialog::RenamePage { .. } => ("Rename page".into(), "New title:".into()),
                Dialog::DeletePage { title, subpages, .. } => (
                    "Delete page".into(),
                    if *subpages {
                        format!("Delete \"{title}\" and its subpages?")
                    } else {
                        format!("Delete \"{title}\"?")
                    },
                ),
                Dialog::NewSection { parent, .. } => ("New section".into(), format!("Name of the new section inside \"{}\":", folder_name(parent))),
                Dialog::RenameSection { .. } => ("Rename section".into(), "New name:".into()),
                Dialog::DeleteSection { name, pages, .. } => (
                    "Delete section".into(),
                    format!("Delete the section \"{name}\" with its {pages} page(s)?"),
                ),
                Dialog::Message { title, text } => (title.clone(), text.clone()),
            };
            let is_message = matches!(dialog, Dialog::Message { .. });
            ui.label(RichText::new(heading).size(18.0).color(TEXT));
            ui.add_space(6.0);
            ui.label(prompt);
            let text: Option<&mut String> = match &mut dialog {
                Dialog::NewPage { title, .. } | Dialog::RenamePage { title, .. } => Some(title),
                Dialog::NewSection { name, .. } | Dialog::RenameSection { name, .. } => Some(name),
                _ => None,
            };
            let is_delete = text.is_none();
            if let Some(t) = text {
                let r = ui.add(TextEdit::singleline(t).desired_width(f32::INFINITY).margin(Margin::symmetric(6, 5)));
                if focus {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    confirm = true;
                }
            } else if !is_message {
                ui.label(RichText::new("You can restore it later from Folders > Deleted items.").size(12.0).color(MUTED));
            }
            if !self.dialog_error.is_empty() {
                ui.label(RichText::new(&self.dialog_error).color(Color32::from_rgb(0xC0, 0x20, 0x20)));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !is_message && ui.button("Cancel").clicked() {
                        close = true;
                    }
                    let ok_label = if is_delete && !is_message { "Delete" } else { "OK" };
                    if ui.add(accent_button(ok_label)).hover_fx().clicked() {
                        confirm = true;
                    }
                });
            });
        });
        if modal.should_close() {
            close = true;
        }
        if close {
            return;
        }
        if !confirm {
            self.dialog = Some(dialog);
            return;
        }
        match self.do_dialog(&dialog) {
            Ok(()) => {}
            Err(e) => {
                self.dialog_error = e;
                self.dialog = Some(dialog);
            }
        }
    }

    fn do_dialog(&mut self, dialog: &Dialog) -> Result<(), String> {
        let need = |s: &str| if s.trim().is_empty() { Err("Please type a name.".to_string()) } else { Ok(()) };
        match dialog {
            Dialog::NewPage { dir, title, .. } => {
                need(title)?;
                let path = store::create_page(dir, title)?;
                self.index.add(&path);
                self.index_stale = true;
                self.reload_tree();
                let section = self.folder_of_page(&path);
                self.select_folder(section, false);
                self.open_page(&path);
                self.start_edit();
                self.set_status(format!("Created \"{}\"", title.trim()));
            }
            Dialog::RenamePage { path, title } => {
                need(title)?;
                page_free(path)?;
                let new_path = store::rename_page(path, title)?;
                self.index.remove_tree(&store::subpage_dir(path));
                self.index.remove(path);
                self.index.add_tree(&store::subpage_dir(&new_path));
                self.index.add(&new_path);
                self.index_stale = true;
                let was_open = self.page.as_ref().is_some_and(|p| &p.path == path);
                self.reload_tree();
                self.reload_pages();
                if was_open {
                    self.page = None;
                    self.open_page(&new_path);
                }
                self.set_status(format!("Renamed to \"{}\"", title.trim()));
            }
            Dialog::DeletePage { path, title, .. } => {
                let pos = self.pages.iter().position(|p| &p.path == path);
                page_free(path)?;
                store::delete_page(path)?;
                self.index.remove(path);
                self.index.remove_tree(&store::subpage_dir(path));
                self.index_stale = true;
                let was_open = self.page.as_ref().is_some_and(|p| p.path.starts_with(store::subpage_dir(path)) || &p.path == path);
                if was_open {
                    self.page = None;
                }
                self.reload_tree();
                self.reload_pages();
                if was_open {
                    let next = pos.map(|i| i.min(self.pages.len().saturating_sub(1)));
                    if let Some(p) = next.and_then(|i| self.pages.get(i)).map(|p| p.path.clone()) {
                        self.open_page(&p);
                    }
                }
                self.set_status(format!("\"{title}\" deleted; restore it from Folders > Deleted items"));
            }
            Dialog::NewSection { parent, name } => {
                need(name)?;
                let path = store::create_folder(parent, name)?;
                self.reload_tree();
                self.select_folder(path, true);
                self.set_status(format!("Created section \"{}\"", name.trim()));
            }
            Dialog::RenameSection { path, name } => {
                need(name)?;
                folder_free(path)?;
                let new_path = store::rename_folder(path, name)?;
                self.index.remove_tree(path);
                self.index.add_tree(&new_path);
                self.index_stale = true;
                if let Some(sel) = self.selected_folder.clone()
                    && let Ok(rest) = sel.strip_prefix(path)
                {
                    self.selected_folder = Some(new_path.join(rest));
                }
                if let Some(p) = self.page.take() {
                    let moved = p.path.strip_prefix(path).map(|rest| new_path.join(rest)).unwrap_or(p.path.clone());
                    self.open_page(&moved);
                }
                self.reload_tree();
                self.reload_pages();
                self.set_status(format!("Section renamed to \"{}\"", name.trim()));
            }
            Dialog::DeleteSection { path, name, .. } => {
                folder_free(path)?;
                store::delete_folder(path)?;
                self.index.remove_tree(path);
                self.index_stale = true;
                if self.page.as_ref().is_some_and(|p| p.path.starts_with(path)) {
                    self.page = None;
                }
                if self.selected_folder.as_ref().is_some_and(|s| s.starts_with(path)) {
                    self.selected_folder = None;
                    self.pages.clear();
                }
                self.reload_tree();
                self.set_status(format!("Section \"{name}\" deleted; restore it from Folders > Deleted items"));
            }
            Dialog::Message { .. } => {}
        }
        Ok(())
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let sc = |k: Key| KeyboardShortcut::new(Modifiers::COMMAND, k);
        let (find, save, edit, new, print, refresh, esc) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&sc(Key::F)),
                i.consume_shortcut(&sc(Key::S)),
                i.consume_shortcut(&sc(Key::E)),
                i.consume_shortcut(&sc(Key::N)),
                i.consume_shortcut(&sc(Key::P)),
                i.consume_key(Modifiers::NONE, Key::F5),
                i.key_pressed(Key::Escape),
            )
        });
        if self.dialog.is_some() {
            return;
        }
        if find {
            self.focus_search = true;
        }
        if save {
            self.actions.push(Action::Save);
        }
        if edit {
            self.actions.push(if self.mode == Mode::Edit { Action::StopEdit } else { Action::StartEdit });
        }
        if new && let Some(dir) = self.selected_folder.clone() {
            self.actions.push(Action::NewPage(dir));
        }
        if print && let Some(p) = &self.page {
            self.actions.push(Action::PrintPage(p.path.clone()));
        }
        if refresh {
            self.actions.push(Action::Refresh);
        }
        if esc && self.mode == Mode::Edit {
            self.actions.push(Action::StopEdit);
        }
    }

    /// Edit lock upkeep, and picking up changes saved on other computers.
    fn watch_shared_files(&mut self, ctx: &egui::Context) {
        let open = self.page.as_ref().map(|p| p.path.clone());
        if self.lock.as_ref().is_some_and(|l| self.mode != Mode::Edit || open.as_ref() != Some(&l.page)) {
            self.lock = None;
        }
        let beat = self.lock.as_mut().map(|l| l.heartbeat());
        if let Some(Err(e)) = beat {
            let dirty = self.page.as_ref().is_some_and(|p| p.body != p.saved_body);
            if dirty {
                self.save_conflict(&e);
            } else {
                self.lock = None;
                self.mode = Mode::View;
                self.md_cache = CommonMarkCache::default();
                if let Some(p) = &mut self.page {
                    p.display_src_len = usize::MAX;
                }
                self.reload_if_changed(true);
                self.show_message("Page in use", e);
            }
        }
        if self.page.is_some() && self.dialog.is_none() {
            self.reload_if_changed(false);
            ctx.request_repaint_after(Duration::from_secs(3));
        }
    }

    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        let Some(target) = self.screenshot.clone() else { return };
        self.frames += 1;
        ctx.request_repaint();
        // Wait until the page view has drawn the page (it starts a moment after the window).
        let web_settled = self.web.as_ref().and_then(|w| w.loaded_at).is_some_and(|t| t.elapsed() > Duration::from_millis(800));
        let waiting_for_web = self.web_error.is_none() && self.page.is_some() && !web_settled;
        if !self.shot_requested && self.frames >= 120 && (!waiting_for_web || self.frames > 1200) {
            self.shot_requested = true;
            self.shot_frame = self.frames;
            let out = self.shot_ui.clone();
            ctx.request_screenshot(move |img| {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                *out.lock().unwrap() = image::RgbaImage::from_raw(w as u32, h as u32, bytes);
            });
            if let Some(w) = &self.web
                && w.is_visible()
            {
                self.shot_web_bounds = w.bounds();
                w.capture(self.shot_web.clone());
            }
        }
        if self.shot_requested && !self.shot_saved {
            let web_ready = self.shot_web_bounds.is_none() || self.shot_web.lock().unwrap().is_some() || self.frames > self.shot_frame + 300;
            if web_ready && let Some(mut img) = self.shot_ui.lock().unwrap().take() {
                let web_png = self.shot_web.lock().unwrap().take();
                if let (Some(png), Some(b)) = (web_png, self.shot_web_bounds) {
                    overlay_png(&mut img, &png, b);
                }
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = img.save(&target);
                self.shot_saved = true;
            }
        }
        if self.shot_saved || self.frames > 2400 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Formatted page view

    /// Handles what the page view reported since the last frame.
    fn process_web(&mut self) {
        let msgs = match &self.web {
            Some(w) => w.take_messages(),
            None => return,
        };
        for m in msgs {
            match m {
                WebMsg::Ready => {
                    if let Some(w) = &mut self.web {
                        w.ready = true;
                        w.shown = None;
                    }
                }
                WebMsg::Loaded => {
                    if let Some(w) = &mut self.web {
                        w.loaded_at = Some(Instant::now());
                    }
                }
                WebMsg::Changed(md) => {
                    if self.mode == Mode::Edit
                        && let Some(p) = &mut self.page
                    {
                        if p.body != md {
                            p.body = md;
                            p.last_edit = Some(Instant::now());
                        }
                        let shown = (p.path.clone(), quick_hash(&p.body));
                        if let Some(w) = &mut self.web {
                            w.shown = Some(shown);
                        }
                    }
                }
                WebMsg::Toggle(n, on) => {
                    if self.mode == Mode::View
                        && let Some(p) = &mut self.page
                        && let Some(body) = set_task(&p.body, n, on)
                    {
                        p.body = body;
                        self.save_if_dirty();
                    }
                    // Show what the file says now (also when the change was refused).
                    if let Some(w) = &mut self.web {
                        w.shown = None;
                    }
                }
                WebMsg::Open(href) => self.open_link(&href),
                WebMsg::Key(k) => match k.as_str() {
                    "save" => self.actions.push(Action::Save),
                    "edit" => self.actions.push(if self.mode == Mode::Edit { Action::StopEdit } else { Action::StartEdit }),
                    "done" => self.actions.push(Action::StopEdit),
                    "find" => {
                        self.focus_search = true;
                        if let Some(w) = &self.web {
                            w.focus_parent();
                        }
                    }
                    "print" => {
                        if let Some(p) = &self.page {
                            self.actions.push(Action::PrintPage(p.path.clone()));
                        }
                    }
                    "new" => {
                        if let Some(d) = self.selected_folder.clone() {
                            self.actions.push(Action::NewPage(d));
                        }
                    }
                    "refresh" => self.actions.push(Action::Refresh),
                    _ => {}
                },
                WebMsg::Image { mime, data } => self.save_pasted_image(&mime, &data),
                WebMsg::Error(e) => eprintln!("page view: {e}"),
            }
        }
    }

    /// Creates the page view when first needed, keeps it over the page area and showing the open page.
    fn sync_web(&mut self, frame: &eframe::Frame, ctx: &egui::Context) {
        let rect = self.web_rect.take();
        if self.web.is_none() {
            if rect.is_none() || self.web_error.is_some() {
                return;
            }
            match PageWeb::new(frame, ctx) {
                Ok(w) => self.web = Some(w),
                Err(e) => {
                    self.set_status(format!("{e} The simple page view is used instead."));
                    self.web_error = Some(e);
                    return;
                }
            }
        }
        // egui dialogs and menus are drawn under the page view, so it steps aside for them.
        let covered = self.dialog.is_some() || self.history_win.is_some() || self.deleted_win.is_some() || self.history_view.is_some() || egui::Popup::is_any_open(ctx);
        let editing = self.mode == Mode::Edit;
        let Some(web) = self.web.as_mut() else { return };
        if ctx.input(|i| i.pointer.any_pressed()) {
            web.focus_parent();
        }
        web.place(if covered { None } else { rect }, ctx.pixels_per_point());
        let (Some(_), Some(page)) = (rect, self.page.as_ref()) else { return };
        if !web.ready {
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }
        let hash = quick_hash(&page.body);
        let stale = match &web.shown {
            None => true,
            Some((p, h)) => *p != page.path || (*h != hash && !(editing && web.editable)) || (!editing && web.editable),
        };
        if stale {
            let same_page = web.shown.as_ref().is_some_and(|(p, _)| *p == page.path);
            let dir = page.path.parent().unwrap_or(Path::new("."));
            web.load(&page.path, hash, &render::page_html(&page.body), &web::base_url(dir), editing, same_page);
            web.highlight.clear();
        } else if web.editable != editing {
            web.set_editable(editing);
        }
        if editing && self.focus_editor && !covered {
            web.focus();
            self.focus_editor = false;
        }
        if !editing {
            let words = self.search.trim();
            if words != web.highlight {
                web.set_highlight(words);
            }
        }
    }

    /// A link clicked on a page: web addresses open in the browser, other pages open here.
    fn open_link(&mut self, href: &str) {
        const SAFE: [&str; 24] = [
            "pdf", "png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "txt", "md", "markdown", "doc", "docx", "xls",
            "xlsx", "ppt", "pptx", "odt", "ods", "odp", "csv", "rtf", "mp3", "mp4",
        ];
        let href = href.trim();
        let lower = href.to_ascii_lowercase();
        if ["http://", "https://", "mailto:"].iter().any(|s| lower.starts_with(s)) {
            let _ = open::that_detached(href);
            return;
        }
        if lower.starts_with("onenote:") {
            self.set_status("This link points into OneNote, which NoteCraft cannot open.".into());
            return;
        }
        let Some(page) = &self.page else { return };
        let dir = page.path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let raw = href.split('#').next().unwrap_or("");
        let raw = if lower.starts_with("file:///") {
            &raw[8..]
        } else if lower.starts_with("file:") {
            &raw[5..]
        } else {
            raw
        };
        if let Some(i) = raw.find(':')
            && i > 1
            && raw[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        {
            self.set_status(format!("NoteCraft cannot open this kind of link: {href}"));
            return;
        }
        let decoded = web::percent_decode(raw);
        let p = PathBuf::from(&decoded);
        let path = if p.is_absolute() { p } else { dir.join(p) };
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if (ext == "md" || ext == "markdown") && path.is_file() {
            self.actions.push(Action::OpenHit(path));
        } else if path.is_dir() || (path.is_file() && SAFE.contains(&ext.as_str())) {
            let _ = open::that_detached(&path);
        } else if path.is_file() {
            self.set_status(format!("For safety NoteCraft does not open .{ext} files from a page; use Show in File Explorer."));
        } else {
            self.set_status(format!("Could not find \"{decoded}\""));
        }
    }

    /// A picture pasted or dropped into the page is saved in the "resources" folder next to it.
    fn save_pasted_image(&mut self, mime: &str, data: &[u8]) {
        if self.mode != Mode::Edit {
            return;
        }
        let Some(page) = &self.page else { return };
        let dir = page.path.parent().unwrap_or(Path::new(".")).join("resources");
        let ext = match mime {
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "image/bmp" => "bmp",
            "image/svg+xml" => "svg",
            _ => "png",
        };
        let stem = format!("{} {}", store::file_stem(&page.path), chrono::Local::now().format("%Y-%m-%d %H%M%S"));
        let mut target = dir.join(format!("{stem}.{ext}"));
        let mut n = 2;
        while target.exists() {
            target = dir.join(format!("{stem} ({n}).{ext}"));
            n += 1;
        }
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&target, data)) {
            Ok(()) => {
                let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if let Some(w) = &self.web {
                    w.insert_image(&format!("resources/{name}"));
                }
            }
            Err(e) => self.set_status(format!("Could not save the picture: {e}")),
        }
    }
}

impl eframe::App for NoteApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.screenshot.is_none() {
            self.frames += 1;
        }

        if let Some(rx) = &self.index_rx
            && let Ok(idx) = rx.try_recv()
        {
            self.index = idx;
            self.index_rx = None;
            self.index_stale = true;
        }
        if self.index_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        self.process_web();
        self.shortcuts(&ctx);
        self.watch_shared_files(&ctx);

        // Autosave a moment after typing stops.
        if let Some(p) = &self.page
            && let Some(t) = p.last_edit
        {
            if t.elapsed() > Duration::from_millis(1500) {
                self.save_if_dirty();
            } else {
                ctx.request_repaint_after(Duration::from_millis(1600));
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_if_dirty();
            self.lock = None;
            self.remember();
        }

        self.top_bar(ui);
        self.status_bar(ui);
        if !self.roots.is_empty() {
            self.nav_panel(ui);
            self.list_panel(ui);
        }
        self.page_panel(ui);
        self.dialogs(&ctx);
        self.history_window(&ctx);
        self.deleted_window(&ctx);
        self.history_view_window(&ctx);
        self.history_search_tick(&ctx);
        self.apply_actions();
        self.sync_web(frame, &ctx);
        self.screenshot_tick(&ctx);
    }
}

// ---------------------------------------------------------------------------------------------
// Small helpers

static RE_TASK: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?m)^[ \t>]*(?:[-*+]|\d+[.)])[ \t]+\[([ xX])\]").unwrap());

/// Ticked state of every task-list checkbox, in order.
fn task_states(md: &str) -> Vec<bool> {
    RE_TASK.captures_iter(md).map(|c| &c[1] != " ").collect()
}

/// Sets the n-th checkbox of the text.
fn set_task(md: &str, n: usize, checked: bool) -> Option<String> {
    let c = RE_TASK.captures_iter(md).nth(n)?;
    let m = c.get(1)?;
    let mut out = md.to_string();
    out.replace_range(m.range(), if checked { "x" } else { " " });
    Some(out)
}

fn other_editor_banner(ui: &mut egui::Ui, who: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0xFF, 0xF4, 0xCE))
        .stroke(Stroke::new(1.0, Color32::from_rgb(0xE6, 0xC8, 0x5C)))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(format!("This page is being edited {who}. You see the last saved version; it updates by itself."))
                    .color(TEXT)
                    .size(13.0),
            );
        });
    ui.add_space(6.0);
}

/// Puts the page view's picture (PNG) over the window picture at `bounds` (x, y, width, height).
fn overlay_png(img: &mut image::RgbaImage, png: &[u8], bounds: [i32; 4]) {
    let Ok(web) = image::load_from_memory(png) else { return };
    let mut web = web.into_rgba8();
    let [x, y, w, h] = bounds;
    if web.width() != w as u32 || web.height() != h as u32 {
        web = image::imageops::resize(&web, w.max(1) as u32, h.max(1) as u32, image::imageops::FilterType::Triangle);
    }
    image::imageops::overlay(img, &web, x as i64, y as i64);
}

/// Refuses when another computer is editing the page or one of its subpages.
fn page_free(path: &Path) -> Result<(), String> {
    if let Some(h) = lock::held_by_other(path) {
        return Err(format!("This page is being edited {}. Try again when they are done.", h.describe()));
    }
    let sub = store::subpage_dir(path);
    if sub.is_dir() {
        folder_free(&sub)?;
    }
    Ok(())
}

/// Refuses when another computer is editing any page inside the folder.
fn folder_free(dir: &Path) -> Result<(), String> {
    match lock::held_below(dir) {
        Some((page, h)) => Err(format!(
            "The page \"{}\" in here is being edited {}. Try again when they are done.",
            store::file_stem(&page),
            h.describe()
        )),
        None => Ok(()),
    }
}

fn first_folder_with_pages(f: &Folder) -> Option<PathBuf> {
    for c in &f.children {
        if c.page_count > 0 {
            return Some(c.path.clone());
        }
        if let Some(p) = first_folder_with_pages(c) {
            return Some(p);
        }
    }
    None
}

fn folder_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string())
}

fn section_color(name: &str) -> Color32 {
    let h = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    SECTION_COLORS[(h % SECTION_COLORS.len() as u32) as usize]
}

fn quick_hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Base for relative image paths on a page, e.g. `file:///E:/notes/Section/`.
fn file_base_uri(dir: &Path) -> String {
    let s = dir.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s);
    format!("file:///{}/", s.trim_end_matches('/'))
}

/// Buttons that set their own fill and outline lose egui's hover effect, so a clickable button gets
/// this one: a slight darkening and a lilac outline while the pointer is over it.
trait HoverFx {
    fn hover_fx(self) -> Self;
}

impl HoverFx for egui::Response {
    fn hover_fx(self) -> Self {
        if self.hovered() && self.enabled() {
            let painter = self.ctx.layer_painter(self.layer_id);
            painter.rect_filled(self.rect, CornerRadius::same(4), Color32::from_rgba_unmultiplied(0, 0, 0, 22));
            painter.rect_stroke(self.rect, CornerRadius::same(4), Stroke::new(1.5, Color32::from_rgb(0xB0, 0x89, 0xDC)), egui::StrokeKind::Inside);
        }
        self
    }
}

fn top_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(Color32::WHITE).size(14.0))
        .fill(ACCENT_DARK)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(4))
        .min_size(Vec2::new(30.0, 28.0))
}

fn accent_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(Color32::WHITE).size(14.0))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(4))
        .min_size(Vec2::new(64.0, 28.0))
}

fn tool_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(TEXT).size(13.5))
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(4))
        .min_size(Vec2::new(0.0, 28.0))
}

fn small_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(ACCENT_DARK).size(12.5))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(4))
}

fn paint_text_clipped(painter: &egui::Painter, pos: egui::Pos2, text: &str, font: FontId, color: Color32, max_w: f32) {
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), color);
    if galley.size().x <= max_w {
        painter.galley(pos, galley, color);
        return;
    }
    // Shorten with an ellipsis.
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let s: String = chars.iter().collect::<String>() + "…";
        let g = painter.layout_no_wrap(s, font.clone(), color);
        if g.size().x <= max_w {
            painter.galley(pos, g, color);
            return;
        }
    }
}

fn list_row(ui: &mut egui::Ui, title: &str, sub: &str, selected: bool, indent: f32, height: f32) -> egui::Response {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, CornerRadius::same(4), ACCENT_SOFT);
        painter.rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())), CornerRadius::same(1), ACCENT);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(4), HOVER_SOFT);
    }
    let x = rect.left() + 10.0 + indent;
    let max_w = rect.right() - x - 6.0;
    paint_text_clipped(&painter, egui::pos2(x, rect.top() + 4.0), title, FontId::proportional(14.0), if selected { ACCENT_DARK } else { TEXT }, max_w);
    if !sub.is_empty() {
        paint_text_clipped(&painter, egui::pos2(x, rect.top() + 22.0), sub, FontId::proportional(11.5), MUTED, max_w);
    }
    resp.on_hover_text(title)
}

fn search_row(ui: &mut egui::Ui, title: &str, place: &str, snippet: &str, count: usize, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let inner = egui::Frame::new()
        .inner_margin(Margin::symmetric(10, 6))
        .corner_radius(CornerRadius::same(4))
        .fill(if selected { ACCENT_SOFT } else { Color32::TRANSPARENT })
        .show(ui, |ui| {
            ui.set_width(width - 20.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(RichText::new(title).size(14.0).color(if selected { ACCENT_DARK } else { TEXT }).strong()).truncate());
            let place = if count > 1 { format!("{place}  ·  {count} matches") } else { place.to_string() };
            ui.add(egui::Label::new(RichText::new(place).size(11.5).color(ACCENT)).truncate());
            if !snippet.is_empty() {
                ui.add(egui::Label::new(RichText::new(snippet).size(12.0).color(MUTED)).wrap());
            }
        });
    let resp = ui.interact(inner.response.rect, ui.scope_id().with("hit"), Sense::click());
    if resp.hovered() && !selected {
        ui.painter().rect_stroke(inner.response.rect, CornerRadius::same(4), Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
    }
    resp
}

fn setup_style(ctx: &egui::Context) {
    // Use the Windows UI font when it is there; egui's own fonts stay as fallback (symbols, emoji).
    let mut fonts = egui::FontDefinitions::default();
    for (name, file) in [("segoe", r"C:\Windows\Fonts\segoeui.ttf"), ("segoe-symbol", r"C:\Windows\Fonts\seguisym.ttf")] {
        if let Ok(bytes) = std::fs::read(file) {
            fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
            let fam = fonts.families.entry(FontFamily::Proportional).or_default();
            if name == "segoe" {
                fam.insert(0, name.into());
            } else {
                fam.push(name.into());
            }
        }
    }
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\consola.ttf") {
        fonts.font_data.insert("consolas".into(), Arc::new(egui::FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "consolas".into());
    }
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::ThemePreference::Light);
    let mut v = egui::Visuals::light();
    v.panel_fill = PAGE_BG;
    v.window_fill = Color32::WHITE;
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = Color32::from_rgb(0xF6, 0xF4, 0xF9);
    v.hyperlink_color = Color32::from_rgb(0x2B, 0x5C, 0xB8);
    v.selection.bg_fill = Color32::from_rgb(0xD9, 0xC6, 0xEE);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.override_text_color = Some(TEXT);
    v.window_corner_radius = CornerRadius::same(8);
    v.widgets.hovered.weak_bg_fill = HOVER_SOFT;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xC9, 0xB3, 0xE3));
    v.widgets.active.weak_bg_fill = ACCENT_SOFT;
    v.widgets.inactive.corner_radius = CornerRadius::same(4);
    v.widgets.hovered.corner_radius = CornerRadius::same(4);
    // egui keeps a button's size constant by trimming its padding by the width of the state's
    // outline. A `.stroke(..)` set on the button replaces the outline but not that trimming, so with
    // an invisible inactive outline (width 0) and a 1 px hover outline a button lost 2 px of width on
    // hover and its neighbours moved. Equal widths in every state make the trimming constant.
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::TRANSPARENT);
    v.widgets.active.corner_radius = CornerRadius::same(4);
    ctx.set_visuals_of(egui::Theme::Light, v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 5.0);
        s.spacing.button_padding = Vec2::new(10.0, 4.0);
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(13.5));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::proportional(22.0));
    });
}

// ---------------------------------------------------------------------------------------------
// Settings (remembered between runs)

struct Settings {
    /// The open top-level folders (one `root=` line each; an old single line loads as a list of one).
    roots: Vec<PathBuf>,
    page: Option<PathBuf>,
    sort: SortOrder,
}

impl Settings {
    fn file() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("NoteCraft").join("settings.txt"))
    }

    fn load() -> Settings {
        let mut s = Settings { roots: Vec::new(), page: None, sort: SortOrder::Name };
        let Some(text) = Self::file().and_then(|f| std::fs::read_to_string(f).ok()) else { return s };
        for line in text.lines() {
            match line.split_once('=') {
                Some(("root", v)) if !v.is_empty() => s.roots.push(PathBuf::from(v)),
                Some(("page", v)) if !v.is_empty() => s.page = Some(PathBuf::from(v)),
                Some(("sort", "created")) => s.sort = SortOrder::Created,
                Some(("sort", "updated")) => s.sort = SortOrder::Updated,
                _ => {}
            }
        }
        s
    }

    fn save(&self) {
        let Some(f) = Self::file() else { return };
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let sort = match self.sort {
            SortOrder::Name => "name",
            SortOrder::Created => "created",
            SortOrder::Updated => "updated",
        };
        let roots: String = self.roots.iter().map(|p| format!("root={}\n", p.display())).collect();
        let text = format!(
            "{roots}page={}\nsort={}\n",
            self.page.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            sort
        );
        let _ = std::fs::write(f, text);
    }
}
