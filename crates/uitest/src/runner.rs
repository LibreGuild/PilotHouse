//! A workspace's test runs from the Tests tab, held outside the tab
//! ([`TestRunHub`]), so a run keeps going when the tab is closed or rebuilt
//! (its folders changed, another window), and the workspace's status can say
//! a run is going or failed.
//!
//! Runs wait in a line and go one at a time: *Run all* is every suite of
//! the project, then every UI test. UI tests share one browser (or phone),
//! and a suite's command would slow a page a test is waiting on.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task};
use ph_control::ControlServer;
use ph_store::Store;
use serde_json::json;

use crate::model::{self, TestCase};
use crate::run::{self, Progress, Report};
use crate::suite::{self, Case, Suite, SuiteResult};
use crate::suite_run::{self, Stop};

/// Every workspace's test runs, by workspace id.
#[derive(Default)]
pub struct TestRunHub {
    /// The app's store: where a workspace's own test commands are kept.
    store: Option<Store>,
    runs: HashMap<i64, Entity<WorkspaceTestRuns>>,
}

impl Global for TestRunHub {}

impl TestRunHub {
    pub(crate) fn set_store(store: Store, cx: &mut App) {
        cx.default_global::<Self>().store = Some(store);
    }

    pub(crate) fn store(cx: &App) -> Option<Store> {
        cx.try_global::<Self>()?.store.clone()
    }

    /// The test runs of workspace `project_id`, made on first use.
    pub fn runs(project_id: i64, cx: &mut App) -> Entity<WorkspaceTestRuns> {
        if let Some(runs) = Self::existing(project_id, cx) {
            return runs;
        }
        let runs = cx.new(|_| WorkspaceTestRuns::new(project_id));
        cx.default_global::<Self>()
            .runs
            .insert(project_id, runs.clone());
        runs
    }

    pub fn existing(project_id: i64, cx: &App) -> Option<Entity<WorkspaceTestRuns>> {
        cx.try_global::<Self>()?.runs.get(&project_id).cloned()
    }

    /// Stops a removed workspace's run and deletes its runs, accepted looks
    /// and saved passwords.
    pub fn remove(project_id: i64, cx: &mut App) {
        if cx.has_global::<Self>()
            && let Some(runs) = cx.global_mut::<Self>().runs.remove(&project_id)
        {
            runs.update(cx, |runs, cx| runs.stop(cx));
        }
        cx.background_spawn(async move { run::forget_workspace(project_id) })
            .detach();
    }
}

/// One thing to run.
#[derive(Debug, Clone)]
pub enum Job {
    /// A UI test (at `path`); with `matrix`, at every screen size or on
    /// every connected device.
    Check {
        path: PathBuf,
        test: TestCase,
        matrix: bool,
    },
    /// A suite of the project, or only `only` of its tests.
    Suite { suite: Suite, only: Vec<Case> },
}

impl Job {
    fn name(&self) -> &str {
        match self {
            Job::Check { test, .. } => &test.name,
            Job::Suite { suite, .. } => &suite.name,
        }
    }
}

/// What is running now.
enum Running {
    Check {
        path: PathBuf,
        name: String,
        /// The steps that ended so far, of the variant being run.
        steps: Vec<Progress>,
    },
    Suite {
        id: String,
        name: String,
        stop: Stop,
        started: Instant,
    },
}

pub struct WorkspaceTestRuns {
    project_id: i64,
    waiting: VecDeque<Job>,
    running: Option<Running>,
    /// Of the runs asked for together: how many ended, how many there are,
    /// and those that failed, by name.
    done: usize,
    total: usize,
    failures: Vec<String>,
    /// How the run of a single UI test went, in its own words.
    single: Option<(SharedString, bool)>,
    /// How the newest runs went (a line, and whether it is bad news), until
    /// the Tests tab shows it.
    outcome: Option<(SharedString, bool)>,
    /// The newest runs failed.
    failed: bool,
    /// Runs ended while the workspace was not on screen.
    unseen: bool,
    on_screen: bool,
    /// Counts the runs that ended: whoever shows results reads them again
    /// when it moves.
    ended: usize,
    _run: Option<Task<()>>,
}

