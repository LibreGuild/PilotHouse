//! The PilotHouse MCP server.
//!
//! Coding agents start it as a stdio MCP server (`pilothouse mcp`). It speaks
//! the small subset of MCP a tool server needs (`initialize`, `tools/list`,
//! `tools/call`, `ping`) as newline-delimited JSON-RPC, and turns each tool
//! call into a request on the running app's control socket. The app owns all
//! state; this process holds none, so it is safe to start one per agent.
//!
//! Implemented by hand rather than with an SDK: the protocol surface is tiny
//! and this keeps the binary free of an async runtime.

mod protocol;
mod tools;

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use ph_control::ControlClient;
use serde_json::{Value, json};

/// How the bridge finds the project and who it acts as.
#[derive(Debug, Clone)]
pub struct BridgeOptions {
    pub socket_path: PathBuf,
    /// The project folder; agents usually run in it, so it defaults to the cwd.
    pub project_path: PathBuf,
    /// Shown as the author of card notes and as the card assignee.
    pub agent_name: String,
}

/// Serves MCP on stdin/stdout until stdin closes.
pub fn run_stdio(options: BridgeOptions) -> Result<()> {
    let mut session = Session::new(options);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.context("reading stdin")?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = session.handle_line(&line) {
            let mut text = serde_json::to_string(&response)?;
            text.push('\n');
            stdout.write_all(text.as_bytes())?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// One MCP connection: lazily connects to the app and remembers the project.
pub struct Session {
    options: BridgeOptions,
    client: Option<ControlClient>,
    project_id: Option<i64>,
}

impl Session {
    pub fn new(options: BridgeOptions) -> Self {
        Self {
            options,
            client: None,
            project_id: None,
        }
    }

    /// Handles one JSON-RPC message; returns the response, if one is due.
    pub fn handle_line(&mut self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(err) => {
                return Some(protocol::error(
                    Value::Null,
                    protocol::PARSE_ERROR,
                    &err.to_string(),
                ));
            }
        };
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        // Notifications (no id) never get a response.
        let id = id?;
        let result = match method {
            "initialize" => protocol::initialize_result(&params),
            "ping" => json!({}),
            "tools/list" => json!({ "tools": tools::definitions() }),
            "tools/call" => self.call_tool(&params),
            _ => {
                return Some(protocol::error(
                    id,
                    protocol::METHOD_NOT_FOUND,
                    &format!("unknown method `{method}`"),
                ));
            }
        };
        Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    /// Tool failures are reported inside the result (`isError`), as MCP asks,
    /// so the agent sees the message and can correct itself.
    fn call_tool(&mut self, params: &Value) -> Value {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match self.run_tool(name, arguments) {
            Ok((text, images)) => {
                let mut content = vec![json!({ "type": "text", "text": text })];
                content.extend(images);
                json!({ "content": content })
            }
            Err(err) => {
                json!({ "content": [{ "type": "text", "text": format!("{err:#}") }], "isError": true })
            }
        }
    }

    /// Runs a tool; returns its text and any images (screenshots) the app
    /// attached as `images: [{data, mimeType}]`.
    fn run_tool(&mut self, name: &str, arguments: Value) -> Result<(String, Vec<Value>)> {
        let tool = tools::find(name).with_context(|| format!("unknown tool `{name}`"))?;
        let project_id = self.project_id()?;
        let agent = self.options.agent_name.clone();
        let (method, params) = (tool.build)(arguments, project_id, &agent)?;
        let result = self.client()?.call(method, params)?;
        let images = result
            .get("images")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|image| json!({ "type": "image", "data": image["data"], "mimeType": image["mimeType"] }))
            .collect();
        Ok(((tool.format)(&result), images))
    }

    fn client(&mut self) -> Result<&mut ControlClient> {
        if self.client.is_none() {
            self.client = Some(
                ControlClient::connect(&self.options.socket_path)
                    .context("PilotHouse must be running for its tools to work")?,
            );
        }
        Ok(self.client.as_mut().expect("connected above"))
    }

    fn project_id(&mut self) -> Result<i64> {
        if let Some(id) = self.project_id {
            return Ok(id);
        }
        let path = self.options.project_path.to_string_lossy().to_string();
        let result = self
            .client()?
            .call("project.resolve", json!({ "path": path }))?;
        let id = result.get("id").and_then(Value::as_i64).with_context(|| {
            format!("{path} is not in a PilotHouse workspace; open it in PilotHouse first")
        })?;
        self.project_id = Some(id);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(BridgeOptions {
            socket_path: PathBuf::from("/nonexistent/pilothouse.sock"),
            project_path: PathBuf::from("/tmp"),
            agent_name: "test".into(),
        })
    }

    #[test]
    fn initialize_and_list_tools_work_without_the_app() {
        let mut session = session();
        let init = session
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#)
            .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert!(
            session
                .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );

        let list = session
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"board_next_card"));
    }

    #[test]
    fn tool_errors_are_reported_in_the_result() {
        let mut session = session();
        let call = session
            .handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"board_list","arguments":{}}}"#)
            .unwrap();
        assert_eq!(call["result"]["isError"], true);
        let unknown = session
            .handle_line(r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#)
            .unwrap();
        assert_eq!(unknown["error"]["code"], protocol::METHOD_NOT_FOUND);
    }
}
