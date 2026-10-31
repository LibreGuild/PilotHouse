//! Theme mods: the user describes a look in a chat, the agent changes the
//! theme as they talk (a preview, shown at once in every window), and saves
//! it as one of the user's themes. The `theme.*` control methods behind the
//! MCP bridge's `theme_*` tools.

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::component::ThemeConfig;
use gpui_kit::{App, AppContext as _};
use ph_control::ControlServer;
use ph_theme::{Appearance, themes};
use serde_json::{Map, Value, json};

use crate::AppState;

/// Where PilotHouse takes its colors from, for the agent.
const GUIDE: &str = "\
Where PilotHouse takes its colors from (`colors`):
- sidebar.background: the window's frame: the left sidebar, the strip of tools on the right and around the panels. title_bar.background, status_bar.background and tab_bar.background are part of it: change them together.
- background: the panels on the frame (chat, editor, files, the settings page) and dialogs; border: their outlines and the lines between things.
- foreground: text; muted.foreground: secondary text (descriptions, times, placeholders).
- muted.background: the user's messages in a chat, segmented switches, code in text.
- popover.background / popover.foreground: menus, pop-ups and dropdowns.
- list.hover.background / list.active.background: a row under the pointer / the selected row (sidebar, lists, menus).
- primary.background / primary.foreground (and .hover / .active): filled main buttons (Send, Add).
- secondary.background (and .hover / .active): plain buttons; input.border: text fields' outline.
- ring: the focus outline; info.background: highlights; link: links; selection.background: selected text.
- accent.background / accent.foreground: hovered menu items and tabs.
- tab.active.background / tab.active.foreground: the open tab; tab.foreground: the others.
- success / warning / danger (.background, .foreground): status badges, errors.
- scrollbar.thumb.background (and .hover): scrollbars. caret: the text cursor.
- base.red, base.green, base.blue, base.yellow, base.magenta, base.cyan (and .light): colored marks and charts.
The code editor (`highlight`): editor.background, editor.foreground, editor.active_line.background, editor.line_number, editor.active_line_number; created, modified, deleted (change bars, diffs); syntax.<name>.color for code: keyword, string, function, type, comment, number, constant, property, variable, attribute, tag, title, operator, punctuation; a syntax entry can also set font_style (italic) and font_weight (700 for bold).
The terminal (`terminal`, beside `colors`): its 16 colors by name: black, red, green, yellow, blue, magenta, cyan, white, and bright_ of each; a theme without them keeps the terminal's usual palette.
Change bars beside the editor's line numbers use success (added), info (changed) and danger (deleted).
Colors are #rrggbb or #rrggbbaa. A light theme has mode \"light\", a dark one \"dark\". Fonts, sizes, corners, the accent color, density and the window's background are the user's settings, not part of a theme.";

pub fn register_control_methods(cx: &mut App) {
    ControlServer::handle("theme.get", |_, cx| get(cx), cx);
    ControlServer::handle("theme.preview", preview, cx);
    ControlServer::handle("theme.save", save, cx);
    ControlServer::handle("theme.discard", |_, cx| discard(cx), cx);
    ControlServer::handle("theme.list", |_, cx| list(cx), cx);
    ControlServer::handle("theme.use", use_theme, cx);
    ControlServer::handle("theme.delete", delete, cx);
    ControlServer::handle("theme.import", import, cx);
}

/// Tells the user in the main window (an action started from Settings
/// that ended later).
fn tell(notification: gpui_kit::component::notification::Notification, cx: &mut App) {
    use gpui_kit::component::WindowExt as _;
    if let Some((handle, _)) = crate::main_workspace(cx) {
        let _ = handle.update(cx, |_, window, cx| {
            window.push_notification(notification, cx)
        });
    }
}

