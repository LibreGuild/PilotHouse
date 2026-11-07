//! Agents' models, loaded in the background a few seconds after start, so
//! the model picker lists them the first time it opens.
//!
//! An agent says which models, modes and efforts it offers when a session
//! starts (ACP's session options). For each installed agent whose options
//! were never cached, or were cached more than a day ago (found at start,
//! or installed while PilotHouse runs), one at a time,
//! PilotHouse starts the agent in its own neutral folder, asks for a new
//! session, caches what it offers (as a chat would) and stops it: no chat
//! is made and nothing is prompted. An agent that does not answer within
//! [`TIMEOUT`] or fails (not signed in, offline) is left for this run; the
//! picker then says why, and its *Load models* row still loads them the
//! usual way. Claude Code and Codex run through an npm adapter, which `npx`
//! downloads on the first start.

use std::collections::HashMap;
use std::time::Duration;

use futures::FutureExt as _;
use gpui_kit::{App, AppContext as _, AsyncApp, Global, SharedString, Task};
use ph_store::Store;
use serde_json::json;

use crate::agents::AgentSpec;
use crate::connection::AgentConnection;
use crate::options::{self, AgentOption};

/// After the window is up and the startup work done.
const DELAY: Duration = Duration::from_secs(4);
/// The longest one agent may take: the first start downloads its adapter.
pub const TIMEOUT: Duration = Duration::from_secs(90);
/// Options cached longer ago than this are loaded again.
const STALE_MS: i64 = 24 * 60 * 60 * 1000;

/// Where an agent's models stand while they load in the background.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadState {
    Loading,
    Failed(SharedString),
}

#[derive(Default)]
pub struct ModelLoads {
    states: HashMap<&'static str, LoadState>,
    running: bool,
    _task: Option<Task<()>>,
}

impl Global for ModelLoads {}

impl ModelLoads {
    /// Agent `id`'s background load, while loading or after it failed.
    pub fn state(id: &str, cx: &App) -> Option<LoadState> {
        cx.try_global::<Self>()?.states.get(id).cloned()
    }

    /// Records agent `id`'s state; observers (the chat panels) hear of it.
    fn set(id: &'static str, state: Option<LoadState>, cx: &mut App) {
        cx.default_global::<Self>();
        let states = &mut cx.global_mut::<Self>().states;
        match state {
            Some(state) => states.insert(id, state),
            None => states.remove(id),
        };
    }
}

/// Whether an agent's options should be loaded: never cached (or cached by
/// a version that kept no date), or cached longer ago than a day.
pub fn needs_models(cached_at: Option<i64>, now: i64) -> bool {
    cached_at.is_none_or(|at| now.saturating_sub(at) > STALE_MS)
}

/// Loads the models of the `installed` agents that need it, one at a
/// time, after [`DELAY`]. An agent already tried in this run (loading, or
/// failed) is not tried again: its picker row loads it on request.
pub(crate) fn start(store: Store, installed: Vec<&'static AgentSpec>, cx: &mut App) {
    if cx
        .try_global::<ModelLoads>()
        .is_some_and(|loads| loads.running)
    {
        return;
    }
    let now = options::unix_millis();
    let due: Vec<&'static AgentSpec> = installed
        .into_iter()
        .filter(|spec| {
            ModelLoads::state(spec.id, cx).is_none()
                && needs_models(options::cached_at(&store, spec.id), now)
        })
        .collect();
    if due.is_empty() {
        return;
    }
    // Queued ones show as loading too: they will be, in a moment.
    for spec in &due {
        ModelLoads::set(spec.id, Some(LoadState::Loading), cx);
    }
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(DELAY).await;
        for spec in due {
            let env = if spec.id == "cline" {
                crate::cline::env_for(&store)
            } else {
                Vec::new()
            };
            let result = load(spec, env, cx).await;
            cx.update(|cx| match result {
                Ok(offered) => {
                    tracing::info!("loaded {}'s options ({})", spec.name, offered.len());
                    options::cache(&store, spec.id, &offered);
                    ModelLoads::set(spec.id, None, cx);
                }
                Err(reason) => {
                    tracing::info!("loading {}'s models failed: {reason}", spec.name);
                    ModelLoads::set(spec.id, Some(LoadState::Failed(reason)), cx);
                }
            });
        }
        // An agent installed later is loaded by the next look.
        cx.update(|cx| cx.default_global::<ModelLoads>().running = false);
    });
    let loads = cx.default_global::<ModelLoads>();
    loads.running = true;
    loads._task = Some(task);
}

/// Starts `spec`, asks it for a new session in a neutral folder, and stops
/// it: what the session offers.
async fn load(
    spec: &'static AgentSpec,
    env: Vec<(String, String)>,
    cx: &mut AsyncApp,
) -> Result<Vec<AgentOption>, SharedString> {
    // The folder folderless chats use: nothing of the user's for the agent
    // to read, and no project's settings or hooks to run.
    let cwd = ph_store::paths::data_dir().join("agents");
    let _ = std::fs::create_dir_all(&cwd);
    let connection = cx.new(|cx| AgentConnection::start_with(spec, cwd.clone(), env, cx));
    let ready = cx.update(|cx| connection.read(cx).ready());
    let session = async move {
        let ready = ready.await?;
        let result = ready
            .process
            .request("session/new", json!({ "cwd": cwd, "mcpServers": [] }))
            .await
            .map_err(|err| SharedString::from(format!("{err:#}")))?;
        Ok::<_, SharedString>(options::parse_session_options(&result))
    };
    let timeout = cx.background_executor().timer(TIMEOUT);
    let result = futures::select_biased! {
        result = session.fuse() => result,
        _ = timeout.fuse() => Err(SharedString::from(format!(
            "{} did not answer within {} seconds",
            spec.name,
            TIMEOUT.as_secs()
        ))),
    };
    // Its process ends with the connection.
    drop(connection);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_what_was_never_cached_or_is_a_day_old() {
        let now = 10 * STALE_MS;
        assert!(needs_models(None, now));
        assert!(!needs_models(Some(now - 1000), now));
        assert!(!needs_models(Some(now - STALE_MS), now));
        assert!(needs_models(Some(now - STALE_MS - 1), now));
        // A clock set back is not a reason to load again and again.
        assert!(!needs_models(Some(now + 5000), now));
    }
}
