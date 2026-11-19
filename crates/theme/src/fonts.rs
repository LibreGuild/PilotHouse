//! The fonts PilotHouse carries: Inter for the interface, JetBrains Mono for
//! code (its "NL" cut, without ligatures: `!=` stays two characters, as it is
//! typed).
//!
//! Each system's own fonts have different shapes and metrics, so the same
//! layout sat differently on macOS, Windows and Linux. With its own fonts the
//! text takes the same room everywhere. Both families are under the SIL Open
//! Font License 1.1; their licences are beside the files in `fonts/`.

use std::borrow::Cow;

use gpui_kit::App;

/// The interface font's family name.
pub const INTERFACE: &str = "Inter";
/// The code font's family name.
pub const CODE: &str = "JetBrains Mono NL";

/// The system's own interface font, whatever it is called there (GPUI
/// resolves this name on every platform).
pub const SYSTEM_INTERFACE: &str = ".SystemUIFont";

/// Static files, one per weight and style the app draws: GPUI picks a face
/// by family, weight and style, and does not set a variable font's axes.
const FILES: [&[u8]; 10] = [
    include_bytes!("../fonts/inter/Inter-Regular.ttf"),
    include_bytes!("../fonts/inter/Inter-Italic.ttf"),
    include_bytes!("../fonts/inter/Inter-Medium.ttf"),
    include_bytes!("../fonts/inter/Inter-SemiBold.ttf"),
    include_bytes!("../fonts/inter/Inter-Bold.ttf"),
    include_bytes!("../fonts/inter/Inter-BoldItalic.ttf"),
    include_bytes!("../fonts/jetbrains-mono/JetBrainsMonoNL-Regular.ttf"),
    include_bytes!("../fonts/jetbrains-mono/JetBrainsMonoNL-Italic.ttf"),
    include_bytes!("../fonts/jetbrains-mono/JetBrainsMonoNL-Bold.ttf"),
    include_bytes!("../fonts/jetbrains-mono/JetBrainsMonoNL-BoldItalic.ttf"),
];

/// Makes the bundled families known to the text system. Without them the
/// defaults fall back to the system's fonts (see `preferences`).
pub fn register(cx: &App) {
    let fonts = FILES.iter().copied().map(Cow::Borrowed).collect();
    if let Err(err) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("loading the bundled fonts failed: {err:#}");
    }
}
