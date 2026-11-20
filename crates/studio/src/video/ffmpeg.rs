//! Video through `ffmpeg`, when it is installed: for systems PilotHouse
//! has no native reader for yet, kinds the system cannot read (WebM, MKV),
//! and writing WebM. It runs as a separate program, fed and read through
//! pipes as raw RGBA frames.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::OnceLock;

use anyhow::{Context as _, Result, anyhow, bail};
use image::RgbaImage;
use serde_json::Value;

use super::media::{Codec, Decoder, Encoder, Info, Source, Target};

#[derive(Debug, Clone)]
pub struct Ffmpeg {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Places ffmpeg is often installed that a GUI app's PATH can miss.
const USUAL: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"];

impl Ffmpeg {
    pub fn find() -> Option<Ffmpeg> {
        let find = |name: &str| {
            ph_env::which(name).or_else(|| {
                USUAL
                    .iter()
                    .map(|dir| Path::new(dir).join(name))
                    .find(|path| path.is_file())
            })
        };
        Some(Ffmpeg {
            ffmpeg: find("ffmpeg")?,
            ffprobe: find("ffprobe")?,
        })
    }

    fn run(&self, program: &Path, args: &[&str]) -> Result<Vec<u8>> {
        let output = ph_env::command(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("could not run {}", program.display()))?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr);
            bail!("{}", message.lines().last().unwrap_or("ffmpeg failed"));
        }
        Ok(output.stdout)
    }

    pub fn probe(&self, path: &Path) -> Result<Info> {
        let path = path.to_string_lossy();
        let json = self.run(
            &self.ffprobe,
            &[
                "-protocol_whitelist",
                "file,pipe",
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_streams",
                "-show_format",
                &path,
            ],
        )?;
        let probe: Value = serde_json::from_slice(&json).context("ffprobe's answer")?;
        let streams = probe["streams"].as_array().cloned().unwrap_or_default();
        let video = streams
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .context("no video in it")?;
        let number = |value: &Value| value.as_str().and_then(|text| text.parse::<f64>().ok());
        let duration = number(&probe["format"]["duration"])
            .or_else(|| number(&video["duration"]))
            .filter(|duration| *duration > 0.)
            .context("ffprobe cannot tell how long it is")?;
        let (width, height) = (
            video["width"].as_u64().unwrap_or(0) as u32,
            video["height"].as_u64().unwrap_or(0) as u32,
        );
        if width == 0 || height == 0 {
            bail!("ffprobe cannot tell the frame size");
        }
        // ffmpeg turns frames upright as it reads them.
        let rotation = video["side_data_list"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|data| data["rotation"].as_f64())
            .or_else(|| number(&video["tags"]["rotate"]))
            .unwrap_or(0.);
        let turned = (rotation.round() as i64).rem_euclid(180) == 90;
        let fps = video["avg_frame_rate"]
            .as_str()
            .and_then(ratio)
            .or_else(|| video["r_frame_rate"].as_str().and_then(ratio))
            .filter(|fps| *fps > 0. && *fps < 1000.)
            .unwrap_or(30.);
        Ok(Info {
            duration,
            size: if turned {
                (height, width)
            } else {
                (width, height)
            },
            fps,
            audio: streams.iter().any(|stream| stream["codec_type"] == "audio"),
            source: Source::Ffmpeg(self.clone()),
        })
    }

    /// Where `-ss` has to point for ffmpeg to begin with the frame showing
    /// at `t`: just before that frame's start. ffmpeg begins with the
    /// first frame starting at or after `-ss`; in a recording that has a
    /// frame only when the screen changes, that one can be seconds later
    /// than `t`, and past the last frame's start there is none. So the
    /// frames' own times around `t` are read first (from the key frame
    /// before, without decoding). None when they cannot be told.
    fn seek_for(&self, path: &Path, t: f64) -> Option<f64> {
        let listing = self
            .run(
                &self.ffprobe,
                &[
                    "-protocol_whitelist",
                    "file,pipe",
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    // Some way past `t`: with B-frames the packets are not
                    // stored in the order shown.
                    "-read_intervals",
                    &format!("{:.6}%{:.6}", (t - NEAR).max(0.), t + 2. * NEAR),
                    "-show_entries",
                    "packet=pts_time:format=start_time",
                    "-of",
                    "csv",
                    &path.to_string_lossy(),
                ],
            )
            .ok()?;
        let shown = showing_at(&String::from_utf8_lossy(&listing), t)?;
        Some((shown - SEEK_LEAD).max(0.))
    }

    pub fn frame_at(&self, path: &Path, info: &Info, t: f64) -> Result<RgbaImage> {
        let (width, height) = info.size;
        // Where it cannot be told, the first frame from `t` on.
        let seek = self.seek_for(path, t).unwrap_or(t);
        let bytes = self.run(
            &self.ffmpeg,
            &[
                "-v",
                "error",
                "-ss",
                &format!("{seek:.6}"),
                // Files only: a ".mp4" that is really a playlist must not
                // make ffmpeg fetch addresses or read other files.
                "-protocol_whitelist",
                "file,pipe",
                "-i",
                &path.to_string_lossy(),
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ],
        )?;
        if bytes.is_empty() {
            bail!("ffmpeg found no frame at {t:.3}s");
        }
        RgbaImage::from_raw(width, height, bytes).context("ffmpeg gave a frame of another size")
    }

    pub fn frame_times(&self, path: &Path) -> Result<Vec<f64>> {
        let text = self.run(
            &self.ffprobe,
            &[
                "-protocol_whitelist",
                "file,pipe",
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "packet=pts_time",
                "-of",
                "csv=p=0",
                &path.to_string_lossy(),
            ],
        )?;
        let mut times: Vec<f64> = String::from_utf8_lossy(&text)
            .lines()
            .filter_map(|line| line.trim().trim_end_matches(',').parse().ok())
            .collect();
        times.sort_by(f64::total_cmp);
        times.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        Ok(times)
    }

    /// Frames of `from`..`to` at `rate` a second (ffmpeg drops or repeats
    /// to keep the rate, so each frame's time is known).
    pub fn decoder(
        &self,
        path: &Path,
        info: &Info,
        from: f64,
        to: f64,
        rate: f64,
    ) -> Result<FfmpegDecoder> {
        // It begins with the frame showing at `from` (see `seek_for`), put
        // at `from`. Where a part starts in a stretch with no new frame,
        // the first frame from `from` on is later by the rest of that
        // stretch, and every frame of the part would be given that much
        // too early.
        let seek = self
            .seek_for(path, from)
            .filter(|seek| *seek < from)
            .unwrap_or(from);
        let lead = from - seek;
        let mut child = ph_env::command(&self.ffmpeg)
            .args([
                "-v",
                "error",
                "-ss",
                &format!("{seek:.6}"),
                // Files only: a ".mp4" that is really a playlist must not
                // make ffmpeg fetch addresses or read other files.
                "-protocol_whitelist",
                "file,pipe",
                "-i",
                &path.to_string_lossy(),
                "-t",
                &format!("{:.4}", (to - from).max(0.001)),
                "-map",
                "0:v:0",
                // The frames from `seek` on, `lead` earlier (the first at
                // 0, which is `from`), then at a steady rate from 0: a
                // first frame that is later is repeated back to there, not
                // taken as the start.
                "-vf",
                &format!("setpts=max(PTS-{lead:.6}/TB\\,0),fps={rate:.4}:start_time=0"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("could not start ffmpeg")?;
        let stdout = child.stdout.take().context("no output from ffmpeg")?;
        Ok(FfmpegDecoder {
            child,
            stdout,
            size: info.size,
            from,
            rate,
            index: 0,
        })
    }

    /// The encoders this ffmpeg has, by name (asked once).
    fn encoders(&self) -> &'static str {
        static LIST: OnceLock<String> = OnceLock::new();
        LIST.get_or_init(|| {
            self.run(&self.ffmpeg, &["-hide_banner", "-encoders"])
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default()
        })
    }

    /// The first encoder this ffmpeg has for `codec`.
    fn video_encoder(&self, codec: Codec) -> Result<&'static str> {
        let wanted: &[&str] = match codec {
            Codec::H264 => &[
                "libx264",
                "h264_videotoolbox",
                "h264_mf",
                "h264_nvenc",
                "h264_qsv",
                "libopenh264",
            ],
            Codec::Hevc => &[
                "libx265",
                "hevc_videotoolbox",
                "hevc_mf",
                "hevc_nvenc",
                "hevc_qsv",
            ],
            Codec::Vp9 => &["libvpx-vp9", "libvpx"],
        };
        let list = self.encoders();
        wanted
            .iter()
            .find(|name| list.split_whitespace().any(|word| word == **name))
            .copied()
            .ok_or_else(|| anyhow!("this ffmpeg has no encoder for {codec:?}"))
    }

    pub fn encoder(&self, source: &Path, out: &Path, target: &Target) -> Result<FfmpegEncoder> {
        let (size, fps, bitrate, codec) = (target.size, target.fps, target.bitrate, target.codec);
        let audio = &target.audio;
        let encoder = self.video_encoder(codec)?;
        let mut args: Vec<String> = ["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba"]
            .map(String::from)
            .to_vec();
        args.extend([
            "-s".into(),
            format!("{}x{}", size.0, size.1),
            "-r".into(),
            format!("{fps:.4}"),
            "-i".into(),
            "pipe:0".into(),
        ]);
        if !audio.is_empty() {
            args.extend([
                "-protocol_whitelist".into(),
                "file,pipe".into(),
                "-i".into(),
                source.to_string_lossy().into_owned(),
            ]);
            // Each kept piece of sound, delayed to its place in the result.
            let mut graph = String::new();
            for (ix, piece) in audio.iter().enumerate() {
                let delay = (piece.at * 1000.).round() as u64;
                graph.push_str(&format!(
                    "[1:a]atrim={:.4}:{:.4},asetpts=PTS-STARTPTS,adelay={delay}:all=1[a{ix}];",
                    piece.from, piece.to
                ));
            }
            for ix in 0..audio.len() {
                graph.push_str(&format!("[a{ix}]"));
            }
            graph.push_str(&format!(
                "amix=inputs={}:normalize=0:dropout_transition=0[aout]",
                audio.len()
            ));
            args.extend([
                "-filter_complex".into(),
                graph,
                "-map".into(),
                "0:v".into(),
                "-map".into(),
                "[aout]".into(),
            ]);
        }
        let rate = bitrate.to_string();
        args.extend(["-c:v".into(), encoder.into(), "-b:v".into(), rate.clone()]);
        args.extend([
            "-maxrate".into(),
            (bitrate as u64 * 3 / 2).to_string(),
            "-bufsize".into(),
            (bitrate as u64 * 2).to_string(),
        ]);
        match codec {
            Codec::H264 | Codec::Hevc => {
                args.extend([
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                    "-movflags".into(),
                    "+faststart".into(),
                ]);
                if encoder == "libx264" || encoder == "libx265" {
                    args.extend(["-preset".into(), "veryfast".into()]);
                }
                if codec == Codec::Hevc {
                    args.extend(["-tag:v".into(), "hvc1".into()]);
                }
                if !audio.is_empty() {
                    args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "128k".into()]);
                }
            }
            Codec::Vp9 => {
                args.extend([
                    "-pix_fmt".into(),
                    "yuv420p".into(),
                    "-deadline".into(),
                    "realtime".into(),
                    "-row-mt".into(),
                    "1".into(),
                ]);
                if !audio.is_empty() {
                    args.extend([
                        "-c:a".into(),
                        "libopus".into(),
                        "-b:a".into(),
                        "128k".into(),
                    ]);
                }
            }
        }
        args.push(out.to_string_lossy().into_owned());
        let mut child = ph_env::command(&self.ffmpeg)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not start ffmpeg")?;
        let stdin = child.stdin.take().context("no input to ffmpeg")?;
        Ok(FfmpegEncoder {
            child,
            stdin: Some(stdin),
        })
    }
}

