//! The offline utilities (formats, encodings, hashes, times, regex, SQLite),
//! the same ones as PilotHouse's Utilities dialog.

use serde_json::json;

use super::{Tool, with_project};

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "util_list",
        description: "List PilotHouse's offline utilities: JSON/TOML/XML/CSV, Base64/URL/hex/HTML encoding, JWT decoding, hashes and HMAC, UUIDs and ULIDs, Unix times, cron lines, regex, text diff, colors, Markdown to HTML, read-only SQLite queries. Each has an id and modes for util_run.",
        schema: || json!({ "type": "object", "properties": {} }),
        build: |arguments, _, _| Ok(("util.list", arguments)),
        format: |result| {
            result["utilities"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|utility| {
                    let modes: Vec<&str> = utility["modes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|mode| mode.as_str())
                        .collect();
                    format!(
                        "{} ({}): {}. Modes: {}",
                        utility["id"].as_str().unwrap_or_default(),
                        utility["name"].as_str().unwrap_or_default(),
                        utility["input"].as_str().unwrap_or_default(),
                        modes.join(", ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        },
    },
    Tool {
        name: "util_run",
        description: "Run one of PilotHouse's offline utilities (see util_list) on some text, e.g. decode a JWT, format JSON, explain a cron line, or query a SQLite file of this workspace read-only.",
        schema: || {
            json!({ "type": "object", "required": ["id", "text"], "properties": {
                "id": { "type": "string", "description": "The utility, e.g. jwt, json, cron, sqlite." },
                "text": { "type": "string", "description": "The main input." },
                "second": { "type": "string", "description": "The second input for utilities that take one (diff, hmac key, regex text, SQL query)." },
                "mode": { "type": "string", "description": "A mode's name from util_list; default the first." }
            } })
        },
        // The workspace goes along, so a SQLite file must be one of its own.
        build: |arguments, project_id, _| Ok(("util.run", with_project(arguments, project_id))),
        format: |result| result["output"].as_str().unwrap_or_default().to_string(),
    },
];
