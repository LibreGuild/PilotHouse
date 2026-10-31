//! Video in Studio: the edit (kept parts, speed, crop, size, marks over
//! time, sound), reading and writing through the system's frameworks
//! (AVFoundation, Media Foundation, GStreamer) or an installed ffmpeg, and
//! making each frame of the result.

#[cfg(target_os = "macos")]
mod av;
pub mod compose;
pub mod document;
pub mod edit;
mod editor;
pub mod export;
mod ffmpeg;
#[cfg(any(windows, target_os = "linux", test))]
mod frames;
#[cfg(target_os = "linux")]
mod gst;
pub mod media;
#[cfg(windows)]
mod mf;
mod sound;
#[cfg(any(windows, test))]
mod yuv;

pub use edit::{Overlay, Segment, VideoEdit, VideoExport, VideoFormat};
pub use editor::VideoEditor;
pub use media::Info;
