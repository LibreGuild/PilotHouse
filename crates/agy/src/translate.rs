//! agy stream-json events to ACP `session/update`s, and ACP prompts to agy
//! user messages.
//!
//! agy reports a turn as numbered steps (`user_input`, `agent_response`,
//! `tool`, …), each sent as it starts (`ACTIVE`) and ends (`DONE`/`ERROR`).
//! Responses carry `text_delta`/`thinking_delta`; tools carry `tool_info`
//! with their parameters and output. The final `result` ends the turn.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// What one agy event means for the turn.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// `session/update` params to send.
    Updates(Vec<Value>),
    Done(TurnResult),
}

#[derive(Debug, Default, PartialEq)]
pub struct TurnResult {
    pub error: Option<String>,
    /// Actions agy refused for lack of permission, by display name.
    pub refused: Vec<String>,
    /// The last tool call that failed, `(id, title)`: what a permission
    /// prompt is about.
    pub failed_tool: Option<(String, String)>,
}

pub struct Translator {
    session_id: String,
    cwd: PathBuf,
    /// Tool steps already announced with a `tool_call`.
    announced: HashSet<u64>,
    failed_tool: Option<(String, String)>,
}

impl Translator {
    pub fn new(session_id: &str, cwd: &Path) -> Self {
        Self {
            session_id: session_id.to_string(),
            cwd: cwd.to_path_buf(),
            announced: HashSet::new(),
            failed_tool: None,
        }
    }

    pub fn event(&mut self, event: &Value) -> Step {
        match event.get("event").and_then(Value::as_str) {
            Some("step_update") => {
                let step = event.get("step_update").unwrap_or(&Value::Null);
                Step::Updates(
                    self.step(step)
                        .into_iter()
                        .map(|update| self.wrap(update))
                        .collect(),
                )
            }
            Some("result") => Step::Done(self.result(event.get("result").unwrap_or(&Value::Null))),
            _ => Step::Updates(Vec::new()),
        }
    }

    fn wrap(&self, update: Value) -> Value {
        json!({ "sessionId": self.session_id, "update": update })
    }

    fn step(&mut self, step: &Value) -> Vec<Value> {
        let text = |key: &str| {
            step.get(key)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
        };
        match step.get("step_type").and_then(Value::as_str) {
            Some("agent_response") => {
                let mut updates = Vec::new();
                if let Some(thought) = text("thinking_delta") {
                    updates.push(chunk("agent_thought_chunk", thought));
                }
                if let Some(message) = text("text_delta") {
                    updates.push(chunk("agent_message_chunk", message));
                }
                updates
            }
            Some("tool") => self.tool(step),
            _ => Vec::new(),
        }
    }

