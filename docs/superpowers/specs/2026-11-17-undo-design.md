# Undo and redo of note management - design

Date: 2026-11-17. Branch: `feat/undo` (not created yet). Follows "Moving pages" and "Moving
sections" (`2026-11-16-move-pages-design.md`, `2026-11-16-move-sections-design.md`).

## Why

A drag in the tree that drops a page or section in the wrong place is easy to make and is noticed
right away (user, 2026-11-17). Today the only fix is to find the item and move it back by hand, and
a move that renamed the item or its pictures on a name clash cannot be put back exactly at all.
The same applies to a rename or delete done by mistake.

## What the user gets

- **Ctrl+Z** undoes the last note-management action, **Ctrl+Y** (and Ctrl+Shift+Z) redoes it.
- **Two buttons in the top bar**, ↶ and ↷, left of ⟳. Their hover text names the action
  (`Undo move of "Budget" (Ctrl+Z)`); they are disabled when there is nothing to undo or redo.
  *(My call: the top bar is where the eye is after a drag; the Folders menu is for rarer things.)*
- After an undo or redo the tree and page list show the affected item, as after a move (selection
  and `reveal` follow it), and the status line says `Undone: move of "Budget"` /
  `Redone: move of "Budget"`.
- **Session only** (user, 2026-11-17): the list lives in memory and is gone when the app closes.
- **Depth**: the last 20 actions *(my call)*. A new action clears the redo list.

## Which actions

| Action                  | Undo                                                        | Redo                  |
|-------------------------|-------------------------------------------------------------|-----------------------|
| Move page               | Move back to the old folder, old file name, old picture names | Move again          |
| Move section            | Move back to the old parent under the old name              | Move again            |
| Rename page             | Rename back to the old title                                | Rename again          |
| Rename section          | Rename back to the old name                                 | Rename again          |
| Delete page / section   | Restore from Deleted items, to the old name                 | Delete again          |
| New page / new section  | Delete it into Deleted items (nothing is lost)              | Restore it from Deleted items |

Moves come from drag and drop and from the "Move to…" dialog alike; both go through `do_move` /
`do_move_section` and record there.

## Goals / non-goals / anti-goals

