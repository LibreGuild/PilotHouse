//! Studio's window: a picture or video being edited, with a chat beside it
//! whose agent changes it as the user describes (the `studio_*` tools).
//!
//! One window per file: opening it again brings its window forward. The
//! chat is the workspace's (its agents, its history); without a workspace
//! (a file opened from outside every project) there is none.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, DragMoveEvent, Entity, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Size,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowBounds,
    WindowOptions, div, px,
};
use ph_agent::{ChatContext, ChatPanel, ChatPanelEvent, Instructions, Starter};
use ph_store::Store;
use ph_store::projects::Project;
use ph_studio::video::VideoEditor;
use ph_studio::video::document::VideoDocument;
use ph_studio::{ImageDocument, ImageEditor};
use ph_theme::layout;
use ph_ui::Icons;

/// Open Studio windows, by file.
#[derive(Default)]
struct StudioWindows {
    open: HashMap<PathBuf, AnyWindowHandle>,
    /// Their views, to ask about unsaved work before the app quits.
    views: HashMap<PathBuf, gpui_kit::WeakEntity<StudioWindow>>,
    /// Whether the chat shows in new windows, and how wide: as it was
    /// last left.
    chat_shown: Option<bool>,
    chat_width: Option<f32>,
}

/// The chat's width at first, and its least.
const CHAT_WIDTH: f32 = 340.;
const CHAT_MIN: f32 = 260.;
/// What the picture keeps when the chat is dragged wider.
const EDITOR_MIN: f32 = 420.;
/// How long the chat takes to slide in or out.
const SLIDE: std::time::Duration = std::time::Duration::from_millis(200);

/// The chat's edge being dragged.
#[derive(Clone)]
struct ChatEdge;

impl Render for ChatEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

/// The chat sliding from one width to another.
#[derive(Clone, Copy)]
struct Slide {
    start: std::time::Instant,
    from: f32,
    to: f32,
}

impl Slide {
    /// The width now, eased out, and whether it is still moving.
    fn now(&self) -> (f32, bool) {
        let t = (self.start.elapsed().as_secs_f32() / SLIDE.as_secs_f32()).min(1.);
        let eased = 1. - (1. - t).powi(3);
        (self.from + (self.to - self.from) * eased, t < 1.)
    }
}

impl Global for StudioWindows {}

/// Opens `path` (a picture) in Studio, or brings its window forward. With
/// a workspace (`store`, `project`), the window has its chat.
pub fn open(path: PathBuf, workspace: Option<(Store, Project)>, cx: &mut App) -> Result<()> {
    let path = dunce::canonicalize(&path).unwrap_or(path);
    let existing = cx
        .try_global::<StudioWindows>()
        .and_then(|windows| windows.open.get(&path).copied());
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    open_window_for(path, workspace, cx)
}

fn open_window_for(path: PathBuf, workspace: Option<(Store, Project)>, cx: &mut App) -> Result<()> {
    let screen = cx
        .primary_display()
        .map(|display| display.bounds().size)
        .unwrap_or(Size::new(px(1440.), px(900.)));
    let size = Size::new(
        px(1440.).min(screen.width * 0.94),
        px(900.).min(screen.height * 0.92),
    );
    let mut options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
        window_min_size: Some(Size::new(px(900.), px(580.))),
        ..TitleBar::window_options()
    };
    if let Some(titlebar) = options.titlebar.as_mut() {
        let (x, y) = layout::TRAFFIC_LIGHTS;
        titlebar.traffic_light_position = Some(gpui_kit::point(x, y));
    }
    let (handle, view) = gpui_kit::open_window(options, cx, |window, cx| {
        ph_theme::apply_appearance(&crate::AppState::appearance(cx), Some(window), cx);
        let path = path.clone();
        cx.new(|cx| StudioWindow::new(path, workspace, window, cx))
    })
    .context("opening Studio")?;
    let windows = cx.default_global::<StudioWindows>();
    windows.views.insert(path.clone(), view.downgrade());
    windows.open.insert(path, handle);
    Ok(())
}

/// Before the app quits: each Studio window asks about edits that are not
/// saved (as closing it does). `false` when one
/// asked: the app stays, and the window closes if the user says so.
pub fn ready_to_quit(cx: &mut App) -> bool {
    let windows: Vec<(AnyWindowHandle, gpui_kit::WeakEntity<StudioWindow>)> = cx
        .try_global::<StudioWindows>()
        .map(|windows| {
            windows
                .open
                .iter()
                .filter_map(|(path, handle)| Some((*handle, windows.views.get(path)?.clone())))
                .collect()
        })
        .unwrap_or_default();
    let mut ready = true;
    for (handle, view) in windows {
        let closes = handle
            .update(cx, |_, window, cx| {
                view.update(cx, |view, cx| view.should_close(window, cx))
                    .unwrap_or(true)
            })
            .unwrap_or(true);
        ready &= closes;
    }
    ready
}

