//! The MCP/JSON-RPC envelope pieces the server needs.

use serde_json::{Value, json};

pub const PARSE_ERROR: i64 = -32700;
pub const METHOD_NOT_FOUND: i64 = -32601;

/// Protocol revisions this server understands, newest first.
const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

pub fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Answers `initialize`, agreeing to the client's version when we know it.
pub fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| SUPPORTED_VERSIONS.contains(version))
        .unwrap_or(SUPPORTED_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "pilothouse", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "PilotHouse tools for this project.\n\
            Board: the plan as cards in four columns (todo, in_progress, review, done). Use \
            board_add_cards to plan, board_next_card to claim work, board_add_note to report \
            progress and board_update_card to move a card when you finish it.\n\
            Browser: the project's browser, shown to the user in PilotHouse (logins persist). Use \
            browser_open, then browser_snapshot to find elements and their selectors, \
            browser_click / browser_type / browser_press to act, and browser_screenshot to check \
            the result visually. browser_logs shows console errors and failed requests.\n\
            Devices: Android emulators and phones connected over adb. device_list first, then \
            device_screenshot and device_ui to see the screen, device_tap / device_swipe / \
            device_type / device_key to act, device_logcat for logs.",
    })
}
