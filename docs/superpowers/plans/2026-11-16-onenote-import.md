# Import from OneNote Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the Python OneNote importer (`tools\onenote-import`) into NoteCraft and expose it as a
three-step wizard (Folders ▸ Import from OneNote…) plus a `--import-onenote` command-line option.

**Architecture:** A new module `src\onenote\` with three layers: `parse.rs` (port of `onepar.py`,
bytes → object spaces), `convert.rs` (port of `convert.py` + `cleanup.strip_spans`, object spaces →
page Markdown) and `mod.rs` (port of `run_import.py`: walks the source, writes pages and resources,
keeps history, honours locks, reports progress). The wizard `src\app\import_ui.rs` runs `mod.rs` on a
thread and shows progress from an `mpsc` channel.

**Tech Stack:** Rust 2024, eframe/egui 0.37, chrono, `encoding_rs` (new), `md-5` (new), `rfd`.

**Spec:** `docs\superpowers\specs\2026-11-16-onenote-import-design.md`. The Python sources in
`tools\onenote-import` are the reference for every behaviour not listed as a deliberate change;
port them line by line and keep the same order of operations.

## Global Constraints

- `NoteCraft.exe` stays a single file: only pure-Rust crates compiled in; no Python, no DLLs.
- Never write into or below the source folder; open `.one` files read-only.
- An existing page is replaced only after `history::snapshot_now` succeeded for it; unchanged text
  is not rewritten.
- Untitled pages: file name **and** title `Untitled page`.
- Times: UTC from the file → `chrono::Local`, formatted `%Y-%m-%dT%H:%M:%S`.
- Every page body goes through `strip_spans`.
- Underline/superscript/subscript: `<u>…</u>`, `<sup>…</sup>`, `<sub>…</sub>` inside the
  `**`/`*`/`~~` marks; no `<u>` on links; code runs only get backticks.
- Pictures and attachments: `<section folder>\resources\<md5 hex><ext>`, linked as
  `resources/<md5 hex><ext>`; written only when the file does not exist.
- Section/group name collisions (case-insensitive, per folder, one name space for sections and
  groups) and names equal to `resources` or starting with `.` get `-1`, `-2`, …
- The confirmation is asked once, only when `<target>\<notebook>` exists and is not empty.
- The parser never panics on any input: all reads are bounds-checked and return `Err`.
- Code style follows the existing sources (doc comments in plain English, `Result<_, String>` errors
  with readable messages, tests in `#[cfg(test)] mod tests` at the end of each file).

## Review Focus

1. A damaged, truncated or encrypted `.one` file → that section is listed with an error, the rest of
   the notebook imports, nothing panics (Task 2 test `garbage_is_an_error_not_a_panic`).
2. The page open in NoteCraft has unsaved edits when the import starts → it is saved first and
   reloaded from disk afterwards (Task 6 step "save before start").
3. A target that is the source folder or lies inside it → Next is disabled in step 2 with a reason
   (Task 5 test `target_inside_source_is_refused`).
4. Cancel during the import → stops after the current section, the summary says "Cancelled", the
   written pages stay and the folder is still added (Task 5 test `cancel_stops_after_section`).
5. Re-import of an unchanged notebook → zero pages rewritten, zero new history versions (Task 5
   test `reimport_unchanged_writes_nothing`).

---

### Task 1: Parser primitives

**Files:**
- Create: `src\onenote\mod.rs` (only `pub mod parse; pub mod convert;` for now — `convert` may be an
  empty file until Task 3)
- Create: `src\onenote\parse.rs`
- Modify: `src\main.rs` (add `mod onenote;`)
- Modify: `Cargo.toml` (add `encoding_rs = "0.8"`, `md-5 = "0.10"`)

