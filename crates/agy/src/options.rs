//! The choices a session offers (model, effort, mode) and the agy flags
//! they become.
//!
//! `agy models` lists each model once per effort (`gemini-4-argon-high` is
//! "Gemini 4 Argon (High)"). The picker shows each model once, with effort as
//! its own option; the pair is turned back into the variant's id.

use anyhow::{Result, bail};
use serde_json::{Value, json};

/// The effort levels agy's model variants come in.
const EFFORTS: [(&str, &str); 3] = [("low", "Low"), ("medium", "Medium"), ("high", "High")];

/// The model choice that leaves the pick to agy.
const DEFAULT_MODEL: &str = "default";

/// One model, with its per-effort variants.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// The id without the effort suffix.
    pub id: String,
    pub name: String,
    /// `(effort, full id)`; effort is `None` for a model without variants.
    pub variants: Vec<(Option<&'static str>, String)>,
}

/// Parses `agy models` output: `id<TAB>name` per line.
pub fn parse_models(output: &str) -> Vec<Model> {
    let mut models: Vec<Model> = Vec::new();
    for line in output.lines() {
        let Some((id, name)) = line.split_once('\t') else {
            continue; // "Fetching available models..."
        };
        let (id, name) = (id.trim(), name.trim());
        let variant = EFFORTS.iter().find_map(|(effort, label)| {
            let base_id = id.strip_suffix(&format!("-{effort}"))?;
            let base_name = name.strip_suffix(&format!(" ({label})"))?;
            Some((*effort, base_id, base_name))
        });
        let (effort, base_id, base_name) = match variant {
            Some((effort, base_id, base_name)) => (Some(effort), base_id, base_name),
            None => (None, id, name),
        };
        match models.iter_mut().find(|model| model.id == base_id) {
            Some(model) => model.variants.push((effort, id.to_string())),
            None => models.push(Model {
                id: base_id.to_string(),
                name: base_name.to_string(),
                variants: vec![(effort, id.to_string())],
            }),
        }
    }
    models
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// agy's default: actions that need approval are refused, then the
    /// bridge asks the user and continues if allowed.
    Ask,
    AcceptEdits,
    Auto,
    Plan,
    /// `--dangerously-skip-permissions`.
    FullAccess,
}

impl Mode {
    const ALL: [Mode; 5] = [
        Mode::Ask,
        Mode::AcceptEdits,
        Mode::Auto,
        Mode::Plan,
        Mode::FullAccess,
    ];

    fn value(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::AcceptEdits => "accept-edits",
            Mode::Auto => "auto",
            Mode::Plan => "plan",
            Mode::FullAccess => "full-access",
        }
    }

    fn label(self) -> (&'static str, &'static str) {
        match self {
            Mode::Ask => ("Ask", "Asks before actions that need approval"),
            Mode::AcceptEdits => ("Accept edits", "Edits files without asking"),
            Mode::Auto => ("Auto", "agy decides what needs approval"),
            Mode::Plan => ("Plan", "Reads and plans; changes nothing"),
            Mode::FullAccess => ("Full access", "Runs every action without asking"),
        }
    }

    /// Whether a refused action is worth asking the user about.
    pub fn asks_when_refused(self) -> bool {
        !matches!(self, Mode::Plan | Mode::FullAccess)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub model: String,
    pub effort: &'static str,
    pub mode: Mode,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.into(),
            effort: "medium",
            mode: Mode::Ask,
        }
    }
}

impl Settings {
    /// Applies one config option change from the client.
    pub fn set(&mut self, models: &[Model], config: &str, value: &str) -> Result<()> {
        match config {
            "model" if value == DEFAULT_MODEL || models.iter().any(|model| model.id == value) => {
                self.model = value.to_string();
            }
            "effort" => match EFFORTS.iter().find(|(effort, _)| *effort == value) {
                Some((effort, _)) => self.effort = effort,
                None => bail!("unknown effort `{value}`"),
            },
            "mode" => match Mode::ALL.iter().find(|mode| mode.value() == value) {
                Some(mode) => self.mode = *mode,
                None => bail!("unknown mode `{value}`"),
            },
            _ => bail!("unknown option `{config}` = `{value}`"),
        }
        Ok(())
    }

