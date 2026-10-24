//! Run tools: what is listening locally.

use serde_json::json;

use super::{Tool, with_project};

pub const TOOLS: &[Tool] = &[Tool {
    name: "ports_list",
    description: "List local TCP ports that servers are listening on, with the process, pid and its working directory. Useful to find the dev server to open with browser_open.",
    schema: || json!({ "type": "object", "properties": {} }),
    build: |arguments, project_id, _| Ok(("runs.ports", with_project(arguments, project_id))),
    format: |result| {
        let lines: Vec<String> = result["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|port| {
                format!(
                    ":{} {} (pid {}){}",
                    port["port"],
                    port["process"].as_str().unwrap_or_default(),
                    port["pid"],
                    port["cwd"]
                        .as_str()
                        .map(|cwd| format!(" in {cwd}"))
                        .unwrap_or_default()
                )
            })
            .collect();
        if lines.is_empty() {
            "Nothing is listening.".to_string()
        } else {
            lines.join("\n")
        }
    },
}];
