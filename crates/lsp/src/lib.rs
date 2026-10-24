//! Language servers for PilotHouse's editor.
//!
//! - [`servers`]: which server serves which files (used when installed).
//! - [`transport`], [`client`]: LSP over stdio.
//! - [`LspManager`]: a project's servers, started on demand; open
//!   documents, requests, diagnostics.
//! - [`Binding`]: the editor's hover, go to definition and completion hooks.
//!
//! Everything is local: servers are the user's installs, run as child
//! processes in the project folder.

mod client;
mod manager;
pub mod position;
mod providers;
pub mod servers;
mod transport;
pub mod uri;

pub use manager::{LspEvent, LspManager, ServerStatus};
pub use providers::Binding;
