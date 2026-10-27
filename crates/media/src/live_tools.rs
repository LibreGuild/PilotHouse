//! The floating toolbar on a live pane: annotate, capture, record, compare.
//!
//! Shared by the browser and device panes, which lay it over the whole live
//! area: it sits at the bottom centre until dragged by its grip somewhere
//! that hides less (double-click the grip to put it back), and folds to a
//! single button. It owns the recording and the
//! "before" frame; captures are saved to the data directory and handed to the
//! owner as [`LiveToolsEvent::Captured`], which the project forwards to chat.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, DragMoveEvent, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Point, Render, StatefulInteractiveElement as _,
    Styled as _, Task, Window, div, point, px, relative,
};
use ph_ui::Icons;

use crate::annotate::Tool;
use crate::capture;
use crate::compare::compare;
use crate::frame::Frame;
use crate::recorder::Recorder;
use crate::surface::{SurfaceMode, SurfaceView};

/// Keyframes sent to the agent from a recording.
const RECORDING_KEYFRAMES: usize = 4;

#[derive(Debug, Clone)]
pub struct CapturedImage {
    pub label: String,
    pub path: PathBuf,
}

/// What a live pane hands to the chat. The browser and device panes emit
/// this; the project adds it to the composer.
#[derive(Debug, Clone)]
pub enum ToChat {
    /// Screenshots, recording frames or a before/after set, with a note.
    Captured {
        images: Vec<CapturedImage>,
        note: Option<String>,
    },
    /// A picked element, log lines…
    Text { label: String, text: String },
}

pub struct LiveTools {
    surface: Entity<SurfaceView>,
    /// Names captures: "browser", "pixel-8"…
    source: String,
    /// The pane supports the element picker.
    can_pick: bool,
    recorder: Option<Recorder>,
    baseline: Option<Frame>,
    busy: bool,
    /// Where the user dragged the bar, as fractions of the live area (top
    /// left corner); `None` for the bottom centre.
    placed: Option<(f32, f32)>,
    /// Where on the grip the drag started.
    grab: Point<Pixels>,
    /// Folded to one button.
    folded: bool,
    /// The bar's size as last drawn, to keep it whole inside the area.
    bar_size: gpui_kit::Size<Pixels>,
    /// Redraws the recording timer once a second while recording.
    _tick: Option<Task<()>>,
}

impl EventEmitter<ToChat> for LiveTools {}

