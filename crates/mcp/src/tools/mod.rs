//! The tools the server offers, and how each maps to a control method.

mod board;
mod browser;
mod devices;
mod runs;
mod tests;
mod todos;
mod utils;

use anyhow::Result;
use serde_json::{Value, json};

/// One MCP tool.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema of the tool's arguments.
    pub schema: fn() -> Value,
    /// Turns tool arguments into a control-socket call.
    pub build: fn(Value, i64, &str) -> Result<(&'static str, Value)>,
    /// Renders the control result as the text the agent reads. Images in the
    /// result (`images: [{data, mimeType}]`) are added by the server.
    pub format: fn(&Value) -> String,
}

pub fn all() -> impl Iterator<Item = &'static Tool> {
    board::TOOLS
        .iter()
        .chain(browser::TOOLS)
        .chain(devices::TOOLS)
        .chain(runs::TOOLS)
        .chain(tests::TOOLS)
        .chain(todos::TOOLS)
        .chain(utils::TOOLS)
}

pub fn find(name: &str) -> Option<&'static Tool> {
    all().find(|tool| tool.name == name)
}

pub fn definitions() -> Vec<Value> {
    all()
        .map(|tool| json!({ "name": tool.name, "description": tool.description, "inputSchema": (tool.schema)() }))
        .collect()
}

/// Copies the agent's arguments and adds the project, for tools whose
/// arguments map one to one onto the control method's parameters.
fn with_project(mut arguments: Value, project_id: i64) -> Value {
    if !arguments.is_object() {
        arguments = json!({});
    }
    arguments["project_id"] = json!(project_id);
    arguments
}
