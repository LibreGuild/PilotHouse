//! UI test tools: making a test from a chat (start its recording, save
//! it), the checks an agent adds while it records, and listing and running
//! the workspace's tests.

use serde_json::json;

use super::{Tool, with_project};

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "test_record",
        description: concat!(
            "Make a new UI test of this workspace and start recording it. Use it when the user asks for a UI test, an end-to-end test, or a check of a flow in the browser or on a phone (tests of code, like unit tests, are written in the project's own test framework instead). Say what it checks (`checks`, in plain words) and where it starts: `url` for a page, or `package` for an Android app. Then do the flow yourself with the browser_* tools (device_* for an app), acting on real controls (a selector from browser_snapshot, the middle of an element from device_ui) rather than a point you guessed: each action that works becomes a step that names its element by what it says. Call test_expect after each result the user should see. The recording starts where every run of the test will: the site signed out with nothing kept, or the app at its first screen, so sign in as a part of it when it needs that. A password you type into a password field is kept on this computer, never in the test's file. End with test_save, then test_run to see that it replays. ",
            ph_brand::name!(),
            " shows the test in its Tests tab."
        ),
        schema: || {
            json!({ "type": "object", "required": ["checks"], "properties": {
            "checks": { "type": "string", "description": "What the test should check, in plain words: \"Sign in as Ada, add a coffee to the cart; the cart should say Coffee.\"" },
            "url": { "type": "string", "description": "Where a test of a page starts, like http://localhost:3000." },
            "package": { "type": "string", "description": "The Android app a test of a phone starts, like com.example.app." },
            "name": { "type": "string", "description": "A short name. Left out, it is the first words of what it checks." }
        } })
        },
        build: |arguments, project_id, _| Ok(("test.record", with_project(arguments, project_id))),
        format: |result| {
            format!(
                "Recording “{}” ({}). Do the flow now with the {} tools, with test_expect for what should show, then call test_save.",
                result["name"].as_str().unwrap_or_default(),
                result["file"].as_str().unwrap_or_default(),
                if result["android"] == true {
                    "device_*"
                } else {
                    "browser_*"
                }
            )
        },
    },
    Tool {
        name: "test_save",
        description: "End the recording started with test_record and save the test: says the steps it has. (A recording still going when your turn ends is saved then.) Run it with test_run afterwards to see that it replays.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| Ok(("test.save", with_project(arguments, project_id))),
        format: |result| {
            let steps: Vec<String> = result["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|step| step.as_str())
                .enumerate()
                .map(|(at, step)| format!("{}. {step}", at + 1))
                .collect();
            let name = result["name"].as_str().unwrap_or_default();
            if steps.is_empty() {
                return format!(
                    "Saved “{name}” with no steps: nothing was done in the browser or on the device while it recorded."
                );
            }
            format!("Saved “{name}”:\n{}", steps.join("\n"))
        },
    },
    Tool {
        name: "test_expect",
        description: concat!(
            "Check that the browser page (or, with device: true, the Android screen) shows some text, does not show it, or that the page URL contains something. It waits a few seconds for the screen to settle and fails if the check does not hold. While ",
            ph_brand::name!(),
            " records a UI test, each passing check is saved as a step of the test."
        ),
        schema: || {
            json!({ "type": "object", "properties": {
            "kind": { "type": "string", "enum": ["text", "no_text", "url", "look"], "description": "Default text. `look` records a check of how the screen looks here (no value): later runs compare it with the look the user accepts." },
            "value": { "type": "string", "description": "The text to look for, or part of the URL." },
            "device": { "type": "boolean", "description": "Check the Android device's screen instead of the browser." }
        } })
        },
        build: |arguments, project_id, _| Ok(("test.expect", with_project(arguments, project_id))),
        format: |result| result["note"].as_str().unwrap_or("It holds.").to_string(),
    },
    Tool {
        name: "test_list",
        description: concat!(
            "List this workspace's UI tests (recorded in ",
            ph_brand::name!(),
            "'s Tests tab) with what each checks and how its newest run went, and the commands that run the project's own tests."
        ),
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| Ok(("test.list", with_project(arguments, project_id))),
        format: |result| {
            let tests = result["tests"].as_array().cloned().unwrap_or_default();
            let mut lines: Vec<String> = tests
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
                .collect();
            if lines.is_empty() {
                lines.push("There are no UI tests in this workspace yet.".to_string());
            }
            // The project's own tests, which are run in the shell.
            let suites = result["suites"].as_array().cloned().unwrap_or_default();
            if !suites.is_empty() {
                lines.push("The project's own tests (run them in your shell):".to_string());
                lines.extend(suites.iter().map(|suite| {
                    format!(
                        "{}: `{}` ({})",
                        suite["name"].as_str().unwrap_or_default(),
                        suite["command"].as_str().unwrap_or_default(),
                        suite["status"].as_str().unwrap_or_default()
                    )
                }));
            }
            lines.join("\n")
        },
    },
    Tool {
        name: "test_run",
        description: "Run one of this workspace's UI tests (by name, see test_list) the way the Tests tab does, without AI: it replays the recorded steps in the browser pane or on the Android device, from a clean start (signed out), and compares its look checks with the accepted look. Use it to check your change before you call it done. Takes up to a minute.",
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
                        "step {} ({} pixels changed)",
                        step["step"], step["changed_pixels"]
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
