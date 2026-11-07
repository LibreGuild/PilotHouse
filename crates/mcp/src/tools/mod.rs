//! The tools the server offers, and how each maps to a control method.

mod board;
mod browser;
mod chrome;
mod devices;
mod runs;
mod slides;
mod studio;
mod tests;
mod theme;
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

/// Every tool agents get: those of the features this build has (the brand's
/// `[features]`; a group with no feature is always there).
pub fn all() -> impl Iterator<Item = &'static Tool> {
    use ph_brand::Feature;
    let groups: [(Option<Feature>, &'static [Tool]); 11] = [
        (Some(Feature::Board), board::TOOLS),
        (Some(Feature::Browser), browser::TOOLS),
        (Some(Feature::ChromeExtension), chrome::TOOLS),
        (Some(Feature::Devices), devices::TOOLS),
        (Some(Feature::Runs), runs::TOOLS),
        (Some(Feature::Presentations), slides::TOOLS),
        (None, studio::TOOLS),
        (Some(Feature::Tests), tests::TOOLS),
        (None, theme::TOOLS),
        (None, todos::TOOLS),
        (Some(Feature::Utilities), utils::TOOLS),
    ];
    groups
        .into_iter()
        .filter(|(feature, _)| feature.is_none_or(ph_brand::enabled))
        .flat_map(|(_, tools)| tools.iter())
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