    /// The agy flags for these settings.
    pub fn flags(&self, models: &[Model], skip_permissions: bool) -> Vec<String> {
        let mut flags = Vec::new();
        match models.iter().find(|model| model.id == self.model) {
            Some(model) => flags.push(format!("--model={}", variant(model, self.effort))),
            // agy's own model; effort as a flag.
            None => flags.push(format!("--effort={}", self.effort)),
        }
        match self.mode {
            Mode::Ask | Mode::FullAccess => {}
            mode => flags.push(format!("--mode={}", mode.value())),
        }
        if skip_permissions || self.mode == Mode::FullAccess {
            flags.push("--dangerously-skip-permissions".into());
        }
        flags
    }

    /// The options as ACP `configOptions`.
    pub fn config_options(&self, models: &[Model]) -> Value {
        let mut model_choices = vec![json!({
            "value": DEFAULT_MODEL,
            "name": "Default",
            "description": "agy's default model",
        })];
        model_choices.extend(models.iter().map(|model| {
            json!({ "value": model.id, "name": model.name, "description": vendor(&model.id) })
        }));
        let efforts: Vec<Value> = EFFORTS
            .iter()
            .map(|(value, name)| json!({ "value": value, "name": name }))
            .collect();
        let modes: Vec<Value> = Mode::ALL
            .iter()
            .map(|mode| {
                let (name, description) = mode.label();
                json!({ "value": mode.value(), "name": name, "description": description })
            })
            .collect();
        json!([
            { "id": "model", "name": "Model", "category": "model", "type": "select",
              "currentValue": self.model, "options": model_choices },
            { "id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
              "currentValue": self.effort, "options": efforts },
            { "id": "mode", "name": "Mode", "category": "mode", "type": "select",
              "currentValue": self.mode.value(), "options": modes },
        ])
    }
}

/// The model's id at `effort`, else its medium or first variant.
fn variant(model: &Model, effort: &str) -> String {
    let pick = |wanted: &str| {
        model
            .variants
            .iter()
            .find(|(level, _)| *level == Some(wanted))
            .map(|(_, id)| id.clone())
    };
    pick(effort)
        .or_else(|| pick("medium"))
        .or_else(|| model.variants.first().map(|(_, id)| id.clone()))
        .unwrap_or_else(|| model.id.clone())
}

/// Who makes the model, from its id.
fn vendor(id: &str) -> &'static str {
    if id.starts_with("gemini") {
        "Google"
    } else if id.starts_with("claude") {
        "Anthropic"
    } else if id.starts_with("gpt") {
        "OpenAI"
    } else {
        "In Antigravity"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = "Fetching available models...\n\
        gemini-4-argon-medium\tGemini 4 Argon (Medium)\n\
        gemini-4-argon-high\tGemini 4 Argon (High)\n\
        gemini-4-argon-low\tGemini 4 Argon (Low)\n\
        gpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n\
        plain-model\tPlain Model\n";

    #[test]
    fn groups_effort_variants() {
        let models = parse_models(OUTPUT);
        let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["gemini-4-argon", "gpt-oss-120b", "plain-model"]);
        assert_eq!(models[0].name, "Gemini 4 Argon");
        assert_eq!(models[0].variants.len(), 3);
        assert_eq!(models[2].variants, vec![(None, "plain-model".to_string())]);
    }

    #[test]
    fn settings_become_flags() {
        let models = parse_models(OUTPUT);
        let mut settings = Settings::default();
        assert_eq!(settings.flags(&models, false), ["--effort=medium"]);

        settings.set(&models, "model", "gemini-4-argon").unwrap();
        settings.set(&models, "effort", "high").unwrap();
        settings.set(&models, "mode", "plan").unwrap();
        assert_eq!(
            settings.flags(&models, false),
            ["--model=gemini-4-argon-high", "--mode=plan"]
        );
        // A model without the chosen effort uses its closest variant.
        settings.set(&models, "model", "gpt-oss-120b").unwrap();
        settings.set(&models, "mode", "ask").unwrap();
        assert_eq!(
            settings.flags(&models, true),
            [
                "--model=gpt-oss-120b-medium",
                "--dangerously-skip-permissions"
            ]
        );
        assert!(settings.set(&models, "model", "nope").is_err());
        assert!(settings.set(&models, "mode", "nope").is_err());
    }
}
