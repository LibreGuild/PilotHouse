//! Reading and writing video: the system's own frameworks first
//! (AVFoundation on macOS, Media Foundation on Windows, GStreamer on
//! Linux), else `ffmpeg` when it is installed. Nothing is bundled:
//! PilotHouse carries no codec.
//!
//! Every call here blocks; callers run them off the UI thread.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use image::RgbaImage;

use super::ffmpeg::Ffmpeg;

/// What a video is.
#[derive(Debug, Clone)]
pub struct Info {
    /// Seconds.
    pub duration: f64,
    /// Frame size in pixels, upright.
    pub size: (u32, u32),
    /// Frames a second, nominal (screen recordings vary).
    pub fps: f64,
    pub audio: bool,
    pub(crate) source: Source,
}

/// Who reads it.
#[derive(Debug, Clone)]
pub(crate) enum Source {
    /// AVFoundation; frames turned `turns` quarters to stand upright.
    #[cfg(target_os = "macos")]
    Av {
        turns: u8,
    },
    /// Media Foundation; frames turned `turns` quarters to stand upright.
    #[cfg(windows)]
    Mf {
        turns: u8,
    },
    /// GStreamer (which stands frames upright itself).
    #[cfg(target_os = "linux")]
    Gst,
    Ffmpeg(Ffmpeg),
}

impl Info {
    /// What reads it, for the user.
    pub fn reader(&self) -> &'static str {
        match self.source {
            #[cfg(target_os = "macos")]
            Source::Av { .. } => "AVFoundation",
            #[cfg(windows)]
            Source::Mf { .. } => "Media Foundation",
            #[cfg(target_os = "linux")]
            Source::Gst => "GStreamer",
            Source::Ffmpeg(_) => "ffmpeg",
        }
    }
}

/// A part of the source's sound kept: `from`..`to` of the source, at
/// result time `at`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioPiece {
    pub from: f64,
    pub to: f64,
    pub at: f64,
}

/// Frames in order: when each starts (source seconds) and its pixels.
/// Made and used on one thread (the system readers stay where made).
pub trait Decoder {
    fn next(&mut self) -> Result<Option<(f64, RgbaImage)>>;
}

/// Writes frames (and the source's sound for the kept parts), on the
/// thread it was made on.
pub trait Encoder {
    /// `frame` shown from result time `time`.
    fn push(&mut self, frame: &RgbaImage, time: f64) -> Result<()>;
    fn finish(self: Box<Self>) -> Result<()>;
    /// Stops, leaving a broken file for the caller to remove.
    fn abort(self: Box<Self>);
}

/// What writes a video file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Vp9,
}

/// What an encoder writes.
#[derive(Debug, Clone)]
pub struct Target {
    pub size: (u32, u32),
    pub fps: f64,
    /// The result's length; its sound ends there too.
    pub seconds: f64,
    /// Bits a second, for the pictures.
    pub bitrate: u32,
    pub codec: Codec,
    /// The source's sound for the kept parts (none: silent).
    pub audio: Vec<AudioPiece>,
}

/// `ffmpeg` and `ffprobe`, when installed (looked for once).
pub fn ffmpeg() -> Option<&'static Ffmpeg> {
    static FOUND: OnceLock<Option<Ffmpeg>> = OnceLock::new();
    FOUND.get_or_init(Ffmpeg::find).as_ref()
}

/// The system's own reading.
fn native_probe(path: &Path) -> Result<Info> {
    #[cfg(target_os = "macos")]
    return super::av::probe(path);
    #[cfg(windows)]
    return super::mf::probe(path);
    #[cfg(target_os = "linux")]
    return super::gst::probe(path);
    #[allow(unreachable_code)]
    {
        let _ = path;
        Err(anyhow!(
            "this system has no video reader {} uses",
            ph_brand::NAME
        ))
    }
}

pub fn probe(path: &Path) -> Result<Info> {
    let native = match native_probe(path) {
        Ok(info) => return Ok(info),
        Err(err) => err,
    };
    match ffmpeg() {
        Some(ffmpeg) => ffmpeg.probe(path),
        // The system's reason says what to install.
        None => Err(native.context(concat!(
            ph_brand::name!(),
            " cannot read this video, and ffmpeg (which reads more kinds) is not installed"
        ))),
    }
}

/// The frame shown at source time `t`, full size, upright.
pub fn frame_at(path: &Path, info: &Info, t: f64) -> Result<RgbaImage> {
    let t = t.clamp(0., (info.duration - 0.001).max(0.));
    match &info.source {
        #[cfg(target_os = "macos")]
        Source::Av { turns } => super::av::frame_at(path, *turns, t, info.fps),
        #[cfg(windows)]
        Source::Mf { turns } => super::mf::frame_at(path, *turns, t),
        #[cfg(target_os = "linux")]
        Source::Gst => super::gst::frame_at(path, t),
        Source::Ffmpeg(ffmpeg) => ffmpeg.frame_at(path, info, t),
    }
}

/// When each frame starts, in order.
pub fn frame_times(path: &Path, info: &Info) -> Result<Vec<f64>> {
    match &info.source {
        #[cfg(target_os = "macos")]
        Source::Av { .. } => super::av::frame_times(path, info.duration),
        #[cfg(windows)]
        Source::Mf { .. } => super::mf::frame_times(path),
        #[cfg(target_os = "linux")]
        Source::Gst => super::gst::frame_times(path),
        Source::Ffmpeg(ffmpeg) => ffmpeg.frame_times(path),
    }
}

