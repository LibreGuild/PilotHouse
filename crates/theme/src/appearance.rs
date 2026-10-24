use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Window};
use serde::{Deserialize, Serialize};

/// The theme set bundled into the binary.
const PILOTHOUSE_THEMES: &str = include_str!("../themes/pilothouse.json");
const LIGHT_THEME_NAME: &str = "PilotHouse Light";
const DARK_THEME_NAME: &str = "PilotHouse Dark";

/// The user's appearance preference, stored in settings.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Follow the operating system's light/dark setting.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }
}

/// Registers the PilotHouse themes and makes them the light/dark pair.
///
/// Must run after `gpui_kit::init`, which creates the theme registry.
pub fn init(cx: &mut App) {
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(PILOTHOUSE_THEMES) {
        // The JSON is compiled in and covered by a test, so this only fires
        // during development when the file is edited by hand.
        tracing::error!("failed to parse bundled PilotHouse themes: {err:#}");
        return;
    }

    let registry = ThemeRegistry::global(cx);
    let light = registry.themes().get(LIGHT_THEME_NAME).map(Rc::clone);
    let dark = registry.themes().get(DARK_THEME_NAME).map(Rc::clone);

    Theme::update(cx, |theme| {
        if let Some(light) = light {
            theme.light_theme = light;
        }
        if let Some(dark) = dark {
            theme.dark_theme = dark;
        }
    });
}

/// Switches the active theme to match `appearance`.
///
/// For [`Appearance::System`] the window's current appearance decides; callers
/// re-run this from `observe_window_appearance` so the app follows OS changes.
///
/// The user's [`crate::Preferences`] (accent, fonts, corners…) are applied on
/// top afterwards, since loading a theme resets them.
pub fn apply_appearance(appearance: Appearance, mut window: Option<&mut Window>, cx: &mut App) {
    match appearance {
        Appearance::System => Theme::sync_system_appearance(window.as_deref_mut(), cx),
        Appearance::Light => Theme::change(ThemeMode::Light, window.as_deref_mut(), cx),
        Appearance::Dark => Theme::change(ThemeMode::Dark, window.as_deref_mut(), cx),
    }
    crate::preferences::apply(window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled theme file must parse and contain both modes, otherwise the
    /// app silently falls back to gpui-component's default palette.
    #[test]
    fn bundled_themes_parse_with_both_modes() {
        let value: serde_json::Value =
            serde_json::from_str(PILOTHOUSE_THEMES).expect("theme JSON parses");
        let names: Vec<&str> = value["themes"]
            .as_array()
            .expect("themes array")
            .iter()
            .filter_map(|theme| theme["name"].as_str())
            .collect();
        assert!(names.contains(&LIGHT_THEME_NAME));
        assert!(names.contains(&DARK_THEME_NAME));
    }
}