impl WorkspaceTestRuns {
    fn new(project_id: i64) -> Self {
        Self {
            project_id,
            waiting: VecDeque::new(),
            running: None,
            done: 0,
            total: 0,
            failures: Vec::new(),
            single: None,
            outcome: None,
            failed: false,
            unseen: false,
            on_screen: false,
            ended: 0,
            _run: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// The UI test being replayed now.
    pub fn running(&self) -> Option<&PathBuf> {
        match &self.running {
            Some(Running::Check { path, .. }) => Some(path),
            _ => None,
        }
    }

    /// The suite being run now.
    pub fn running_suite(&self) -> Option<&str> {
        match &self.running {
            Some(Running::Suite { id, .. }) => Some(id),
            _ => None,
        }
    }

    /// What is running, by name, and its place among the runs asked for
    /// together (`2 of 5`).
    pub fn now(&self) -> Option<(&str, usize, usize)> {
        let name = match self.running.as_ref()? {
            Running::Check { name, .. } | Running::Suite { name, .. } => name.as_str(),
        };
        Some((name, self.done + 1, self.total))
    }

    /// Whether UI test `path` or suite `id` waits for its turn.
    pub fn waits(&self, path: Option<&PathBuf>, suite: Option<&str>) -> bool {
        self.waiting.iter().any(|job| match job {
            Job::Check { path: waiting, .. } => Some(waiting) == path,
            Job::Suite { suite: waiting, .. } => Some(waiting.id.as_str()) == suite,
        })
    }

    /// The steps of the running UI test that ended so far.
    pub fn live_steps(&self) -> &[Progress] {
        match &self.running {
            Some(Running::Check { steps, .. }) => steps,
            _ => &[],
        }
    }

    /// The end of what the running suite printed so far, and how long it
    /// has run.
    pub fn live_output(&self, lines: usize) -> Option<(String, std::time::Duration)> {
        match &self.running {
            Some(Running::Suite { stop, started, .. }) => {
                Some((stop.tail(lines), started.elapsed()))
            }
            _ => None,
        }
    }

    /// How many runs ended since the app started.
    pub fn ended(&self) -> usize {
        self.ended
    }

    /// The workspace's tests changed without a run (an agent started or
    /// saved a recording from its chat): whoever shows them reads them
    /// again.
    pub fn changed(&mut self, cx: &mut Context<Self>) {
        self.ended += 1;
        cx.notify();
    }

    /// The newest runs' outcome, once (the Tests tab shows it).
    pub fn take_outcome(&mut self) -> Option<(SharedString, bool)> {
        self.outcome.take()
    }

    /// The newest runs failed.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Runs ended while the workspace was not on screen.
    pub fn unseen(&self) -> bool {
        self.unseen
    }

    /// The workspace is (or is no longer) shown. Showing it counts as
    /// seeing how the newest run went: whether one ended while it was away
    /// is returned (and forgotten).
    pub fn set_on_screen(&mut self, on_screen: bool, cx: &mut Context<Self>) -> bool {
        self.on_screen = on_screen;
        if !on_screen || !self.unseen {
            return false;
        }
        self.unseen = false;
        cx.notify();
        true
    }

    /// Runs `jobs`, after what already waits. They count as one asking: the
    /// line that says how it went speaks of all of them.
    pub fn start(&mut self, jobs: Vec<Job>, cx: &mut Context<Self>) {
        if jobs.is_empty() {
            return;
        }
        if self.running.is_none() {
            (self.done, self.total) = (0, 0);
            self.failures.clear();
            self.single = None;
            self.outcome = None;
        }
        self.total += jobs.len();
        self.waiting.extend(jobs);
        self.next(cx);
        cx.notify();
    }

    /// Stops what runs and forgets what waits.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.waiting.clear();
        match self.running.take() {
            Some(Running::Suite { stop, .. }) => {
                // Its thread ends with the command and is told nothing more.
                stop.stop();
            }
            Some(Running::Check { .. }) => {
                // The page follows the pane again (a run left mid-way had
                // fixed its size).
                ControlServer::call(
                    "browser.emulate",
                    json!({ "project_id": self.project_id, "quiet": true }),
                    cx,
                )
                .detach();
            }
            None => return,
        }
        self._run = None;
        self.ended += 1;
        self.outcome = Some(("Stopped.".into(), false));
        cx.notify();
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let Some(job) = self.waiting.pop_front() else {
            return;
        };
        let name = job.name().to_string();
        match job {
            Job::Check { path, test, matrix } => {
                self.running = Some(Running::Check {
                    path,
                    name,
                    steps: Vec::new(),
                });
                self._run = Some(self.run_check(test, matrix, cx));
            }
            Job::Suite { suite, only } => {
                let stop = Stop::default();
                self.running = Some(Running::Suite {
                    id: suite.id.clone(),
                    name,
                    stop: stop.clone(),
                    started: Instant::now(),
                });
                self._run = Some(self.run_suite(suite, only, stop, cx));
            }
        }
    }

    /// Replays `test`: a web test at the desktop size (so its screens
    /// compare from run to run), an Android one on the device in view. With
    /// `matrix`, at every screen size or on every connected device.
    fn run_check(&mut self, test: TestCase, matrix: bool, cx: &mut Context<Self>) -> Task<()> {
        let (project_id, android) = (self.project_id, test.is_android());
        let dir = run::report_dir(project_id, &model::slug(&test.name));
        // A phone test runs on a device by name, so that its accepted
        // looks are that device's: every one, or the one in view.
        let devices = android.then(|| {
            if matrix {
                run::devices(cx)
            } else {
                run::device_in_view(cx)
            }
        });
        let name = test.name.clone();
        // Each step as it ends, for the tab to show the run going.
        let this = cx.weak_entity();
        let progress: run::OnProgress = Rc::new(move |progress: Progress, cx: &mut App| {
            _ = this.update(cx, |this, cx| {
                if let Some(Running::Check { steps, .. }) = &mut this.running {
                    // The next variant of a matrix starts its steps over.
                    if steps
                        .last()
                        .is_some_and(|last| last.variant != progress.variant)
                    {
                        steps.clear();
                    }
                    steps.push(progress);
                    cx.notify();
                }
            });
        });
        cx.spawn(async move |this, cx| {
            let variants = match devices {
                Some(devices) => devices.await,
                None if matrix => Ok(run::screens()),
                None => Ok(run::screens().into_iter().take(1).collect()),
            };
            let result = match variants {
                Ok(variants) if android && variants.is_empty() => Err(anyhow::anyhow!(
                    "No Android device is ready: connect a phone or start an emulator."
                )),
                // No device could be asked for: said as it is.
                Err(err) if android => Err(err.context("No Android device could be reached")),
                Ok(variants) if variants.is_empty() => {
                    let task =
                        cx.update(|cx| run::run(test, project_id, None, dir, Some(progress), cx));
                    task.await.map(|report| vec![report])
                }
                Ok(variants) => {
                    let task = cx.update(|cx| {
                        run::run_matrix(test, project_id, variants, dir, Some(progress), cx)
                    });
                    task.await
                }
                Err(err) => Err(err),
            };
            _ = this.update(cx, |this, cx| {
                let (line, bad) = match &result {
                    Ok(reports) => summary(reports),
                    Err(err) => (format!("Could not run: {err:#}").into(), true),
                };
                this.single = Some((line, bad));
                this.end(name, bad, cx);
            });
        })
    }

    fn run_suite(
        &mut self,
        suite: Suite,
        only: Vec<Case>,
        stop: Stop,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let project_id = self.project_id;
        let name = suite.name.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let before = suite::last_result(project_id, &suite.id);
                    let dir = suite::suite_dir(project_id, &suite.id);
                    let result = suite_run::run(&suite, &only, before.as_ref(), &dir, &stop);
                    // A run that was stopped found nothing: what the one
                    // before found stays.
                    if !result.stopped {
                        let _ = suite::save_result(project_id, &suite.id, &result);
                    }
                    result
                })
                .await;
            _ = this.update(cx, |this, cx| {
                if result.stopped {
                    return;
                }
                this.single = Some((
                    format!("{name}: {}.", lower_first(&suite_run::summary(&result))).into(),
                    !result.passed(),
                ));
                this.end(name, !result.passed(), cx);
            });
        })
    }

    /// The running job ended: the next one starts, or the line that says
    /// how they all went is made.
    fn end(&mut self, name: String, bad: bool, cx: &mut Context<Self>) {
        self.running = None;
        self._run = None;
        self.done += 1;
        self.ended += 1;
        if bad {
            self.failures.push(name);
        }
        if self.waiting.is_empty() {
            self.failed = !self.failures.is_empty();
            self.unseen = !self.on_screen;
            self.outcome = Some(match (self.total, self.single.take()) {
                (1, Some(single)) => single,
                _ => batch_summary(self.total, &self.failures),
            });
        } else {
            self.next(cx);
        }
        cx.notify();
    }
}

