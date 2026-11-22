//! A deck's window: the chat on the left, whose agent makes and changes the
//! deck, and the deck on the right, where the user changes it by hand
//! (`ph_slides`). One window a deck: opening it again brings it forward.
//!
//! The deck is a web page shown in the system's web view. That view covers
//! what the app draws in its place, so nothing of the app's is put over the
//! deck: the title bar and the chat are beside it, the deck's own controls
//! are in its page, and while a dialog of the app is open the web view
//! steps aside.
//!
//! What the deck's page asks of the app comes through `ph_slides::Decks`:
//! to present (the window fills the screen), to share, to export, to send a
//! comment to the agent.
//!
//! The chat is the deck's own (it works in the deck's folder), or, for a
//! deck that belongs to a workspace, that workspace's: the same chats the
//! workspace's window shows, whose agent works in the workspace's folders
//! and on the deck. A deck made from a workspace's chat belongs to it from
//! the start; the title bar says which workspace a deck works from, and
//! changes it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, DragMoveEvent, Entity, FocusHandle,
    Global, InteractiveElement as _, IntoElement, ParentElement as _, Render, Size,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowBounds,
    WindowOptions, div, px,
};
use ph_agent::{ChatContext, ChatPanel, ChatPanelEvent, Instructions, Starter};
use ph_artifacts::{Artifact, Artifacts};
use ph_slides::{DeckEvent, DeckView, Decks};
use ph_store::Store;
use ph_store::projects::Project;
use ph_theme::layout;
use ph_ui::Icons;
use ph_ui::MenuButton as _;
use serde_json::Value;

/// The chat's width at first, and its least: narrower, and its menus would
/// reach over the deck, where they cannot show.
const CHAT_WIDTH: f32 = 400.;
const CHAT_MIN: f32 = 340.;
/// What the deck keeps when the chat is dragged wider.
const DECK_MIN: f32 = 520.;
/// How long a deck is left alone, after it changed, before its picture for
/// the list is taken anew.
const PICTURE_AFTER: Duration = Duration::from_secs(3);
/// The look a deck starts in until one is chosen.
const FIRST_LOOK: &str = "paper";

/// Open deck windows, by the deck's artifact.
#[derive(Default)]
struct DeckWindows {
    open: HashMap<String, AnyWindowHandle>,
    chat_shown: Option<bool>,
    chat_width: Option<f32>,
}

impl Global for DeckWindows {}

/// The chat's edge being dragged.
#[derive(Clone)]
struct ChatEdge;

impl Render for ChatEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

/// The empty chat's suggestions for a deck: id, title, hint, prompt.
const STARTERS: [(&str, &str, &str, &str); 4] = [
    (
        "draft",
        "Draft the deck",
        "From a few lines about it",
        "Draft this deck. Ask me in one message what it is for, who will see it and what they should take away (and for notes or files to work from). Then show me an outline of the slides, one line each, and make them once I agree.",
    ),
    (
        "file",
        "Start from a file",
        "A document, a PDF, a sheet, a PowerPoint",
        "I will give you a file to make this deck from. Ask me for it (I can drop it on this chat), read it, then show me an outline of the slides, one line each, and make them once I agree.",
    ),
    (
        "design",
        "Make it look better",
        "Layout, spacing, a picture or chart",
        "Look at every slide as drawn (deck_look with render) and improve its design within the deck's look: alignment, spacing, hierarchy, text that does not fit, and a chart or a diagram where one says it better than words. Tell me what you changed.",
    ),
    (
        "tighten",
        "Tighten the words",
        "Shorter, clearer slides",
        "Go through every slide and make the wording shorter and clearer: one idea a slide, short titles, at most six bullets, no full sentences where a phrase will do. Keep the facts. Tell me what you changed.",
    ),
];

/// The first suggestion in the chat of a deck that belongs to a workspace,
/// in place of the one that asks for a file: what is there to draft from.
const FROM_WORKSPACE: (&str, &str, &str, &str) = (
    "workspace",
    "Draft from this workspace",
    "What its code and documents say",
    "Draft this deck from this workspace: read what matters here (the README, the documents, how the code is laid out), then show me an outline of the slides, one line each, and make them once I agree. Ask me first who the deck is for if that is not clear.",
);

/// The suggestions of a deck's chat: its own, or a workspace's.
fn starters(from_workspace: bool) -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    STARTERS
        .iter()
        .map(|starter| {
            if from_workspace && starter.0 == "file" {
                FROM_WORKSPACE
            } else {
                *starter
            }
        })
        .collect()
}

/// The workspace a deck belongs to, while there is such a workspace: its
/// chats are the deck's.
fn workspace_of(artifact: &Artifact, cx: &App) -> Option<Project> {
    let source = artifact.manifest.source.as_ref()?;
    crate::AppState::store(cx)
        .project(source.project)
        .ok()
        .flatten()
        .filter(Project::is_workspace)
}

