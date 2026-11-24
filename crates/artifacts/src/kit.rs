//! What the app's own pages share: a deck's page and a board's are drawn
//! with the same controls, in the same hand, in the app's colours.
//!
//! - `kit/ui.js`: icons, buttons, menus, popovers, dialogs, a colour picker.
//! - `kit/kit.css`: how they look, and the `--ui-*` variables' stand-ins.
//! - [`chrome`]: the app's colours and sizes as those variables.
//! - The app's fonts, declared for a page.
//!
//! Served at every artifact's address under `/_artifact/kit/` (see
//! `server.rs`): a kind's page links them from there.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla};

use crate::http::{Body, Response};

const UI: &str = include_str!("kit/ui.js");
const CSS: &str = include_str!("kit/kit.css");

/// A file of the kit, by name: as built in, or, in a debug build, as it is
/// in the folder `PILOTHOUSE_KIT_PAGE` names (so a change shows on reload).
fn asset(name: &str) -> Option<String> {
    let built_in = match name {
        "ui.js" => UI,
        "kit.css" => CSS,
        _ => return None,
    };
    if cfg!(debug_assertions)
        && let Some(folder) = std::env::var_os("PILOTHOUSE_KIT_PAGE")
        && let Ok(text) = std::fs::read_to_string(std::path::PathBuf::from(folder).join(name))
    {
        return Some(text);
    }
    Some(built_in.to_string())
}

/// Answers a read of `/_artifact/kit/<name>`; `None` for what the kit has
/// not.
pub(crate) fn respond(name: &str) -> Option<Response> {
    let answer = |kind: &'static str, body: Vec<u8>| {
        Response::new(200)
            .header("Content-Type", kind)
            // Asked for again each time: an app that was updated serves
            // another file under the same name.
            .header("Cache-Control", "no-cache")
            .body(Body::Bytes(body))
    };
    if name == "fonts.css" {
        return Some(answer(
            "text/css; charset=utf-8",
            fonts_css("/_artifact/kit/font/").into_bytes(),
        ));
    }
    if let Some(file) = name.strip_prefix("font/") {
        let face = ph_theme::fonts::WEB_FACES
            .iter()
            .find(|face| face.file == file)?;
        return Some(
            Response::new(200)
                .header("Content-Type", "font/ttf")
                .header("Cache-Control", "max-age=86400")
                .body(Body::Bytes(face.bytes.to_vec())),
        );
    }
    let text = asset(name)?;
    let kind = if name.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else {
        "text/css; charset=utf-8"
    };
    Some(answer(kind, text.into_bytes()))
}

/// The fonts the app brings, declared for a page; their files are under
/// `base`.
pub fn fonts_css(base: &str) -> String {
    ph_theme::fonts::WEB_FACES
        .iter()
        .map(|face| {
            format!(
                "@font-face {{ font-family: \"{}\"; font-weight: {}; font-style: {}; font-display: block; src: url(\"{base}{}\") format(\"truetype\"); }}\n",
                face.family,
                face.weight,
                if face.italic { "italic" } else { "normal" },
                face.file,
            )
        })
        .collect()
}

/// The app's colours and sizes as CSS variables (`--ui-*`): what makes a
/// page of the app's own look like the window it is in.
pub fn chrome(cx: &App) -> String {
    let theme = cx.theme();
    let colour = |colour: Hsla| {
        let rgba = colour.to_rgb();
        let part = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
        format!(
            "rgba({},{},{},{:.3})",
            part(rgba.r),
            part(rgba.g),
            part(rgba.b),
            rgba.a.clamp(0., 1.)
        )
    };
    let dark = theme.is_dark();
    let font = |family: &str| match family {
        // The system's own font has no name a page could ask for.
        name if name.starts_with('.') => "system-ui".to_string(),
        name => format!("\"{name}\", system-ui"),
    };
    [
        ("bg", colour(theme.background)),
        ("fg", colour(theme.foreground)),
        ("muted", colour(theme.muted_foreground)),
        ("border", colour(theme.border)),
        ("surface", colour(theme.popover)),
        ("hover", colour(theme.secondary_hover)),
        ("accent", colour(theme.primary)),
        ("accent-fg", colour(theme.primary_foreground)),
        ("danger", colour(theme.danger)),
        ("success", colour(theme.success)),
        ("warning", colour(theme.warning)),
        ("stage", colour(theme.muted)),
        ("radius", format!("{}px", f32::from(theme.radius))),
        ("size", format!("{}px", f32::from(theme.font_size))),
        ("font", format!("{}, sans-serif", font(&theme.font_family))),
        (
            "mono",
            format!(
                "{}, monospace",
                font(&theme.mono_font_family).replace(" NL\"", "\"")
            ),
        ),
        ("scheme", if dark { "dark" } else { "light" }.to_string()),
    ]
    .iter()
    .map(|(name, value)| format!("--ui-{name}:{value};"))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kit_is_served_whole() {
        for name in ["ui.js", "kit.css", "fonts.css"] {
            assert_eq!(
                respond(name).map(|answer| answer.status),
                Some(200),
                "{name}"
            );
        }
        assert!(respond("nothing.js").is_none());
        assert!(respond("font/nothing.ttf").is_none());
        // Every icon a page names is one the kit draws.
        let ui = asset("ui.js").unwrap();
        for icon in [
            "text", "sticky", "frame", "present", "share", "lock", "tidy",
        ] {
            assert!(ui.contains(&format!("\n  {icon}: `")), "{icon}");
        }
        assert_eq!(
            fonts_css("/f/").matches("@font-face").count(),
            ph_theme::fonts::WEB_FACES.len()
        );
    }
}
