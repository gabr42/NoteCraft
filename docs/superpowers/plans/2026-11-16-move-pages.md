# Moving Pages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move a page (with subpages, versions and pictures) to another section or under another
page, by drag and drop or with a "Move to…" dialog.

**Architecture:** A new module `src/mover.rs` holds the file-system operation `move_page` and its
helpers (link scanning, free names, cross-volume moves), unit-tested against temp folders. A new
UI module `src/app/move_ui.rs` draws the "Move to…" dialog; `app.rs` gets the action, the menu
items and the drag-and-drop wiring.

**Tech Stack:** Rust 2024, eframe/egui 0.37 (drag-and-drop payloads), `regex` (already a dependency).

**Spec:** `docs/superpowers/specs/2026-11-16-move-pages-design.md`

## Global Constraints

- Build/test: `cargo` lives in `%USERPROFILE%\.cargo\bin`; in PowerShell run
  `$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"` first. Run from `H:\RAZVOJ\NoteCraft\app`.
- Work on branch `feat/move-pages`; never commit to `master`.
- Error strings returned to the UI are plain user-facing sentences (like `store.rs`), they are shown as is.
- A name clash in the target uses the `create_page` convention: `Name.md`, `Name (2).md`, `Name (3).md` …
  (the spec says "like `create_page`"; its `-1` example is superseded by this).
- Menu label: **Move to…** (with the ellipsis character), in the page-list context menu and the page `⋯` menu,
  placed right after **New subpage…**.
- Status line after a move: `Moved "<title>" to <place>` where `<place>` is
  `Notebook › Section › Page` from `Index::place_of(new_path)` (`store.rs` ~line 636; make it `pub(crate)`).

## Review Focus

1. Pasted pictures have spaces in their names and are linked percent-encoded
   (`resources/Plan%202026-11-16%20103000.png`); they must be found, moved and, on rename, re-linked
   in the same encoded form. Test: Task 1 `links_decode_percent`, Task 2 `move_pasted_picture_with_spaces`.
2. A picture used only by a deleted page (in `.history\deleted`) must not be deleted from the source,
   because restoring that page would show a broken picture. Test: Task 2 `keeps_picture_used_by_deleted_page`.
3. A target folder that already has `Name.md`, or only an orphan `Name\` folder or
   `.history\pages\Name` folder, must give `Name (2)`. Test: Task 1 `free_stem_skips_folders_and_versions`.
4. Moving a subpage back to the top level of its own section, and "moving" a page to the folder it is
   already in (must be a no-op, no file touched). Tests: Task 2 `subpage_to_top_level`, `same_folder_is_noop`.
5. The open page in edit mode with unsaved text is moved: the text must be saved before the move and the
   page reopened at the new path. Not unit-testable; it is step 6 of Task 3's manual check.

---

### Task 1: Helpers in `mover.rs` - picture links, free names, moving across drives

**Files:**
- Create: `src/mover.rs` (module + `#[cfg(test)] mod tests`)
- Modify: `src/main.rs` (add `mod mover;` next to the other `mod` lines)

