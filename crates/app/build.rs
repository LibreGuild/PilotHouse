//! On Windows, embeds the app icon (`brand/icons/app.ico`) into
//! `pilothouse.exe` (resource 1, which GPUI uses for the window and the
//! taskbar). Elsewhere nothing happens: the macOS bundle carries
//! `brand/icons/app.icns`, and Linux desktop entries point at
//! `brand/icons/app-1024.png` (see scripts/package).

fn main() {
    println!("cargo:rerun-if-changed=assets/pilothouse.rc");
    println!("cargo:rerun-if-changed=../../brand/icons/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/pilothouse.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("embedding the Windows icon");
    }
}