/// Makes a deck and opens its window: the *Slides* card at the head of the
/// Artifacts page, and *New slides* in the palette.
pub(crate) fn create(window: &mut Window, cx: &mut App) {
    let Some(decks) = Decks::global(cx) else {
        return;
    };
    let made = decks.update(cx, |decks, cx| {
        decks.create("Untitled deck".into(), FIRST_LOOK, cx)
    });
    open_made(made, window, cx);
}

/// Makes a deck from the PowerPoint file at `path` and opens its window:
/// how such a file is opened from Files. The file itself stays as it is.
pub(crate) fn create_from(path: PathBuf, window: &mut Window, cx: &mut App) {
    let Some(decks) = Decks::global(cx) else {
        return;
    };
    let made = decks.update(cx, |decks, cx| decks.create_from(path, cx));
    open_made(made, window, cx);
}

/// Whether this build makes decks, and `path` is a file one is made from.
pub(crate) fn opens(path: &std::path::Path, cx: &App) -> bool {
    Decks::global(cx).is_some() && ph_slides::is_powerpoint(path)
}

fn open_made(made: gpui_kit::Task<Result<Artifact>>, window: &mut Window, cx: &mut App) {
    let from = window.window_handle();
    cx.spawn(async move |cx| {
        let opened = match made.await {
            Ok(artifact) => cx.update(|cx| open(&artifact, cx)),
            Err(err) => Err(err),
        };
        if let Err(err) = opened {
            _ = from.update(cx, |_, window, cx| {
                window.push_notification(
                    Notification::error(format!("The deck could not be made: {err:#}")),
                    cx,
                )
            });
        }
    })
    .detach();
}

/// Opens the deck `artifact` in its window, or brings that window forward.
pub(crate) fn open(artifact: &Artifact, cx: &mut App) -> Result<()> {
    let id = artifact.id().to_string();
    let existing = cx
        .try_global::<DeckWindows>()
        .and_then(|windows| windows.open.get(&id).copied());
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    let artifacts = Artifacts::global(cx).context("this build has no artifacts")?;
    let store = crate::AppState::store(cx).clone();
    // Made before the window either way: the deck's own chats are there
    // when its workspace is gone, or let go of.
    let own = store.presentation_space(&artifact.manifest.title, &artifact.folder)?;
    let (project, workspace) = match workspace_of(artifact, cx) {
        Some(workspace) => (workspace, true),
        None => (own, false),
    };
    // Files to work from are kept beside the deck, not in what is served.
    // (Made off the window's thread, like all that touches the disk; the
    // agent that will use it has not been started yet.)
    let references = artifact.folder.join("references");
    cx.background_spawn(async move {
        let _ = std::fs::create_dir_all(references);
    })
    .detach();
    let address = artifacts.update(cx, |artifacts, cx| artifacts.address(&id, cx));
    let artifact = artifact.clone();
    cx.spawn(async move |cx| {
        let address = address.await?;
        cx.update(|cx| open_window(artifact, address, store, project, workspace, cx))
    })
    .detach_and_log(cx);
    Ok(())
}

/// A task whose failure is said in the log.
trait DetachAndLog {
    fn detach_and_log(self, cx: &App);
}

impl DetachAndLog for gpui_kit::Task<Result<()>> {
    fn detach_and_log(self, cx: &App) {
        cx.background_spawn(async move {
            if let Err(err) = self.await {
                tracing::warn!("a deck could not be opened: {err:#}");
            }
        })
        .detach();
    }
}

fn open_window(
    artifact: Artifact,
    address: String,
    store: Store,
    project: Project,
    workspace: bool,
    cx: &mut App,
) -> Result<()> {
    let screen = cx
        .primary_display()
        .map(|display| display.bounds().size)
        .unwrap_or(Size::new(px(1440.), px(900.)));
    let size = Size::new(
        px(1480.).min(screen.width * 0.96),
        px(920.).min(screen.height * 0.94),
    );
    let mut options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
        window_min_size: Some(Size::new(px(940.), px(600.))),
        ..TitleBar::window_options()
    };
    if let Some(titlebar) = options.titlebar.as_mut() {
        let (x, y) = layout::TRAFFIC_LIGHTS;
        titlebar.traffic_light_position = Some(gpui_kit::point(x, y));
    }
    let id = artifact.id().to_string();
    let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
        ph_theme::apply_appearance(&crate::AppState::appearance(cx), Some(window), cx);
        cx.new(|cx| DeckWindow::new(artifact, address, store, project, workspace, window, cx))
    })
    .context("opening the deck")?;
    cx.default_global::<DeckWindows>().open.insert(id, handle);
    Ok(())
}

