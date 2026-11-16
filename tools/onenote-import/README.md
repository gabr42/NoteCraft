# OneNote (.one) to NoteCraft markdown

Standalone Python 3 parser for the MS-ONESTORE/MS-ONE format, no third-party packages.

- `onepar.py` - `.one` parser (`OneFile`). Per object, the copy with the latest LastModifiedTime wins.
- `convert.py` - converts parsed pages to NoteCraft markdown (front matter, tables, images, autolinks).
- `run_import.py <src folder> <dest .note folder>` - converts a notebook tree to `<dest>\<name>\<section>\<page>.md` plus `<dest>\resources\`.
- `cleanup.py` - removes leftover `<span>` tags (used by `convert.py`; also runs on existing folders: `cleanup.py <folder> [--apply] [--backup <dir>]`).

Run with `python -I`.

## NoteCraft's built-in import

NoteCraft imports OneNote notebooks itself (**Folders > Import from OneNote...**, or
`NoteCraft.exe --import-onenote <notebook folder> <target folder>`), so Python is not needed for
that. These scripts stay as the reference implementation of the port.

- `compare.py <python_out> <rust_out>` - compares the output of both importers page by page. Run
  both from the same source into empty scratch folders, then compare the notebook folders:

      python -I run_import.py "E:\OneNote\Work" D:\scratch\py
      NoteCraft.exe --import-onenote "E:\OneNote\Work" D:\scratch\rs
      python -I compare.py D:\scratch\py\Work D:\scratch\rs\Work

  It ignores the deliberate differences (span tags, the untitled page title, the place of the
  `resources` folder, `<u>`, `<sup>`, `<sub>`), lists differing files with the start of a diff and
  checks that the `resources/` links of the Rust output point to existing files. The exit code is 0
  when nothing differs.
