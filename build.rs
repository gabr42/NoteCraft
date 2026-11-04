//! Puts the icon and the program details into NoteCraft.exe (seen in File Explorer and the taskbar).

fn main() {
    println!("cargo:rerun-if-changed=assets/notecraft.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/notecraft.ico")
            .set("ProductName", "NoteCraft")
            .set("FileDescription", "NoteCraft - notes in Markdown folders")
            .set("OriginalFilename", "NoteCraft.exe");
        if let Err(e) = res.compile() {
            // A missing resource compiler only costs the icon, not the program.
            println!("cargo:warning=Could not add the program icon: {e}");
        }
    }
}
