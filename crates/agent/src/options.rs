//! Agent session options: model, mode, thinking effort and anything else the
//! agent offers (ACP "session config options").
//!
//! Options are read leniently from JSON so a newer agent's extra kinds are
//! skipped instead of breaking the chat. The last options an agent offered are
//! cached per agent, so the composer can show the model picker before the
//! agent has started; the user's choices are remembered per agent and applied
//! to every new session.

use std::collections::HashMap;

use ph_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionCategory {
    Mode,
    Model,
    Effort,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OptionKind {
    Select(Vec<OptionChoice>),
    Toggle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentOption {
    pub id: String,
    pub name: String,
    pub category: OptionCategory,
    /// The selected value; `"true"`/`"false"` for toggles.
    pub current: String,
    pub kind: OptionKind,
}

impl AgentOption {
    /// The selected choice's display name.
    pub fn current_name(&self) -> String {
        match &self.kind {
            OptionKind::Select(choices) => choices
                .iter()
                .find(|choice| choice.value == self.current)
                .map(|choice| choice.name.clone())
                .unwrap_or_else(|| self.current.clone()),
            OptionKind::Toggle => if self.current == "true" { "On" } else { "Off" }.into(),
        }
    }

    /// The JSON-RPC params that set this option to `value`.
    pub fn set_params(&self, session_id: &str, value: &str) -> Value {
        match self.kind {
            OptionKind::Toggle => json!({
                "sessionId": session_id,
                "configId": self.id,
                "type": "boolean",
                "value": value == "true",
            }),
            OptionKind::Select(_) => json!({
                "sessionId": session_id,
                "configId": self.id,
                "value": value,
            }),
        }
    }
}

/// Parses ACP `configOptions` (from `session/new` or a config update).
pub fn parse_options(config_options: &Value) -> Vec<AgentOption> {
    let Some(items) = config_options.as_array() else {
        return Vec::new();
    };
    items.iter().filter_map(parse_option).collect()
}

fn parse_option(item: &Value) -> Option<AgentOption> {
    let id = item.get("id")?.as_str()?.to_string();
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();
    let category = match item.get("category").and_then(Value::as_str) {
        Some("mode") => OptionCategory::Mode,
        Some("model") => OptionCategory::Model,
        Some("thought_level") => OptionCategory::Effort,
        _ => OptionCategory::Other,
    };
    let (kind, current) = match item.get("type").and_then(Value::as_str) {
        Some("boolean") => (
            OptionKind::Toggle,
            item.get("currentValue")?.as_bool()?.to_string(),
        ),
        _ => {
            let current = item.get("currentValue")?.as_str()?.to_string();
            let mut choices = Vec::new();
            for entry in item.get("options")?.as_array()? {
                // Grouped options nest their choices under `options`.
                match entry.get("options").and_then(Value::as_array) {
                    Some(group) => choices.extend(group.iter().filter_map(parse_choice)),
                    None => choices.extend(parse_choice(entry)),
                }
            }
            (OptionKind::Select(choices), current)
        }
    };
    Some(AgentOption {
        id,
        name,
        category,
        current,
        kind,
    })
}

fn parse_choice(entry: &Value) -> Option<OptionChoice> {
    Some(OptionChoice {
        value: entry.get("value")?.as_str()?.to_string(),
        name: entry.get("name").and_then(Value::as_str)?.to_string(),
        description: entry
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn cache_key(agent_id: &str) -> String {
    format!("agent.{agent_id}.options")
}

fn prefs_key(agent_id: &str) -> String {
    format!("agent.{agent_id}.prefs")
}

/// The options the agent offered last time (for the picker before it starts).
pub fn cached(store: &Store, agent_id: &str) -> Vec<AgentOption> {
    store.setting(&cache_key(agent_id))
}

pub fn cache(store: &Store, agent_id: &str, options: &[AgentOption]) {
    if let Err(err) = store.set_setting(&cache_key(agent_id), &options) {
        tracing::warn!("caching agent options failed: {err:#}");
    }
}

/// The user's chosen values, by option id.
pub fn preferences(store: &Store, agent_id: &str) -> HashMap<String, String> {
    store.setting(&prefs_key(agent_id))
}

pub fn remember(store: &Store, agent_id: &str, option_id: &str, value: &str) {
    let mut prefs = preferences(store, agent_id);
    prefs.insert(option_id.to_string(), value.to_string());
    if let Err(err) = store.set_setting(&prefs_key(agent_id), &prefs) {
        tracing::warn!("saving agent preference failed: {err:#}");
    }
}

/// Applies remembered choices to `options` for display before a session exists.
pub fn with_preferences(
    mut options: Vec<AgentOption>,
    prefs: &HashMap<String, String>,
) -> Vec<AgentOption> {
    for option in &mut options {
        if let Some(value) = prefs.get(&option.id) {
            option.current = value.clone();
        }
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_selects_groups_and_toggles() {
        let raw = json!([
            { "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "opus",
              "options": [ { "value": "opus", "name": "Opus" }, { "value": "haiku", "name": "Haiku" } ] },
            { "id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "high",
              "options": [ { "group": "g", "name": "Levels", "options": [ { "value": "high", "name": "High" } ] } ] },
            { "id": "fast", "name": "Fast", "type": "boolean", "currentValue": false },
            { "id": "weird", "type": "future_kind" }
        ]);
        let options = parse_options(&raw);
        assert_eq!(options.len(), 3);
        assert_eq!(options[0].category, OptionCategory::Model);
        assert_eq!(options[0].current_name(), "Opus");
        assert_eq!(options[1].category, OptionCategory::Effort);
        assert!(matches!(&options[1].kind, OptionKind::Select(choices) if choices.len() == 1));
        assert_eq!(options[2].kind, OptionKind::Toggle);
        assert_eq!(options[2].set_params("s", "true")["value"], true);
        assert_eq!(options[0].set_params("s", "haiku")["value"], "haiku");
    }
}
