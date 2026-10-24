//! On Windows, embeds the app icon into `pilothouse.exe` (resource 1, which
//! GPUI uses for the window and the taskbar). Elsewhere nothing happens: the
//! macOS bundle carries `assets/icon/PilotHouse.icns`, and Linux desktop
//! entries point at `assets/icon/PilotHouse-1024.png`.

fn main() {
    println!("cargo:rerun-if-changed=assets/pilothouse.rc");
    println!("cargo:rerun-if-changed=assets/icon/PilotHouse.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/pilothouse.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("embedding the Windows icon");
    }
}
