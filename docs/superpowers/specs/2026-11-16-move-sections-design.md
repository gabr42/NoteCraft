# Moving sections - design (short)

Date: 2026-11-16. Branch: `feat/move-sections`. Follows "Moving pages"
(`2026-11-16-move-pages-design.md`), whose behaviour this copies wherever it fits.

## Why

Same as moving pages: reorganise notes after the OneNote import. Pages can now move; whole sections
(with their pages, subpages, versions and pictures) cannot.

## What the user gets

- **Drag and drop**: drag a section (or section group) in the tree and drop it on any other folder
  in the tree (notebook, section group, section). It becomes a child of that folder.
- **"Move to…"** in the section's right-click menu: the same dialog as for pages, showing only the
  folders of all open notebooks (pages are not targets).
- **Not** on a page: a section dropped on a page row is refused (decided by the user, 2026-11-16).

## Goals / non-goals / anti-goals

- Goal: move a section with everything in it, across notebooks and drives.
- Non-goals: moving a notebook itself (an open root folder); turning a section into pages or
  subpages; reordering (the tree is sorted by name); undo (moving back is the undo).
- Anti-goals: a broken picture link; overwriting another folder or page; moving a section in which
  another computer edits a page; losing unsaved text of the open page.

## Approaches

- **A. Extend the move-pages code** - `mover::move_section` beside `move_page`, one dialog and one
  drag feedback for both kinds. **Chosen by the user.**
- B. A separate dialog and drag for sections - duplicates the tree and drag code, drifts apart.
- C. "Rename into another folder" in `store::rename_folder` - refuses clashes, has no cross-drive
  fallback and no "under itself" check; would grow into A anyway.

## Design

**Core: `mover::move_section(folder, target) -> Result<Option<PathBuf>, String>`** and
`mover::can_move_section_to(folder, target) -> bool`.

- A section folder is self-contained: its pages, `resources\` and `.history\` live inside it, so
  moving the folder keeps every link valid. No picture handling.
- `None` (nothing to do) when `target` is the folder's current parent. Refused when `target` is the
  folder itself or lies inside it.
- New name: `free_stem(target, name)` - free as a folder *and* as a page `Name.md` (a folder next to
  `Name.md` would be read as that page's subpages); a clash gives `Name (2)` like page moves.
- The move itself: `move_path` (rename, or copy + delete across drives).

**UI**

- Folder rows that are not open roots are drag sources (`DragSection(path, name)` payload). A plain
  click still selects; the expand arrow still works.
- Every folder row is a drop target for both pages and sections; page rows accept only pages.
  Valid targets get the same highlight; elsewhere the "no entry" cursor (`drag_feedback` handles
  both payloads, the section name follows the pointer).
- `MoveDialog` gets a kind (page / section). For a section: title "Move section", no page rows,
  the section itself, its descendants and its current parent are greyed out.
- `Action::MoveSection { folder, target }`: `folder_free(folder)`, release the open page if it is
  inside (below), move, update the index (`remove_tree` old, `add_tree` new, mark stale), the
  selected folder follows if it was the moved section or inside it, reload tree and page list,
  reopen the page at its new path, status `Moved section "<name>" to <place>`.

**Releasing the open page (also fixes rename and delete)** - one helper used by section move,
section rename and section delete when the open page lies in the folder: `save_if_dirty()`; if the
text is still unsaved, refuse ("The open page could not be saved… Save it first."); `leave_edit()`
(drops this computer's edit lock, whose file is inside the folder); `self.page = None`. Afterwards
move/rename reopen the page at its new path; delete does not (the saved text is in the deleted
section's history and can be restored). Today rename and delete drop the open page without saving
and keep the edit lock.

## Tests

- `move_section`: into another section, into another notebook root, into a section group; pages,
  subpages, `resources\` and `.history\` come along; clash with a folder and with a page stem gives
  `Name (2)`; same parent is a no-op; refused into itself and into a descendant.
- `can_move_section_to` for the cases above.
- UI and the rename/delete fix: checked by the user by hand.

## Calls made without the user (check these)

- After a move the view follows the moved section only if it was selected (or something inside
  it); otherwise it stays where it was.
- Delete saves the open page before deleting, so its last text is in the restorable history.

## Left to the builder

- Helper names, how `MoveDialog` stores its kind, drag threshold details.
