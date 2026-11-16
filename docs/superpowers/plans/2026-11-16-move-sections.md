# Moving Sections Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move a section (folder, with everything in it) into any other folder of the open
notebooks, by drag and drop in the tree or with "Move to…"; and make section rename/delete save and
release the open page first.

**Architecture:** `mover.rs` gets `can_move_section_to` and `move_section` (a folder move through
the existing `free_stem` and `move_path`), unit-tested against temp folders. `app.rs` gets one helper
that saves and releases the open page before a folder operation; rename, delete, page move and
section move use it. `move_ui.rs` learns a second drag payload and a second dialog kind.

**Tech Stack:** Rust 2024, eframe/egui 0.37 (drag-and-drop payloads).

**Spec:** `docs/superpowers/specs/2026-11-16-move-sections-design.md`

## Global Constraints

- Build/test: in PowerShell run `$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"` first, then
  `cargo test` from `H:\RAZVOJ\NoteCraft\app`. Baseline: 156 passed, 1 ignored.
- Branch `feat/move-sections`; never commit to `master`.
- Error strings returned to the UI are plain user-facing sentences; they are shown as is.
- Name clash in the target: `Name`, `Name (2)`, `Name (3)` … via the existing `mover::free_stem`
  (free as folder `Name\`, as page `Name.md` and as a versions folder).
- Folder comparisons are case-insensitive: use `store::path_key` / `mover::is_within`.
- Menu label **Move to…** (ellipsis character) in the section context menu, right after **Rename…**,
  only for non-root folders.
- Status line after a section move: `Moved section "<name>" to <place>`, where `<place>` is the new
  parent as `Notebook › Group` — `Index::place_of(&new_folder)` (it shows the parent of the path).
- Dialog title for a section: **Move section**; prompt: `Where should "<name>" go? Pick a folder.`
  Line under the tree: `Move into "<folder name>"`.

## Review Focus

1. Dropping a section on its current parent typed in another case (`h:\nb` vs `H:\NB`) must be a
   no-op, not a move to `Name (2)`. Test: Task 1 `same_parent_other_case_is_noop`.
2. Target holds a page `X.md` (no folder `X\`): the moved section must become `X (2)`, otherwise it
   silently turns into the subpages of `X.md`. Test: Task 1 `clash_with_page_gets_suffix`.
3. A sibling whose name starts with the section's name (`A` into `AB`) is a valid target, while `A`
   into `A\Sub` is not. Tests: Task 1 `sibling_with_prefix_name_is_allowed`, `refuses_into_descendant`.
4. The open page, in edit mode with unsaved text, lies inside a section that is moved, renamed or
   deleted: the text is saved first and the page reopens at the new path (not after delete). Not
   unit-testable; Task 2 and Task 3 manual checks.
5. Another computer edits a page inside the dragged section: the move is refused with the
   `folder_free` message and nothing changes. Manual check in Task 3.

---

### Task 1: `move_section` in `mover.rs`

**Files:**
- Modify: `src/mover.rs` (new functions after `move_page`; tests in the existing `mod tests`)

**Interfaces:**
- Consumes: `free_stem(dir, stem) -> String`, `move_path(from, to) -> Result<(), String>`,
  `is_within(p, dir) -> bool`, `store::path_key`; test helpers `temp`, `notebook`, `page_with`,
  `pic`, `version_of`.
- Produces:
  - `pub fn can_move_section_to(folder: &Path, target: &Path) -> bool` — false when `target` is the
    folder's parent or lies within `folder` (itself included).
  - `pub fn move_section(folder: &Path, target: &Path) -> Result<Option<PathBuf>, String>` —
    `Ok(None)` when `target` is the current parent; `Err` when `target` is within `folder`
    (message: `A section cannot be moved into itself.`); otherwise `Ok(Some(target.join(free_stem(target, name))))`.
    Creates nothing else; does not check locks (the UI does).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn section_to_other_section() {
    // notebook: A/P.md (+ A/P/S.md subpage, A/resources/x.png, a version of P), B
    // move_section(&a, &b) == Some(b.join("A")); b/A/P.md, b/A/P/S.md, b/A/resources/x.png exist;
    // history::list_versions(&b.join("A").join("P.md")).len() == 1; !a.exists()
}
#[test] fn section_to_notebook_root() { /* d/A/G/C (C has a page) -> move C to d: Some(d.join("C")) */ }
#[test] fn clash_with_folder_gets_suffix() { /* B/A exists -> move A into B gives B/"A (2)" */ }
#[test] fn clash_with_page_gets_suffix() { /* B/A.md exists -> move A into B gives B/"A (2)"; B/A.md untouched */ }
#[test] fn same_parent_is_noop() { /* move_section(&a, &d) == Ok(None); a still exists */ }
#[test] fn same_parent_other_case_is_noop() {
    /* target = PathBuf::from(d.to_string_lossy().to_uppercase()) -> Ok(None) (Windows only: #[cfg(windows)]) */
}
#[test] fn refuses_into_itself() { /* move_section(&a, &a) is Err; can_move_section_to(&a, &a) false */ }
#[test] fn refuses_into_descendant() { /* a/Sub exists: move_section(&a, &a.join("Sub")) is Err, a/Sub untouched */ }
#[test] fn sibling_with_prefix_name_is_allowed() {
    /* d/A and d/AB: can_move_section_to(&a, &d.join("AB")) true; move gives d/AB/A */
}
```

Each test ends with `let _ = fs::remove_dir_all(&d);`.

- [ ] **Step 2: Run them, expect compile failure** (`move_section` not found)

Run: `cargo test mover::tests::section`
Expected: error `cannot find function move_section`.

- [ ] **Step 3: Implement `can_move_section_to` and `move_section`** in `src/mover.rs`

Parent compare with `path_key`; "within" with `is_within`; folder name from `folder.file_name()`
(`Err("This folder cannot be moved.")` when there is none); move with `move_path`.

- [ ] **Step 4: Run the tests**

Run: `cargo test mover::tests` then `cargo test`
Expected: all pass; total 165 passed, 1 ignored (156 + 9).

- [ ] **Step 5: Commit**

```bash
git add src/mover.rs
git commit -m "feat: move_section moves a section folder into another folder"
```

---

### Task 2: Save and release the open page before section rename and delete

**Files:**
- Modify: `src/app.rs` (`Dialog::RenameSection` / `Dialog::DeleteSection` handling ~1587-1621; new helper next to `save_if_dirty`)
- Modify: `src/app/move_ui.rs` (`do_move` uses the helper)

**Interfaces:**
- Produces: `fn release_open_page(&mut self, inside: impl Fn(&Path) -> bool) -> Result<Option<PathBuf>, String>`
  on `NoteApp` (`pub(super)`). When an open page exists and `inside(&page.path)`:
  `save_if_dirty()`; if `body != saved_body` afterwards return
  `Err("The open page could not be saved, so nothing was changed. Save it first.")`;
  `leave_edit()`; `self.page = None`; return `Ok(Some(old_path))`. Otherwise `Ok(None)`, no change.

- [ ] **Step 1: Add `release_open_page`** in `src/app.rs`.

- [ ] **Step 2: Use it in `do_move`** (`move_ui.rs`): replace the inline save/check/`leave_edit`/`page = None`
  block with `release_open_page(|p| p == page || p.starts_with(&sub))?`, deriving `open_rel` from the
  returned path exactly as now. Behaviour must not change.

- [ ] **Step 3: Use it in `Dialog::RenameSection`**: after `folder_free(path)?`, call
  `release_open_page(|p| p.starts_with(path))?` *before* `store::rename_folder`. After the rename,
  reopen `new_path.join(old.strip_prefix(path))` when a page was released. If the rename fails,
  reopen the old path, then return the error. Remove the old `self.page.take()` block.

- [ ] **Step 4: Use it in `Dialog::DeleteSection`**: `release_open_page(|p| p.starts_with(path))?`
  before `store::delete_folder`; remove the old `self.page = None` block; do not reopen. If the
  delete fails, reopen the old path, then return the error.

- [ ] **Step 5: Build and run the tests**

Run: `cargo test`
Expected: 165 passed, 1 ignored; no new warnings from `cargo build`.

- [ ] **Step 6: Manual check (user)** — run the app; in section S open a page, enter edit mode,
  type text without saving:
  1. Rename S → page reopens at the new path showing the typed text; no lock file left in the old
     or new folder for that page except the one of the reopened editing session.
  2. Again with unsaved text: delete S → restore it from Folders > Deleted items → the page has the
     typed text.
  3. Move a page (drag) whose text is unsaved → still saved and reopened as before.

- [ ] **Step 7: Commit**

```bash
git add src/app.rs src/app/move_ui.rs
git commit -m "fix: save and release the open page before renaming or deleting its section"
```

---

### Task 3: Drag, drop and "Move to…" for sections

**Files:**
- Modify: `src/app/move_ui.rs` (payload, drop target, feedback, dialog kind, `do_move_section`)
- Modify: `src/app.rs` (`Action::MoveSection`, `Action::ShowMoveSection`, `folder_label` drag source
  and menu item, action handling next to `Action::MovePage` ~669)

**Interfaces:**
- Consumes: `mover::can_move_section_to`, `mover::move_section` (Task 1); `release_open_page` (Task 2).
- Produces:
  - `pub struct DragSection(pub PathBuf, pub String)` (path, name).
  - `Action::MoveSection { folder: PathBuf, target: PathBuf }`, `Action::ShowMoveSection(PathBuf)`.
  - `pub(super) fn do_move_section(&mut self, folder: &Path, target: &Path) -> Result<(), String>`.
  - `pub(super) fn open_move_section(&mut self, folder: PathBuf)`.
  - `MoveDialog` gains `kind: MoveKind` with `enum MoveKind { Page, Section }`; `page` field renamed
    `item` (the page file or the section folder).

- [ ] **Step 1: `do_move_section`** — sequence: return `Ok(())` if `!can_move_section_to`;
  `folder_free(folder)?`; `release_open_page(|p| p.starts_with(folder))?`; `move_section`;
  on `Ok(None)` / `Err` reopen the released page at its old path (and return the error);
  on `Ok(Some(new))`: `index.remove_tree(folder)`, `index.add_tree(&new)`, `index_stale = true`;
  if `selected_folder` is `folder` or inside it, map it to `new.join(rest)`; `reload_tree()`,
  `reload_pages()`; reopen the released page at `new.join(rest)`; status per Global Constraints.

- [ ] **Step 2: Drag source and drop target** — in `folder_label`, when the folder is not in
  `self.roots`: `Sense::click_and_drag()` and `dnd_set_drag_payload(DragSection(path, name))`
  (a plain click must still select). Extend the folder drop target to also accept `DragSection`
  when `can_move_section_to(&p.0, &f.path)`: set `drop_allowed`, same highlight, release pushes
  `Action::MoveSection`. Page rows keep accepting only `DragPage`.

- [ ] **Step 3: Drag feedback** — `drag_feedback` shows the title of whichever payload is active
  (`DragPage.1` or `DragSection.1`) and the `NoDrop` cursor when `!allowed`, for both.

- [ ] **Step 4: Dialog** — `open_move_section` sets `kind: Section`, `home` = the folder's parent.
  In `move_folder_rows` for `Section`: no page rows; a folder row is enabled iff
  `can_move_section_to(&win.item, &f.path)`; selecting sets the label `Move into "<name>"`.
  `move_window` uses the title/prompt from Global Constraints and calls `do_move_section` for
  `Section`. Context menu item **Move to…** after **Rename…** pushes `Action::ShowMoveSection`.
  Action errors go to `show_message("Move section", e)`.

- [ ] **Step 5: Build and run the tests**

Run: `cargo test` and `cargo build`
Expected: 165 passed, 1 ignored; no warnings.

- [ ] **Step 6: Manual check (user)**
  1. Drag section A onto section B → A appears under B with its pages; pictures show; versions
     list intact. Status `Moved section "A" to Notebook › B`.
  2. Drag A onto its own parent, onto itself, onto a child of itself, onto a page row → no
     highlight, "no entry" cursor, nothing happens.
  3. Drag a notebook (root) → it does not start a drag; a click still selects it.
  4. Drag A into another notebook, and onto a folder holding a page named `A` → `A (2)`.
  5. Move to… on A → dialog lists only folders; A, its children and its parent are greyed;
     Move works; Cancel changes nothing.
  6. Open page inside A in edit mode with unsaved text, then move A → text saved, page reopens
     at the new place, the selected section follows if it was A.
  7. On a second computer, edit a page inside A, then try to move A → refusal message, nothing moved.
  8. Plain page drag and drop and the page Move to… dialog still work as before.

- [ ] **Step 7: Commit**

```bash
git add src/app.rs src/app/move_ui.rs
git commit -m "feat: move sections by drag and drop or Move to…"
```