**Interfaces:**
- Produces (all `pub(crate)` in `crate::mover`):
  - `fn picture_links(text: &str) -> Vec<String>` - file names (percent-decoded, no duplicates, in order of
    first appearance) of every `resources/<name>` link in markdown `](resources/…)`, autolink
    `<resources/…>` and HTML `src="resources/…"` / `src='resources/…'`. A name ends at `)`, `"`, `'`, `>`
    or whitespace. `%XX` sequences are decoded (UTF-8).
  - `fn rewrite_picture_link(text: &str, old: &str, new: &str) -> String` - replaces every link to `old`
    (matched by its decoded name, in any of the three forms) by `new`, written with `encode_name`.
  - `fn encode_name(name: &str) -> String` - percent-encodes `%`, space, `(`, `)` and nothing else.
  - `fn free_stem(dir: &Path, stem: &str) -> String` - first of `stem`, `stem (2)`, `stem (3)` … for
    which none of `dir\<s>.md`, `dir\<s>\` and `history::versions_dir(&dir.join("<s>.md"))` exists.
  - `fn move_path(from: &Path, to: &Path) -> Result<(), String>` - `fs::rename`; when that fails and the two
    paths are on different drives (first path component differs, case-insensitive), copy (file or whole
    folder tree) and then remove the source. Error text includes both paths and the OS error.

- [ ] **Step 1: Write the failing tests** in `src/mover.rs` (temp folders as in `store.rs` `temp_root`):

```rust
#[test] fn links_all_forms() {
    let t = "![a](resources/x.png) <img src=\"resources/y.jpg\"> [f](resources/z.pdf) <resources/w.png> ![a](resources/x.png) ![b](other/q.png)";
    assert_eq!(picture_links(t), vec!["x.png", "y.jpg", "z.pdf", "w.png"]);
}
#[test] fn links_decode_percent() {
    assert_eq!(picture_links("![](resources/Plan%202026-11-16%20103000.png)"), vec!["Plan 2026-11-16 103000.png"]);
}
#[test] fn rewrite_keeps_other_links() {
    let t = "![](resources/a%20b.png) ![](resources/c.png) <img src=\"resources/a%20b.png\">";
    assert_eq!(rewrite_picture_link(t, "a b.png", "a b-2.png"),
        "![](resources/a%20b-2.png) ![](resources/c.png) <img src=\"resources/a%20b-2.png\">");
}
#[test] fn free_stem_skips_folders_and_versions() {
    // dir has Plan.md; Plan (2)\ folder only; .history\pages\Plan (3)\ only  -> "Plan (4)"
    // and for a fresh stem "New" -> "New"
}
#[test] fn move_path_copy_fallback() {
    // call the private copy-then-remove helper directly on a folder with a nested file:
    // target has the same tree and content, source is gone.
}
```

- [ ] **Step 2: Run** `cargo test mover` - expected: compile errors / failures for the missing functions.
- [ ] **Step 3: Implement** the five functions with the signatures above; one `regex::Regex` (in a
  `LazyLock`) covering the three link forms; the copy fallback as a private
  `fn copy_then_remove(from: &Path, to: &Path) -> std::io::Result<()>`.
- [ ] **Step 4: Run** `cargo test mover` - expected: all pass; `cargo build` without warnings.
- [ ] **Step 5: Commit** `feat(move): helpers for picture links, free names and moves across drives`

### Task 2: `mover::move_page` - the move itself, with pictures (option B)

**Files:**
- Modify: `src/mover.rs`

**Interfaces:**
- Consumes: Task 1 helpers; `store::subpage_dir`, `store::file_stem`, `history::versions_dir`.
- Produces: `pub fn move_page(page: &Path, target_dir: &Path) -> Result<Option<PathBuf>, String>` -
  `Ok(None)` when `target_dir` is already the page's folder (nothing touched), `Ok(Some(new_path))` after a move.
  Also `pub fn can_move_to(page: &Path, target_dir: &Path) -> bool` - false for the page's own folder,
  its subpage folder and anything below it (used by the UI to grey out / ignore targets).
  The edit-lock check is NOT inside `move_page`; the caller runs `page_free` (Task 3).

Algorithm (order is the safety guarantee from the spec; implement in this order):

```text
1. if same folder -> Ok(None); if !can_move_to -> Err("A page cannot be moved under itself.")
2. create target_dir; stem = free_stem(target_dir, file_stem(page)); new = target_dir\stem.md
3. texts = page file + every file in versions_dir(page) (recursive)
   for name in union of picture_links(texts):
       src = page_dir\resources\name; skip if missing
       dst = target_dir\resources\name
       if !dst.exists(): copy src -> dst                       (copy, not move)
       elif same bytes: nothing
       else: n = first free "<stem>-<k>.<ext>" (k = 2,3,..) in target resources; copy src -> it;
             record the rename (old name -> new name) - texts are NOT written yet
       remember name in `copied`
