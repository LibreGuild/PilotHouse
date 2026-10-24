use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One call over the control socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// The answer to a [`Request`]: exactly one of `result` or `error` is set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn from_result(id: u64, result: anyhow::Result<Value>) -> Self {
        match result {
            Ok(value) => Self {
                id,
                result: Some(value),
                error: None,
            },
            Err(err) => Self {
                id,
                result: None,
                error: Some(format!("{err:#}")),
            },
        }
    }
}