/// How far before a frame's start `-ss` is sent for ffmpeg to begin with
/// that frame: less than any frame lasts, more than a time loses when it
/// is printed and rounded to the file's own clock.
const SEEK_LEAD: f64 = 0.0005;
/// How far a file's times may start from zero and still be worked out,
/// in seconds: the packets around a moment are read from this much before
/// it.
const NEAR: f64 = 1.;

/// The start of the frame showing at `t`, as `-ss` counts (from the
/// file's start), out of ffprobe's list of the packets around `t`:
/// `packet,<its time>` lines in the order stored, which with B-frames is
/// not the order shown, and `format,<the file's start>`. None when no
/// frame listed has started by `t`, or the file starts too far from zero
/// for the list to hold the right packets.
fn showing_at(listing: &str, t: f64) -> Option<f64> {
    let mut starts = Vec::new();
    let mut file_start = 0.;
    for line in listing.lines() {
        let Some((kind, rest)) = line.trim().split_once(',') else {
            continue;
        };
        let Some(time) = rest
            .split(',')
            .next()
            .and_then(|time| time.parse::<f64>().ok())
        else {
            continue;
        };
        match kind {
            "packet" => starts.push(time),
            "format" => file_start = time,
            _ => {}
        }
    }
    if file_start.abs() > NEAR {
        return None;
    }
    starts
        .into_iter()
        .map(|start| start - file_start)
        .filter(|start| *start <= t + 1e-4)
        .max_by(f64::total_cmp)
}