/// `12 passed` after `cargo test: `.
fn lower_first(text: &str) -> String {
    let mut letters = text.chars();
    match letters.next() {
        Some(first) => first.to_lowercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// How `total` runs asked for together went, of which `failures` failed.
fn batch_summary(total: usize, failures: &[String]) -> (SharedString, bool) {
    if failures.is_empty() {
        return (format!("All {total} passed.").into(), false);
    }
    let named: Vec<&str> = failures.iter().take(3).map(String::as_str).collect();
    let more = failures.len().saturating_sub(named.len());
    (
        format!(
            "{} of {total} failed: {}{}.",
            failures.len(),
            named.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        )
        .into(),
        true,
    )
}

/// A UI test's run in a line: passed, where it failed, or how many of its
/// looks differ.
pub(crate) fn summary(reports: &[Report]) -> (SharedString, bool) {
    let on = |report: &Report| {
        report
            .variant
            .as_ref()
            .filter(|_| reports.len() > 1)
            .map(|variant| format!(" on {}", variant.label()))
            .unwrap_or_default()
    };
    if let Some(report) = reports.iter().find(|report| report.failure().is_some()) {
        let step = report.failure().map_or(0, |(ix, _)| ix + 1);
        return (format!("Failed at step {step}{}.", on(report)).into(), true);
    }
    let different: usize = reports.iter().map(Report::looks_different).sum();
    if different > 0 {
        let report = reports.iter().find(|report| report.looks_different() > 0);
        return (
            format!(
                "The steps worked, but {} different from what was accepted{}.",
                if different == 1 {
                    "one screen looks".to_string()
                } else {
                    format!("{different} screens look")
                },
                report.map(on).unwrap_or_default()
            )
            .into(),
            true,
        );
    }
    let new: usize = reports.iter().map(Report::looks_new).sum();
    let text = match (reports.len(), new) {
        (1, 0) => "Passed.".to_string(),
        (n, 0) => format!("Passed on all {n}."),
        (1, _) => "Passed. Accept how it looks, and later runs compare with it.".to_string(),
        (n, _) => {
            format!("Passed on all {n}. Accept how it looks, and later runs compare with it.")
        }
    };
    (text.into(), false)
}

/// How a suite's last result reads beside its name: `14`, `2 failed`.
pub fn suite_badge(result: &SuiteResult) -> String {
    use crate::suite::Outcome;
    let failed = result.count(Outcome::Failed);
    if failed > 0 {
        return format!("{failed} failed");
    }
    if result.broke() {
        return "failed".into();
    }
    match result.cases.len() {
        0 => String::new(),
        count => count.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_asked_for_together_are_told_in_one_line() {
        assert_eq!(batch_summary(5, &[]), ("All 5 passed.".into(), false));
        let failures: Vec<String> = ["Sign in", "cargo test"].map(String::from).to_vec();
        assert_eq!(
            batch_summary(5, &failures),
            ("2 of 5 failed: Sign in, cargo test.".into(), true)
        );
        let many: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        assert_eq!(
            batch_summary(9, &many),
            ("5 of 9 failed: a, b, c and 2 more.".into(), true)
        );
        assert_eq!(lower_first("12 passed"), "12 passed");
        assert_eq!(lower_first("Could not run"), "could not run");
    }
}
