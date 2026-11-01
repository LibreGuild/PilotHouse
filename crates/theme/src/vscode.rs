//! Importing a VS Code color theme: its workbench colors and token colors
//! become a PilotHouse theme, its terminal colors the terminal's.
//!
//! VS Code themes name hundreds of colors; PilotHouse takes the ones that
//! say what its parts are (editor, side bar, title and status bars, lists,
//! buttons, inputs, menus) and starts from Modern Dark or Modern Light (by
//! the theme's type) for anything the theme leaves out. Token colors are
//! matched by TextMate scope: an exact scope first, then a narrower one
//! (`keyword.control` for `keyword`). Theme files are often JSON with
//! comments and trailing commas; both are accepted.

use anyhow::{Context as _, Result, bail};
use gpui_kit::component::{ThemeConfig, ThemeMode};
use serde_json::{Map, Value, json};

use crate::themes::TerminalColors;

/// PilotHouse color tokens and the VS Code colors each is taken from, the
/// first one the theme has.
const COLORS: &[(&str, &[&str])] = &[
    ("background", &["editor.background"]),
    ("foreground", &["foreground", "editor.foreground"]),
    // Not the line numbers' color: often too dim for text.
    (
        "muted.foreground",
        &["descriptionForeground", "tab.inactiveForeground"],
    ),
    (
        "border",
        &[
            "panel.border",
            "editorGroup.border",
            "sideBar.border",
            "contrastBorder",
        ],
    ),
    (
        "window.border",
        &["window.activeBorder", "panel.border", "sideBar.border"],
    ),
    (
        "sidebar.background",
        &["sideBar.background", "activityBar.background"],
    ),
    ("sidebar.foreground", &["sideBar.foreground", "foreground"]),
    ("sidebar.border", &["sideBar.border", "panel.border"]),
    (
        "title_bar.background",
        &["titleBar.activeBackground", "sideBar.background"],
    ),
    (
        "title_bar.border",
        &["titleBar.border", "sideBar.border", "panel.border"],
    ),
    (
        "status_bar.background",
        &["statusBar.background", "sideBar.background"],
    ),
    ("status_bar.border", &["statusBar.border", "panel.border"]),
    (
        "tab_bar.background",
        &["editorGroupHeader.tabsBackground", "sideBar.background"],
    ),
    (
        "tab.active.background",
        &["tab.activeBackground", "editor.background"],
    ),
    (
        "tab.active.foreground",
        &["tab.activeForeground", "foreground"],
    ),
    (
        "tab.foreground",
        &["tab.inactiveForeground", "descriptionForeground"],
    ),
    (
        "popover.background",
        &[
            "menu.background",
            "editorWidget.background",
            "dropdown.background",
        ],
    ),
    (
        "popover.foreground",
        &["menu.foreground", "editorWidget.foreground", "foreground"],
    ),
    (
        "muted.background",
        &[
            "textCodeBlock.background",
            "input.background",
            "editorWidget.background",
        ],
    ),
    (
        "secondary.background",
        &["button.secondaryBackground", "input.background"],
    ),
    (
        "secondary.hover.background",
        &["button.secondaryHoverBackground", "list.hoverBackground"],
    ),
    (
        "secondary.foreground",
        &["button.secondaryForeground", "foreground"],
    ),
    (
        "input.border",
        &["input.border", "dropdown.border", "panel.border"],
    ),
    ("list.hover.background", &["list.hoverBackground"]),
    (
        "list.active.background",
        &[
            "list.activeSelectionBackground",
            "list.inactiveSelectionBackground",
        ],
    ),
    (
        "accent.background",
        &["list.hoverBackground", "toolbar.hoverBackground"],
    ),
    ("sidebar.accent.background", &["list.hoverBackground"]),
    ("primary.background", &["button.background"]),
    (
        "primary.hover.background",
        &["button.hoverBackground", "button.background"],
    ),
    ("primary.foreground", &["button.foreground"]),
    ("sidebar.primary.background", &["button.background"]),
    ("sidebar.primary.foreground", &["button.foreground"]),
    ("ring", &["focusBorder"]),
    ("info.background", &["focusBorder", "button.background"]),
    ("link", &["textLink.foreground"]),
    (
        "link.hover",
        &["textLink.activeForeground", "textLink.foreground"],
    ),
    (
        "selection.background",
        &["editor.selectionBackground", "selection.background"],
    ),
    ("caret", &["editorCursor.foreground"]),
    (
        "scrollbar.thumb.background",
        &["scrollbarSlider.background"],
    ),
    (
        "scrollbar.thumb.hover.background",
        &["scrollbarSlider.hoverBackground"],
    ),
    (
        "danger.background",
        &["errorForeground", "editorError.foreground"],
    ),
    (
        "warning.background",
        &["editorWarning.foreground", "list.warningForeground"],
    ),
    (
        "success.background",
        &[
            "gitDecoration.addedResourceForeground",
            "terminal.ansiGreen",
        ],
    ),
    ("progress.bar.background", &["progressBar.background"]),
    ("base.red", &["terminal.ansiRed"]),
    ("base.green", &["terminal.ansiGreen"]),
    ("base.blue", &["terminal.ansiBlue"]),
    ("base.yellow", &["terminal.ansiYellow"]),
    ("base.magenta", &["terminal.ansiMagenta"]),
    ("base.cyan", &["terminal.ansiCyan"]),
];

