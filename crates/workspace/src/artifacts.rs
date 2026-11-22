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
    if let Some(decks) = ph_slides::Decks::global(cx) {
        // A deck made from a workspace's chat, and its page asking for the
        // deck's own window.
        cx.subscribe(&decks, |_, event: &ph_slides::DeckEvent, cx| {
            deck_said(event, cx)
        })
        .detach();
        if let Some(artifacts) = ph_artifacts::Artifacts::global(cx) {
            // Which decks belong to which workspace changes with the list.
            cx.observe(&artifacts, |artifacts, cx| trust_decks(&artifacts, cx))
                .detach();
            trust_decks(&artifacts, cx);
        }
    }
}

/// The setting that keeps which chat a deck was last worked on in.
fn chat_key(deck: &str) -> String {
    format!("slides.chat.{deck}")
}

/// Notes the chat deck `deck` is worked on in (a workspace's chat).
pub(crate) fn remember_deck_chat(deck: &str, session: Option<i64>, cx: &App) {
    if let Some(session) = session
        && let Err(err) = AppState::store(cx).set_setting(&chat_key(deck), &session)
    {
        tracing::warn!("a deck's chat could not be noted: {err:#}");
    }
}

/// The chat deck `deck` was last worked on in, while it is still one of
/// workspace `project`'s.
pub(crate) fn deck_chat(deck: &str, project: i64, cx: &App) -> Option<ph_store::sessions::Session> {
    let store = AppState::store(cx);
    let session: Option<i64> = store.setting(&chat_key(deck));
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
        // No chat is at work: none is known to have asked, and the deck
        // opens on a new one rather than on whichever was looked at.
        [] => None,
        several => chats
            .viewing()
            .filter(|shown| several.contains(shown))
            .or(several.first().copied()),
    }
}

/// The folders the chats of each workspace change without asking: the
/// files of the decks made from it.
fn trust_decks(artifacts: &gpui_kit::Entity<ph_artifacts::Artifacts>, cx: &mut App) {
    let mut folders: std::collections::HashMap<i64, Vec<PathBuf>> = Default::default();
    for artifact in artifacts.read(cx).all() {
        if artifact.manifest.kind == ph_slides::deck::KIND
            && let Some(source) = &artifact.manifest.source
        {
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

/// What the decks said that the windows act on.
fn deck_said(event: &ph_slides::DeckEvent, cx: &mut App) {
    let Some(artifacts) = ph_artifacts::Artifacts::global(cx) else {
        return;
    };
    if let Some(project) = event.what["made"]["project"].as_i64() {
        // A workspace's agent made it: the chat that asked is the deck's
        // from here on, and the deck shows beside that chat as it is
        // written (its own window is the user's to open).
        trust_decks(&artifacts, cx);
        remember_deck_chat(&event.id, chat_at_work(project, cx), cx);
        let id = event.id.clone();
        let address = artifacts.update(cx, |artifacts, cx| artifacts.address(&id, cx));
        cx.spawn(async move |cx| {
            let Ok(address) = address.await else {
                return;
            };
            cx.update(|cx| {
                if enabled(Feature::Browser) {
                    show(project, format!("{address}?preview=1"), cx).detach();
                } else if let Some(deck) = artifacts.read(cx).find(&id).cloned() {
                    // No pane to show it in: its own window, then.
                    _ = crate::slides_window::open(&deck, cx);
                }
            });
        })
        .detach();
    } else if event.what.get("open").is_some()
        && let Some(deck) = artifacts.read(cx).find(&event.id).cloned()
        && let Err(err) = crate::slides_window::open(&deck, cx)
    {
        tracing::warn!("a deck could not be opened: {err:#}");
    }
}

/// An artifact was renamed in the list. A deck's chats are kept by its
/// folder, which took the new name; and the deck says the name of itself.
fn renamed(artifact: &ph_artifacts::Artifact, before: &std::path::Path, cx: &mut App) {
    if artifact.manifest.kind != ph_slides::deck::KIND {
        return;
    }
    if artifact.folder != before
        && let Err(err) = AppState::store(cx).move_presentation_space(before, &artifact.folder)
    {
        tracing::warn!("a deck's chats could not follow its folder: {err:#}");
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
