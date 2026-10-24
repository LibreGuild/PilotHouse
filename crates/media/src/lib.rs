//! Live pictures for the device and browser panes.
//!
//! - [`Frame`]: an RGBA picture, decoded from PNG/JPEG, convertible for GPUI.
//! - [`SurfaceView`]: shows the latest frame, maps input to frame pixels and
//!   draws annotations on top.
//! - [`annotate`]: shapes and burning them into a capture.
//! - [`Recorder`]: recordings, exported as GIF or a few keyframes.
//! - [`compare()`]: before/after visual diff.
//! - [`LiveTools`]: the floating toolbar that ties these together on a pane.
//!
//! Streaming pipeline: the source decodes each frame on a background thread,
//! the view converts it to BGRA once and releases the previous GPU texture on
//! the next render, so a long session does not grow the texture atlas.

pub mod annotate;
pub mod capture;
mod compare;
mod frame;
mod live_tools;
mod recorder;
mod surface;

pub use compare::{Comparison, compare};
pub use frame::Frame;
pub use live_tools::{CapturedImage, LiveTools, ToChat};
pub use recorder::{Recorder, Recording};
pub use surface::{
    PointerKind, SURFACE_CONTEXT, SurfaceEvent, SurfaceMode, SurfacePointer, SurfaceView,
};
