//! Slides: decks made of web pages, edited beside a chat.
//!
//! A deck is an artifact (`ph_artifacts`) of the kind `slides`: its folder
//! holds the slides as plain files (`deck.rs`), and its address serves the
//! app's own page for them (`page/`): the editor for the owner, a viewer
//! for the people it is shared with. An agent changes a deck by changing
//! its files with the tools it has for any file; the user changes it by
//! hand in the page, and by asking the agent in the chat beside it.
//!
//! - [`deck`]: a deck's files.
//! - `live`: a deck held open: every change as a step that can be undone,
//!   whoever made it.
//! - `kind`: what a deck's address answers.
//! - [`looks`]: the looks a new deck starts from, and the layouts of a new
//!   slide.
//! - `comments`: what is said about a slide.
//! - [`export`], `pptx`, `import`: a deck written out (PDF, a web page, a
//!   picture, PowerPoint), and a PowerPoint file turned into slides.
//! - `control`: `deck.*` for agents (the `deck_` tools); [`guide`]: what
//!   agents are told about decks.
//! - [`hub`]: the running app's decks ([`Decks`]).
//! - [`view`]: the deck in a window ([`DeckView`]): the system's web view
//!   where there is one.
//!
//! Behind the `presentations` feature (`brand/brand.toml`).

mod art;
mod comments;
mod control;
pub mod deck;
pub mod export;
pub mod guide;
mod hub;
mod import;
mod kind;
mod live;
pub mod looks;
mod pptx;
mod view;

pub use art::card as card_art;
pub use comments::Comment;
pub use control::register_control_methods;
pub use hub::{DeckEvent, Decks, init};
pub use import::is_powerpoint;
pub use kind::Slides;
pub use live::{Change, Live};
pub use view::DeckView;
