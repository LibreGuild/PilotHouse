//! Studio's window: a picture, video or presentation being edited, with a
//! chat beside it whose agent changes it as the user describes (the
//! `studio_*` and `slides_*` tools).
//!
//! One window per file (or presentation): opening it again brings its
//! window forward. A picture's or video's chat is the workspace's (its
//! agents, its history); without a workspace (a file opened from outside
//! every project) there is none. A presentation has a space of its own, so
//! its chats stay with it.

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
use ph_studio::slides::{DeckDocument, SlidesEditor, SlidesEditorEvent};
use ph_studio::video::VideoEditor;
use ph_studio::video::document::VideoDocument;
use ph_studio::{ImageDocument, ImageEditor};
use ph_theme::layout;
use ph_ui::Icons;

/// Open Studio windows, by file.
#[derive(Default)]
struct StudioWindows {
    open: HashMap<PathBuf, AnyWindowHandle>,
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
    open_window_for(path, Opening::File(workspace), cx)
}

/// Opens the presentation in `folder` (a space in the presentations
/// folder), or brings its window forward.
pub fn open_presentation(folder: PathBuf, prompt: Option<String>, cx: &mut App) -> Result<()> {
    let folder = dunce::canonicalize(&folder).unwrap_or(folder);
    let existing = cx
        .try_global::<StudioWindows>()
        .and_then(|windows| windows.open.get(&folder).copied());
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    let store = crate::AppState::store(cx).clone();
    let title = ph_studio::slides::space::Space::open(&folder)
        .map(|(_, deck)| deck.title)
        .with_context(|| format!("{} is not a presentation", folder.display()))?;
    let project = store.presentation_space(&title, &folder)?;
    open_window_for(folder, Opening::Slides(store, project, prompt), cx)
}

/// Opens `path` (a picture) in Studio, or brings its window forward.
fn open_window_for(path: PathBuf, opening: Opening, cx: &mut App) -> Result<()> {
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
    let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
        ph_theme::apply_appearance(&crate::AppState::appearance(cx), Some(window), cx);
        let path = path.clone();
        cx.new(|cx| StudioWindow::new(path, opening, window, cx))
    })
    .context("opening Studio")?;
    cx.default_global::<StudioWindows>()
        .open
        .insert(path, handle);
    Ok(())
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

/// What a Studio window is opened on.
enum Opening {
    /// A picture or video, with its workspace's chat when it has one.
    File(Option<(Store, Project)>),
    /// A presentation, with its own space's chat (and what to ask its
    /// agent first, if anything).
    Slides(Store, Project, Option<String>),
}

/// What the agent is told with each message in a presentation's chat:
/// the deck, where its files are, and what the user has selected.
fn slides_instructions(document: &gpui_kit::Entity<DeckDocument>, cx: &App) -> String {
    let doc = document.read(cx);
    let deck = doc.deck();
    let selection = &doc.selection;
    let slide = selection
        .slide
        .as_ref()
        .and_then(|id| deck.slides.iter().position(|slide| &slide.id == id))
        .map(|index| format!("slide {} ({})", index + 1, deck.slides[index].id))
        .unwrap_or_else(|| "no slide".into());
    let elements = if selection.elements.is_empty() {
        "nothing on it".to_string()
    } else {
        format!("elements {}", selection.elements.join(", "))
    };
    let text = selection
        .text
        .as_ref()
        .map(|text| {
            format!(
                " and the text \"{}\" in it",
                text.chars().take(200).collect::<String>()
            )
        })
        .unwrap_or_default();
    format!(
        "[PilotHouse Studio · Slides] The user is editing the presentation \"{}\" ({} slides, {} × {} points, theme {}) beside this chat. Its folder is {}: put files the user gives you to work from in references/, and pictures go into the deck with slides_edit `import`. Change the deck only with the slides_* tools: slides_get to read it (with `render` to see slides), slides_edit to change it (the user sees each change at once and can undo it), slides_export only when asked. Now selected: {slide}, {elements}{text}; \"this\" in the user's message means what is selected.",
        deck.title,
        deck.slides.len(),
        deck.size.width,
        deck.size.height,
        deck.theme.name,
        doc.folder().display(),
    )
}

