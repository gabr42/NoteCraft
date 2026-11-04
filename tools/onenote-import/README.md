# OneNote (.one) to NoteCraft markdown

Standalone Python 3 parser for the MS-ONESTORE/MS-ONE format, no third-party packages.

- `onepar.py` - `.one` parser (`OneFile`). Per object, the copy with the latest LastModifiedTime wins.
- `convert.py` - converts parsed pages to NoteCraft markdown (front matter, tables, images, autolinks).
- `run_import.py <src folder> <dest .note folder>` - converts a notebook tree to `<dest>\<name>\<section>\<page>.md` plus `<dest>\resources\`.
- `cleanup.py` - removes leftover `<span>` tags (used by `convert.py`; also runs on existing folders: `cleanup.py <folder> [--apply] [--backup <dir>]`).

Run with `python -I`.
