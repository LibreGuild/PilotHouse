//! How the user tuned the look: fonts, sizes, corners, translucency,
//! and editor behavior.
//!
//! Preferences sit on top of the light/dark theme: [`apply`] re-applies them
//! whenever the theme is (re)loaded, so switching light/dark keeps them.
//! They are a GPUI global; panels read the editor and terminal parts.

use gpui_kit::component::Theme;
use gpui_kit::{App, Global, Hsla, Window, WindowBackgroundAppearance, px, rgb};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Interface font family; empty for the platform's own.
    pub ui_font: String,
    pub ui_font_size: f32,
    /// Code font for the editor, diffs and terminals; empty for the default.
    pub code_font: String,
    pub code_font_size: f32,
    pub terminal_font_size: f32,
    pub corners: Corners,
    pub editor: EditorPreferences,
    /// Which palette dark mode used before themes had names: read once,
    /// to keep an old "Dark" choice looking the same.
    pub dark_theme: DarkTheme,
    pub chat_width: ChatWidth,
    /// Names under the tool icons on the right, not only in tooltips.
    pub tool_labels: bool,
    /// One color for focus, links, selection and highlights, over the
    /// theme's own.
    pub accent: Accent,
    /// How much room rows and spacing take.
    pub density: Density,
    /// Which default sizes these preferences were saved with (missing in
    /// ones saved before; see [`Preferences::migrate_sizes`]).
    #[serde(default)]
    pub sizes_version: u32,
    /// Fields this build does not know, kept as they were: ones a newer
    /// PilotHouse or an organisation's build added. Without this, saving
    /// here would drop them.
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// Default interface and code sizes, the same on every system. The
/// interface size is the rem: at 16, every step of the spacing and type
/// scales is a whole pixel (14 and 12 for text, 28 for a row, 4 for a gap),
/// also on a screen at 150 % or 200 %. Other sizes land between pixels, which
/// a screen at 100 % shows as rows and paddings a pixel off.
const DEFAULT_SIZES: (f32, f32) = (16.0, 14.0);

/// The default sizes of each earlier [`Preferences::sizes_version`], oldest
/// first: 14 and 13 before PilotHouse 0.2, then sizes of their own for
/// Windows (its fonts read smaller than macOS's at the same size).
const EARLIER_SIZES: [(f32, f32); 2] = [
    (14.0, 13.0),
    if cfg!(windows) {
        (18.0, 15.0)
    } else {
        (15.0, 14.0)
    },
];

/// The version of [`DEFAULT_SIZES`].
const SIZES_VERSION: u32 = EARLIER_SIZES.len() as u32;

/// The largest interface size (the whole UI scales with it).
pub const MAX_UI_FONT_SIZE: f32 = 24.0;

impl Default for Preferences {
    fn default() -> Self {
        let (ui_size, code_size) = DEFAULT_SIZES;
        Self {
            ui_font: String::new(),
            ui_font_size: ui_size,
            code_font: String::new(),
            code_font_size: code_size,
            terminal_font_size: code_size,
            corners: Corners::Rounded,
            editor: EditorPreferences::default(),
            dark_theme: DarkTheme::default(),
            chat_width: ChatWidth::Comfortable,
            tool_labels: false,
            accent: Accent::Theme,
            density: Density::Comfortable,
            sizes_version: SIZES_VERSION,
            other: serde_json::Map::new(),
        }
    }
}

/// One zoom step, in points: small, so a size that is just right can be
/// found (VS Code's steps are a fifth of the size).
pub const ZOOM_STEP: f32 = 0.5;

impl Preferences {
    /// Zooms in (`steps` > 0) or out: interface, code and terminal text grow
    /// or shrink together, each kept within its bounds.
    pub fn zoom(&mut self, steps: i32) {
        let by = steps as f32 * ZOOM_STEP;
        self.ui_font_size = (self.ui_font_size + by).clamp(11.0, MAX_UI_FONT_SIZE);
        self.code_font_size = (self.code_font_size + by).clamp(10.0, 24.0);
        self.terminal_font_size = (self.terminal_font_size + by).clamp(9.0, 24.0);
    }

    /// Back to the default sizes.
    pub fn reset_zoom(&mut self) {
        let (ui, code) = DEFAULT_SIZES;
        self.ui_font_size = ui;
        self.code_font_size = code;
        self.terminal_font_size = code;
    }