/// Asks for a VS Code color theme file and imports it: saved as one of
/// the user's themes, and used.
pub(crate) fn import_vscode_file(cx: &mut App) {
    use gpui_kit::component::notification::Notification;
    let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Import".into()),
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = paths.await else {
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        let read = cx
            .background_spawn(async move { std::fs::read_to_string(&path) })
            .await;
        cx.update(|cx| {
            let imported = read
                .map_err(anyhow::Error::from)
                .and_then(|text| themes::import_vscode(&text, None, cx))
                .and_then(|(theme, terminal)| themes::save(theme, terminal, cx));
            match imported {
                Ok(saved) => {
                    AppState::set_appearance(Appearance::Theme(saved.name.to_string()), cx);
                    tell(
                        Notification::success(format!("Imported “{}”.", saved.name)),
                        cx,
                    );
                }
                Err(err) => tell(
                    Notification::warning(format!("That theme could not be imported: {err:#}")),
                    cx,
                ),
            }
        });
    })
    .detach();
}

/// Asks for an image to show behind the frame; PilotHouse's own copy
/// (made to fit a screen, off the UI thread) is what shows.
pub(crate) fn choose_backdrop(cx: &mut App) {
    use gpui_kit::component::notification::Notification;
    let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Use as background".into()),
    });
    let dir = ph_store::paths::data_dir().join("backdrops");
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = paths.await else {
            return;
        };
        let Some(source) = paths.into_iter().next() else {
            return;
        };
        let prepared = cx
            .background_spawn(async move {
                ph_theme::preferences::prepare_backdrop_image(&source, &dir)
            })
            .await;
        cx.update(|cx| match prepared {
            Ok(path) => AppState::update_preferences(cx, |preferences| {
                preferences.window = ph_theme::WindowStyle::Image;
                preferences.backdrop_image = path.to_string_lossy().into_owned();
            }),
            Err(err) => tell(Notification::warning(format!("{err:#}")), cx),
        });
    })
    .detach();
}

fn reply(text: impl Into<String>) -> Result<Value> {
    Ok(json!({ "text": text.into() }))
}

fn name(params: &Value) -> Result<String> {
    params["name"]
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .context("give the theme's `name`")
}

fn get(cx: &mut App) -> Result<Value> {
    let current = themes::current(cx);
    let previewing = themes::preview(cx).is_some();
    let mut value = serde_json::to_value(&*current)?;
    if let Some(terminal) = themes::terminal_colors(cx) {
        value["terminal"] = themes::terminal_json(&terminal);
    }
    let json = serde_json::to_string_pretty(&value)?;
    reply(format!(
        "{}{}\n\n{json}\n\n{GUIDE}",
        if previewing {
            "A preview is shown (not saved yet), made from "
        } else {
            "The theme in use: "
        },
        current.name
    ))
}

fn preview(params: Value, cx: &mut App) -> Result<Value> {
    let base = match params["from"].as_str() {
        Some(from) => themes::find(from, cx)
            .with_context(|| format!("no theme `{from}` (theme_list shows them)"))?,
        None => themes::current(cx),
    };
    let changes = match &params["changes"] {
        Value::Null => Value::Object(Map::new()),
        Value::Object(changes) => Value::Object(changes.clone()),
        _ => bail!("`changes` is part of a theme: an object such as {{\"colors\": {{...}}}}"),
    };
    // The terminal's colors, laid over the base theme's.
    let mut terminal = if params["from"].is_string() || themes::preview(cx).is_none() {
        themes::terminal_of(&base.name, cx)
    } else {
        themes::terminal_colors(cx)
    };
    let mut changes = changes;
    if let Some(colors) = changes
        .as_object_mut()
        .and_then(|changes| changes.remove("terminal"))
    {
        let given = themes::parse_terminal(&colors)?;
        let mut merged_terminal = terminal.unwrap_or([None; 16]);
        for (slot, color) in merged_terminal.iter_mut().zip(given) {
            if color.is_some() {
                *slot = color;
            }
        }
        terminal = Some(merged_terminal);
    }
    let theme = merged(&base, &changes)?;
    let counted = count_leaves(&changes);
    themes::set_preview(Some((theme, terminal)), cx);
    AppState::refresh_theme(cx);
    reply(format!(
        "Shown now ({counted} change{} on {}). Ask the user how it looks; theme_save keeps it, theme_discard drops it.",
        if counted == 1 { "" } else { "s" },
        base.name
    ))
}