**Interfaces:**
- Produces (in `parse.rs`):
  - `struct Rd<'a> { d: &'a [u8] }` with `u8_at/u16_at/u32_at/u64_at(pos) -> Result<_, String>`,
    `bytes(pos, len) -> Result<&'a [u8], String>`, `guid_at(pos) -> Result<Guid, String>`.
  - `type Guid = [u8; 16]` (raw bytes as in the file, little-endian fields); `fn guid_string(g: &Guid) -> String`
    giving the lowercase `xxxxxxxx-xxxx-…` form that `uuid.UUID(bytes_le=…)` prints.
  - `type ExGuid = (Guid, u32)`; `type ObjId = ExGuid`.
  - `enum PropValue { NoData, Bool(bool), Bytes(Vec<u8>), Ids(Vec<Option<ObjId>>), Array(Vec<PropValue>), Set(PropSet) }`
    (types 1, 2, 3–7, 8/9/A/B/C/D, 0x10, 0x11 as in `read_propset.value`).
  - `struct PropSet(HashMap<u32, PropValue>)` keyed by `prid & 0x7FFF_FFFF`, with
    `get_bytes(id) -> Option<&[u8]>`, `get_bool(id) -> bool`, `get_ids(id) -> &[Option<ObjId>]`,
    `get_array(id) -> &[PropValue]`.
  - `fn read_propset(r: &Rd, pos: usize, cid: &dyn Fn(u32) -> Option<ObjId>) -> Result<PropSet, String>`.
  - `fn fcr(r: &Rd, pos: usize, stp_f: u32, cb_f: u32) -> Result<(u64, u64, usize), String>`.
  - `fn filetime(b: &[u8]) -> Option<NaiveDateTime>`, `fn time32(b: &[u8]) -> Option<NaiveDateTime>` (UTC).
  - Property and JCID constants with the Python names in Rust form (`P_PAGE_LEVEL`, … `J_PAGEMANIFEST`).

- [ ] **Step 1: Write failing tests** in `parse.rs`:
  - `fcr_formats`: for each `(stp_f, cb_f)` in `{0,1,2,3}²` on a hand-built buffer, the decoded
    `(stp, cb, next_pos)` equals the Python rule (formats 2/3 multiply by 8).
  - `propset_types`: a buffer with one property each of types 1, 2 (bit 31 set → `true`), 3, 5, 7
    (`cb=3`), 8, 9 (`cnt=2`) and 0x10 (array of two type-5 values) and 0x11 (nested set with one
    type-3 value) decodes to the expected `PropValue`s; ids are taken from the oid stream in order.
  - `times`: `time32(&[0,0,0,0]) == None`; `time32(&60u32.to_le_bytes()) == 1980-01-01T00:01:00`;
    `filetime(&116444736000000000u64.to_le_bytes()) == 1970-01-01T00:00:00`.
  - `guid_text`: bytes `E4 52 5C 7B 8C D8 A7 4D AE B1 53 78 D0 29 96 D3` → `"7b5c52e4-d88c-4da7-aeb1-5378d02996d3"`.
  - `short_buffer_is_err`: every reader on a 3-byte buffer past its end returns `Err`.
- [ ] **Step 2:** `cargo test onenote::parse` → FAIL (does not compile / missing items).
- [ ] **Step 3:** Implement the items above, porting `fcr`, `read_propset`, `u16s`, `filetime`,
  `time32` from `onepar.py`. `u16s(b) -> String` decodes UTF-16LE lossily and trims trailing `\0`.
- [ ] **Step 4:** `cargo test onenote::parse` → PASS.
- [ ] **Step 5:** Commit `feat(onenote): parser primitives`.

### Task 2: Parser: file nodes, object spaces, revisions

**Files:**
- Modify: `src\onenote\parse.rs`

**Interfaces:**
- Consumes: Task 1 items.
- Produces:
  - `struct Obj { pub jcid: u32, pub props: PropSet }`
  - `struct Space { pub objects: HashMap<ObjId, Obj>, pub filedecl: HashMap<ObjId, (String, String)>, pub roots: HashMap<u32, ObjId> }`
  - `struct OneFile { pub spaces: HashMap<ExGuid, Space>, pub root_gosid: Option<ExGuid>, pub filedata: HashMap<String, Vec<u8>> }`
    (`filedata` key = lowercase GUID text without braces)
  - `fn OneFile::parse(data: &[u8]) -> Result<OneFile, String>` — error text for a wrong header:
    `"not a .one section file"`.
  - `fn finalize(revs: Vec<RevData>) -> Space` where `RevData { objects, filedecl, roots, ctx_nil }`
    in file order (separate function so it can be tested without a file).

