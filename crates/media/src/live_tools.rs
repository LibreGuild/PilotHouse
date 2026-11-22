//! The tools of a live pane: pick an element, mark the picture, capture it,
//! record it. Shared by the browser and device panes, which put them in
//! their own toolbar, beside what else they have there.
//!
//! They used to float over the picture, at its bottom until dragged away,
//! and so always hid a part of what one was looking at (a phone's
//! navigation bar, a page's footer). In the pane's toolbar they hide
//! nothing and need no placing.
//!
//! In a pane too narrow for them beside what else its toolbar has, they
//! fold ([`LiveTools::set_folded`]): the pane lists them in its own menu
//! ([`LiveTools::menu`]), and what stays in the row is only what is going
//! on (a mode to leave, marks to clear, a recording to stop).
//!
//! The four kinds of mark (box, arrow, freehand, numbered pin) are one
//! button, which draws the kind used last, with a menu beside it for
//! another. A mode is left by its button again or by Escape; a pick ends
//! itself, and so does marking once the picture is captured. This owns the
//! recording; captures are saved to the data directory and handed to the
//! owner as [`ToChat::Captured`], which the project forwards to chat.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Task, Window,
};
use ph_ui::Icons;
use ph_ui::MenuButton as _;

use crate::annotate::Tool;
use crate::capture;
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
    /// Screenshots or recording frames, with a note.
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
    busy: bool,
    /// The kind of mark the Mark button draws: the one used last.
    mark: Tool,
    /// The pane has no room for the tools in its row: they are in its
    /// menu, and the row shows only what is going on.
    folded: bool,
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
            busy: false,
            mark: Tool::Rect,
            folded: false,
            _tick: None,
        }
    }

    pub fn set_source(&mut self, source: impl Into<String>) {
        self.source = source.into();
    }

    /// Whether the tools are in the pane's menu instead of its row.
    pub fn set_folded(&mut self, folded: bool, cx: &mut Context<Self>) {
        if self.folded != folded {
            self.folded = folded;
            cx.notify();
        }
    }

    /// About how wide the tools are in a row, in pixels at the default
    /// interface size: what a pane asks before it gives them the room.
    pub fn width(&self, folded: bool, cx: &App) -> f32 {
        const BUTTON: f32 = 26.;
        let surface = self.surface.read(cx);
        let mode = surface.mode();
        let clear = surface.has_annotations() || surface.has_highlight();
        let undo = surface.has_annotations();
        let recording = if self.recorder.is_some() { 36. } else { 0. };
        if folded {
            let shown = [
                mode != SurfaceMode::Interact,
                clear,
                self.recorder.is_some(),
            ];
            return shown.into_iter().filter(|shown| *shown).count() as f32 * BUTTON + recording;
        }
        let buttons = usize::from(self.can_pick) + 3 + usize::from(undo) + usize::from(clear);
        // The chooser of the kind of mark is a narrower button.
        buttons as f32 * BUTTON + 20. + recording
    }

    /// The tools as the items of a pane's menu, for when they are folded.
    pub fn menu(
        this: &Entity<Self>,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let tools = this.read(cx);
        let surface = tools.surface.read(cx);
        let idle = surface.frame().is_none() || tools.busy;
        let recording = tools.recorder.is_some();
        let (can_pick, current) = (tools.can_pick, tools.mark);
        let pick = this.downgrade();
        let menu = menu.when(can_pick, |menu| {
            menu.item(
                PopupMenuItem::new("Pick an element")
                    .icon(Icons::Crosshair)
                    .on_click(move |_, _, cx| {
                        _ = pick.update(cx, |this, cx| this.set_mode(SurfaceMode::Pick, cx));
                    }),
            )
        });
        let marks = this.downgrade();
        let kind = gpui_kit::component::Icon::new(mark_kind(current).0);
        let menu = menu.submenu_with_icon(
            Some(kind),
            "Mark the picture",
            window,
            cx,
            move |menu, _, _| {
                [Tool::Rect, Tool::Arrow, Tool::Pen, Tool::Pin]
                    .into_iter()
                    .fold(menu, |menu, tool| {
                        let this = marks.clone();
                        let (icon, name) = mark_kind(tool);
                        menu.item(
                            PopupMenuItem::new(name)
                                .icon(icon)
                                .checked(tool == current)
                                .on_click(move |_, _, cx| {
                                    _ = this.update(cx, |this, cx| this.mark_with(tool, cx));
                                }),
                        )
                    })
            },
        );
        let (capture, record) = (this.downgrade(), this.downgrade());
        menu.item(
            PopupMenuItem::new("Send a screenshot to chat")
                .icon(Icons::Camera)
                .disabled(idle)
                .on_click(move |_, _, cx| {
                    _ = capture.update(cx, |this, cx| this.capture(cx));
                }),
        )
        .item(
            PopupMenuItem::new(if recording {
                "Stop and send the recording"
            } else {
                "Record the screen"
            })
            .icon(if recording {
                Icons::CircleStop
            } else {
                Icons::Video
            })
            .disabled(idle)
            .on_click(move |_, _, cx| {
                _ = record.update(cx, |this, cx| this.toggle_recording(cx));
            }),
        )
        .separator()
    }

    /// Feeds the recorder; call for every frame the pane shows.
    pub fn push_frame(&mut self, frame: &Frame) {
        if let Some(recorder) = &mut self.recorder {
            recorder.push(frame);
        }
    }

    /// Saves the current frame, with annotations, and sends it to chat.
    pub fn capture(&mut self, cx: &mut Context<Self>) {
        let surface = self.surface.read(cx);
        let Some(frame) = surface.frame().cloned() else {
            return;
        };
        let marks = surface.marks();
        let annotated = !marks.is_empty();
        let stem = file_stem(&self.source);
        let label = format!("{} screenshot", self.source);
        // The marks were for this picture: with it taken they go, and the
        // pointer is the app's again.
        self.surface.update(cx, |surface, cx| {
            surface.clear_annotations(cx);
            surface.set_mode(SurfaceMode::Interact, cx);
        });
        self.save(cx, move || {
            // Burning the marks in copies the whole picture: done here, off
            // the UI thread, with the encoding.
            let frame = if annotated {
                crate::annotate::render(&frame, &marks)
            } else {
                frame
            };
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

impl LiveTools {
    /// Marks with `tool`, which the Mark button then draws.
    fn mark_with(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.mark = tool;
        self.surface.update(cx, |surface, cx| {
            surface.set_mode(SurfaceMode::Annotate(tool), cx)
        });
        cx.notify();
    }
}

/// A kind of mark's icon and name.
fn mark_kind(tool: Tool) -> (Icons, &'static str) {
    match tool {
        Tool::Rect => (Icons::Square, "Box"),
        Tool::Arrow => (Icons::MoveUpRight, "Arrow"),
        Tool::Pen => (Icons::Pencil, "Freehand"),
        Tool::Pin => (Icons::MapPin, "Numbered pin"),
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
        // The outline of a picked element is cleared as the marks are.
        let can_clear = has_marks || surface.has_highlight();
        let marking = matches!(mode, SurfaceMode::Annotate(_));
        let folded = self.folded;
        let recording = self
            .recorder
            .as_ref()
            .map(|recorder| recorder.elapsed().as_secs());
        let (mark_icon, mark_name) = mark_kind(self.mark);
        let mark = self.mark;
        let this = cx.entity().downgrade();
        h_flex()
            .id("live-tools")
            .flex_none()
            .gap_0p5()
            .when(
                self.can_pick && (!folded || mode == SurfaceMode::Pick),
                |this| {
                    this.child(
                        Button::new("tool-pick")
                            .ghost()
                            .small()
                            .icon(Icons::Crosshair)
                            .selected(mode == SurfaceMode::Pick)
                            .tooltip(ph_ui::tip(if mode == SurfaceMode::Pick {
                                "Stop picking (Esc)"
                            } else {
                                "Pick an element: the next click sends what is under it to the chat"
                            }))
                            .accessibility_label("Pick an element")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.set_mode(SurfaceMode::Pick, cx)),
                            ),
                    )
                },
            )
            .when(!folded || marking, |row| {
                row.child(
                    // The two halves of one button: draw, and which kind.
                    h_flex()
                        .child(
                            Button::new("tool-mark")
                                .ghost()
                                .small()
                                .icon(mark_icon)
                                .selected(marking)
                                .tooltip(ph_ui::tip(if marking {
                                    "Stop marking (Esc)".to_string()
                                } else {
                                    format!(
                                        "Mark the picture ({}): marks go with the screenshot",
                                        mark_name.to_lowercase()
                                    )
                                }))
                                .accessibility_label("Mark the picture")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_mode(SurfaceMode::Annotate(mark), cx)
                                })),
                        )
                        .when(!folded, |row| {
                            row.child(
                                Button::new("tool-mark-kind")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ChevronDown)
                                    .tooltip("The kind of mark")
                                    .accessibility_label("The kind of mark")
                                    .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                                        [Tool::Rect, Tool::Arrow, Tool::Pen, Tool::Pin]
                                            .into_iter()
                                            .fold(menu, |menu, tool| {
                                                let this = this.clone();
                                                let (icon, name) = mark_kind(tool);
                                                menu.item(
                                                    PopupMenuItem::new(name)
                                                        .icon(icon)
                                                        .checked(tool == mark)
                                                        .on_click(move |_, _, cx| {
                                                            _ = this.update(cx, |this, cx| {
                                                                this.mark_with(tool, cx)
                                                            });
                                                        }),
                                                )
                                            })
                                    }),
                            )
                        }),
                )
            })
            .when(has_marks && !folded, |this| {
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
            })
            .when(can_clear, |this| {
                this.child(
                    Button::new("tool-clear")
                        .ghost()
                        .small()
                        .icon(Icons::Eraser)
                        .tooltip(if has_marks {
                            "Clear all marks"
                        } else {
                            "Clear the outline"
                        })
                        .accessibility_label("Clear all marks")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.surface
                                .update(cx, |surface, cx| surface.clear_annotations(cx))
                        })),
                )
            })
            .when(!folded, |this| {
                this.child(
                    Button::new("tool-capture")
                        .ghost()
                        .small()
                        .icon(Icons::Camera)
                        .disabled(!has_frame || self.busy)
                        .tooltip(ph_ui::tip(format!(
                            "Send a screenshot to chat ({})",
                            ph_ui::platform::shortcut("secondary-shift-s")
                        )))
                        .accessibility_label("Send a screenshot to chat")
                        .on_click(cx.listener(|this, _, _, cx| this.capture(cx))),
                )
            })
            .when(!folded || recording.is_some(), |this| {
                this.child(
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
            })
    }
}