    fn tool(&mut self, step: &Value) -> Vec<Value> {
        let index = step.get("step_index").and_then(Value::as_u64).unwrap_or(0);
        let id = format!("step-{index}");
        let info = step.get("tool_info").unwrap_or(&Value::Null);
        let name = step
            .get("tool_name")
            .or_else(|| info.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("tool");
        let parameters = info.get("parameters").cloned().unwrap_or(Value::Null);
        let status = match step.get("state").and_then(Value::as_str) {
            Some("DONE") => "completed",
            Some("ERROR") => "failed",
            _ => "in_progress",
        };
        let mut updates = Vec::new();
        if self.announced.insert(index) {
            let (title, path) = describe(name, &parameters, &self.cwd);
            if status == "failed" {
                self.failed_tool = Some((id.clone(), title.clone()));
            }
            let locations: Vec<Value> = path
                .into_iter()
                .map(|path| json!({ "path": path }))
                .collect();
            updates.push(json!({
                "sessionUpdate": "tool_call",
                "toolCallId": id,
                "title": title,
                "kind": kind(name),
                "status": "in_progress",
                "rawInput": parameters,
                "locations": locations,
            }));
        } else if status == "failed" {
            let (title, _) = describe(name, &parameters, &self.cwd);
            self.failed_tool = Some((id.clone(), title));
        }
        if status != "in_progress" {
            let output = match status {
                "failed" => info.pointer("/error/message"),
                _ => info.get("output"),
            };
            let content: Vec<Value> = output
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(|text| json!({ "type": "content", "content": { "type": "text", "text": text } }))
                .into_iter()
                .collect();
            updates.push(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": id,
                "status": status,
                "content": content,
            }));
        }
        updates
    }

    fn result(&mut self, result: &Value) -> TurnResult {
        let refused = result
            .get("denied_actions")
            .and_then(Value::as_array)
            .map(|actions| {
                actions
                    .iter()
                    .filter_map(|action| {
                        action
                            .get("display_name")
                            .or_else(|| action.get("action"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let error = match result.get("status").and_then(Value::as_str) {
            Some("ERROR") => Some(
                result
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("agy reported an error")
                    .to_string(),
            ),
            _ => None,
        };
        TurnResult {
            error,
            refused,
            failed_tool: self.failed_tool.take(),
        }
    }
}

fn chunk(kind: &str, text: &str) -> Value {
    json!({ "sessionUpdate": kind, "content": { "type": "text", "text": text } })
}

/// ACP tool kind from agy's tool name.
fn kind(name: &str) -> &'static str {
    let has = |words: &[&str]| words.iter().any(|word| name.contains(word));
    if has(&["read", "view", "list_dir", "get_dom"]) {
        "read"
    } else if has(&["write", "edit", "replace", "create"]) {
        "edit"
    } else if has(&["delete", "remove"]) {
        "delete"
    } else if has(&["move", "rename"]) {
        "move"
    } else if has(&["search", "grep", "find"]) {
        "search"
    } else if has(&["command", "shell", "terminal", "run"]) {
        "execute"
    } else if has(&["browser", "url", "fetch", "web"]) {
        "fetch"
    } else {
        "other"
    }
}

/// A title like "Read file notes.txt" and the file it touches, if any.
fn describe(name: &str, parameters: &Value, cwd: &Path) -> (String, Option<String>) {
    let mut title = name.replace('_', " ");
    if let Some(first) = title.get(..1) {
        title = first.to_uppercase() + &title[1..];
    }
    let find = |keys: &[&str]| {
        parameters.as_object().and_then(|object| {
            object.iter().find_map(|(key, value)| {
                let key = key.to_ascii_lowercase();
                keys.contains(&key.as_str())
                    .then(|| value.as_str())
                    .flatten()
            })
        })
    };
    let path = find(&[
        "path",
        "absolutepath",
        "targetfile",
        "file",
        "filepath",
        "directorypath",
    ]);
    if let Some(path) = path {
        let shown = Path::new(path)
            .strip_prefix(cwd)
            .map(|relative| relative.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string());
        return (format!("{title} {shown}"), Some(path.to_string()));
    }
    if let Some(subject) = find(&["commandline", "command", "query", "pattern", "url"]) {
        let short: String = subject.chars().take(80).collect();
        return (format!("{title} `{short}`"), None);
    }
    (title, None)
}

/// An ACP prompt as agy message content (text blocks; files inline).
pub fn prompt_content(prompt: &Value) -> Value {
    let blocks = prompt.as_array().map(Vec::as_slice).unwrap_or_default();
    let mut content = Vec::new();
    for block in blocks {
        let text = match block.get("type").and_then(Value::as_str) {
            Some("text") => block
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string),
            Some("resource_link") => block
                .get("uri")
                .and_then(Value::as_str)
                .map(|uri| format!("Attached file: {}", uri_path(uri))),
            Some("resource") => block.get("resource").and_then(|resource| {
                let uri = resource
                    .get("uri")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let text = resource.get("text").and_then(Value::as_str)?;
                Some(format!(
                    "Attached file {}:\n```\n{text}\n```",
                    uri_path(uri)
                ))
            }),
            _ => None,
        };
        if let Some(text) = text {
            content.push(json!({ "type": "text", "text": text }));
        }
    }
    Value::Array(content)
}

/// A `file://` URI as a local path (other URIs unchanged).
fn uri_path(uri: &str) -> String {
    let Some(rest) = uri.strip_prefix("file://") else {
        return uri.to_string();
    };
    // `file:///C:/x` on Windows.
    let rest = match rest.as_bytes() {
        [b'/', _, b':', ..] => &rest[1..],
        _ => rest,
    };
    percent_decode(rest)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol_schema::v1::SessionNotification;

    fn updates(step: Step) -> Vec<Value> {
        match step {
            Step::Updates(updates) => updates,
            Step::Done(_) => panic!("expected updates"),
        }
    }

    #[test]
    fn translates_a_turn() {
        let cwd = Path::new("/work");
        let mut translator = Translator::new("s1", cwd);
        let lines = [
            r#"{"event":"step_update","step_update":{"step_index":0,"state":"DONE","step_type":"user_input"}}"#,
            r#"{"event":"step_update","step_update":{"step_index":1,"state":"ACTIVE","step_type":"agent_response","thinking_delta":"hm","text_delta":"ok"}}"#,
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"read_file","tool_info":{"name":"read_file","parameters":{"Path":"/work/notes.txt"}}}}"#,
            r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"tool","tool_name":"read_file","tool_info":{"name":"read_file","parameters":{"Path":"/work/notes.txt"},"output":"1 lines"}}}"#,
            r#"{"event":"step_update","step_update":{"step_index":3,"state":"ERROR","step_type":"tool","tool_name":"write_file","tool_info":{"name":"write_file","parameters":{"Path":"/work/a.txt"},"error":{"message":"user denied permission"}}}}"#,
        ];
        let mut all = Vec::new();
        for line in lines {
            all.extend(updates(
                translator.event(&serde_json::from_str(line).unwrap()),
            ));
        }
        // Every update is a valid ACP notification.
        for update in &all {
            serde_json::from_value::<SessionNotification>(update.clone()).unwrap();
        }
        let kinds: Vec<_> = all
            .iter()
            .map(|update| update["update"]["sessionUpdate"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "agent_thought_chunk",
                "agent_message_chunk",
                "tool_call",
                "tool_call_update",
                "tool_call",
                "tool_call_update"
            ]
        );
        assert_eq!(all[2]["update"]["title"], "Read file notes.txt");
        assert_eq!(all[2]["update"]["kind"], "read");
        assert_eq!(all[5]["update"]["status"], "failed");

        let done = translator.event(&json!({"event":"result","result":{"status":"SUCCESS",
            "denied_actions":[{"action":"write_file","display_name":"WriteFile"}]}}));
        assert_eq!(
            done,
            Step::Done(TurnResult {
                error: None,
                refused: vec!["WriteFile".into()],
                failed_tool: Some(("step-3".into(), "Write file a.txt".into())),
            })
        );
    }

    #[test]
    fn prompts_become_text() {
        let prompt = json!([
            { "type": "text", "text": "Fix it" },
            { "type": "resource_link", "uri": "file:///C:/My%20Repo/main.rs", "name": "main.rs" },
            { "type": "resource", "resource": { "uri": "file:///w/a.txt", "text": "hi" } },
            { "type": "image", "data": "", "mimeType": "image/png" },
        ]);
        let content = prompt_content(&prompt);
        assert_eq!(content.as_array().unwrap().len(), 3);
        assert_eq!(content[1]["text"], "Attached file: C:/My Repo/main.rs");
        assert_eq!(content[2]["text"], "Attached file /w/a.txt:\n```\nhi\n```");
    }
}