/// How the window shows while the deck is presented.
#[derive(Clone, Copy, PartialEq)]
enum Presenting {
    No,
    /// The slides fill the window, which fills the screen.
    Full,
    /// The presenter's view here; the slides in a window for the audience.
    Notes,
}

pub struct DeckWindow {
    id: String,
    folder: PathBuf,
    title: String,
    deck: Entity<DeckView>,
    /// What has the app's focus while the deck has the keyboard: the
    /// window itself, so that nothing in the chat acts on a key meant for
    /// the deck (⌘Z, ⌘V, ⌘A reach the app before the web view).
    focus: FocusHandle,
    /// Whether something of the app's is drawn where the deck is, for
    /// which the deck's web view has stepped aside.
    covered: bool,
    /// A dialog is about to open (the deck steps aside first).
    opening: bool,
    /// The window has drawn a frame.
    drawn: bool,
    chat: Entity<ChatPanel>,
    /// The workspace whose chat this is (its id and name), for a deck that
    /// belongs to one; `None` for the deck's own chat.
    workspace: Option<(i64, String)>,
    _chat: Subscription,
    chat_shown: bool,
    chat_width: f32,
    presenting: Presenting,
    /// Whether the window filled the screen before presenting began.
    was_fullscreen: bool,
    audience: Option<AnyWindowHandle>,
    /// The picture of the deck being taken for the list, when one is due.
    _picture: Option<gpui_kit::Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl DeckWindow {
    fn new(
        artifact: Artifact,
        address: String,
        store: Store,
        project: Project,
        workspace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = artifact.id().to_string();
        let title = artifact.manifest.title.clone();
        window.set_window_title(&format!("{title} — Slides"));
        let deck = cx.new(|cx| DeckView::new(&id, &address, window, cx));
        if let Some(decks) = Decks::global(cx) {
            let (view, handle) = (deck.downgrade(), window.window_handle());
            decks.update(cx, |decks, _| decks.opened(&id, view, handle));
        }
        let linked = workspace.then(|| (project.id, project.name.clone()));
        let (chat, chat_events) = Self::chat_for(&artifact, &store, project, workspace, window, cx);
        let mut subscriptions = vec![
            // Forget the window when it closes, so the deck opens anew.
            cx.on_release({
                let id = id.clone();
                move |this, cx| {
                    cx.default_global::<DeckWindows>().open.remove(&id);
                    this.close_audience(cx);
                    // A workspace has many chats: the one the deck was
                    // worked on in is the one it opens with next time.
                    if this.workspace.is_some() {
                        let chat = this.chat.read(cx).active_session_id(cx);
                        crate::artifacts::remember_deck_chat(&id, chat, cx);
                    }
                }
            }),
        ];
        if let Some(decks) = Decks::global(cx) {
            subscriptions.push(cx.subscribe_in(
                &decks,
                window,
                |this, _, event: &DeckEvent, window, cx| {
                    if event.id == this.id {
                        this.told(&event.what, window, cx);
                    }
                },
            ));
        }
        if let Some(artifacts) = Artifacts::global(cx) {
            // Renamed (in the list, or by the deck taking a title), or gone.
            subscriptions.push(
                cx.observe_in(&artifacts, window, |this, artifacts, window, cx| {
                    let found = artifacts.read(cx).find(&this.id).cloned();
                    match found {
                        Some(artifact) => {
                            // Given to another workspace, or let go of one,
                            // from elsewhere (or its workspace is gone):
                            // the chat beside it follows.
                            let from = workspace_of(&artifact, cx);
                            if from.as_ref().map(|project| project.id)
                                != this.workspace.as_ref().map(|(id, _)| *id)
                            {
                                this.work_from(from, window, cx);
                            }
                            this.folder = artifact.folder;
                            if artifact.manifest.title != this.title {
                                this.title = artifact.manifest.title;
                                window.set_window_title(&format!("{} — Slides", this.title));
                                let title = heading(&this.title);
                                this.chat.update(cx, |chat, cx| chat.set_title(title, cx));
                                cx.notify();
                            }
                        }
                        // Deleted: its window goes with it.
                        None if artifacts.read(cx).loaded() => window.remove_window(),
                        None => {}
                    }
                }),
            );
        }
        let remembered = cx.try_global::<DeckWindows>();
        let chat_shown = remembered
            .and_then(|windows| windows.chat_shown)
            .unwrap_or(true);
        let chat_width = remembered
            .and_then(|windows| windows.chat_width)
            .unwrap_or(CHAT_WIDTH);
        Self {
            id,
            folder: artifact.folder,
            title,
            deck,
            focus: cx.focus_handle(),
            covered: false,
            opening: false,
            drawn: false,
            chat,
            workspace: linked,
            _chat: chat_events,
            chat_shown,
            chat_width,
            presenting: Presenting::No,
            was_fullscreen: false,
            audience: None,
            _picture: None,
            _subscriptions: subscriptions,
        }
    }

    /// The chat beside deck `artifact`, in `project`'s space: the deck's own
    /// (working in the deck's folder), or, with `workspace`, the chats of
    /// the workspace the deck belongs to (working in its folders, and
    /// changing the deck's files as the deck's own chat does).
    fn chat_for(
        artifact: &Artifact,
        store: &Store,
        project: Project,
        workspace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<ChatPanel>, Subscription) {
        let id = artifact.id().to_string();
        let title = artifact.manifest.title.clone();
        let (space, name, folder) = (project.id, project.name.clone(), project.path.clone());
        let chat = cx.new(|cx| ChatPanel::new(store.clone(), project, window, cx));
        let instructions: Instructions = {
            let (id, deck) = (id.clone(), artifact.folder.clone());
            Rc::new(move |cx: &App| {
                let mut text = instructions(&id, &deck, cx);
                if workspace {
                    // The workspace may have other decks: this one is named.
                    text.push_str(&format!(
                        " This is the workspace's deck `{id}`: name it (`deck`) when you call a deck_ tool."
                    ));
                }
                text
            })
        };
        let subtitle = if workspace {
            format!(
                "This is {name}'s chat: the agent works in its folders and on this deck. Say what the deck should show, or click something on a slide and say \"this\". Each change shows at once and can be undone."
            )
        } else {
            "Say what the deck is about, or what to change. Click something on a slide first and say \"this\". Each change shows at once and can be undone.".into()
        };
        let context = ChatContext {
            title: heading(&title),
            subtitle,
            starters: Rc::new(move || {
                starters(workspace)
                    .into_iter()
                    .map(|(id, title, hint, _)| Starter {
                        id: id.to_string(),
                        title: title.to_string(),
                        hint: hint.to_string(),
                    })
                    .collect()
            }),
            placeholder: "Describe the deck, or the change: a slide on pricing, shorten this, make a chart of…".into(),
            // A workspace's chat works where the workspace is; the deck's
            // own, in the deck's folder.
            cwd: if workspace { folder } else { artifact.folder.clone() },
            prepare: None,
            instructions: Some(instructions),
            // In the deck's own chat the app's tools are the deck's. A
            // workspace's chat keeps its own rules for them: the deck's
            // tools and files are let through for every chat of a
            // workspace with a deck (`ph_agent::own_files`).
            app_tools: !workspace,
            // The deck's own files: what the agent changes there shows at
            // once and is undone in the deck.
            own_files: Some(artifact.folder.join("site")),
        };
        // A workspace has many chats: the one the deck was made or last
        // worked on in is the one shown.
        let session = workspace
            .then(|| crate::artifacts::deck_chat(&id, space, cx))
            .flatten();
        chat.update(cx, |chat, cx| {
            chat.set_context(Some(context), window, cx);
            chat.set_session(session, cx);
        });
        let store = store.clone();
        let events = cx.subscribe_in(
            &chat,
            window,
            move |_, chat, event: &ChatPanelEvent, window, cx| match event {
                ChatPanelEvent::Starter(id) => {
                    if let Some((_, title, _, prompt)) = starters(workspace)
                        .into_iter()
                        .find(|(starter, ..)| starter == id)
                    {
                        chat.update(cx, |chat, cx| {
                            chat.send_text(prompt.to_string(), Some(title.to_string()), window, cx)
                        });
                    }
                }
                ChatPanelEvent::SelectSession(id) => {
                    let session = id.and_then(|id| store.session(id).ok().flatten());
                    chat.update(cx, |chat, cx| chat.set_session(session, cx));
                }
                _ => cx.notify(),
            },
        );
        (chat, events)
    }

    /// The deck works from workspace `to` from here on (`None`: from none,
    /// in its own chat): the deck is noted as that workspace's, and the
    /// chat beside it becomes that workspace's.
    fn work_from(&mut self, to: Option<Project>, window: &mut Window, cx: &mut Context<Self>) {
        if to.as_ref().map(|project| project.id) == self.workspace.as_ref().map(|(id, _)| *id) {
            return;
        }
        let Some(artifacts) = Artifacts::global(cx) else {
            return;
        };
        let Some(artifact) = artifacts.read(cx).find(&self.id).cloned() else {
            return;
        };
        let store = crate::AppState::store(cx).clone();
        let project = match &to {
            Some(workspace) => workspace.clone(),
            None => match store.presentation_space(&artifact.manifest.title, &artifact.folder) {
                Ok(own) => own,
                Err(err) => {
                    tracing::warn!("the deck's own chat could not be opened: {err:#}");
                    return;
                }
            },
        };
        // The chat it leaves keeps its place for the next time.
        if self.workspace.is_some() {
            let chat = self.chat.read(cx).active_session_id(cx);
            crate::artifacts::remember_deck_chat(&self.id, chat, cx);
        }
        let source = to.as_ref().map(|workspace| ph_artifacts::library::Source {
            project: workspace.id,
            workspace: workspace.name.clone(),
            path: workspace.path.clone(),
        });
        artifacts
            .update(cx, |artifacts, cx| {
                artifacts.set_source(&self.id, source, cx)
            })
            .detach_and_log(cx);
        self.workspace = to
            .as_ref()
            .map(|workspace| (workspace.id, workspace.name.clone()));
        let (chat, events) = Self::chat_for(&artifact, &store, project, to.is_some(), window, cx);
        self.chat = chat;
        self._chat = events;
        cx.notify();
    }

    /// What the deck's page told the app.
    fn told(&mut self, what: &Value, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(present) = what.get("present") {
            let to = match present.as_str() {
                Some("full") => Presenting::Full,
                Some("notes") => Presenting::Notes,
                _ => Presenting::No,
            };
            self.present(to, window, cx);
        } else if what.get("focus").is_some() {
            // The deck's page took the keyboard (a click in it).
            window.focus(&self.focus, cx);
        } else if what.get("share").is_some() {
            // The deck steps aside first, so that the dialog is over it
            // from its first frame.
            self.opening = true;
            let aside = self.deck.update(cx, |deck, cx| deck.cover(cx));
            cx.spawn_in(window, async move |this, cx| {
                aside.await;
                _ = this.update_in(cx, |this, window, cx| {
                    if let Some(artifacts) = Artifacts::global(cx) {
                        ph_artifacts::ShareView::open(artifacts, &this.id, window, cx);
                    }
                    this.opening = false;
                    cx.notify();
                });
            })
            .detach();
        } else if let Some(comment) = what.get("comment") {
            self.send_comment(comment, window, cx);
        } else if let Some(format) = what["export"].as_str() {
            self.export(format, what["slide"].as_str().map(str::to_string), cx);
        } else if let Some(file) = what["thumbnail"].as_str() {
            // The user's choice of the slide the list shows the deck by.
            if let Some(live) = self.live(cx) {
                let mut deck = live.deck();
                deck.other.insert("thumbnail".into(), file.into());
                let change = ph_slides::Change {
                    path: ph_slides::deck::DECK.into(),
                    text: Some(deck.to_text()),
                };
                if let (Ok(applied), Some(decks)) =
                    (live.apply(&[change], &[], ""), Decks::global(cx))
                {
                    decks.read(cx).slides().changed(&self.id, &applied, "page");
                }
            }
            self.picture_soon(Duration::ZERO, window, cx);
        } else if what.get("changed").is_some() || what.get("opened").is_some() {
            self.picture_soon(PICTURE_AFTER, window, cx);
        }
    }

    fn live(&self, cx: &App) -> Option<std::sync::Arc<ph_slides::Live>> {
        Decks::global(cx)?.read(cx).slides().held(&self.id)
    }

    /// Says something at the foot of the deck's page.
    fn say(id: &str, text: String, error: bool, cx: &App) {
        if let Some(decks) = Decks::global(cx) {
            decks
                .read(cx)
                .slides()
                .command(id, &serde_json::json!({ "toast": text, "error": error }));
        }
    }

    /// Writes the deck out as `format`, shows the file in its folder, and
    /// says so in the page.
    fn export(&mut self, format: &str, slide: Option<String>, cx: &mut Context<Self>) {
        let Some(format) = ph_slides::export::Format::parse(format) else {
            return;
        };
        let id = self.id.clone();
        cx.spawn(async move |_, cx| {
            // Begun from here, not from where the page's wish arrived: the
            // export asks things of this window, which is busy until then.
            let written = cx
                .update(|cx| ph_slides::export::run(&id, format, slide, cx))
                .await;
            cx.update(|cx| match written {
                Ok(path) => {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    Self::say(
                        &id,
                        format!("Saved {name} in the deck's exports folder"),
                        false,
                        cx,
                    );
                    ph_ui::platform::reveal(&path, cx);
                }
                Err(err) => Self::say(&id, format!("The export failed: {err:#}"), true, cx),
            });
        })
        .detach();
    }

    /// Takes a picture of the deck for the Artifacts list (the slide the
    /// user chose for it, else the first), once the deck has been quiet
    /// for `after`: not at every letter typed.
    fn picture_soon(&mut self, after: Duration, window: &mut Window, cx: &mut Context<Self>) {
        self._picture = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(after).await;
            let taken = this.update_in(cx, |this, window, cx| {
                let deck = this.live(cx)?.deck();
                let chosen = deck.other.get("thumbnail").and_then(|file| file.as_str());
                let file = chosen
                    .filter(|file| deck.slides.iter().any(|slide| slide.file == *file))
                    .or_else(|| deck.slides.first().map(|slide| slide.file.as_str()))?
                    .to_string();
                Some(
                    this.deck
                        .update(cx, |deck, cx| deck.picture(&file, window, cx)),
                )
            });
            let Ok(Some(taken)) = taken else {
                return;
            };
            let Ok(png) = taken.await else {
                return;
            };
            _ = this.update(cx, |this, cx| {
                if let Some(artifacts) = Artifacts::global(cx) {
                    artifacts
                        .update(cx, |artifacts, cx| {
                            artifacts.set_thumbnail(&this.id, png, cx)
                        })
                        .detach();
                }
            });
        }));
    }

    /// A comment the user sent to the agent: a message in the chat, saying
    /// where it is and how to answer it.
    fn send_comment(&mut self, comment: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let text = |key: &str| comment[key].as_str().unwrap_or_default();
        let said = comment["replies"]
            .as_array()
            .and_then(|replies| replies.last())
            .and_then(|reply| reply["text"].as_str())
            .unwrap_or_else(|| text("text"));
        let about = match (text("element"), text("label")) {
            ("", _) => "the slide as a whole".to_string(),
            (element, "") => format!("the element with data-id=\"{element}\""),
            (element, label) => format!("{label} (data-id=\"{element}\")"),
        };
        let prompt = format!(
            "Comment {id} on {file}, about {about}:\n\n{said}\n\nMake the change, then answer the comment with deck_comments (reply, and resolve it when it is done).",
            id = text("id"),
            file = text("file"),
        );
        if !self.chat_shown && self.presenting == Presenting::No {
            self.toggle_chat(cx);
        }
        self.chat.update(cx, |chat, cx| {
            chat.send_aside(prompt, Some(format!("Comment: {said}")), window, cx)
        });
    }

    fn present(&mut self, to: Presenting, window: &mut Window, cx: &mut Context<Self>) {
        if self.presenting == to {
            return;
        }
        let was = self.presenting;
        self.presenting = to;
        if was == Presenting::No {
            self.was_fullscreen = window.is_fullscreen();
        }
        // The slides alone fill the screen; the presenter's view stays a
        // window, beside whatever else the presenter needs.
        let fill = to == Presenting::Full || (to == Presenting::No && self.was_fullscreen);
        if window.is_fullscreen() != fill {
            window.toggle_fullscreen();
        }
        self.close_audience(cx);
        if to == Presenting::Notes {
            let address = self.deck.read(cx).address().to_string();
            self.audience = audience_window(&self.id, &address, &self.title, window, cx);
        }
        if to == Presenting::No {
            window.activate_window();
        }
        self.deck.read(cx).focus(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn close_audience(&mut self, cx: &mut App) {
        if let Some(audience) = self.audience.take() {
            _ = audience.update(cx, |_, window, _| window.remove_window());
        }
    }

    fn toggle_chat(&mut self, cx: &mut Context<Self>) {
        self.chat_shown = !self.chat_shown;
        cx.default_global::<DeckWindows>().chat_shown = Some(self.chat_shown);
        cx.notify();
    }

    /// The chat's edge dragged: the chat takes what is left of the pointer,
    /// leaving the deck room.
    fn drag_edge(&mut self, event: &DragMoveEvent<ChatEdge>, cx: &mut Context<Self>) {
        let left = f32::from(event.bounds.origin.x);
        let widest = (f32::from(event.bounds.size.width) - DECK_MIN).max(CHAT_MIN);
        let width = (f32::from(event.event.position.x) - left).clamp(CHAT_MIN, widest);
        if (width - self.chat_width).abs() >= 0.5 {
            self.chat_width = width;
            cx.default_global::<DeckWindows>().chat_width = Some(width);
            cx.notify();
        }
    }
}

fn heading(title: &str) -> String {
    format!("Work on {title} with an agent")
}

/// What the agent is told with each message in a deck's chat: what a deck
/// is, where this one is, what the user has selected, and what they
/// changed by hand since the agent last heard.
fn instructions(id: &str, folder: &std::path::Path, cx: &App) -> String {
    let app = ph_brand::NAME;
    let live = Decks::global(cx).and_then(|decks| decks.read(cx).slides().held(id));
    let mut text = format!(
        "[{app} Slides] The user is working on a deck beside this chat: a folder of web pages at {site}. `deck.json` lists the slides in order (with their speaker notes), `deck.css` is the deck's look, each slide is one file in `slides/` holding one `<section class=\"slide\">`, pictures are in `media/`. Change the deck by editing those files with your own file tools: the user sees each change at once and can undo it. Read deck_guide before your first change to a deck in this chat. After changing slides, call deck_look: it says what does not fit on which slide (fix that before you answer), and with `render` shows slides as drawn. Files the user gives you to work from go in {references}.",
        site = folder.join("site").display(),
        references = folder.join("references").display(),
    );
    let Some(live) = live else {
        return text;
    };
    let deck = live.deck();
    text.push_str(&format!(
        " The deck is “{}”, {} slide{}, {} × {} pixels.",
        deck.title,
        deck.slides.len(),
        if deck.slides.len() == 1 { "" } else { "s" },
        deck.size[0],
        deck.size[1],
    ));
    if let Some(selection) = live.selection() {
        let said = |key: &str| selection[key].as_str().unwrap_or_default().to_string();
        text.push_str(&format!(
            " On the stage: slide {} ({}).",
            selection["slide"].as_u64().unwrap_or(1),
            said("file"),
        ));
        if !said("tag").is_empty() {
            let quoted = match said("text") {
                words if words.is_empty() => String::new(),
                words => format!(" “{words}”"),
            };
            let by_id = match said("element") {
                id if id.is_empty() => String::new(),
                id => format!(", data-id=\"{id}\""),
            };
            text.push_str(&format!(
                " Selected: <{}>{quoted} (at `{}`{by_id}); \"this\" in the user's message means it.",
                said("tag"),
                said("path"),
            ));
        } else {
            text.push_str(" Nothing on it is selected; \"this slide\" means that slide.");
        }
    }
    let journal = live.take_journal();
    if !journal.is_empty() {
        text.push_str(&format!(
            " Since your last turn the user changed the deck by hand ({}): read a file again before you change it.",
            journal.join("; ")
        ));
    }
    text
}

/// The window the audience sees while the presenter has their notes: the
/// slides alone, on another screen when there is one, following the
/// presenter.
fn audience_window(
    id: &str,
    address: &str,
    title: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    if !ph_webview::supported() {
        // The deck is in the user's browser: so is the audience's view.
        cx.open_url(&format!("{address}?audience=1"));
        return None;
    }
    let own = window.display(cx).map(|display| display.id());
    let other = cx
        .displays()
        .into_iter()
        .find(|display| Some(display.id()) != own);
    let bounds = match &other {
        Some(display) => WindowBounds::Fullscreen(display.bounds()),
        None => WindowBounds::Windowed(Bounds::centered(None, Size::new(px(960.), px(540.)), cx)),
    };
    let options = WindowOptions {
        window_bounds: Some(bounds),
        display_id: other.as_ref().map(|display| display.id()),
        window_min_size: Some(Size::new(px(320.), px(180.))),
        ..TitleBar::window_options()
    };
    let (id, address, title) = (
        id.to_string(),
        format!("{address}?audience=1"),
        format!("{title} — Audience"),
    );
    gpui_kit::open_window(options, cx, move |window, cx| {
        window.set_window_title(&title);
        cx.new(|cx| Audience {
            deck: cx.new(|cx| DeckView::new(&id, &address, window, cx)),
        })
    })
    .inspect_err(|err| tracing::warn!("the audience's window could not be opened: {err:#}"))
    .ok()
    .map(|(handle, _)| handle)
}

/// The slides alone, for the audience.
struct Audience {
    deck: Entity<DeckView>,
}

impl Render for Audience {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(gpui_kit::black())
            .child(self.deck.clone())
    }
}

