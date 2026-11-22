//! GitHub and GitHub Enterprise for PilotHouse.
//!
//! - [`accounts`]: one account per host (`github.com`, any Enterprise
//!   server), its token in the OS credential store ([`secrets`]).
//! - [`client`]: a blocking REST client over the OS's TLS (works with
//!   company CAs installed on the machine).
//! - [`pulls`]: pull requests, files, diffs, conversation, line comments,
//!   reviews, checks; commenting, reviewing, merging, creating.
//! - [`search`]: pull requests across repositories (needing the user's
//!   review, waiting for their author, the user's own).
//! - [`issues`]: a repository's issues, and opening one.
//!
//! Every call blocks on the network: use a background thread. Requests go
//! only to the hosts the user added.

pub mod accounts;
pub mod client;
pub mod issues;
pub mod models;
pub mod pulls;
pub mod search;
// Where tokens are kept: the store's, under this crate's old name for it.
pub use ph_store::secrets;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use accounts::Account;
pub use client::Client;
pub use pulls::Repo;
