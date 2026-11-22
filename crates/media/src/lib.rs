//! Live pictures for the device and browser panes.
//!
//! - [`Frame`]: an RGBA picture, decoded from PNG/JPEG, convertible for GPUI.
//! - [`SurfaceView`]: shows the latest frame, maps input to frame pixels and
//!   draws annotations on top.
//! - [`annotate`]: shapes and burning them into a capture.
//! - [`Recorder`]: recordings, exported as GIF or a few keyframes.
//! - [`LiveTools`]: the tools that tie these together in a pane's toolbar.
//!
//! Streaming pipeline: the source decodes each frame on a background thread,
//! the view converts it to BGRA once and releases the previous GPU texture on
//! the next render, so a long session does not grow the texture atlas.

pub mod annotate;
pub mod capture;
mod frame;
mod live_tools;
mod recorder;
mod surface;

pub use frame::Frame;
pub use live_tools::{CapturedImage, LiveTools, ToChat};
pub use recorder::{Recorder, Recording};
pub use surface::{
    PointerKind, SURFACE_CONTEXT, SurfaceEvent, SurfaceMode, SurfacePointer, SurfaceView,
    editing_shortcut,
};
