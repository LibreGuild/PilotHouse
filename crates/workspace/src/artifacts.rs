//! Artifacts, seen from the window shell: what `ph_artifacts` cannot do by
//! itself and is lent here (its [`Host`]). Which folders an agent of a
//! workspace may publish from; showing a published page in that workspace's
//! built-in browser, beside the chat that made it; and bringing a workspace
//! in front with an artifact, to go on working on it.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use gpui_kit::{App, Task, Window};
use ph_artifacts::{Host, NewKind};
use ph_brand::{Feature, enabled};

use crate::AppState;

/// Starts the artifacts where the build has them. Call once at startup.
pub(crate) fn install(cx: &mut App) {
    if !enabled(Feature::Artifacts) {
        return;
    }
    ph_artifacts::init(cx);
    // Slides are artifacts of a kind of their own, with a window of their
    // own: made and opened from the list.
    let mut kinds = Vec::new();
    if enabled(Feature::Presentations) {
        ph_slides::init(cx);
        kinds.push(NewKind {
            kind: ph_slides::deck::KIND,
            label: "Slides",
            hint: "a deck made and changed beside a chat",
            icon: ph_ui::Icons::Presentation,
            art: Rc::new(|width, _, cx| ph_slides::card_art(width, cx)),
            create: Rc::new(crate::slides_window::create),
            open: Rc::new(|artifact, window, cx| {
                if let Err(err) = crate::slides_window::open(artifact, cx) {
                    use gpui_kit::component::WindowExt as _;
                    window.push_notification(
                        gpui_kit::component::notification::Notification::error(format!(
                            "The deck could not be opened: {err:#}"
                        )),
                        cx,
                    );
                }
            }),
        });
    }
    // So are whiteboards.
    if enabled(Feature::Whiteboards) {
        ph_boards::init(cx);
        kinds.push(NewKind {
            kind: ph_boards::board::KIND,
            label: "Whiteboard",
            hint: "notes, shapes and arrows on an endless board, beside a chat",
            icon: ph_ui::Icons::StickyNote,
            art: Rc::new(|width, _, cx| ph_boards::card_art(width, cx)),
            create: Rc::new(crate::board_window::create),
            open: Rc::new(|artifact, window, cx| {
                if let Err(err) = crate::board_window::open(artifact, cx) {
                    use gpui_kit::component::WindowExt as _;
                    window.push_notification(
                        gpui_kit::component::notification::Notification::error(format!(
                            "The board could not be opened: {err:#}"
                        )),
                        cx,
                    );
                }
            }),
        });
    }
    ph_artifacts::set_host(
        Host {
            workspace: Rc::new(workspace),
            show: Rc::new(show),
            open_in_workspace: Rc::new(open_in_workspace),
            trash: ph_files::file_ops::trash,
            renamed,
            kinds,
        },
        cx,
    );
    // A deck or a board made from a workspace's chat, and its page asking
    // for its own window.
    let decks = ph_slides::Decks::global(cx);
    let boards = ph_boards::Boards::global(cx);
    if let Some(decks) = &decks {
        cx.subscribe(decks, |_, event: &ph_slides::DeckEvent, cx| {
            said(Made::DECK, &event.id, &event.what, cx)
        })
        .detach();
    }
    if let Some(boards) = &boards {
        cx.subscribe(boards, |_, event: &ph_boards::BoardEvent, cx| {
            said(Made::BOARD, &event.id, &event.what, cx)
        })
        .detach();
    }
    if (decks.is_some() || boards.is_some())
        && let Some(artifacts) = ph_artifacts::Artifacts::global(cx)
    {
        // Which of them belong to which workspace changes with the list.
        cx.observe(&artifacts, |artifacts, cx| trust_made(&artifacts, cx))
            .detach();
        trust_made(&artifacts, cx);
    }
}

/// A kind of artifact a workspace's agent makes for it and its chats then
/// work on (a deck, a whiteboard): what it is, and its own window.
#[derive(Clone, Copy)]
struct Made {
    kind: &'static str,
    open: fn(&ph_artifacts::Artifact, &mut App) -> Result<()>,
}