impl LiveTools {
    pub fn new(
        surface: Entity<SurfaceView>,
        source: impl Into<String>,
        can_pick: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&surface, |_, _, cx| cx.notify()).detach();
        Self {
            surface,
            source: source.into(),
            can_pick,
            recorder: None,
            baseline: None,
            busy: false,
            placed: None,
            grab: Point::default(),
            folded: false,
            bar_size: gpui_kit::size(px(96.), px(44.)),
            _tick: None,
        }
    }

    pub fn set_source(&mut self, source: impl Into<String>) {
        self.source = source.into();
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Feeds the recorder; call for every frame the pane shows.
    pub fn push_frame(&mut self, frame: &Frame) {
        if let Some(recorder) = &mut self.recorder {
            recorder.push(frame);
        }
    }

    /// Saves the current frame, with annotations, and sends it to chat.
    pub fn capture(&mut self, cx: &mut Context<Self>) {
        let Some(frame) = self.surface.read(cx).composite() else {
            return;
        };
        let annotated = self.surface.read(cx).has_annotations();
        let stem = file_stem(&self.source);
        let label = format!("{} screenshot", self.source);
        self.surface
            .update(cx, |surface, cx| surface.clear_annotations(cx));
        self.save(cx, move || {
            let path = capture::save_png(&frame, &stem)?;
            let note = annotated.then(|| {
                "The red marks on the screenshot are mine: boxes, arrows and numbered pins point at what I mean.".to_string()
            });
            Ok((vec![CapturedImage { label, path }], note))
        });
    }

    pub fn toggle_recording(&mut self, cx: &mut Context<Self>) {
        let Some(recorder) = self.recorder.take() else {
            let mut recorder = match Recorder::start(&capture::recordings_dir()) {
                Ok(recorder) => recorder,
                Err(err) => {
                    tracing::warn!("starting a recording failed: {err:#}");
                    return;
                }
            };
            if let Some(frame) = self.surface.read(cx).frame() {
                recorder.push(frame);
            }
            self.recorder = Some(recorder);
            self._tick = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        return;
                    }
                }
            }));
            cx.notify();
            return;
        };
        self._tick = None;
        let stem = file_stem(&self.source);
        let source = self.source.clone();
        self.save(cx, move || {
            let recording = recorder.finish(RECORDING_KEYFRAMES)?;
            let count = recording.keyframes.len();
            let mut images = Vec::new();
            for (ix, frame) in recording.keyframes.iter().enumerate() {
                images.push(CapturedImage {
                    label: format!("{source} recording {}/{count}", ix + 1),
                    path: capture::save_png(frame, &format!("{stem}-rec{}", ix + 1))?,
                });
            }
            let note = format!(
                "A {}s screen recording of the {source}, as {count} frames in order (first to last). The full recording is at {}.",
                recording.duration.as_secs(),
                recording.gif.display()
            );
            Ok((images, Some(note)))
        });
    }

    /// First press keeps the current frame as "before"; the second compares
    /// it with the current frame and sends before, after and a diff.
    pub fn compare(&mut self, cx: &mut Context<Self>) {
        let Some(after) = self.surface.read(cx).frame().cloned() else {
            return;
        };
        let Some(before) = self.baseline.take() else {
            self.baseline = Some(after);
            cx.notify();
            return;
        };
        let stem = file_stem(&self.source);
        let source = self.source.clone();
        self.save(cx, move || {
            let result = compare(&before, &after);
            let note = match result.region {
                None => "Before and after look the same.".to_string(),
                Some((x, y, w, h)) => format!(
                    "Before/after of the {source}: {:.1}% of pixels changed, mostly in the area at x {x}, y {y}, {w}×{h} px. The third image shows the changes in red.",
                    result.changed * 100.0
                ),
            };
            Ok((
                vec![
                    CapturedImage { label: format!("{source} before"), path: capture::save_png(&before, &format!("{stem}-before"))? },
                    CapturedImage { label: format!("{source} after"), path: capture::save_png(&after, &format!("{stem}-after"))? },
                    CapturedImage { label: format!("{source} changes"), path: capture::save_png(&result.highlight, &format!("{stem}-diff"))? },
                ],
                Some(note),
            ))
        });
    }

    /// Runs `work` off the UI thread and emits its images.
    fn save(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> anyhow::Result<(Vec<CapturedImage>, Option<String>)> + Send + 'static,
    ) {
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { work() }).await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((images, note)) => cx.emit(ToChat::Captured { images, note }),
                    Err(err) => tracing::warn!("capture failed: {err:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_mode(&mut self, mode: SurfaceMode, cx: &mut Context<Self>) {
        self.surface.update(cx, |surface, cx| {
            let mode = if surface.mode() == mode {
                SurfaceMode::Interact
            } else {
                mode
            };
            surface.set_mode(mode, cx);
        });
    }
}

/// What is dragged when the bar moves: whose bar (every live area hears
/// every drag of this kind).
struct MoveBar(gpui_kit::EntityId);

/// Nothing follows the pointer: the bar itself moves.
struct NoGhost;

impl Render for NoGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// How far in from the bar's corner the grip is (the bar's padding and
/// border).
const EDGE: f32 = 5.0;

