//! Browser tools: the project's browser pane, driven over DevTools.

use serde_json::{Value, json};

use super::{Tool, with_project};

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "browser_open",
        description: "Open a URL in the project's browser (the pane the user sees in PilotHouse) and wait for it to load. Logins made there persist for this project. Local addresses like localhost:3000 work.",
        schema: || json!({ "type": "object", "required": ["url"], "properties": { "url": { "type": "string" } } }),
        build: |arguments, project_id, _| Ok(("browser.open", with_project(arguments, project_id))),
        format: format_page,
    },
    Tool {
        name: "browser_screenshot",
        description: "Take a screenshot of the browser page, as the user sees it.",
        schema: || {
            json!({ "type": "object", "properties": {
                "full_page": { "type": "boolean", "description": "Capture the whole page, not just the visible part." }
            } })
        },
        build: |arguments, project_id, _| {
            Ok(("browser.screenshot", with_project(arguments, project_id)))
        },
        format: format_page,
    },
    Tool {
        name: "browser_snapshot",
        description: "Read the page: its title, URL, visible text and the interactive elements with a CSS selector for each. Use the selectors with browser_click and browser_type.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| {
            Ok(("browser.snapshot", with_project(arguments, project_id)))
        },
        format: |result| {
            let elements = result["elements"]
                .as_array()
                .map(|elements| {
                    elements
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            format!(
                "{}\n{}\n\nInteractive elements:\n{elements}\n\nText:\n{}",
                result["title"].as_str().unwrap_or_default(),
                result["url"].as_str().unwrap_or_default(),
                result["text"].as_str().unwrap_or_default()
            )
        },
    },
    Tool {
        name: "browser_click",
        description: "Click an element by CSS selector (preferred), or a point in CSS pixels.",
        schema: || {
            json!({ "type": "object", "properties": {
                "selector": { "type": "string" },
                "x": { "type": "number" },
                "y": { "type": "number" }
            } })
        },
        build: |arguments, project_id, _| {
            Ok(("browser.click", with_project(arguments, project_id)))
        },
        format: format_page,
    },
    Tool {
        name: "browser_type",
        description: "Type text into the page. With `selector`, that field is focused (and its text selected) first. With `submit`, Enter is pressed after.",
        schema: || {
            json!({ "type": "object", "required": ["text"], "properties": {
                "text": { "type": "string" },
                "selector": { "type": "string" },
                "submit": { "type": "boolean" }
            } })
        },
        build: |arguments, project_id, _| Ok(("browser.type", with_project(arguments, project_id))),
        format: format_page,
    },
    Tool {
        name: "browser_press",
        description: "Press a key: Enter, Tab, Escape, Backspace, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, PageDown, PageUp, Home, End, or a single character.",
        schema: || json!({ "type": "object", "required": ["key"], "properties": { "key": { "type": "string" } } }),
        build: |arguments, project_id, _| {
            Ok(("browser.press", with_project(arguments, project_id)))
        },
        format: format_page,
    },
    Tool {
        name: "browser_scroll",
        description: "Scroll the page by `dy` CSS pixels (negative scrolls up). Default 600.",
        schema: || json!({ "type": "object", "properties": { "dy": { "type": "number" } } }),
        build: |arguments, project_id, _| {
            Ok(("browser.scroll", with_project(arguments, project_id)))
        },
        format: format_page,
    },
    Tool {
        name: "browser_evaluate",
        description: "Run a JavaScript expression in the page and return its JSON value. Promises are awaited.",
        schema: || json!({ "type": "object", "required": ["expression"], "properties": { "expression": { "type": "string" } } }),
        build: |arguments, project_id, _| {
            Ok(("browser.evaluate", with_project(arguments, project_id)))
        },
        format: |result| serde_json::to_string_pretty(&result["value"]).unwrap_or_default(),
    },
    Tool {
        name: "browser_logs",
        description: "Recent console output (kind \"console\", the default) or network requests (kind \"network\") from the page, oldest first.",
        schema: || {
            json!({ "type": "object", "properties": {
                "kind": { "type": "string", "enum": ["console", "network"] },
                "errors_only": { "type": "boolean" },
                "limit": { "type": "integer", "description": "Default 50." }
            } })
        },
        build: |arguments, project_id, _| Ok(("browser.logs", with_project(arguments, project_id))),
        format: |result| match result["text"].as_str().filter(|text| !text.is_empty()) {
            Some(text) => text.to_string(),
            None => "Nothing logged yet.".to_string(),
        },
    },
];

fn format_page(result: &Value) -> String {
    let title = result["title"].as_str().unwrap_or_default();
    let url = result["url"].as_str().unwrap_or_default();
    if title.is_empty() {
        url.to_string()
    } else {
        format!("{title}\n{url}")
    }
}
