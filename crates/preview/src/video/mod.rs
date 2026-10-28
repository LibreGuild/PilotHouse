//! Video files, played through the system's own media framework, so no
//! codec ships with PilotHouse: AVFoundation on macOS, Media Foundation on
//! Windows, GStreamer on Linux (loaded when a video opens; without it, or
//! without a codec, the view says so and the file opens in the default
//! player). PilotHouse draws the frames itself, with its own controls.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux::Engine;
#[cfg(target_os = "macos")]
pub(crate) use macos::Engine;
#[cfg(windows)]
pub(crate) use windows::Engine;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::{
    AnyElement, Bounds, Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, ParentElement as _,
    Pixels, Render, RenderImage, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Task, Window, canvas, div, img, px,
};

use crate::audio::clock;

/// One picture of the video.
#[derive(Clone)]
pub enum Frame {
    /// A pixel buffer the GPU draws as it is (macOS).
    #[cfg(target_os = "macos")]
    Surface(gpui_kit::SurfaceSource),
    /// Pixels decoded on the CPU (Windows, Linux).
    #[allow(dead_code)]
    Image(Arc<RenderImage>),
}

/// How far a video has got with opening.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Loading,
    Ready,
    Failed(String),
}

const SPEEDS: [f32; 5] = [0.5, 1., 1.25, 1.5, 2.];
/// How often a video that is not playing is checked (loading, seeking).
const IDLE_TICK: Duration = Duration::from_millis(120);

pub struct VideoPlayer {
    path: PathBuf,
    engine: Option<Result<Engine, SharedString>>,
    frame: Option<Frame>,
    /// Frames decoded on the CPU are released on the next frame.
    old_images: Vec<Arc<RenderImage>>,
    speed: usize,
    muted: bool,
    /// Where the scrubber was drawn, for clicks and drags.
    bar: Rc<Cell<Bounds<Pixels>>>,
    dragging: bool,
    focus: FocusHandle,
    _tick: Option<Task<()>>,
}

