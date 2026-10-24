//! Console and network records collected from the page.

use std::collections::VecDeque;

use serde_json::Value;

/// How many console lines and requests are kept per browser.
const KEEP: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warning,
    Error,
}

impl Level {
    fn parse(text: &str) -> Self {
        match text {
            "error" | "assert" => Level::Error,
            "warning" | "warn" => Level::Warning,
            "debug" | "verbose" | "trace" => Level::Debug,
            _ => Level::Info,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warning => "warn",
            Level::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConsoleLine {
    pub level: Level,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub id: String,
    pub method: String,
    pub url: String,
    /// `None` while pending.
    pub status: Option<u16>,
    pub failed: Option<String>,
    pub kind: String,
}

impl Request {
    pub fn is_error(&self) -> bool {
        self.failed.is_some() || self.status.is_some_and(|status| status >= 400)
    }

    pub fn summary(&self) -> String {
        let status = match (&self.failed, self.status) {
            (Some(error), _) => error.clone(),
            (None, Some(status)) => status.to_string(),
            (None, None) => "pending".into(),
        };
        format!("{} {} → {}", self.method, self.url, status)
    }
}

#[derive(Default)]
pub struct Logs {
    pub console: VecDeque<ConsoleLine>,
    pub network: VecDeque<Request>,
}

impl Logs {
    pub fn clear(&mut self) {
        self.console.clear();
        self.network.clear();
    }

    fn push_console(&mut self, level: Level, text: String) {
        if self.console.len() == KEEP {
            self.console.pop_front();
        }
        self.console.push_back(ConsoleLine { level, text });
    }

    /// Records a DevTools event; returns whether anything changed.
    pub fn record(&mut self, method: &str, params: &Value) -> bool {
        match method {
            "Runtime.consoleAPICalled" => {
                let text = params["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(remote_object_text)
                    .collect::<Vec<_>>()
                    .join(" ");
                self.push_console(Level::parse(params["type"].as_str().unwrap_or("log")), text);
            }
            "Runtime.exceptionThrown" => {
                let details = &params["exceptionDetails"];
                let text = details["exception"]["description"]
                    .as_str()
                    .or_else(|| details["text"].as_str())
                    .unwrap_or("Uncaught exception");
                self.push_console(Level::Error, text.to_string());
            }
            "Log.entryAdded" => {
                let entry = &params["entry"];
                let mut text = entry["text"].as_str().unwrap_or_default().to_string();
                if let Some(url) = entry["url"].as_str().filter(|url| !url.is_empty()) {
                    text.push_str(&format!(" ({url})"));
                }
                self.push_console(
                    Level::parse(entry["level"].as_str().unwrap_or("info")),
                    text,
                );
            }
            "Network.requestWillBeSent" => {
                let request = &params["request"];
                let url = request["url"].as_str().unwrap_or_default();
                if url.starts_with("data:") {
                    return false;
                }
                if self.network.len() == KEEP {
                    self.network.pop_front();
                }
                self.network.push_back(Request {
                    id: params["requestId"].as_str().unwrap_or_default().to_string(),
                    method: request["method"].as_str().unwrap_or("GET").to_string(),
                    url: url.to_string(),
                    status: None,
                    failed: None,
                    kind: params["type"].as_str().unwrap_or_default().to_string(),
                });
            }
            "Network.responseReceived" => {
                let Some(request) = self.find(params) else {
                    return false;
                };
                request.status = params["response"]["status"]
                    .as_u64()
                    .map(|status| status as u16);
            }
            "Network.loadingFailed" => {
                let Some(request) = self.find(params) else {
                    return false;
                };
                let canceled = params["canceled"].as_bool().unwrap_or(false);
                request.failed = Some(if canceled {
                    "canceled".into()
                } else {
                    params["errorText"].as_str().unwrap_or("failed").to_string()
                });
            }
            _ => return false,
        }
        true
    }

    fn find(&mut self, params: &Value) -> Option<&mut Request> {
        let id = params["requestId"].as_str()?;
        self.network
            .iter_mut()
            .rev()
            .find(|request| request.id == id)
    }

    /// The last `limit` console lines as text, oldest first.
    pub fn console_text(&self, limit: usize, errors_only: bool) -> String {
        let lines: Vec<String> = self
            .console
            .iter()
            .filter(|line| !errors_only || line.level == Level::Error)
            .rev()
            .take(limit)
            .map(|line| format!("[{}] {}", line.level.label(), line.text))
            .collect();
        lines.into_iter().rev().collect::<Vec<_>>().join("\n")
    }

    /// The last `limit` requests as text, oldest first.
    pub fn network_text(&self, limit: usize, errors_only: bool) -> String {
        let lines: Vec<String> = self
            .network
            .iter()
            .filter(|request| !errors_only || request.is_error())
            .rev()
            .take(limit)
            .map(Request::summary)
            .collect();
        lines.into_iter().rev().collect::<Vec<_>>().join("\n")
    }
}

/// A readable form of a console argument.
fn remote_object_text(object: &Value) -> String {
    match object.get("value") {
        Some(Value::String(text)) => text.clone(),
        Some(value) => value.to_string(),
        None => object["description"]
            .as_str()
            .or_else(|| object["unserializableValue"].as_str())
            .unwrap_or_else(|| object["type"].as_str().unwrap_or("?"))
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_console_and_requests() {
        let mut logs = Logs::default();
        logs.record(
            "Runtime.consoleAPICalled",
            &json!({ "type": "error", "args": [{ "type": "string", "value": "boom" }, { "type": "number", "value": 3 }] }),
        );
        logs.record(
            "Network.requestWillBeSent",
            &json!({ "requestId": "1", "request": { "method": "GET", "url": "http://x/api" }, "type": "Fetch" }),
        );
        logs.record(
            "Network.responseReceived",
            &json!({ "requestId": "1", "response": { "status": 500 } }),
        );
        assert_eq!(logs.console_text(10, true), "[error] boom 3");
        assert_eq!(logs.network_text(10, true), "GET http://x/api → 500");
    }
}
