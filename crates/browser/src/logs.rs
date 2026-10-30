//! Console and network records collected from the page.
//!
//! Each record gets a number in the order it arrived, so an agent can ask
//! for what is new since it last looked. Request headers that carry
//! credentials are kept only as `••••`.

use std::collections::VecDeque;
use std::time::Instant;

use serde_json::Value;

/// How many console lines and requests are kept per page.
const KEEP: usize = 1000;
/// Most of a request body kept (form posts, JSON sent).
const MAX_POST_DATA: usize = 8 * 1024;

/// Headers whose values are never shown.
const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-auth-token",
    "x-csrf-token",
    "x-xsrf-token",
];

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
    /// Its place among the page's records.
    pub seq: u64,
    /// Where it was logged or thrown (`https://x/app.js:12:5`).
    pub source: Option<String>,
    /// An uncaught error's stack, a frame a line.
    pub stack: Option<String>,
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
    pub seq: u64,
    pub status_text: String,
    pub mime: String,
    /// Credentials replaced by `••••`.
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    /// What was sent (form fields, JSON), cut at 8 KB.
    pub post_data: Option<String>,
    /// Bytes received over the network, once finished.
    pub size: Option<u64>,
    pub started: Instant,
    pub duration_ms: Option<u64>,
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
    /// The number the next record gets.
    next_seq: u64,
}

impl Logs {
    pub fn clear(&mut self) {
        self.console.clear();
        self.network.clear();
    }

    /// The number the next record will get: records from here on are new.
    pub fn mark(&self) -> u64 {
        self.next_seq
    }

    fn take_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// Marks a move to another page in the console, so an agent reading
    /// it knows which page a message came from.
    pub fn navigated(&mut self, url: &str) {
        self.push_console_at(Level::Info, format!("— Navigated to {url} —"), None, None);
    }

