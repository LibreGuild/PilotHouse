//! Event schedules: the ones that wait for something to happen, started
//! when it does. Looked at on each tick of the runner:
//!
//! - **files**: the workspaces' file watchers report changes; once they have
//!   been quiet for a few seconds, schedules on file changes start, with the
//!   changed paths. Changes made while such a run is going do not start it
//!   again, so an agent editing files does not loop.
//! - **commits**: each Git folder's HEAD; a different one starts them.
//! - **GitHub**, every 5 minutes and only for workspaces with such
//!   schedules: a pull request that is new since the last look, and failing
//!   checks on the commit checked out.
//! - **chains**: [`Automation::after_finished`], when a schedule finishes.
//!
//! The first look only learns the current state, so opening PilotHouse does
//! not count every open pull request as new.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui_kit::{AppContext as _, Context, Entity, Subscription, Task};
use ph_files::{FilesChanged, ProjectWatcher, WatcherHub};
use ph_store::projects::ProjectId;
use ph_store::schedules::{Schedule, ScheduleId, When};

use super::Automation;
use crate::AppState;

/// How long files must stay unchanged before a change counts.
const QUIET: Duration = Duration::from_secs(5);
/// The shortest time between two event runs of one schedule.
const COOL_DOWN: Duration = Duration::from_secs(120);
/// GitHub is asked every this many ticks (about 5 minutes).
const GITHUB_EVERY: u32 = 10;

#[derive(Default)]
pub(crate) struct Triggers {
    /// Changed paths per workspace not acted on yet, and when the last came.
    files: HashMap<ProjectId, (Vec<PathBuf>, Instant)>,
    /// The HEAD last seen in each folder.
    heads: HashMap<PathBuf, String>,
    /// Open pull requests seen per workspace.
    pulls: HashMap<ProjectId, HashSet<u64>>,
    /// Commits whose failing checks were already acted on.
    failed_checks: HashSet<String>,
    /// When each schedule last started on an event.
    fired: HashMap<ScheduleId, Instant>,
    ticks: u32,
    /// The background look at Git and GitHub, while it runs.
    looking: Option<Task<()>>,
    /// The folders watched for workspaces with schedules on file changes.
    watching: HashMap<ProjectId, Vec<(Entity<ProjectWatcher>, Subscription)>>,
}

impl Triggers {
    /// Drops what is known about a removed workspace.
    pub(super) fn forget(&mut self, project_id: ProjectId) {
        self.files.remove(&project_id);
        self.pulls.remove(&project_id);
        self.watching.remove(&project_id);
    }
}

/// An open pull request: number, title and author.
type OpenPull = (u64, String, String);

/// What a background look found.
#[derive(Default)]
struct Seen {
    heads: Vec<(ProjectId, PathBuf, String)>,
    pulls: Vec<(ProjectId, Vec<OpenPull>)>,
    /// The commit checked out and the names of its failing checks.
    checks: Vec<(ProjectId, String, Vec<String>)>,
}

impl Automation {
    /// Files of workspace `project_id` changed.
    fn files_changed(&mut self, project_id: ProjectId, paths: &[PathBuf]) {
        let entry = self
            .triggers
            .files
            .entry(project_id)
            .or_insert_with(|| (Vec::new(), Instant::now()));
        entry.0.extend(paths.iter().cloned());
        entry.1 = Instant::now();
    }

