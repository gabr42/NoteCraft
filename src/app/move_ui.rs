//! Moving a page or a section: the "Move to…" dialog and the moves themselves (also used by drag and drop).

use super::*;
use std::collections::HashMap;

/// What the "Move to…" dialog moves.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MoveKind {
    /// A page, into a section or under another page.
    Page,
    /// A section folder, into another folder.
    Section,
}

/// The "Move to…" dialog: a tree of all open notebooks with their sections (and pages, for a page).
pub struct MoveDialog {
    kind: MoveKind,
    /// The page file or the section folder being moved.
    item: PathBuf,
    title: String,
    /// The chosen target folder: a section, or the subpage folder of a page.
    selected: Option<PathBuf>,
    /// What `selected` is, for the line under the tree.
    selected_label: String,
    /// The pages of each folder whose branch was opened (read on first use).
    pages: HashMap<PathBuf, Vec<store::PageEntry>>,
    /// The folder the item is in now; the way to it starts open.
    home: PathBuf,
    message: String,
    /// Differs for every opening, so branches start collapsed again (except the way to `home`).
    salt: u64,
}

/// Counts dialog openings for `MoveDialog::salt`.
static OPENINGS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Height of one notebook row in the drop strip shown during a drag (with the gap below it).
const NOTEBOOK_DROP_ROW: f32 = 30.0;

/// What is carried while a page is dragged: its path and title.
pub struct DragPage(pub PathBuf, pub String);

/// What is carried while a section is dragged: its folder and name.
pub struct DragSection(pub PathBuf, pub String);

impl NoteApp {
    /// Lets a page row be dragged.
    pub(super) fn page_drag_source(resp: &egui::Response, page: &Path, title: &str) -> egui::Response {
        let resp = resp.interact(Sense::click_and_drag());
        resp.dnd_set_drag_payload(DragPage(page.to_path_buf(), title.to_string()));
        resp
    }

    /// Lets a section row in the tree be dragged.
    pub(super) fn section_drag_source(resp: &egui::Response, folder: &Path, name: &str) -> egui::Response {
        let resp = resp.interact(Sense::click_and_drag());
        resp.dnd_set_drag_payload(DragSection(folder.to_path_buf(), name.to_string()));
        resp
    }