/// The editor's own colors, under `highlight`.
const EDITOR: &[(&str, &[&str])] = &[
    ("editor.background", &["editor.background"]),
    ("editor.foreground", &["editor.foreground", "foreground"]),
    (
        "editor.gutter.background",
        &["editorGutter.background", "editor.background"],
    ),
    (
        "editor.active_line.background",
        &["editor.lineHighlightBackground"],
    ),
    ("editor.line_number", &["editorLineNumber.foreground"]),
    (
        "editor.active_line_number",
        &["editorLineNumber.activeForeground"],
    ),
    ("editor.invisible", &["editorWhitespace.foreground"]),
];

/// Syntax names and the TextMate scopes they are taken from, best first.
const SYNTAX: &[(&str, &[&str])] = &[
    ("comment", &["comment", "punctuation.definition.comment"]),
    ("comment.doc", &["comment.block.documentation", "comment"]),
    ("string", &["string", "string.quoted"]),
    ("string.escape", &["constant.character.escape"]),
    ("string.regex", &["string.regexp"]),
    (
        "keyword",
        &[
            "keyword",
            "keyword.control",
            "storage.type",
            "storage.modifier",
            "storage",
        ],
    ),
    ("operator", &["keyword.operator"]),
    (
        "function",
        &[
            "entity.name.function",
            "support.function",
            "meta.function-call",
        ],
    ),
    (
        "type",
        &[
            "entity.name.type",
            "entity.name.class",
            "support.class",
            "support.type",
        ],
    ),
    (
        "constructor",
        &["entity.name.class", "entity.name.type", "support.class"],
    ),
    ("number", &["constant.numeric"]),
    (
        "boolean",
        &["constant.language.boolean", "constant.language"],
    ),
    (
        "constant",
        &[
            "variable.other.constant",
            "constant.language",
            "support.constant",
            "constant",
        ],
    ),
    ("variable", &["variable", "variable.other"]),
    ("variable.special", &["variable.language"]),
    (
        "property",
        &[
            "variable.other.property",
            "support.type.property-name",
            "meta.object-literal.key",
            "variable.other.object.property",
        ],
    ),
    ("attribute", &["entity.other.attribute-name"]),
    ("tag", &["entity.name.tag"]),
    ("title", &["markup.heading", "entity.name.section"]),
    ("punctuation", &["punctuation"]),
    ("link_text", &["string.other.link", "markup.link"]),
    ("link_uri", &["markup.underline.link"]),
    ("emphasis", &["markup.italic"]),
    ("emphasis.strong", &["markup.bold"]),
];

/// VS Code's terminal colors, in ANSI order.
const TERMINAL: [&str; 16] = [
    "terminal.ansiBlack",
    "terminal.ansiRed",
    "terminal.ansiGreen",
    "terminal.ansiYellow",
    "terminal.ansiBlue",
    "terminal.ansiMagenta",
    "terminal.ansiCyan",
    "terminal.ansiWhite",
    "terminal.ansiBrightBlack",
    "terminal.ansiBrightRed",
    "terminal.ansiBrightGreen",
    "terminal.ansiBrightYellow",
    "terminal.ansiBrightBlue",
    "terminal.ansiBrightMagenta",
    "terminal.ansiBrightCyan",
    "terminal.ansiBrightWhite",
];