- [ ] **Step 1: Write failing tests:**
  - `finalize_latest_stamp_wins`: two revisions (file order A, B) holding the same oid; A's copy has
    `P_MOD32` = 200, B's = 100 → the result holds A's copy. A stamp-less object present in both →
    copy from the revision with the fresher maximum stamp (A). Roots: taken from A (freshest that
    has roots).
  - `finalize_ties_keep_file_order`: equal freshness → later file-order revision applied last wins
    for equal stamps (`>=` rule).
  - `finalize_ignores_non_nil_context`: a revision with `ctx_nil = false` is not used.
  - `garbage_is_an_error_not_a_panic`: `OneFile::parse` on empty input, 200 zero bytes, a valid
    16-byte header followed by `0xFF` × 300, and every prefix length of a 600-byte pseudo-random
    buffer (fixed LCG seed) returns without panicking; the first three return `Err`.
  - `real_sample` (`#[ignore]`): reads the path in env `ONENOTE_SAMPLE` (a `.one` file), parses it,
    asserts `root_gosid.is_some()` and that the root space has a root object with role 1.
- [ ] **Step 2:** `cargo test onenote::parse` → new tests FAIL.
- [ ] **Step 3:** Port `OneFile.read_list`, `node`, `dep_rev_table`, `Space.finalize` from
  `onepar.py`. The global id table, the saved-table stack and `dep_table` are parser state fields.
  Guard against cycles in node lists (a visited set of `(stp, cb)`) and against `fid`/size values
  that run past the buffer. Per-object propset errors are stored as an object with an empty
  `PropSet` (Python stores `_error`; nothing downstream reads it).
- [ ] **Step 4:** `cargo test onenote::parse` → PASS; then
  `$env:ONENOTE_SAMPLE='u:\OneNote\Primož\Blog.one'; cargo test real_sample -- --ignored` → PASS.
- [ ] **Step 5:** Commit `feat(onenote): parse .one object spaces and revisions`.

### Task 3: Text helpers

**Files:**
- Modify: `src\onenote\convert.rs`

**Interfaces:**
- Produces:
  - `fn esc_plain(t: &str) -> String`, `fn esc(t: &str) -> String` (autolinks), as in `convert.py`.
  - `fn strip_spans(text: &str) -> String`, port of `cleanup.strip_spans` (same regexes, fences left
    alone, a line of only tags dropped).
  - `fn local_stamp(utc: NaiveDateTime) -> String` → local time `%Y-%m-%dT%H:%M:%S`.
  - `fn front_title(title: &str) -> String` → the `title: …` value with the quoting rule of
    `page_markdown`.
  - `fn safe(s: &str) -> String` (port of `run_import.safe`; empty → `Untitled page`).
  - `struct NameAllocator` with `fn take(&mut self, name: &str) -> String` giving `name`, then
    `name-1`, `name-2`, … case-insensitively; `fn reserved(name)` true for `resources` (any case)
    and names starting with `.`, which always get a suffix.

- [ ] **Step 1: Write failing tests:**
  - `esc_specials`: `esc_plain("a*b[c]<d>$e\\")` == `r"a\*b\[c\]\<d>\$e\\"`.
  - `esc_autolink`: `esc("see https://x.org/a), now")` == `"see <https://x.org/a>), now"`.
  - `spans_removed`: `"<span style='x'>Hi</span>\n<span a>\n```\n<span>\n```"` →
    `"Hi\n```\n<span>\n```"`; guillemet form `«span style='x'»Hi«/span»` → `"Hi"`.
  - `title_quoting`: `front_title("Plain")=="Plain"`, `front_title("a: b")=="\"a: b\""`,
    `front_title("say \"x\"")=="\"say \\\"x\\\"\""`, `front_title(" pad")=="\" pad\""`.
  - `safe_names`: `safe("a/b:c?")=="abc"`, `safe("x.")=="x"`, `safe("  ")=="Untitled page"`.
  - `allocator`: `take("Plan")`, `take("plan")`, `take("Plan")` → `Plan`, `plan-1`, `Plan-2`;
    `take("resources")` → `resources-1`; `take(".x")` → `.x-1`.
  - `local_time`: `local_stamp(t)` equals `Utc.from_utc_datetime(&t).with_timezone(&Local).format(…)`
    for a summer and a winter date (asserts the conversion is used, not a fixed offset).
