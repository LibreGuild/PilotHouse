//! The themes to choose from: PilotHouse's own, and the user's own (theme
//! mods), each a JSON file in a folder of the data folder.
//!
//! One theme is selected: the user's choice, or the system's light or dark
//! one. While an agent designs a theme with the user, a preview shows
//! instead, change by change, until it is saved (a file of its own, then
//! selected) or dropped.
//!
//! A theme file is gpui-component's theme set: a name and a list of themes,
//! each with a mode, its colors (`colors`) and its editor colors
//! (`highlight`, with `syntax`), and PilotHouse's own addition: the
//! terminal's 16 colors (`terminal`, by name; a theme without them keeps
//! the terminal's palette for its mode). Fonts, corners and translucency
//! are preferences of their own ([`crate::Preferences`]), kept whichever
//! theme is chosen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result, bail};
use gpui_kit::component::{ThemeConfig, ThemeSet};
use gpui_kit::{App, Global, Hsla, SharedString};
use serde_json::Value;

/// PilotHouse's own themes, compiled in.
const BUNDLED: &str = include_str!("../themes/pilothouse.json");

/// The themes matching the system's light and dark modes.
pub const DEFAULT_LIGHT: &str = "Modern Light";
pub const DEFAULT_DARK: &str = "Modern Dark";

/// Light and dark versions of the same theme, for switching between them
/// (the command palette's light and dark appearance).
const PAIRS: &[(&str, &str)] = &[
    ("Modern Light", "Modern Dark"),
    ("Warm Light", "Warm Dark"),
    ("One Light", "One Dark"),
];

/// The terminal's colors in a theme file, in ANSI order: the eight, then
/// their bright versions.
pub const TERMINAL_COLOR_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_white",
];

/// A theme's terminal colors; one it leaves out keeps the default.
pub type TerminalColors = [Option<Hsla>; 16];

/// A theme the user made, and the file it is in.
struct UserTheme {
    config: Rc<ThemeConfig>,
    path: PathBuf,
}

#[derive(Default)]
struct Themes {
    bundled: Vec<Rc<ThemeConfig>>,
    user: Vec<UserTheme>,
    /// Themes' terminal colors, by theme name.
    terminal: HashMap<SharedString, TerminalColors>,
    /// Where the user's themes are kept.
    dir: Option<PathBuf>,
    selected: Option<Rc<ThemeConfig>>,
    preview: Option<Rc<ThemeConfig>>,
    preview_terminal: Option<TerminalColors>,
}

impl Global for Themes {}

/// The themes of a theme file, each with its terminal colors.
fn parse_set(text: &str) -> Result<Vec<(ThemeConfig, Option<TerminalColors>)>> {
    let value: Value = serde_json::from_str(text)?;
    let set: ThemeSet = serde_json::from_value(value.clone())?;
    Ok(set
        .themes
        .into_iter()
        .enumerate()
        .map(|(ix, theme)| {
            let terminal = value["themes"][ix]
                .get("terminal")
                .and_then(|terminal| parse_terminal(terminal).ok());
            (theme, terminal)
        })
        .collect())
}

/// Terminal colors by name (`red`, `bright_blue`…), checked.
pub fn parse_terminal(value: &Value) -> Result<TerminalColors> {
    let Some(map) = value.as_object() else {
        bail!("`terminal` is colors by name: {{\"red\": \"#e06c75\", ...}}");
    };
    let mut colors: TerminalColors = [None; 16];
    for (name, color) in map {
        let Some(ix) = TERMINAL_COLOR_NAMES.iter().position(|known| known == name) else {
            bail!(
                "no terminal color `{name}`: {}",
                TERMINAL_COLOR_NAMES.join(", ")
            );
        };
        if color.is_null() {
            continue;
        }
        let parsed = color
            .as_str()
            .and_then(|color| gpui_kit::component::try_parse_color(color).ok())
            .with_context(|| format!("terminal.{name} is not a color (#rrggbb)"))?;
        colors[ix] = Some(parsed);
    }
    Ok(colors)
}

/// Terminal colors as a theme file writes them.
pub fn terminal_json(colors: &TerminalColors) -> Value {
    let map: serde_json::Map<String, Value> = TERMINAL_COLOR_NAMES
        .iter()
        .zip(colors)
        .filter_map(|(name, color)| {
            color.map(|color| {
                (
                    name.to_string(),
                    serde_json::to_value(color).unwrap_or_default(),
                )
            })
        })
        .collect();
    Value::Object(map)
}

