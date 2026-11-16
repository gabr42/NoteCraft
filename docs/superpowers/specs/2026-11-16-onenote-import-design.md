# Import from OneNote

Status: design, 2026-11-16 (decisions by the user on 2026-11-16 are marked "user").

## Intent

Today OneNote `.one` files are converted with the standalone Python 3 tool in
`tools\onenote-import` (`run_import.py`, `convert.py`, `onepar.py`, `cleanup.py`). It needs Python
on the computer and a command line. The wish: the import is part of NoteCraft, so anyone with
`NoteCraft.exe` can bring a OneNote notebook in through the UI.

## Goals

- **Folders ▸ Import from OneNote…** opens a wizard (user): choose the source folder → Next →
  choose the target folder → Next → progress, then a summary.
- The importer is ported to Rust inside NoteCraft (user). `NoteCraft.exe` stays a single file with
  no Python and no new runtime dependencies.
- The output matches what the Python tool produces today, page for page, except for the deliberate
  changes listed below.
- Re-importing into the same place is safe: a page that already exists is overwritten, and its old
  text is kept as a history version first (user).

## Non-goals

- No OneNote 2007 (`.one` of the older format), no `.onepkg` packages, no OneNote online/cloud.
- No mapping of OneNote's page levels to NoteCraft subpages. The Python tool writes every page of a
  section as a flat list, and so does the port (the page level is parsed but unused).
