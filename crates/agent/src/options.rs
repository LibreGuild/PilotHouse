//! Agent session options: model, mode, thinking effort and anything else the
//! agent offers (ACP "session config options").
//!
//! Options are read leniently from JSON so a newer agent's extra kinds are
//! skipped instead of breaking the chat. Agents on the older protocol (Gemini
//! CLI) offer their models and modes as `models` and `modes` instead, set
//! with `session/set_model` and `session/set_mode`; those become options too. The last options an agent offered are
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

/// How an option is changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionSource {
    /// `session/set_config_option`.
    #[default]
    ConfigOption,
    /// The older `models` list, `session/set_model`.
    Models,
    /// The older `modes` list, `session/set_mode`.
    Modes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentOption {
    pub id: String,
    pub name: String,
    pub category: OptionCategory,
    /// The selected value; `"true"`/`"false"` for toggles.
    pub current: String,
    pub kind: OptionKind,
    #[serde(default)]
    pub source: OptionSource,
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

    /// The JSON-RPC method and params that set this option to `value`.
    pub fn set_request(&self, session_id: &str, value: &str) -> (&'static str, Value) {
        match self.source {
            OptionSource::ConfigOption => (
                "session/set_config_option",
                self.set_params(session_id, value),
            ),
            OptionSource::Models => (
                "session/set_model",
                json!({ "sessionId": session_id, "modelId": value }),
            ),
            OptionSource::Modes => (
                "session/set_mode",
                json!({ "sessionId": session_id, "modeId": value }),
            ),
        }
    }

    fn set_params(&self, session_id: &str, value: &str) -> Value {
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

/// The options a `session/new` (or `session/load`) result offers: its
/// `configOptions`, then a model and a mode from the older `models` and
/// `modes` lists when the config options have none.
pub fn parse_session_options(result: &Value) -> Vec<AgentOption> {
    let mut options = parse_options(result.get("configOptions").unwrap_or(&Value::Null));
    let has = |options: &[AgentOption], category| {
        options.iter().any(|option| option.category == category)
    };
    if !has(&options, OptionCategory::Model)
        && let Some(models) = result.get("models")
    {
        options.extend(parse_legacy(
            models,
            ("availableModels", "currentModelId", "modelId"),
            (
                "model",
                "Model",
                OptionCategory::Model,
                OptionSource::Models,
            ),
        ));
    }
    if !has(&options, OptionCategory::Mode)
        && let Some(modes) = result.get("modes")
    {
        options.extend(parse_legacy(
            modes,
            ("availableModes", "currentModeId", "id"),
            ("mode", "Mode", OptionCategory::Mode, OptionSource::Modes),
        ));
    }
    options
}

/// One older-protocol list (`models` or `modes`) as a select option.
fn parse_legacy(
    state: &Value,
    (list, current, value): (&str, &str, &str),
    (id, name, category, source): (&str, &str, OptionCategory, OptionSource),
) -> Option<AgentOption> {
    let choices: Vec<OptionChoice> = state
        .get(list)?
        .as_array()?
        .iter()
        .filter_map(|entry| {
            let value = entry.get(value)?.as_str()?.to_string();
            Some(OptionChoice {
                name: entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&value)
                    .to_string(),
                value,
                description: entry
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    if choices.is_empty() {
        return None;
    }
    Some(AgentOption {
        id: id.to_string(),
        name: name.to_string(),
        category,
        current: state.get(current)?.as_str()?.to_string(),
        kind: OptionKind::Select(choices),
        source,
    })
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
        source: OptionSource::ConfigOption,
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
        let (method, params) = options[0].set_request("s", "haiku");
        assert_eq!(method, "session/set_config_option");
        assert_eq!(params["value"], "haiku");
    }

    #[test]
    fn reads_older_models_and_modes() {
        // What Gemini CLI answers `session/new` with.
        let result = json!({
            "sessionId": "s",
            "models": {
                "availableModels": [
                    { "modelId": "auto", "name": "Auto", "description": "Picks for you" },
                    { "modelId": "gemini-2.5-pro", "name": "Gemini 2.5 Pro" }
                ],
                "currentModelId": "auto"
            },
            "modes": {
                "availableModes": [
                    { "id": "default", "name": "Default" },
                    { "id": "yolo", "name": "YOLO" }
                ],
                "currentModeId": "default"
            }
        });
        let options = parse_session_options(&result);
        assert_eq!(options.len(), 2);
        let (model, mode) = (&options[0], &options[1]);
        assert_eq!(model.category, OptionCategory::Model);
        assert_eq!(model.current_name(), "Auto");
        assert_eq!(
            model.set_request("s", "gemini-2.5-pro"),
            (
                "session/set_model",
                json!({ "sessionId": "s", "modelId": "gemini-2.5-pro" })
            )
        );
        assert_eq!(mode.category, OptionCategory::Mode);
        assert_eq!(
            mode.set_request("s", "yolo"),
            (
                "session/set_mode",
                json!({ "sessionId": "s", "modeId": "yolo" })
            )
        );
    }

    #[test]
    fn config_options_win_over_older_lists() {
        let result = json!({
            "configOptions": [
                { "id": "model", "name": "Model", "category": "model", "type": "select",
                  "currentValue": "a", "options": [ { "value": "a", "name": "A" } ] }
            ],
            "models": { "availableModels": [ { "modelId": "b", "name": "B" } ], "currentModelId": "b" }
        });
        let options = parse_session_options(&result);
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].source, OptionSource::ConfigOption);
    }

    #[test]
    fn cached_options_from_older_builds_still_load() {
        let old = json!([{ "id": "model", "name": "Model", "category": "model",
            "current": "a", "kind": { "Select": [] } }]);
        let options: Vec<AgentOption> = serde_json::from_value(old).unwrap();
        assert_eq!(options[0].source, OptionSource::ConfigOption);
    }
}
