//! The Tests tab: the project's own tests and its UI tests in one list,
//! how each last went, and the ways to run, make and fix them.
//!
//! One list at the left (the project's suites with their tests under them,
//! then the UI tests), what is picked at the right. In a narrow pane the
//! list is all there is, and picking a row goes into it.

mod check;
mod create;
mod list;
mod suites;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task,
    UniformListScrollHandle, Window, div,
};
use ph_ui::{Icons, MenuButton as _};

use crate::model::{self, Step, TestCase};
use crate::record;
use crate::run::{self, Report};
use crate::runner::{Job, TestRunHub, WorkspaceTestRuns};
use crate::suite::{self, Case, Outcome, Suite, SuiteResult};

/// Below this width (at the default interface size) the tab is its list
/// alone, and a picked row takes its place.
const TWO_COLUMNS: f32 = 560.;

/// What the panel needs the workspace for.
#[derive(Debug, Clone)]
pub enum TestsPanelEvent {
    /// Start a chat with `prompt` (the agent records or repairs a test
    /// meanwhile); stop recording with [`TestsPanel::finish_recording`]
    /// when its turn ends.
    Agent { title: String, prompt: String },
    /// Open a file, at a 1-based line when one is known.
    Open { path: PathBuf, line: Option<u32> },
    /// Show the Run tab: the app a test opens is not running.
    ShowRun,
    /// Show where a UI test runs: the phone, else the browser.
    ShowPreview { phone: bool },
}

impl EventEmitter<TestsPanelEvent> for TestsPanel {}

/// What is picked in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Picked {
    /// A UI test, by its file.
    Check(PathBuf),
    /// A suite of the project, by its id.
    Suite(String),
    /// One test of a suite.
    Case {
        suite: String,
        group: String,
        name: String,
    },
}

/// Which picture of a look check is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Picture {
    Now,
    Accepted,
    Changes,
}

/// What the tab shows, read from the project and the data folder.
#[derive(Default)]
struct Loaded {
    tests: Vec<(PathBuf, TestCase)>,
    reports: HashMap<PathBuf, Vec<(PathBuf, Report)>>,
    suites: Vec<Suite>,
    results: HashMap<String, SuiteResult>,
}

/// Reads everything the tab shows. Blocks: call it off the UI thread.
fn load(project_id: i64, root: &std::path::Path, own: &[String]) -> Loaded {
    let tests = model::list(root);
    let reports = tests
        .iter()
        .filter_map(|(path, test)| {
            let reports = run::last_reports(project_id, &model::slug(&test.name));
            (!reports.is_empty()).then(|| (path.clone(), reports))
        })
        .collect();
    let mut suites = suite::detect(root);
    suites.extend(own.iter().map(|command| Suite::custom(command, root)));
    let results = suites
        .iter()
        .filter_map(|suite| Some((suite.id.clone(), suite::last_result(project_id, &suite.id)?)))
        .collect();
    Loaded {
        tests,
        reports,
        suites,
        results,
    }
}

