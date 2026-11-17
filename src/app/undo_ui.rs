//! Rename and delete of pages and sections (shared by the dialogs and by undo/redo) and the runner
//! of undo and redo steps.

use super::*;
use crate::history;
use crate::op_error::{Kind, OpError};
use crate::undo::{Record, Step};

/// The reason when the item a step works on has vanished.
const GONE: &str = "it is not there any more";

/// Why a step did not run.
#[derive(Debug, PartialEq)]
enum StepError {
    /// Something that can pass (another computer edits the item, the open page could not be saved,
    /// a file is in use): nothing changed and the record stays on its list, up to `undo::BUSY_TRIES`
    /// failures in a row.
    Busy(String),
    /// The step cannot be done (the item is gone, a name it needs is taken, the operation failed for
    /// another reason or only halfway): the record is dropped, so that older steps stay reachable.
    Refused(String),
}

/// Only an operation that failed because a file is in use may work on another try.
impl From<OpError> for StepError {
    fn from(e: OpError) -> Self {
        match e.kind {
            Kind::Busy => StepError::Busy(e.text),
            Kind::Refused | Kind::Failed | Kind::Partial => StepError::Refused(e.text),
        }
    }
}

impl StepError {
    fn text(&self) -> &str {
        match self {
            StepError::Busy(t) | StepError::Refused(t) => t,
        }
    }
}

impl From<String> for StepError {
    fn from(e: String) -> Self {
        StepError::Refused(e)
    }
}

impl From<&str> for StepError {
    fn from(e: &str) -> Self {
        StepError::Refused(e.to_string())
    }
}

/// The folder an item lies in, or the "not there" reason for a path without one.
fn parent_of(p: &Path) -> Result<PathBuf, String> {
    p.parent().map(Path::to_path_buf).ok_or_else(|| GONE.to_string())
}

/// Whether `p` lies in one of the open top-level folders.
fn in_open_root(roots: &[PathBuf], p: &Path) -> bool {
    roots.iter().any(|r| store::is_within(p, r))
}

impl NoteApp {
    /// Renames a page; the file is named after `stem` when given (refused when that name is taken),
    /// otherwise after `title`. The open page is saved first and reopened under the new name.
    /// Returns the new path of the page.
    pub(super) fn rename_page_now(&mut self, path: &Path, title: &str, exact: Option<&Path>) -> Result<PathBuf, OpError> {
        page_free(path).map_err(OpError::busy)?;
        let sub = store::subpage_dir(path);
        let open = self.release_open_page(|p| store::same_path(p, path) || store::is_within(p, &sub)).map_err(OpError::busy)?;
        let new_path = match store::rename_page(path, title, exact) {
            Ok(p) => p,
            Err(e) => {
                self.after_partial(&e);
                if let Some(r) = &open {
                    self.reopen(r, &r.path.clone());
                }
                return Err(e);
            }
        };
        self.index.relocate(&sub, &store::subpage_dir(&new_path));
        self.index.remove(path);
        self.index.add(&new_path);
        self.index_stale = true;
        self.reload_tree();
        self.reload_pages();
        if let Some(r) = &open {
            let at = if store::same_path(&r.path, path) {
                new_path.clone()
            } else {
                store::rebase(&r.path, &sub, &store::subpage_dir(&new_path)).unwrap_or_else(|| new_path.clone())
            };
            self.reopen(r, &at);
        }
        Ok(new_path)
    }

    /// Renames a section folder to `name` (refused when the name is taken). Returns the new path;
    /// `None`: nothing was done.
    pub(super) fn rename_section_now(&mut self, path: &Path, name: &str) -> Result<Option<PathBuf>, OpError> {
        let moved = self.relocate_folder(path, || {
            store::rename_folder(path, name).map(|p| Some(mover::Moved { path: p, left_behind: Vec::new(), pictures: Vec::new(), kept: Vec::new() }))
        })?;
        Ok(moved.map(|m| m.path))
    }

    /// Deletes a page (with its subpages) into the history; the open page is saved first. Returns the
    /// `deleted\<time>` entry that holds it.
    pub(super) fn delete_page_now(&mut self, path: &Path) -> Result<PathBuf, OpError> {
        let pos = self.pages.iter().position(|p| p.path == *path);
        page_free(path).map_err(OpError::busy)?;
        let sub = store::subpage_dir(path);
        let open = self.release_open_page(|p| store::is_within(p, &sub) || store::same_path(p, path)).map_err(OpError::busy)?;
        let entry = match store::delete_page(path) {
            Ok(e) => e,
            Err(e) => {
                self.after_partial(&e);
                if let Some(r) = &open {
                    self.reopen(r, &r.path.clone());
                }
                return Err(e);
            }
        };
        self.index.remove(path);
        self.index.remove_tree(&sub);
        self.index_stale = true;
        self.reload_tree();
        self.reload_pages();
        if open.is_some() {
            let next = pos.map(|i| i.min(self.pages.len().saturating_sub(1)));
            if let Some(p) = next.and_then(|i| self.pages.get(i)).map(|p| p.path.clone()) {
                self.open_page(&p);
            }
        }
        Ok(entry)
    }

