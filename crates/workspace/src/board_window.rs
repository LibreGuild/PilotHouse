//! A whiteboard's window: the chat on the left, whose agent draws on the
//! board and changes it, and the board on the right, where the user works
//! by hand (`ph_boards`). One window a board: opening it again brings it
//! forward.
//!
//! The board is a web page shown in the system's web view, as a deck is
//! (`slides_window`), with the same consequence: the web view covers what
//! the app draws in its place, so the board's own controls are in its
//! page, and while a dialog or a menu of the app is open the web view
//! steps aside.
//!
//! What the board's page asks of the app comes through `ph_boards::Boards`:
//! to share, and to send a pinned note to the agent.
//!
//! The chat is the board's own (it works in the board's folder), or, for a
//! board that belongs to a workspace, that workspace's: the same chats the
//! workspace's window shows, whose agent works in the workspace's folders
//! and on the board. A board made from a workspace's chat belongs to it
//! from the start; the title bar says which workspace a board works from,
//! and changes it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    v_flex,
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, DragMoveEvent, Entity, FocusHandle,
    Global, InteractiveElement as _, IntoElement, ParentElement as _, Render, Size,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, WindowBounds,
    WindowOptions, div, px,
};
use ph_agent::{ChatContext, ChatPanel, ChatPanelEvent, Instructions, Starter};
use ph_artifacts::{Artifact, Artifacts};
use ph_boards::{BoardEvent, Boards};
use ph_slides::DeckView;
use ph_store::Store;
use ph_store::projects::Project;
use ph_theme::layout;
use ph_ui::Icons;
use serde_json::Value;

use crate::artifact_window::{overlay_open, workspace_button, workspace_of};

/// The chat's width at first, and its least: narrower, and its menus would
/// reach over the board, where they cannot show.
const CHAT_WIDTH: f32 = 400.;
const CHAT_MIN: f32 = 340.;
/// What the board keeps when the chat is dragged wider.
const BOARD_MIN: f32 = 520.;

/// Open board windows, by the board's artifact.
#[derive(Default)]
struct BoardWindows {
    open: HashMap<String, (AnyWindowHandle, WeakEntity<BoardWindow>)>,
    /// The boards whose window is being opened (its address is asked for):
    /// asked for again meanwhile, a board does not get a second window.
    opening: std::collections::HashSet<String>,
    chat_shown: Option<bool>,
    chat_width: Option<f32>,
}

impl Global for BoardWindows {}

/// The chat's edge being dragged.
#[derive(Clone)]
struct ChatEdge;

impl Render for ChatEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui_kit::Empty
    }
}

/// The empty chat's suggestions for a board: id, title, hint, prompt.
const STARTERS: [(&str, &str, &str, &str); 4] = [
    (
        "flow",
        "Draw a flow",
        "The steps and decisions of a process",
        "Draw a flowchart on this board. Ask me in one message which process it is (or what to read to find out). Then draw it: a shape a step, a diamond a decision, arrows tied to the things they join, and words on the arrows that need them.",
    ),
    (
        "system",
        "Map a system",
        "Its parts, and what talks to what",
        "Draw a map of a system on this board: who uses it, its parts, where the data lies, and what talks to what. Ask me in one message which system (or which folder to read to find out), then draw it.",
    ),
    (
        "ideas",
        "Think it through with me",
        "Ideas as notes, grouped by theme",
        "Think something through with me on this board. Ask me the question we are answering. Then put down a first round of ideas as notes, one idea a note in a few words, in sections by theme, and tell me which you would pick and why.",
    ),
    (
        "tidy",
        "Tidy the board",
        "Group, line up, name the sections",
        "Look at the board (board_look) and tidy it: put what belongs together into sections with a title, line things up and even out the space between them. Keep every word as it is. Tell me what you moved.",
    ),
];

/// The second suggestion in the chat of a board that belongs to a
/// workspace, in place of the one that asks which system: the one here.
const FROM_WORKSPACE: (&str, &str, &str, &str) = (
    "workspace",
    "Map this workspace",
    "Its parts, and what talks to what",
    "Draw a map of this workspace on this board: read what matters here (the README, the documents, how the code is laid out), then draw who uses it, its parts, where the data lies, and what talks to what, in sections, with arrows tied to the things they join. Tell me what you left out.",
);

