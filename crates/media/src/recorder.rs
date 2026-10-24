//! Screen recordings of a live pane, written to a GIF as they happen.
//!
//! Frames go to a worker thread that downscales them and appends them to the
//! GIF file, so memory stays flat however long the recording runs. The
//! worker keeps a small, evenly spaced sample of frames for the agent.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use image::Delay;
use image::codecs::gif::{GifEncoder, Repeat};

use crate::frame::Frame;

/// Longest side of recorded frames.
const MAX_SIDE: u32 = 720;
/// At most this many frames per second are kept.
const MAX_FPS: u32 = 8;
/// Recording stops taking frames after this long (keeps the GIF reasonable).
const MAX_DURATION: Duration = Duration::from_secs(10 * 60);
/// Frames kept for choosing keyframes at the end (at most twice this).
const SAMPLE: usize = 12;

/// What a finished recording produced.
pub struct Recording {
    pub gif: PathBuf,
    pub duration: Duration,
    /// Up to the requested number of frames, evenly spaced, first to last.
    pub keyframes: Vec<Frame>,
}

pub struct Recorder {
    started: Instant,
    last: Option<Instant>,
    frames: smol::channel::Sender<(Duration, Frame)>,
    worker: std::thread::JoinHandle<Result<Vec<Frame>>>,
    path: PathBuf,
}

impl Recorder {
    /// Starts a recording into a new GIF in `dir`.
    pub fn start(dir: &Path) -> Result<Self> {
        ph_store::paths::ensure_private_dir(dir)?;
        let path = dir.join(format!("recording-{}.gif", ph_store::now_millis()));
        let file = File::create(&path).with_context(|| format!("creating {}", path.display()))?;
        // A few frames of slack; if encoding falls behind, frames are skipped.
        let (frames, queue) = smol::channel::bounded::<(Duration, Frame)>(4);
        let worker = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || encode(file, queue))?;
        Ok(Self {
            started: Instant::now(),
            last: None,
            frames,
            worker,
            path,
        })
    }

    /// Offers a frame; it is dropped when it comes too soon after the last
    /// one, after the time limit, or while the encoder is busy.
    pub fn push(&mut self, frame: &Frame) {
        let now = Instant::now();
        let elapsed = now - self.started;
        let too_soon = self
            .last
            .is_some_and(|last| now - last < Duration::from_secs(1) / MAX_FPS);
        if elapsed > MAX_DURATION || too_soon {
            return;
        }
        if self.frames.try_send((elapsed, frame.clone())).is_ok() {
            self.last = Some(now);
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Finishes the GIF and picks `keyframes` frames. Blocks until the
    /// encoder is done, so call it off the UI thread.
    pub fn finish(self, keyframes: usize) -> Result<Recording> {
        let duration = self.started.elapsed();
        drop(self.frames);
        let sample = self
            .worker
            .join()
            .map_err(|_| anyhow!("the recorder crashed"))??;
        Ok(Recording {
            gif: self.path,
            duration,
            keyframes: spread(&sample, keyframes),
        })
    }
}

/// The worker: appends each frame to the GIF, giving the previous frame the
/// time until this one as its delay, and keeps an evenly spaced sample.
fn encode(file: File, queue: smol::channel::Receiver<(Duration, Frame)>) -> Result<Vec<Frame>> {
    let mut encoder = GifEncoder::new_with_speed(file, 20);
    encoder.set_repeat(Repeat::Infinite)?;
    let mut sample: Vec<Frame> = Vec::new();
    let mut stride = 1;
    let mut pending: Option<(Duration, Frame)> = None;
    let frames = std::iter::from_fn(|| queue.recv_blocking().ok());
    for (count, (at, frame)) in frames.enumerate() {
        let frame = frame.downscaled(MAX_SIDE);
        if count % stride == 0 {
            sample.push(frame.clone());
            // Too many: keep every other one and sample half as often.
            if sample.len() == SAMPLE * 2 {
                sample = sample.into_iter().step_by(2).collect();
                stride *= 2;
            }
        }
        if let Some((previous_at, previous)) = pending.replace((at, frame)) {
            write_frame(&mut encoder, &previous, at.saturating_sub(previous_at))?;
        }
    }
    if let Some((_, last)) = &pending {
        write_frame(&mut encoder, last, Duration::from_millis(500))?;
        // The last frame shows how the recording ended; always offer it.
        let id = last.render_image().id;
        if sample
            .last()
            .is_none_or(|kept| kept.render_image().id != id)
        {
            sample.push(last.clone());
        }
    }
    Ok(sample)
}

fn write_frame(encoder: &mut GifEncoder<File>, frame: &Frame, delay: Duration) -> Result<()> {
    let delay = Delay::from_saturating_duration(delay);
    encoder.encode_frame(image::Frame::from_parts(frame.to_image(), 0, 0, delay))?;
    Ok(())
}

/// `count` items spread evenly from first to last.
fn spread(items: &[Frame], count: usize) -> Vec<Frame> {
    if items.is_empty() || count == 0 {
        return Vec::new();
    }
    let last = items.len() - 1;
    let count = count.min(items.len());
    (0..count)
        .map(|ix| {
            items[if count <= 1 {
                last
            } else {
                ix * last / (count - 1)
            }]
            .clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_to_a_gif_with_keyframes() {
        let dir = std::env::temp_dir().join(format!("ph-rec-{}", std::process::id()));
        let mut recorder = Recorder::start(&dir).unwrap();
        for shade in [0u8, 120, 240] {
            recorder.push(&Frame::from_rgba(4, 4, vec![shade; 64]));
            std::thread::sleep(Duration::from_millis(130));
        }
        let recording = recorder.finish(2).unwrap();
        assert_eq!(recording.keyframes.len(), 2);
        assert_eq!(recording.keyframes[1].bgra()[0], 240);
        assert!(std::fs::metadata(&recording.gif).unwrap().len() > 0);
    }
}