impl Made {
    const DECK: Self = Self {
        kind: ph_slides::deck::KIND,
        open: crate::slides_window::open,
    };
    const BOARD: Self = Self {
        kind: ph_boards::board::KIND,
        open: crate::board_window::open,
    };
}

/// The setting that keeps which chat a deck or a board (`kind`) was last
/// worked on in.
fn chat_key(kind: &str, id: &str) -> String {
    format!("{kind}.chat.{id}")
}

/// Notes the chat the deck or board `id` is worked on in (a workspace's
/// chat).
pub(crate) fn remember_chat(kind: &str, id: &str, session: Option<i64>, cx: &App) {
    if let Some(session) = session
        && let Err(err) = AppState::store(cx).set_setting(&chat_key(kind, id), &session)
    {
        tracing::warn!("an artifact's chat could not be noted: {err:#}");
    }
}

/// The chat the deck or board `id` was last worked on in, while it is
/// still one of workspace `project`'s.
pub(crate) fn chat_of(
    kind: &str,
    id: &str,
    project: i64,
    cx: &App,
) -> Option<ph_store::sessions::Session> {
    let store = AppState::store(cx);
    let session: Option<i64> = store.setting(&chat_key(kind, id));
    store
        .session(session?)
        .ok()
        .flatten()
        .filter(|session| session.project_id == project && session.kind != "terminal")
}

/// The chat of workspace `project` that is asking its agent something now:
/// the one at work (the one on screen when several are).
fn chat_at_work(project: i64, cx: &App) -> Option<i64> {
    use ph_agent::{ChatHub, ChatState};
    let chats = ChatHub::existing(project, cx)?;
    let chats = chats.read(cx);
    let working: Vec<i64> = chats
        .live(cx)
        .into_iter()
        .filter(|(_, chat)| matches!(chat.state, ChatState::Working | ChatState::NeedsYou))
        .map(|(session, _)| session)
        .collect();
    match working.as_slice() {
        [one] => Some(*one),
        // No chat is at work: none is known to have asked, and the artifact
        // opens on a new one rather than on whichever was looked at.
        [] => None,
        several => chats
            .viewing()
            .filter(|shown| several.contains(shown))
            .or(several.first().copied()),
    }
}

/// The folders the chats of each workspace change without asking: the
/// files of the decks and the boards made from it.
fn trust_made(artifacts: &gpui_kit::Entity<ph_artifacts::Artifacts>, cx: &mut App) {
    let mut folders: std::collections::HashMap<i64, Vec<PathBuf>> = Default::default();
    for artifact in artifacts.read(cx).all() {
        // (Of the kinds this build has: one switched off has no way in.)
        let kind = artifact.manifest.kind.as_str();
        let made = (kind == ph_slides::deck::KIND && enabled(Feature::Presentations))
            || (kind == ph_boards::board::KIND && enabled(Feature::Whiteboards));
        if made && let Some(source) = &artifact.manifest.source {
            folders
                .entry(source.project)
                .or_default()
                .push(artifact.site());
        }
    }
    let kept: Vec<i64> = folders.keys().copied().collect();
    for (project, folders) in folders {
        ph_agent::own_files::set(project, folders, cx);
    }
    ph_agent::own_files::keep_only(&kept, cx);
}

