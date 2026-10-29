//! No H.264 decoder here: the screen streams raw frames instead.

use anyhow::{Result, bail};
use ph_media::Frame;

pub struct Decoder;

impl Decoder {
    pub fn new() -> Result<Self> {
        bail!("no H.264 decoder on this system")
    }

    pub fn decode(&mut self, _: &[Vec<u8>]) -> Result<Option<Frame>> {
        Ok(None)
    }
}
