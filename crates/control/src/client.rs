use std::io::{BufRead as _, BufReader, Read, Write};
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use crate::protocol::{Request, Response};
use crate::transport;

/// A blocking connection to the running app's control endpoint.
pub struct ControlClient {
    reader: BufReader<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    next_id: u64,
}

impl ControlClient {
    pub fn connect(endpoint: &Path) -> Result<Self> {
        let connection = transport::connect(endpoint)?;
        Ok(Self {
            reader: BufReader::new(connection.reader),
            writer: connection.writer,
            next_id: 1,
        })
    }

    /// Sends one request and waits for its response.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let request = Request {
            id: self.next_id,
            method: method.to_string(),
            params,
        };
        self.next_id += 1;

        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;

        let mut answer = String::new();
        if self.reader.read_line(&mut answer)? == 0 {
            bail!("PilotHouse closed the control connection");
        }
        let response: Response = serde_json::from_str(&answer)?;
        match (response.result, response.error) {
            (_, Some(error)) => Err(anyhow!(error)),
            (Some(result), None) => Ok(result),
            (None, None) => Ok(Value::Null),
        }
    }
}
