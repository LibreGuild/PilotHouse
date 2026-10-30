//! Chrome tools: a real, visible Chrome (or Edge) with a profile of its own
//! that PilotHouse starts for agents, to use websites as a person would.
//! Each agent session works in its own tab; the bridge adds who it is.

use serde_json::{Value, json};

use super::{Tool, with_project};

/// How to name an element, shared by the tools that act on one.
fn element_properties() -> Value {
    json!({
        "ref": { "type": "integer", "description": "The element's number from the latest chrome_snapshot (preferred)." },
        "selector": { "type": "string", "description": "A CSS selector." },
        "text": { "type": "string", "description": "Text on the element (a button's or link's words)." }
    })
}

fn with(mut properties: Value, extra: Value) -> Value {
    if let (Some(properties), Some(extra)) = (properties.as_object_mut(), extra.as_object()) {
        properties.extend(extra.clone());
    }
    properties
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "chrome_open",
        description: "Open a URL in your tab of the user's PilotHouse Chrome: a real, visible Chrome (or Edge) with its own profile, so sites the user signed in to there stay signed in. Waits for the page to load. Use it for real websites (web apps, dashboards, docs, accounts); use browser_open for the project's own dev server in the PilotHouse pane. The first call starts Chrome. Each chat has its own tab; new_tab opens another. Never try to solve a CAPTCHA or other check that a person is there: ask the user to complete it in that window.",
        schema: || {
            json!({ "type": "object", "required": ["url"], "properties": {
            "url": { "type": "string" },
            "new_tab": { "type": "boolean", "description": "Open in a new tab of yours instead of the current one." }
        } })
        },
        build: |arguments, project_id, _| Ok(("chrome.open", with_project(arguments, project_id))),
        format: format_page,
    },
    Tool {
        name: "chrome_snapshot",
        description: "Read your Chrome page: title, address, scroll position, the visible interactive elements numbered [n] (with their state: values, checked, expanded, disabled, link targets) and the page's text. Act on elements by their number (`ref`) with the other chrome_ tools. Numbers last until the next snapshot or page change.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| {
            Ok(("chrome.snapshot", with_project(arguments, project_id)))
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
            let scroll = &result["scroll"];
            let mut text = format!(
                "{}\n{}\nScrolled to {} of {} px (window {} px tall).",
                result["title"].as_str().unwrap_or_default(),
                result["url"].as_str().unwrap_or_default(),
                scroll["y"],
                scroll["height"],
                scroll["view"]
            );
            if let Some(focused) = result["focused"].as_str() {
                text.push_str(&format!("\nFocused: {focused}"));
            }
            if result["frames"].as_u64().unwrap_or(0) > 0 {
                text.push_str(&format!(
                    "\n{} embedded frame(s) not listed; use chrome_evaluate to look inside same-origin ones.",
                    result["frames"]
                ));
            }
            text.push_str(&notes(result));
            format!(
                "{text}\n\nElements:\n{elements}\n\nText:\n{}",
                result["text"].as_str().unwrap_or_default()
            )
        },
    },
    Tool {
        name: "chrome_screenshot",
        description: "A screenshot of your Chrome tab as the user sees it (the PilotHouse glow is left out); full_page for the whole page, or one element.",
        schema: || {
            json!({ "type": "object", "properties": with(element_properties(), json!({
            "full_page": { "type": "boolean" }
        })) })
        },
        build: |arguments, project_id, _| {
            Ok(("chrome.screenshot", with_project(arguments, project_id)))
        },
        format: format_page,
    },
    Tool {
        name: "chrome_click",
        description: "Click an element (by `ref`, `selector` or `text`) or a point (`x`, `y` in CSS pixels). The pointer is shown moving there. `button` right for a context menu, `count` 2 for a double click, `modifiers` held (Alt, Control, Meta, Shift).",
        schema: || {
            json!({ "type": "object", "properties": with(element_properties(), json!({
            "x": { "type": "number" },
            "y": { "type": "number" },
            "button": { "type": "string", "enum": ["left", "right", "middle"] },
            "count": { "type": "integer", "description": "2 for a double click." },
            "modifiers": { "type": "array", "items": { "type": "string", "enum": ["Alt", "Control", "Meta", "Shift"] } }
        })) })
        },
        build: |arguments, project_id, _| Ok(("chrome.click", with_project(arguments, project_id))),
        format: format_page,
    },
    Tool {
        name: "chrome_type",
        description: "Type text: into an element (by `ref`, `selector` or `text`; `clear` replaces what is there) or where the focus is. `submit` presses Enter after; `slowly` types a key at a time for fields that react to each key (search suggestions). Password fields are refused: ask the user to sign in in the PilotHouse Chrome window.",
        schema: || {
            json!({ "type": "object", "required": ["text"], "properties": with(element_properties(), json!({
            "text": { "type": "string" },
            "clear": { "type": "boolean" },
            "submit": { "type": "boolean" },
            "slowly": { "type": "boolean" }
        })) })
        },
        build: |arguments, project_id, _| Ok(("chrome.type", with_project(arguments, project_id))),
        format: format_page,
    },
    Tool {
        name: "chrome_select",
        description: "Choose options in a drop-down list (<select>) by value or visible text.",
        schema: || {
            json!({ "type": "object", "required": ["values"], "properties": with(element_properties(), json!({
            "values": { "type": "array", "items": { "type": "string" } }
        })) })
        },
        build: |arguments, project_id, _| {
            Ok(("chrome.select", with_project(arguments, project_id)))
        },
        format: |result| {
            let chosen = result["chosen"]
                .as_array()
                .map(|chosen| {
                    chosen
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            format!("Chosen: {chosen}\n{}", format_page(result))
        },
    },
    Tool {
        name: "chrome_press",
        description: "Press a key or combination in your Chrome tab: Enter, Tab, Escape, Backspace, Delete, ArrowDown, PageDown, Home, F5, a character, or combinations like Control+A, Meta+Shift+K.",
        schema: || json!({ "type": "object", "required": ["key"], "properties": { "key": { "type": "string" } } }),
        build: |arguments, project_id, _| Ok(("chrome.press", with_project(arguments, project_id))),
        format: format_page,
    },
    Tool {
        name: "chrome_hover",
        description: "Move the pointer over an element or point (menus and tooltips that show on hover).",
        schema: || {
            json!({ "type": "object", "properties": with(element_properties(), json!({
            "x": { "type": "number" }, "y": { "type": "number" }
        })) })
        },
        build: |arguments, project_id, _| Ok(("chrome.hover", with_project(arguments, project_id))),
        format: format_page,
    },
    Tool {
        name: "chrome_drag",
        description: "Drag from one element or point to another (`from` and `to`, each with ref, selector, text, or x and y).",
        schema: || {
            json!({ "type": "object", "required": ["from", "to"], "properties": {
            "from": { "type": "object", "properties": with(element_properties(), json!({ "x": { "type": "number" }, "y": { "type": "number" } })) },
            "to": { "type": "object", "properties": with(element_properties(), json!({ "x": { "type": "number" }, "y": { "type": "number" } })) }
        } })
        },
        build: |arguments, project_id, _| Ok(("chrome.drag", with_project(arguments, project_id))),
        format: format_page,
    },
    Tool {
        name: "chrome_scroll",
        description: "Scroll with the mouse wheel: the page, or (with an element) the part of it holding that element. `dy` down (negative: up), `dx` right; CSS pixels, default dy 600.",
        schema: || {
            json!({ "type": "object", "properties": with(element_properties(), json!({
            "dx": { "type": "number" }, "dy": { "type": "number" }
        })) })
        },
        build: |arguments, project_id, _| {
            Ok(("chrome.scroll", with_project(arguments, project_id)))
        },
        format: |result| {
            let scroll = &result["scroll"];
            format!(
                "Scrolled to {} of {} px.\n{}",
                scroll["y"],
                scroll["height"],
                format_page(result)
            )
        },
    },
    Tool {
        name: "chrome_wait",
        description: "Wait until an element appears (or with gone, disappears), the address contains `url`, or the network is quiet (network_idle); with none of these, until the page has loaded. timeout_ms default 10000, at most 60000.",
        schema: || {
            json!({ "type": "object", "properties": with(element_properties(), json!({
            "gone": { "type": "boolean" },
            "url": { "type": "string" },
            "network_idle": { "type": "boolean" },
            "timeout_ms": { "type": "integer" }
        })) })
        },
        build: |arguments, project_id, _| Ok(("chrome.wait", with_project(arguments, project_id))),
        format: |result| {
            format!(
                "Done after {} ms.\n{}",
                result["waited_ms"],
                format_page(result)
            )
        },
    },
    Tool {
        name: "chrome_navigate",
        description: "Go back, forward, or reload your Chrome tab.",
        schema: || {
            json!({ "type": "object", "required": ["action"], "properties": {
            "action": { "type": "string", "enum": ["back", "forward", "reload"] }
        } })
        },
        build: |arguments, project_id, _| {
            Ok(("chrome.history", with_project(arguments, project_id)))
        },
        format: format_page,
    },
    Tool {
        name: "chrome_tabs",
        description: "Your tabs in Chrome: list them, open a new one (optionally at url), switch to one, or close one (by its index from list). Tabs a page opens (sign-in popups) become your current tab by themselves. list also shows the tabs the user opened, with their ids and which is on screen. When the user asks you to work in their tab (\"the active tab\", \"this page\", a form they started), take it: action take with its id as tab, or without one for the tab on screen. It becomes your current tab, with what the user typed kept; fill only what they asked. close on a taken tab gives it back without closing it.",
        schema: || {
            json!({ "type": "object", "properties": {
            "action": { "type": "string", "enum": ["list", "new", "switch", "close", "take"] },
            "index": { "type": "integer", "description": "Your tab's index from list (switch, close)." },
            "url": { "type": "string", "description": "For new." },
            "tab": { "type": "string", "description": "For take: the user's tab id from list, e.g. 3F2A9C1B. Leave out for the tab on screen." }
        } })
        },
        build: |arguments, project_id, _| Ok(("chrome.tabs", with_project(arguments, project_id))),
        format: |result| match result["text"].as_str() {
            Some(text) => text.to_string(),
            None => format_page(result),
        },
    },
    Tool {
        name: "chrome_evaluate",
        description: "Run JavaScript in your Chrome page (its own scripts' world) and return the JSON value; promises are awaited. For reading app state, calling page functions, or checking what the DOM holds.",
        schema: || json!({ "type": "object", "required": ["expression"], "properties": { "expression": { "type": "string" } } }),
        build: |arguments, project_id, _| {
            Ok(("chrome.evaluate", with_project(arguments, project_id)))
        },
        format: text_result,
    },
    Tool {
        name: "chrome_console",
        description: "Your Chrome page's console: messages and uncaught errors with where they came from (file:line:column) and stacks, oldest first. Filter by level (error, warn, info, debug and up), text contained; new_only for what came since you last looked; clear to start fresh.",
        schema: || {
            json!({ "type": "object", "properties": {
            "level": { "type": "string", "enum": ["error", "warn", "info", "debug"] },
            "contains": { "type": "string" },
            "new_only": { "type": "boolean" },
            "limit": { "type": "integer", "description": "Default 100." },
            "clear": { "type": "boolean" }
        } })
        },
        build: |arguments, project_id, _| {
            Ok(("chrome.console", with_project(arguments, project_id)))
        },
        format: text_result,
    },
    Tool {
        name: "chrome_network",
        description: "Your Chrome page's requests, oldest first: id, method, address, status, type, size and time. Filter by errors_only, text in the address (contains), kind (Document, XHR, Fetch, Script, Stylesheet, Image…); new_only since you last looked. chrome_request shows one in full.",
        schema: || {
            json!({ "type": "object", "properties": {
            "errors_only": { "type": "boolean" },
            "contains": { "type": "string" },
            "kind": { "type": "string" },
            "new_only": { "type": "boolean" },
            "limit": { "type": "integer", "description": "Default 100." }
        } })
        },
        build: |arguments, project_id, _| {
            Ok(("chrome.network", with_project(arguments, project_id)))
        },
        format: text_result,
    },
    Tool {
        name: "chrome_request",
        description: "One request in full by its id from chrome_network: headers (credentials hidden), what was sent, and with body: true, the response (text, up to 64 KB).",
        schema: || {
            json!({ "type": "object", "required": ["id"], "properties": {
            "id": { "type": "string" },
            "body": { "type": "boolean" }
        } })
        },
        build: |arguments, project_id, _| {
            Ok(("chrome.request", with_project(arguments, project_id)))
        },
        format: text_result,
    },
    Tool {
        name: "chrome_dialog",
        description: "Answer the dialog your page opened (alert, confirm, prompt, or leaving a page with unsaved changes): accept or dismiss, with `text` for a prompt. Before confirming anything that deletes, sends, pays or publishes, check with the user.",
        schema: || {
            json!({ "type": "object", "required": ["accept"], "properties": {
            "accept": { "type": "boolean" },
            "text": { "type": "string" }
        } })
        },
        build: |arguments, project_id, _| {
            Ok(("chrome.dialog", with_project(arguments, project_id)))
        },
        format: format_page,
    },
    Tool {
        name: "chrome_upload",
        description: "Put files into a file field (input type=file) of your page: absolute paths of files in the workspace's folders or the downloads folder.",
        schema: || {
            json!({ "type": "object", "required": ["paths"], "properties": with(element_properties(), json!({
            "paths": { "type": "array", "items": { "type": "string" } }
        })) })
        },
        build: |arguments, project_id, _| {
            Ok(("chrome.upload", with_project(arguments, project_id)))
        },
        format: format_page,
    },
    Tool {
        name: "chrome_downloads",
        description: "Files downloaded in Chrome, with where they were saved (a PilotHouse folder of the user's Downloads) and how far along they are.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| {
            Ok(("chrome.downloads", with_project(arguments, project_id)))
        },
        format: text_result,
    },
];

fn notes(result: &Value) -> String {
    result["notes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|note| format!("\nNote: {note}"))
        .collect()
}

fn format_page(result: &Value) -> String {
    let title = result["title"].as_str().unwrap_or_default();
    let url = result["url"].as_str().unwrap_or_default();
    let page = if title.is_empty() {
        url.to_string()
    } else {
        format!("{title}\n{url}")
    };
    format!("{page}{}", notes(result))
}

fn text_result(result: &Value) -> String {
    result["text"].as_str().unwrap_or_default().to_string()
}
