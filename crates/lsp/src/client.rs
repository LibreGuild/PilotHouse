//! One running language server: the `initialize` handshake and typed
//! requests and notifications.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};

use crate::servers::ServerSpec;
use crate::transport::Transport;

pub struct Server {
    pub transport: Arc<Transport>,
}

impl Server {
    /// Starts `program` for the project at `root` and initializes it.
    pub async fn start(spec: &'static ServerSpec, program: &Path, root: &Path) -> Result<Self> {
        let transport = Transport::spawn(program, spec.args, root)?;
        let root_uri = crate::uri::from_path(root).map(|uri| uri.as_str().to_string());
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "PilotHouse", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": root_uri,
            "workspaceFolders": root_uri.as_ref().map(|uri| json!([{ "uri": uri, "name": name }])),
            "capabilities": {
                "workspace": {
                    "applyEdit": false,
                    "workspaceFolders": true,
                    "configuration": true,
                    "symbol": {},
                },
                "textDocument": {
                    "synchronization": { "didSave": true, "dynamicRegistration": false },
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "completion": {
                        "contextSupport": true,
                        "completionItem": { "snippetSupport": false, "documentationFormat": ["markdown", "plaintext"] },
                    },
                    "definition": { "linkSupport": true },
                    "references": {},
                    "rename": { "prepareSupport": false },
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    // Unused code (1) is shown faded, deprecated (2) struck through.
                    "publishDiagnostics": { "relatedInformation": true, "tagSupport": { "valueSet": [1, 2] } },
                },
                "window": { "workDoneProgress": false },
            },
        });
        transport.request("initialize", params).await?;
        transport.notify("initialized", json!({}))?;
        // Some servers (Pyright) wait for settings before analyzing.
        transport.notify(
            "workspace/didChangeConfiguration",
            json!({ "settings": {} }),
        )?;
        Ok(Self { transport })
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.transport.notify(method, params)
    }

    /// Stops the server politely, then for sure.
    pub fn shutdown(&self) {
        let _ = self.transport.notify("exit", Value::Null);
        self.transport.kill();
    }
}