pub struct TestsPanel {
    project_id: i64,
    root: PathBuf,
    tests: Vec<(PathBuf, TestCase)>,
    /// Each UI test's newest run: one report, or one per variant of a
    /// matrix run, each with its folder.
    reports: HashMap<PathBuf, Vec<(PathBuf, Report)>>,
    suites: Vec<Suite>,
    /// Each suite's newest result, by its id.
    results: HashMap<String, SuiteResult>,
    /// Nothing was read yet.
    loading: bool,
    picked: Option<Picked>,
    /// In a narrow pane: what is picked is shown, in place of the list.
    inside: bool,
    /// The list as drawn: sections, suites, groups, tests.
    rows: Vec<list::Row>,
    /// The suites and groups shown open, by [`list::open_key`].
    open: HashSet<String>,
    only_failed: bool,
    search: Entity<InputState>,
    list_scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// Which variant of a matrix run is shown.
    shown: usize,
    /// The step whose screen is shown; `None` follows the run (the step
    /// that failed, else the last).
    step: Option<usize>,
    picture: Picture,
    /// The workspace's runs, which outlive this panel.
    runs: Entity<WorkspaceTestRuns>,
    /// How many runs had ended when the results were last read.
    seen_ended: usize,
    /// Steps an agent proposed for a failing test, to keep or throw away.
    proposal: Option<(PathBuf, Vec<Step>)>,
    /// Recording a repair (its steps become a proposal) rather than a new
    /// recording (saved at once).
    repairing: bool,
    /// The form for a new UI test, while it is open.
    creating: Option<create::NewTest>,
    /// A test command of the user's own being typed.
    command: Option<Entity<InputState>>,
    /// A saved password being entered: its name, and the field.
    secret: Option<(String, Entity<InputState>)>,
    message: Option<(SharedString, bool)>,
    _tick: Option<Task<()>>,
    _load: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl TestsPanel {
    pub fn new(
        project_id: i64,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let runs = TestRunHub::runs(project_id, cx);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a test…"));
        let subscriptions = vec![
            cx.observe(&runs, |this, runs, cx| {
                // A run ended: show how it went, with its results.
                if let Some(outcome) = runs.update(cx, |runs, _| runs.take_outcome()) {
                    this.message = Some(outcome);
                }
                let ended = runs.read(cx).ended();
                if ended != this.seen_ended {
                    this.seen_ended = ended;
                    this.shown = 0;
                    this.step = None;
                    this.picture = Picture::Now;
                    this.reload(cx);
                }
                this.watch(cx);
                cx.notify();
            }),
            cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.rebuild_rows(cx);
                    cx.notify();
                }
            }),
        ];
        let seen_ended = runs.read(cx).ended();
        let mut panel = Self {
            project_id,
            root,
            tests: Vec::new(),
            reports: HashMap::new(),
            suites: Vec::new(),
            results: HashMap::new(),
            loading: true,
            picked: None,
            inside: false,
            rows: Vec::new(),
            open: HashSet::new(),
            only_failed: false,
            search,
            list_scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            shown: 0,
            step: None,
            picture: Picture::Now,
            runs,
            seen_ended,
            proposal: None,
            repairing: false,
            creating: None,
            command: None,
            secret: None,
            message: None,
            _tick: None,
            _load: None,
            _subscriptions: subscriptions,
        };
        panel.reload(cx);
        panel.watch(cx);
        panel
    }

    /// The setting a workspace's own test commands are kept in.
    fn commands_key(&self) -> String {
        format!("tests.commands.{}", self.project_id)
    }

    /// The test commands the user added for this workspace.
    fn own_commands(&self, cx: &gpui_kit::App) -> Vec<String> {
        TestRunHub::store(cx)
            .map(|store| store.setting(&self.commands_key()))
            .unwrap_or_default()
    }

    /// Reads the tests and their results again, off the UI thread.
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        let (project_id, root) = (self.project_id, self.root.clone());
        let own = self.own_commands(cx);
        self._load = Some(cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { load(project_id, &root, &own) })
                .await;
            _ = this.update(cx, |this, cx| {
                // A suite that just ran opens at what failed in it.
                let ran: Vec<String> = loaded
                    .results
                    .iter()
                    .filter(|(id, result)| {
                        this.results
                            .get(*id)
                            .is_none_or(|old| old.started_at != result.started_at)
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                this.tests = loaded.tests;
                this.reports = loaded.reports;
                this.suites = loaded.suites;
                this.results = loaded.results;
                this.loading = false;
                for id in ran {
                    this.open_failed(&id);
                }
                this.keep_picked();
                this.rebuild_rows(cx);
                cx.notify();
            });
        }));
    }

    /// What is picked still exists; else the first thing that failed, or
    /// the first there is, is picked.
    fn keep_picked(&mut self) {
        let exists = match &self.picked {
            Some(Picked::Check(path)) => self.tests.iter().any(|(test, _)| test == path),
            Some(Picked::Suite(id)) => self.suites.iter().any(|suite| &suite.id == id),
            Some(Picked::Case { suite, group, name }) => {
                self.results.get(suite).is_some_and(|result| {
                    result
                        .cases
                        .iter()
                        .any(|case| &case.group == group && &case.name == name)
                })
            }
            None => false,
        };
        if exists {
            return;
        }
        let failed_suite = self.suites.iter().find(|suite| {
            self.results
                .get(&suite.id)
                .is_some_and(|result| !result.passed())
        });
        let failed_check = self.tests.iter().find(|(path, _)| {
            self.reports
                .get(path)
                .is_some_and(|reports| reports.iter().any(|(_, report)| !report.passed()))
        });
        self.picked = failed_suite
            .map(|suite| Picked::Suite(suite.id.clone()))
            .or_else(|| failed_check.map(|(path, _)| Picked::Check(path.clone())))
            .or_else(|| {
                self.suites
                    .first()
                    .map(|suite| Picked::Suite(suite.id.clone()))
            })
            .or_else(|| {
                self.tests
                    .first()
                    .map(|(path, _)| Picked::Check(path.clone()))
            });
        self.inside = false;
    }

    /// Redraws while something runs or is recorded, so that what it shows
    /// (steps, output, time) keeps up.
    fn watch(&mut self, cx: &mut Context<Self>) {
        let busy = |this: &Self, cx: &gpui_kit::App| {
            this.runs.read(cx).is_running() || record::current(cx).is_some()
        };
        if self._tick.is_some() || !busy(self, cx) {
            return;
        }
        self._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                let going = this.update(cx, |this, cx| {
                    cx.notify();
                    busy(this, cx)
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
            _ = this.update(cx, |this, _| this._tick = None);
        }));
    }

    pub(crate) fn pick(&mut self, picked: Picked, cx: &mut Context<Self>) {
        if self.picked.as_ref() != Some(&picked) {
            self.shown = 0;
            self.step = None;
            self.picture = Picture::Now;
            self.secret = None;
        }
        self.picked = Some(picked);
        self.inside = true;
        self.creating = None;
        self.command = None;
        cx.notify();
    }

    fn test(&self, path: &PathBuf) -> Option<&TestCase> {
        self.tests
            .iter()
            .find(|(test, _)| test == path)
            .map(|(_, test)| test)
    }

    fn suite(&self, id: &str) -> Option<&Suite> {
        self.suites.iter().find(|suite| suite.id == id)
    }

    /// Changes test `path` and writes it down.
    fn change_test(
        &mut self,
        path: &PathBuf,
        change: impl FnOnce(&mut TestCase),
        cx: &mut Context<Self>,
    ) {
        let Some((_, test)) = self.tests.iter_mut().find(|(test, _)| test == path) else {
            return;
        };
        // As the file has it now: it may have been edited (by hand, by an
        // agent) since it was read, and that must not be undone. A few
        // lines of JSON, read and written here so that the next thing asked
        // of the test finds it changed.
        if let Ok(now) = model::load(path) {
            *test = now;
        }
        change(test);
        if let Err(err) = model::save(path, test) {
            self.message = Some((format!("{err:#}").into(), true));
        }
        self.rebuild_rows(cx);
        cx.notify();
    }

    // ---- Running ----

    fn check_job(&self, path: &PathBuf, matrix: bool) -> Option<Job> {
        let test = self.test(path)?.clone();
        (!test.steps.is_empty()).then(|| Job::Check {
            path: path.clone(),
            test,
            matrix,
        })
    }

    fn start(&mut self, jobs: Vec<Job>, cx: &mut Context<Self>) {
        if jobs.is_empty() {
            return;
        }
        self.message = None;
        self.step = None;
        self.picture = Picture::Now;
        self.runs.update(cx, |runs, cx| runs.start(jobs, cx));
        self.watch(cx);
        cx.notify();
    }

    /// Everything: the project's suites, then the UI tests that have steps.
    fn run_all(&mut self, cx: &mut Context<Self>) {
        let mut jobs: Vec<Job> = self
            .suites
            .iter()
            .map(|suite| Job::Suite {
                suite: suite.clone(),
                only: Vec::new(),
            })
            .collect();
        jobs.extend(
            self.tests
                .iter()
                .filter_map(|(path, _)| self.check_job(path, false)),
        );
        self.start(jobs, cx);
    }

    pub(crate) fn run_check(&mut self, path: &PathBuf, matrix: bool, cx: &mut Context<Self>) {
        match self.check_job(path, matrix) {
            Some(job) => self.start(vec![job], cx),
            None => {
                self.message = Some(("Record the test first.".into(), true));
                cx.notify();
            }
        }
    }

    /// Runs suite `id`: all of it, or only `only` of its tests.
    pub(crate) fn run_suite(&mut self, id: &str, only: Vec<Case>, cx: &mut Context<Self>) {
        if let Some(suite) = self.suite(id).cloned() {
            self.start(vec![Job::Suite { suite, only }], cx);
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.runs.update(cx, |runs, cx| runs.stop(cx));
        cx.notify();
    }

    /// Whether anything runs, waits or is recorded: no second run is
    /// started beside it.
    fn busy(&self, cx: &gpui_kit::App) -> bool {
        self.runs.read(cx).is_running() || record::current(cx).is_some()
    }

    // ---- Counting ----

    /// How many tests passed, failed and never ran: a suite's tests each
    /// count, a suite that never ran (or says nothing of its tests) and a
    /// UI test count as one.
    fn counts(&self) -> (usize, usize, usize) {
        let (mut passed, mut failed, mut never) = (0, 0, 0);
        for suite in &self.suites {
            match self.results.get(&suite.id) {
                None => never += 1,
                Some(result) if result.cases.is_empty() => {
                    if result.passed() {
                        passed += 1;
                    } else {
                        failed += 1;
                    }
                }
                Some(result) => {
                    passed += result.count(Outcome::Passed);
                    failed += result.count(Outcome::Failed) + usize::from(result.broke());
                }
            }
        }
        for (path, _) in &self.tests {
            match self.reports.get(path) {
                None => never += 1,
                Some(reports) if reports.iter().all(|(_, report)| report.passed()) => passed += 1,
                Some(_) => failed += 1,
            }
        }
        (passed, failed, never)
    }

    // ---- Own commands ----

    fn add_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.command.take() else {
            return;
        };
        let command = input.read(cx).value().trim().to_string();
        if command.is_empty() {
            cx.notify();
            return;
        }
        let mut own = self.own_commands(cx);
        if !own.contains(&command) {
            own.push(command.clone());
        }
        if let Some(store) = TestRunHub::store(cx)
            && let Err(err) = store.set_setting(&self.commands_key(), &own)
        {
            self.message = Some((format!("{err:#}").into(), true));
        }
        self.picked = Some(Picked::Suite(Suite::custom(&command, &self.root).id));
        self.inside = true;
        let _ = window;
        self.reload(cx);
        cx.notify();
    }

    fn remove_command(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(command) = self
            .suite(id)
            .filter(|suite| suite.custom)
            .map(|suite| suite.command.clone())
        else {
            return;
        };
        let mut own = self.own_commands(cx);
        own.retain(|known| *known != command);
        if let Some(store) = TestRunHub::store(cx) {
            let _ = store.set_setting(&self.commands_key(), &own);
        }
        self.picked = None;
        self.reload(cx);
        cx.notify();
    }

    // ---- The header ----

    fn render_header(&self, tight: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (passed, failed, never) = self.counts();
        let runs = self.runs.read(cx);
        let running = runs.now().map(|(name, at, of)| {
            if of > 1 {
                format!("Running {name} · {at} of {of}")
            } else {
                format!("Running {name}")
            }
        });
        let recording = record::current(cx).is_some();
        let nothing = self.suites.is_empty() && self.tests.is_empty();
        let part = |count: usize, word: &str, color: gpui_kit::Hsla| {
            (count > 0).then(|| {
                div()
                    .flex_none()
                    .text_color(color)
                    .child(format!("{count} {word}"))
            })
        };
        let summary = h_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .overflow_hidden()
            .text_xs()
            .text_color(theme.muted_foreground)
            .map(|this| match running {
                Some(running) => this
                    .child(ph_ui::Loader::new().xsmall())
                    .child(div().min_w_0().truncate().child(running)),
                None if self.loading => this.child("Looking for tests…"),
                None if nothing => this.child("No tests yet"),
                None => this
                    .children(part(failed, "failed", theme.danger))
                    .children(part(passed, "passed", theme.success))
                    .children(part(never, "not run", theme.muted_foreground)),
            });
        let failed_filter = (failed > 0 || self.only_failed).then(|| {
            Button::new("tests-only-failed")
                .xsmall()
                .ghost()
                .selected(self.only_failed)
                .icon(IconName::CircleX)
                .when(!tight, |this| this.label("Failed"))
                .tooltip(if self.only_failed {
                    "Show every test"
                } else {
                    "Show only what failed"
                })
                .accessibility_label("Show only what failed")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.only_failed = !this.only_failed;
                    this.rebuild_rows(cx);
                    cx.notify();
                }))
        });
        let run = if runs.is_running() {
            Button::new("tests-stop")
                .xsmall()
                .outline()
                .icon(Icons::Square)
                .when(!tight, |this| this.label("Stop"))
                .tooltip("Stop what runs, and what waits")
                .accessibility_label("Stop the tests")
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            Button::new("tests-run-all")
                .xsmall()
                .primary()
                .icon(IconName::Play)
                .when(!tight, |this| this.label("Run all"))
                .tooltip("Run the project's tests, then the UI tests")
                .accessibility_label("Run all tests")
                .disabled(nothing || recording)
                .on_click(cx.listener(|this, _, _, cx| this.run_all(cx)))
        };
        let panel = cx.weak_entity();
        h_flex()
            .flex_none()
            .h_9()
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(div().w_1())
            .child(summary)
            .children(failed_filter)
            .child(run)
            .child(
                Button::new("test-new")
                    .xsmall()
                    .ghost()
                    .icon(IconName::Plus)
                    .tooltip("New UI test")
                    .accessibility_label("New UI test")
                    .on_click(cx.listener(|this, _, window, cx| this.open_new_test(window, cx))),
            )
            .child(
                Button::new("tests-more")
                    .xsmall()
                    .ghost()
                    .icon(IconName::Ellipsis)
                    .tooltip("More")
                    .accessibility_label("More about tests")
                    .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                        let (add, again) = (panel.clone(), panel.clone());
                        menu.item(
                            PopupMenuItem::new("Add a test command…")
                                .icon(Icon::new(Icons::Terminal))
                                .on_click(move |_, window, cx| {
                                    _ = add.update(cx, |this, cx| this.open_command(window, cx));
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Look for tests again")
                                .icon(Icon::new(Icons::RotateCw))
                                .on_click(move |_, _, cx| {
                                    _ = again.update(cx, |this, cx| this.reload(cx));
                                }),
                        )
                    }),
            )
    }

    fn open_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("A command that runs tests, like make check")
        });
        self._subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.add_command(window, cx);
                }
            },
        ));
        input.update(cx, |input, cx| input.focus(window, cx));
        self.command = Some(input);
        self.creating = None;
        self.inside = true;
        cx.notify();
    }

    /// The form for a test command of the user's own.
    fn render_command(
        &self,
        input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::component::input::Input;
        v_flex()
            .p_4()
            .gap_3()
            .child(
                div()
                    .text_base()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child("Add a test command"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "For tests that were not found here. It runs in the project folder, and passes when it ends without an error.",
                    ),
            )
            .child(Input::new(input).small())
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("test-command-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.command = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("test-command-add")
                            .small()
                            .primary()
                            .label("Add")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_command(window, cx)
                            })),
                    ),
            )
    }

    /// What is picked, or the form that is open.
    fn render_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        if self.creating.is_some() {
            return self.render_new_test(cx).into_any_element();
        }
        if let Some(input) = self.command.clone() {
            return self.render_command(&input, cx).into_any_element();
        }
        match self.picked.clone() {
            Some(Picked::Check(path)) => self.render_check(&path, window, cx),
            Some(Picked::Suite(id)) => self.render_suite(&id, cx),
            Some(Picked::Case { suite, group, name }) => {
                self.render_case(&suite, &group, &name, cx)
            }
            None => div().into_any_element(),
        }
    }

    /// What says nothing is here yet, and the two ways to change that.
    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        ph_ui::EmptyState::new("No tests yet")
            .icon(Icons::FlaskConical)
            .description(
                "The project's own tests (cargo test, pytest, Vitest, Gradle…) show here once it has some. A UI test checks a flow in the browser or on a phone: say what to check, and an agent records the steps, which run again without AI.",
            )
            .action(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .justify_center()
                    .child(
                        Button::new("tests-empty-new")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .label("New UI test")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_new_test(window, cx)
                            })),
                    )
                    .child(
                        Button::new("tests-empty-command")
                            .small()
                            .outline()
                            .label("Add a test command")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_command(window, cx)
                            })),
                    ),
            )
    }
}

