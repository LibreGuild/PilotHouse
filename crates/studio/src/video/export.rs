//! Writing the result: each kept part decoded in turn, each frame of the
//! result made (cropped, hidden areas hidden, resized, marked) and handed
//! to the encoder at a steady frame rate. The source's sound for parts
//! kept at their own speed goes with it; parts sped up or slowed down are
//! silent. The file is written beside its place and moved there when done.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use anyhow::{Context as _, Result, bail};
use image::RgbaImage;
use image::codecs::gif::{GifEncoder, Repeat};

use super::compose::Composer;
use super::edit::{VideoEdit, VideoExport, VideoFormat};
use super::media::{self, AudioPiece, Codec, Info, Target};

/// How far an export has got, and asking it to stop.
#[derive(Default)]
pub struct Progress {
    /// Thousandths done.
    done: AtomicU32,
    cancel: AtomicBool,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        self.done.load(Ordering::Relaxed) as f32 / 1000.
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn set(&self, fraction: f64) {
        self.done
            .store((fraction.clamp(0., 1.) * 1000.) as u32, Ordering::Relaxed);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A file written.
#[derive(Debug, Clone)]
pub struct Exported {
    pub path: PathBuf,
    pub bytes: u64,
    pub size: (u32, u32),
    pub seconds: f64,
}

/// What an export writes: the size of its frames, frames a second, its
/// bitrate and its sound.
#[derive(Debug, Clone)]
pub struct Plan {
    pub size: (u32, u32),
    pub fps: f64,
    pub seconds: f64,
    pub bitrate: u32,
    pub audio: Vec<AudioPiece>,
}

impl Plan {
    pub fn new(info: &Info, edit: &VideoEdit, export: &VideoExport) -> Plan {
        let fps = export.frame_rate(info.fps);
        let seconds = edit.output_duration(info.duration);
        let mut size = edit.output_size(info.size);
        if export.format == VideoFormat::Gif && size.0 > export.gif_width {
            let scale = export.gif_width as f32 / size.0 as f32;
            size = (
                export.gif_width & !1,
                (((size.1 as f32 * scale).round() as u32) & !1).max(2),
            );
        }
        let mut audio = Vec::new();
        if !edit.mute && export.format.has_sound() && info.audio {
            let mut at = 0.;
            for piece in edit.pieces(info.duration) {
                if piece.speed == 1. {
                    audio.push(AudioPiece {
                        from: piece.start,
                        to: piece.end,
                        at,
                    });
                }
                at += piece.length();
            }
        }
        let audio_rate = if audio.is_empty() { 0 } else { 128_000 };
        Plan {
            size,
            fps,
            seconds,
            bitrate: export.bitrate(size.0, size.1, fps, seconds, audio_rate),
            audio,
        }
    }

    /// About how many bytes it writes (GIFs: unknown).
    pub fn estimate(&self, format: VideoFormat) -> Option<u64> {
        if format == VideoFormat::Gif {
            return None;
        }
        let audio = if self.audio.is_empty() { 0. } else { 128_000. };
        Some(((self.bitrate as f64 + audio) * self.seconds / 8. * 1.02) as u64)
    }
}

/// What writes `format` (a GIF is written here).
fn codec(format: VideoFormat) -> Codec {
    match format {
        VideoFormat::Hevc => Codec::Hevc,
        VideoFormat::Webm => Codec::Vp9,
        _ => Codec::H264,
    }
}

/// Whether this system writes `format` (for what the export offers).
pub fn writable(format: VideoFormat) -> bool {
    format == VideoFormat::Gif || media::can_write(codec(format))
}

/// Writes `edit` of the video at `source` to `out`.
pub fn export(
    source: &Path,
    info: &Info,
    edit: &VideoEdit,
    export: &VideoExport,
    out: &Path,
    progress: &Progress,
) -> Result<Exported> {
    let mut plan = Plan::new(info, edit, export);
    let partial = media::partial(out);
    let result = if export.format == VideoFormat::Gif {
        // A GIF over its limit is made again, smaller.
        let mut tries = 0;
        loop {
            write_gif(source, info, edit, &plan, &partial, progress)?;
            let bytes = std::fs::metadata(&partial)
                .map(|meta| meta.len())
                .unwrap_or(0);
            let limit = export.max_mb.map(|mb| mb as u64 * 1024 * 1024);
            match limit {
                Some(limit) if bytes > limit && tries < 3 && plan.size.0 > 160 => {
                    let shrink = ((limit as f64 / bytes as f64).sqrt() * 0.9).clamp(0.3, 0.95);
                    plan.size = (
                        ((plan.size.0 as f64 * shrink) as u32 & !1).max(2),
                        ((plan.size.1 as f64 * shrink) as u32 & !1).max(2),
                    );
                    tries += 1;
                }
                _ => break Ok(()),
            }
        }
    } else {
        let codec = codec(export.format);
        let encoder = media::encoder(
            info,
            source,
            &partial,
            Target {
                size: plan.size,
                fps: plan.fps,
                seconds: plan.seconds,
                bitrate: plan.bitrate,
                codec,
                audio: plan.audio.clone(),
            },
        )?;
        write_frames(source, info, edit, &plan, encoder, progress)
    };
    if let Err(err) = result {
        let _ = std::fs::remove_file(&partial);
        return Err(err);
    }
    std::fs::rename(&partial, out).map_err(|err| {
        let _ = std::fs::remove_file(&partial);
        anyhow::anyhow!("could not save {}: {err}", out.display())
    })?;
    progress.set(1.);
    Ok(Exported {
        path: out.to_path_buf(),
        bytes: std::fs::metadata(out).map(|meta| meta.len()).unwrap_or(0),
        size: plan.size,
        seconds: plan.seconds,
    })
}

/// Each frame of the result, in order, at the plan's steady rate: the
/// source frame showing at that moment, made into the result's.
fn each_frame(
    source: &Path,
    info: &Info,
    edit: &VideoEdit,
    plan: &Plan,
    progress: &Progress,
    mut take: impl FnMut(&RgbaImage, f64) -> Result<()>,
) -> Result<()> {
    let mut composer = Composer::new(edit.clone(), info.size);
    let total = plan.seconds.max(0.001);
    let mut index: u64 = 0;
    let mut piece_start = 0.;
    for piece in edit.pieces(info.duration) {
        let piece_end = piece_start + piece.length();
        let speed = piece.speed as f64;
        let mut decoder = media::decoder(source, info, piece.start, piece.end, plan.fps * speed)?;
        let mut current: Option<RgbaImage> = None;
        let mut upcoming = decoder.next()?;
        loop {
            let time = index as f64 / plan.fps;
            if time >= piece_end - 1e-9 {
                break;
            }
            if progress.cancelled() {
                bail!("Stopped.");
            }
            let at = piece.start + (time - piece_start) * speed;
            while upcoming
                .as_ref()
                .is_some_and(|(start, _)| *start <= at + 1e-4)
            {
                current = upcoming.take().map(|(_, frame)| frame);
                upcoming = decoder.next()?;
            }
            let frame = current
                .as_ref()
                .or(upcoming.as_ref().map(|(_, frame)| frame))
                .context("no frames could be read")?;
            let made = composer.frame(frame, at, plan.size);
            take(&made, time)?;
            index += 1;
            progress.set(time / total);
        }
        piece_start = piece_end;
    }
    if index == 0 {
        bail!("there is nothing to write");
    }
    Ok(())
}

fn write_frames(
    source: &Path,
    info: &Info,
    edit: &VideoEdit,
    plan: &Plan,
    mut encoder: Box<dyn media::Encoder>,
    progress: &Progress,
) -> Result<()> {
    let written = each_frame(source, info, edit, plan, progress, |frame, time| {
        encoder.push(frame, time)
    });
    match written {
        Ok(()) => encoder.finish(),
        Err(err) => {
            encoder.abort();
            Err(err)
        }
    }
}

fn write_gif(
    source: &Path,
    info: &Info,
    edit: &VideoEdit,
    plan: &Plan,
    out: &Path,
    progress: &Progress,
) -> Result<()> {
    let file =
        std::fs::File::create(out).with_context(|| format!("could not write {}", out.display()))?;
    let mut encoder = GifEncoder::new_with_speed(std::io::BufWriter::new(file), 10);
    encoder.set_repeat(Repeat::Infinite)?;
    let delay = image::Delay::from_numer_denom_ms(1000, plan.fps.round().max(1.) as u32);
    each_frame(source, info, edit, plan, progress, |frame, _| {
        encoder
            .encode_frame(image::Frame::from_parts(frame.clone(), 0, 0, delay))
            .context("writing the GIF")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::edit::Segment;

    fn info() -> Info {
        Info {
            duration: 60.,
            size: (1920, 1080),
            fps: 29.97,
            audio: true,
            source: media::Source::Ffmpeg(crate::video::ffmpeg::Ffmpeg {
                ffmpeg: "ffmpeg".into(),
                ffprobe: "ffprobe".into(),
            }),
        }
    }

    #[test]
    fn plans_sound_only_for_parts_at_their_own_speed() {
        let edit = VideoEdit {
            segments: vec![
                Segment::new(0., 10., 1.),
                Segment::new(10., 50., 4.),
                Segment::new(50., 60., 1.),
            ],
            ..Default::default()
        };
        let plan = Plan::new(&info(), &edit, &VideoExport::default());
        assert_eq!(plan.fps, 30.);
        assert_eq!(plan.seconds, 30.);
        assert_eq!(
            plan.audio,
            [
                AudioPiece {
                    from: 0.,
                    to: 10.,
                    at: 0.
                },
                AudioPiece {
                    from: 50.,
                    to: 60.,
                    at: 20.
                },
            ]
        );
        let muted = VideoEdit {
            mute: true,
            ..edit.clone()
        };
        assert!(
            Plan::new(&info(), &muted, &VideoExport::default())
                .audio
                .is_empty()
        );
        let gif = VideoExport {
            format: VideoFormat::Gif,
            ..Default::default()
        };
        let plan = Plan::new(&info(), &edit, &gif);
        assert_eq!(plan.size, (720, 404));
        assert!(plan.audio.is_empty());
    }
}

/// Reads and writes a real video, with each reader this system has:
/// `STUDIO_TEST_VIDEO=clip.mp4 cargo test -p ph-studio real_video -- --ignored`.
#[cfg(test)]
mod real {
    use super::*;
    use crate::edit::{Color, MarkType};
    use crate::video::edit::Overlay;

    #[test]
    #[ignore]
    fn real_video() {
        let Ok(path) = std::env::var("STUDIO_TEST_VIDEO") else {
            return;
        };
        let source = PathBuf::from(path);
        let mut infos = vec![media::probe(&source).unwrap()];
        if let Some(ffmpeg) = media::ffmpeg() {
            infos.push(ffmpeg.probe(&source).unwrap());
        }
        for info in infos {
            let reader = info.reader();
            println!("{reader}: {info:?}");
            let times = media::frame_times(&source, &info).unwrap();
            println!(
                "{reader}: {} frames, first {:?}",
                times.len(),
                &times[..3.min(times.len())]
            );
            let frame = media::frame_at(&source, &info, 2.0).unwrap();
            assert_eq!(frame.dimensions(), info.size);
            let mut edit = VideoEdit::default();
            edit.cut(info.duration, 2., 4.);
            edit.speed_range(info.duration, 5., 7., 4.);
            edit.add_overlay(
                Overlay {
                    id: 0,
                    start: 0.5,
                    end: 6.,
                    fade: 0.3,
                    kind: MarkType::Arrow.mark([100., 100.], [400., 300.], Color::RED),
                },
                info.size,
            )
            .unwrap();
            edit.add_overlay(
                Overlay {
                    id: 0,
                    start: 0.,
                    end: 8.,
                    fade: 0.,
                    kind: MarkType::Blur.mark([800., 50.], [1200., 250.], Color::RED),
                },
                info.size,
            )
            .unwrap();
            for format in [VideoFormat::Mp4, VideoFormat::Gif] {
                let out = source.with_file_name(format!("out-{reader}.{}", format.extension()));
                let settings = VideoExport {
                    format,
                    ..Default::default()
                };
                let start = std::time::Instant::now();
                let done =
                    export(&source, &info, &edit, &settings, &out, &Progress::default()).unwrap();
                println!("{reader} {format:?}: {done:?} in {:?}", start.elapsed());
                assert!(done.bytes > 1000);
                assert!(
                    (done.seconds - 4.5).abs() < 0.01,
                    "0–2, 4–5, 5–7 at 4×, 7–8"
                );
            }
        }
    }
}
