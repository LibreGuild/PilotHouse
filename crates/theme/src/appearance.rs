use gpui_kit::component::Theme;
use gpui_kit::{App, Window, WindowAppearance};
use serde::{Deserialize, Serialize};

use crate::themes::{self, DEFAULT_DARK, DEFAULT_LIGHT};

/// The theme the user chose, stored in settings: one by name, or the
/// system's light or dark one.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Modern Light or Modern Dark, as the operating system is.
    #[default]
    System,
    /// Saved before themes had names: Modern Light.
    Light,
    /// Saved before themes had names: the dark palette of the preferences
    /// (`dark_theme`).
    Dark,
    /// A theme by name: PilotHouse's or one of the user's.
    Theme(String),
}

impl Appearance {
    /// The name of the theme to show.
    pub fn theme_name(&self, system_dark: bool, preferences: &crate::Preferences) -> String {
        match self {
            Appearance::System if system_dark => DEFAULT_DARK.into(),
            Appearance::System | Appearance::Light => DEFAULT_LIGHT.into(),
            Appearance::Dark => preferences.dark_theme.theme_name().into(),
            Appearance::Theme(name) => name.clone(),
        }
    }

    /// The same choice in today's terms: a theme by name (or the system).
    pub fn migrated(&self, preferences: &crate::Preferences) -> Appearance {
        match self {
            Appearance::Light | Appearance::Dark => {
                Appearance::Theme(self.theme_name(false, preferences))
            }
            other => other.clone(),
        }
    }
}

/// Loads PilotHouse's themes and shows Modern Dark until the user's choice
/// is applied.
///
/// Must run after `gpui_kit::init`, which creates the theme.
pub fn init(cx: &mut App) {
    themes::init(cx);
    let dark = themes::select(DEFAULT_DARK, true, cx);
    let light = themes::find(DEFAULT_LIGHT, cx);
    Theme::update(cx, |theme| {
        theme.dark_theme = dark;
        if let Some(light) = light {
            theme.light_theme = light;
        }
    });
}

/// Whether the system (as the window sees it) is in dark mode.
pub fn system_is_dark(window: Option<&Window>, cx: &App) -> bool {
    let appearance = window
        .map(|window| window.appearance())
        .unwrap_or_else(|| cx.window_appearance());
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

/// Shows the theme `appearance` names.
///
/// For [`Appearance::System`] the window's appearance decides; callers
/// re-run this from `observe_window_appearance` so the app follows OS
/// changes. The user's [`crate::Preferences`] (fonts, corners…) are applied
/// on top afterwards, since loading a theme resets them.
pub fn apply_appearance(appearance: &Appearance, mut window: Option<&mut Window>, cx: &mut App) {
    let dark = system_is_dark(window.as_deref(), cx);
    let name = appearance.theme_name(dark, &crate::preferences::preferences(cx));
    themes::select(&name, dark, cx);
    // A preview, while there is one, shows over the choice.
    let shown = themes::current(cx);
    Theme::update(cx, |theme| {
        if shown.mode.is_dark() {
            theme.dark_theme = shown.clone();
        } else {
            theme.light_theme = shown.clone();
        }
    });
    Theme::change(shown.mode, window.as_deref_mut(), cx);
    crate::preferences::apply(window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DarkTheme, Preferences};

    #[test]
    fn choices_saved_before_themes_had_names_keep_their_look() {
        let mut preferences = Preferences::default();
        assert_eq!(
            Appearance::Light.migrated(&preferences),
            Appearance::Theme("Modern Light".into())
        );
        assert_eq!(
            Appearance::Dark.migrated(&preferences),
            Appearance::Theme("Modern Dark".into())
        );
        preferences.dark_theme = DarkTheme::Warm;
        assert_eq!(
            Appearance::Dark.migrated(&preferences),
            Appearance::Theme("Warm Dark".into())
        );
        assert_eq!(
            Appearance::System.migrated(&preferences),
            Appearance::System
        );
        // Stored as before, and the new choice beside it.
        let saved: Appearance = serde_json::from_str("\"dark\"").unwrap();
        assert_eq!(saved, Appearance::Dark);
        let named = serde_json::to_string(&Appearance::Theme("Dracula".into())).unwrap();
        assert_eq!(
            serde_json::from_str::<Appearance>(&named).unwrap(),
            Appearance::Theme("Dracula".into())
        );
        assert_eq!(
            Appearance::System.theme_name(true, &preferences),
            "Modern Dark"
        );
        assert_eq!(
            Appearance::System.theme_name(false, &preferences),
            "Modern Light"
        );
    }
}