/// The suggestions of a board's chat: its own, or a workspace's.
fn starters(from_workspace: bool) -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    STARTERS
        .iter()
        .map(|starter| {
            if from_workspace && starter.0 == "system" {
                FROM_WORKSPACE
            } else {
                *starter
            }
        })
        .collect()
}

/// Makes a board and opens its window: the *Whiteboard* card at the head of
/// the Artifacts page, and *New whiteboard* in the palette.
pub(crate) fn create(window: &mut Window, cx: &mut App) {
    let Some(boards) = Boards::global(cx) else {
        return;
    };
    let made = boards.update(cx, |boards, cx| boards.create("Untitled board".into(), cx));
    let from = window.window_handle();
    cx.spawn(async move |cx| {
        let opened = match made.await {
            Ok(artifact) => cx.update(|cx| open(&artifact, cx)),
            Err(err) => Err(err),
        };
        if let Err(err) = opened {
            _ = from.update(cx, |_, window, cx| {
                window.push_notification(
                    Notification::error(format!("The board could not be made: {err:#}")),
                    cx,
                )
            });
        }
    })
    .detach();
}

/// Opens the board `artifact` in its window, or brings that window forward.
pub(crate) fn open(artifact: &Artifact, cx: &mut App) -> Result<()> {
    let id = artifact.id().to_string();
    let existing = cx
        .try_global::<BoardWindows>()
        .and_then(|windows| windows.open.get(&id).map(|(handle, _)| *handle));
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    if !cx
        .default_global::<BoardWindows>()
        .opening
        .insert(id.clone())
    {
        return Ok(());
    }
    let artifacts = Artifacts::global(cx).context("this build has no artifacts")?;
    let store = crate::AppState::store(cx).clone();
    // Made before the window either way (the chats beside an artifact are
    // kept by its folder, as a deck's are): the board's own chats are
    // there when its workspace is gone, or let go of.
    let own = store
        .presentation_space(&artifact.manifest.title, &artifact.folder)
        .inspect_err(|_| {
            cx.default_global::<BoardWindows>().opening.remove(&id);
        })?;
    let (project, workspace) = match workspace_of(artifact, cx) {
        Some(workspace) => (workspace, true),
        None => (own, false),
    };
    let address = artifacts.update(cx, |artifacts, cx| artifacts.address(&id, cx));
    let artifact = artifact.clone();
    cx.spawn(async move |cx| {
        let opened = match address.await {
            Ok(address) => {
                cx.update(|cx| open_window(artifact, address, store, project, workspace, cx))
            }
            Err(err) => Err(err),
        };
        cx.update(|cx| {
            cx.default_global::<BoardWindows>().opening.remove(&id);
        });
        if let Err(err) = opened {
            tracing::warn!("a board could not be opened: {err:#}");
        }
    })
    .detach();
    Ok(())
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
    let (handle, shown) = gpui_kit::open_window(options, cx, |window, cx| {
        ph_theme::apply_appearance(&crate::AppState::appearance(cx), Some(window), cx);
        cx.new(|cx| BoardWindow::new(artifact, address, store, project, workspace, window, cx))
    })
    .context("opening the board")?;
    cx.default_global::<BoardWindows>()
        .open
        .insert(id, (handle, shown.downgrade()));
    Ok(())
}

/// The board `artifact` was renamed in the list, and its folder took the
/// new name: the chat of its open window works there from here on (an
/// agent started in the folder's old name would write beside the board).
pub(crate) fn moved(artifact: &Artifact, cx: &mut App) {
    let open = cx
        .try_global::<BoardWindows>()
        .and_then(|windows| windows.open.get(artifact.id()).cloned());
    let Some((handle, shown)) = open else {
        return;
    };
    _ = handle.update(cx, |_, window, cx| {
        _ = shown.update(cx, |this, cx| this.folder_moved(artifact, window, cx));
    });
}