/// `30000/1001` as a number.
fn ratio(text: &str) -> Option<f64> {
    let (top, bottom) = text.split_once('/')?;
    let (top, bottom): (f64, f64) = (top.parse().ok()?, bottom.parse().ok()?);
    (bottom > 0.).then(|| top / bottom)
}

pub struct FfmpegDecoder {
    child: Child,
    stdout: ChildStdout,
    size: (u32, u32),
    from: f64,
    rate: f64,
    index: u64,
}

impl Decoder for FfmpegDecoder {
    fn next(&mut self) -> Result<Option<(f64, RgbaImage)>> {
        let mut bytes = vec![0; self.size.0 as usize * self.size.1 as usize * 4];
        let mut read = 0;
        while read < bytes.len() {
            match self.stdout.read(&mut bytes[read..])? {
                0 => return Ok(None),
                count => read += count,
            }
        }
        let time = self.from + self.index as f64 / self.rate;
        self.index += 1;
        let image = RgbaImage::from_raw(self.size.0, self.size.1, bytes).context("a frame")?;
        Ok(Some((time, image)))
    }
}

impl Drop for FfmpegDecoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct FfmpegEncoder {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Encoder for FfmpegEncoder {
    fn push(&mut self, frame: &RgbaImage, _: f64) -> Result<()> {
        let stdin = self.stdin.as_mut().context("ffmpeg stopped")?;
        stdin
            .write_all(frame.as_raw())
            .map_err(|_| anyhow!("ffmpeg stopped taking frames"))
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        drop(self.stdin.take());
        let mut errors = String::new();
        if let Some(mut stderr) = self.child.stderr.take() {
            let _ = stderr.read_to_string(&mut errors);
        }
        let status = self.child.wait().context("ffmpeg")?;
        if !status.success() {
            bail!(
                "ffmpeg could not write the video: {}",
                errors.lines().last().unwrap_or("it failed")
            );
        }
        Ok(())
    }