    /// Starts the event schedules whose event happened (see the module docs).
    pub(super) fn check_triggers(&mut self, cx: &mut Context<Self>) {
        let schedules = AppState::store(cx).event_schedules().unwrap_or_default();
        if schedules.is_empty() {
            self.triggers.files.clear();
            self.triggers.watching.clear();
            return;
        }
        self.triggers.ticks += 1;
        let wants = |test: fn(&When) -> bool| -> Vec<ProjectId> {
            let mut projects: Vec<ProjectId> = schedules
                .iter()
                .filter(|schedule| test(&schedule.timing))
                .map(|schedule| schedule.project_id)
                .collect();
            projects.sort_unstable();
            projects.dedup();
            projects
        };

        // Files: watch the workspaces that need it, and act on quiet changes.
        let watched = wants(|when| matches!(when, When::FilesChange { .. }));
        self.triggers
            .watching
            .retain(|project_id, _| watched.contains(project_id));
        for project_id in watched {
            self.watch_files(project_id, cx);
        }
        let quiet: Vec<(ProjectId, Vec<PathBuf>)> = self
            .triggers
            .files
            .iter()
            .filter(|(_, (_, last))| last.elapsed() >= QUIET)
            .map(|(id, (paths, _))| (*id, paths.clone()))
            .collect();
        for (project_id, mut paths) in quiet {
            self.triggers.files.remove(&project_id);
            paths.sort();
            paths.dedup();
            for schedule in schedules
                .iter()
                .filter(|schedule| schedule.project_id == project_id)
            {
                let When::FilesChange { pattern } = &schedule.timing else {
                    continue;
                };
                let matching: Vec<String> = paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .filter(|path| {
                        pattern
                            .as_ref()
                            .is_none_or(|pattern| path.contains(pattern.as_str()))
                    })
                    .collect();
                if !matching.is_empty() {
                    let shown: Vec<&str> = matching.iter().take(20).map(String::as_str).collect();
                    let more = matching.len().saturating_sub(shown.len());
                    let context = format!(
                        "These files changed: {}{}",
                        shown.join(", "),
                        if more > 0 {
                            format!(" and {more} more")
                        } else {
                            String::new()
                        }
                    );
                    self.fire(schedule, context, cx);
                }
            }
        }

        // Git and GitHub: looked at in the background, acted on here.
        if self.triggers.looking.is_some() {
            return;
        }
        let commits = wants(|when| matches!(when, When::Commit));
        let github_due = self.triggers.ticks % GITHUB_EVERY == 1;
        let pulls = if github_due {
            wants(|when| matches!(when, When::PullRequest))
        } else {
            Vec::new()
        };
        let checks = if github_due {
            wants(|when| matches!(when, When::ChecksFail))
        } else {
            Vec::new()
        };
        if commits.is_empty() && pulls.is_empty() && checks.is_empty() {
            return;
        }
        let store = AppState::store(cx).clone();
        let folders = |ids: &[ProjectId]| -> Vec<(ProjectId, Vec<PathBuf>)> {
            ids.iter()
                .filter_map(|id| Some((*id, store.project(*id).ok().flatten()?.folders())))
                .collect()
        };
        let (commits, pulls, checks) = (folders(&commits), folders(&pulls), folders(&checks));
        let look = cx.background_spawn(async move { look(&store, commits, pulls, checks) });
        self.triggers.looking = Some(cx.spawn(async move |this, cx| {
            let seen = look.await;
            let _ = this.update(cx, |this, cx| {
                this.triggers.looking = None;
                this.apply_seen(seen, &schedules, cx);
            });
        }));
    }

    fn apply_seen(&mut self, seen: Seen, schedules: &[Schedule], cx: &mut Context<Self>) {
        let of = |project_id: ProjectId, test: fn(&When) -> bool| -> Vec<Schedule> {
            schedules
                .iter()
                .filter(|schedule| schedule.project_id == project_id && test(&schedule.timing))
                .cloned()
                .collect()
        };
        for (project_id, folder, head) in seen.heads {
            let before = self.triggers.heads.insert(folder.clone(), head.clone());
            if before.is_some_and(|before| before != head) {
                let short: String = head.chars().take(8).collect();
                let context = format!("{} is now at commit {short}.", folder.display());
                for schedule in of(project_id, |when| matches!(when, When::Commit)) {
                    self.fire(&schedule, context.clone(), cx);
                }
            }
        }
        for (project_id, open) in seen.pulls {
            let numbers: HashSet<u64> = open.iter().map(|(number, _, _)| *number).collect();
            let Some(before) = self.triggers.pulls.insert(project_id, numbers) else {
                continue;
            };
            for (number, title, author) in open
                .iter()
                .filter(|(number, _, _)| !before.contains(number))
            {
                let context = format!("Pull request #{number} “{title}” by {author} was opened.");
                for schedule in of(project_id, |when| matches!(when, When::PullRequest)) {
                    self.fire(&schedule, context.clone(), cx);
                }
            }
        }
        for (project_id, sha, failing) in seen.checks {
            if failing.is_empty() || !self.triggers.failed_checks.insert(sha.clone()) {
                continue;
            }
            let short: String = sha.chars().take(8).collect();
            let context = format!("Checks failed on commit {short}: {}.", failing.join(", "));
            for schedule in of(project_id, |when| matches!(when, When::ChecksFail)) {
                self.fire(&schedule, context.clone(), cx);
            }
        }
    }