/// The chat a Studio window has: its space and what it is told.
struct ChatSetup {
    store: Store,
    project: Project,
    title: String,
    subtitle: String,
    placeholder: String,
    cwd: PathBuf,
    instructions: Instructions,
    starters: &'static [(&'static str, &'static str, &'static str, &'static str); 4],
}

/// What the agent is told with each message in Studio's chat.
fn instructions(path: &std::path::Path, video: bool) -> String {
    let (what, look) = if video {
        (
            "video",
            "studio_get to look at it (with `at` for the moments to see)",
        )
    } else {
        ("picture", "studio_get to look at it")
    };
    format!(
        "[{app} Studio] The user is editing the {what} {} in Studio, beside this chat. Change it only with the studio_* tools: {look}, studio_edit to change it (each change shows to the user at once and can be undone). Save with studio_export only when the user asks; never edit the file by other means.",
        path.display(),
        app = ph_brand::NAME,
    )
}

/// The empty chat's suggestions for a video: id, title, hint, prompt.
const VIDEO_STARTERS: [(&str, &str, &str, &str); 4] = [
    (
        "tighten",
        "Tighten it",
        "Cut or speed up the slow parts",
        "Make this recording tighter: find the parts where nothing happens (waiting, loading, typing) and speed them up or cut them, keeping every step a viewer needs. Tell me what you changed.",
    ),
    (
        "explain",
        "Explain the steps",
        "Labels and arrows as it plays",
        "Add short labels and arrows that explain each step as it happens, each shown while it is on screen. Ask me what to point out if it is unclear.",
    ),
    (
        "private",
        "Hide private details",
        "Blur emails, names, tokens",
        "Look through the video for anything private on screen (emails, names, tokens, keys, phone numbers) and blur it for as long as it shows. Tell me what you hid.",
    ),
    (
        "smaller",
        "Fit an upload limit",
        "Under 50 MB for Confluence",
        "Set this video up to export under 50 MB (for Confluence) while staying readable, then tell me the size it will have. Export only when I say.",
    ),
];

/// The empty chat's suggestions for a picture: id, title, hint, prompt.
const STARTERS: [(&str, &str, &str, &str); 4] = [
    (
        "docs",
        "Ready for docs",
        "Crop, frame and keep it sharp",
        "Make this picture ready for documentation: crop it to what matters, put it in a frame, and keep text sharp. Show me the result before saving anything.",
    ),
    (
        "private",
        "Hide private details",
        "Blur emails, names, tokens",
        "Look for anything private in this picture (emails, names, tokens, keys, phone numbers, faces) and blur it. Tell me what you hid.",
    ),
    (
        "explain",
        "Point out what matters",
        "Arrows, steps and labels",
        "Mark the important parts of this picture with arrows, numbered steps and short labels so a reader can follow it. Ask me what to point out if it is unclear.",
    ),
    (
        "smaller",
        "Make it smaller",
        "Fit an upload limit",
        "Make this picture small enough to upload, under 500 KB unless I say otherwise, keeping it readable. Tell me the size and what changed.",
    ),
];

/// What Studio shows: a picture's editor or a video's.
enum Content {
    Image(Entity<ImageEditor>),
    Video(Entity<VideoEditor>),
}

impl Content {
    /// Whether closing would lose changes.
    fn is_dirty(&self, cx: &App) -> bool {
        match self {
            Content::Image(editor) => editor.read(cx).document().read(cx).is_dirty(),
            Content::Video(editor) => editor.read(cx).document().read(cx).is_dirty(),
        }
    }

    fn title_actions(&self, cx: &mut App) -> gpui_kit::AnyElement {
        match self {
            Content::Image(editor) => editor.update(cx, |editor, cx| editor.title_actions(cx)),
            Content::Video(editor) => editor.update(cx, |editor, cx| editor.title_actions(cx)),
        }
    }

    fn view(&self) -> gpui_kit::AnyView {
        match self {
            Content::Image(editor) => editor.clone().into(),
            Content::Video(editor) => editor.clone().into(),
        }
    }
}

pub struct StudioWindow {
    path: PathBuf,
    editor: Content,
    chat: Option<Entity<ChatPanel>>,
    chat_shown: bool,
    /// The chat's width when shown (the user drags its edge).
    chat_width: f32,
    slide: Option<Slide>,
    /// The user agreed to close with changes unsaved.
    may_close: bool,
    _subscriptions: Vec<Subscription>,
}