- [ ] **Step 2:** `cargo test onenote::convert` → FAIL.
- [ ] **Step 3:** Implement the helpers.
- [ ] **Step 4:** `cargo test onenote::convert` → PASS.
- [ ] **Step 5:** Commit `feat(onenote): markdown text helpers`.

### Task 4: Converter (pages to Markdown)

**Files:**
- Modify: `src\onenote\convert.rs`

**Interfaces:**
- Consumes: `OneFile`, `Space`, `Obj`, `PropSet`, constants (Tasks 1–2); helpers (Task 3).
- Produces:
  - `struct Page { pub title: String, pub created: Option<NaiveDateTime>, pub updated: Option<NaiveDateTime>, pub level: u32, pub blocks: Vec<Vec<String>> }`
  - `struct Converter<'a> { … pub warnings: Vec<String> }`,
    `fn Converter::new(one: &'a OneFile, add_resource: &'a mut dyn FnMut(&[u8], &str) -> String) -> Self`
    (`add_resource(bytes, ext_with_dot)` returns the file name inside `resources`),
    `fn section_pages(&mut self) -> Result<Vec<Page>, String>`.
  - `fn page_markdown(p: &Page) -> String` — body links use `resources/<name>`; title
    `Untitled page` when empty; result already passed through `strip_spans`.

- [ ] **Step 1: Write failing tests** building small `Space`s by hand (helper `fn obj(jcid, props)`):
  - `runs_formatting`: one rich-text object `"abcdefgh"` with run indexes `[2,4,6]` and run formats
    bold / italic+underline / superscript / (strike+subscript) → 
    `"**ab***<u>cd</u>*<sup>ef</sup>~~<sub>gh</sub>~~"`.
  - `underline_not_on_links`: a run `﷟HYPERLINK "https://a.b"x` with underline → `"[x](https://a.b)"`.
  - `code_run`: font `Consolas` with bold + underline → `` "`text`" `` with only `**` around it
    (`"**`text`**"`), no `<u>`.
  - `line_breaks`: text with `\r\n`, `\u{000B}`, `\u{2028}` → lines joined with `"  \n"`.
  - `heading_style`: paragraph style id `h2` → `"## Title"`.
  - `lists_and_todos`: an outline with two numbered OEs, a bulleted child and a to-do (note tag
    state completed=1) → `["1. a", "  - b", "2. c", "- [x]  d"]`.
  - `table`: 2×2 table, a cell containing `a|b` → `["| x | a\\|b |", "|---|---|", "| y |  |"]`.
  - `untitled_page`: `page_markdown` of a `Page` with empty title and no times →
    `"---\ntitle: Untitled page\n---\n"`.
  - `page_md_shape`: a page with `updated`, `created` and two blocks → front matter order
    `title, updated, created`, blocks separated by a `---` line, image link `![alt](resources/abc.png)`.
  - `missing_picture_is_warned`: an image whose file data is absent → empty line content and a
    warning containing the page title.
- [ ] **Step 2:** `cargo test onenote::convert` → new tests FAIL.
- [ ] **Step 3:** Port `Converter` and `page_markdown` from `convert.py`. Underline uses `P_UNDER`,
  superscript `P_SUPER`, subscript `P_SUB`; nesting order from inside out: text → `<sub>`/`<sup>` →
  `<u>` → `~~` → `**`/`*`/`***`. Code-page decoding of `P_TEXT_ASCII` uses `encoding_rs`
  (`windows-1252/1250/1251/1253/1254/1257`, unknown → 1252). Element sort key on a page:
  `(round1(v), round1(h))` with Python's round-half-even. Warnings name the page title.
- [ ] **Step 4:** `cargo test onenote` → PASS.
- [ ] **Step 5:** Commit `feat(onenote): convert pages to markdown`.

### Task 5: Import driver and command-line option

**Files:**
- Modify: `src\onenote\mod.rs`
- Modify: `src\main.rs` (`--import-onenote <src> <target>`)
- Modify: `Cargo.toml` (windows feature `Win32_System_Console` for `AttachConsole`)
- Create: `tools\onenote-import\compare.py`

**Interfaces:**
- Consumes: `OneFile::parse`, `Converter`, `page_markdown`, `safe`, `NameAllocator`,
  `history::snapshot_now`, `lock::held_by_other`.
