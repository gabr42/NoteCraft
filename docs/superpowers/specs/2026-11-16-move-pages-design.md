# Moving pages - design

Date: 2026-11-16. Branch: `feat/move-pages`.

## Why

After the OneNote import the user wants to reorganise notes: put a page into another section, or
fix the nesting (make a page a subpage of another page, or lift a subpage back to the top of a
section). Today a page can only be created, renamed and deleted where it is.

## What the user gets

1. **Drag and drop.** Drag a page from the page list and drop it
   - on a section in the tree: the page goes to the top level of that section;
   - on another page in the page list: the page becomes a subpage of that page.
2. **"Move to…"** in the page's right-click menu (page list) and in the page's `⋯` menu. It opens a
   dialog with a tree of all open notebooks: sections, and the pages inside them (with subpages,
   indented). Picking a section means "top level of that section"; picking a page means "as a
   subpage of that page". Buttons: **Move** and **Cancel**.

The user will test whether this behaviour is obvious enough; wording and visuals may change after that.

## Goals

- Move a page with everything that belongs to it: its subpages, its saved versions, and the
  pictures/attachments it links to.
- Work across sections, across levels (page <-> subpage) and across open notebooks.
- Never lose a picture another page still needs.

## Non-goals

- Moving several pages at once.
- Moving sections (folders) - only pages.
- Reordering pages inside a section (the list is sorted by name or date).
- Undo of a move (moving back is the undo).
- Dragging into or out of the page view (the web view).

## Anti-goals

- A move that breaks a picture link, in the moved page or in any page left behind.
- A move that silently overwrites another page or another picture.
- A move of a page someone else is editing.

## How pages are stored (relevant facts)