    /// Brings sizes saved with older defaults to the current ones; a size
    /// the user chose stays. True when something changed (save it).
    pub fn migrate_sizes(&mut self) -> bool {
        let Some(&(old_ui, old_code)) = EARLIER_SIZES.get(self.sizes_version as usize) else {
            return false;
        };
        let (ui, code) = DEFAULT_SIZES;
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
    /// Whether a file's changes are written without ⌘S.
    pub auto_save: AutoSave,
}

/// When a file with unsaved changes is written by itself.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoSave {
    /// A moment after typing stops.
    AfterPause,
    /// When the keyboard leaves the editor (another pane, another app).
    OnFocusLoss,
    /// Only on ⌘S. Also what a value this build does not know reads as (a
    /// newer build's, an organisation's): one unknown word must not cost
    /// the user every other preference. Last, as `serde(other)` asks.
    #[default]
    #[serde(other)]
    Off,
}

impl AutoSave {
    pub const ALL: [AutoSave; 3] = [AutoSave::Off, AutoSave::AfterPause, AutoSave::OnFocusLoss];

    pub fn label(self) -> &'static str {
        match self {
            AutoSave::Off => "Off",
            AutoSave::AfterPause => "After a pause",
            AutoSave::OnFocusLoss => "When leaving the editor",
        }
    }
}

impl Default for EditorPreferences {
    fn default() -> Self {
        Self {
            tab_size: 4,
            soft_wrap: false,
            line_numbers: true,
            indent_guides: true,
            folding: true,
            auto_save: AutoSave::Off,
        }
    }
}

/// The palette of dark mode: VS Code's Dark Modern greys, or a warm dark
/// brown that is softer on the eyes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DarkTheme {
    #[default]
    Classic,
    Warm,
}

impl DarkTheme {
    pub const ALL: [DarkTheme; 2] = [DarkTheme::Classic, DarkTheme::Warm];

    pub fn label(self) -> &'static str {
        match self {
            DarkTheme::Classic => "Modern",
            DarkTheme::Warm => "Warm",
        }
    }

    /// Its theme's name.
    pub fn theme_name(self) -> &'static str {
        match self {
            DarkTheme::Classic => "Modern Dark",
            DarkTheme::Warm => "Warm Dark",
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

/// The accent: the theme's own, or one of these over it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    #[default]
    Theme,
    Blue,
    Violet,
    Pink,
    Red,
    Orange,
    Amber,
    Green,
    Teal,
    Graphite,
}

impl Accent {
    pub const ALL: [Accent; 10] = [
        Accent::Theme,
        Accent::Blue,
        Accent::Violet,
        Accent::Pink,
        Accent::Red,
        Accent::Orange,
        Accent::Amber,
        Accent::Green,
        Accent::Teal,
        Accent::Graphite,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Accent::Theme => "Theme's own",
            Accent::Blue => "Blue",
            Accent::Violet => "Violet",
            Accent::Pink => "Pink",
            Accent::Red => "Red",
            Accent::Orange => "Orange",
            Accent::Amber => "Amber",
            Accent::Green => "Green",
            Accent::Teal => "Teal",
            Accent::Graphite => "Graphite",
        }
    }

    /// The color, in a shade that reads on a dark or a light theme; `None`
    /// for the theme's own.
    pub fn color(self, dark: bool) -> Option<Hsla> {
        let (on_dark, on_light) = match self {
            Accent::Theme => return None,
            Accent::Blue => (0x3b82f6, 0x2563eb),
            Accent::Violet => (0x8b5cf6, 0x7c3aed),
            Accent::Pink => (0xec4899, 0xdb2777),
            Accent::Red => (0xef4444, 0xdc2626),
            Accent::Orange => (0xf97316, 0xea580c),
            Accent::Amber => (0xf59e0b, 0xd97706),
            Accent::Green => (0x22c55e, 0x16a34a),
            Accent::Teal => (0x14b8a6, 0x0d9488),
            Accent::Graphite => (0x9ca3af, 0x4b5563),
        };
        Some(rgb(if dark { on_dark } else { on_light }).into())
    }
}

/// How much room rows and spacing take: the sidebar, the file tree, the
/// Git lists and the chat.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Compact,
    #[default]
    Comfortable,
    Spacious,
}

impl Density {
    pub const ALL: [Density; 3] = [Density::Compact, Density::Comfortable, Density::Spacious];

    pub fn label(self) -> &'static str {
        match self {
            Density::Compact => "Compact",
            Density::Comfortable => "Comfortable",
            Density::Spacious => "Spacious",
        }
    }

    /// Row heights and spacing are multiplied by this.
    pub fn factor(self) -> f32 {
        match self {
            Density::Compact => 0.85,
            Density::Comfortable => 1.0,
            Density::Spacious => 1.15,
        }
    }
}