/// `base` with `changes` laid over it, checked: every key one PilotHouse
/// reads, every color one it can show.
fn merged(base: &ThemeConfig, changes: &Value) -> Result<ThemeConfig> {
    if let Some(changes) = changes.as_object() {
        for key in changes.keys() {
            if !["colors", "highlight", "mode", "name", "terminal"].contains(&key.as_str()) {
                bail!(
                    "`{key}` is not part of a theme: put colors under \"colors\" and the editor's under \"highlight\""
                );
            }
        }
    }
    if let Some(mode) = changes.get("mode")
        && !matches!(mode.as_str(), Some("light" | "dark"))
    {
        bail!("`mode` is \"light\" or \"dark\"");
    }
    let mut value = serde_json::to_value(base)?;
    merge(&mut value, changes);
    let theme: ThemeConfig = serde_json::from_value(value)
        .map_err(|err| anyhow!("that does not make a theme: {err}"))?;
    // Keys that went nowhere: a misspelled token reads as no change.
    let read = serde_json::to_value(&theme)?;
    let mut unknown = Vec::new();
    missing_keys(changes, &read, String::new(), &mut unknown);
    if !unknown.is_empty() {
        bail!(
            "no such token: {} (theme_get lists them all)",
            unknown.join(", ")
        );
    }
    let mut bad = Vec::new();
    if let Some(colors) = changes.get("colors").and_then(Value::as_object) {
        for (key, color) in colors {
            if let Some(color) = color.as_str()
                && gpui_kit::component::try_parse_color(color).is_err()
            {
                bad.push(format!("colors.{key} = {color}"));
            }
        }
    }
    if !bad.is_empty() {
        bail!("not colors: {} (use #rrggbb or #rrggbbaa)", bad.join(", "));
    }
    Ok(theme)
}

/// Lays `patch` over `target`: objects merge, anything else replaces.
fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                merge(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}

/// The paths of `patch` that `read` does not have (keys a theme does not
/// know, dropped when it was read).
fn missing_keys(patch: &Value, read: &Value, at: String, out: &mut Vec<String>) {
    let Some(patch) = patch.as_object() else {
        return;
    };
    for (key, value) in patch {
        let path = if at.is_empty() {
            key.clone()
        } else {
            format!("{at}.{key}")
        };
        match read.get(key) {
            None => out.push(path),
            // A syntax entry's own fields (color, font_style) are free-form.
            Some(_) if at.ends_with("syntax") => {}
            Some(read) => missing_keys(value, read, path, out),
        }
    }
}

fn count_leaves(value: &Value) -> usize {
    match value {
        Value::Object(map) => map.values().map(count_leaves).sum(),
        _ => 1,
    }
}

fn save(params: Value, cx: &mut App) -> Result<Value> {
    let name = name(&params)?;
    let mut theme = (*themes::current(cx)).clone();
    theme.name = name.clone().into();
    let saved = themes::save(theme, themes::terminal_colors(cx), cx)?;
    themes::set_preview(None, cx);
    AppState::set_appearance(Appearance::Theme(saved.name.to_string()), cx);
    reply(format!(
        "Saved as “{name}” and in use. The user can pick it, or another, in Settings → Appearance → Theme."
    ))
}

/// A VS Code color theme (its JSON as `text`), saved as one of the user's
/// and used.
fn import(params: Value, cx: &mut App) -> Result<Value> {
    let text = params["text"]
        .as_str()
        .context("give the theme file's JSON as `text`")?;
    let name = params["name"]
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let (theme, terminal) = themes::import_vscode(text, name, cx)?;
    let saved = themes::save(theme, terminal, cx)?;
    AppState::set_appearance(Appearance::Theme(saved.name.to_string()), cx);
    reply(format!(
        "Imported as “{}” ({}) and in use. theme_preview can adjust it; theme_save keeps the changes.",
        saved.name,
        if saved.mode.is_dark() {
            "dark"
        } else {
            "light"
        }
    ))
}

