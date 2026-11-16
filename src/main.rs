#![windows_subsystem = "windows"]

mod app;
mod history;
mod lock;
mod mover;
mod onenote;
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
            "--import-onenote" => {
                let (Some(src), Some(target)) = (args.next(), args.next()) else {
                    attach_console();
                    println!("Usage: notecraft --import-onenote <OneNote notebook folder> <target folder>");
                    std::process::exit(2);
                };
                std::process::exit(import_onenote(&PathBuf::from(src), &PathBuf::from(target)));
            }
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

/// Makes `println!` reach the console this program was started from (the program is built for the
/// Windows GUI subsystem, which has no console of its own). Does nothing when stdout is redirected.
#[cfg(windows)]
fn attach_console() {
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE, SetStdHandle};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        let valid = GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|h| !h.is_invalid() && !h.0.is_null());
        if !valid
            && let Ok(con) = std::fs::OpenOptions::new().write(true).open("CONOUT$")
        {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, HANDLE(con.into_raw_handle()));
        }
    }
}

#[cfg(not(windows))]
fn attach_console() {}

/// `--import-onenote <src> <target>`: the same import as the wizard, without a window. Returns the
/// exit code: 0 when every section was imported, 1 otherwise.
fn import_onenote(src: &std::path::Path, target: &std::path::Path) -> i32 {
    use onenote::Progress;
    attach_console();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let progress = |p: Progress| {
        if let Progress::Section { index, count, place } = p {
            println!("[{}/{}] {}", index + 1, count, place);
        }
    };
    let report = onenote::import(src, target, &progress, &cancel);
    println!("{}", report.summary());
    if report.cancelled || report.sections_failed > 0 { 1 } else { 0 }
}

/// The NoteCraft icon for the title bar and the taskbar.
fn window_icon() -> egui::IconData {
    let decoded = image::load_from_memory(include_bytes!("../assets/notecraft.ico"))
        .map(|img| img.into_rgba8());
    match decoded {
        Ok(rgba) => egui::IconData { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() },
        Err(_) => egui::IconData::default(),
    }
}