/// What the decks or the boards said that the windows act on (`what`,
/// about the one with the id `id`).
fn said(made: Made, id: &str, what: &serde_json::Value, cx: &mut App) {
    let Some(artifacts) = ph_artifacts::Artifacts::global(cx) else {
        return;
    };
    if let Some(project) = what["made"]["project"].as_i64() {
        // A workspace's agent made it: the chat that asked is its chat
        // from here on, and it shows beside that chat as it is written
        // (its own window is the user's to open).
        trust_made(&artifacts, cx);
        remember_chat(made.kind, id, chat_at_work(project, cx), cx);
        let id = id.to_string();
        let address = artifacts.update(cx, |artifacts, cx| artifacts.address(&id, cx));
        cx.spawn(async move |cx| {
            let Ok(address) = address.await else {
                return;
            };
            cx.update(|cx| {
                if enabled(Feature::Browser) {
                    show(project, format!("{address}?preview=1"), cx).detach();
                } else if let Some(artifact) = artifacts.read(cx).find(&id).cloned() {
                    // No pane to show it in: its own window, then.
                    _ = (made.open)(&artifact, cx);
                }
            });
        })
        .detach();
    } else if what.get("open").is_some()
        && let Some(artifact) = artifacts.read(cx).find(id).cloned()
        && artifact.manifest.kind == made.kind
        && let Err(err) = (made.open)(&artifact, cx)
    {
        tracing::warn!("an artifact's window could not be opened: {err:#}");
    }
}

/// An artifact was renamed in the list. The chats of a deck or a
/// whiteboard are kept by its folder, which took the new name; and a deck
/// says the name of itself.
fn renamed(artifact: &ph_artifacts::Artifact, before: &std::path::Path, cx: &mut App) {
    let kind = artifact.manifest.kind.as_str();
    if kind != ph_slides::deck::KIND && kind != ph_boards::board::KIND {
        return;
    }
    if artifact.folder != before
        && let Err(err) = AppState::store(cx).move_presentation_space(before, &artifact.folder)
    {
        tracing::warn!("an artifact's chats could not follow its folder: {err:#}");
    }
    if kind == ph_boards::board::KIND {
        if let Some(boards) = ph_boards::Boards::global(cx) {
            boards.read(cx).renamed(artifact);
        }
        crate::board_window::moved(artifact, cx);
        return;
    }
    if let Some(decks) = ph_slides::Decks::global(cx) {
        decks.read(cx).renamed(artifact);
    }
}

/// A workspace's name, and where an agent of it works: its folders (the
/// scratch folder of one without), and the copies its tasks run in.
fn workspace(project: i64, cx: &App) -> Option<(String, Vec<PathBuf>)> {
    let store = AppState::store(cx);
    let project = store.project(project).ok().flatten()?;
    let mut folders = project.roots();
    folders.extend(
        store
            .sessions(project.id)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|session| session.cwd)
            .map(PathBuf::from),
    );
    Some((project.name, folders))
}

/// Shows `url` in the workspace's built-in browser, its tab in front, and
/// answers with a picture of the page for the Artifacts list.
fn show(project: i64, url: String, cx: &mut App) -> Task<Result<Vec<u8>>> {
    if !enabled(Feature::Browser) {
        return Task::ready(Err(anyhow!("this build has no built-in browser")));
    }
    crate::browser_inbox::show_browser(project, cx);
    let browser = ph_browser::BrowserHub::browser(project, cx);
    // A page that is open already reloads by itself on a new publish.
    let there = browser.read(cx).url().starts_with(&url);
    let opened = (!there).then(|| browser.update(cx, |browser, cx| browser.navigate(&url, cx)));
    cx.spawn(async move |cx| {
        if let Some(opened) = opened {
            opened.await?;
        }
        // Time to draw (or, open already, to reload) before the picture.
        let wait = Duration::from_millis(if there { 1500 } else { 600 });
        cx.background_executor().timer(wait).await;
        browser
            .update(cx, |browser, cx| browser.capture_png(cx))
            .await
    })
}

/// The user asked for an artifact beside the chat of the workspace it came
/// from: that workspace in front, its Browser tab showing the page.
fn open_in_workspace(project: i64, url: String, window: &mut Window, cx: &mut App) {
    let Some(workspace) = crate::windows::workspace_of(window.window_handle(), cx) else {
        return;
    };
    workspace.update(cx, |workspace, cx| {
        workspace.open_project(project, window, cx)
    });
    // Once the workspace is in front (it may be in another window).
    cx.defer(move |cx| {
        if !enabled(Feature::Browser) {
            return;
        }
        crate::browser_inbox::show_browser(project, cx);
        ph_browser::BrowserHub::browser(project, cx)
            .update(cx, |browser, cx| browser.open(&url, cx));
    });
}