/// The row that leads back to the list, in a narrow pane.
fn back_row(cx: &mut Context<TestsPanel>) -> impl IntoElement {
    h_flex().flex_none().px_2().pt_2().child(
        Button::new("tests-back")
            .xsmall()
            .ghost()
            .icon(IconName::ChevronLeft)
            .label("Tests")
            .on_click(cx.listener(|this, _, _, cx| {
                this.inside = false;
                this.creating = None;
                this.command = None;
                cx.notify();
            })),
    )
}

impl Render for TestsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (width, probe) = ph_ui::measure::width_of("tests-room", window, cx);
        let width = width.map(|width| ph_ui::measure::design_px(width, window));
        let narrow = width.is_some_and(|width| width < TWO_COLUMNS);
        let tight = width.is_some_and(|width| width < 400.);
        let nothing = !self.loading && self.suites.is_empty() && self.tests.is_empty();
        let form = self.creating.is_some() || self.command.is_some();
        let body = if nothing && !form {
            self.render_empty(cx).into_any_element()
        } else if narrow {
            if self.inside || form {
                v_flex()
                    .size_full()
                    .child(back_row(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_detail(window, cx)),
                    )
                    .into_any_element()
            } else {
                self.render_list(true, cx).into_any_element()
            }
        } else {
            h_flex()
                .size_full()
                .child(self.render_list(false, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(self.render_detail(window, cx)),
                )
                .into_any_element()
        };
        v_flex()
            .id("tests-panel")
            .relative()
            .size_full()
            .child(probe)
            .child(self.render_header(tight, cx))
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
    }
}