- A page is `<dir>\<Name>.md`. Its subpages live in `<dir>\<Name>\` (`store::subpage_dir`).
- Saved versions: `<dir>\.history\pages\<Name>\...` (`history::versions_dir`). Deleted items:
  `<dir>\.history\deleted\...`.
- Pictures and attachments: links `resources/<file>` are relative to the page's own folder, files in
  `<dir>\resources\`. Pasted pictures are saved there (`app.rs`, paste handler). Imported files are
  named by the MD5 of their content, one copy per section, so several pages may link the same file.
- Edit locks: `.Name.md.notecraft-lock` next to the page; `page_free()` in `app.rs` refuses an
  operation when another computer edits the page or one of its subpages.
- Rename (`store::rename_page`) already shows how subpages and versions follow a page.

## Approaches considered (pictures)

- **A. Copy the pictures, leave the originals.** Safe and simple, but leaves unused files behind.
- **B. Move the pictures; remove an original only when nothing left behind links to it.** No unused
  files; needs a scan of the source folder on every move. **Chosen by the user.**
- **C. One shared picture folder per notebook.** Changes every link and needs a migration. Rejected
  as too big for this feature.

## Design

### Core operation: `store::move_page(page, target_dir) -> Result<PathBuf, String>`

`target_dir` is either a section folder (top level) or `subpage_dir(target_page)` (as a subpage).

1. **Checks** (refuse with a message, change nothing):
   - `target_dir` equals the page's folder: nothing to do (not an error; the UI does nothing).
   - `target_dir` is the page's own subpage folder or below it (a page cannot go under itself).
   - `page_free(page)` fails (someone else edits the page or a subpage).
2. **New name.** Like `create_page`, a clash gives `Name-1.md`; the stem must be free both as
   `Name.md` and as a folder `Name\` in the target (an existing folder would otherwise be taken
   for the page's subpages). The title in the front matter stays unchanged. `target_dir` is
   created if it does not exist yet (first subpage of a page).
   The target page itself is not checked with `page_free`: the move only adds files to its
   subpage folder and does not touch the target page file.
3. **Pictures** (only the moved page's own links; see below why subpages need nothing):
   - Collect `resources/<file>` links from the page and from all its saved versions.
   - For each linked file that exists in `<source dir>\resources\`:
     - target has no file with that name: copy it to `<target dir>\resources\`;
     - target has a file with that name and identical content: nothing to copy;
     - target has a different file with that name: copy under a free name (`name-1.ext`) and
       rewrite the links in the page and its versions to the new name.
   - Links whose file does not exist are left as they are.
4. **Move** the page file, its subpage folder and its versions folder
   (`history::rename_versions` already handles a versions folder that moves to another folder).
   If a later one of these three fails, the earlier ones are moved back (best effort) and the error
   is reported; the picture originals are then not deleted.
5. **Clean up pictures in the source** (option B): for each picture copied in step 3, delete the
   original only if no file left in the source folder still links it. "Left in the source folder"
   means: every `*.md` directly in the source dir, and every file under `<source dir>\.history\`
   (versions of the other pages and deleted pages, which can be restored). A file that cannot be
   read counts as "links it" (keep the picture). The scan runs after the move, so the moved page
   is no longer in it.
6. Returns the new page path.

Order matters for safety: pictures are copied before anything is moved, originals are deleted last,
and only after the move succeeded. A failure before step 4 leaves the source untouched (the extra
copies in the target are harmless); a failure in step 5 only leaves unused files.

**Subpages need no picture handling**: they live in the subpage folder with their own
`resources\` and `.history\` (versions of a subpage are kept in the `.history\` of the subpage
folder, since `versions_dir` uses the page's own folder), and the whole folder moves, so their
relative links stay valid. Links that leave the page's folder (`../resources/...`) are written
neither by the importer nor by the paste handler and are not handled.

**Links between pages**: NoteCraft has no page-to-page links, so only picture/attachment links
need care.

**Different drives** (notebooks on different volumes): `fs::rename` fails across volumes; fall back
to copy + delete for files and folders.

### UI

- `Action::MovePage { page, target_dir }` does the move like `Dialog::RenamePage` handles a rename.
  If the moved page (or one of its subpages) is open: `save_if_dirty()` first, then drop the open
  page (`self.page = None`, which releases this computer's edit lock) before moving, and open it
  again at the new path afterwards. Then update the search index (one `Index` covers all open
  notebooks: remove old page + subpage tree, add new), mark the index stale, reload
  tree and page list, reopen the page at its new path if it was open, status line
  `Moved "<title>" to <place>` (place as in search results: `Notebook › Section › Page`).
  Errors go to the usual error display.
- The view stays on the source section, so several pages can be moved one after the other.
- **Drag and drop** with egui's drag-and-drop payloads (`dnd_set_drag_payload` /
  `dnd_release_payload`, egui 0.37): page rows in the list are drag sources carrying the page
  path; every folder row in the tree (sections, section groups, notebooks) and page rows in the
  list are drop targets. A valid target is
  highlighted while hovered; an invalid one (the page itself, its own subpages, its current
  folder) shows no highlight and ignores the drop. A plain click on a row must still select it.
- **"Move to…" dialog**: a tree of all open notebooks → sections (nested section groups) → pages →
  subpages. Collapsible, opens with the current section expanded. The page being moved and its
  subpages are greyed out and not selectable. **Move** is enabled when a valid target is selected.

## Tests

- `store::move_page`: to another section, to a subpage of another page, a subpage back to the top
  level, name clash (`-1`), subpages and versions follow, refused under itself.
- Pictures: copied; identical file in target not duplicated; different file with the same name gets
  a new name and links are rewritten (page and versions); original removed when unused; original
  kept when another page, another page's version or a deleted page links it.
- The UI is checked by the user by hand (drag and drop and the dialog).

## Calls made without the user (check these)

- An open page that is moved is saved first and reopened at its new place.
- Dropping on a page another computer is editing is allowed (its file is not touched).
- A failure in the middle of the move is rolled back as far as possible.
- Any folder in the tree is a drop target, including notebooks and section groups.
- After a move the view stays on the source section.

## Left to the builder

- Exact look of the drop highlight and the dialog layout.
- Where the move code lives (`store.rs` or a new `move_page.rs`), helper names.
- Link detection: a regex over `](resources/<file>)` like the importer writes and the paste handler
  inserts; HTML `<img src="resources/...">` should be covered as well.
