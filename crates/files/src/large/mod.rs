//! Files too big for the editor, shown read-only with a line index built in
//! the background: logs, dumps and data files of many gigabytes.

pub mod index;
mod view;

pub use view::{LargeFileView, format_size};