impl LiveTools {
    fn move_bar(&mut self, event: &DragMoveEvent<MoveBar>, cx: &mut Context<Self>) {
        if event.drag(cx).0 != cx.entity_id() {
            return;
        }
        let area = event.bounds;
        let (width, height) = (f32::from(area.size.width), f32::from(area.size.height));
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        // The grip sits just inside the bar's left edge.
        let corner = event.event.position - area.origin - self.grab - point(px(EDGE), px(EDGE));
        // The whole bar stays inside, as wide as it is drawn.
        let (bar_width, bar_height) = (
            f32::from(self.bar_size.width),
            f32::from(self.bar_size.height),
        );
        let x = f32::from(corner.x).clamp(0.0, (width - bar_width).max(0.0));
        let y = f32::from(corner.y).clamp(0.0, (height - bar_height).max(0.0));
        self.placed = Some((x / width, y / height));
        cx.notify();
    }

    fn grip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let owner = cx.entity_id();
        div()
            .id("tool-grip")
            .flex()
            .items_center()
            .justify_center()
            .w_4()
            .h_6()
            .rounded_full()
            .cursor_grab()
            .text_color(cx.theme().muted_foreground)
            .hover(|style| style.bg(cx.theme().secondary_hover))
            .child(gpui_kit::component::Icon::new(Icons::GripVertical).xsmall())
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Drag to move; double-click to put back")
                    .build(window, cx)
            })
            .on_drag(MoveBar(owner), move |_, offset, _, cx| {
                _ = this.update(cx, |this, _| this.grab = offset);
                cx.new(|_| NoGhost)
            })
            .on_click(cx.listener(|this, event: &gpui_kit::ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.placed = None;
                    cx.notify();
                }
            }))
    }
}

fn file_stem(source: &str) -> String {
    let stem: String = source
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    stem.trim_matches('-').to_string()
}

impl Render for LiveTools {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let surface = self.surface.read(cx);
        let mode = surface.mode();
        let has_frame = surface.frame().is_some();
        let has_marks = surface.has_annotations();