/// How much room rows and spacing take now (1 for Comfortable). Cheap: for
/// render code.
pub fn density(cx: &App) -> f32 {
    cx.try_global::<Preferences>()
        .map_or(1.0, |preferences| preferences.density.factor())
}

/// `rems` scaled by the density: for row heights and spacing.
pub fn dense(rems: f32, cx: &App) -> gpui_kit::Rems {
    gpui_kit::rems(rems * density(cx))
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

    /// Widest the chat's reading column gets: in rems, so a line holds as
    /// many words at every interface size (640, 760 and 1000 pixels at the
    /// default one).
    pub fn max_width(self) -> gpui_kit::Rems {
        gpui_kit::rems(match self {
            ChatWidth::Narrow => 40.0,
            ChatWidth::Comfortable => 47.5,
            ChatWidth::Wide => 62.5,
        })
    }
}

/// Font families tried in order when the user picked none: the ones
/// PilotHouse carries, the same on every system, then each platform's own in
/// case those could not be loaded.
fn default_fonts() -> (&'static [&'static str], &'static [&'static str]) {
    use crate::fonts::{CODE, INTERFACE, SYSTEM_INTERFACE};
    if cfg!(target_os = "macos") {
        (&[INTERFACE, SYSTEM_INTERFACE], &[CODE, "SF Mono", "Menlo"])
    } else if cfg!(windows) {
        (
            &[INTERFACE, "Segoe UI Variable Text", "Segoe UI"],
            &[CODE, "Cascadia Code", "Cascadia Mono", "Consolas"],
        )
    } else {
        (
            &[INTERFACE, "Noto Sans", "Ubuntu", "Cantarell", "DejaVu Sans"],
            &[CODE, "Fira Code", "Ubuntu Mono", "DejaVu Sans Mono"],
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
    // From the theme's own colors every time, in a pass of its own, so the
    // edits below register as changes and reach the theme's tokens.
    // The theme shown is the selected one, or the preview of one being
    // designed.
    let config = crate::themes::current(cx);
    Theme::update(cx, |theme| theme.apply_config(&config));
    Theme::update(cx, |theme| {
        theme.font_family = ui_font.into();
        theme.font_size = px(preferences.ui_font_size.clamp(11.0, MAX_UI_FONT_SIZE));
        theme.mono_font_family = code_font.into();
        theme.mono_font_size = px(preferences.code_font_size.clamp(10.0, 24.0));
        theme.radius = px(radius);
        theme.radius_lg = px(radius_lg);

        let dark = theme.mode.is_dark();
        let colors = &mut theme.colors;
        // Accent, links and selection are the theme's own (a theme says
        // where the eye should go), unless the user chose an accent.
        if let Some(accent) = preferences.accent.color(dark) {
            apply_accent(colors, accent, dark);
        }
    });
    // The window is opaque. (It could be see-through, Mica, or show a
    // gradient or picture behind the frame before 0.7: five looks to keep
    // readable in every theme, for a setting few changed. Old values of
    // those settings are kept unread in `Preferences::other`.)
    if let Some(window) = window {
        window.set_background_appearance(WindowBackgroundAppearance::Opaque);
    }
}

/// Lays the user's accent over the theme: focus, links, selection,
/// highlights, progress; and the main buttons when the theme draws them in
/// its accent (Modern Dark's blue Send), not when they are its own color
/// (Modern Light's black).
fn apply_accent(colors: &mut gpui_kit::component::ThemeColor, accent: Hsla, dark: bool) {
    let on_accent = if accent.l > 0.62 {
        gpui_kit::hsla(0., 0., 0.08, 1.)
    } else {
        gpui_kit::hsla(0., 0., 1., 1.)
    };
    let shade = |by: f32| Hsla {
        l: (accent.l + by).clamp(0., 1.),
        ..accent
    };
    // Links: lighter on a dark theme, darker on a light one, so they read.
    let link = if dark {
        Hsla {
            l: accent.l.max(0.68),
            ..accent
        }
    } else {
        Hsla {
            l: accent.l.min(0.45),
            ..accent
        }
    };
    let primary_follows = colors.primary == colors.ring;
    colors.ring = accent;
    colors.info = accent;
    colors.info_foreground = on_accent;
    colors.drag_border = accent;
    colors.drop_target = accent.opacity(0.12);
    colors.slider_bar = accent;
    colors.progress_bar = accent;
    colors.selection = accent.opacity(if dark { 0.35 } else { 0.22 });
    colors.link = link;
    colors.link_hover = link.opacity(0.85);
    colors.link_active = link;
    colors.button_info = accent;
    colors.button_info_hover = shade(0.05);
    colors.button_info_active = shade(-0.05);
    colors.button_info_foreground = on_accent;
    if primary_follows {
        colors.primary = accent;
        colors.primary_hover = shade(0.05);
        colors.primary_active = shade(-0.05);
        colors.primary_foreground = on_accent;
        colors.sidebar_primary = accent;
        colors.sidebar_primary_foreground = on_accent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_auto_save_reads_as_off_and_costs_nothing_else() {
        let saved: Preferences = serde_json::from_str(
            r#"{"ui_font_size":20.0,"editor":{"tab_size":2,"auto_save":"on_window_change"}}"#,
        )
        .unwrap();
        assert_eq!(saved.editor.auto_save, AutoSave::Off);
        assert_eq!((saved.ui_font_size, saved.editor.tab_size), (20.0, 2));
        let saved: Preferences =
            serde_json::from_str(r#"{"editor":{"auto_save":"on_focus_loss"}}"#).unwrap();
        assert_eq!(saved.editor.auto_save, AutoSave::OnFocusLoss);
    }

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
        let mut zoomed = Preferences::default();
        zoomed.zoom(2);
        assert_eq!(zoomed.ui_font_size, DEFAULT_SIZES.0 + 1.0);
        zoomed.zoom(-100);
        assert_eq!(zoomed.ui_font_size, 11.0, "kept within bounds");
        zoomed.reset_zoom();
        assert_eq!(zoomed, Preferences::default());
        assert!(!saved.migrate_sizes(), "once");
        assert!(!Preferences::default().clone().migrate_sizes());
    }

    #[test]
    fn the_sizes_each_system_had_move_to_the_shared_ones() {
        let (ui, code) = EARLIER_SIZES[1];
        let mut saved = Preferences {
            ui_font_size: ui,
            code_font_size: code,
            terminal_font_size: 12.0,
            sizes_version: 1,
            ..Preferences::default()
        };
        assert!(saved.migrate_sizes());
        assert_eq!((saved.ui_font_size, saved.code_font_size), DEFAULT_SIZES);
        assert_eq!(
            saved.terminal_font_size, 12.0,
            "a size the user chose stays"
        );
        assert!(!saved.migrate_sizes(), "once");
    }

    #[test]
    fn accents_read_on_both_modes() {
        for accent in Accent::ALL {
            assert_eq!(accent.color(true).is_none(), accent == Accent::Theme);
        }
        assert_eq!(Density::default(), Density::Comfortable);
        assert_eq!(
            Density::Comfortable.factor(),
            1.0,
            "the default changes nothing"
        );
    }

    #[test]
    fn old_or_partial_settings_still_load() {
        let partial: Preferences =
            serde_json::from_str(r#"{"accent":"teal","code_font_size":15}"#).unwrap();
        assert_eq!(partial.code_font_size, 15.0);
        assert_eq!(partial.editor.tab_size, 4);
        // Saved by a version that had window backgrounds to choose from:
        // it loads, and what it chose is kept unread.
        let old: Preferences = serde_json::from_str(
            r#"{"window":"translucent","opacity":0.7,"gradient":"ocean","backdrop_image":"/tmp/a.jpg"}"#,
        )
        .unwrap();
        assert_eq!(old.other["window"], "translucent");
        assert_eq!(
            first_installed(&["Nope", "Menlo"], &["Menlo".to_string()]),
            "Menlo"
        );
        assert_eq!(first_installed(&["Nope"], &[]), "Nope");
    }

    #[test]
    fn fields_this_build_does_not_know_are_kept() {
        // Saved by a newer build or an organisation's: "x.acme.sidebar_icon"
        // means nothing here, and must survive this build saving.
        let saved = r#"{"accent":"teal","x.acme.sidebar_icon":{"name":"rocket"},"future":3}"#;
        let mut preferences: Preferences = serde_json::from_str(saved).unwrap();
        assert_eq!(preferences.accent, Accent::Teal);
        preferences.zoom(1);
        let again: serde_json::Value =
            serde_json::to_value(&preferences).expect("preferences serialize");
        assert_eq!(again["x.acme.sidebar_icon"]["name"], "rocket");
        assert_eq!(again["future"], 3);
        assert_eq!(again["accent"], "teal");
        // Known fields are not duplicated into the leftovers.
        assert!(!preferences.other.contains_key("accent"));
        // Defaults carry none.
        assert!(
            serde_json::to_value(Preferences::default())
                .unwrap()
                .as_object()
                .unwrap()
                .keys()
                .all(|key| !key.starts_with("x."))
        );
    }
}