/// The frames of `from`..`to`. `rate` is how many a second are wanted at
/// most (a decoder may give fewer or more).
pub fn decoder(
    path: &Path,
    info: &Info,
    from: f64,
    to: f64,
    rate: f64,
) -> Result<Box<dyn Decoder>> {
    match &info.source {
        #[cfg(target_os = "macos")]
        Source::Av { turns } => Ok(Box::new(super::av::decoder(path, *turns, from, to)?)),
        #[cfg(windows)]
        Source::Mf { turns } => Ok(Box::new(super::mf::decoder(path, *turns, from, to)?)),
        #[cfg(target_os = "linux")]
        Source::Gst => Ok(Box::new(super::gst::decoder(path, from, to)?)),
        Source::Ffmpeg(ffmpeg) => Ok(Box::new(ffmpeg.decoder(path, info, from, to, rate)?)),
    }
}

/// The system's own writer for a source it reads, when it writes `codec`.
fn native_encoder(
    info: &Info,
    source: &Path,
    out: &Path,
    target: &Target,
) -> Option<Result<Box<dyn Encoder>>> {
    let _ = (source, out);
    match &info.source {
        #[cfg(target_os = "macos")]
        Source::Av { .. } if target.codec != Codec::Vp9 => Some(
            super::av::encoder(source, out, target)
                .map(|encoder| Box::new(encoder) as Box<dyn Encoder>),
        ),
        #[cfg(windows)]
        Source::Mf { .. } if target.codec != Codec::Vp9 => Some(
            super::mf::encoder(source, out, target)
                .map(|encoder| Box::new(encoder) as Box<dyn Encoder>),
        ),
        #[cfg(target_os = "linux")]
        Source::Gst => Some(
            super::gst::encoder(source, out, target)
                .map(|encoder| Box::new(encoder) as Box<dyn Encoder>),
        ),
        _ => None,
    }
}

/// An encoder writing `target` to `out`, with the sound of `source`: the
/// system's own when it read the source, else ffmpeg (which reads the
/// sound itself).
pub fn encoder(
    info: &Info,
    source: &Path,
    out: &Path,
    mut target: Target,
) -> Result<Box<dyn Encoder>> {
    if !info.audio {
        target.audio.clear();
    }
    let native = match native_encoder(info, source, out, &target) {
        Some(Ok(encoder)) => return Ok(encoder),
        Some(Err(err)) => Some(err),
        None => None,
    };
    match (ffmpeg(), native) {
        (Some(ffmpeg), None) => Ok(Box::new(ffmpeg.encoder(source, out, &target)?)),
        (Some(ffmpeg), Some(native)) => match ffmpeg.encoder(source, out, &target) {
            Ok(encoder) => Ok(Box::new(encoder)),
            Err(err) => Err(native.context(format!("ffmpeg could not either: {err:#}"))),
        },
        (None, Some(native)) => Err(native.context(concat!(
            ph_brand::name!(),
            " cannot write it, and ffmpeg (which writes more kinds) is not installed"
        ))),
        (None, None) => Err(anyhow!(
            "{} is written with ffmpeg here, and ffmpeg is not installed",
            match target.codec {
                Codec::Vp9 => "WebM",
                Codec::Hevc => "HEVC",
                Codec::H264 => "H.264",
            }
        )),
    }
}

/// Whether this system writes `codec`, itself or with ffmpeg (for what the
/// export offers).
pub fn can_write(codec: Codec) -> bool {
    ffmpeg().is_some() || native_writes(codec)
}

fn native_writes(codec: Codec) -> bool {
    #[cfg(target_os = "macos")]
    return codec != Codec::Vp9;
    #[cfg(windows)]
    return match codec {
        Codec::H264 => true,
        Codec::Hevc => super::mf::writes_hevc(),
        Codec::Vp9 => false,
    };
    #[cfg(target_os = "linux")]
    return super::gst::writes(codec);
    #[allow(unreachable_code)]
    {
        let _ = codec;
        false
    }
}

/// `fps` as a fraction: the NTSC rates exactly (30000/1001), whole ones
/// over 1, others to a thousandth.
#[cfg(any(windows, target_os = "linux", test))]
pub(crate) fn rational(fps: f64) -> (u32, u32) {
    let fps = if fps.is_finite() && fps > 0. {
        fps.min(1000.)
    } else {
        30.
    };
    for base in [24., 30., 48., 60., 120., 240.] {
        if (fps - base * 1000. / 1001.).abs() < 0.005 {
            return ((base * 1000.) as u32, 1001);
        }
    }
    if (fps - fps.round()).abs() < 0.005 {
        return (fps.round().max(1.) as u32, 1);
    }
    ((fps * 1000.).round().max(1.) as u32, 1000)
}

/// Where a file is written until it is done: beside it, hidden, with the
/// same extension (ffmpeg goes by it).
pub(crate) fn partial(out: &Path) -> PathBuf {
    let name = out
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = out
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    out.with_file_name(format!(
        ".{name}.pilothouse-{}{extension}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_as_fractions() {
        assert_eq!(rational(29.97), (30_000, 1001));
        assert_eq!(rational(30_000. / 1001.), (30_000, 1001));
        assert_eq!(rational(23.976), (24_000, 1001));
        assert_eq!(rational(30.), (30, 1));
        assert_eq!(rational(12.5), (12_500, 1000));
        assert_eq!(rational(0.), (30, 1));
        assert_eq!(rational(f64::NAN), (30, 1));
    }
}