    /// Schedule `id` finished (`passed`: its tests passed, or its agent
    /// replied): start the ones chained after it.
    pub(crate) fn after_finished(&mut self, id: ScheduleId, passed: bool, cx: &mut Context<Self>) {
        let store = AppState::store(cx).clone();
        cx.emit(super::AutomationChanged);
        let Ok(Some(finished)) = store.schedule(id) else {
            return;
        };
        // Its own changes do not start it again.
        if matches!(finished.timing, When::FilesChange { .. }) {
            self.triggers.files.remove(&finished.project_id);
        }
        let context = format!(
            "“{}” just finished: {}",
            finished.name,
            finished.last_result.clone().unwrap_or_default()
        );
        for schedule in store.event_schedules().unwrap_or_default() {
            if let When::After {
                schedule: after,
                on_failure,
            } = schedule.timing
                && after == id
                && (!on_failure || !passed)
            {
                self.fire(&schedule, context.clone(), cx);
            }
        }
    }

    /// Watches the folders of workspace `project_id`, once.
    fn watch_files(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        if self.triggers.watching.contains_key(&project_id) {
            return;
        }
        let Ok(Some(project)) = AppState::store(cx).project(project_id) else {
            return;
        };
        let watchers = project
            .roots()
            .into_iter()
            .map(|root| {
                let watcher = WatcherHub::watcher(root, cx);
                let subscription =
                    cx.subscribe(&watcher, move |this, _, event: &FilesChanged, _| {
                        this.files_changed(project_id, &event.paths)
                    });
                (watcher, subscription)
            })
            .collect();
        self.triggers.watching.insert(project_id, watchers);
    }

    /// Starts an event schedule, unless it runs already or ran very lately.
    fn fire(&mut self, schedule: &Schedule, context: String, cx: &mut Context<Self>) {
        let running = schedule.last_result.as_deref() == Some("Running…");
        let lately = self
            .triggers
            .fired
            .get(&schedule.id)
            .is_some_and(|at| at.elapsed() < COOL_DOWN);
        if running || lately {
            return;
        }
        self.triggers.fired.insert(schedule.id, Instant::now());
        self.run_schedule(schedule, Some(context), cx);
    }
}

/// Reads HEADs and asks GitHub, off the UI thread. A folder or workspace
/// that cannot be read is skipped.
fn look(
    store: &ph_store::Store,
    commits: Vec<(ProjectId, Vec<PathBuf>)>,
    pulls: Vec<(ProjectId, Vec<PathBuf>)>,
    checks: Vec<(ProjectId, Vec<PathBuf>)>,
) -> Seen {
    let mut seen = Seen::default();
    let head = |folder: &PathBuf| {
        ph_git::git(folder, &["rev-parse", "HEAD"])
            .ok()
            .map(|head| head.trim().to_string())
            .filter(|head| !head.is_empty())
    };
    for (project_id, folders) in commits {
        for folder in folders {
            if let Some(sha) = head(&folder) {
                seen.heads.push((project_id, folder, sha));
            }
        }
    }
    // The first folder with a GitHub remote stands for the workspace.
    let hosting = |folders: &[PathBuf]| {
        folders.iter().find_map(|folder| {
            ph_pulls::hosting::resolve(folder, store)
                .ok()
                .map(|hosting| (folder.clone(), hosting))
        })
    };
    for (project_id, folders) in pulls {
        let Some((_, hosting)) = hosting(&folders) else {
            continue;
        };
        if let Ok(list) = ph_github::pulls::list(&hosting.client, &hosting.repo) {
            let open = list
                .into_iter()
                .filter(|pull| pull.state == "open")
                .map(|pull| (pull.number, pull.title, pull.user.login))
                .collect();
            seen.pulls.push((project_id, open));
        }
    }
    for (project_id, folders) in checks {
        let Some((folder, hosting)) = hosting(&folders) else {
            continue;
        };
        let Some(sha) = head(&folder) else { continue };
        if let Ok(list) = ph_github::pulls::checks(&hosting.client, &hosting.repo, &sha) {
            let failing = list
                .into_iter()
                .filter(|check| {
                    matches!(
                        check.outcome.as_str(),
                        "failure" | "timed_out" | "cancelled"
                    )
                })
                .map(|check| check.name)
                .collect();
            seen.checks.push((project_id, sha, failing));
        }
    }
    seen
}
