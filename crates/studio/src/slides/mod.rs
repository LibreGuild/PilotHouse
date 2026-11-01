//! Slides in Studio: presentations made by hand or by an agent, in a
//! model of PowerPoint's own kind, drawn by PilotHouse and exported as
//! editable PPTX, PDF or pictures.
//!
//! - [`model`]: the deck (slides, elements, theme, layouts) as JSON.
//! - [`ops`]: every change, for the editor and for agents.
//! - [`theme`]: the built-in themes and the layouts each has.
//! - [`markdown`]: light Markdown to slide text.

pub mod charts;
pub mod code;
pub mod connectors;
pub mod control;
pub mod document;
pub mod editor;
pub mod fonts;
pub mod icons;
pub mod markdown;
pub mod model;
pub mod ops;
pub mod pdf;
pub mod pptx;
pub mod pptx_read;
pub mod raster;
pub mod shapes;
pub mod space;
pub mod svg;
pub mod text;
pub mod theme;
pub mod xlsx;

pub use document::DeckDocument;
pub use editor::{SlidesEditor, SlidesEditorEvent};
pub use model::Deck;
pub use ops::{Op, apply};
