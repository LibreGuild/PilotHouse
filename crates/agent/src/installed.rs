//! Which agents are installed: looked for at start, off the UI thread, and
//! again on request (*Check again*, or while the first-run card waits for
//! one), so new chats start with an agent that works and a first run is
//! greeted with the agents to choose from rather than told one is missing.
//!
//! The finding itself lives in [`crate::agents::installed`]; views observe
//! the [`Installed`] global to hear that it changed.

use gpui_kit::{App, AppContext as _, Global, Task};
use ph_store::Store;

use crate::agents;

#[derive(Default)]
pub struct Installed {
    /// A look in progress (only one at a time).
    looking: Option<Task<()>>,
    /// Where the models of agents found are cached (set by [`install`]).
    store: Option<Store>,
}

impl Global for Installed {}

/// Looks for the agents now, in the background, then loads the models of
/// those installed ([`crate::preload`]), as every later look does. Call
/// once at startup.
pub fn install(store: Store, cx: &mut App) {
    if cx.has_global::<Installed>() {
        return;
    }
    cx.set_global(Installed {
        looking: None,
        store: Some(store),
    });
    look(cx);
}

/// Looks again: an agent may have been installed since. Observers of
/// [`Installed`] hear when it is done.
pub fn recheck(cx: &mut App) {
    look(cx);
}

/// Whether a look is under way (the first one waits for the login shell).
pub fn looking(cx: &App) -> bool {
    cx.try_global::<Installed>()
        .is_some_and(|installed| installed.looking.is_some())
}

/// Looks, then loads the models of what was found that need it (an agent
/// installed while PilotHouse runs too).
fn look(cx: &mut App) {
    if looking(cx) {
        return;
    }
    let found = cx.background_spawn(async { agents::detect() });
    let task = cx.spawn(async move |cx| {
        let found = found.await;
        tracing::info!(
            "agents installed: {}",
            found
                .iter()
                .map(|spec| spec.id)
                .collect::<Vec<_>>()
                .join(", ")
        );
        cx.update(|cx| {
            // Telling observers: the finding is in `agents::installed`.
            let installed = cx.global_mut::<Installed>();
            installed.looking = None;
            if let Some(store) = installed.store.clone() {
                crate::preload::start(store, found, cx);
            }
        });
    });
    cx.default_global::<Installed>().looking = Some(task);
}
