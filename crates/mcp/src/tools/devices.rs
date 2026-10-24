//! Device tools: Android emulators and phones over adb.
//!
//! Every tool takes an optional `serial`; without it the device shown in the
//! PilotHouse Devices pane is used, or the only one connected.

use serde_json::{Value, json};

use super::{Tool, with_project};

fn serial() -> Value {
    json!({ "type": "string", "description": "Device serial from device_list. Optional." })
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "device_list",
        description: "List connected Android devices and emulators, and the emulators (AVDs) that can be started.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| Ok(("device.list", with_project(arguments, project_id))),
        format: |result| {
            let devices = result["devices"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|device| {
                    format!(
                        "{} — {} ({}, {})",
                        device["serial"].as_str().unwrap_or_default(),
                        device["name"].as_str().unwrap_or_default(),
                        if device["emulator"] == true {
                            "emulator"
                        } else {
                            "device"
                        },
                        device["state"].as_str().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>();
            let avds = result["avds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "Connected:\n{}\n\nEmulators available to start: {}",
                if devices.is_empty() {
                    "none".to_string()
                } else {
                    devices.join("\n")
                },
                if avds.is_empty() { "none" } else { &avds }
            )
        },
    },
    Tool {
        name: "device_start_emulator",
        description: "Start an Android emulator (AVD) by name and wait until it has booted. It is shown in the PilotHouse Devices pane.",
        schema: || json!({ "type": "object", "required": ["avd"], "properties": { "avd": { "type": "string" } } }),
        build: |arguments, project_id, _| {
            Ok(("device.start_emulator", with_project(arguments, project_id)))
        },
        format: |result| {
            format!(
                "Started {}.",
                result["serial"].as_str().unwrap_or("the emulator")
            )
        },
    },
    Tool {
        name: "device_screenshot",
        description: "Take a screenshot of the device screen. Coordinates in the image are device pixels, as device_tap expects.",
        schema: || json!({ "type": "object", "properties": { "serial": serial() } }),
        build: |arguments, project_id, _| {
            Ok(("device.screenshot", with_project(arguments, project_id)))
        },
        format: |result| format!("{}×{} px", result["width"], result["height"]),
    },
    Tool {
        name: "device_ui",
        description: "The on-screen UI hierarchy (uiautomator): each element's text, resource id, class and bounds, so you can tap the center of an element.",
        schema: || json!({ "type": "object", "properties": { "serial": serial() } }),
        build: |arguments, project_id, _| Ok(("device.ui", with_project(arguments, project_id))),
        format: |result| result["text"].as_str().unwrap_or_default().to_string(),
    },
    Tool {
        name: "device_tap",
        description: "Tap the screen at device-pixel coordinates.",
        schema: || {
            json!({ "type": "object", "required": ["x", "y"], "properties": {
                "x": { "type": "integer" }, "y": { "type": "integer" }, "serial": serial()
            } })
        },
        build: |arguments, project_id, _| Ok(("device.tap", with_project(arguments, project_id))),
        format: |_| "Tapped.".to_string(),
    },
    Tool {
        name: "device_swipe",
        description: "Swipe from (x1, y1) to (x2, y2) in device pixels. A long press is a swipe that does not move, with a duration of 800 ms or more.",
        schema: || {
            json!({ "type": "object", "required": ["x1", "y1", "x2", "y2"], "properties": {
                "x1": { "type": "integer" }, "y1": { "type": "integer" },
                "x2": { "type": "integer" }, "y2": { "type": "integer" },
                "duration_ms": { "type": "integer", "description": "Default 300." },
                "serial": serial()
            } })
        },
        build: |arguments, project_id, _| Ok(("device.swipe", with_project(arguments, project_id))),
        format: |_| "Swiped.".to_string(),
    },
    Tool {
        name: "device_type",
        description: "Type text into the focused field on the device.",
        schema: || json!({ "type": "object", "required": ["text"], "properties": { "text": { "type": "string" }, "serial": serial() } }),
        build: |arguments, project_id, _| Ok(("device.type", with_project(arguments, project_id))),
        format: |_| "Typed.".to_string(),
    },
    Tool {
        name: "device_key",
        description: "Press a key: back, home, enter, delete, tab, menu, recents, up, down, left, right, volume_up, volume_down, power.",
        schema: || json!({ "type": "object", "required": ["key"], "properties": { "key": { "type": "string" }, "serial": serial() } }),
        build: |arguments, project_id, _| Ok(("device.key", with_project(arguments, project_id))),
        format: |_| "Pressed.".to_string(),
    },
    Tool {
        name: "device_logcat",
        description: "Recent logcat lines from the device, oldest first.",
        schema: || {
            json!({ "type": "object", "properties": {
                "lines": { "type": "integer", "description": "Default 100." },
                "filter": { "type": "string", "description": "Only lines containing this text (a tag, package or message)." },
                "errors_only": { "type": "boolean" },
                "serial": serial()
            } })
        },
        build: |arguments, project_id, _| {
            Ok(("device.logcat", with_project(arguments, project_id)))
        },
        format: |result| match result["text"].as_str().filter(|text| !text.is_empty()) {
            Some(text) => text.to_string(),
            None => "No matching log lines.".to_string(),
        },
    },
    Tool {
        name: "device_install",
        description: "Install (or update) an APK on the device.",
        schema: || json!({ "type": "object", "required": ["apk"], "properties": { "apk": { "type": "string", "description": "Path to the .apk file." }, "serial": serial() } }),
        build: |arguments, project_id, _| {
            Ok(("device.install", with_project(arguments, project_id)))
        },
        format: |_| "Installed.".to_string(),
    },
    Tool {
        name: "device_launch",
        description: "Launch an installed app by package name, e.g. com.example.app.",
        schema: || json!({ "type": "object", "required": ["package"], "properties": { "package": { "type": "string" }, "serial": serial() } }),
        build: |arguments, project_id, _| {
            Ok(("device.launch", with_project(arguments, project_id)))
        },
        format: |_| "Launched.".to_string(),
    },
];
