# Several note folders open at once

Status: design approved 2026-11-04. Supersedes the earlier "container detection" idea (dropped: the
global `resources` folder of the OneNote exporter turned out to be an artifact; the three databases
were restructured so every picture lives in a `resources` folder next to its page).

## Intent

NoteCraft opens one folder; each of its subfolders is a top-level node. The wish: open several
folders (`A`, `B`, ...) at once. Each added folder becomes a top-level node named after the folder;
its tree is populated from its subfolders as today. No detection, no special folder names.

## Model

- The app holds an ordered list of folders (order of adding) instead of one root.
- Every folder is a top-level node, even when only one is open (changed from before, when the
  single root was not shown and its children were top-level).
- Added 2026-11-04: the left column shows only one top folder at a time. A combo box at its top
  (full path on hover) picks the folder; below it the tree shows that folder's subfolders.
  Choosing a folder selects its first subfolder that has pages.
- Pages lying directly in an added folder show when its node is clicked (replaces "(top folder)").
- A top-level node cannot be renamed or deleted from the app.

## Behaviour

- Search: one index over all folders; a hit's place text starts with the top-level node name
  (`Work › Archive`).
- Adding a folder that is already present, or that lies inside / contains another added folder, is
  refused with a status message.
- A folder missing at startup is skipped with a status note.
- "+ Section" with nothing selected uses the first folder.
- Settings keep a list of folders; an old single `root=` line loads as a list of one.
- The command line accepts several folder arguments.
- Top bar: the `📁 name` button becomes a `📁 Folders` menu: "Add folder…" and
  "Remove <name> from view" per folder ("Open only this folder…" was dropped on 2026-11-04). A top-level node's context menu also has
  "Remove from NoteCraft". Removing only hides it; files on disk are never touched.
- Edit locks, sort order, printing, page links and the pasted-image convention (a `resources`
  folder next to the page) are unchanged.

## Tests

Unit tests in `store.rs`: multi-folder tree, search place text, refusing duplicate / nested folders.

## Page list speed (2026-11-04)

Listing a folder read every page file to get title and dates; on a network drive that cost about
5 ms per file (888 pages: about 5 s). Measured: parallel reads are 2-30x faster than sequential.
`list_pages` now takes size and time from the directory listing, serves unchanged pages from an
in-memory cache of titles and dates (keyed by path, size and time), and reads the rest on 8 threads.
The 888-page section lists in about 0.26 s. Not done: background loading, list virtualization, and
running `scan_tree` off the UI thread (about 0.2 s for the largest folder).
