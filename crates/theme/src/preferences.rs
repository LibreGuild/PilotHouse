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
    pub window: WindowStyle,
    /// How opaque panels are when the window is translucent (0.55–1.0).
    pub opacity: f32,
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
    /// The gradient behind the frame ([`WindowStyle::Gradient`]).
    pub gradient: Gradient,
    /// The image behind the frame ([`WindowStyle::Image`]): PilotHouse's
    /// own copy, made to fit a screen.
    pub backdrop_image: String,
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
            ui_font: String::new(),
            ui_font_size: ui_size,
            code_font: String::new(),
            code_font_size: code_size,
            terminal_font_size: code_size,
            corners: Corners::Rounded,
            window: WindowStyle::Solid,
            opacity: 0.86,
            editor: EditorPreferences::default(),
            dark_theme: DarkTheme::default(),
            chat_width: ChatWidth::Comfortable,
            tool_labels: false,
            accent: Accent::Theme,
            density: Density::Comfortable,
            gradient: Gradient::Dusk,
            backdrop_image: String::new(),
            sizes_version: SIZES_VERSION,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowStyle {
    Solid,
    /// See-through panels over a blurred desktop.
    Translucent,
    /// Windows 11's Mica material.
    Mica,
    /// A gradient behind the frame ([`Preferences::gradient`]).
    Gradient,
    /// An image behind the frame ([`Preferences::backdrop_image`]).
    Image,
}

impl WindowStyle {
    /// The styles this platform can show.
    pub fn available() -> Vec<WindowStyle> {
        let mut styles = vec![WindowStyle::Solid, WindowStyle::Translucent];
        if cfg!(windows) {
            styles.push(WindowStyle::Mica);
        }
        styles.extend([WindowStyle::Gradient, WindowStyle::Image]);
        styles
    }

    pub fn label(self) -> &'static str {
        match self {
            WindowStyle::Solid => "Solid",
            WindowStyle::Translucent => "Translucent",
            WindowStyle::Mica => "Mica",
            WindowStyle::Gradient => "Gradient",
            WindowStyle::Image => "Image",
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

/// A gradient behind the frame: deep on a dark theme, soft on a light one.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gradient {
    #[default]
    Dusk,
    Ocean,
    Aurora,
    Ember,
    Graphite,
}

impl Gradient {
    pub const ALL: [Gradient; 5] = [
        Gradient::Dusk,
        Gradient::Ocean,
        Gradient::Aurora,
        Gradient::Ember,
        Gradient::Graphite,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Gradient::Dusk => "Dusk",
            Gradient::Ocean => "Ocean",
            Gradient::Aurora => "Aurora",
            Gradient::Ember => "Ember",
            Gradient::Graphite => "Graphite",
        }
    }