    fn push_console_at(
        &mut self,
        level: Level,
        text: String,
        source: Option<String>,
        stack: Option<String>,
    ) {
        if self.console.len() == KEEP {
            self.console.pop_front();
        }
        let seq = self.take_seq();
        self.console.push_back(ConsoleLine {
            level,
            text,
            seq,
            source,
            stack,
        });
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
                let frames = &params["stackTrace"]["callFrames"];
                self.push_console_at(
                    Level::parse(params["type"].as_str().unwrap_or("log")),
                    text,
                    frames.get(0).and_then(frame_place),
                    None,
                );
            }
            "Runtime.exceptionThrown" => {
                let details = &params["exceptionDetails"];
                let text = details["exception"]["description"]
                    .as_str()
                    .or_else(|| details["text"].as_str())
                    .unwrap_or("Uncaught exception");
                // The description holds the stack already; the frames are
                // kept apart for a message that has none.
                let (message, described) = match text.split_once("\n    at ") {
                    Some((message, _)) => (message.to_string(), true),
                    None => (text.to_string(), false),
                };
                let frames: Vec<String> = details["stackTrace"]["callFrames"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(12)
                    .filter_map(|frame| {
                        let place = frame_place(frame)?;
                        let name = frame["functionName"].as_str().unwrap_or_default();
                        Some(if name.is_empty() {
                            format!("at {place}")
                        } else {
                            format!("at {name} ({place})")
                        })
                    })
                    .collect();
                let stack = if !frames.is_empty() {
                    Some(frames.join("\n"))
                } else if described {
                    text.split_once('\n')
                        .map(|(_, rest)| rest.trim().to_string())
                } else {
                    None
                };
                let source = details["url"]
                    .as_str()
                    .filter(|url| !url.is_empty())
                    .map(|url| {
                        format!(
                            "{url}:{}:{}",
                            details["lineNumber"].as_u64().unwrap_or(0) + 1,
                            details["columnNumber"].as_u64().unwrap_or(0) + 1
                        )
                    });
                self.push_console_at(Level::Error, message, source, stack);
            }
            "Log.entryAdded" => {
                let entry = &params["entry"];
                let text = entry["text"].as_str().unwrap_or_default().to_string();
                let source = entry["url"]
                    .as_str()
                    .filter(|url| !url.is_empty())
                    .map(|url| match entry["lineNumber"].as_u64() {
                        Some(line) => format!("{url}:{}", line + 1),
                        None => url.to_string(),
                    });
                self.push_console_at(
                    Level::parse(entry["level"].as_str().unwrap_or("info")),
                    text,
                    source,
                    None,
                );
            }
            "Network.requestWillBeSent" => {
                let request = &params["request"];
                let url = request["url"].as_str().unwrap_or_default();
                if url.starts_with("data:") {
                    return false;
                }
                let id = params["requestId"].as_str().unwrap_or_default().to_string();
                // A redirect is the same request id again: the hop before
                // it stays, answered by the redirect.
                if let Some(redirect) = params.get("redirectResponse")
                    && let Some(earlier) = self.find(params)
                {
                    earlier.status = redirect["status"].as_u64().map(|status| status as u16);
                    earlier.status_text =
                        redirect["statusText"].as_str().unwrap_or_default().into();
                    earlier.id = format!("{id}:redirect");
                }
                if self.network.len() == KEEP {
                    self.network.pop_front();
                }
                let seq = self.take_seq();
                self.network.push_back(Request {
                    id,
                    method: request["method"].as_str().unwrap_or("GET").to_string(),
                    url: url.to_string(),
                    status: None,
                    failed: None,
                    kind: params["type"].as_str().unwrap_or_default().to_string(),
                    seq,
                    status_text: String::new(),
                    mime: String::new(),
                    request_headers: headers(&request["headers"]),
                    response_headers: Vec::new(),
                    post_data: request["postData"].as_str().map(|data| {
                        let mut data = redact_body(data);
                        if data.len() > MAX_POST_DATA {
                            let mut cut = MAX_POST_DATA;
                            while !data.is_char_boundary(cut) {
                                cut -= 1;
                            }
                            data.truncate(cut);
                            data.push('…');
                        }
                        data
                    }),
                    size: None,
                    started: Instant::now(),
                    duration_ms: None,
                });
            }
            "Network.responseReceived" => {
                let Some(request) = self.find(params) else {
                    return false;
                };
                let response = &params["response"];
                request.status = response["status"].as_u64().map(|status| status as u16);
                request.status_text = response["statusText"].as_str().unwrap_or_default().into();
                request.mime = response["mimeType"].as_str().unwrap_or_default().into();
                request.response_headers = headers(&response["headers"]);
            }
            "Network.loadingFinished" => {
                let Some(request) = self.find(params) else {
                    return false;
                };
                request.size = params["encodedDataLength"].as_f64().map(|size| size as u64);
                request.duration_ms = Some(request.started.elapsed().as_millis() as u64);
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
                request.duration_ms = Some(request.started.elapsed().as_millis() as u64);
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

/// `https://x/app.js:12:5` for a stack frame (lines and columns 1-based).
fn frame_place(frame: &Value) -> Option<String> {
    let url = frame["url"].as_str().filter(|url| !url.is_empty())?;
    Some(format!(
        "{url}:{}:{}",
        frame["lineNumber"].as_u64().unwrap_or(0) + 1,
        frame["columnNumber"].as_u64().unwrap_or(0) + 1
    ))
}

/// Headers as name and value, credentials hidden, sorted by name.
/// Whether a header carries a credential: the known ones, and any whose
/// name says so.
fn secret_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_HEADERS.contains(&name.as_str())
        || [
            "token", "secret", "api-key", "apikey", "password", "session",
        ]
        .iter()
        .any(|word| name.contains(word))
}

/// Whether a form or JSON field holds a secret, by its name.
fn secret_field(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "pass",
        "pwd",
        "secret",
        "token",
        "otp",
        "cvv",
        "cvc",
        "apikey",
        "api_key",
        "api-key",
        "credential",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// A request or response body with its secrets hidden: the values of JSON
/// and form fields named like passwords, tokens and keys. A body of
/// another shape that names one (a multipart form) is not shown at all.
/// The user signs in to sites in the agents' tabs; what they type must not
/// reach the agent through the logs.
pub fn redact_body(text: &str) -> String {
    const HIDDEN: &str = "••••";
    fn redact_json(value: &mut Value) -> bool {
        match value {
            Value::Object(fields) => {
                let mut changed = false;
                for (name, value) in fields.iter_mut() {
                    if secret_field(name) && !value.is_null() {
                        *value = Value::String(HIDDEN.into());
                        changed = true;
                    } else {
                        changed |= redact_json(value);
                    }
                }
                changed
            }
            Value::Array(items) => items
                .iter_mut()
                .fold(false, |changed, item| redact_json(item) | changed),
            _ => false,
        }
    }
    let trimmed = text.trim_start();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(mut value) = serde_json::from_str::<Value>(text)
    {
        return if redact_json(&mut value) {
            value.to_string()
        } else {
            text.to_string()
        };
    }
    // A form: name=value&…
    if text.contains('=') && !text.contains(char::is_whitespace) {
        return text
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((name, _)) if secret_field(name) => format!("{name}={HIDDEN}"),
                _ => pair.to_string(),
            })
            .collect::<Vec<_>>()
            .join("&");
    }
    let lower = text.to_ascii_lowercase();
    if ["password", "passwd", "secret", "token"]
        .iter()
        .any(|word| lower.contains(word))
    {
        return "(not shown: it may hold a password or token)".into();
    }
    text.to_string()
}

fn headers(object: &Value) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = object
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, value)| {
            let value = if secret_header(name) {
                "••••".to_string()
            } else {
                value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string())
            };
            (name.clone(), value)
        })
        .collect();
    headers.sort_by_key(|(name, _)| name.to_ascii_lowercase());
    headers
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

    #[test]
    fn keeps_where_errors_come_from_and_hides_credentials() {
        let mut logs = Logs::default();
        let mark = logs.mark();
        logs.record(
            "Runtime.exceptionThrown",
            &json!({ "exceptionDetails": {
                "text": "Uncaught",
                "url": "https://x/app.js", "lineNumber": 9, "columnNumber": 4,
                "exception": { "description": "TypeError: x is undefined\n    at render (https://x/app.js:10:5)" },
                "stackTrace": { "callFrames": [{ "functionName": "render", "url": "https://x/app.js", "lineNumber": 9, "columnNumber": 4 }] }
            } }),
        );
        let line = logs.console[0].clone();
        assert_eq!(line.text, "TypeError: x is undefined");
        assert_eq!(line.source.as_deref(), Some("https://x/app.js:10:5"));
        assert_eq!(
            line.stack.as_deref(),
            Some("at render (https://x/app.js:10:5)")
        );
        assert!(line.seq >= mark);
        logs.record(
            "Network.requestWillBeSent",
            &json!({ "requestId": "7", "type": "XHR", "request": {
                "method": "POST", "url": "https://x/api", "postData": "{\"a\":1}",
                "headers": { "Authorization": "Bearer secret", "Accept": "*/*" }
            } }),
        );
        let request = &logs.network[0];
        assert_eq!(
            request.request_headers,
            [
                ("Accept".to_string(), "*/*".to_string()),
                ("Authorization".to_string(), "••••".to_string())
            ]
        );
        assert_eq!(request.post_data.as_deref(), Some("{\"a\":1}"));
        // What a sign-in sends is hidden; the rest stays.
        assert_eq!(
            redact_body("user=sam&password=hunter2&remember=1"),
            "user=sam&password=••••&remember=1"
        );
        assert_eq!(
            redact_body(r#"{"user":"sam","auth":{"access_token":"abc"},"list":[{"pwd":"x"}]}"#),
            r#"{"user":"sam","auth":{"access_token":"••••"},"list":[{"pwd":"••••"}]}"#
        );
        assert_eq!(redact_body("{\"a\":1}"), "{\"a\":1}");
        assert!(redact_body("--b\r\nname=\"password\"\r\n\r\nx").starts_with("(not shown"));
        assert!(secret_header("X-Session-Token") && !secret_header("Content-Type"));
        assert!(request.seq > line.seq);
    }
}
