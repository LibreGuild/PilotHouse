//! The tools the server offers, and how each maps to a control method.

mod artifacts;
mod browser;
mod chrome;
mod decks;
mod devices;
mod runs;
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
    use ph_brand::{Feature, enabled};
    // Whether the build has each group, and the group.
    let groups: [(bool, &'static [Tool]); 11] = [
        (enabled(Feature::Artifacts), artifacts::TOOLS),
        (enabled(Feature::Browser), browser::TOOLS),
        (enabled(Feature::ChromeExtension), chrome::TOOLS),
        // A deck is an artifact of a kind: without artifacts there are none.
        (
            enabled(Feature::Presentations) && enabled(Feature::Artifacts),
            decks::TOOLS,
        ),
        (enabled(Feature::Devices), devices::TOOLS),
        (enabled(Feature::Runs), runs::TOOLS),
        (true, studio::TOOLS),
        (enabled(Feature::Tests), tests::TOOLS),
        (true, theme::TOOLS),
        (true, todos::TOOLS),
        (enabled(Feature::Utilities), utils::TOOLS),
    ];
    groups
        .into_iter()
        .filter(|(there, _)| *there)
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