    /// Its two colors, from the top left to the bottom right.
    pub fn colors(self, dark: bool) -> (Hsla, Hsla) {
        let (from, to) = match (self, dark) {
            (Gradient::Dusk, true) => (0x1e1b4b, 0x701a75),
            (Gradient::Dusk, false) => (0xe0e7ff, 0xfae8ff),
            (Gradient::Ocean, true) => (0x0c1e3a, 0x0f4c5c),
            (Gradient::Ocean, false) => (0xdbeafe, 0xccfbf1),
            (Gradient::Aurora, true) => (0x064e3b, 0x4c1d95),
            (Gradient::Aurora, false) => (0xd1fae5, 0xede9fe),
            (Gradient::Ember, true) => (0x431407, 0x831843),
            (Gradient::Ember, false) => (0xffedd5, 0xffe4e6),
            (Gradient::Graphite, true) => (0x111827, 0x374151),
            (Gradient::Graphite, false) => (0xf3f4f6, 0xd1d5db),
        };
        (rgb(from).into(), rgb(to).into())
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
    // An image style without an image is solid.
    let style = match preferences.window {
        WindowStyle::Image if preferences.backdrop_image.is_empty() => WindowStyle::Solid,
        style => style,
    };
    let painted = matches!(style, WindowStyle::Gradient | WindowStyle::Image);
    let translucent = style != WindowStyle::Solid;
    let alpha = preferences.opacity.clamp(0.55, 1.0);
    let mut frame = None;
    // From the theme's own colors every time: the edits below scale
    // alphas, and on top of an earlier pass they would compound (more
    // transparent with each change, never solid again). A pass of its own,
    // so the edits below register as changes and reach the theme's tokens.
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
        frame = Some((colors.sidebar, dark));
        if painted {
            // A gradient or image shows through the frame (under a veil of
            // the frame's color, so its text reads); the panels keep the
            // chosen opacity over it.
            colors.background = colors.background.opacity(alpha);
            for color in [
                &mut colors.sidebar,
                &mut colors.title_bar,
                &mut colors.status_bar,
                &mut colors.tab_bar,
                &mut colors.list,
            ] {
                *color = color.opacity(0.);
            }
        } else if translucent {
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
    let backdrop = match (style, frame) {
        (WindowStyle::Gradient, Some((frame, dark))) => {
            let (from, to) = preferences.gradient.colors(dark);
            Some(Backdrop {
                paint: BackdropPaint::Gradient { from, to },
                veil: frame.opacity(if dark { 0.55 } else { 0.5 }),
            })
        }
        // A picture is busier than a gradient: a thicker veil keeps the
        // frame's text readable over it.
        (WindowStyle::Image, Some((frame, dark))) => Some(Backdrop {
            paint: BackdropPaint::Image(
                std::path::PathBuf::from(&preferences.backdrop_image).into(),
            ),
            veil: frame.opacity(if dark { 0.72 } else { 0.66 }),
        }),
        _ => None,
    };
    cx.set_global(CurrentBackdrop(backdrop));
    if let Some((frame, _)) = frame {
        cx.set_global(OpaqueFrame(frame));
    }
    if let Some(window) = window {
        window.set_background_appearance(match style {
            WindowStyle::Solid | WindowStyle::Gradient | WindowStyle::Image => {
                WindowBackgroundAppearance::Opaque
            }
            WindowStyle::Translucent => WindowBackgroundAppearance::Blurred,
            WindowStyle::Mica => WindowBackgroundAppearance::MicaBackdrop,
        });
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

/// What is painted behind the frame, under its veil.
#[derive(Clone)]
pub enum BackdropPaint {
    Gradient { from: Hsla, to: Hsla },
    Image(std::sync::Arc<std::path::Path>),
}

#[derive(Clone)]
pub struct Backdrop {
    pub paint: BackdropPaint,
    /// The frame's own color, see-through: over the picture so the frame's
    /// text reads.
    pub veil: Hsla,
}

#[derive(Default)]
struct CurrentBackdrop(Option<Backdrop>);

impl Global for CurrentBackdrop {}

/// The theme's frame color as the theme has it: opaque even when the frame
/// lets the desktop or a picture through.
struct OpaqueFrame(Hsla);

impl Global for OpaqueFrame {}

/// The frame's color, always opaque: for what sits over the window on its
/// own (dialogs drawn like the frame).
pub fn opaque_frame(cx: &App) -> Hsla {
    cx.try_global::<OpaqueFrame>().map_or_else(
        || gpui_kit::component::ActiveTheme::theme(cx).sidebar,
        |frame| frame.0,
    )
}

/// The largest backdrop kept: a screen's worth, so a camera photo costs a
/// screen's memory, not its own.
const BACKDROP_MAX: (u32, u32) = (2560, 1600);

/// Makes PilotHouse's copy of an image for behind the frame, in `dir`:
/// scaled to fit a screen, saved as JPEG; earlier copies go. Slow on a big
/// photo: run it off the UI thread.
pub fn prepare_backdrop_image(
    source: &std::path::Path,
    dir: &std::path::Path,
) -> anyhow::Result<std::path::PathBuf> {
    use anyhow::Context as _;
    let image = image::ImageReader::open(source)
        .with_context(|| format!("opening {}", source.display()))?
        .with_guessed_format()?
        .decode()
        .with_context(|| {
            format!(
                "{} is not an image PilotHouse can read (PNG, JPEG, WebP, GIF, BMP)",
                source.display()
            )
        })?;
    let (width, height) = BACKDROP_MAX;
    let image = if image.width() > width || image.height() > height {
        image.resize(width, height, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    std::fs::create_dir_all(dir)?;
    // A new name each time: the old one may still be shown (and cached).
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default();
    let target = dir.join(format!("backdrop-{stamp}.jpg"));
    image
        .to_rgb8()
        .save_with_format(&target, image::ImageFormat::Jpeg)
        .with_context(|| format!("writing {}", target.display()))?;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path != target
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("backdrop-"))
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(target)
}

/// The gradient or image behind the frame, if the user chose one.
pub fn backdrop(cx: &App) -> Option<&Backdrop> {
    cx.try_global::<CurrentBackdrop>()?.0.as_ref()
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
    fn a_backdrop_image_is_kept_at_a_screen_size() {
        let dir = std::env::temp_dir().join(format!("ph-backdrop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("photo.png");
        image::RgbImage::from_pixel(4000, 3000, image::Rgb([30, 60, 90]))
            .save(&source)
            .unwrap();
        let backdrops = dir.join("backdrops");
        let first = prepare_backdrop_image(&source, &backdrops).unwrap();
        let (width, height) = image::image_dimensions(&first).unwrap();
        assert!(
            width <= BACKDROP_MAX.0 && height <= BACKDROP_MAX.1,
            "{width}x{height}"
        );
        assert!(
            (width as f32 / height as f32 - 4. / 3.).abs() < 0.01,
            "the shape is kept"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = prepare_backdrop_image(&source, &backdrops).unwrap();
        assert!(!first.exists() && second.exists(), "the earlier copy goes");
        std::fs::write(dir.join("notes.txt"), "not an image").unwrap();
        assert!(prepare_backdrop_image(&dir.join("notes.txt"), &backdrops).is_err());
        let _ = std::fs::remove_dir_all(&dir);
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
        assert_eq!(Preferences::default().window, WindowStyle::Solid);
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
        assert_eq!(partial.code_font_size, 15.0);
        assert_eq!(partial.editor.tab_size, 4);
        assert_eq!(
            first_installed(&["Nope", "Menlo"], &["Menlo".to_string()]),
            "Menlo"
        );
        assert_eq!(first_installed(&["Nope"], &[]), "Nope");
    }
}
