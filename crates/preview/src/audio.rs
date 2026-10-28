//! Sound files: a waveform to click and seek in, play and pause, the time,
//! and playback speed. MP3, WAV, FLAC, Ogg Vorbis and AAC/M4A decode in
//! PilotHouse itself, the same on every system.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Bounds, Context, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, Render, SharedString, Styled as _, Task, Window,
    canvas, div, fill, point, px, size,
};
use rodio::Source as _;

use crate::byte_size;

/// Bars in the waveform.
const BARS: usize = 240;
const SPEEDS: [f32; 5] = [0.75, 1., 1.25, 1.5, 2.];
/// How often the time moves while playing.
const TICK: Duration = Duration::from_millis(100);

/// What the whole file looks like, read once.
#[derive(Debug, Clone, PartialEq)]
pub struct Waveform {
    /// Loudest sample per bar, 0–1.
    pub peaks: Vec<f32>,
    pub duration: Duration,
    pub channels: u16,
    pub sample_rate: u32,
}

/// Reads the whole file for its waveform and exact length. The samples are
/// not kept: each block of them leaves only its loudest, and when there are
/// too many blocks, pairs of them become one (an hour of sound is over a
/// gigabyte of samples).
pub fn waveform(path: &Path) -> Result<Waveform, String> {
    const MOST_BLOCKS: usize = 8192;
    let file = std::fs::File::open(path).map_err(|err| err.to_string())?;
    let decoder = rodio::Decoder::try_from(file).map_err(|err| describe(&err.to_string()))?;
    let channels = decoder.channels().get();
    let sample_rate = decoder.sample_rate().get();
    let mut blocks = Vec::new();
    let mut block = 1usize;
    let (mut loudest_in_block, mut in_block, mut samples) = (0f32, 0usize, 0usize);
    for sample in decoder {
        loudest_in_block = loudest_in_block.max(sample.abs());
        in_block += 1;
        samples += 1;
        if in_block == block {
            blocks.push(loudest_in_block);
            (loudest_in_block, in_block) = (0., 0);
            if blocks.len() == MOST_BLOCKS {
                blocks = blocks
                    .chunks(2)
                    .map(|pair| pair.iter().copied().fold(0., f32::max))
                    .collect();
                block *= 2;
            }
        }
    }
    if in_block > 0 {
        blocks.push(loudest_in_block);
    }
    if samples == 0 {
        return Err("The file has no sound.".into());
    }
    let peaks = bars(&blocks);
    let frames = samples as f64 / channels.max(1) as f64;
    Ok(Waveform {
        peaks,
        duration: Duration::from_secs_f64(frames / sample_rate.max(1) as f64),
        channels,
        sample_rate,
    })
}

/// [`BARS`] bars (fewer for a short sound) from the loudest sample of each
/// block, scaled so the loudest bar is full height.
fn bars(blocks: &[f32]) -> Vec<f32> {
    let per_bar = blocks.len().div_ceil(BARS).max(1);
    let peaks: Vec<f32> = blocks
        .chunks(per_bar)
        .map(|chunk| chunk.iter().copied().fold(0., f32::max).min(1.))
        .collect();
    let loudest = peaks.iter().copied().fold(0.01, f32::max);
    peaks.into_iter().map(|peak| peak / loudest).collect()
}

fn describe(err: &str) -> String {
    if err.to_lowercase().contains("unrecognized") || err.to_lowercase().contains("unsupported") {
        "This sound format cannot be played here (try Open in Default App).".into()
    } else {
        format!("Could not read the sound: {err}")
    }
}

