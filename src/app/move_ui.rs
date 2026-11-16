//! Moving a page: the "Move to…" dialog and the move itself (also used by drag and drop).

use super::*;
use crate::mover;
use std::collections::HashMap;

/// The "Move to…" dialog: a tree of all open notebooks with their sections and pages.
pub struct MoveDialog {
    page: PathBuf,
    title: String,
    /// The chosen target folder: a section, or the subpage folder of a page.
    selected: Option<PathBuf>,
    /// What `selected` is, for the line under the tree.
    selected_label: String,
    /// The pages of each folder whose branch was opened (read on first use).
    pages: HashMap<PathBuf, Vec<store::PageEntry>>,
    /// The section the page is in now; its branch starts open.
    home: PathBuf,
    message: String,
}

/// What is carried while a page is dragged: its path and title.
pub struct DragPage(pub PathBuf, pub String);

impl NoteApp {
    /// Lets a page row be dragged.
    pub(super) fn page_drag_source(resp: &egui::Response, page: &Path, title: &str) -> egui::Response {
        let resp = resp.interact(Sense::click_and_drag());
        resp.dnd_set_drag_payload(DragPage(page.to_path_buf(), title.to_string()));
        resp
    }

    /// Highlights a row a dragged page may be dropped on, and turns the drop into a move.
    pub(super) fn page_drop_target(&mut self, ui: &egui::Ui, resp: &egui::Response, target_dir: &Path) {
        let Some(p) = resp.dnd_hover_payload::<DragPage>() else { return };
        if !mover::can_move_to(&p.0, target_dir) {
            return;
        }
        ui.painter().rect_stroke(resp.rect, CornerRadius::same(4), Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
        if let Some(p) = resp.dnd_release_payload::<DragPage>() {
            self.actions.push(Action::MovePage { page: p.0.clone(), target_dir: target_dir.to_path_buf() });
        }
    }

    /// The title of the dragged page next to the pointer.
    pub(super) fn drag_label(&self, ctx: &egui::Context) {
        let Some(p) = egui::DragAndDrop::payload::<DragPage>(ctx) else { return };
        let Some(pos) = ctx.pointer_latest_pos() else { return };
        egui::Area::new(Id::unique("drag-page"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + Vec2::new(14.0, 10.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(RichText::new(&p.1).color(TEXT));
                });
            });
    }

    pub(super) fn open_move(&mut self, page: PathBuf) {
        let title = store::read_doc(&page).map(|d| d.title_or(&page)).unwrap_or_else(|_| store::file_stem(&page));
        let home = self.folder_of_page(&page);
        self.move_win = Some(MoveDialog {
            page,
            title,
            selected: None,
            selected_label: String::new(),
            pages: HashMap::new(),
            home,
            message: String::new(),
        });
    }

    /// Moves `page` into `target_dir`; the open page is saved first and reopened at its new place.
    pub(super) fn do_move(&mut self, page: &Path, target_dir: &Path) -> Result<(), String> {
        if !mover::can_move_to(page, target_dir) {
            return Ok(());
        }
        page_free(page)?;
        let sub = store::subpage_dir(page);
        // The open page, as a path relative to the moved page's folder, when it moves along.
        let open_rel = self
            .page
            .as_ref()
            .filter(|p| p.path == page || p.path.starts_with(&sub))
            .and_then(|p| p.path.strip_prefix(page.parent()?).ok().map(|r| r.to_path_buf()));
        if open_rel.is_some() {
            self.save_if_dirty();
            self.leave_edit();
            self.page = None;
        }
        let title = store::read_doc(page).map(|d| d.title_or(page)).unwrap_or_else(|_| store::file_stem(page));
        let result = mover::move_page(page, target_dir);
        let new = match result {
            Ok(Some(new)) => new,
            Ok(None) => {
                if let Some(rel) = &open_rel {
                    self.open_page(&page.parent().unwrap_or(Path::new(".")).join(rel));
                }
                return Ok(());
            }
            Err(e) => {
                if let Some(rel) = &open_rel {
                    self.open_page(&page.parent().unwrap_or(Path::new(".")).join(rel));
                }
                return Err(e);
            }
        };
        self.index.remove(page);
        self.index.remove_tree(&sub);
        self.index.add(&new);
        self.index.add_tree(&store::subpage_dir(&new));
        self.index_stale = true;
        self.reload_tree();
        self.reload_pages();
        if let Some(rel) = open_rel {
            // `rel` starts with the old file name (page) or the old subpage folder name.
            let mut parts = rel.components();
            let first = parts.next().map(|c| c.as_os_str().to_string_lossy().to_string()).unwrap_or_default();
            let rest: PathBuf = parts.collect();
            let reopened = if first.eq_ignore_ascii_case(&format!("{}.md", store::file_stem(page))) {
                new.clone()
            } else {
                store::subpage_dir(&new).join(rest)
            };
            self.open_page(&reopened);
        }
        self.set_status(format!("Moved \"{title}\" to {}", self.index.place_of(&new)));
        Ok(())
    }

    fn move_folder_rows(&self, ui: &mut egui::Ui, win: &mut MoveDialog, f: &Folder, depth: usize) {
        let id = Id::unique(("move-folder", &f.path));
        let open_default = depth == 0 || win.home.starts_with(&f.path);
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open_default);
        let is_open = state.is_open();
        state
            .show_header(ui, |ui| {
                let ok = mover::can_move_to(&win.page, &f.path);
                let selected = win.selected.as_ref() == Some(&f.path);
                let text = RichText::new(&f.name).strong();
                let r = ui.add_enabled(ok, egui::Button::selectable(selected, text));
                if r.clicked() {
                    win.selected = Some(f.path.clone());
                    win.selected_label = format!("the top of \"{}\"", f.name);
                }
            })
            .body(|ui| {
                for c in &f.children {
                    self.move_folder_rows(ui, win, c, depth + 1);
                }
                if is_open {
                    let pages = win.pages.entry(f.path.clone()).or_insert_with(|| store::list_pages(&f.path, SortOrder::Name)).clone();
                    for pg in &pages {
                        let target = store::subpage_dir(&pg.path);
                        let ok = mover::can_move_to(&win.page, &target) && pg.path != win.page;
                        let selected = win.selected.as_ref() == Some(&target);
                        ui.horizontal(|ui| {
                            ui.add_space(14.0 * pg.level as f32);
                            let r = ui.add_enabled(ok, egui::Button::selectable(selected, RichText::new(&pg.title).color(TEXT)));
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
        let modal = egui::Modal::new(Id::unique("move-page")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.label(RichText::new("Move page").size(18.0).color(TEXT));
            ui.add_space(6.0);
            ui.label(format!("Where should \"{}\" go? Pick a section, or a page to put it under.", win.title));
            ui.add_space(6.0);
            let height = (ctx.content_rect().height() - 260.0).clamp(160.0, 420.0);
            egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(ui, |ui| {
                for t in &trees {
                    self.move_folder_rows(ui, &mut win, t, 0);
                }
            });
            ui.add_space(6.0);
            if win.selected.is_some() {
                ui.label(RichText::new(format!("Move to {}", win.selected_label)).color(MUTED));
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
            match self.do_move(&win.page, &target) {
                Ok(()) => return,
                Err(e) => win.message = e,
            }
        }
        self.move_win = Some(win);
    }
}