- Pages that disappeared from OneNote since an earlier import are not deleted from the target.
- No colour, font or ink import (the Python tool does not do it either).
- The Python tool is not changed by this work. It stays in `tools\` as the reference
  implementation until the port is verified (removing it is a later, separate decision).

## Anti-goals

- An import must never lose text that was in the target folder: overwritten pages always get a
  history version, even when the normal "10 minutes / big deletion" rule would skip it.
- An import must never touch the source `.one` files (read only).
- The window must not freeze during an import.

## Deliberate changes from the Python tool (user, 2026-11-16)

1. **Time zone.** Python converts the UTC times with a hard-coded Central European rule. The port
   uses the computer's own time zone (`chrono::Local`).
2. **Untitled pages.** Python names the file `Untitled page` but writes the title
   `Neimenovana stran`. The port uses `Untitled page` for both.
3. **Leftover `<span>` tags.** `convert.py` imports `strip_spans` from `cleanup.py` but never calls
   it, so span tags stay in the output although the README says they are removed. The port applies
   `strip_spans` to every page body.
4. **Pictures and attachments next to the pages** (Claude's call, see below). Python writes them to
   one `<target>\resources\` folder. NoteCraft's convention (see
   `2026-11-04-note-containers-design.md` and pasted pictures) is a `resources` folder next to the
   page. The port writes each section's pictures and attachments to `<section folder>\resources\`
   and links them as `resources/<name>`.
5. **Underline, superscript, subscript** (user, 2026-11-16). Python parses but ignores them. The
   port writes them as `<u>…</u>`, `<sup>…</sup>`, `<sub>…</sub>` around the run, nested inside the
   Markdown bold/italic/strikethrough marks, the way the page editor writes them (`editor.js`,
   `inlineNode`). Underline is not written on links (they show underlined anyway). Code runs
   (monospace) keep only the code marks, as now.

## Approaches considered

1. **Port to Rust** (chosen by the user). Single exe, works on every PC. Most work; the binary
   parser has to be ported exactly and checked against the Python output.
2. **NoteCraft runs the Python scripts.** Little work, but works only where Python 3 is installed
   and the scripts must be shipped or found. Rejected: breaks the "copy one exe" model.

## User interface

The wizard is one egui window (modal, like the existing dialogs), with three steps:

```
┌ Import from OneNote ──────────────────────────────┐
│ Step 1 of 3 – OneNote notebook                     │
│ Folder with the notebook's .one files:             │
│ [E:\OneNote\Work                    ] [Browse…]    │
│ Found 14 sections in 3 folders.                    │
│                                 [Cancel] [Next >]  │
└────────────────────────────────────────────────────┘
```

- **Step 1 – source.** A path box and Browse… (`rfd` folder picker). Below it, a live count of the
  `.one` files found (recursively, skipping `OneNote_RecycleBin`). Next is enabled only when at
  least one `.one` file is found; otherwise a short reason is shown ("No .one files in this
  folder.").
- **Step 2 – target.** A path box and Browse…. The notebook will be written to
  `<target>\<source folder name>\`; the window shows that full path. When Next is pressed and that
  notebook folder (`<target>\<source folder name>\`, not the chosen parent; user left the choice to
  Claude, 2026-11-16) exists and is not empty, NoteCraft asks once (user): "<folder> is not empty.
  Pages that already exist will be replaced; their current text is kept in their history.
  Continue?" (Yes / No). No returns to step 2. < Back returns to step 1.
- **Step 3 – progress.** A progress bar over sections ("Section 5 of 14: Work › Projects"), and a
  Cancel button. Cancel stops after the section being written; what was written stays. When done,
  the step shows a summary: sections, pages written, pages replaced, pictures/attachments written,
  and a list of problems (sections that could not be read, with the error; pages skipped because
  they are locked; missing picture data). Buttons: Close.
- After a successful (or cancelled) import the notebook folder (`<target>\<notebook>`, not the
  chosen target; user, 2026-11-16) is added to NoteCraft's folder list,
  as if added with Folders ▸ Add folder…, and shown. If it cannot be added (it lies
  inside or contains an already added folder, see `store::check_new_root`), the tree is reloaded
  instead and the new notebook is selected if it is visible. The search index is rebuilt.
- The wizard remembers the last source and target folders in the settings (Claude's call).

## Behaviour of the import

- Source layout: the chosen folder is one notebook. Each `.one` file is a section; subfolders are
  section groups. `OneNote_RecycleBin` is skipped. Files are processed in sorted order per folder,
  as in `run_import.py`.
- Output: `<target>\<notebook>\<group path>\<section>\<page>.md`, file and section names cleaned as
  in `run_import.py` (`safe`), with `^J` / newlines in section names turned into spaces. Duplicate
  page titles in a section get `-1`, `-2`, … (case-insensitive), as in Python.
- Page text: identical to `page_markdown` in `convert.py` (front matter with quoted title when
  needed, `updated` then `created`, blocks separated by `---`), with the changes listed above.
- Pictures/attachments: content-addressed names (hash of the bytes + extension) in
  `<section folder>\resources\`, written only when the file does not exist yet. Python uses MD5;
  the port may use any stable hash (builder's choice; a small crate such as `md-5` is fine).
- Overwriting: before replacing an existing page whose text differs from the new text,
  `history::snapshot_now` keeps the old file as a version. If the snapshot fails, the page is not
  replaced and the problem is listed in the summary (stricter than a normal save, because there is
  no user in the loop). A page whose text is unchanged is not rewritten.
- Locks: a page that is locked by another computer (`lock.rs`) is skipped and listed. The page open
  in NoteCraft itself: if it has unsaved edits, the import saves it first (as on any page switch);
  after the import the open page is reloaded from disk.
- A section that fails to parse is skipped with its error; the import goes on with the next one
  (as in Python).

## Structure of the code (builder may adjust)

| Path                      | Contents                                                       |
|---------------------------|----------------------------------------------------------------|
| `src\onenote\mod.rs`      | Public entry: `import(src, target, progress, cancel) -> Report`|
| `src\onenote\parse.rs`    | Port of `onepar.py`: file node lists, object spaces, revisions, property sets, file data |
| `src\onenote\convert.rs`  | Port of `convert.py` + `strip_spans` from `cleanup.py`         |
| `src\app\import_ui.rs`    | The wizard window                                              |

The import runs on a `std::thread`, reporting progress over an `mpsc` channel and stopping on an
`AtomicBool`, like `start_indexing` and `history::search_history`. The parser works on a byte
slice; every read is bounds-checked and a malformed file returns an error instead of panicking
(Python relied on exceptions for this).

## Points the port must keep exact

These are subtle in the Python code and easy to get wrong:

- Revision resolution in `Space.finalize`: per object the copy with the latest LastModifiedTime
  wins; revisions are applied freshest-last; ties keep file order (stable sort); roots come from the
  freshest revision that has them.
- Global id tables (`0x21`, `0x22`, `0x24`–`0x26`), the saved-table stack for object groups
  (`0xB4`/`0xB8`), and the dependency revision's table.
- Property value types including nested property sets (`0x11`) and arrays (`0x10`), and the
  running indexes into the oid / osid / context id streams.
- Text: UTF-16 text, or the ASCII text with the code page from the charset property; run splitting;
  hyperlinks marked with U+FDDF; U+2028 and U+000B as line breaks; monospace fonts become code.
- Escaping (`esc`, `esc_plain`), bare URLs to `<url>` autolinks with trailing punctuation moved out.
- Lists: numbered vs bulleted detection, counters per depth, to-do state from note tags.
- Element order on a page: by vertical then horizontal offset rounded to 0.1.
- Title: page metadata wins over the page's cached title, which wins over the title outline text.

## Testing and verification

- Unit tests for the pure parts: `esc`, autolinks, `strip_spans`, title quoting in the front
  matter, `safe` names, duplicate names, list markers, time conversion.
- Parser tests on small synthetic byte buffers for the primitives (file chunk references, property
  sets, global id tables).
- **Comparison with Python on real notebooks** (the main proof): run `run_import.py` and the Rust
  import on the same OneNote folders and diff the trees. Expected differences are only the four
  deliberate changes (times, untitled title, spans, resources location/link path, underline/sup/
  sub). A small test helper (`--import-onenote <src> <target>` command-line option, like the
  existing development options) makes this repeatable without clicking through the wizard. Sample
  notebook (user, 2026-11-16): `u:\OneNote\Primož` (19 `.one` files, 17 MB, with section groups
  and a `OneNote_RecycleBin`); Python 3 is installed on the PC. Both imports go to scratch folders,
  never into the source. No `.one` files are committed to the repository.
- Manual check of the wizard: empty / non-empty target confirmation, Back, Cancel during import,
  re-import producing history versions, locked page skipped.

## Calls made after the design review (Claude's calls, 2026-11-16)

- **Code pages.** The ASCII text's charset (0, 238, 204, 161, 162, 186 → cp1252, cp1250, cp1251,
  cp1253, cp1254, cp1257) is decoded with the `encoding_rs` crate; an unknown charset falls back to
  cp1252, as in Python. (A pure Rust crate compiled into the exe, so "no new runtime dependencies"
  still holds; so is `md-5` if chosen.)
- **Name collisions of sections and groups.** Python silently merges two sections or groups whose
  cleaned names are equal. The port gives the later one a `-1`, `-2`, … suffix (case-insensitive,
  per folder, sections and groups share one name space). A section or group whose cleaned name is
  `resources` or starts with `.` gets the suffix too, because NoteCraft hides those folders.
- **Page identity on re-import** stays by file name, as in Python. A page renamed in OneNote
  arrives as a new file; the old one stays. If numbering of equal titles shifts, a page may replace
  another page of the same title; its text is kept in history. Stable OneNote ids are not stored
  (they would change the output format).
- **Content Python does not handle** (ink, audio, other unknown elements) is dropped, as in Python.
  Encrypted or unreadable sections fail to parse and are listed in the summary with the error.
- **Missing picture or attachment data** behaves as in Python (picture: nothing written;
  attachment: `` `name` (attachment data missing) ``), and each case is listed in the summary with
  the page title.
- **`--import-onenote <src> <target>`** runs the same import code as the wizard (history versions,
  lock checks) without any window or confirmation, prints the summary to stdout and exits with 0
  when every section was imported, 1 otherwise. The comparison with Python is run on the user's PC
  (Central European time zone), so the time conversion differs from Python's only around DST
  transitions outside the EU rule, i.e. not at all there; no extra flag.

## Left to the builder

- Exact widget layout and wording of the wizard, within the steps above.
- Hash function for resource names.
- How progress is reported (per section is enough; per page if cheap).
- Internal data structures of the parser.

## Documentation

- README: replace the "Importing from OneNote" section with the wizard; mention that
  `tools\onenote-import` is the reference implementation.
- `tools\onenote-import\README.md`: note that NoteCraft has the import built in.
