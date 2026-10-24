//! Where captures and recordings are kept: the data directory, never the
//! project folder, so screenshots do not end up in the repository.

use std::path::PathBuf;

use anyhow::Result;

use crate::frame::Frame;

pub fn captures_dir() -> PathBuf {
    ph_store::paths::data_dir().join("captures")
}

pub fn recordings_dir() -> PathBuf {
    ph_store::paths::data_dir().join("recordings")
}

/// Saves `frame` as a PNG named after `stem`; returns the path.
pub fn save_png(frame: &Frame, stem: &str) -> Result<PathBuf> {
    let dir = captures_dir();
    ph_store::paths::ensure_private_dir(&dir)?;
    let path = dir.join(format!("{stem}-{}.png", ph_store::now_millis()));
    std::fs::write(&path, frame.encode_png()?)?;
    Ok(path)
}