    fn abort(mut self: Box<Self>) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::showing_at;

    #[test]
    fn the_frame_showing_is_the_last_one_started() {
        // A frame a second, stored as B-frames are: not in the order shown.
        let listing = "packet,0.000000\npacket,3.000000\npacket,1.000000\n\
                       packet,2.000000\npacket,4.000000\nformat,0.000000\n";
        assert_eq!(showing_at(listing, 2.5), Some(2.));
        assert_eq!(showing_at(listing, 2.), Some(2.));
        assert_eq!(showing_at(listing, 1.99), Some(1.));
        // Past the last frame's start it is the last frame.
        assert_eq!(showing_at(listing, 9.), Some(4.));
        // A file that starts a little after zero: counted from its start.
        let late = "packet,0.250000\npacket,1.250000\npacket,2.250000\nformat,0.250000\n";
        assert_eq!(showing_at(late, 1.5), Some(1.));
        // More after the time on a line does not matter.
        assert_eq!(showing_at("packet,1.000000,K__\n", 1.), Some(1.));
        // Not told: a file starting far from zero, no frame started yet,
        // times ffprobe does not know, nothing at all.
        assert_eq!(
            showing_at("packet,90.000000\nformat,90.000000\n", 0.5),
            None
        );
        assert_eq!(showing_at("packet,2.000000\nformat,0.000000\n", 1.), None);
        assert_eq!(showing_at("packet,N/A\nformat,N/A\n", 1.), None);
        assert_eq!(showing_at("", 1.), None);
    }
}
