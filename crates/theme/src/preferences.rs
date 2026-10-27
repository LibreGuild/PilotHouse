//! How the user tuned the look: accent, fonts, sizes, corners, translucency,
//! and editor behavior.
//!
//! Preferences sit on top of the light/dark theme: [`apply`] re-applies them
//! whenever the theme is (re)loaded, so switching light/dark keeps them.
//! They are a GPUI global; panels read the editor and terminal parts.

use gpui_kit::component::Theme;
use gpui_kit::{App, Global, Hsla, Window, WindowBackgroundAppearance, hsla, px, rgb};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub accent: Accent,
    /// Interface font family; empty for the platform's own.
    pub ui_font: String,
    pub ui_font_size: f32,
    /// Code font for the editor, diffs and terminals; empty for the default.
    pub code_font: String,
    pub code_font_size: f32,
    pub terminal_font_size: f32,
    pub corners: Corners,
    pub window: WindowStyle,
    /// How opaque panels are when the window is translucent (0.55–1.0).
    pub opacity: f32,
    pub editor: EditorPreferences,
    pub chat_width: ChatWidth,
    /// Names under the tool icons on the right, not only in tooltips.
    pub tool_labels: bool,
    /// Which default sizes these preferences were saved with (missing in
    /// ones saved before; see [`Preferences::migrate_sizes`]).
    #[serde(default)]
    pub sizes_version: u32,
}

/// Default interface and code sizes. A Windows pixel reads smaller than a
/// macOS point, so the same numbers look cramped there.
const DEFAULT_SIZES: (f32, f32) = if cfg!(windows) {
    (18.0, 15.0)
} else {
    (15.0, 14.0)
};

/// The default sizes before version 1 (PilotHouse 0.2), on every system.
const OLD_SIZES: (f32, f32) = (14.0, 13.0);

/// The version of [`DEFAULT_SIZES`].
const SIZES_VERSION: u32 = 1;

/// The largest interface size (the whole UI scales with it).
pub const MAX_UI_FONT_SIZE: f32 = 24.0;

impl Default for Preferences {
    fn default() -> Self {
        let (ui_size, code_size) = DEFAULT_SIZES;
        Self {
            accent: Accent::Green,
            ui_font: String::new(),
            ui_font_size: ui_size,
            code_font: String::new(),
            code_font_size: code_size,
            terminal_font_size: code_size,
            corners: Corners::Rounded,
            window: WindowStyle::Solid,
            opacity: 0.86,
            editor: EditorPreferences::default(),
            chat_width: ChatWidth::Comfortable,
            tool_labels: false,
            sizes_version: SIZES_VERSION,
        }
    }
}

impl Preferences {
    /// Brings sizes saved with older defaults to the current ones; a size
    /// the user chose stays. True when something changed (save it).
    pub fn migrate_sizes(&mut self) -> bool {
        if self.sizes_version >= SIZES_VERSION {
            return false;
        }
        let (ui, code) = DEFAULT_SIZES;
        let (old_ui, old_code) = OLD_SIZES;
        if self.ui_font_size == old_ui {
            self.ui_font_size = ui;
        }
        if self.code_font_size == old_code {
            self.code_font_size = code;
        }
        if self.terminal_font_size == old_code {
            self.terminal_font_size = code;
        }
        self.sizes_version = SIZES_VERSION;
        true
    }
}