/// `1:05`, `1:02:03`.
pub fn clock(time: Duration) -> String {
    let seconds = time.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Where the sound plays: rodio on macOS and Windows (Core Audio,
/// WASAPI); on Linux the GStreamer player the video view uses, loaded when
/// needed, so PilotHouse does not depend on ALSA to start.
#[cfg(not(target_os = "linux"))]
struct Output {
    // The device goes quiet when this is dropped.
    _sink: rodio::MixerDeviceSink,
    player: rodio::Player,
    path: PathBuf,
}

#[cfg(not(target_os = "linux"))]
impl Output {
    fn open(path: &Path) -> Result<Self, String> {
        let mut sink = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|err| format!("No sound output: {err}"))?;
        sink.log_on_drop(false);
        let player = rodio::Player::connect_new(sink.mixer());
        let output = Self {
            _sink: sink,
            player,
            path: path.to_path_buf(),
        };
        output.queue()?;
        output.player.pause();
        Ok(output)
    }

    fn queue(&self) -> Result<(), String> {
        let file = std::fs::File::open(&self.path).map_err(|err| err.to_string())?;
        let decoder = rodio::Decoder::try_from(file).map_err(|err| describe(&err.to_string()))?;
        self.player.append(decoder);
        Ok(())
    }

    fn play(&self, from: Duration, speed: f32) -> Result<(), String> {
        // After the end, the sound is queued again.
        if self.player.empty() {
            self.queue()?;
        }
        _ = self.player.try_seek(from);
        self.player.set_speed(speed);
        self.player.play();
        Ok(())
    }

    fn pause(&self) {
        self.player.pause();
    }

    fn seek(&self, to: Duration) {
        _ = self.player.try_seek(to);
    }

    fn position(&self) -> Duration {
        self.player.get_pos()
    }

    fn set_speed(&self, speed: f32) {
        self.player.set_speed(speed);
    }

    fn is_playing(&self) -> bool {
        !self.player.is_paused() && !self.player.empty()
    }

    fn finished(&self) -> bool {
        self.player.empty()
    }
}

#[cfg(target_os = "linux")]
struct Output {
    engine: crate::video::Engine,
}

#[cfg(target_os = "linux")]
impl Output {
    fn open(path: &Path) -> Result<Self, String> {
        Ok(Self {
            engine: crate::video::Engine::open(path)?,
        })
    }

    fn play(&self, from: Duration, speed: f32) -> Result<(), String> {
        if let crate::video::Status::Failed(err) = self.engine.status() {
            return Err(err);
        }
        self.engine.seek(from.as_secs_f64());
        self.engine.play(speed);
        Ok(())
    }

    fn pause(&self) {
        self.engine.pause();
    }

    fn seek(&self, to: Duration) {
        self.engine.seek(to.as_secs_f64());
    }

    fn position(&self) -> Duration {
        Duration::from_secs_f64(self.engine.position())
    }

    fn set_speed(&self, speed: f32) {
        self.engine.set_rate(speed);
    }

    fn is_playing(&self) -> bool {
        self.engine.is_playing()
    }

    fn finished(&self) -> bool {
        !self.engine.is_playing()
    }
}

pub struct AudioPlayer {
    path: PathBuf,
    size: u64,
    wave: Option<Result<Arc<Waveform>, SharedString>>,
    output: Option<Output>,
    position: Duration,
    speed: usize,
    error: Option<SharedString>,
    /// Where the waveform was drawn, for clicks.
    wave_bounds: Rc<Cell<Bounds<Pixels>>>,
    _load: Option<Task<()>>,
    _tick: Option<Task<()>>,
}

impl AudioPlayer {
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            size: 0,
            wave: None,
            output: None,
            position: Duration::ZERO,
            speed: 1,
            error: None,
            wave_bounds: Rc::default(),
            _load: None,
            _tick: None,
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.stop();
        let path = self.path.clone();
        self.size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let read = cx.background_spawn(async move { waveform(&path) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let wave = read.await;
            _ = this.update(cx, |this, cx| {
                this.wave = Some(wave.map(Arc::new).map_err(Into::into));
                cx.notify();
            });
        }));
    }

    fn duration(&self) -> Duration {
        match &self.wave {
            Some(Ok(wave)) => wave.duration,
            _ => Duration::ZERO,
        }
    }

    fn is_playing(&self) -> bool {
        self.output.as_ref().is_some_and(Output::is_playing)
    }

    /// Plays from `from` (opening the sound output the first time).
    fn start(&mut self, from: Duration, cx: &mut Context<Self>) {
        if self.output.is_none() {
            match Output::open(&self.path) {
                Ok(output) => self.output = Some(output),
                Err(err) => {
                    self.error = Some(err.into());
                    cx.notify();
                    return;
                }
            }
        }
        if let Some(output) = &self.output
            && let Err(err) = output.play(from, SPEEDS[self.speed])
        {
            self.error = Some(err.into());
            cx.notify();
            return;
        }
        self.error = None;
        self.follow(cx);
        cx.notify();
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.is_playing() {
            self.pause(cx);
        } else {
            let from = if self.position >= self.duration() {
                Duration::ZERO
            } else {
                self.position
            };
            self.start(from, cx);
        }
    }

    pub fn pause(&mut self, cx: &mut Context<Self>) {
        if let Some(output) = &self.output {
            self.position = output.position();
            output.pause();
        }
        self._tick = None;
        cx.notify();
    }

    fn stop(&mut self) {
        self.output = None;
        self.position = Duration::ZERO;
        self._tick = None;
    }

    fn seek(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let to = self.duration().mul_f32(fraction.clamp(0., 1.));
        self.position = to;
        if self.is_playing() {
            self.start(to, cx);
        } else if let Some(output) = &self.output {
            output.seek(to);
        }
        cx.notify();
    }

    fn next_speed(&mut self, cx: &mut Context<Self>) {
        self.speed = (self.speed + 1) % SPEEDS.len();
        if let Some(output) = &self.output {
            output.set_speed(SPEEDS[self.speed]);
        }
        cx.notify();
    }

    /// Moves the time along while playing; to the end when it ends.
    fn follow(&mut self, cx: &mut Context<Self>) {
        self._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let going = this
                    .update(cx, |this, cx| {
                        let Some(output) = &this.output else {
                            return false;
                        };
                        if output.finished() {
                            this.position = this.duration();
                            cx.notify();
                            return false;
                        }
                        this.position = output.position();
                        cx.notify();
                        output.is_playing()
                    })
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        }));
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Render for AudioPlayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wave = match &self.wave {
            None => return crate::notice("Reading the sound…", cx),
            Some(Err(err)) => return crate::notice(err.clone(), cx),
            Some(Ok(wave)) => wave.clone(),
        };
        let theme = cx.theme();
        let duration = wave.duration;
        let progress = if duration.is_zero() {
            0.
        } else {
            (self.position.as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.)
        };
        let (played, unplayed) = (theme.progress_bar, theme.muted_foreground.opacity(0.35));
        let peaks = wave.peaks.clone();
        let drawn = self.wave_bounds.clone();
        let bars = canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, _, window, _| {
                drawn.set(bounds);
                let count = peaks.len().max(1);
                let step = f32::from(bounds.size.width) / count as f32;
                let width = (step * 0.6).max(1.);
                let middle = f32::from(bounds.size.height) / 2.;
                for (ix, peak) in peaks.iter().enumerate() {
                    let height = (peak * f32::from(bounds.size.height)).max(2.);
                    let x = f32::from(bounds.origin.x) + ix as f32 * step;
                    let y = f32::from(bounds.origin.y) + middle - height / 2.;
                    let color = if (ix as f32 + 0.5) / count as f32 <= progress {
                        played
                    } else {
                        unplayed
                    };
                    window.paint_quad(
                        fill(
                            Bounds::new(point(px(x), px(y)), size(px(width), px(height))),
                            color,
                        )
                        .corner_radii(px(width / 2.)),
                    );
                }
            },
        )
        .size_full();
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let details = format!(
            "{} · {} Hz · {} · {}",
            if wave.channels == 1 { "Mono" } else { "Stereo" },
            wave.sample_rate,
            clock(duration),
            byte_size(self.size)
        );
        let playing = self.is_playing();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.background)
            .p_6()
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(720.))
                    .gap_4()
                    .p_6()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary.opacity(0.4))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(details),
                            ),
                    )
                    .child(
                        div()
                            .id("audio-wave")
                            .h(px(88.))
                            .w_full()
                            .cursor_pointer()
                            .child(bars)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    let bounds = this.wave_bounds.get();
                                    let width = f32::from(bounds.size.width);
                                    if width > 0. {
                                        let x = f32::from(event.position.x - bounds.origin.x);
                                        this.seek(x / width, cx);
                                    }
                                }),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Button::new("audio-play")
                                    .primary()
                                    .icon(if playing {
                                        IconName::Pause
                                    } else {
                                        IconName::Play
                                    })
                                    .rounded_full()
                                    .tooltip(if playing { "Pause" } else { "Play" })
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle(cx))),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_family(theme.mono_font_family.clone())
                                    .child(format!(
                                        "{} / {}",
                                        clock(self.position.min(duration)),
                                        clock(duration)
                                    )),
                            )
                            .child(div().flex_1())
                            .when_some(self.error.clone(), |this, error| {
                                this.child(div().text_xs().text_color(theme.danger).child(error))
                            })
                            .child(
                                Button::new("audio-speed")
                                    .ghost()
                                    .small()
                                    .label(format!("{}×", SPEEDS[self.speed]))
                                    .tooltip("Playback speed")
                                    .on_click(cx.listener(|this, _, _, cx| this.next_speed(cx))),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_reads_well() {
        assert_eq!(clock(Duration::from_secs(5)), "0:05");
        assert_eq!(clock(Duration::from_secs(65)), "1:05");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    fn waveform_of_a_wav() {
        // One second of a 440 Hz tone at 8 kHz, mono, 16-bit.
        let path = std::env::temp_dir().join(format!("ph-audio-{}.wav", std::process::id()));
        let rate = 8000u32;
        let samples: Vec<i16> = (0..rate)
            .map(|n| {
                ((n as f32 * 440. * std::f32::consts::TAU / rate as f32).sin() * 12000.) as i16
            })
            .collect();
        let mut bytes = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        let wave = waveform(&path).unwrap();
        assert_eq!(wave.channels, 1);
        assert_eq!(wave.sample_rate, rate);
        assert!((wave.duration.as_secs_f32() - 1.).abs() < 0.01);
        assert!((BARS * 9 / 10..=BARS).contains(&wave.peaks.len()));
        assert!(wave.peaks.iter().all(|peak| *peak <= 1.));
        assert!(waveform(Path::new("/nonexistent.wav")).is_err());
    }
}