impl Focusable for VideoPlayer {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl VideoPlayer {
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            engine: None,
            frame: None,
            old_images: Vec::new(),
            speed: 1,
            muted: false,
            bar: Rc::default(),
            dragging: false,
            focus: cx.focus_handle(),
            _tick: None,
        };
        this.reload(window, cx);
        // Closing the tab lets the last frames' textures go.
        cx.on_release(|this, cx| {
            let current = match this.frame.take() {
                Some(Frame::Image(image)) => Some(image),
                _ => None,
            };
            for image in this.old_images.drain(..).chain(current) {
                cx.drop_image(image, None);
            }
        })
        .detach();
        this
    }

    pub fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.engine = Some(Engine::open(&self.path).map_err(Into::into));
        self.frame = None;
        self.watch(cx);
        cx.notify();
    }

    fn engine(&self) -> Option<&Engine> {
        self.engine.as_ref().and_then(|engine| engine.as_ref().ok())
    }

    /// Checks on the video now and then while it is not playing (playing,
    /// each display frame does).
    fn watch(&mut self, cx: &mut Context<Self>) {
        self._tick = Some(cx.spawn(async move |this, cx| {
            let mut seen = None;
            loop {
                cx.background_executor().timer(IDLE_TICK).await;
                let alive = this
                    .update(cx, |this, cx| {
                        // Redrawn only when something changed: a frame, the
                        // state, the time.
                        let new_frame = this.pull_frame(cx);
                        let now = this.engine().map(|engine| {
                            (
                                engine.status(),
                                (engine.position() * 10.).round() as i64,
                                engine.is_playing(),
                            )
                        });
                        if new_frame || now != seen {
                            seen = now;
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    /// Takes the newest frame, if there is one; whether there was.
    fn pull_frame(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(frame) = self.engine().and_then(Engine::frame) else {
            return false;
        };
        if let Some(Frame::Image(old)) = self.frame.replace(frame) {
            self.old_images.push(old);
        }
        // A texture is let go once a newer frame has been drawn.
        if self.old_images.len() > 1 {
            for image in self.old_images.drain(..self.old_images.len() - 1) {
                cx.drop_image(image, None);
            }
        }
        true
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine() else {
            return;
        };
        if engine.is_playing() {
            engine.pause();
        } else {
            if let Some(duration) = engine.duration()
                && engine.position() >= duration - 0.05
            {
                engine.seek(0.);
            }
            engine.play(SPEEDS[self.speed]);
        }
        cx.notify();
    }

    pub fn pause(&mut self, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine() {
            engine.pause();
        }
        cx.notify();
    }

    fn seek_to(&mut self, x: Pixels) {
        let bounds = self.bar.get();
        let width = f32::from(bounds.size.width);
        let Some(engine) = self.engine() else {
            return;
        };
        if let (Some(duration), true) = (engine.duration(), width > 0.) {
            let fraction = (f32::from(x - bounds.origin.x) / width).clamp(0., 1.);
            engine.seek(duration * fraction as f64);
        }
    }

    fn step(&mut self, seconds: f64, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine() {
            engine.seek(engine.position() + seconds);
        }
        cx.notify();
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "space" | "k" => self.toggle(cx),
            "left" | "j" => self.step(-5., cx),
            "right" | "l" => self.step(5., cx),
            "m" => self.set_muted(!self.muted, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn set_muted(&mut self, muted: bool, cx: &mut Context<Self>) {
        self.muted = muted;
        if let Some(engine) = self.engine() {
            engine.set_muted(muted);
        }
        cx.notify();
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        // Stops the sound at once, not when the engine is finally freed.
        if let Some(engine) = self.engine() {
            engine.pause();
        }
    }
}

fn picture(frame: &Frame) -> AnyElement {
    match frame {
        #[cfg(target_os = "macos")]
        Frame::Surface(source) => gpui_kit::surface(source.clone())
            .object_fit(ObjectFit::Contain)
            .size_full()
            .into_any_element(),
        Frame::Image(image) => img(gpui_kit::ImageSource::Render(image.clone()))
            .object_fit(ObjectFit::Contain)
            .size_full()
            .into_any_element(),
    }
}

impl Render for VideoPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let engine = match &self.engine {
            None => return crate::notice("Opening the video…", cx),
            Some(Err(err)) => {
                return crate::notice(format!("{err} Use Open in Default App to watch it."), cx);
            }
            Some(Ok(engine)) => engine,
        };
        match engine.status() {
            Status::Failed(err) => {
                return crate::notice(
                    format!(
                        "This video cannot be played here: {err} Use Open in Default App to watch it."
                    ),
                    cx,
                );
            }
            Status::Loading => return crate::notice("Opening the video…", cx),
            Status::Ready => {}
        }
        let playing = engine.is_playing();
        let duration = engine.duration().unwrap_or(0.);
        let position = engine.position().min(duration.max(0.));
        let size = engine.size();
        if playing {
            _ = self.pull_frame(cx);
            window.request_animation_frame();
        }
        let progress = if duration > 0. {
            (position / duration) as f32
        } else {
            0.
        };
        let theme = cx.theme();
        let bar_bounds = self.bar.clone();
        let (played, rest) = (theme.progress_bar, gpui_kit::white().opacity(0.3));
        let scrubber = canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, _, window, _| {
                bar_bounds.set(bounds);
                let height = px(4.);
                let y = bounds.origin.y + (bounds.size.height - height) / 2.;
                let track = Bounds::new(
                    gpui_kit::point(bounds.origin.x, y),
                    gpui_kit::size(bounds.size.width, height),
                );
                window.paint_quad(gpui_kit::fill(track, rest).corner_radii(px(2.)));
                let done = Bounds::new(
                    track.origin,
                    gpui_kit::size(track.size.width * progress, height),
                );
                window.paint_quad(gpui_kit::fill(done, played).corner_radii(px(2.)));
                let knob = px(12.);
                let knob_bounds = Bounds::new(
                    gpui_kit::point(
                        track.origin.x + track.size.width * progress - knob / 2.,
                        bounds.origin.y + (bounds.size.height - knob) / 2.,
                    ),
                    gpui_kit::size(knob, knob),
                );
                window.paint_quad(
                    gpui_kit::fill(knob_bounds, gpui_kit::white()).corner_radii(knob / 2.),
                );
            },
        )
        .size_full();
        let picture = match (&self.frame, size) {
            (Some(frame), _) => picture(frame),
            (None, None) => crate::notice("Sound only.", cx),
            (None, Some(_)) => div().into_any_element(),
        };
        let info = size
            .map(|(width, height)| {
                format!(
                    "{width:.0} × {height:.0} · {}",
                    clock(Duration::from_secs_f64(duration))
                )
            })
            .unwrap_or_else(|| clock(Duration::from_secs_f64(duration)));
        v_flex()
            .id("video-player")
            .track_focus(&self.focus)
            .key_context("VideoPlayer")
            .on_key_down(cx.listener(Self::on_key))
            .on_click(cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)))
            .size_full()
            .bg(gpui_kit::black())
            .child(
                div()
                    .id("video-picture")
                    .flex_1()
                    .min_h_0()
                    .child(picture)
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.focus(&this.focus, cx);
                        this.toggle(cx);
                    })),
            )
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .px_3()
                    .gap_3()
                    .bg(gpui_kit::black().opacity(0.85))
                    .text_color(gpui_kit::white())
                    .text_sm()
                    .child(
                        Button::new("video-play")
                            .ghost()
                            .small()
                            .icon(if playing {
                                IconName::Pause
                            } else {
                                IconName::Play
                            })
                            .tooltip(if playing {
                                "Pause (Space)"
                            } else {
                                "Play (Space)"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .child(format!(
                                "{} / {}",
                                clock(Duration::from_secs_f64(position)),
                                clock(Duration::from_secs_f64(duration))
                            )),
                    )
                    .child(
                        div()
                            .id("video-scrubber")
                            .flex_1()
                            .h_full()
                            .cursor_pointer()
                            .child(scrubber)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.dragging = true;
                                    this.seek_to(event.position.x);
                                    cx.notify();
                                }),
                            )
                            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                                if this.dragging && event.dragging() {
                                    this.seek_to(event.position.x);
                                    cx.notify();
                                } else {
                                    this.dragging = false;
                                }
                            }))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, _| this.dragging = false),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(gpui_kit::white().opacity(0.6))
                            .child(info),
                    )
                    .child(
                        Button::new("video-speed")
                            .ghost()
                            .xsmall()
                            .label(format!("{}×", SPEEDS[self.speed]))
                            .tooltip("Playback speed")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.speed = (this.speed + 1) % SPEEDS.len();
                                if let Some(engine) = this.engine() {
                                    engine.set_rate(SPEEDS[this.speed]);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("video-mute")
                            .ghost()
                            .xsmall()
                            .icon(if self.muted {
                                ph_ui::Icons::VolumeOff
                            } else {
                                ph_ui::Icons::Volume2
                            })
                            .selected(self.muted)
                            .tooltip(if self.muted { "Unmute (M)" } else { "Mute (M)" })
                            .on_click(
                                cx.listener(|this, _, _, cx| this.set_muted(!this.muted, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }
}