pub struct BoardWindow {
    id: String,
    title: String,
    board: Entity<DeckView>,
    /// What has the app's focus while the board has the keyboard: the
    /// window itself, so that nothing in the chat acts on a key meant for
    /// the board (⌘Z, ⌘V, ⌘A reach the app before the web view).
    focus: FocusHandle,
    /// Whether something of the app's is drawn where the board is, for
    /// which the board's web view has stepped aside.
    covered: bool,
    /// A dialog is about to open (the board steps aside first).
    opening: bool,
    /// The window has drawn a frame.
    drawn: bool,
    chat: Entity<ChatPanel>,
    /// The workspace whose chat this is (its id and name), for a board
    /// that belongs to one; `None` for the board's own chat.
    workspace: Option<(i64, String)>,
    /// The workspace just chosen is being noted with the board: until it
    /// is, the list still names the one before, which is not followed.
    noting: bool,
    _chat: Subscription,
    chat_shown: bool,
    chat_width: f32,
    _subscriptions: Vec<Subscription>,
}

impl BoardWindow {
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
        window.set_window_title(&format!("{title} — Whiteboard"));
        let board = cx.new(|cx| DeckView::of("board", &id, &address, window, cx));
        let linked = workspace.then(|| (project.id, project.name.clone()));
        let (chat, chat_events) = Self::chat_for(&artifact, &store, project, workspace, window, cx);
        let mut subscriptions = vec![
            // Forget the window when it closes, so the board opens anew.
            cx.on_release({
                let (id, me) = (id.clone(), cx.entity_id());
                move |this, cx| {
                    let windows = cx.default_global::<BoardWindows>();
                    if windows
                        .open
                        .get(&id)
                        .is_some_and(|(_, shown)| shown.entity_id() == me)
                    {
                        windows.open.remove(&id);
                    }
                    this.remember_chat(cx);
                }
            }),
        ];
        if let Some(boards) = Boards::global(cx) {
            subscriptions.push(cx.subscribe_in(
                &boards,
                window,
                |this, _, event: &BoardEvent, window, cx| {
                    if event.id == this.id {
                        this.told(&event.what, window, cx);
                    }
                },
            ));
        }
        if let Some(artifacts) = Artifacts::global(cx) {
            // Renamed in the list, given to another workspace, or gone.
            subscriptions.push(
                cx.observe_in(&artifacts, window, |this, artifacts, window, cx| {
                    let found = artifacts.read(cx).find(&this.id).cloned();
                    match found {
                        Some(artifact) => {
                            // Given to another workspace, or let go of one,
                            // from elsewhere (or its workspace is gone):
                            // the chat beside it follows.
                            let from = workspace_of(&artifact, cx);
                            if !this.noting
                                && from.as_ref().map(|project| project.id)
                                    != this.workspace.as_ref().map(|(id, _)| *id)
                            {
                                this.work_from(from, window, cx);
                            }
                            if artifact.manifest.title != this.title {
                                this.title = artifact.manifest.title;
                                window.set_window_title(&format!("{} — Whiteboard", this.title));
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
        let remembered = cx.try_global::<BoardWindows>();
        let chat_shown = remembered
            .and_then(|windows| windows.chat_shown)
            .unwrap_or(true);
        let chat_width = remembered
            .and_then(|windows| windows.chat_width)
            .unwrap_or(CHAT_WIDTH);
        Self {
            id,
            title,
            board,
            focus: cx.focus_handle(),
            covered: false,
            opening: false,
            drawn: false,
            chat,
            workspace: linked,
            noting: false,
            _chat: chat_events,
            chat_shown,
            chat_width,
            _subscriptions: subscriptions,
        }
    }

    /// What the chat beside board `artifact` is told and offers, in
    /// `project`'s space: the board's own (working in the board's folder),
    /// or, with `workspace`, a chat of the workspace the board belongs to
    /// (working in its folders, and changing the board's file as the
    /// board's own chat does).
    fn context(artifact: &Artifact, project: &Project, workspace: bool) -> ChatContext {
        let instructions: Instructions = {
            let (id, board) = (artifact.id().to_string(), artifact.folder.clone());
            Rc::new(move |cx: &App| {
                let mut text = instructions(&id, &board, cx);
                if workspace {
                    // The workspace may have other boards: this one is named.
                    text.push_str(&format!(
                        " This is the workspace's board `{id}`: name it (`board`) when you call board_look."
                    ));
                }
                text
            })
        };
        let subtitle = if workspace {
            format!(
                "This is {}'s chat: the agent works in its folders and on this board. Say what the board should show, or select something on it and say \"this\". Each change shows at once and can be undone.",
                project.name
            )
        } else {
            "Say what the board should show: a flow, a plan, the parts of a system. Select something on the board first and say \"this\". Each change shows at once and can be undone.".into()
        };
        ChatContext {
            title: heading(&artifact.manifest.title),
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
            placeholder: "Describe what to draw, or the change: a flow of the sign-up, group these, add what is missing…".into(),
            // A workspace's chat works where the workspace is; the board's
            // own, in the board's folder.
            cwd: if workspace {
                project.path.clone()
            } else {
                artifact.folder.clone()
            },
            prepare: None,
            instructions: Some(instructions),
            // In the board's own chat the app's tools are the board's. A
            // workspace's chat keeps its own rules for them: the board's
            // tools and file are let through for every chat of a workspace
            // with a board (`ph_agent::own_files`).
            app_tools: !workspace,
            // The board's own file: what the agent changes there shows at
            // once and is undone on the board.
            own_files: Some(artifact.site()),
        }
    }

    /// The chat beside board `artifact`, in `project`'s space
    /// ([`Self::context`]).
    fn chat_for(
        artifact: &Artifact,
        store: &Store,
        project: Project,
        workspace: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<ChatPanel>, Subscription) {
        let id = artifact.id().to_string();
        let space = project.id;
        let context = Self::context(artifact, &project, workspace);
        let chat = cx.new(|cx| ChatPanel::new(store.clone(), project, window, cx));
        // A workspace has many chats: the one the board was made or last
        // worked on in is the one shown.
        let session = workspace
            .then(|| crate::artifacts::chat_of(ph_boards::board::KIND, &id, space, cx))
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

    /// A workspace has many chats: the one the board is worked on in is
    /// the one it opens with next time.
    fn remember_chat(&self, cx: &App) {
        if self.workspace.is_some() {
            let chat = self.chat.read(cx).active_session_id(cx);
            crate::artifacts::remember_chat(ph_boards::board::KIND, &self.id, chat, cx);
        }
    }

    /// The board works from workspace `to` from here on (`None`: from
    /// none, in its own chat): the board is noted as that workspace's, and
    /// the chat beside it becomes that workspace's.
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
        // The chat it leaves keeps its place for the next time.
        self.remember_chat(cx);
        let source = to.as_ref().map(|workspace| ph_artifacts::library::Source {
            project: workspace.id,
            workspace: workspace.name.clone(),
            path: workspace.path.clone(),
        });
        let noted = artifacts.update(cx, |artifacts, cx| {
            artifacts.set_source(&self.id, source, cx)
        });
        self.noting = true;
        cx.spawn(async move |this, cx| {
            if let Err(err) = noted.await {
                tracing::warn!("a board's workspace could not be noted: {err:#}");
            }
            _ = this.update(cx, |this, _| this.noting = false);
        })
        .detach();
        self.workspace = to
            .as_ref()
            .map(|workspace| (workspace.id, workspace.name.clone()));
        self.rebuild_chat(&artifact, window, cx);
    }

    /// The board's folder took another name. A workspace's chat stays as it
    /// is (what is typed, where it is read), and is told where the board
    /// is now; the board's own chat works in that folder, and is made anew
    /// there.
    fn folder_moved(&mut self, artifact: &Artifact, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = self
            .workspace
            .as_ref()
            .and_then(|(id, _)| crate::AppState::store(cx).project(*id).ok().flatten());
        match workspace {
            Some(workspace) => {
                let context = Self::context(artifact, &workspace, true);
                self.chat
                    .update(cx, |chat, cx| chat.set_context(Some(context), window, cx));
            }
            None => self.rebuild_chat(artifact, window, cx),
        }
    }

    /// The chat beside the board, made anew for `artifact` as it is now:
    /// in the workspace the board works from, or in the board's own space
    /// (kept by its folder).
    fn rebuild_chat(&mut self, artifact: &Artifact, window: &mut Window, cx: &mut Context<Self>) {
        let store = crate::AppState::store(cx).clone();
        let workspace = self
            .workspace
            .as_ref()
            .and_then(|(id, _)| store.project(*id).ok().flatten());
        let project = match workspace {
            Some(workspace) => workspace,
            None => match store.presentation_space(&artifact.manifest.title, &artifact.folder) {
                Ok(own) => own,
                Err(err) => {
                    tracing::warn!("the board's own chat could not be opened: {err:#}");
                    return;
                }
            },
        };
        let (chat, events) = Self::chat_for(
            artifact,
            &store,
            project,
            self.workspace.is_some(),
            window,
            cx,
        );
        self.chat = chat;
        self._chat = events;
        cx.notify();
    }

    /// What the board's page told the app.
    fn told(&mut self, what: &Value, window: &mut Window, cx: &mut Context<Self>) {
        if what.get("focus").is_some() {
            // The board's page took the keyboard (a click in it).
            window.focus(&self.focus, cx);
        } else if what.get("share").is_some() {
            // The board steps aside first, so that the dialog is over it
            // from its first frame.
            self.opening = true;
            let aside = self.board.update(cx, |board, cx| board.cover(cx));
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
        } else if let Some(ask) = what.get("ask") {
            self.send_note(ask, window, cx);
        } else if let Some(error) = what["error"].as_str() {
            // A web view has no console anyone reads.
            tracing::warn!(
                "a board's page failed: {error} ({})",
                what["at"].as_str().unwrap_or_default()
            );
        }
    }

    /// A note the user pinned to the board and sent to the agent: a message
    /// in the chat, saying what it is about and how to answer it.
    fn send_note(&mut self, ask: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let text = |key: &str| ask[key].as_str().unwrap_or_default();
        let said = text("text");
        let prompt = format!(
            "Note {id} pinned to the board, about {about}:\n\n{said}\n\nDo what it asks on the board, then answer it in board.json: add `{{\"by\": \"Agent\", \"text\": …}}` to that comment's `replies`, saying what you changed, and set its `resolved` to true once it is done.",
            id = text("id"),
            about = text("about"),
        );
        if !self.chat_shown {
            self.toggle_chat(cx);
        }
        self.chat.update(cx, |chat, cx| {
            chat.send_aside(prompt, Some(format!("Note: {said}")), window, cx)
        });
    }

    fn toggle_chat(&mut self, cx: &mut Context<Self>) {
        self.chat_shown = !self.chat_shown;
        cx.default_global::<BoardWindows>().chat_shown = Some(self.chat_shown);
        cx.notify();
    }

    /// The chat's edge dragged: the chat takes what is left of the pointer,
    /// leaving the board room.
    fn drag_edge(&mut self, event: &DragMoveEvent<ChatEdge>, cx: &mut Context<Self>) {
        let left = f32::from(event.bounds.origin.x);
        let widest = (f32::from(event.bounds.size.width) - BOARD_MIN).max(CHAT_MIN);
        let width = (f32::from(event.event.position.x) - left).clamp(CHAT_MIN, widest);
        if (width - self.chat_width).abs() >= 0.5 {
            self.chat_width = width;
            cx.default_global::<BoardWindows>().chat_width = Some(width);
            cx.notify();
        }
    }
}

fn heading(title: &str) -> String {
    format!("Work on {title} with an agent")
}

/// What the agent is told with each message in a board's chat: what a
/// board is, where this one is, what the user has selected, and whether
/// they changed it by hand since the agent last heard.
fn instructions(id: &str, folder: &Path, cx: &App) -> String {
    let app = ph_brand::NAME;
    let file: PathBuf = folder.join("site").join(ph_boards::board::BOARD);
    let mut text = format!(
        "[{app} Whiteboard] The user is working on a whiteboard beside this chat: one file, {file}, that lists what is on the board (notes, shapes, text, sections, arrows, tables). Change the board by editing that file with your own file tools: the user sees each change at once and can undo it. Read board_guide before your first change to the board in this chat, and call board_look to see what is on it.",
        file = file.display(),
    );
    let Some(boards) = Boards::global(cx) else {
        return text;
    };
    let boards = boards.read(cx);
    let selected: Vec<String> = boards
        .selection(id)
        .and_then(|selection| selection["ids"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect();
    if selected.is_empty() {
        text.push_str(" Nothing on it is selected.");
    } else {
        text.push_str(&format!(
            " Selected on the board: {} (ids in board.json); \"this\" or \"these\" in the user's message means that.",
            selected.join(", ")
        ));
    }
    if boards.take_by_hand(id) > 0 {
        text.push_str(
            " Since your last turn the user changed the board by hand: read board.json again before you change it.",
        );
    }
    text
}

impl Render for BoardWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The system draws the board's web view over everything the app
        // draws: while something of the app's may be there, it steps aside.
        let drawn = std::mem::replace(&mut self.drawn, true);
        let covered = self.opening || overlay_open(drawn, window, cx);
        if covered != self.covered {
            self.covered = covered;
            let board = self.board.clone();
            cx.defer(move |cx| {
                board.update(cx, |board, cx| {
                    if covered {
                        board.cover(cx).detach();
                    } else {
                        board.uncover(cx);
                    }
                })
            });
        }
        let theme = cx.theme().clone();
        let chat_width = self.chat_width;
        let chat = self.chat_shown.then(|| {
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
                        .id("board-chat-edge")
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
                            div().flex_none().occlude().child(
                                Button::new("board-chat")
                                    .ghost()
                                    .small()
                                    .icon(Icons::MessageSquare)
                                    .selected(self.chat_shown)
                                    .accessibility_label("Show or hide the chat")
                                    .tooltip(ph_ui::tip(
                                        "Show or hide the chat: say what the board should show and an agent draws it",
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_chat(cx))),
                            ),
                        )
                        .child(
                            Icon::new(Icons::StickyNote)
                                .small()
                                .text_color(theme.muted_foreground)
                                .flex_none(),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Whiteboard"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("·"),
                        )
                        .child(div().min_w_0().truncate().text_sm().child(self.title.clone()))
                        .child(div().flex_1().min_w_2())
                        .child(div().flex_none().occlude().child(workspace_button(
                            "board",
                            self.workspace.as_ref(),
                            Self::work_from,
                            cx,
                        ))),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .children(chat)
                    .child(div().flex_1().min_w_0().h_full().child(self.board.clone())),
            )
    }
}

/// The app's own tests of a board's window (development builds only, like
/// the `ui.*` methods): `board.screen` is a picture of the page as the
/// window shows it, `board.script` runs a script in it, `board.keys` says
/// whether the page has the keyboard (`take` gives it first; `press` types
/// a key as the system does). `id` names the board when several are open.
#[cfg(feature = "automation")]
pub fn register_automation(cx: &mut App) {
    use base64::Engine as _;
    use ph_control::ControlServer;
    use serde_json::json;

    fn view(params: &Value, cx: &App) -> Result<Entity<DeckView>> {
        let windows = cx
            .try_global::<BoardWindows>()
            .context("no board's window is open")?;
        let shown = |(_, window): &(AnyWindowHandle, WeakEntity<BoardWindow>)| {
            Some(window.upgrade()?.read(cx).board.clone())
        };
        match params["id"].as_str() {
            Some(id) => windows.open.get(id).and_then(shown),
            None => windows.open.values().find_map(shown),
        }
        .context("the board's window is not open")
    }

    ControlServer::handle_async(
        "board.screen",
        |params, cx| {
            let picture = match view(&params, cx) {
                Ok(view) => view.read(cx).screen(cx),
                Err(err) => return gpui_kit::Task::ready(Err(err)),
            };
            cx.background_spawn(async move {
                let png = picture.await?;
                Ok(json!({ "images": [{
                    "data": base64::engine::general_purpose::STANDARD.encode(png),
                    "mimeType": "image/png",
                }] }))
            })
        },
        cx,
    );
    ControlServer::handle(
        "board.script",
        |params, cx| {
            let view = view(&params, cx)?;
            view.read(cx)
                .evaluate(params["script"].as_str().context("missing `script`")?, cx);
            Ok(json!({}))
        },
        cx,
    );
    ControlServer::handle(
        "board.keys",
        |params, cx| {
            let view = view(&params, cx)?;
            if params["take"].as_bool().unwrap_or(false) {
                view.read(cx).focus(cx);
            }
            if let Some(characters) = params["press"].as_str() {
                let flag = |name: &str| params[name].as_bool().unwrap_or(false);
                let key = ph_webview::Key {
                    characters,
                    plain: params["plain"].as_str().unwrap_or(characters),
                    code: params["code"].as_u64().unwrap_or_default() as u16,
                    command: flag("command"),
                    shift: flag("shift"),
                    function: flag("function"),
                };
                let front = view.read(cx).press(&key, flag("front"), cx)?;
                return Ok(json!({ "web": view.read(cx).has_keys(cx), "front": front }));
            }
            Ok(json!({ "web": view.read(cx).has_keys(cx) }))
        },
        cx,
    );
}
