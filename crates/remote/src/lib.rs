//! Network drives whose work runs on the server.
//!
//! A [`Mapping`] pairs a folder on this computer that is a network share (a
//! mapped drive `Z:\`, a share `\\server\share`, a mount `/Volumes/share`)
//! with the same folder on a server reached over SSH. Commands that would run
//! in a folder under a mapping (Git, shells, run configurations) are started
//! on the server instead, in the matching folder, through the system's
//! `ssh`. Files stay where they are and are read and saved through the drive;
//! the work happens next to them, on the server's system.
//!
//! - [`mapping`]: the mappings, and turning paths from one side to the other.
//! - [`ssh`]: `ssh` command lines (options, quoting) and [`route`].
//! - [`check`]: what Settings checks: the host key, signing in, and that both
//!   sides really are one folder.
//! - [`drives`]: whether a folder is on a network drive, and on which.
//!
//! Signing in uses the user's own SSH setup (keys, agent, `~/.ssh/config`).
//! Commands never prompt (`BatchMode`); shells do, in the terminal.

pub mod check;
pub mod drives;
pub mod mapping;
pub mod ssh;

pub use mapping::{Mapping, Remote, mappings, remote_of, set_mappings, to_local};
pub use ssh::{Connection, close_shared, configure, explain_failure, route};
