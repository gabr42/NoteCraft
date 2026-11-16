#![allow(dead_code)]
//! Built-in OneNote importer.

pub mod convert;
mod import;
pub mod parse;

#[allow(unused_imports)] // the wizard (next task) uses the rest
pub use import::{Progress, Report, check_target, find_sections, import, notebook_dir, write_section};
