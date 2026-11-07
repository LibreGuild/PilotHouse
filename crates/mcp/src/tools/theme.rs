//! Theme tools: an agent designs a PilotHouse theme with the user, showing
//! each change live in the app, and saves it as one of the user's themes.

use serde_json::{Value, json};

use super::Tool;

fn text(result: &Value) -> String {
    result["text"].as_str().unwrap_or_default().to_string()
}

fn name_only() -> Value {
    json!({ "type": "object", "required": ["name"], "properties": { "name": { "type": "string" } } })
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "theme_get",
        description: concat!(
            "The theme ",
            ph_brand::name!(),
            " shows now (a preview being designed, else the chosen theme) as JSON: its name, mode (light or dark), every color token (null: not set, a default applies) and the editor colors, with a guide to where each part of ",
            ph_brand::name!(),
            " takes its colors from. Read it before changing a theme."
        ),
        schema: || json!({ "type": "object", "properties": {} }),
        build: |_, _, _| Ok(("theme.get", json!({}))),
        format: text,
    },
    Tool {
        name: "theme_preview",
        description: concat!(
            "Changes how ",
            ph_brand::name!(),
            " looks, live: the user sees each change at once while you talk. Give only what changes, as part of a theme: {\"colors\": {\"sidebar.background\": \"#1b1d2a\", ...}, \"highlight\": {\"editor.background\": \"#...\", \"syntax\": {\"keyword\": {\"color\": \"#...\"}}}, \"terminal\": {\"red\": \"#...\"}, \"mode\": \"dark\"}. Changes add up over calls, on top of the theme shown; `from` starts again from another theme (by name, see theme_list). Colors are #rrggbb or #rrggbbaa. Keep text readable (enough contrast with what it sits on), change related tokens together (sidebar, title bar, status bar and tab bar are one frame), and ask the user how it looks. Nothing is kept until theme_save; theme_discard drops it."
        ),
        schema: || {
            json!({ "type": "object", "properties": {
                "changes": { "type": "object", "description": "Part of a theme: colors, highlight (editor colors, with syntax), mode." },
                "from": { "type": "string", "description": "Start from this theme instead of the one shown." }
            } })
        },
        build: |arguments, _, _| Ok(("theme.preview", arguments)),
        format: text,
    },
    Tool {
        name: "theme_save",
        description: concat!(
            "Saves the theme being previewed as one of the user's themes, named `name`, and makes it the theme ",
            ph_brand::name!(),
            " uses. Ask the user for a name if they did not give one. Saving again under the same name updates it."
        ),
        schema: name_only,
        build: |arguments, _, _| Ok(("theme.save", arguments)),
        format: text,
    },
    Tool {
        name: "theme_discard",
        description: concat!(
            "Drops the theme being previewed: ",
            ph_brand::name!(),
            " looks as before the preview."
        ),
        schema: || json!({ "type": "object", "properties": {} }),
        build: |_, _, _| Ok(("theme.discard", json!({}))),
        format: text,
    },
    Tool {
        name: "theme_list",
        description: concat!(
            "The themes to choose from: ",
            ph_brand::name!(),
            "'s own and the user's, with their modes and which is in use."
        ),
        schema: || json!({ "type": "object", "properties": {} }),
        build: |_, _, _| Ok(("theme.list", json!({}))),
        format: text,
    },
    Tool {
        name: "theme_use",
        description: concat!(
            "Makes ",
            ph_brand::name!(),
            " use the theme `name` (from theme_list), or `system` to follow the system's light or dark mode. Drops a preview."
        ),
        schema: name_only,
        build: |arguments, _, _| Ok(("theme.use", arguments)),
        format: text,
    },
    Tool {
        name: "theme_import",
        description: "Imports a VS Code color theme: give the theme file's JSON as `text` (fetch it first when the user gives a link, e.g. the raw file of a theme on GitHub; comments and trailing commas are fine). Its colors, code colors and terminal colors become one of the user's themes, saved and in use; `name` names it (else the theme's own name).",
        schema: || json!({ "type": "object", "required": ["text"], "properties": { "text": { "type": "string" }, "name": { "type": "string" } } }),
        build: |arguments, _, _| Ok(("theme.import", arguments)),
        format: text,
    },
    Tool {
        name: "theme_delete",
        description: concat!(
            "Deletes one of the user's own themes (never ",
            ph_brand::name!(),
            "'s). Only when the user asks for it."
        ),
        schema: name_only,
        build: |arguments, _, _| Ok(("theme.delete", arguments)),
        format: text,
    },
];