    /// Deletes a section into the history; the open page inside it is saved first. Returns the
    /// `deleted\<time>` entry that holds it.
    pub(super) fn delete_section_now(&mut self, path: &Path) -> Result<PathBuf, OpError> {
        folder_free(path).map_err(OpError::busy)?;
        let open = self.release_open_page(|p| store::is_within(p, path)).map_err(OpError::busy)?;
        let entry = match store::delete_folder(path) {
            Ok(e) => e,
            Err(e) => {
                self.after_partial(&e);
                if let Some(r) = &open {
                    self.reopen(r, &r.path.clone());
                }
                return Err(e);
            }
        };
        self.index.remove_tree(path);
        self.index_stale = true;
        if self.selected_folder.as_ref().is_some_and(|s| store::is_within(s, path)) {
            self.selected_folder = None;
            self.pages.clear();
        }
        self.reload_tree();
        Ok(entry)
    }

    /// Takes back the newest action. Ignored while a window is open (`undo_blocked`).
    pub(super) fn undo(&mut self) {
        if self.undo_blocked() {
            return;
        }
        match self.undo.take_undo() {
            Some(rec) => self.run_step(rec, true),
            None => self.set_status("Nothing to undo".to_string()),
        }
    }

    /// Does the newest undone action again. Ignored while a window is open (`undo_blocked`).
    pub(super) fn redo(&mut self) {
        if self.undo_blocked() {
            return;
        }
        match self.undo.take_redo() {
            Some(rec) => self.run_step(rec, false),
            None => self.set_status("Nothing to redo".to_string()),
        }
    }

    /// Whether a window or dialog is open that undo and redo must not change things behind (its
    /// content would be out of date).
    pub(super) fn undo_blocked(&self) -> bool {
        self.dialog.is_some()
            || self.import_win.is_some()
            || self.move_win.is_some()
            || self.history_win.is_some()
            || self.deleted_win.is_some()
            || self.history_view.is_some()
    }

    /// Runs `rec` backwards (`undoing`) or forwards. On success it moves to the other list; when it
    /// is refused it is dropped, when it is only busy it goes back.
    fn run_step(&mut self, mut rec: Record, undoing: bool) {
        let (verb, done) = if undoing { ("undo", "Undone") } else { ("redo", "Redone") };
        match self.apply_step(&mut rec.step, undoing) {
            Ok(reveal) => {
                if reveal.is_some() {
                    self.reveal = reveal;
                }
                self.set_status(format!("{done}: {}", rec.label));
                if undoing { self.undo.undone(rec) } else { self.undo.redone(rec) }
            }
            Err(e) => {
                let text = format!("Cannot {verb} {}: {}", rec.label, e.text());
                self.set_status(text.clone());
                let sentence = if text.ends_with('.') { text.clone() } else { format!("{text}.") };
                if matches!(e, StepError::Busy(_)) && self.undo.return_busy(rec, undoing) {
                    self.show_message(&format!("Cannot {verb}"), sentence);
                } else {
                    self.show_message(&format!("Cannot {verb}"), format!("{sentence}\n\nIt was removed from the {verb} list."));
                }
            }
        }
    }

    /// The first two checks of a step: the item is there, and its place lies in an open notebook.
    fn check_step(&self, item: &Path, place: &Path) -> Result<(), String> {
        if !item.exists() {
            return Err(GONE.to_string());
        }
        if !in_open_root(&self.roots, place) || !in_open_root(&self.roots, item) {
            return Err("its folder is not open in NoteCraft".to_string());
        }
        Ok(())
    }

    /// The third check, on an item that is there: nobody else edits it, and the open page, when it
    /// lies in it, is saved. Both can pass, so they fail as `Busy`; the operation repeats them.
    fn check_free(&mut self, item: &Path, section: bool) -> Result<(), StepError> {
        let free = if section { folder_free(item) } else { page_free(item) };
        free.map_err(StepError::Busy)?;
        let sub = store::subpage_dir(item);
        let inside = |p: &Path| if section { store::is_within(p, item) } else { store::same_path(p, item) || store::is_within(p, &sub) };
        if self.page.as_ref().is_some_and(|p| inside(&p.path)) && !self.save_if_dirty() {
            return Err(StepError::Busy(SAVE_FAILED.to_string()));
        }
        Ok(())
    }