/// A VS Code color theme (`text`) as a PilotHouse theme named `name` (the
/// theme's own name when `None`), over `dark_base` or `light_base`.
pub fn import(
    text: &str,
    name: Option<&str>,
    dark_base: &ThemeConfig,
    light_base: &ThemeConfig,
) -> Result<(ThemeConfig, Option<TerminalColors>)> {
    let source: Value =
        serde_json::from_str(&strip_jsonc(text)).context("that is not a theme file (JSON)")?;
    let colors = source["colors"].as_object().cloned().unwrap_or_default();
    let tokens = source["tokenColors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if colors.is_empty() && tokens.is_empty() {
        bail!("that is not a VS Code color theme: it has no `colors` or `tokenColors`");
    }
    let color = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|key| colors.get(*key).and_then(Value::as_str))
            .and_then(normalize)
    };
    let dark = match source["type"].as_str() {
        Some("light") | Some("hcLight") => false,
        Some(_) => true,
        None => color(&["editor.background"])
            .and_then(|background| gpui_kit::component::try_parse_color(&background).ok())
            .is_none_or(|background| background.l < 0.5),
    };
    let base = if dark { dark_base } else { light_base };
    let mut theme = serde_json::to_value(base)?;
    let name = name
        .map(str::to_string)
        .or_else(|| source["name"].as_str().map(str::to_string))
        .unwrap_or_else(|| "Imported theme".into());
    theme["name"] = json!(name);
    theme["mode"] = json!(if dark { "dark" } else { "light" });
    for (token, keys) in COLORS {
        if let Some(value) = color(keys) {
            theme["colors"][*token] = json!(value);
        }
    }
    if !theme["highlight"].is_object() {
        theme["highlight"] = json!({});
    }
    for (token, keys) in EDITOR {
        if let Some(value) = color(keys) {
            theme["highlight"][*token] = json!(value);
        }
    }
    if !theme["highlight"]["syntax"].is_object() {
        theme["highlight"]["syntax"] = json!({});
    }
    let rules = scope_rules(&tokens);
    for (name, scopes) in SYNTAX {
        if let Some(style) = best_style(&rules, scopes) {
            theme["highlight"]["syntax"][*name] = style;
        }
    }
    let theme: ThemeConfig =
        serde_json::from_value(theme).context("the theme's colors could not be read")?;
    let mut terminal: TerminalColors = [None; 16];
    for (slot, key) in terminal.iter_mut().zip(TERMINAL) {
        *slot = color(&[key]).and_then(|value| gpui_kit::component::try_parse_color(&value).ok());
    }
    let terminal = terminal.iter().any(Option::is_some).then_some(terminal);
    Ok((theme, terminal))
}

/// Whether the imported theme's mode is dark.
pub fn is_dark(theme: &ThemeConfig) -> bool {
    theme.mode == ThemeMode::Dark
}

/// A token color rule: its scopes and its style as PilotHouse writes one.
struct Rule {
    scopes: Vec<String>,
    style: Value,
}

fn scope_rules(tokens: &[Value]) -> Vec<Rule> {
    tokens
        .iter()
        .filter_map(|token| {
            let scopes: Vec<String> = match &token["scope"] {
                Value::String(scopes) => scopes
                    .split(',')
                    .map(|scope| scope.trim().to_string())
                    .collect(),
                Value::Array(scopes) => scopes
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|scope| scope.trim().to_string())
                    .collect(),
                _ => return None,
            };
            let settings = &token["settings"];
            let mut style = Map::new();
            if let Some(color) = settings["foreground"].as_str().and_then(normalize) {
                style.insert("color".into(), json!(color));
            }
            let font_style = settings["fontStyle"].as_str().unwrap_or_default();
            if font_style.contains("italic") {
                style.insert("font_style".into(), json!("italic"));
            }
            if font_style.contains("bold") {
                style.insert("font_weight".into(), json!(700));
            }
            (!style.is_empty()).then_some(Rule {
                scopes,
                style: Value::Object(style),
            })
        })
        .collect()
}

/// The style of the rule that best matches `wanted` (in order): a scope
/// equal to one, else one under it (`keyword.control` for `keyword`).
fn best_style(rules: &[Rule], wanted: &[&str]) -> Option<Value> {
    for scope in wanted {
        if let Some(rule) = rules
            .iter()
            .find(|rule| rule.scopes.iter().any(|known| known == scope))
        {
            return Some(rule.style.clone());
        }
    }
    for scope in wanted {
        let narrower = format!("{scope}.");
        if let Some(rule) = rules
            .iter()
            .find(|rule| rule.scopes.iter().any(|known| known.starts_with(&narrower)))
        {
            return Some(rule.style.clone());
        }
    }
    None
}

