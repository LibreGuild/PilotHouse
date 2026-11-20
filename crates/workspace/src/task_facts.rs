//! What stands around a task besides its chat: the branch its folder is on
//! and, where the repository is hosted and an account is connected, that
//! branch's pull request and its checks. A card on the Bridge and the title
//! bar say them, so a task shows as the piece of work it is (a chat, a
//! branch, a pull request) and not only as a conversation.
//!
//! Git and the host are asked off the UI thread, for the folders the
//! windows say they show, and what was found is kept here for them to read
//! while they draw: the branch every quarter of a minute, the pull request
//! and its checks every few minutes (they cost requests).
//!
//! What a chat itself did (files and lines changed, tests run) is not here:
//! it is kept with the chat (see `ph_store::sessions::SessionFacts`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{App, AppContext as _, Context, Entity, Global, Task, WindowId};
use ph_github::models::Check;
use ph_store::Store;

/// The branch of each folder is read this often.
const BRANCH_EVERY: Duration = Duration::from_secs(15);
/// Pull requests and checks are read every this many rounds.
const HOSTED_EVERY: u32 = 12;
/// Folders looked at in one round, at most: a window with hundreds of
/// tasks must not start hundreds of `git` runs.
const FOLDERS: usize = 32;

/// Where a branch's checks stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Checks {
    Passed,
    /// So many failed.
    Failed(usize),
    Running,
}

impl Checks {
    fn of(checks: &[Check]) -> Option<Self> {
        if checks.is_empty() {
            return None;
        }
        let (mut failed, mut pending) = (0, 0);
        for check in checks {
            match check.outcome.as_str() {
                "success" | "neutral" | "skipped" => {}
                "failure" | "timed_out" | "cancelled" | "action_required" | "error" => failed += 1,
                _ => pending += 1,
            }
        }
        Some(if failed > 0 {
            Checks::Failed(failed)
        } else if pending > 0 {
            Checks::Running
        } else {
            Checks::Passed
        })
    }
}

/// A branch's open pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pull {
    pub number: u64,
    pub url: String,
    pub draft: bool,
    pub checks: Option<Checks>,
}

/// What was found for a folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Place {
    /// The branch checked out (`None`: no repository, or a detached HEAD).
    pub branch: Option<String>,
    pub pull: Option<Pull>,
}

pub(crate) struct TaskFacts {
    store: Store,
    places: HashMap<PathBuf, Place>,
    /// The folders each window shows tasks of.
    wanted: HashMap<WindowId, Vec<PathBuf>>,
    rounds: u32,
    /// A look under way (one at a time).
    looking: Option<Task<()>>,
    _poll: Task<()>,
}

struct Hub(Entity<TaskFacts>);

impl Global for Hub {}

/// The facts, made on first use.
pub(crate) fn hub(store: &Store, cx: &mut App) -> Entity<TaskFacts> {
    if let Some(hub) = cx.try_global::<Hub>() {
        return hub.0.clone();
    }
    let store = store.clone();
    let facts = cx.new(|cx: &mut Context<TaskFacts>| TaskFacts {
        store,
        places: HashMap::new(),
        wanted: HashMap::new(),
        rounds: 0,
        looking: None,
        _poll: cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(BRANCH_EVERY).await;
                if this.update(cx, |this, cx| this.look(false, cx)).is_err() {
                    break;
                }
            }
        }),
    });
    cx.set_global(Hub(facts.clone()));
    facts
}

/// What is known of `folder` (nothing until it was looked at).
pub(crate) fn of<'a>(folder: &Path, cx: &'a App) -> Option<&'a Place> {
    cx.try_global::<Hub>()?.0.read(cx).places.get(folder)
}

impl TaskFacts {
    /// Says which folders `window` shows tasks of. New ones are looked at
    /// at once.
    pub(crate) fn want(&mut self, window: WindowId, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.wanted.get(&window) != Some(&folders) {
            self.wanted.insert(window, folders);
        }
        // New among the folders a round looks at. One past the most looked
        // at never gets a place: counted as new, it had the host asked at
        // every call (every few seconds) for as long as it was wanted.
        let new = self
            .looked_at()
            .iter()
            .any(|folder| !self.places.contains_key(folder));
        if new {
            self.look(true, cx);
        }
    }

