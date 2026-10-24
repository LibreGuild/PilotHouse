//! The local control socket.
//!
//! The running app listens on a local endpoint (a Unix socket, or a named
//! pipe on Windows; see [`transport`]). Other local processes send one JSON
//! request per line and get one JSON response per line:
//!
//! ```text
//! → {"id":1,"method":"board.list","params":{"project_id":3}}
//! ← {"id":1,"result":[...]}
//! ```
//!
//! Two clients use it:
//!
//! - the **MCP bridge** (`pilothouse mcp`), which coding agents launch so they
//!   can read and update the board;
//! - the **automation harness** (`pilothouse ctl`, `automation` feature only),
//!   which drives the UI for end-to-end checks without touching the real mouse.
//!
//! Only the same user account can connect, and nothing listens on the
//! network.
//!
//! Handlers run on the UI thread with full `App` access; register them with
//! [`ControlServer::handle`] before calling [`ControlServer::start`].

#[cfg(feature = "automation")]
pub mod automation;
mod client;
mod protocol;
mod server;
pub mod transport;

pub use client::ControlClient;
pub use protocol::{Request, Response};
pub use server::ControlServer;