4. move page file -> new; then subpage_dir(page) -> subpage_dir(new) if it exists;
   then versions_dir(page) -> versions_dir(new) if it exists (create its parent first);
   all with move_path. On failure: move the already-moved ones back (reverse order, best effort),
   return the error; source files and source pictures are unchanged.
4b. if renames were recorded: apply rewrite_picture_link to the moved page file and every moved
   version file and write them back.
5. for name in copied: delete page_dir\resources\name unless some remaining file still links it:
   every *.md directly in page_dir, and every file under page_dir\.history\ (recursive).
   A file that cannot be read counts as linking it. Failures to delete are ignored.
6. Ok(Some(new))
```

- [ ] **Step 1: Write the failing tests** (helper `page_with(dir, name, body)` writing a minimal front
  matter + body; versions via `history::snapshot_now`):

```rust
#[test] fn to_other_section()            // page + subpage folder + versions dir arrive; source has none of them
#[test] fn subpage_to_top_level()        // Sec\P\S.md -> Sec\S.md
#[test] fn under_other_page()            // Sec\A.md -> Sec\B\A.md, B\ created
#[test] fn same_folder_is_noop()         // Ok(None), mtime of page unchanged
#[test] fn refuses_under_itself()        // target = subpage_dir(page) and subpage_dir(page)\X -> Err, nothing moved
#[test] fn name_clash_gets_suffix()      // target has A.md -> result A (2).md, title in front matter unchanged
#[test] fn picture_moves_when_unused()   // source resources\x.png gone, target has it, link unchanged
#[test] fn keeps_picture_used_by_sibling()        // other page in source links x.png -> source keeps it
#[test] fn keeps_picture_used_by_version_of_sibling()
#[test] fn keeps_picture_used_by_deleted_page()   // store::delete_page(other) first, then move
#[test] fn identical_picture_not_duplicated()     // target has same bytes x.png -> only one, link unchanged
#[test] fn different_picture_same_name_renamed()  // target has other x.png -> x-2.png, page AND its versions link x-2.png, target x.png untouched
#[test] fn move_pasted_picture_with_spaces()      // link resources/P%202026.png, file "P 2026.png" moves
#[test] fn missing_picture_link_left_alone()
```

- [ ] **Step 2: Run** `cargo test mover` - expected: failures for the missing `move_page`.
- [ ] **Step 3: Implement** `move_page` and `can_move_to` per the algorithm above. Comparing paths:
  case-insensitive on components (reuse the approach of `store::path_key`; make it `pub(crate)`).
- [ ] **Step 4: Run** `cargo test` (whole suite) - expected: all pass.
- [ ] **Step 5: Commit** `feat(move): move a page with subpages, versions and pictures`

### Task 3: "Move to…" dialog, menu items and the move action

**Files:**
- Create: `src/app/move_ui.rs` (dialog drawing, like `history_ui.rs` / `import_ui.rs` are split out)
- Modify: `src/app.rs` - `Action` (`MovePage { page: PathBuf, target_dir: PathBuf }`, `ShowMove(PathBuf)`),
  `page_context_menu` (~line 1107) and the `⋯` menu (~line 1211), action handling, `mod move_ui;`
- Modify: `README.md` - in the paragraph at line ~79 ("Right-click a section or a page…") add moving:
  right-click ▸ Move to…, or drag a page onto a section in the tree / onto another page in the list.

**Interfaces:**
- Consumes: `mover::move_page`, `mover::can_move_to` (Task 2); `page_free` (app.rs).
- Produces: `fn do_move(&mut self, page: &Path, target_dir: &Path)` on the app, used by Task 4's drop handler.
  `do_move` = the spec's "UI" bullet: if the open page is the moved page or under its subpage folder,
  `save_if_dirty()`, remember its path relative to the moved page, `self.page = None`; `page_free(page)?`;
  `mover::move_page`; on `Ok(None)` do nothing; on `Ok(Some(new))` index remove (old page + old subpage tree),
  add (new page + new subpage tree), `index_stale = true`, `reload_tree()`, `reload_pages()`, reopen the
  remembered page at its new path, status line per Global Constraints. Errors go to the same error display
  `Dialog::RenamePage` errors use. The selected section does not change.
- Dialog state (in `move_ui.rs`): `pub struct MoveDialog { page: PathBuf, title: String, selected: Option<PathBuf> }`
  where `selected` is a target *dir* (a section folder, or `subpage_dir(target page)`).
  Tree: for every open root (`self.roots` / `self.trees`): folders (collapsing, current section opened),
  and under each folder its pages from `store::list_pages(folder, SortOrder::Name)` with their `level`
  indentation. Rows that fail `can_move_to` are drawn greyed and are not selectable. Buttons
  **Move** (enabled when `selected` is `Some`) and **Cancel**; Move → `Action::MovePage`.
  Load page lists lazily: only for folders whose header is open.

- [ ] **Step 1:** Add the actions, the menu items and `do_move`; open a `MoveDialog` from `ShowMove`.
- [ ] **Step 2:** Draw the dialog in `move_ui.rs`.
- [ ] **Step 3: Run** `cargo build` and `cargo test` - expected: no warnings, all tests pass.
- [ ] **Step 4:** Update `README.md`.
- [ ] **Step 5: Manual check** with `cargo run` on a copy of a notebook (e.g. copy
  `U:\NoteCraft\Primož` to a temp folder and add it via **Folders**). Expected:
  1. Move to… on a page → dialog; choose another section → page leaves the list, status line shows place.
  2. Move to… → choose a page → the moved page appears indented under it (open that section).
  3. Move a subpage to a section → it is a top-level page there.
  4. The page itself and its subpages are grey in the dialog.
  5. A page with a picture: picture still shows after the move.
  6. Start editing a page, type text, immediately Move to… elsewhere → text is there at the new place,
     the page is open, no lock file left at the old place.
- [ ] **Step 6: Commit** `feat(move): Move to… dialog and menu items`

### Task 4: Drag and drop

**Files:**
- Modify: `src/app.rs` - `list_row` (~line 2168) and `folder_label` (~line 971) senses, page list loop
  (~line 1094), drop handling.

**Interfaces:**
- Consumes: `do_move` (Task 3), `mover::can_move_to` (Task 2).
- Payload: `egui` drag-and-drop payload of type `DragPage(PathBuf)` (new small struct in `app.rs`).

- [ ] **Step 1:** Page rows: `Sense::click_and_drag()`; on drag start `resp.dnd_set_drag_payload(DragPage(path))`.
  A click without drag must still open the page (check `clicked()` stays true for a plain click).
- [ ] **Step 2:** Drop targets: every tree folder row (target dir = folder path) and every page row (target
  dir = `store::subpage_dir(row path)`). While `resp.dnd_hover_payload::<DragPage>()` is `Some` and
  `can_move_to(payload, target)` and target ≠ the page's current folder: draw `ACCENT_SOFT` fill plus a
  1 px `ACCENT` outline on the row. On `resp.dnd_release_payload::<DragPage>()` with a valid target:
  push `Action::MovePage`. Invalid targets: no highlight, drop ignored.
- [ ] **Step 3:** While dragging, show the dragged page's title next to the pointer
  (`egui::Area` at the pointer, `Order::Tooltip`) so the user sees what is being moved.
- [ ] **Step 4: Run** `cargo build` - no warnings; `cargo test` - all pass.
- [ ] **Step 5: Manual check:** drag a page onto a section in the tree, onto another page in the list,
  onto itself (nothing happens, no highlight), onto one of its own subpages (no highlight); a plain click
  still opens a page; scrolling the list still works.
- [ ] **Step 6: Commit** `feat(move): drag pages onto sections and pages`