    /// The folders a round looks at: every window's, [`FOLDERS`] at most.
    fn looked_at(&self) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = self.wanted.values().flatten().cloned().collect();
        folders.sort();
        folders.dedup();
        folders.truncate(FOLDERS);
        folders
    }

    /// A window closed: its folders are no longer looked at.
    pub(crate) fn forget(&mut self, window: WindowId) {
        self.wanted.remove(&window);
    }

    /// Reads the folders' branches (and, every few rounds or when `hosted`,
    /// their pull requests and checks) off the UI thread.
    fn look(&mut self, hosted: bool, cx: &mut Context<Self>) {
        if self.looking.is_some() {
            return;
        }
        let folders = self.looked_at();
        self.places.retain(|folder, _| folders.contains(folder));
        if folders.is_empty() {
            return;
        }
        self.rounds = self.rounds.wrapping_add(1);
        let hosted = hosted || self.rounds.is_multiple_of(HOSTED_EVERY);
        let known = self.places.clone();
        let store = self.store.clone();
        let read = cx.background_spawn(async move { read(&folders, hosted, &known, &store) });
        self.looking = Some(cx.spawn(async move |this, cx| {
            let places = read.await;
            _ = this.update(cx, |this, cx| {
                this.looking = None;
                if this.places != places {
                    this.places = places;
                    cx.notify();
                }
            });
        }));
    }
}

/// Each folder's branch; with `hosted`, also the open pull request of each
/// branch and its checks (else what was `known` of them, while the branch
/// is the same). Blocking.
fn read(
    folders: &[PathBuf],
    hosted: bool,
    known: &HashMap<PathBuf, Place>,
    store: &Store,
) -> HashMap<PathBuf, Place> {
    let mut places: HashMap<PathBuf, Place> = folders
        .iter()
        .map(|folder| {
            let branch = ph_git::is_repository(folder)
                .then(|| ph_git::current_branch(folder))
                .flatten()
                // A detached HEAD is no branch.
                .filter(|branch| branch != "HEAD");
            let pull = known
                .get(folder)
                .filter(|was| was.branch == branch)
                .and_then(|was| was.pull.clone());
            (folder.clone(), Place { branch, pull })
        })
        .collect();
    if !hosted {
        return places;
    }
    // One list of open pull requests per repository, shared by the folders
    // (worktrees) that are checkouts of it.
    let mut lists: HashMap<String, Vec<ph_github::models::PullRequest>> = HashMap::new();
    for folder in folders {
        let Some(branch) = places.get(folder).and_then(|place| place.branch.clone()) else {
            continue;
        };
        // No remote, or no account for its host: nothing to ask.
        let Ok(hosting) = ph_pulls::hosting::resolve(folder, store) else {
            continue;
        };
        let key = format!(
            "{}/{}/{}",
            hosting.account.host, hosting.repo.owner, hosting.repo.name
        );
        let pulls = lists.entry(key).or_insert_with(|| {
            ph_github::pulls::list(&hosting.client, &hosting.repo).unwrap_or_default()
        });
        let pull = pulls
            .iter()
            .find(|pull| pull.head.name == branch)
            .map(|pull| Pull {
                number: pull.number,
                url: pull.html_url.clone(),
                draft: pull.draft,
                checks: ph_github::pulls::checks(&hosting.client, &hosting.repo, &pull.head.sha)
                    .ok()
                    .and_then(|checks| Checks::of(&checks)),
            });
        if let Some(place) = places.get_mut(folder) {
            place.pull = pull;
        }
    }
    places
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(outcome: &str) -> Check {
        Check {
            name: "ci".into(),
            outcome: outcome.into(),
            url: String::new(),
        }
    }

    #[test]
    fn checks_stand_as_their_worst() {
        assert_eq!(Checks::of(&[]), None);
        assert_eq!(
            Checks::of(&[check("success"), check("skipped")]),
            Some(Checks::Passed)
        );
        assert_eq!(
            Checks::of(&[check("success"), check("in_progress")]),
            Some(Checks::Running)
        );
        assert_eq!(
            Checks::of(&[check("failure"), check("in_progress"), check("error")]),
            Some(Checks::Failed(2))
        );
    }
}