- Produces:
  - `fn find_sections(src: &Path) -> Vec<PathBuf>` (sorted per folder like `os.walk` + `sorted`,
    skipping `OneNote_RecycleBin`).
  - `fn notebook_dir(src: &Path, target: &Path) -> PathBuf` = `target\<src folder name>`.
  - `fn check_target(src: &Path, target: &Path) -> Result<(), String>` — `Err` when the target equals
    the source or lies inside it.
  - `enum Progress { Section { index: usize, count: usize, place: String }, Done(Report) }`
  - `#[derive(Default)] struct Report { pub sections: usize, pub pages_written: usize, pub pages_replaced: usize, pub pages_unchanged: usize, pub resources_written: usize, pub problems: Vec<String>, pub cancelled: bool }`
    with `fn summary(&self) -> String` (multi-line text used by the wizard and the CLI).
  - `fn import(src: &Path, target: &Path, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> Report`
    (synchronous; the caller puts it on a thread).
  - `fn write_section(dir: &Path, pages: &[(String, String)], report: &mut Report)` — writes
    `(file stem, markdown)` pairs, used by `import` and directly by tests.

- [ ] **Step 1: Write failing tests** (temp dirs under `std::env::temp_dir()`, like the existing
  store/history tests):
  - `write_new_pages`: two pages into an empty dir → files exist, `pages_written == 2`.
  - `replace_keeps_history`: existing `A.md` with other text → replaced, `history::list_versions`
    has 1 version with the old text, `pages_replaced == 1` — even when a version younger than 10
    minutes already exists.
  - `reimport_unchanged_writes_nothing`: same text → file mtime unchanged, no new version,
    `pages_unchanged == 1`.
  - `locked_page_is_skipped`: a page with a fresh lock file from another computer (write the lock
    file as `lock.rs` does) → not replaced, one problem mentioning the page.
  - `target_inside_source_is_refused`: `check_target(src, src)` and `check_target(src, src\sub)` are
    `Err`; a sibling is `Ok`.
  - `cancel_stops_after_section`: `import` with `cancel` already set → `report.cancelled`, nothing
    written beyond at most one section.
  - `sections_found_sorted`: a temp tree with `b.one`, `A.one`, `g\c.one`,
    `OneNote_RecycleBin\x.one`, `t.onetoc2` → `[A.one, b.one, g\c.one]` in `os.walk` order.
