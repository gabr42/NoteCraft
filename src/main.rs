#![windows_subsystem = "windows"]

mod app;
mod history;
mod lock;
mod render;
mod store;
mod web;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let mut start = app::Start::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--screenshot" => start.screenshot = args.next().map(PathBuf::from),
            "--page" => start.page = args.next().map(PathBuf::from),
            "--search" => start.search = args.next(),
            "--edit" => start.edit = true,
            "--show-history" => start.show_history = true,
            "--show-deleted" => start.show_deleted = true,
            "--history" => start.history = true,
            other if !other.starts_with("--") => start.folders.push(PathBuf::from(other)),
            _ => {}
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("NoteCraft")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 420.0])
            .with_icon(window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "NoteCraft",
        options,
        Box::new(move |cc| Ok(Box::new(app::NoteApp::new(cc, start)))),
    )
}

use eframe::egui;

/// The NoteCraft icon for the title bar and the taskbar.
fn window_icon() -> egui::IconData {
    let decoded = image::load_from_memory(include_bytes!("../assets/notecraft.ico"))
        .map(|img| img.into_rgba8());
    match decoded {
        Ok(rgba) => egui::IconData { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() },
        Err(_) => egui::IconData::default(),
    }
}
