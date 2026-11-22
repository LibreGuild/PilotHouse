//! The fonts PilotHouse carries: Inter for the interface, JetBrains Mono for
//! code (its "NL" cut, without ligatures: `!=` stays two characters, as it is
//! typed).
//!
//! Each system's own fonts have different shapes and metrics, so the same
//! layout sat differently on macOS, Windows and Linux. With its own fonts the
//! text takes the same room everywhere. Both families are under the SIL Open
//! Font License 1.1; their licences are beside the files in `fonts/`.

use std::borrow::Cow;

use gpui_kit::{App, Global};

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

/// A bundled face for a web page of the app's own (a deck's): what CSS
/// calls its family, its weight and style, and its file.
pub struct WebFace {
    pub file: &'static str,
    pub family: &'static str,
    pub weight: u16,
    pub italic: bool,
    pub bytes: &'static [u8],
}

/// The bundled faces, for pages the app serves: such a page then reads the
/// same on every system, as the app's own text does.
pub const WEB_FACES: [WebFace; 10] = {
    const fn face(
        file: &'static str,
        family: &'static str,
        weight: u16,
        italic: bool,
        index: usize,
    ) -> WebFace {
        WebFace {
            file,
            family,
            weight,
            italic,
            bytes: FILES[index],
        }
    }
    [
        face("Inter-Regular.ttf", "Inter", 400, false, 0),
        face("Inter-Italic.ttf", "Inter", 400, true, 1),
        face("Inter-Medium.ttf", "Inter", 500, false, 2),
        face("Inter-SemiBold.ttf", "Inter", 600, false, 3),
        face("Inter-Bold.ttf", "Inter", 700, false, 4),
        face("Inter-BoldItalic.ttf", "Inter", 700, true, 5),
        face("JetBrainsMono-Regular.ttf", "JetBrains Mono", 400, false, 6),
        face("JetBrainsMono-Italic.ttf", "JetBrains Mono", 400, true, 7),
        face("JetBrainsMono-Bold.ttf", "JetBrains Mono", 700, false, 8),
        face(
            "JetBrainsMono-BoldItalic.ttf",
            "JetBrains Mono",
            700,
            true,
            9,
        ),
    ]
};

/// Whether this app's text system took the bundled families (set once, by
/// [`register`]). Per app, not per process: each test has an app of its own.
struct Registered(bool);

impl Global for Registered {}

/// Makes the bundled families known to the text system, once, and says
/// whether it has them. Without them the defaults fall back to the system's
/// fonts (see `preferences`).
pub fn register(cx: &mut App) -> bool {
    if let Some(registered) = cx.try_global::<Registered>() {
        return registered.0;
    }
    let fonts = FILES.iter().copied().map(Cow::Borrowed).collect();
    let loaded = match cx.text_system().add_fonts(fonts) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!("loading the bundled fonts failed: {err:#}");
            false
        }
    };
    cx.set_global(Registered(loaded));
    loaded
}

/// The bundled families are there to draw with. `false` before [`register`]
/// ran, or when it could not load them.
pub fn registered(cx: &App) -> bool {
    cx.try_global::<Registered>()
        .is_some_and(|registered| registered.0)
}