pub(crate) fn init(cx: &mut App) {
    let parsed = parse_set(BUNDLED).unwrap_or_else(|err| {
        // Covered by a test: only while the file is edited by hand.
        tracing::error!("the bundled themes do not parse: {err:#}");
        Vec::new()
    });
    let mut state = Themes::default();
    for (theme, terminal) in parsed {
        if let Some(terminal) = terminal {
            state.terminal.insert(theme.name.clone(), terminal);
        }
        state.bundled.push(Rc::new(theme));
    }
    cx.set_global(state);
}

fn themes(cx: &App) -> &Themes {
    cx.global::<Themes>()
}

fn themes_mut(cx: &mut App) -> &mut Themes {
    cx.global_mut::<Themes>()
}

/// Keeps the user's themes in `dir` and reads them.
pub fn set_user_dir(dir: PathBuf, cx: &mut App) {
    themes_mut(cx).dir = Some(dir);
    reload_user_themes(cx);
}

/// The folder of the user's themes, if set.
pub fn user_dir(cx: &App) -> Option<PathBuf> {
    themes(cx).dir.clone()
}

/// Reads the user's themes again (a file was added, changed or removed).
pub fn reload_user_themes(cx: &mut App) {
    let Some(dir) = themes(cx).dir.clone() else {
        return;
    };
    let bundled: Vec<SharedString> = themes(cx)
        .bundled
        .iter()
        .map(|theme| theme.name.clone())
        .collect();
    let read = read_user_themes(&dir, &bundled);
    let state = themes_mut(cx);
    state.terminal.retain(|name, _| bundled.contains(name));
    state.user = Vec::new();
    for (theme, terminal) in read {
        if let Some(terminal) = terminal {
            state.terminal.insert(theme.config.name.clone(), terminal);
        }
        state.user.push(theme);
    }
    // The selected theme as its file has it now.
    if let Some(name) = themes(cx).selected.as_ref().map(|theme| theme.name.clone())
        && let Some(fresh) = find(&name, cx)
    {
        themes_mut(cx).selected = Some(fresh);
    }
}

fn read_user_themes(
    dir: &Path,
    bundled: &[SharedString],
) -> Vec<(UserTheme, Option<TerminalColors>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    let mut user: Vec<(UserTheme, Option<TerminalColors>)> = Vec::new();
    for path in paths {
        let set = std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|text| parse_set(&text));
        let set = match set {
            Ok(set) => set,
            Err(err) => {
                tracing::warn!("skipping the theme file {}: {err:#}", path.display());
                continue;
            }
        };
        for (theme, terminal) in set {
            // PilotHouse's own names stay PilotHouse's.
            if bundled.contains(&theme.name)
                || user
                    .iter()
                    .any(|(known, _)| known.config.name == theme.name)
            {
                tracing::warn!(
                    "skipping the theme `{}` in {}: the name is taken",
                    theme.name,
                    path.display()
                );
                continue;
            }
            user.push((
                UserTheme {
                    config: Rc::new(theme),
                    path: path.clone(),
                },
                terminal,
            ));
        }
    }
    user.sort_by_key(|(theme, _)| theme.config.name.to_lowercase());
    user
}

/// Every theme: PilotHouse's, then the user's, each with whether it is the
/// user's own.
pub fn list(cx: &App) -> Vec<(Rc<ThemeConfig>, bool)> {
    let state = themes(cx);
    state
        .bundled
        .iter()
        .map(|theme| (theme.clone(), false))
        .chain(state.user.iter().map(|theme| (theme.config.clone(), true)))
        .collect()
}

pub fn find(name: &str, cx: &App) -> Option<Rc<ThemeConfig>> {
    let state = themes(cx);
    state
        .bundled
        .iter()
        .find(|theme| theme.name == name)
        .cloned()
        .or_else(|| {
            state
                .user
                .iter()
                .find(|theme| theme.config.name == name)
                .map(|theme| theme.config.clone())
        })
}

/// Whether `name` is one of the user's own themes.
pub fn is_user_theme(name: &str, cx: &App) -> bool {
    themes(cx)
        .user
        .iter()
        .any(|theme| theme.config.name == name)
}

/// Selects the theme `name`; one that is gone gives way to the default of
/// its mode (`dark` when not known). Returns the theme now selected.
pub(crate) fn select(name: &str, dark: bool, cx: &mut App) -> Rc<ThemeConfig> {
    let theme = find(name, cx)
        .or_else(|| find(if dark { DEFAULT_DARK } else { DEFAULT_LIGHT }, cx))
        .unwrap_or_else(|| Rc::new(ThemeConfig::default()));
    themes_mut(cx).selected = Some(theme.clone());
    theme
}

