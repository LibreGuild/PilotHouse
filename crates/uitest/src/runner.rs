//! A workspace's test runs from the Tests tab, held outside the tab
//! ([`TestRunHub`]), so a run keeps going when the tab is closed or rebuilt
//! (its folders changed, another window), and the workspace's status can say
//! a run is going or failed.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task};

use crate::model::{self, TestCase};
use crate::run;

/// Every workspace's test runs, by workspace id.
#[derive(Default)]
pub struct TestRunHub {
    runs: HashMap<i64, Entity<WorkspaceTestRuns>>,
}

impl Global for TestRunHub {}

impl TestRunHub {
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

    /// Stops a removed workspace's run.
    pub fn remove(project_id: i64, cx: &mut App) {
        if cx.has_global::<Self>() {
            cx.global_mut::<Self>().runs.remove(&project_id);
        }
    }
}

pub struct WorkspaceTestRuns {
    project_id: i64,
    /// The test being replayed now.
    running: Option<PathBuf>,
    /// How the newest run went (a line, and whether it is bad news), until
    /// the Tests tab shows it.
    outcome: Option<(SharedString, bool)>,
    /// The newest run failed.
    failed: bool,
    /// A run ended while the workspace was not on screen.
    unseen: bool,
    on_screen: bool,
    _run: Option<Task<()>>,
}

impl WorkspaceTestRuns {
    fn new(project_id: i64) -> Self {
        Self {
            project_id,
            running: None,
            outcome: None,
            failed: false,
            unseen: false,
            on_screen: false,
            _run: None,
        }
    }

    pub fn running(&self) -> Option<&PathBuf> {
        self.running.as_ref()
    }

    /// The newest run's outcome, once (the Tests tab shows it).
    pub fn take_outcome(&mut self) -> Option<(SharedString, bool)> {
        self.outcome.take()
    }

    /// The newest run failed.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// A run ended while the workspace was not on screen.
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

    /// Replays `test` (at `path`): a web test at the desktop size (so its
    /// screens compare from run to run), an Android one on the device in
    /// view. With `matrix`, at every screen size or on every connected
    /// device. One run at a time.
    pub fn start(&mut self, path: PathBuf, test: TestCase, matrix: bool, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        self.running = Some(path);
        self.outcome = None;
        let (project_id, android) = (self.project_id, test.is_android());
        let dir = run::report_dir(project_id, &model::slug(&test.name));
        let devices = (matrix && android).then(|| run::devices(cx));
        self._run = Some(cx.spawn(async move |this, cx| {
            let variants = match devices {
                Some(devices) => devices.await,
                None if matrix => Ok(run::screens()),
                None if android => Ok(Vec::new()),
                None => Ok(run::screens().into_iter().take(1).collect()),
            };
            let result = match variants {
                Ok(variants) if matrix && variants.is_empty() => {
                    Err(anyhow::anyhow!("No Android device is ready."))
                }
                Ok(variants) if variants.is_empty() => {
                    let task = cx.update(|cx| run::run(test, project_id, None, dir, cx));
                    task.await.map(|report| vec![report])
                }
                Ok(variants) => {
                    let task = cx.update(|cx| run::run_matrix(test, project_id, variants, dir, cx));
                    task.await
                }
                Err(err) => Err(err),
            };
            _ = this.update(cx, |this, cx| {
                this.running = None;
                let outcome = match result {
                    Ok(reports) => crate::panel::summary(&reports),
                    Err(err) => (format!("Could not run: {err:#}").into(), true),
                };
                this.failed = outcome.1;
                this.unseen = !this.on_screen;
                this.outcome = Some(outcome);
                this._run = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
}
