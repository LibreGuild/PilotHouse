//! Whiteboards: an endless board of notes, shapes, arrows and drawings,
//! changed by hand and by an agent in the chat beside it.
//!
//! A board is an artifact (`ph_artifacts`) of the kind `whiteboard`: its
//! folder holds one file, `site/board.json`, that lists what is on the
//! board, and its address serves the app's own page for it (`page/`). An
//! agent changes a board by changing that file with the tools it has for
//! any file; the user changes it by hand in the page, and by asking the
//! agent.
//!
//! - [`board`]: a board's file.
//! - `kind`: what a board's address answers.
//! - [`hub`]: the running app's boards ([`Boards`]).
//! - `control`: `board.*` for agents (the `board_` tools); [`guide`]: what
//!   agents are told about boards.
//! - `art`: the picture on the card that offers a new one.
//!
//! Behind the `whiteboards` feature (`brand/brand.toml`).

mod art;
pub mod board;
mod control;
pub mod guide;
mod hub;
mod kind;

pub use art::card as card_art;
pub use control::register_control_methods;
pub use hub::{BoardEvent, Boards, init};