/// The selected theme (not the preview).
pub fn selected(cx: &App) -> Option<Rc<ThemeConfig>> {
    themes(cx).selected.clone()
}

/// The theme shown: the preview while there is one, else the selected one.
pub fn current(cx: &App) -> Rc<ThemeConfig> {
    let state = themes(cx);
    state
        .preview
        .clone()
        .or_else(|| state.selected.clone())
        .or_else(|| {
            state
                .bundled
                .iter()
                .find(|theme| theme.name == DEFAULT_DARK)
                .cloned()
        })
        .unwrap_or_else(|| Rc::new(ThemeConfig::default()))
}

pub fn preview(cx: &App) -> Option<Rc<ThemeConfig>> {
    themes(cx).preview.clone()
}

/// Shows `theme` (with its terminal colors) instead of the selected one
/// (`None`: the selected one again). Apply the preferences afterwards to
/// see it.
pub fn set_preview(theme: Option<(ThemeConfig, Option<TerminalColors>)>, cx: &mut App) {
    let state = themes_mut(cx);
    match theme {
        Some((theme, terminal)) => {
            state.preview = Some(Rc::new(theme));
            state.preview_terminal = terminal;
        }
        None => {
            state.preview = None;
            state.preview_terminal = None;
        }
    }
}

/// The terminal colors of the theme shown (the preview's while there is
/// one); `None` keeps the terminal's palette for the mode.
pub fn terminal_colors(cx: &App) -> Option<TerminalColors> {
    let state = themes(cx);
    if state.preview.is_some() {
        return state.preview_terminal;
    }
    let name = &state.selected.as_ref()?.name;
    state.terminal.get(name).copied()
}

/// The terminal colors of the theme `name`.
pub fn terminal_of(name: &str, cx: &App) -> Option<TerminalColors> {
    themes(cx).terminal.get(name).copied()
}

/// Saves `theme` (with its terminal colors) as one of the user's,
/// replacing the user's theme of the same name, and returns it as saved.
pub fn save(
    theme: ThemeConfig,
    terminal: Option<TerminalColors>,
    cx: &mut App,
) -> Result<Rc<ThemeConfig>> {
    let name = theme.name.trim().to_string();
    if name.is_empty() {
        bail!("give the theme a name");
    }
    if themes(cx).bundled.iter().any(|known| known.name == name) {
        bail!("`{name}` is one of PilotHouse's own themes: save under another name");
    }
    let dir = themes(cx)
        .dir
        .clone()
        .context("the themes folder is not set up")?;
    std::fs::create_dir_all(&dir)?;
    let path = themes(cx)
        .user
        .iter()
        .find(|known| known.config.name == name)
        .map(|known| known.path.clone())
        .unwrap_or_else(|| unused_path(&dir, &slug(&name)));
    let mut theme = theme;
    theme.name = name.clone().into();
    theme.is_default = false;
    let set = ThemeSet {
        name: name.clone().into(),
        themes: vec![theme],
        ..Default::default()
    };
    let mut value = serde_json::to_value(&set)?;
    if let Some(terminal) = &terminal
        && let Some(theme) = value["themes"].get_mut(0).and_then(Value::as_object_mut)
    {
        theme.insert("terminal".into(), terminal_json(terminal));
    }
    let mut text = serde_json::to_string_pretty(&value)?;
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    reload_user_themes(cx);
    find(&name, cx).context("the saved theme did not read back")
}

/// Removes the user's theme `name` (its file, when it holds only it).
pub fn delete(name: &str, cx: &mut App) -> Result<()> {
    let Some(path) = themes(cx)
        .user
        .iter()
        .find(|theme| theme.config.name == name)
        .map(|theme| theme.path.clone())
    else {
        bail!("`{name}` is not one of your themes");
    };
    let others = themes(cx)
        .user
        .iter()
        .filter(|theme| theme.path == path && theme.config.name != name)
        .count();
    if others > 0 {
        bail!("{} holds other themes too: edit it by hand", path.display());
    }
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    reload_user_themes(cx);
    Ok(())
}

/// A VS Code color theme as a PilotHouse theme (see [`crate::vscode`]),
/// over Modern Dark or Modern Light for what it leaves out.
pub fn import_vscode(
    text: &str,
    name: Option<&str>,
    cx: &App,
) -> Result<(ThemeConfig, Option<TerminalColors>)> {
    let dark = find(DEFAULT_DARK, cx).context("Modern Dark is missing")?;
    let light = find(DEFAULT_LIGHT, cx).context("Modern Light is missing")?;
    crate::vscode::import(text, name, &dark, &light)
}