- [ ] **Step 2:** `cargo test onenote` → FAIL.
- [ ] **Step 3:** Implement. Port `run_import.py`: section names with `^J`/newlines → spaces, then
  `safe`; one `NameAllocator` per output folder for sections and group folders; one per section
  for pages (Python's `-1` rule). Resources: `md5` of the bytes, written to
  `<section dir>\resources\` if missing, counted in `resources_written`. Before replacing a file,
  `snapshot_now(path, Local::now().naive_local())`; on `Err` add a problem and keep the old file.
  Parse failures add `"<relative .one path>: <error>"` to problems. Converter warnings go to
  problems as `"<section> › <page>: <warning>"`.
- [ ] **Step 4:** In `main.rs`, `--import-onenote <src> <target>`: call
  `AttachConsole(ATTACH_PARENT_PROCESS)` (ignore failure), run `onenote::import` with a progress
  callback that prints one line per section, print `report.summary()`, exit with code 0 when
  `problems` holds no section failure and not cancelled, else 1. No window is created.
- [ ] **Step 5:** Write `tools\onenote-import\compare.py <python_out> <rust_out>` (stdlib only, run
  with `python -I`): for every `.md` in either tree, normalize and compare; print the differing
  files with a unified diff of the first 20 lines, then counts. Normalization:
  Python side → `strip_spans` (import from `cleanup.py`), `title: Neimenovana stran` →
  `title: Untitled page`, `(\.\./)+resources/` → `resources/`. Rust side → remove
  `</?(u|sup|sub)>`. Also check that every `resources/<name>` link in the Rust tree points to an
  existing file.
- [ ] **Step 6:** `cargo test` → all PASS.
- [ ] **Step 7: Compare with Python on the real notebook** (scratch folders, never the source):
  ```
  python -I tools\onenote-import\run_import.py "u:\OneNote\Primož" <scratch>\py
  cargo build --release
  target\release\notecraft.exe --import-onenote "u:\OneNote\Primož" <scratch>\rs
  python -I tools\onenote-import\compare.py <scratch>\py\Primož <scratch>\rs\Primož
  ```
  Expected: same page count as Python, `0 differing files`, no broken resource links. Any
  difference is a porting bug unless it is a section collision suffix (list it in the commit
  message). Fix and repeat until clean.
- [ ] **Step 8:** Commit `feat(onenote): import driver, --import-onenote, compare tool`.

### Task 6: Wizard and app integration

**Files:**
- Create: `src\app\import_ui.rs`
- Modify: `src\app.rs` (module, field, menu item, `covered` check at ~line 1772, call in `ui`,
  settings `import_src=`/`import_dst=` lines in `Settings`)

**Interfaces:**
- Consumes: `onenote::{find_sections, notebook_dir, check_target, import, Progress, Report}`,
  `NoteApp::add_root`, `store::check_new_root`, `save_if_dirty`, `reload_tree`, `reload_pages`,
  `start_indexing`.
- Produces: `pub struct ImportWin` (state: step 1/2/3, source and target text, section count,
  confirm flag, receiver, `Arc<AtomicBool>` cancel, last progress, final report) with
  `ImportWin::new(src: Option<PathBuf>, dst: Option<PathBuf>) -> Self`; `impl NoteApp { pub(super) fn open_import(&mut self); fn import_window(&mut self, ctx: &egui::Context) }`.

- [ ] **Step 1:** Add **Import from OneNote…** to the Folders menu (after "Add folder…") calling
  `open_import`, which builds `ImportWin` from the remembered settings.
- [ ] **Step 2:** Step 1 of the wizard (layout as in the spec): path box, Browse… (`rfd`), live
  `"Found N sections in M folders."` (recount when the path changes), Next enabled only when N > 0,
  otherwise `"No .one files in this folder."`; Cancel closes.
- [ ] **Step 3:** Step 2: path box, Browse…, the line `"The notebook will be written to <path>"`,
  `check_target` error shown and Next disabled on `Err`; < Back. On Next, when
  `notebook_dir` exists and is non-empty, show the confirmation text from the spec with Yes/No
  inside the window (No returns to step 2).
- [ ] **Step 4: save before start.** On starting the import: `save_if_dirty()`, leave edit mode,
  store the two paths in settings, spawn the thread running `onenote::import` with a progress
  closure that sends over the channel and calls `ctx.request_repaint()`.
- [ ] **Step 5:** Step 3: progress bar `index/count` with `"Section i of n: place"`, Cancel sets the
  flag (button disabled after a click, label "Cancelling…"). On `Progress::Done`: show
  `report.summary()` in a scroll area and a Close button; then add the target with `add_root`
  when `check_new_root` accepts it, else `reload_tree()`; `reload_pages()`, reload the open page
  from disk, `start_indexing()`; status line `"Imported N pages from OneNote"`.
- [ ] **Step 6:** Hide the page web view while the window is open (add `self.import_win.is_some()`
  to `covered`). Closing the window while the import runs sets cancel and keeps showing progress
  until `Done` (the window cannot close mid-section).
- [ ] **Step 7:** `cargo build` and `cargo test` → PASS. Manual check with
  `target\debug\notecraft.exe`: import `u:\OneNote\Primož` into a scratch folder (fresh), then
  again into the same folder (confirmation appears, summary shows 0 replaced / N unchanged), edit
  one imported page, import again (1 replaced, its History shows the edited text), Cancel during a
  run, and a target inside the source (Next disabled).
- [ ] **Step 8:** Commit `feat: Import from OneNote wizard`.

### Task 7: Documentation

**Files:**
- Modify: `README.md` (section "Importing from OneNote": the wizard, `--import-onenote`, the five
  deliberate differences from the Python tool; `tools\onenote-import` is the reference
  implementation; "Layout of the source" gets `src\onenote\` and `src\app\import_ui.rs`; the
  development options list gets `--import-onenote`)
- Modify: `tools\onenote-import\README.md` (NoteCraft has the import built in; `compare.py`)

- [ ] **Step 1:** Update both files.
- [ ] **Step 2:** Commit `docs: OneNote import`.
