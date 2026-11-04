# History: recovering from mistakes (design)

Date: 2026-11-04. Status: implemented.

## Why

The user is afraid of three mistakes and has not made any of them yet:

1. deleting part of a note and saving,
2. deleting a note,
3. deleting a whole section or tree.

A mistake may be noticed days or weeks later (when searching for something), so history is kept
indefinitely. Disk space is not a concern. `U:` is a RAID that is backed up only occasionally, so
the backup is not a substitute. The user wants a safety net that works without being managed.

Today deleting goes through the `trash` crate (Windows Recycle Bin). On a network drive such as
`U:` Windows has no Recycle Bin, so deletes there are probably permanent already (not tested).

## Goals

- Every overwrite of a page keeps the earlier text, so an edit can be undone weeks later.
- Deleting a page, section or tree is never final: it can be restored with its content.
- Restoring is possible from inside NoteCraft, with a preview for page versions.
- No setup and no day-to-day management; nothing is removed automatically.

## Non-goals

- Changes made to the files outside NoteCraft are not captured (only NoteCraft's own saves).
- No protection against disk failure (that is the RAID backup's job).
- No version history for pictures in `resources`.
- No merging or diffing of versions; a version is restored whole.
- No history for top folders themselves: "Remove from NoteCraft" only hides them, files stay.

## Where history lives

A hidden folder `.history` inside every folder that needs one. Names starting with a dot are
already skipped by the tree scan, so the tree and page lists need no change. The search index
must skip it as well (to verify, and to cover with a test).

```
<folder>\
  Page.md
  Page\                      subpages of Page
  resources\                 pictures shared by the pages of this folder
  .history\
    pages\Page\2026-11-04T110500.md      older versions of Page.md
    deleted\2026-11-04T111200\Old page.md    things deleted from this folder
    deleted\2026-11-04T111500\Old section\   (a whole folder, with its resources)
```

Rule: history never lives inside the thing being deleted. A deleted folder moves into its
*parent's* `.history\deleted`, so its own `.history` travels with it.

## Behaviour

### Versions of a page

- Before `save_body` overwrites a page, it copies the file as it is on disk to
  `.history\pages\<page name>\<local time, yyyy-MM-ddTHHmmss>.md` when all of this holds:
  the text differs from the newest version, and either there is no version yet, the newest
  version is older than 10 minutes, or the new body is shorter than the old one by more than
  200 characters or by more than 20 %. (Time rule and size rule are the designer's calls; they keep
  a long editing session from making hundreds of versions while a large deletion is always kept.)
- Renaming a page renames its `.history\pages\<name>` folder with it. Moving a page to another
  folder (if the app has such an action) moves its versions along.
- Page menu (`...`) gets **History...**: a dialog listing versions, newest first, by date, size and
  first lines; selecting one shows a preview of its text. **Restore** first snapshots the current
  page (always, ignoring the rules above), then writes the chosen version over the page. A
  restore can therefore be undone from the same list.

### Deleting

- Delete page / Delete section move the item with `fs::rename` into
  `<parent>\.history\deleted\<local time>\`. A page takes its subpages folder along. A page's
  `resources` are not moved (the folder is shared by the other pages). The Recycle Bin is no longer
  used and the `trash` dependency is removed.
- If the move fails (locked file, no rights) nothing is deleted and the error is shown with the OS
  error text.
- The delete confirmation text says the item can be restored from "Deleted items".
- **Deleted items...** (Folders menu) lists everything under `.history\deleted` in the active top
  folder: name, kind (page / section), original folder, deletion time. The list is built by walking
  the tree on demand in a background thread. **Restore** moves the item back to its original
  folder, recreating that folder if needed; when the name is taken it gets a numbered name.
  **Delete forever** removes the item from the history.

### Purging (security)

- History dialog: **Delete all versions** of that page.
- Folders menu: **Purge history...** removes every `.history` folder under the active top folder
  after a confirmation that says it cannot be undone.

## Constraints

- Works on a network drive: only `fs::copy` and `fs::rename` inside one volume.
- Failing to write a snapshot must not block saving: the save goes ahead and the status bar says
  the history copy failed (with the OS error code and text).
- Edit locks are unaffected (they live in the temp folder).
- Exception and status texts name the routine, as elsewhere in the code.

## Details settled after the builder check

All of these are the designer's calls unless stated otherwise.

- **Versions follow the page.** Deleting a page also moves its `.history\pages\<name>` folder into
  the deleted entry (as `.versions`); restoring moves it back under the name the page gets. A new
  page with the same name therefore starts with no history. Same for a deleted section: the
  versions of its pages are inside it already.
- **Compare body only.** "Text differs" and the 200 character / 20 % rule compare the new body with
  the body of the file on disk, in characters. The front matter (which `save_body` always updates)
  is ignored.
- **Restore a version** writes the version's body into the current page the way `save_body` does:
  current title kept, `updated` set to now. It goes through the same checks as a normal save (edit
  lock held by another computer, page changed on disk). The button is disabled while the page is
  being edited with unsaved text. After a restore the page is reloaded and its stamp refreshed.
- **Deleted items shows each deletion once.** The walk does not descend into
  `.history\deleted\<time>\...`, so deletions that happened inside an already deleted section are
  not listed; restore the section first, then look in it. Kind: a `.md` file in the time folder is a
  page, a folder without one is a section. Restore recreates the original folder only if it is
  gone and not itself in history.
- **Delete is best effort atomic.** Two deletes in the same second get a numeric suffix on the time
  folder. If the second move of a page delete (its subpages) fails, the first move is moved back
  and the error is shown.
- **Deleted items dialog** has a name filter and, for pages, a text preview.

## Searching the history (decided by the user, 2026-11-04)

- Off by default. The search box gets a checkbox **Include history**; normal search is unchanged
  and stays fast.
- When it is on, the search also looks through every old version and every deleted page under
  all open top folders, using the same word matching as normal search. There is no persistent
  index of history: the files are read at search time, on background threads (about 8 reads in
  parallel, as in the page lists), and the result list fills in as hits arrive. It may be slow on
  a big history; that is accepted.
- History hits are marked in the result list ("old version, 3 Nov 2026" / "deleted page, 3 Nov
  2026") and show the page's place and title as it was. For a page with several matching versions
  only the newest matching one is shown, with a count of the others.
- Opening a history hit shows its text read-only, with **Restore** (a version goes back into its
  page; a deleted page goes back to its folder, same rules as in the two dialogs above). The
  normal search hits keep working as before.
- Text inside deleted sections is searched as well (their pages and their own versions).

## Left to the builder

Dialog layout, wording, the exact time format of version names (it must sort by time and be a valid
Windows file name), how the walk for "Deleted items" reports progress, and unit tests: snapshot
rules (time, size, unchanged text), delete and restore of a page and of a tree, name clashes on
restore, search and tree ignoring `.history`.

## Open decisions made by the designer (not by the user)

- 10 minutes / 200 characters / 20 % as the snapshot rules.
- Removing the Recycle Bin and the `trash` crate.
- Moving the versions along with a moved page.
- Deleted items are listed per top folder, not for all open folders at once.
