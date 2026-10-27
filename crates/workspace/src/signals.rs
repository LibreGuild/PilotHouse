//! What the status bar tells about a workspace besides Git: the ports its
//! processes listen on, and how CI went for the commit checked out.
//!
//! Looked at off the UI thread while the workspace is on screen: ports every
//! 10 seconds (`lsof`), CI every minute or when HEAD moves (the GitHub
//! checks of HEAD, with the account set up for its remote). Workspaces on a
//! network drive are left out: their Run tool reads the server.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::{AppContext as _, Context, Task, WeakEntity};
use ph_github::models::Check;
use ph_store::Store;

use crate::workspace_screen::WorkspaceScreen;

const PORTS_EVERY: Duration = Duration::from_secs(10);
/// Listeners that are tools' plumbing, not the workspace's servers (the
/// Android bridge and emulator have the Devices tool).
const PLUMBING: [&str; 4] = ["adb", "qemu-system", "netsimd", "emulator"];
/// CI is read every this many port rounds (a minute), or when HEAD moves.
const CI_EVERY: u32 = 6;

/// What was seen last.
#[derive(Default)]
pub(crate) struct Signals {
    /// Ports listened on by processes started in the workspace's folders.
    pub ports: Vec<u16>,
    pub ci: Option<Ci>,
    /// This workspace is a task whose branch is merged.
    pub merged_task: bool,
}

/// The CI checks of the commit checked out.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ci {
    pub passed: usize,
    pub pending: usize,
    /// Failed checks: name and link.
    pub failed: Vec<(String, String)>,
    /// A link to look at: the first failed check, else the first pending.
    pub url: Option<String>,
}

impl Ci {
    fn of(checks: &[Check]) -> Option<Self> {
        if checks.is_empty() {
            return None;
        }
        let mut ci = Ci {
            passed: 0,
            pending: 0,
            failed: Vec::new(),
            url: None,
        };
        let mut pending_url = None;
        for check in checks {
            match check.outcome.as_str() {
                "success" | "neutral" | "skipped" => ci.passed += 1,
                "failure" | "timed_out" | "cancelled" | "action_required" | "error" => {
                    ci.failed.push((check.name.clone(), check.url.clone()))
                }
                _ => {
                    ci.pending += 1;
                    pending_url.get_or_insert_with(|| check.url.clone());
                }
            }
        }
        ci.url = ci
            .failed
            .first()
            .map(|(_, url)| url.clone())
            .or(pending_url)
            .filter(|url| !url.is_empty());
        Some(ci)
    }
}

/// Starts looking, for the life of the screen.
pub(crate) fn watch(store: Store, cx: &mut Context<WorkspaceScreen>) -> Task<()> {
    cx.spawn(async move |this: WeakEntity<WorkspaceScreen>, cx| {
        let mut round = 0u32;
        let mut head = String::new();
        loop {
            let Ok(Some((folders, git_root, task))) = this.update(cx, |space, _| {
                (space.is_visible() && space.server().is_none()).then(|| {
                    let folders: Vec<PathBuf> = space.project().folders();
                    let git_root = folders.get(space.git_folder()).cloned();
                    let task = crate::tasks::of(&store, space.project().id);
                    (folders, git_root, task)
                })
            }) else {
                if this.upgrade().is_none() {
                    return;
                }
                cx.background_executor().timer(PORTS_EVERY).await;
                continue;
            };
            let ports = cx
                .background_spawn(async move {
                    let mut ports: Vec<u16> = ph_runs::ports::listening_within(PORTS_EVERY / 2)
                        .into_iter()
                        .filter(|port| folders.iter().any(|folder| port.belongs_to(folder)))
                        .filter(|port| !PLUMBING.iter().any(|name| port.process.starts_with(name)))
                        .map(|port| port.port)
                        .collect();
                    ports.dedup();
                    ports
                })
                .await;
            // CI when HEAD moved, else once a minute.
            let ci = match git_root {
                Some(root) => {
                    let store = store.clone();
                    let last = head.clone();
                    let due = round.is_multiple_of(CI_EVERY);
                    cx.background_spawn(async move {
                        let now = ph_git::git(&root, &["rev-parse", "HEAD"])
                            .map(|sha| sha.trim().to_string())
                            .unwrap_or_default();
                        if now.is_empty() || (now == last && !due) {
                            return (now, None);
                        }
                        let checks =
                            ph_pulls::hosting::resolve(&root, &store)
                                .ok()
                                .and_then(|hosting| {
                                    ph_github::pulls::checks(&hosting.client, &hosting.repo, &now)
                                        .ok()
                                });
                        (now, Some(checks.as_deref().and_then(Ci::of)))
                    })
                    .await
                }
                None => (String::new(), Some(None)),
            };
            head = ci.0;
            // A task whose branch is merged: once a minute.
            let merged = match task.filter(|_| round.is_multiple_of(CI_EVERY)) {
                Some(task) => Some(
                    cx.background_spawn(async move { crate::tasks::is_merged(&task) })
                        .await,
                ),
                None => None,
            };
            let alive = this.update(cx, |space, cx| {
                let signals = space.signals_mut();
                let mut changed = signals.ports != ports;
                signals.ports = ports;
                if let Some(merged) = merged
                    && signals.merged_task != merged
                {
                    signals.merged_task = merged;
                    changed = true;
                }
                if let Some(ci) = ci.1
                    && signals.ci != ci
                {
                    signals.ci = ci;
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            });
            if alive.is_err() {
                return;
            }
            round = round.wrapping_add(1);
            cx.background_executor().timer(PORTS_EVERY).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, outcome: &str) -> Check {
        Check {
            name: name.into(),
            outcome: outcome.into(),
            url: format!("https://ci/{name}"),
        }
    }

    #[test]
    fn sums_up_checks() {
        assert_eq!(Ci::of(&[]), None);
        let ci = Ci::of(&[
            check("lint", "success"),
            check("test", "failure"),
            check("build", "in_progress"),
        ])
        .unwrap();
        assert_eq!((ci.passed, ci.pending, ci.failed.len()), (1, 1, 1));
        assert_eq!(ci.url.as_deref(), Some("https://ci/test"));
        let waiting = Ci::of(&[check("build", "queued")]).unwrap();
        assert_eq!(waiting.url.as_deref(), Some("https://ci/build"));
    }
}