/// The theme's light or dark version: its pair, else the default of the
/// other mode.
pub fn partner(name: &str, dark: bool) -> &str {
    PAIRS
        .iter()
        .find_map(|(light, dark_one)| {
            if *light == name {
                Some(*dark_one)
            } else if *dark_one == name {
                Some(*light)
            } else {
                None
            }
        })
        .unwrap_or(if dark { DEFAULT_LIGHT } else { DEFAULT_DARK })
}

/// A file name for a theme: its name in lower case, words joined by `-`.
fn slug(name: &str) -> String {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if words.is_empty() {
        "theme".into()
    } else {
        words.join("-")
    }
}

fn unused_path(dir: &Path, slug: &str) -> PathBuf {
    let mut path = dir.join(format!("{slug}.json"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{slug}-{n}.json"));
        n += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_themes_load_whole() {
        let set: ThemeSet = serde_json::from_str(BUNDLED).expect("the bundled themes parse");
        let names: Vec<&str> = set.themes.iter().map(|theme| theme.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "Modern Dark",
                "Modern Light",
                "Warm Dark",
                "Warm Light",
                "Dracula",
                "One Dark",
                "One Light",
                "Studio Dark"
            ]
        );
        for theme in &set.themes {
            assert!(
                theme.highlight.is_some(),
                "{} has editor colors",
                theme.name
            );
            // Every color is one gpui-component reads (no typo falls back
            // to its default unseen).
            let colors = serde_json::to_value(&theme.colors).unwrap();
            for (key, value) in colors.as_object().unwrap() {
                if let Some(value) = value.as_str() {
                    assert!(
                        gpui_kit::component::try_parse_color(value).is_ok(),
                        "{}: {key} = {value}",
                        theme.name
                    );
                }
            }
        }
        for (light, dark) in PAIRS {
            assert!(names.contains(light) && names.contains(dark));
            assert_eq!(partner(light, false), *dark);
            assert_eq!(partner(dark, true), *light);
        }
        assert_eq!(partner("Dracula", true), DEFAULT_LIGHT);
        // Terminal colors: the new themes bring theirs; the old ones keep
        // the terminal as it was.
        for (theme, terminal) in parse_set(BUNDLED).unwrap() {
            let old = ["Modern Dark", "Modern Light", "Warm Dark"].contains(&theme.name.as_ref());
            assert_eq!(terminal.is_none(), old, "{}", theme.name);
            if let Some(terminal) = terminal {
                assert!(
                    terminal.iter().all(Option::is_some),
                    "{} has all 16",
                    theme.name
                );
            }
        }
    }

    #[test]
    fn terminal_colors_are_checked_and_written_back() {
        let colors =
            parse_terminal(&serde_json::json!({ "red": "#ff0000", "bright_white": "#ffffff" }))
                .unwrap();
        assert!(colors[1].is_some() && colors[15].is_some() && colors[0].is_none());
        let written = terminal_json(&colors);
        assert_eq!(written["red"], "#ff0000ff");
        assert!(written.get("black").is_none());
        assert!(parse_terminal(&serde_json::json!({ "purple": "#ff0000" })).is_err());
        assert!(parse_terminal(&serde_json::json!({ "red": "crimson-ish" })).is_err());
    }

    #[test]
    fn theme_files_are_named_after_their_theme() {
        assert_eq!(slug("Sunset Ink (mine)"), "sunset-ink-mine");
        assert_eq!(slug("  "), "theme");
        let dir = std::env::temp_dir().join(format!("ph-theme-slug-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), "{}").unwrap();
        assert_eq!(unused_path(&dir, "a"), dir.join("a-2.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_themes_never_take_a_bundled_name() {
        let dir = std::env::temp_dir().join(format!("ph-theme-user-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let theme = |name: &str| {
            format!(
                r##"{{"name": "x", "themes": [{{"name": "{name}", "mode": "dark", "colors": {{"background": "#101010"}}}}]}}"##
            )
        };
        std::fs::write(dir.join("mine.json"), theme("Night Owl")).unwrap();
        std::fs::write(dir.join("clash.json"), theme("Dracula")).unwrap();
        std::fs::write(dir.join("broken.json"), "{").unwrap();
        let read = read_user_themes(&dir, &["Dracula".into()]);
        let names: Vec<&str> = read
            .iter()
            .map(|(theme, _)| theme.config.name.as_ref())
            .collect();
        assert_eq!(names, ["Night Owl"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
