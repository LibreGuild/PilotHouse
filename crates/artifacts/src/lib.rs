//! Artifacts: web pages agents make for the user (a planner, a dashboard, a
//! small game), kept in a folder of the user's and served from this
//! computer, so each has an address to open in a browser, today and later.
//!
//! - [`library`]: the artifacts on disk, and publishing into them.
//! - [`server`]: the address each is served at, on the loopback interface.
//! - [`storage`]: what a page stores, kept with its artifact.
//! - [`sharing`]: the link that opens one to people on the same network.
//! - `room`: who has a page open, and what they say to each other live.
//! - [`hub`]: the running app's artifacts ([`Artifacts`]).
//! - [`control`]: `artifacts.*` for agents (the `artifact_` tools).
//! - [`guide`]: what agents are told about making one.
//! - [`view`]: the list (the *Artifacts* place of the window).
//!
//! Behind the `artifacts` feature (`brand/brand.toml`).

mod control;
pub mod guide;
mod http;
mod hub;
pub mod library;
mod room;
mod server;
mod share_view;
pub mod sharing;
pub mod storage;
mod view;

pub use control::register_control_methods;
pub use http::{Body, Request, Response, content_type};
pub use hub::{Artifacts, Host, NewKind, Sharing, folders_of, init, set_host, start, workspace_of};
pub use library::Artifact;
pub use server::{Kind, Server, Visit};
pub use share_view::ShareView;
pub use view::ArtifactsView;