- Goal: undo a wrong move, rename, delete or create immediately, exactly as it was before.
- Non-goals: page text (it has its own version history and the editor's own undo); restoring or
  purging in the History / Deleted items windows; OneNote import; adding or removing top-level
  folders; undo after restarting the app; undo of actions made on another computer.
- Anti-goals: an undo that half-happens; an undo that overwrites another page or folder; undoing
  over a change made since (outside the app or on another computer); losing unsaved text of the open
  page; Ctrl+Z in a text field or in the page editor reaching the notebook undo.

## Approaches

- **A. Action log on top of the existing operations.** Each successful action pushes a record of
  what it did; undo and redo call the same `mover` / `store` / `history` functions in the other
  direction. Move and delete are extended to report what is needed to reverse them exactly.
  **Chosen (my call; the user left the how to me, 2026-11-17).**
- B. A file-level journal: record every rename and copy made by `mover`, `store` and `history`, undo
  replays them backwards. Exact, but touches all file code, and a cross-drive move has already
  deleted originals that a journal cannot recreate.
- C. Snapshot the affected folders before each action. Too slow and too large on network drives.

## Design

### The log (`src/undo.rs`, new)

```rust
pub enum Step {
    MovePage    { from: PathBuf, to: PathBuf, pictures: Vec<(String, String)> },
    MoveSection { from: PathBuf, to: PathBuf },
    RenamePage  { from: PathBuf, to: PathBuf, old_title: String, new_title: String },
    RenameSection { from: PathBuf, to: PathBuf },
    Delete      { path: PathBuf, entry: PathBuf, section: bool },
    Create      { path: PathBuf, section: bool, entry: Option<PathBuf> },
}
pub struct Record { pub step: Step, pub label: String }   // label: `move of "Budget"`
pub struct UndoLog { undo: Vec<Record>, redo: Vec<Record> } // cap 20, oldest dropped
```

`from` / `to` are the full paths before and after the action (`to` is where the item is now, with
any "(2)" the action added). `pictures` are the picture renames a page move made on a clash
(old name in the source `resources`, new name in the target). The log lives in `NoteApp`; the UI
code pushes after each successful action, never inside `mover` / `store` / `history`.

### What the existing operations must report

- `mover::move_page` gets an optional wanted stem: `move_page(page, target_dir, stem: Option<&str>)`.
  Undo passes the old stem so the page gets its old file name back (when free, see checks).
  `Moved` gets `pictures: Vec<(String, String)>` (the clash renames it made).
- Undo of a page move must leave the page and its versions linking the **old** picture names, and the
  pictures in the old `resources` folder under those names. The builder chooses how: e.g. an
  optional rename map for `move_page` (copy `new` back as `old` and rewrite links), applied by the
  same step 1/3 code that makes clash renames today.
- `mover::move_section` gets the same optional wanted name.
- `store::rename_page` gets an optional wanted stem too, so undo restores the old **file name**, not
  only the old title (a stem can differ from `safe_name(title)`: an earlier "(2)", an imported
  page). The `RenamePage` record keeps both stems. `store::rename_folder` already refuses a clash,
  so it needs no change. Top-level folders cannot be renamed (the menu offers only "Remove from
  NoteCraft"), so `roots` and the settings are never touched.
- **Pictures on undo of a page move** *(my calls)*: the rename map is applied to the page **and** its
  versions. A picture is put back under its old name in the old `resources` folder; if a file with
  that name is there already with other bytes, check 2 refuses the whole undo. A same-byte file
  counts as already there. The copies in the target `resources` folder are removed only when
  nothing left in the target folder links them, as step 4 of `move_page` does today. Links the page
  gained after the move are moved like any picture the page links.
- **A move that left files behind** (`left_behind` not empty, a cross-drive move with a file open
  elsewhere) is **not recorded** *(my call)*: its "could not remove" message already asks the user to
  look, and leftovers would make the undo's checks unreliable. The message adds "This move cannot
  be undone."
- `store::delete_page` / `store::delete_folder` return the Deleted-items entry folder that
  `history::delete_page` / `history::delete_folder` already return (today it is dropped). Undo uses
  `history::deleted_item_at(entry)` + `history::restore_deleted`.
- `history::restore_deleted` gives a numbered name when the old one is taken. Undo must not: it
  checks first (below) and refuses instead.

### Checks before an undo or redo (all before anything changes)

The step is refused, the status line says why, and the record is dropped from the list (the rest of
the list stays) when:

1. The item is not where the record says (`to` for undo, `from` for redo; for a delete, the entry
   folder is gone - e.g. "Purge history" or "Delete forever" ran).
2. The name it would get back is taken (another page/folder now uses the old name), so nothing is
   overwritten and nothing gets a "(2)" by surprise. **Redo is exact the same way** *(my call)*: it
   asks for the stem/name in `to` (including any "(2)" the first action gave) and is refused, not
   numbered, when that is taken. Names compare without case, so a case-only rename back
   (`budget` → `Budget`) is the item's own name, not a clash.
3. It, or a page inside it, is being edited on another computer (`page_free` / `folder_free`).
4. Its place is no longer in an open top-level folder (removed from view).

*(My call: drop the record rather than keep it, so the next Ctrl+Z does not hit the same wall.)*
The open page is saved and released first with `release_open_page`, exactly like a move; if the
save fails, nothing happens and the record stays.

If the operation itself then fails half-way, the existing rollback in `mover` / `history` applies and
its message is shown as today; the record is dropped and the tree reloaded.

### Doing the step

Undo and redo go through the same code as the UI actions (`do_move`, `do_move_section`, the
rename/delete/create branches of `do_dialog`), refactored so that the file operation and the
index/tree/reopen follow-up can be called with explicit names and without opening a dialog. They do
**not** push a new record when called from undo/redo; instead the record moves between the undo and
redo lists, with its paths updated to where the item really ended up.

- Undo of **create**: the new page/section is deleted into Deleted items (with `release_open_page`
  if open); the record keeps the entry so redo can restore it.
- Undo of **rename page**: `store::rename_page(to, old_title)`; it rewrites `title` and `updated` in
  the front matter (accepted: `updated` changes).

### Keys

- In the egui window: Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z in `shortcuts()`, only when no egui text field
  has focus (search box, dialog fields keep their own undo) and no dialog / move / import / history
  window is open.
- In the page view: `editor.js` forwards `z` / `y` with Ctrl as `undo` / `redo` **only when not
  editing**; while editing they stay with the editor. `WebMsg::Key` handles `undo` / `redo`.

## Left to the builder

- How the picture names are put back on undo of a page move (see above), as long as the result is
  as before the move.
- The shape of the refactor that lets undo call the actions without dialogs.
- Exact labels and status texts, following the existing tone.
- Tests: `mover` / `store` / `history` unit tests for the new parameters (move with wanted stem,
  picture rename map, delete returning the entry) and `undo.rs` tests for the log (cap, redo cleared
  on new action, record dropped on a failed check).
