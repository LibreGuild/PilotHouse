//! Run tools: what is listening locally, and the workspace's run
//! configurations, which an agent adds when asked and the user starts.

use serde_json::json;

use super::{Tool, with_project};

pub const TOOLS: &[Tool] = &[
    Tool {
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
    },
    Tool {
        name: "run_list",
        description: concat!(
            "List this workspace's run configurations: those ",
            ph_brand::name!(),
            " found in the project (package.json scripts, cargo, Gradle, Makefile targets…) and those added. Look here before adding one."
        ),
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, project_id, _| Ok(("runs.configs", with_project(arguments, project_id))),
        format: |result| {
            let lines: Vec<String> = result["configs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|config| {
                    format!(
                        "{}: `{}`{}{}",
                        config["name"].as_str().unwrap_or_default(),
                        config["command"].as_str().unwrap_or_default(),
                        config["folder"]
                            .as_str()
                            .map(|folder| format!(" in {folder}"))
                            .unwrap_or_default(),
                        if config["added"] == true {
                            " (added)"
                        } else {
                            ""
                        }
                    )
                })
                .collect();
            if lines.is_empty() {
                "This workspace has no run configurations yet.".to_string()
            } else {
                lines.join("\n")
            }
        },
    },
    Tool {
        name: "run_add",
        description: concat!(
            "Add a run configuration to this workspace's Run tab: a name and the command it runs. Use it when the user asks for a run configuration, or for a way to start, build or check something with one click. The user starts it with the Run tab's button, in a terminal of the workspace: you do not start it (run a command in your own shell when you need its result). The command is one line and runs in the project folder, or in `folder` (a folder of the project, said from the project). One of the same name is replaced, which is how a configuration is changed. Those ",
            ph_brand::name!(),
            " finds in the project are listed already: see run_list first."
        ),
        schema: || {
            json!({ "type": "object", "required": ["name", "command"], "properties": {
            "name": { "type": "string", "description": "What the button says, like \"Dev server\" or \"Seed the database\"." },
            "command": { "type": "string", "description": "The command, as typed in a terminal: one line." },
            "folder": { "type": "string", "description": "A folder of the project to run it in, like \"web\". Left out: the project folder." }
        } })
        },
        build: |arguments, project_id, _| Ok(("runs.add", with_project(arguments, project_id))),
        format: |result| {
            format!(
                "{} “{}” in the Run tab: `{}`. The user starts it there.",
                if result["replaced"] == true {
                    "Changed"
                } else {
                    "Added"
                },
                result["name"].as_str().unwrap_or_default(),
                result["command"].as_str().unwrap_or_default()
            )
        },
    },
    Tool {
        name: "run_remove",
        description: "Remove a run configuration that was added to this workspace (by name). Those found in the project's own files are not removed here: they come from those files.",
        schema: || {
            json!({ "type": "object", "required": ["name"], "properties": {
            "name": { "type": "string", "description": "The configuration's name, as run_list says it." }
        } })
        },
        build: |arguments, project_id, _| Ok(("runs.remove", with_project(arguments, project_id))),
        format: |result| {
            format!(
                "Removed “{}” from the Run tab.",
                result["name"].as_str().unwrap_or_default()
            )
        },
    },
];
