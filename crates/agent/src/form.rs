//! Questions an agent asks with a form (ACP `elicitation/create`, form
//! mode): Claude Code's AskUserQuestion, an MCP server asking for a value.
//! The schema is read into a few kinds of fields the chat can answer
//! without a form engine: pick one, pick several, a flag, free text.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// A form an agent wants filled in.
#[derive(Debug, Clone)]
pub struct Form {
    /// What it asks, e.g. "Which database should we use?".
    pub message: String,
    pub fields: Vec<Field>,
    /// The choices picked per field.
    picks: Vec<BTreeSet<usize>>,
    /// The text typed per field (its own text, or its "Other" answer).
    texts: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Field {
    key: String,
    /// A short label ("Database").
    pub title: Option<String>,
    /// The question, when the form holds several.
    pub prompt: Option<String>,
    pub kind: FieldKind,
    /// The companion field for an answer in the user's own words.
    other_key: Option<String>,
    required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    One(Vec<Choice>),
    Many(Vec<Choice>),
    Text,
    Number { integer: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    value: Value,
    pub label: String,
    pub detail: Option<String>,
}

impl Field {
    pub fn choices(&self) -> &[Choice] {
        match &self.kind {
            FieldKind::One(choices) | FieldKind::Many(choices) => choices,
            _ => &[],
        }
    }

    /// Whether the user can type an answer for it.
    pub fn takes_text(&self) -> bool {
        self.other_key.is_some() || matches!(self.kind, FieldKind::Text | FieldKind::Number { .. })
    }
}

impl Form {
    /// Reads an `elicitation/create` request in form mode.
    pub fn parse(params: &Value) -> Option<Form> {
        if params["mode"].as_str() != Some("form") {
            return None;
        }
        let schema = &params["requestedSchema"];
        let properties = schema["properties"].as_object()?;
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|keys| keys.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut fields: Vec<Field> = Vec::new();
        for (key, property) in properties {
            // `question_0_custom` is the "Other" box of `question_0`.
            if let Some(owner) = key.strip_suffix("_custom")
                && properties.contains_key(owner)
            {
                continue;
            }
            let Some(kind) = kind_of(property) else {
                continue;
            };
            let other_key = format!("{key}_custom");
            fields.push(Field {
                key: key.clone(),
                title: text(&property["title"]),
                prompt: text(&property["description"]),
                kind,
                other_key: properties.contains_key(&other_key).then_some(other_key),
                required: required.contains(&key.as_str()),
            });
        }
        if fields.is_empty() {
            return None;
        }
        let count = fields.len();
        Some(Form {
            message: text(&params["message"]).unwrap_or_else(|| "The agent asks".into()),
            fields,
            picks: vec![BTreeSet::new(); count],
            texts: vec![String::new(); count],
        })
    }

    /// What it asks, in a line: the message, or with several questions
    /// (whose message is generic) the first question and how many more.
    pub fn headline(&self) -> String {
        match self.fields.as_slice() {
            [first, rest @ ..] if !rest.is_empty() => match &first.prompt {
                Some(prompt) => format!("{prompt} (+{} more)", rest.len()),
                None => self.message.clone(),
            },
            _ => self.message.clone(),
        }
    }

    pub fn is_picked(&self, field: usize, choice: usize) -> bool {
        self.picks
            .get(field)
            .is_some_and(|picks| picks.contains(&choice))
    }

    pub fn text(&self, field: usize) -> &str {
        self.texts.get(field).map_or("", String::as_str)
    }

    /// Picks `choice` (one-of fields drop the previous pick; many-of
    /// fields toggle it).
    pub fn pick(&mut self, field: usize, choice: usize) {
        let Some(kind) = self.fields.get(field).map(|field| &field.kind) else {
            return;
        };
        let picks = &mut self.picks[field];
        match kind {
            FieldKind::One(_) => {
                picks.clear();
                picks.insert(choice);
            }
            FieldKind::Many(_) if !picks.remove(&choice) => {
                picks.insert(choice);
            }
            _ => {}
        }
    }

    pub fn set_text(&mut self, field: usize, text: String) {
        if let Some(slot) = self.texts.get_mut(field) {
            *slot = text.trim().to_string();
        }
    }

    /// A single question with one answer: picking it answers the form.
    pub fn answers_on_pick(&self) -> bool {
        matches!(self.fields.as_slice(), [field] if matches!(field.kind, FieldKind::One(_)))
    }

    /// Every question has an answer (fields with choices always need
    /// one; text fields only when required).
    pub fn is_ready(&self) -> bool {
        self.fields.iter().enumerate().all(|(ix, field)| {
            let answered = !self.picks[ix].is_empty() || !self.texts[ix].is_empty();
            answered || (field.choices().is_empty() && !field.required)
        })
    }

    /// The answers, keyed as the schema asked.
    pub fn content(&self) -> Map<String, Value> {
        let mut content = Map::new();
        for (ix, field) in self.fields.iter().enumerate() {
            let picked: Vec<Value> = self.picks[ix]
                .iter()
                .filter_map(|choice| field.choices().get(*choice))
                .map(|choice| choice.value.clone())
                .collect();
            let text = &self.texts[ix];
            match &field.kind {
                FieldKind::One(_) => {
                    if let Some(value) = picked.into_iter().next() {
                        content.insert(field.key.clone(), value);
                    }
                }
                FieldKind::Many(_) => {
                    if !picked.is_empty() {
                        content.insert(field.key.clone(), Value::Array(picked));
                    }
                }
                FieldKind::Text => {
                    if !text.is_empty() {
                        content.insert(field.key.clone(), Value::String(text.clone()));
                    }
                }
                FieldKind::Number { integer } => {
                    let number = if *integer {
                        text.parse::<i64>().ok().map(Value::from)
                    } else {
                        text.parse::<f64>().ok().map(Value::from)
                    };
                    if let Some(number) = number {
                        content.insert(field.key.clone(), number);
                    }
                }
            }
            if let Some(other) = &field.other_key
                && !text.is_empty()
            {
                content.insert(other.clone(), Value::String(text.clone()));
            }
        }
        content
    }

    /// The answers in a line, for the answered card and the transcript.
    pub fn summary(&self) -> String {
        self.fields
            .iter()
            .enumerate()
            .map(|(ix, field)| {
                let mut parts: Vec<String> = self.picks[ix]
                    .iter()
                    .filter_map(|choice| field.choices().get(*choice))
                    .map(|choice| choice.label.clone())
                    .collect();
                if !self.texts[ix].is_empty() {
                    parts.push(format!("“{}”", self.texts[ix]));
                }
                parts.join(", ")
            })
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// The field a JSON-schema property describes, if the chat can show it.
fn kind_of(property: &Value) -> Option<FieldKind> {
    match property["type"].as_str()? {
        "string" => Some(match choices(property) {
            Some(choices) => FieldKind::One(choices),
            None => FieldKind::Text,
        }),
        "array" => choices(&property["items"]).map(FieldKind::Many),
        "boolean" => Some(FieldKind::One(vec![
            Choice {
                value: Value::Bool(true),
                label: "Yes".into(),
                detail: None,
            },
            Choice {
                value: Value::Bool(false),
                label: "No".into(),
                detail: None,
            },
        ])),
        "number" => Some(FieldKind::Number { integer: false }),
        "integer" => Some(FieldKind::Number { integer: true }),
        _ => None,
    }
}

/// `oneOf`/`anyOf` of `{const, title, description}`, or a plain `enum`.
fn choices(schema: &Value) -> Option<Vec<Choice>> {
    let titled = schema["oneOf"]
        .as_array()
        .or_else(|| schema["anyOf"].as_array());
    let choices: Vec<Choice> = match titled {
        Some(options) => options
            .iter()
            .filter_map(|option| {
                let value = option.get("const")?.clone();
                let label = text(&option["title"])
                    .or_else(|| value.as_str().map(str::to_string))
                    .unwrap_or_else(|| value.to_string());
                // A description that only repeats the label says nothing.
                let detail = text(&option["description"]).filter(|detail| *detail != label);
                Some(Choice {
                    value,
                    label,
                    detail,
                })
            })
            .collect(),
        None => schema["enum"]
            .as_array()?
            .iter()
            .map(|value| Choice {
                label: value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string()),
                value: value.clone(),
                detail: None,
            })
            .collect(),
    };
    (!choices.is_empty()).then_some(choices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ask_user_question() -> Value {
        json!({
            "mode": "form",
            "sessionId": "s",
            "message": "Please answer the following questions.",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "question_0": {
                        "type": "string", "title": "Database", "description": "Which database?",
                        "oneOf": [
                            {"const": "Postgres", "title": "Postgres", "description": "Relational"},
                            {"const": "SQLite", "title": "SQLite"}
                        ]
                    },
                    "question_0_custom": {"type": "string", "title": "Other"},
                    "question_1": {
                        "type": "array", "title": "Extras",
                        "items": {"anyOf": [{"const": "Auth", "title": "Auth"}, {"const": "Cache", "title": "Cache"}]}
                    },
                    "question_1_custom": {"type": "string", "title": "Other"}
                }
            }
        })
    }

    #[test]
    fn reads_ask_user_question_forms() {
        let mut form = Form::parse(&ask_user_question()).unwrap();
        assert_eq!(
            form.fields.len(),
            2,
            "the Other boxes belong to their questions"
        );
        assert!(form.fields[0].takes_text());
        assert_eq!(
            form.fields[0].choices()[0].detail.as_deref(),
            Some("Relational")
        );
        assert!(!form.answers_on_pick());
        assert!(!form.is_ready());
        form.pick(0, 1);
        form.pick(0, 0);
        form.pick(1, 0);
        form.pick(1, 1);
        form.pick(1, 0);
        assert!(form.is_ready());
        form.set_text(0, " with pgvector ".into());
        assert_eq!(
            Value::Object(form.content()),
            json!({"question_0": "Postgres", "question_0_custom": "with pgvector", "question_1": ["Cache"]})
        );
        assert_eq!(form.summary(), "Postgres, “with pgvector” · Cache");
        assert_eq!(form.headline(), "Which database? (+1 more)");
    }

    #[test]
    fn plain_fields_and_single_questions() {
        let form = Form::parse(&json!({
            "mode": "form", "message": "Confirm",
            "requestedSchema": {"type": "object", "required": ["name"], "properties": {
                "ok": {"type": "boolean"},
                "name": {"type": "string"},
                "count": {"type": "integer"}
            }}
        }))
        .unwrap();
        assert_eq!(form.fields.len(), 3);
        assert!(!form.is_ready(), "the flag and the required name");
        let single = Form::parse(&json!({
            "mode": "form", "message": "Pick",
            "requestedSchema": {"type": "object", "properties": {
                "q": {"type": "string", "enum": ["a", "b"]}
            }}
        }))
        .unwrap();
        assert!(single.answers_on_pick());
        assert!(Form::parse(&json!({"mode": "url", "url": "https://x"})).is_none());
    }
}