impl StudioWindow {
    fn new(
        path: PathBuf,
        workspace: Option<(Store, Project)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = file_name(&path);
        let mut subscriptions = Vec::new();
        window.set_window_title(&format!("{name} — Studio"));
        let project_id = workspace.as_ref().map(|(_, project)| project.id);
        let video = !ph_studio::edits(&path);
        let editor = if video {
            let doc = cx.new(|cx| VideoDocument::new(path.clone(), project_id, cx));
            let editor = cx.new(|cx| VideoEditor::new(doc.clone(), window, cx));
            subscriptions.push(cx.observe(&doc, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&editor, |_, _, cx| cx.notify()));
            Content::Video(editor)
        } else {
            let doc = cx.new(|cx| ImageDocument::new(path.clone(), project_id, cx));
            let editor = cx.new(|cx| ImageEditor::new(doc.clone(), window, cx));
            subscriptions.push(cx.observe(&doc, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&editor, |_, _, cx| cx.notify()));
            Content::Image(editor)
        };
        let setup = workspace.map(|(store, project)| {
            let text = instructions(&path, video);
            ChatSetup {
                store,
                project,
                title: format!("Edit {name} with an agent"),
                subtitle: "Describe the change; it shows here as the agent makes it, and you can undo any step. Nothing is saved until you say so.".into(),
                placeholder: if video {
                    "Describe the edit: cut the loading, speed up the build, label each step…"
                } else {
                    "Describe the edit: crop to the dialog, blur the email, add a title…"
                }
                .into(),
                cwd: path.parent().map(PathBuf::from).unwrap_or_default(),
                instructions: Rc::new(move |_: &App| text.clone()),
                starters: if video { &VIDEO_STARTERS } else { &STARTERS },
            }
        });
        subscriptions.push(
            // Forget the window when it closes, so the file opens anew.
            cx.on_release({
                let path = path.clone();
                move |_, cx| {
                    let windows = cx.default_global::<StudioWindows>();
                    windows.open.remove(&path);
                    windows.views.remove(&path);
                }
            }),
        );
        let chat = setup.map(|setup| {
            let ChatSetup {
                store,
                project,
                title,
                subtitle,
                placeholder,
                cwd,
                instructions,
                starters,
            } = setup;
            let chat = cx.new(|cx| ChatPanel::new(store.clone(), project, window, cx));
            let context = ChatContext {
                title,
                subtitle,
                starters: Rc::new(move || {
                    starters
                        .iter()
                        .map(|(id, title, hint, _)| Starter {
                            id: id.to_string(),
                            title: title.to_string(),
                            hint: hint.to_string(),
                        })
                        .collect()
                }),
                placeholder,
                cwd,
                prepare: None,
                instructions: Some(instructions),
                app_tools: true,
                own_files: None,
            };
            chat.update(cx, |chat, cx| {
                chat.set_context(Some(context), window, cx);
                chat.set_session(None, cx);
            });
            subscriptions.push(cx.subscribe_in(
                &chat,
                window,
                move |_, chat, event: &ChatPanelEvent, window, cx| match event {
                    ChatPanelEvent::Starter(id) => {
                        if let Some((_, title, _, prompt)) =
                            starters.iter().find(|(starter, ..)| starter == id)
                        {
                            chat.update(cx, |chat, cx| {
                                chat.send_text(
                                    prompt.to_string(),
                                    Some(title.to_string()),
                                    window,
                                    cx,
                                )
                            });
                        }
                    }
                    ChatPanelEvent::SelectSession(id) => {
                        let session = id.and_then(|id| store.session(id).ok().flatten());
                        chat.update(cx, |chat, cx| chat.set_session(session, cx));
                    }
                    _ => cx.notify(),
                },
            ));
            chat
        });
        // Closing with changes not saved asks first.
        window.on_window_should_close(cx, {
            let this = cx.entity().downgrade();
            move |window, cx| {
                let Some(this) = this.upgrade() else {
                    return true;
                };
                this.update(cx, |this, cx| this.should_close(window, cx))
            }
        });
        let remembered = cx.try_global::<StudioWindows>();
        let chat_shown = remembered
            .and_then(|windows| windows.chat_shown)
            .unwrap_or(true);
        let chat_width = remembered
            .and_then(|windows| windows.chat_width)
            .unwrap_or(CHAT_WIDTH);
        Self {
            path,
            editor,
            chat,
            chat_shown,
            chat_width,
            slide: None,
            may_close: false,
            _subscriptions: subscriptions,
        }
    }

    fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let dirty = self.editor.is_dirty(cx);
        if !dirty || self.may_close {
            return true;
        }
        let answer = window.prompt(
            gpui_kit::PromptLevel::Warning,
            "Close without saving?",
            Some("The changes are lost; the file itself was never changed."),
            &["Close", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                _ = this.update_in(cx, |this, window, _| {
                    this.may_close = true;
                    window.remove_window();
                });
            }
        })
        .detach();
        false
    }

    /// Slides the chat in or out.
    fn toggle_chat(&mut self, cx: &mut Context<Self>) {
        let from = self.shown_width();
        self.chat_shown = !self.chat_shown;
        let to = if self.chat_shown { self.chat_width } else { 0. };
        self.slide = Some(Slide {
            start: std::time::Instant::now(),
            from,
            to,
        });
        cx.default_global::<StudioWindows>().chat_shown = Some(self.chat_shown);
        cx.notify();
    }

    /// How wide the chat is on screen now (mid-slide, part of its width).
    fn shown_width(&self) -> f32 {
        match self.slide {
            Some(slide) => slide.now().0,
            None if self.chat_shown => self.chat_width,
            None => 0.,
        }
    }

    /// The chat's edge dragged: the chat takes what is right of the
    /// pointer, leaving the picture room.
    fn drag_edge(&mut self, event: &DragMoveEvent<ChatEdge>, cx: &mut Context<Self>) {
        let right = f32::from(event.bounds.origin.x + event.bounds.size.width);
        let widest = (f32::from(event.bounds.size.width) - EDITOR_MIN).max(CHAT_MIN);
        let width = (right - f32::from(event.event.position.x)).clamp(CHAT_MIN, widest);
        if (width - self.chat_width).abs() >= 0.5 {
            self.chat_width = width;
            self.slide = None;
            cx.default_global::<StudioWindows>().chat_width = Some(width);
            cx.notify();
        }
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

impl Render for StudioWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let name = file_name(&self.path);
        let dirty = self.editor.is_dirty(cx);
        let actions = self.editor.title_actions(cx);
        let chat_button = self.chat.as_ref().map(|_| {
            Button::new("studio-chat")
                .occlude()
                .ghost()
                .small()
                .icon(Icons::MessageSquare)
                .label("Agent")
                .selected(self.chat_shown)
                .tooltip(ph_ui::tip(
                    "Show or hide the chat: describe an edit and an agent makes it",
                ))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_chat(cx)))
        });
        // Mid-slide the chat keeps its full width and is cut by its pane,
        // so it slides rather than reflowing its text every frame.
        let width = match self.slide.map(|slide| slide.now()) {
            Some((width, true)) => {
                window.request_animation_frame();
                width
            }
            Some((width, false)) => {
                self.slide = None;
                width
            }
            None => self.shown_width(),
        };
        let chat_width = self.chat_width;
        let chat = self.chat.clone().filter(|_| width >= 0.5).map(|chat| {
            div()
                .relative()
                .flex_none()
                .w(px(width))
                .h_full()
                .overflow_hidden()
                .border_l_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(px(chat_width))
                        .child(chat),
                )
                // Drag the edge to make the chat wider or narrower.
                .child(
                    div()
                        .id("studio-chat-edge")
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(ph_ui::rems_from_px(5.))
                        .cursor_col_resize()
                        .on_drag(ChatEdge, |edge, _, _, cx| cx.new(|_| edge.clone())),
                )
        });
        v_flex()
            .size_full()
            .bg(theme.background)
            // Closing by the key is closing by the button: with changes
            // not saved, it asks.
            .on_action(cx.listener(|this, _: &ph_files::CloseFile, window, cx| {
                if this.should_close(window, cx) {
                    window.remove_window();
                }
            }))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ChatEdge>, _, cx| {
                    this.drag_edge(event, cx)
                }),
            )
            .child(
                TitleBar::new().h(layout::TITLE_BAR_HEIGHT).child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .pr_3()
                        .gap_2()
                        .child(
                            Icon::new(Icons::SlidersHorizontal)
                                .small()
                                .text_color(theme.muted_foreground)
                                .flex_none(),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Studio"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("·"),
                        )
                        .child(div().min_w_0().truncate().text_sm().child(name))
                        .when(dirty, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .size(ph_ui::rems_from_px(7.))
                                    .rounded_full()
                                    .bg(theme.muted_foreground),
                            )
                        })
                        .child(div().flex_1().min_w_2())
                        // Windows treats the bar as the window's drag area:
                        // a button in it takes clicks only from an area
                        // that blocks the bar (Export did nothing there,
                        // while its shortcut worked).
                        .child(div().flex_none().occlude().child(actions))
                        .children(chat_button),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(div().flex_1().min_w_0().h_full().child(self.editor.view()))
                    .children(chat),
            )
    }
}
