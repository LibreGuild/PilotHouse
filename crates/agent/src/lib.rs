//! Agent chat for PilotHouse.
//!
//! PilotHouse drives the coding agents the user already has (Claude Code,
//! Codex, Gemini CLI, Antigravity) over the [Agent Client Protocol](https://agentclientprotocol.com):
//! each agent runs as a child process speaking JSON-RPC on stdio.
//!
//! - [`rpc`]: the process and JSON-RPC plumbing.
//! - [`AgentConnection`]: one agent process per project; the ACP handshake and
//!   message routing.
//! - [`ChatThread`]: one session's transcript and prompt turns.
//! - [`chats`]: each workspace's agents and chats, kept outside any window.
//! - [`ChatPanel`]: the chat column UI.
//! - [`ApprovalInbox`]: permission requests from every agent, in one place.
//!
//! Every new agent session is given the PilotHouse MCP server, so agents can
//! read and update the project board.

pub mod agents;
pub mod approvals;
mod attachment_view;
pub mod attachments;
mod chat_panel;
pub mod chats;
mod connection;
mod entry_view;
pub mod instructions;
pub mod mcp_config;
mod mention_complete;
mod model_picker;
pub mod oneshot;
pub mod options;
mod permission;
pub mod review_card;
mod review_policy;
pub mod rpc;
pub mod setup;
pub mod setup_view;
pub mod skills;
mod thread;
pub mod tools_view;
mod transcript;
pub mod usage_meter;

pub use attachments::{Attachment, LiveSources, PullBrief};
pub use chat_panel::{ChatPanel, ChatPanelEvent};
pub use chats::{ChatHub, ChatsActivity, WorkspaceChats, WorkspaceChatsEvent};
pub use connection::AgentConnection;
pub use permission::{ApprovalInbox, PermissionRequest};
pub use thread::{ChatThread, ThreadStatus};
