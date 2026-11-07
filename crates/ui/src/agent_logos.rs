//! PilotHouse's icon (made for it), and the coding agents' own marks
//! (Claude, Codex, Gemini, Antigravity, Cline), so a chat says at a glance which
//! agent is writing. The marks are from Lobe Icons (MIT, see
//! `assets/agents/LICENSE`) and bundled, so nothing is fetched; they belong
//! to their makers and only name their tools.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use gpui_kit::{AnyElement, Image, ImageFormat, IntoElement as _, Pixels, Styled as _, img};

static LOGOS: LazyLock<HashMap<&'static str, Arc<Image>>> = LazyLock::new(|| {
    let svg = |bytes: &[u8]| Arc::new(Image::from_bytes(ImageFormat::Svg, bytes.to_vec()));
    HashMap::from([
        ("claude", svg(include_bytes!("../assets/agents/claude.svg"))),
        ("codex", svg(include_bytes!("../assets/agents/codex.svg"))),
        ("gemini", svg(include_bytes!("../assets/agents/gemini.svg"))),
        (
            "antigravity",
            svg(include_bytes!("../assets/agents/antigravity.svg")),
        ),
        // Lobe Icons' monochrome mark, on a tile so it shows on either theme.
        ("cline", svg(include_bytes!("../assets/agents/cline.svg"))),
    ])
});

/// The app icon, `size` across (`brand/icons/app-256.png`).
pub fn app_icon(size: Pixels) -> AnyElement {
    static ICON: LazyLock<Arc<Image>> = LazyLock::new(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            ph_brand::icons::APP_256_PNG.to_vec(),
        ))
    });
    img(ICON.clone()).flex_none().size(size).into_any_element()
}

/// The mark for the title bar, `height` square (`brand/icons/title-bar.png`):
/// PilotHouse's is the app icon's tile (a white mast and pennant, the
/// pennant the P's bowl, on violet).
pub fn logo_mark(height: Pixels) -> AnyElement {
    static MARK: LazyLock<Arc<Image>> = LazyLock::new(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            ph_brand::icons::TITLE_BAR_PNG.to_vec(),
        ))
    });
    img(MARK.clone())
        .flex_none()
        .size(height)
        .into_any_element()
}

/// The mark of the agent named `logo` (`claude`, `codex`, `gemini`,
/// `antigravity`, `cline`), `size` across.
pub fn agent_logo(logo: &str, size: Pixels) -> Option<AnyElement> {
    let image = LOGOS.get(logo)?.clone();
    Some(img(image).flex_none().size(size).into_any_element())
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_agent_has_its_mark() {
        for logo in ["claude", "codex", "gemini", "antigravity", "cline"] {
            assert!(super::LOGOS.contains_key(logo));
        }
        assert!(super::agent_logo("unknown", gpui_kit::px(16.)).is_none());
    }
}