        let mode_button = |id: &'static str,
                           icon: Icons,
                           tooltip: &'static str,
                           target: SurfaceMode,
                           cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .selected(mode == target)
                .tooltip(tooltip)
                .accessibility_label(tooltip)
                .on_click(cx.listener(move |this, _, _, cx| this.set_mode(target, cx)))
        };
        let border = cx.theme().border;
        let divider = move || div().w_px().h_4().mx_0p5().bg(border);
        let recording = self
            .recorder
            .as_ref()
            .map(|recorder| recorder.elapsed().as_secs());

        let fold = Button::new("tool-fold")
            .ghost()
            .small()
            .icon(if self.folded {
                Icons::Pencil
            } else {
                Icons::ChevronsLeft
            })
            .tooltip(if self.folded {
                "Show the markup tools"
            } else {
                "Fold the markup tools"
            })
            .accessibility_label(if self.folded {
                "Show the markup tools"
            } else {
                "Fold the markup tools"
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.folded = !this.folded;
                // A mode left on would keep taking the page's clicks with
                // its button hidden.
                if this.folded {
                    this.surface.update(cx, |surface, cx| {
                        surface.set_mode(SurfaceMode::Interact, cx)
                    });
                }
                cx.notify();
            }));
        let this = cx.entity().downgrade();
        let bar = h_flex()
            .id("live-tools")
            .relative()
            // Its size, for keeping it inside the area while it is dragged.
            .child(
                gpui_kit::canvas(
                    move |bounds, _, cx| {
                        _ = this.update(cx, |this, _| this.bar_size = bounds.size);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            // Clicks on the toolbar must not reach the live page under it.
            .occlude()
            .gap_0p5()
            .p_1()
            .rounded_full()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .child(self.grip(cx));
        let bar = if self.folded {
            bar.child(fold).when_some(recording, |this, seconds| {
                this.child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(format!("● {}:{:02}", seconds / 60, seconds % 60)),
                )
            })
        } else {
            bar.child(mode_button(
                "tool-interact",
                Icons::MousePointer2,
                "Use the app",
                SurfaceMode::Interact,
                cx,
            ))
            .when(self.can_pick, |this| {
                this.child(mode_button(
                    "tool-pick",
                    Icons::Crosshair,
                    "Pick an element",
                    SurfaceMode::Pick,
                    cx,
                ))
            })
            .child(divider())
            .child(mode_button(
                "tool-rect",
                Icons::Square,
                "Draw a box",
                SurfaceMode::Annotate(Tool::Rect),
                cx,
            ))
            .child(mode_button(
                "tool-arrow",
                Icons::MoveUpRight,
                "Draw an arrow",
                SurfaceMode::Annotate(Tool::Arrow),
                cx,
            ))
            .child(mode_button(
                "tool-pen",
                Icons::Pencil,
                "Draw freehand",
                SurfaceMode::Annotate(Tool::Pen),
                cx,
            ))
            .child(mode_button(
                "tool-pin",
                Icons::MapPin,
                "Drop a numbered pin",
                SurfaceMode::Annotate(Tool::Pin),
                cx,
            ))
            .when(has_marks, |this| {
                this.child(
                    Button::new("tool-undo")
                        .ghost()
                        .small()
                        .icon(Icons::Undo2)
                        .tooltip("Undo the last mark")
                        .accessibility_label("Undo the last mark")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.surface
                                .update(cx, |surface, cx| surface.undo_annotation(cx))
                        })),
                )
                .child(
                    Button::new("tool-clear")
                        .ghost()
                        .small()
                        .icon(Icons::Eraser)
                        .tooltip("Clear all marks")
                        .accessibility_label("Clear all marks")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.surface
                                .update(cx, |surface, cx| surface.clear_annotations(cx))
                        })),
                )
            })
            .child(divider())
            .child(
                Button::new("tool-capture")
                    .ghost()
                    .small()
                    .icon(Icons::Camera)
                    .disabled(!has_frame || self.busy)
                    .tooltip(format!(
                        "Send a screenshot to chat ({})",
                        ph_ui::platform::shortcut("secondary-shift-s")
                    ))
                    .accessibility_label("Send a screenshot to chat")
                    .on_click(cx.listener(|this, _, _, cx| this.capture(cx))),
            )
            .child(
                Button::new("tool-record")
                    .ghost()
                    .small()
                    .icon(if recording.is_some() {
                        Icons::CircleStop
                    } else {
                        Icons::Video
                    })
                    .when_some(recording, |this, seconds| {
                        this.label(format!("{}:{:02}", seconds / 60, seconds % 60))
                            .text_color(cx.theme().danger)
                    })
                    .disabled(!has_frame || self.busy)
                    .tooltip(if recording.is_some() {
                        "Stop and send the recording"
                    } else {
                        "Record the screen"
                    })
                    .accessibility_label("Record the screen")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_recording(cx))),
            )
            .child(
                Button::new("tool-compare")
                    .ghost()
                    .small()
                    .icon(Icons::GitCompareArrows)
                    .selected(self.baseline.is_some())
                    .when(self.baseline.is_some(), |this| this.label("Compare"))
                    .disabled(!has_frame || self.busy)
                    .tooltip(if self.baseline.is_some() {
                        "Compare with the saved “before” and send both"
                    } else {
                        "Keep this as “before”, to compare after a change"
                    })
                    .accessibility_label("Before and after")
                    .on_click(cx.listener(|this, _, _, cx| this.compare(cx))),
            )
            .child(divider())
            .child(fold)
        };
        // A layer over the live area that only listens for the bar's drag,
        // so the page under it stays usable.
        let layer = div()
            .id("live-tools-layer")
            .size_full()
            .relative()
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<MoveBar>, _, cx| this.move_bar(event, cx)),
            );
        match self.placed {
            Some((x, y)) => layer.child(
                div()
                    .absolute()
                    .left(relative(x))
                    .top(relative(y))
                    .child(bar),
            ),
            None => layer.child(
                h_flex()
                    .absolute()
                    .bottom_3()
                    .left_0()
                    .right_0()
                    .justify_center()
                    .child(bar),
            ),
        }
    }
}
