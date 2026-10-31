//! Studio: a window of its own for editing a media file, by hand or by
//! asking an agent: pictures, videos and slides.
//!
//! An edit never touches the file until it is saved: [`ImageEdit`] says
//! what to do to the original (turn, crop, adjust colors, mark, resize,
//! frame), [`render`] makes the result from it, the same way for the
//! preview and the saved file, and [`encode`] writes it. An
//! [`ImageDocument`] holds one picture's edit, its history and preview;
//! [`ImageEditor`] is the window's content. Agents work through the
//! `studio.*` control methods ([`register_control_methods`]).

mod control;
mod document;
pub mod edit;
mod editor;
pub mod encode;
mod history;
mod icons;
pub mod render;
pub mod slides;
pub mod video;

pub use control::register_control_methods;
pub use document::{Estimate, ImageDocument, View};
pub use edit::ImageEdit;
pub use editor::{ImageEditor, bind_keys};
pub use icons::write_icon_set;

/// Whether Studio edits files of this extension.
pub fn edits(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| encode::Format::from_extension(&extension.to_string_lossy()))
        .is_some()
}
