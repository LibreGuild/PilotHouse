//! GitHub and GitHub Enterprise for PilotHouse.
//!
//! - [`accounts`]: one account per host (`github.com`, any Enterprise
//!   server), its token in the OS credential store ([`secrets`]).
//! - [`client`]: a blocking REST client over the OS's TLS (works with
//!   company CAs installed on the machine).
//! - [`pulls`]: pull requests, files, diffs, conversation, line comments,
//!   reviews, checks; commenting, reviewing, merging, creating.
//! - [`search`]: pull requests across repositories (review requested, the
//!   user's own, assigned).
//!
//! Every call blocks on the network: use a background thread. Requests go
//! only to the hosts the user added.

pub mod accounts;
pub mod client;
pub mod models;
pub mod pulls;
pub mod search;
pub mod secrets;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use accounts::Account;
pub use client::Client;
pub use pulls::Repo;