fn discard(cx: &mut App) -> Result<Value> {
    if themes::preview(cx).is_none() {
        return reply("There was no preview.");
    }
    themes::set_preview(None, cx);
    AppState::refresh_theme(cx);
    reply("Dropped: PilotHouse looks as before.")
}

fn list(cx: &mut App) -> Result<Value> {
    let selected = themes::selected(cx).map(|theme| theme.name.to_string());
    let follows_system = AppState::appearance(cx) == Appearance::System;
    let mut lines = vec![format!(
        "system: follows the system (Modern Light or Modern Dark){}",
        if follows_system { " (in use)" } else { "" }
    )];
    for (theme, mine) in themes::list(cx) {
        let in_use = !follows_system && selected.as_deref() == Some(theme.name.as_ref());
        lines.push(format!(
            "{}: {}{}{}",
            theme.name,
            if theme.mode.is_dark() {
                "dark"
            } else {
                "light"
            },
            if mine { ", the user's" } else { "" },
            if in_use { " (in use)" } else { "" }
        ));
    }
    if themes::preview(cx).is_some() {
        lines.push("A preview is shown over it (theme_save keeps it).".into());
    }
    reply(lines.join("\n"))
}

fn use_theme(params: Value, cx: &mut App) -> Result<Value> {
    let name = name(&params)?;
    let appearance = if name.eq_ignore_ascii_case("system") {
        Appearance::System
    } else {
        let theme = themes::find(&name, cx)
            .with_context(|| format!("no theme `{name}` (theme_list shows them)"))?;
        Appearance::Theme(theme.name.to_string())
    };
    themes::set_preview(None, cx);
    AppState::set_appearance(appearance, cx);
    reply(format!("PilotHouse now uses {name}."))
}

fn delete(params: Value, cx: &mut App) -> Result<Value> {
    let name = name(&params)?;
    themes::delete(&name, cx)?;
    if AppState::appearance(cx) == Appearance::Theme(name.clone()) {
        AppState::set_appearance(Appearance::System, cx);
    } else {
        AppState::refresh_theme(cx);
    }
    reply(format!("Deleted “{name}”."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ThemeConfig {
        serde_json::from_value(json!({
            "name": "Base", "mode": "dark",
            "colors": { "background": "#101010" },
            "highlight": { "editor.background": "#101010", "syntax": { "keyword": { "color": "#ff0000" } } }
        }))
        .unwrap()
    }

    #[test]
    fn changes_lay_over_the_theme_and_are_checked() {
        let theme = merged(
            &base(),
            &json!({ "colors": { "sidebar.background": "#202020" }, "highlight": { "syntax": { "string": { "color": "#00ff00", "font_style": "italic" } } } }),
        )
        .unwrap();
        let value = serde_json::to_value(&theme).unwrap();
        assert_eq!(value["colors"]["background"], "#101010", "kept");
        assert_eq!(value["colors"]["sidebar.background"], "#202020");
        assert_eq!(
            value["highlight"]["syntax"]["keyword"]["color"],
            "#ff0000ff"
        );
        assert_eq!(value["highlight"]["syntax"]["string"]["color"], "#00ff00ff");

        let typo = merged(
            &base(),
            &json!({ "colors": { "sidebar.bakground": "#202020" } }),
        );
        assert!(
            typo.unwrap_err()
                .to_string()
                .contains("colors.sidebar.bakground")
        );
        let not_color = merged(&base(), &json!({ "colors": { "background": "dark blue" } }));
        assert!(not_color.unwrap_err().to_string().contains("not colors"));
        let flat = merged(&base(), &json!({ "background": "#000000" }));
        assert!(flat.unwrap_err().to_string().contains("under \"colors\""));
        let bad_editor = merged(
            &base(),
            &json!({ "highlight": { "editor.background": "nope" } }),
        );
        assert!(bad_editor.is_err());
        assert!(merged(&base(), &json!({ "mode": "dim" })).is_err());
        assert_eq!(
            count_leaves(&json!({ "colors": { "a": 1, "b": 2 }, "mode": "dark" })),
            3
        );
    }
}