impl Global for Preferences {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorPreferences {
    pub tab_size: usize,
    pub soft_wrap: bool,
    pub line_numbers: bool,
    pub indent_guides: bool,
    pub folding: bool,
}

impl Default for EditorPreferences {
    fn default() -> Self {
        Self {
            tab_size: 4,
            soft_wrap: false,
            line_numbers: true,
            indent_guides: true,
            folding: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    /// Warm terracotta, matching the warm dark theme.
    Clay,
    Blue,
    Violet,
    Teal,
    Green,
    Amber,
    Rose,
}

impl Accent {
    pub const ALL: [Accent; 7] = [
        Accent::Clay,
        Accent::Blue,
        Accent::Violet,
        Accent::Teal,
        Accent::Green,
        Accent::Amber,
        Accent::Rose,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Accent::Clay => "Clay",
            Accent::Blue => "Blue",
            Accent::Violet => "Violet",
            Accent::Teal => "Teal",
            Accent::Green => "Green",
            Accent::Amber => "Amber",
            Accent::Rose => "Rose",
        }
    }

    /// The accent in dark and light themes (lighter on dark backgrounds).
    pub fn color(self, dark: bool) -> Hsla {
        let (dark_hex, light_hex) = match self {
            Accent::Clay => (0xd97757, 0xc15f3c),
            Accent::Blue => (0x6b8cff, 0x3d6df2),
            Accent::Violet => (0xa78bfa, 0x7c3aed),
            Accent::Teal => (0x2dd4bf, 0x0d9488),
            // Softer than a pure green, so it marks without shouting.
            Accent::Green => (0x5bbf85, 0x1f8a4c),
            Accent::Amber => (0xfbbf24, 0xd97706),
            Accent::Rose => (0xfb7185, 0xe11d48),
        };
        rgb(if dark { dark_hex } else { light_hex }).into()
    }

    /// The accent as a fill behind white text (an `info` button): a
    /// deeper, calmer shade of [`Self::color`].
    pub fn fill(self, dark: bool) -> Hsla {
        let accent = self.color(dark);
        Hsla {
            s: accent.s * 0.75,
            l: accent.l.min(if dark { 0.34 } else { 0.38 }),
            ..accent
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corners {
    Square,
    Subtle,
    Rounded,
}

impl Corners {
    pub const ALL: [Corners; 3] = [Corners::Square, Corners::Subtle, Corners::Rounded];

    pub fn label(self) -> &'static str {
        match self {
            Corners::Square => "Square",
            Corners::Subtle => "Subtle",
            Corners::Rounded => "Rounded",
        }
    }

    fn radius(self) -> (f32, f32) {
        match self {
            Corners::Square => (3.0, 4.0),
            Corners::Subtle => (6.0, 9.0),
            Corners::Rounded => (10.0, 14.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowStyle {
    Solid,
    /// See-through panels over a blurred desktop.
    Translucent,
    /// Windows 11's Mica material.
    Mica,
}

impl WindowStyle {
    /// The styles this platform can show.
    pub fn available() -> Vec<WindowStyle> {
        let mut styles = vec![WindowStyle::Solid, WindowStyle::Translucent];
        if cfg!(windows) {
            styles.push(WindowStyle::Mica);
        }
        styles
    }

    pub fn label(self) -> &'static str {
        match self {
            WindowStyle::Solid => "Solid",
            WindowStyle::Translucent => "Translucent",
            WindowStyle::Mica => "Mica",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatWidth {
    Narrow,
    Comfortable,
    Wide,
}

impl ChatWidth {
    pub const ALL: [ChatWidth; 3] = [ChatWidth::Narrow, ChatWidth::Comfortable, ChatWidth::Wide];

    pub fn label(self) -> &'static str {
        match self {
            ChatWidth::Narrow => "Narrow",
            ChatWidth::Comfortable => "Comfortable",
            ChatWidth::Wide => "Wide",
        }
    }

    /// Widest the chat's reading column gets.
    pub fn max_width(self) -> gpui_kit::Pixels {
        px(match self {
            ChatWidth::Narrow => 640.0,
            ChatWidth::Comfortable => 760.0,
            ChatWidth::Wide => 1000.0,
        })
    }
}

/// Font families tried in order when the user picked none: each platform's
/// most readable interface and code fonts.
fn default_fonts() -> (&'static [&'static str], &'static [&'static str]) {
    if cfg!(target_os = "macos") {
        (&[".SystemUIFont"], &["SF Mono", "JetBrains Mono", "Menlo"])
    } else if cfg!(windows) {
        (
            &["Segoe UI Variable Text", "Segoe UI"],
            &[
                "Cascadia Code",
                "Cascadia Mono",
                "JetBrains Mono",
                "Consolas",
            ],
        )
    } else {
        (
            &["Inter", "Noto Sans", "Ubuntu", "Cantarell", "DejaVu Sans"],
            &[
                "JetBrains Mono",
                "Fira Code",
                "Ubuntu Mono",
                "DejaVu Sans Mono",
            ],
        )
    }
}

/// The first of `candidates` that is installed (or the first, if the list
/// of installed fonts is not available).
fn first_installed(candidates: &[&str], installed: &[String]) -> String {
    candidates
        .iter()
        .find(|family| family.starts_with('.') || installed.iter().any(|name| name == *family))
        .or(candidates.first())
        .map(|family| family.to_string())
        .unwrap_or_default()
}

/// Installed font families, sorted, for the settings' font pickers.
pub fn installed_fonts(cx: &App) -> Vec<String> {
    let mut names = cx.text_system().all_font_names();
    names.retain(|name| !name.starts_with('.'));
    names.sort();
    names.dedup();
    names
}

/// The families in effect: the user's choice, else the platform default.
pub fn effective_fonts(preferences: &Preferences, cx: &App) -> (String, String) {
    let installed = cx.text_system().all_font_names();
    let (ui, code) = default_fonts();
    let ui_font = if preferences.ui_font.is_empty() {
        first_installed(ui, &installed)
    } else {
        preferences.ui_font.clone()
    };
    let code_font = if preferences.code_font.is_empty() {
        first_installed(code, &installed)
    } else {
        preferences.code_font.clone()
    };
    (ui_font, code_font)
}

pub fn preferences(cx: &App) -> Preferences {
    cx.try_global::<Preferences>().cloned().unwrap_or_default()
}

/// Makes `preferences` current and applies them to the theme and window.
pub fn set(preferences: Preferences, window: Option<&mut Window>, cx: &mut App) {
    cx.set_global(preferences);
    apply(window, cx);
}

/// Applies the current preferences on top of the loaded light/dark theme.
/// Call after every theme change (the theme reload resets colors and fonts).
pub fn apply(window: Option<&mut Window>, cx: &mut App) {
    let preferences = preferences(cx);
    let (ui_font, code_font) = effective_fonts(&preferences, cx);
    let (radius, radius_lg) = preferences.corners.radius();
    let translucent = preferences.window != WindowStyle::Solid;
    let alpha = preferences.opacity.clamp(0.55, 1.0);
    // From the theme's own colors every time: the edits below scale
    // alphas, and on top of an earlier pass they would compound (more
    // transparent with each change, never solid again). A pass of its own,
    // so the edits below register as changes and reach the theme's tokens.
    Theme::update(cx, |theme| {
        let config = if theme.mode.is_dark() {
            theme.dark_theme.clone()
        } else {
            theme.light_theme.clone()
        };
        theme.apply_config(&config);
    });
    Theme::update(cx, |theme| {
        let dark = theme.mode.is_dark();
        let accent = preferences.accent.color(dark);
        let fill = preferences.accent.fill(dark);
        let white = hsla(0., 0., 1., 1.);
        theme.font_family = ui_font.into();
        theme.font_size = px(preferences.ui_font_size.clamp(11.0, MAX_UI_FONT_SIZE));
        theme.mono_font_family = code_font.into();
        theme.mono_font_size = px(preferences.code_font_size.clamp(10.0, 24.0));
        theme.radius = px(radius);
        theme.radius_lg = px(radius_lg);

        // The accent marks focus, selection, links and progress: where the
        // eye should go. Surfaces, filled buttons and selected rows stay
        // neutral (the theme's own), so colour keeps its meaning.
        let colors = &mut theme.colors;
        colors.ring = accent;
        colors.info = accent;
        colors.link = accent;
        colors.link_hover = accent.opacity(0.85);
        colors.link_active = accent;
        colors.selection = accent.opacity(0.25);
        colors.drag_border = accent;
        colors.drop_target = accent.opacity(0.12);
        colors.caret = accent;
        colors.slider_bar = accent;
        colors.progress_bar = accent;
        colors.button_info = fill;
        colors.button_info_hover = fill.opacity(0.88);
        colors.button_info_active = fill.opacity(0.76);
        colors.button_info_foreground = white;

        if translucent {
            // The desktop shows through. The window paints `background`
            // once under everything, and the panels (surfaces) paint it again
            // on top, so each layer gets the alpha that makes a panel come
            // out at `alpha` overall: the chrome around them (sidebar, title
            // and status bars) is the base layer alone, the clearest part.
            // The chrome's own fills turn transparent, so nothing stacks
            // further and the slider's whole range shows.
            let layer = 1. - (1. - alpha).sqrt();
            colors.background = colors.background.opacity(layer);
            for color in [
                &mut colors.sidebar,
                &mut colors.title_bar,
                &mut colors.status_bar,
                &mut colors.tab_bar,
                &mut colors.list,
            ] {
                *color = color.opacity(0.);
            }
        }
    });
    if let Some(window) = window {
        window.set_background_appearance(match preferences.window {
            WindowStyle::Solid => WindowBackgroundAppearance::Opaque,
            WindowStyle::Translucent => WindowBackgroundAppearance::Blurred,
            WindowStyle::Mica => WindowBackgroundAppearance::MicaBackdrop,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_default_sizes_move_to_the_new_ones() {
        let mut saved: Preferences = serde_json::from_str(
            r#"{"ui_font_size":14.0,"code_font_size":16.0,"terminal_font_size":13.0}"#,
        )
        .unwrap();
        assert!(saved.migrate_sizes());
        assert_eq!(saved.ui_font_size, DEFAULT_SIZES.0);
        assert_eq!(saved.code_font_size, 16.0, "a size the user chose stays");
        assert_eq!(saved.terminal_font_size, DEFAULT_SIZES.1);
        assert!(!saved.migrate_sizes(), "once");
        assert!(!Preferences::default().clone().migrate_sizes());
    }

    #[test]
    fn two_layers_make_the_chosen_opacity() {
        for alpha in [0.55_f32, 0.7, 0.85, 1.0] {
            let layer = 1. - (1. - alpha).sqrt();
            let panel = 1. - (1. - layer) * (1. - layer);
            assert!((panel - alpha).abs() < 1e-5, "{alpha}: {panel}");
        }
    }

    #[test]
    fn old_or_partial_settings_still_load() {
        let partial: Preferences =
            serde_json::from_str(r#"{"accent":"teal","code_font_size":15}"#).unwrap();
        assert_eq!(partial.accent, Accent::Teal);
        assert_eq!(partial.code_font_size, 15.0);
        assert_eq!(partial.editor.tab_size, 4);
        assert_eq!(
            first_installed(&["Nope", "Menlo"], &["Menlo".to_string()]),
            "Menlo"
        );
        assert_eq!(first_installed(&["Nope"], &[]), "Nope");
    }
}
