//! The user's to-dos: what an agent can leave for the user to do
//! themselves (sign in somewhere, decide something, review a page).

use serde_json::{Value, json};

use super::{Tool, with_project};

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "todo_add",
        description: "Leave the user a to-do in PilotHouse's inbox, for something only they can do: sign in somewhere, decide something, check a page, reply to someone. With remind_in_minutes, PilotHouse reminds them then. Do not use it for your own work steps (use the board for those).",
        schema: || {
            json!({ "type": "object", "required": ["text"], "properties": {
                "text": { "type": "string", "description": "What to do, in one line." },
                "remind_in_minutes": { "type": "integer", "description": "Remind the user after this many minutes." },
                "all_workspaces": { "type": "boolean", "description": "Show it in every workspace, not just this one." }
            } })
        },
        build: |arguments, project_id, _| Ok(("todo.add", with_project(arguments, project_id))),
        format: |result| {
            let text = result["text"].as_str().unwrap_or_default();
            if result["remind_at"].is_null() {
                format!("Added to the user's to-dos: {text}")
            } else {
                format!("Added to the user's to-dos, with a reminder: {text}")
            }
        },
    },
    Tool {
        name: "todo_list",
        description: "List the user's to-dos in PilotHouse for this workspace (and the ones for all workspaces).",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| Ok(("todo.list", with_project(arguments, project_id))),
        format: |result| {
            let todos = result["todos"].as_array().cloned().unwrap_or_default();
            if todos.is_empty() {
                return "The user has no to-dos.".to_string();
            }
            todos
                .iter()
                .map(|todo: &Value| {
                    format!(
                        "[{}] {}",
                        if todo["done"].as_bool() == Some(true) {
                            "x"
                        } else {
                            " "
                        },
                        todo["text"].as_str().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        },
    },
];