/// A color as PilotHouse reads it: `#rgb` and `#rgba` spelled out; `None`
/// for anything else.
fn normalize(color: &str) -> Option<String> {
    let hex = color.trim().strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let long = match hex.len() {
        3 | 4 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => hex.to_string(),
        _ => return None,
    };
    Some(format!("#{}", long.to_ascii_lowercase()))
}

/// JSON with comments (`//`, `/* */`) and trailing commas, as plain JSON.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => {
                    if let Some(next) = chars.next() {
                        out.push(next);
                    }
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for next in chars.by_ref() {
                    if last == '*' && next == '/' {
                        break;
                    }
                    last = next;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas: a comma with only whitespace before `}` or `]`.
    let mut cleaned = String::with_capacity(out.len());
    let mut in_string = false;
    let mut escaped = false;
    let bytes: Vec<char> = out.chars().collect();
    for (ix, &c) in bytes.iter().enumerate() {
        if in_string {
            cleaned.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
        }
        if c == ',' {
            let next = bytes[ix + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        cleaned.push(c);
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(mode: &str) -> ThemeConfig {
        serde_json::from_value(json!({
            "name": "Base", "mode": mode,
            "colors": { "background": "#111111", "border": "#222222" },
            "highlight": { "syntax": { "keyword": { "color": "#0000ff" } } }
        }))
        .unwrap()
    }

    #[test]
    fn a_vscode_theme_becomes_a_pilothouse_theme() {
        let text = r##"{
            // A comment, as VS Code allows.
            "name": "Night Owl Lite",
            "type": "dark",
            "colors": {
                "editor.background": "#011627",
                "sideBar.background": "#011627",
                "statusBar.background": "#0b2942",
                "button.background": "#7e57c2cc",
                "focusBorder": "#122d42",
                "terminal.ansiRed": "#EF5350",
                "editorLineNumber.foreground": "#4b6479",
                "foreground": "#d6deeb", /* trailing comma next */
            },
            "tokenColors": [
                { "scope": "comment", "settings": { "foreground": "#637777", "fontStyle": "italic" } },
                { "scope": ["keyword.control", "storage.type"], "settings": { "foreground": "#c792ea" } },
                { "scope": "string.quoted.double, string.quoted.single", "settings": { "foreground": "#ecc48d" } },
                { "scope": "entity.name.function", "settings": { "foreground": "#82AAFF", "fontStyle": "bold italic" } },
            ]
        }"##;
        let (theme, terminal) = import(text, None, &base("dark"), &base("light")).unwrap();
        let value = serde_json::to_value(&theme).unwrap();
        assert_eq!(theme.name.as_ref(), "Night Owl Lite");
        assert!(is_dark(&theme));
        assert_eq!(value["colors"]["background"], "#011627");
        assert_eq!(value["colors"]["status_bar.background"], "#0b2942");
        assert_eq!(value["colors"]["primary.background"], "#7e57c2cc");
        assert_eq!(value["colors"]["border"], "#222222", "kept from the base");
        assert_eq!(value["colors"]["base.red"], "#ef5350");
        let syntax = &value["highlight"]["syntax"];
        assert_eq!(syntax["comment"]["color"], "#637777ff");
        assert_eq!(syntax["comment"]["font_style"], "italic");
        assert_eq!(
            syntax["keyword"]["color"], "#c792eaff",
            "keyword.control is under keyword"
        );
        assert_eq!(syntax["string"]["color"], "#ecc48dff");
        assert_eq!(syntax["function"]["font_weight"], 700);
        let terminal = terminal.unwrap();
        assert!(terminal[1].is_some() && terminal[0].is_none());
    }

    #[test]
    fn light_themes_and_bad_files() {
        let text = r##"{ "colors": { "editor.background": "#fdf6e3" } }"##;
        let (theme, _) = import(text, Some("Paper"), &base("dark"), &base("light")).unwrap();
        assert!(!is_dark(&theme), "a light background makes a light theme");
        assert_eq!(theme.name.as_ref(), "Paper");
        assert!(import("{ nope", None, &base("dark"), &base("light")).is_err());
        assert!(import(r#"{"name": "x"}"#, None, &base("dark"), &base("light")).is_err());
        assert_eq!(normalize("#ABC").as_deref(), Some("#aabbcc"));
        assert_eq!(normalize("red"), None);
        assert_eq!(
            strip_jsonc(r#"{"a": "// not a comment", "b": [1,],}"#),
            r#"{"a": "// not a comment", "b": [1]}"#
        );
    }
}