    /// Brings a deleted item back to exactly its old name. Returns what to show in the tree.
    fn restore_exact(&mut self, entry: &Path, place: &Path) -> Result<Option<PathBuf>, StepError> {
        self.check_step(entry, place)?;
        let item = history::deleted_item_at(entry).ok_or(GONE)?;
        let path = history::restore_deleted(&item, true).inspect_err(|e| self.after_partial(e))?;
        self.after_restore_deleted(&item, &path);
        Ok(Some(if item.kind == history::ItemKind::Section { path } else { place.to_path_buf() }))
    }

    /// Deletes an item again; returns the new `deleted` entry.
    fn delete_again(&mut self, path: &Path, section: bool) -> Result<PathBuf, StepError> {
        self.check_step(path, &parent_of(path)?)?;
        self.check_free(path, section)?;
        Ok(if section { self.delete_section_now(path)? } else { self.delete_page_now(path)? })
    }

    /// Does one step in one direction: the checks in their order (item there, folder open, nobody
    /// editing), then the operation, which refuses itself when an exact name it needs is taken.
    /// A failing check of the item refuses the step; a failing operation leaves it for another try
    /// only when a file was in use (see `StepError`). Returns what to make visible in the tree.
    fn apply_step(&mut self, step: &mut Step, undoing: bool) -> Result<Option<PathBuf>, StepError> {
        match &mut *step {
            Step::MovePage { from, to, pictures, kept } => {
                let (item, dest) = if undoing { (to.clone(), from.clone()) } else { (from.clone(), to.clone()) };
                let target = parent_of(&dest)?;
                self.check_step(&item, &target)?;
                self.check_free(&item, false)?;
                let exact = mover::Exact {
                    stem: store::file_stem(&dest),
                    ext: store::page_ext(&dest),
                    pictures: if undoing { undo::invert(pictures) } else { pictures.clone() },
                    // The pictures that were in the target before the move stay there on undo.
                    keep: if undoing { kept.clone() } else { Vec::new() },
                };
                let moved = self.do_move(&item, &target, Some(&exact))?.ok_or("it cannot be moved there")?;
                if !undoing {
                    *kept = moved.kept.clone();
                }
                step.set_result(undoing, &moved.path);
                Ok(Some(target))
            }
            Step::MoveSection { from, to } => {
                let (item, dest) = if undoing { (to.clone(), from.clone()) } else { (from.clone(), to.clone()) };
                let target = parent_of(&dest)?;
                self.check_step(&item, &target)?;
                self.check_free(&item, true)?;
                let moved = self.do_move_section(&item, &target, Some(&folder_name(&dest)))?.ok_or("it cannot be moved there")?;
                step.set_result(undoing, &moved.path);
                Ok(Some(moved.path))
            }
            Step::RenamePage { from, to, old_title, new_title } => {
                let (item, dest, title) = if undoing { (to.clone(), from.clone(), old_title.clone()) } else { (from.clone(), to.clone(), new_title.clone()) };
                let place = parent_of(&item)?;
                self.check_step(&item, &place)?;
                self.check_free(&item, false)?;
                let new = self.rename_page_now(&item, &title, Some(&dest))?;
                step.set_result(undoing, &new);
                Ok(Some(place))
            }
            Step::RenameSection { from, to } => {
                let (item, dest) = if undoing { (to.clone(), from.clone()) } else { (from.clone(), to.clone()) };
                self.check_step(&item, &parent_of(&item)?)?;
                self.check_free(&item, true)?;
                let new = self.rename_section_now(&item, &folder_name(&dest))?.ok_or("it cannot be renamed")?;
                step.set_result(undoing, &new);
                Ok(Some(new))
            }
            Step::Delete { path, entry, section } => {
                if undoing {
                    Ok(self.restore_exact(entry, &parent_of(path)?)?)
                } else {
                    *entry = self.delete_again(path, *section)?;
                    Ok(Some(parent_of(path)?))
                }
            }
            Step::Create { path, section, entry } => {
                if undoing {
                    *entry = Some(self.delete_again(path, *section)?);
                    Ok(Some(parent_of(path)?))
                } else {
                    let e = entry.clone().ok_or(GONE)?;
                    Ok(self.restore_exact(&e, &parent_of(path)?)?)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_and_item_may_lie_in_different_open_roots() {
        let roots = vec![PathBuf::from(r"C:\Notes\A"), PathBuf::from(r"C:\Notes\B")];
        assert!(in_open_root(&roots, Path::new(r"C:\Notes\A\x\p.md")));
        assert!(in_open_root(&roots, Path::new(r"c:\notes\b\q.md")));
        assert!(!in_open_root(&roots, Path::new(r"C:\Notes\C\q.md")));
    }
}