/// A name as an element id: its letters and digits.
pub(crate) fn id_of(prefix: &str, name: &str) -> SharedString {
    let name: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    format!("{prefix}-{name}").into()
}

/// Text as a block of fixed-width lines that wrap, its last `lines` when it
/// has more: a failure, or what a command printed.
pub(crate) fn mono(text: &str, lines: usize, cx: &gpui_kit::App) -> impl IntoElement {
    let all: Vec<&str> = text.lines().collect();
    let skipped = all.len().saturating_sub(lines);
    v_flex()
        .w_full()
        .font_family(cx.theme().mono_font_family.clone())
        .text_xs()
        .when(skipped > 0, |this| {
            this.child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("… {skipped} lines before")),
            )
        })
        .children(all[skipped..].iter().map(|line| {
            // An empty line still takes a line's height.
            div()
                .min_h(ph_ui::rems_from_px(16.))
                .child(line.to_string())
        }))
}

/// How long something took: `0.4 s`, `12 s`, `2 min 5 s`.
pub(crate) fn took(millis: u64) -> String {
    match millis {
        0..=9_949 => format!("{:.1} s", millis as f64 / 1000.0),
        9_950..=59_499 => format!("{:.0} s", millis as f64 / 1000.0),
        _ => {
            let seconds = (millis + 500) / 1000;
            format!("{} min {} s", seconds / 60, seconds % 60)
        }
    }
}

/// The text area a new test's scenario is typed in.
pub(crate) fn area(
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut Context<TestsPanel>,
) -> Entity<TextareaState> {
    cx.new(|cx| TextareaState::new(window, cx).placeholder(placeholder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_reads_at_its_own_scale() {
        assert_eq!(took(0), "0.0 s");
        assert_eq!(took(430), "0.4 s");
        assert_eq!(took(9_949), "9.9 s");
        assert_eq!(took(12_400), "12 s");
        assert_eq!(took(125_000), "2 min 5 s");
        assert_eq!(
            id_of("suite", "web/vitest"),
            SharedString::from("suite-web-vitest")
        );
    }
}