    /// Highlights a row a dragged page may be dropped on, and turns the drop into a move.
    pub(super) fn page_drop_target(&mut self, ui: &egui::Ui, resp: &egui::Response, target_dir: &Path) {
        let Some(p) = resp.dnd_hover_payload::<DragPage>() else { return };
        if !mover::can_move_to(&p.0, target_dir) {
            return;
        }
        self.drop_allowed = true;
        ui.painter().rect_stroke(resp.rect, CornerRadius::same(4), Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
        if let Some(p) = resp.dnd_release_payload::<DragPage>() {
            self.actions.push(Action::MovePage { page: p.0.clone(), target_dir: target_dir.to_path_buf() });
        }
    }

    /// Highlights a folder row a dragged section may be dropped on, and turns the drop into a move.
    pub(super) fn section_drop_target(&mut self, ui: &egui::Ui, resp: &egui::Response, target: &Path) {
        let Some(s) = resp.dnd_hover_payload::<DragSection>() else { return };
        if !mover::can_move_section_to(&s.0, target) {
            return;
        }
        self.drop_allowed = true;
        ui.painter().rect_stroke(resp.rect, CornerRadius::same(4), Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
        if let Some(s) = resp.dnd_release_payload::<DragSection>() {
            self.actions.push(Action::MoveSection { folder: s.0.clone(), target: target.to_path_buf() });
        }
    }

    /// While a page or section is dragged: the other open notebooks (only the shown one has a tree to
    /// drop on). Empty when nothing is dragged.
    pub(super) fn drop_notebooks(&self, ctx: &egui::Context) -> Vec<usize> {
        let dragging = egui::DragAndDrop::has_payload_of_type::<DragPage>(ctx)
            || egui::DragAndDrop::has_payload_of_type::<DragSection>(ctx);
        if !dragging {
            return Vec::new();
        }
        (0..self.roots.len()).filter(|&i| i != self.active).collect()
    }

    /// Height of `notebook_drop_strip` for `count` notebooks.
    pub(super) fn drop_strip_height(count: usize) -> f32 {
        if count == 0 { 0.0 } else { 26.0 + NOTEBOOK_DROP_ROW * count as f32 }
    }

    /// Below the tree during a drag: one drop row for each of the other notebooks, `drop_notebooks`.
    pub(super) fn notebook_drop_strip(&mut self, ui: &mut egui::Ui, notebooks: &[usize]) {
        if notebooks.is_empty() {
            return;
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Drop into another notebook").color(MUTED).size(12.0));
        for &i in notebooks {
            let Some(root) = self.roots.get(i).cloned() else { continue };
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), NOTEBOOK_DROP_ROW - 4.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(4), HOVER_SOFT);
            let chip = egui::Rect::from_min_size(rect.left_top() + Vec2::new(6.0, 4.0), Vec2::new(12.0, rect.height() - 8.0));
            ui.painter().rect_filled(chip, CornerRadius::same(2), ACCENT);
            ui.painter().text(
                rect.left_center() + Vec2::new(26.0, 0.0),
                egui::Align2::LEFT_CENTER,
                folder_name(&root),
                FontId::proportional(14.0),
                TEXT,
            );
            self.page_drop_target(ui, &resp, &root);
            self.section_drop_target(ui, &resp, &root);
            ui.add_space(4.0);
        }
    }

    /// While a page or section is dragged: the cursor, and its title next to the pointer.
    /// Runs after all drop targets of the frame (they set `drop_allowed`).
    pub(super) fn drag_feedback(&mut self, ctx: &egui::Context) {
        let allowed = std::mem::take(&mut self.drop_allowed);
        let title = match egui::DragAndDrop::payload::<DragPage>(ctx) {
            Some(p) => p.1.clone(),
            None => match egui::DragAndDrop::payload::<DragSection>(ctx) {
                Some(s) => s.1.clone(),
                None => return,
            },
        };
        // "No entry" where it cannot be dropped; elsewhere egui's own drag cursor stays.
        if !allowed {
            ctx.set_cursor_icon(egui::CursorIcon::NoDrop);
        }
        let Some(pos) = ctx.pointer_latest_pos() else { return };
        egui::Area::new(Id::unique("drag-page"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + Vec2::new(14.0, 10.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(&title).color(TEXT));
                });
            });
    }

    fn new_move_dialog(kind: MoveKind, item: PathBuf, title: String, home: PathBuf) -> MoveDialog {
        MoveDialog {
            kind,
            item,
            title,
            selected: None,
            selected_label: String::new(),
            pages: HashMap::new(),
            home,
            message: String::new(),
            salt: OPENINGS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }

    pub(super) fn open_move(&mut self, page: PathBuf) {
        let title = store::read_doc(&page).map(|d| d.title_or(&page)).unwrap_or_else(|_| store::file_stem(&page));
        let home = self.folder_of_page(&page);
        self.move_win = Some(Self::new_move_dialog(MoveKind::Page, page, title, home));
    }

    pub(super) fn open_move_section(&mut self, folder: PathBuf) {
        let name = folder_name(&folder);
        let home = folder.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        self.move_win = Some(Self::new_move_dialog(MoveKind::Section, folder, name, home));
    }

    /// Moves `page` into `target_dir`; the open page is saved first and reopened at its new place.
    pub(super) fn do_move(&mut self, page: &Path, target_dir: &Path) -> Result<(), String> {
        if !mover::can_move_to(page, target_dir) {
            return Ok(());
        }
        page_free(page)?;
        let sub = store::subpage_dir(page);
        // The open page, when it moves along (the page itself or one of its subpages).
        let open = self.release_open_page(|p| store::same_path(p, page) || store::is_within(p, &sub))?;
        let title = store::read_doc(page).map(|d| d.title_or(page)).unwrap_or_else(|_| store::file_stem(page));
        let mover::Moved { path: new, left_behind } = match mover::move_page(page, target_dir) {
            Ok(Some(moved)) => moved,
            other => {
                if other.is_err() {
                    // A failed move is undone, but a half-removed copy may be left: show the disk.
                    self.reload_tree();
                    self.reload_pages();
                }
                if let Some(r) = &open {
                    self.reopen(r, &r.path.clone());
                }
                return other.map(|_| ());
            }
        };
        // The page itself is read again (a name clash renames it, which can change its title);
        // its subpages only change place.
        self.index.remove(page);
        self.index.add(&new);
        self.index.relocate(&sub, &store::subpage_dir(&new));
        // What could not be removed is still there and still found.
        if page.is_file() {
            self.index.add(page);
        }
        if sub.is_dir() {
            self.index.add_tree(&sub);
        }
        self.index_stale = true;
        self.reload_tree();
        self.reload_pages();
        if let Some(r) = &open {
            let reopened = store::rebase(&r.path, &sub, &store::subpage_dir(&new)).unwrap_or_else(|| new.clone());
            self.reopen(r, &reopened);
        }
        self.set_status(format!("Moved \"{title}\" to {}", self.index.place_of(&new)));
        if !left_behind.is_empty() {
            let text = left_behind_text(&format!("The page \"{title}\""), &left_behind);
            self.show_message("Move page", text);
        }
        Ok(())
    }

    /// Moves the section `folder` into `target`; the open page inside it is saved first and reopened.
    pub(super) fn do_move_section(&mut self, folder: &Path, target: &Path) -> Result<(), String> {
        if !mover::can_move_section_to(folder, target) {
            return Ok(());
        }
        let mut left_behind = Vec::new();
        let op = || mover::move_section(folder, target).map(|m| m.map(|m| {
            left_behind = m.left_behind;
            m.path
        }));
        let Some(new) = self.relocate_folder(folder, op)? else {
            return Ok(());
        };
        // `new` carries the final name ("Name (2)" after a name clash).
        self.set_status(format!("Moved section \"{}\" to {}", folder_name(&new), self.index.place_of(&new)));
        if !left_behind.is_empty() {
            // What could not be removed is still there and still found.
            self.index.add_tree(folder);
            let text = left_behind_text(&format!("The section \"{}\"", folder_name(&new)), &left_behind);
            self.show_message("Move section", text);
        }
        Ok(())
    }

    fn move_folder_rows(&self, ui: &mut egui::Ui, win: &mut MoveDialog, f: &Folder, depth: usize) {
        let id = Id::unique(("move-folder", win.salt, &f.path));
        let open_default = store::is_within(&win.home, &f.path);
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open_default);
        let is_open = state.is_open();
        state
            .show_header(ui, |ui| {
                let ok = match win.kind {
                    MoveKind::Page => mover::can_move_to(&win.item, &f.path),
                    MoveKind::Section => mover::can_move_section_to(&win.item, &f.path),
                };
                let selected = win.selected.as_ref() == Some(&f.path);
                // The same colour chip as in the main tree: wide for a notebook, narrow for a section.
                let is_notebook = depth == 0 && !f.children.is_empty();
                let color = if is_notebook { ACCENT } else { section_color(&f.name) };
                let (chip, _) = ui.allocate_exact_size(Vec2::new(if is_notebook { 12.0 } else { 5.0 }, 14.0), Sense::hover());
                ui.painter().rect_filled(chip, CornerRadius::same(2), color);
                let text = RichText::new(&f.name).strong();
                let r = ui.add_enabled(ok, egui::Button::selectable(selected, text));
                if r.clicked() {
                    win.selected = Some(f.path.clone());
                    win.selected_label = match win.kind {
                        MoveKind::Page => format!("to the top of \"{}\"", f.name),
                        MoveKind::Section => format!("into \"{}\"", f.name),
                    };
                }
            })
            .body(|ui| {
                for c in &f.children {
                    self.move_folder_rows(ui, win, c, depth + 1);
                }
                if is_open && win.kind == MoveKind::Page {
                    let pages = win.pages.entry(f.path.clone()).or_insert_with(|| store::list_pages(&f.path, SortOrder::Name)).clone();
                    for pg in &pages {
                        let target = store::subpage_dir(&pg.path);
                        let ok = mover::can_move_to(&win.item, &target) && pg.path != win.item;
                        let selected = win.selected.as_ref() == Some(&target);
                        ui.horizontal(|ui| {
                            ui.add_space(14.0 * pg.level as f32);
                            let text = RichText::new(format!("📄 {}", pg.title)).color(TEXT);
                            let r = ui.add_enabled(ok, egui::Button::selectable(selected, text));
                            if r.clicked() {
                                win.selected = Some(target.clone());
                                win.selected_label = format!("under \"{}\"", pg.title);
                            }
                        });
                    }
                }
            });
    }

    pub(super) fn move_window(&mut self, ctx: &egui::Context) {
        let Some(mut win) = self.move_win.take() else { return };
        let mut close = false;
        let mut confirm = false;
        let trees = self.trees.clone();
        let (heading, prompt) = match win.kind {
            MoveKind::Page => ("Move page", format!("Where should \"{}\" go? Pick a section, or a page to put it under.", win.title)),
            MoveKind::Section => ("Move section", format!("Where should \"{}\" go? Pick a folder.", win.title)),
        };
        let modal = egui::Modal::new(Id::unique("move-page")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new(heading).size(18.0).color(TEXT));
            ui.add_space(6.0);
            ui.label(prompt);
            ui.add_space(6.0);
            let height = (ctx.content_rect().height() - 260.0).clamp(160.0, 420.0);
            egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(ui, |ui| {
                for t in &trees {
                    self.move_folder_rows(ui, &mut win, t, 0);
                }
            });
            ui.add_space(6.0);
            if win.selected.is_some() {
                ui.label(RichText::new(format!("Move {}", win.selected_label)).color(MUTED));
            }
            if !win.message.is_empty() {
                ui.label(RichText::new(&win.message).color(Color32::from_rgb(0xC0, 0x20, 0x20)));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    if ui.add_enabled(win.selected.is_some(), accent_button("Move")).hover_fx().clicked() {
                        confirm = true;
                    }
                });
            });
        });
        if modal.should_close() || close {
            return;
        }
        if confirm && let Some(target) = win.selected.clone() {
            let result = match win.kind {
                MoveKind::Page => self.do_move(&win.item, &target),
                MoveKind::Section => self.do_move_section(&win.item, &target),
            };
            match result {
                Ok(()) => return,
                Err(e) => win.message = e,
            }
        }
        self.move_win = Some(win);
    }
}

/// The message after a move whose copy is complete but whose originals were not all removed.
fn left_behind_text(what: &str, left: &[PathBuf]) -> String {
    const SHOWN: usize = 10;
    let mut text = format!(
        "{what} was moved, but these files could not be removed from the old place. They are \
         probably open in another program; delete them once they are free:\n"
    );
    for p in left.iter().take(SHOWN) {
        text.push_str(&format!("\n{}", p.display()));
    }
    if left.len() > SHOWN {
        text.push_str(&format!("\n… and {} more", left.len() - SHOWN));
    }
    text
}