/// The empty chat's suggestions for a presentation: id, title, hint,
/// prompt.
const SLIDES_STARTERS: [(&str, &str, &str, &str); 4] = [
    (
        "draft",
        "Draft the deck",
        "From an outline or a few lines",
        "Draft this presentation. Ask me in one message what it is for, who will see it and what they should take away (and for any notes or files to work from), then make the slides with the theme's layouts, a title slide first, and speaker notes for each. Show me the result.",
    ),
    (
        "tighten",
        "Tighten the words",
        "Shorter, clearer slides",
        "Go through every slide and make the wording shorter and clearer: one idea a slide, short titles, at most six bullets, no full sentences where a phrase will do. Keep the facts. Tell me what you changed.",
    ),
    (
        "design",
        "Make it look better",
        "Layout, spacing, visuals",
        "Look at each slide as drawn and improve its design within the theme: alignment, spacing, hierarchy, text that does not fit, and a visual (icon, chart, diagram) where one would help. Check the result by rendering the slides.",
    ),
    (
        "summary",
        "Executive summary",
        "One slide up front",
        "Add an executive summary slide after the title: the three to five things a busy reader must know from this deck, in plain words.",
    ),
];

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
        "[PilotHouse Studio] The user is editing the {what} {} in Studio, beside this chat. Change it only with the studio_* tools: {look}, studio_edit to change it (each change shows to the user at once and can be undone). Save with studio_export only when the user asks; never edit the file by other means.",
        path.display()
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

/// What Studio shows: a picture's editor, a video's or a presentation's.
enum Content {
    Image(Entity<ImageEditor>),
    Video(Entity<VideoEditor>),
    Slides(Entity<SlidesEditor>),
}

impl Content {
    /// Whether closing would lose changes (a presentation saves itself).
    fn is_dirty(&self, cx: &App) -> bool {
        match self {
            Content::Image(editor) => editor.read(cx).document().read(cx).is_dirty(),
            Content::Video(editor) => editor.read(cx).document().read(cx).is_dirty(),
            Content::Slides(_) => false,
        }
    }

    fn title_actions(&self, cx: &mut App) -> gpui_kit::AnyElement {
        match self {
            Content::Image(editor) => editor.update(cx, |editor, cx| editor.title_actions(cx)),
            Content::Video(editor) => editor.update(cx, |editor, cx| editor.title_actions(cx)),
            Content::Slides(editor) => editor.update(cx, |editor, cx| editor.title_actions(cx)),
        }
    }

