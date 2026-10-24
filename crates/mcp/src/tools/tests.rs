//! UI test tools: checks an agent adds while it records a test, and
//! listing and running the workspace's tests.

use serde_json::json;

use super::{Tool, with_project};

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "test_expect",
        description: "Check that the browser page (or, with device: true, the Android screen) shows some text, does not show it, or that the page URL contains something. It waits a few seconds for the screen to settle and fails if the check does not hold. While PilotHouse records a UI test, each passing check is saved as a step of the test.",
        schema: || {
            json!({ "type": "object", "required": ["value"], "properties": {
            "kind": { "type": "string", "enum": ["text", "no_text", "url"], "description": "Default text." },
            "value": { "type": "string", "description": "The text to look for, or part of the URL." },
            "device": { "type": "boolean", "description": "Check the Android device's screen instead of the browser." }
        } })
        },
        build: |arguments, project_id, _| Ok(("test.expect", with_project(arguments, project_id))),
        format: |result| result["note"].as_str().unwrap_or("It holds.").to_string(),
    },
    Tool {
        name: "test_list",
        description: "List this workspace's UI tests (recorded in PilotHouse's Tests tab) with what each checks and how its newest run went.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| Ok(("test.list", with_project(arguments, project_id))),
        format: |result| {
            let tests = result["tests"].as_array().cloned().unwrap_or_default();
            if tests.is_empty() {
                return "There are no UI tests in this workspace yet.".to_string();
            }
            tests
                .iter()
                .map(|test| {
                    format!(
                        "{} ({} steps, {}): {}",
                        test["name"].as_str().unwrap_or_default(),
                        test["steps"],
                        test["status"].as_str().unwrap_or_default(),
                        test["description"].as_str().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        },
    },
    Tool {
        name: "test_run",
        description: "Run one of this workspace's UI tests (by name, see test_list) the way the Tests tab does, without AI: it replays the recorded steps in the browser pane or on the Android device and compares the screens with the accepted baseline. Use it to check your change before moving a board card to done. Takes up to a minute.",
        schema: || {
            json!({ "type": "object", "required": ["name"], "properties": {
            "name": { "type": "string", "description": "The test's name." }
        } })
        },
        build: |arguments, project_id, _| Ok(("test.run", with_project(arguments, project_id))),
        format: |result| {
            let name = result["test"].as_str().unwrap_or("The test");
            if result["passed"].as_bool() == Some(true) {
                return format!("{name} passed.");
            }
            if let Some(failure) = result["failure"].as_object() {
                return format!(
                    "{name} failed at step {} ({}): {}",
                    failure["step"],
                    failure["does"].as_str().unwrap_or_default(),
                    failure["error"].as_str().unwrap_or_default()
                );
            }
            let steps: Vec<String> = result["looks_different"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|step| {
                    format!(
                        "step {} ({}% changed)",
                        step["step"], step["changed_percent"]
                    )
                })
                .collect();
            format!(
                "{name}: every step worked, but these look different from the accepted screens: {}. If the change is intended, ask the user to accept the new look in the Tests tab.",
                steps.join(", ")
            )
        },
    },
];