/// Whether something of the app's is open that may be drawn where the deck
/// is: a dialog or sheet, a note at the window's edge, a menu or pop-up of
/// the chat (those hold the app's focus while open).
///
/// `drawn`: the window has drawn a frame; before that there is no focus to
/// ask about.
fn overlay_open(drawn: bool, window: &mut Window, cx: &mut App) -> bool {
    window.has_active_dialog(cx)
        || window.has_active_sheet(cx)
        || !window.notifications(cx).is_empty()
        || (drawn
            && window
                .context_stack()
                .iter()
                .any(|context| context.contains("PopupMenu") || context.contains("Popover")))
}

impl DeckWindow {
    /// Which workspace the deck works from, and the menu that changes it:
    /// a workspace's agent has its files to make the deck of.
    fn workspace_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        /// Workspaces offered: the ones used last.
        const OFFERED: usize = 12;
        let current = self.workspace.as_ref().map(|(id, _)| *id);
        let mut workspaces: Vec<Project> = crate::AppState::store(cx)
            .projects()
            .unwrap_or_default()
            .into_iter()
            .filter(Project::is_workspace)
            .collect();
        workspaces.sort_by_key(|project| std::cmp::Reverse(project.last_opened_at));
        // The deck's own is among them however long ago it was used.
        let mut offered: Vec<Project> = workspaces.iter().take(OFFERED).cloned().collect();
        if let Some(own) = workspaces
            .iter()
            .find(|project| Some(project.id) == current)
            .filter(|own| !offered.iter().any(|project| project.id == own.id))
        {
            offered.push(own.clone());
        }
        let this = cx.entity().downgrade();
        let button = Button::new("deck-workspace")
            .ghost()
            .small()
            .icon(Icons::FolderOpen)
            .accessibility_label("The workspace this deck works from")
            .tooltip(ph_ui::tip(match &self.workspace {
                Some(_) => "The chat is this workspace's: its agent works in the workspace's folders and on this deck",
                None => "Work from a workspace: its agent gets the workspace's files to make this deck from",
            }));
        let button = match &self.workspace {
            Some((_, name)) => {
                let short: String = name.chars().take(28).collect();
                button.label(if short.len() < name.len() {
                    format!("{short}…")
                } else {
                    short
                })
            }
            None => button,
        };
        button.menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
            let pick = |to: Option<Project>| {
                let this = this.clone();
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                    let to = to.clone();
                    _ = this.update(cx, |this, cx| this.work_from(to, window, cx));
                }
            };
            let mut menu = menu.label("The agent works from");
            for workspace in &offered {
                // No icon: its place is where the tick of the one chosen is.
                menu = menu.item(
                    PopupMenuItem::new(workspace.name.clone())
                        .checked(Some(workspace.id) == current)
                        .on_click(pick(Some(workspace.clone()))),
                );
            }
            menu.separator().item(
                PopupMenuItem::new("No workspace: this deck only")
                    .checked(current.is_none())
                    .on_click(pick(None)),
            )
        })
    }
}