    fn view(&self) -> gpui_kit::AnyView {
        match self {
            Content::Image(editor) => editor.clone().into(),
            Content::Video(editor) => editor.clone().into(),
            Content::Slides(editor) => editor.clone().into(),
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
    fn new(path: PathBuf, opening: Opening, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = file_name(&path);
        let mut subscriptions = Vec::new();
        let mut first_prompt = None;
        let (editor, setup) = match opening {
            Opening::Slides(store, project, prompt) => {
                first_prompt = prompt;
                let doc = match ph_studio::slides::space::Space::open(&path) {
                    Ok((space, deck)) => {
                        cx.new(|cx| DeckDocument::new(space, deck, Some(project.id), cx))
                    }
                    // Gone since it was listed: an empty one in its place. A
                    // deck there that cannot be read is kept aside first, so
                    // the empty one never saves over it.
                    Err(err) => {
                        let file = path.join(ph_studio::slides::space::DECK);
                        if file.exists() {
                            let aside = path.join(format!(
                                "deck.unreadable-{}.json",
                                ph_studio::slides::ops::now()
                            ));
                            tracing::warn!(
                                "{} cannot be read ({err:#}); kept as {}",
                                file.display(),
                                aside.display()
                            );
                            let _ = std::fs::rename(&file, &aside);
                        }
                        let deck = ph_studio::slides::ops::new_deck(
                            &name,
                            "clean",
                            ph_studio::slides::model::SlideSize::default(),
                        );
                        let space = ph_studio::slides::space::Space { path: path.clone() };
                        cx.new(|cx| DeckDocument::new(space, deck, Some(project.id), cx))
                    }
                };
                let title = doc.read(cx).deck().title.clone();
                window.set_window_title(&format!("{title} — Studio"));
                let editor = cx.new(|cx| SlidesEditor::new(doc.clone(), window, cx));
                subscriptions.push(cx.observe(&doc, |_, _, cx| cx.notify()));
                subscriptions.push(cx.observe(&editor, |_, _, cx| cx.notify()));
                let instructions: Instructions = {
                    let doc = doc.clone();
                    Rc::new(move |cx: &App| slides_instructions(&doc, cx))
                };
                let setup = ChatSetup {
                    store,
                    project,
                    title: format!("Work on {title} with an agent"),
                    subtitle: "Ask for slides, wording, a design or a chart; select something first and say \"this\". Each change shows here and can be undone; the presentation saves itself.".into(),
                    placeholder: "Describe what you want: draft the deck, tighten this slide, make a chart of…".into(),
                    cwd: path.clone(),
                    instructions,
                    starters: &SLIDES_STARTERS,
                };
                (Content::Slides(editor), Some(setup))
            }
            Opening::File(workspace) => {
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
                (editor, setup)
            }
        };
        subscriptions.push(
            // Forget the window when it closes, so the file opens anew.
            cx.on_release({
                let path = path.clone();
                move |_, cx| {
                    cx.default_global::<StudioWindows>().open.remove(&path);
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
            };
            chat.update(cx, |chat, cx| {
                chat.set_context(Some(context), window, cx);
                chat.set_session(None, cx);
                if let Some(prompt) = first_prompt.take() {
                    chat.send_text(prompt, Some("Draft the deck".into()), window, cx);
                }
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
        if let (Content::Slides(slides), Some(chat)) = (&editor, &chat) {
            // A rename shows in the window's title and the chat's heading.
            let doc = slides.read(cx).document().clone();
            let heading = chat.clone();
            let mut last = doc.read(cx).deck().title.clone();
            subscriptions.push(cx.observe_in(&doc, window, move |_, doc, window, cx| {
                let title = doc.read(cx).deck().title.clone();
                if title != last {
                    window.set_window_title(&format!("{title} — Studio"));
                    heading.update(cx, |chat, cx| {
                        chat.set_title(format!("Work on {title} with an agent"), cx)
                    });
                    last = title;
                }
            }));
            // The editor's "ask the agent": a message begun in the chat.
            let asking = chat.clone();
            subscriptions.push(cx.subscribe_in(
                slides,
                window,
                move |this, _, event: &SlidesEditorEvent, window, cx| match event {
                    SlidesEditorEvent::Ask(text) => {
                        if !this.chat_shown {
                            this.toggle_chat(cx);
                        }
                        asking.update(cx, |chat, cx| chat.begin_message(text, window, cx));
                    }
                },
            ));
        }
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
        // A presentation saves what is left before it closes.
        if let Content::Slides(editor) = &self.editor {
            let doc = editor.read(cx).document().clone();
            if let Err(err) = doc.update(cx, |doc, _| doc.save_now()) {
                if self.may_close {
                    return true;
                }
                let answer = window.prompt(
                    gpui_kit::PromptLevel::Warning,
                    "Close without saving?",
                    Some(&format!("The last changes could not be saved: {err:#}")),
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
                return false;
            }
            return true;
        }
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
        let (name, icon) = match &self.editor {
            Content::Slides(editor) => (
                editor.read(cx).document().read(cx).deck().title.clone(),
                Icons::Presentation,
            ),
            _ => (file_name(&self.path), Icons::SlidersHorizontal),
        };
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
                .tooltip("Show or hide the chat: describe an edit and an agent makes it")
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
                        .w(px(5.))
                        .cursor_col_resize()
                        .on_drag(ChatEdge, |edge, _, _, cx| cx.new(|_| edge.clone())),
                )
        });
        v_flex()
            .size_full()
            .bg(theme.background)
            .on_action(cx.listener(|_, _: &ph_files::CloseFile, window, _| window.remove_window()))
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
                            Icon::new(icon)
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
                                    .size(px(7.))
                                    .rounded_full()
                                    .bg(theme.muted_foreground),
                            )
                        })
                        .child(div().flex_1().min_w_2())
                        .child(actions)
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