impl Render for DeckWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The system draws the deck's web view over everything the app
        // draws: while something of the app's may be there, it steps aside.
        let drawn = std::mem::replace(&mut self.drawn, true);
        let covered = self.opening || overlay_open(drawn, window, cx);
        if covered != self.covered {
            self.covered = covered;
            let deck = self.deck.clone();
            cx.defer(move |cx| {
                deck.update(cx, |deck, cx| {
                    if covered {
                        deck.cover(cx).detach();
                    } else {
                        deck.uncover(cx);
                    }
                })
            });
        }
        let theme = cx.theme().clone();
        let presenting = self.presenting != Presenting::No;
        let chat_width = self.chat_width;
        let chat = (self.chat_shown && !presenting).then(|| {
            div()
                .relative()
                .flex_none()
                .w(px(chat_width))
                .h_full()
                .overflow_hidden()
                .border_r_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(self.chat.clone())
                // Drag the edge to make the chat wider or narrower.
                .child(
                    div()
                        .id("deck-chat-edge")
                        .absolute()
                        .top_0()
                        .right_0()
                        .h_full()
                        .w(ph_ui::rems_from_px(5.))
                        .cursor_col_resize()
                        .on_drag(ChatEdge, |edge, _, _, cx| cx.new(|_| edge.clone())),
                )
        });
        v_flex()
            .size_full()
            .bg(theme.background)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &ph_files::CloseFile, window, _| {
                window.remove_window();
            }))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ChatEdge>, _, cx| this.drag_edge(event, cx)),
            )
            // While the slides are presented they are all there is.
            .when(self.presenting != Presenting::Full, |this| {
                this.child(
                    TitleBar::new().h(layout::TITLE_BAR_HEIGHT).child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .pr_3()
                            .gap_2()
                            .child(
                                div().flex_none().occlude().child(
                                    Button::new("deck-chat")
                                        .ghost()
                                        .small()
                                        .icon(Icons::MessageSquare)
                                        .selected(self.chat_shown)
                                        .accessibility_label("Show or hide the chat")
                                        .tooltip(ph_ui::tip(
                                            "Show or hide the chat: say what the deck needs and an agent makes it",
                                        ))
                                        .on_click(cx.listener(|this, _, _, cx| this.toggle_chat(cx))),
                                ),
                            )
                            .child(
                                Icon::new(Icons::Presentation)
                                    .small()
                                    .text_color(theme.muted_foreground)
                                    .flex_none(),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("Slides"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("·"),
                            )
                            .child(div().min_w_0().truncate().text_sm().child(self.title.clone()))
                            .child(div().flex_1().min_w_2())
                            .child(div().flex_none().occlude().child(self.workspace_button(cx))),
                    ),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .children(chat)
                    .child(div().flex_1().min_w_0().h_full().child(self.deck.clone())),
            )
    }
}
