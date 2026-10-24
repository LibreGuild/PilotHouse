//! The Tests tab: the project's UI tests, their last results, and the ways
//! to make, run and fix them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Task, Window, div, img, px,
};
use ph_ui::Icons;
use ph_ui::MenuButton as _;

use crate::model::{self, Step, Target, TestCase};
use crate::record;
use crate::run::{self, Report};

/// What the panel needs the workspace for.
#[derive(Debug, Clone)]
pub enum TestsPanelEvent {
    /// Start a chat with `prompt` (the agent records or repairs a test
    /// meanwhile); stop recording with [`TestsPanel::finish_recording`]
    /// when its turn ends.
    Agent { title: String, prompt: String },
    /// Put a failure on the board; the card needs `test` (its file name) to
    /// pass before it is Done.
    Card {
        title: String,
        description: String,
        test: String,
    },
    /// Open the test's file.
    OpenFile(PathBuf),
}

impl EventEmitter<TestsPanelEvent> for TestsPanel {}

pub struct TestsPanel {
    project_id: i64,
    root: PathBuf,
    tests: Vec<(PathBuf, TestCase)>,
    /// Each test's newest run: one report, or one per variant of a matrix
    /// run, each with its folder.
    reports: HashMap<PathBuf, Vec<(PathBuf, Report)>>,
    selected: Option<PathBuf>,
    /// Which variant of a matrix run is shown.
    shown: usize,
    /// The workspace's runs, which outlive this panel.
    runs: Entity<crate::runner::WorkspaceTestRuns>,
    /// Steps an agent proposed for a failing test, to keep or throw away.
    proposal: Option<(PathBuf, Vec<Step>)>,
    /// Recording a repair (its steps become a proposal) rather than a new
    /// recording (saved at once).
    repairing: bool,
    /// The step whose screenshot (and changes) are shown large.
    zoomed: Option<usize>,
    name: Entity<InputState>,
    description: Entity<InputState>,
    target: Entity<InputState>,
    creating: bool,
    message: Option<(SharedString, bool)>,
    _tick: Option<Task<()>>,
    _runs: gpui_kit::Subscription,
}

impl TestsPanel {
    pub fn new(
        project_id: i64,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let runs = crate::runner::TestRunHub::runs(project_id, cx);
        let _runs = cx.observe(&runs, |this, runs, cx| {
            // A run ended: show how it went, with its reports.
            if let Some(outcome) = runs.update(cx, |runs, _| runs.take_outcome()) {
                this.shown = 0;
                this.message = Some(outcome);
                this.reload();
            }
            cx.notify();
        });
        let mut panel = Self {
            project_id,
            root,
            tests: Vec::new(),
            reports: HashMap::new(),
            selected: None,
            shown: 0,
            runs,
            proposal: None,
            repairing: false,
            zoomed: None,
            name: input("Name, e.g. Sign in works", window, cx),
            description: input("What to do and check, in plain words", window, cx),
            target: input(
                "Start at: a URL (http://localhost:3000) or an Android package (com.acme.app)",
                window,
                cx,
            ),
            creating: false,
            message: None,
            _tick: None,
            _runs,
        };
        panel.reload();
        panel
    }

    fn reload(&mut self) {
        self.tests = model::list(&self.root);
        self.reports = self
            .tests
            .iter()
            .filter_map(|(path, test)| {
                let reports = run::last_reports(self.project_id, &model::slug(&test.name));
                (!reports.is_empty()).then(|| (path.clone(), reports))
            })
            .collect();
        if self
            .selected
            .as_ref()
            .is_none_or(|selected| !self.tests.iter().any(|(path, _)| path == selected))
        {
            self.selected = self.tests.first().map(|(path, _)| path.clone());
        }
    }

    /// The report shown for `path`: the chosen variant of its newest run.
    fn shown_report(&self, path: &PathBuf) -> Option<&(PathBuf, Report)> {
        let reports = self.reports.get(path)?;
        reports.get(self.shown).or_else(|| reports.first())
    }

    fn selected_test(&self) -> Option<&(PathBuf, TestCase)> {
        let selected = self.selected.as_ref()?;
        self.tests.iter().find(|(path, _)| path == selected)
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        let description = self.description.read(cx).value().trim().to_string();
        let start = self.target.read(cx).value().trim().to_string();
        let target = if start.contains("://") || start.starts_with("localhost") {
            Target::Web {
                url: if start.contains("://") {
                    start
                } else {
                    format!("http://{start}")
                },
            }
        } else if !start.is_empty() && start.contains('.') && !start.contains(' ') {
            Target::Android { package: start }
        } else {
            self.message = Some((
                "Start at a URL, or an Android package like com.acme.app.".into(),
                true,
            ));
            cx.notify();
            return;
        };
        let test = TestCase {
            name,
            description,
            target,
            steps: Vec::new(),
        };
        match model::create(&self.root, &test) {
            Ok(path) => {
                for input in [&self.name, &self.description, &self.target] {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
                self.creating = false;
                self.message = None;
                self.reload();
                self.selected = Some(path.clone());
                self.record(path, cx);
            }
            Err(err) => self.message = Some((err.to_string().into(), true)),
        }
        cx.notify();
    }

    /// Has an agent work through the test's scenario while its steps are
    /// recorded.
    fn record(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(test) = self
            .tests
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, test)| test.clone())
        else {
            return;
        };
        record::start(self.project_id, path, Vec::new(), cx);
        self.repairing = false;
        self.watch_recording(cx);
        let (start, tools) = match &test.target {
            Target::Web { url } => (format!("Open {url} with browser_open."), "browser_*"),
            Target::Android { package } => {
                (format!("Launch {package} with device_launch."), "device_*")
            }
        };
        let prompt = format!(
            "Record a UI test in PilotHouse named “{}”.\n\nScenario: {}\n\n{start} Then do the scenario step by step with the \
             {tools} tools, using the selectors from browser_snapshot (or the UI tree) rather than coordinates when you can. \
             After each result the user should see, call test_expect (kind text with words on screen, or kind url). PilotHouse \
             saves every successful action and check as a step, so avoid detours you would undo. When done, reply with a \
             one-line summary.",
            test.name,
            if test.description.is_empty() {
                "(none given: explore the main flow)"
            } else {
                &test.description
            },
        );
        cx.emit(TestsPanelEvent::Agent {
            title: format!("Record test: {}", test.name),
            prompt,
        });
        cx.notify();
    }

    /// Has an agent redo a failing test from the step that broke; what it
    /// records is offered as a fix.
    fn repair(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let (Some((_, test)), Some((_, report))) = (
            self.tests.iter().find(|(p, _)| *p == path).cloned(),
            self.shown_report(&path).cloned(),
        ) else {
            return;
        };
        let Some((failed, result)) = report.failure() else {
            return;
        };
        let plan = run::plan(&test);
        let kept = plan[..failed].to_vec();
        let remaining: Vec<String> = plan[failed..].iter().map(Step::describe).collect();
        record::start(self.project_id, path, kept, cx);
        self.repairing = true;
        self.watch_recording(cx);
        let prompt = format!(
            "The UI test “{}” broke at step {} (“{}”): {}\n\nThe steps before it still work and the {} is in the state they \
             left. Continue the scenario from here, adapting to how the app looks now: {}\n\nScenario: {}\n\nUse the tools as \
             when recording (selectors from the snapshot; test_expect for the checks). PilotHouse records what you do as a \
             proposed fix. Reply with what changed in one line.",
            test.name,
            failed + 1,
            result.step.describe(),
            result.error.clone().unwrap_or_default(),
            if test.is_android() {
                "device"
            } else {
                "browser"
            },
            remaining.join("; "),
            test.description
        );
        cx.emit(TestsPanelEvent::Agent {
            title: format!("Repair test: {}", test.name),
            prompt,
        });
        cx.notify();
    }

    /// Redraws while recording, so the step count keeps up.
    fn watch_recording(&mut self, cx: &mut Context<Self>) {
        self._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(700))
                    .await;
                let recording = this.update(cx, |_, cx| {
                    cx.notify();
                    record::current(cx).is_some()
                });
                if !matches!(recording, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// Ends the recording: a new one is saved, a repair becomes a proposal.
    pub fn finish_recording(&mut self, cx: &mut Context<Self>) {
        let Some(recording) = record::stop(cx) else {
            return;
        };
        self._tick = None;
        let count = recording.steps.len();
        if self.repairing {
            self.proposal = Some((recording.path, recording.steps));
            self.message = Some((
                format!("The agent proposes {count} steps: keep or discard them below.").into(),
                false,
            ));
        } else if let Some((path, mut test)) = self
            .tests
            .iter()
            .find(|(p, _)| *p == recording.path)
            .cloned()
        {
            test.steps = recording.steps;
            self.message = Some(match model::save(&path, &test) {
                Ok(()) => (format!("Recorded {count} steps.").into(), false),
                Err(err) => (err.to_string().into(), true),
            });
        }
        self.repairing = false;
        self.reload();
        cx.notify();
    }

    fn keep_proposal(&mut self, cx: &mut Context<Self>) {
        let Some((path, steps)) = self.proposal.take() else {
            return;
        };
        if let Some((_, mut test)) = self.tests.iter().find(|(p, _)| *p == path).cloned() {
            test.steps = steps;
            if let Err(err) = model::save(&path, &test) {
                self.message = Some((err.to_string().into(), true));
            } else {
                self.message = Some(("Kept the fix. Run the test to check it.".into(), false));
            }
        }
        self.reload();
        cx.notify();
    }

    /// Replays the test: a web test at the desktop size (so its screens
    /// compare from run to run), an Android one on the device in view. With
    /// `matrix`, at every screen size or on every connected device.
    fn run(&mut self, path: PathBuf, matrix: bool, cx: &mut Context<Self>) {
        let Some((_, test)) = self.tests.iter().find(|(p, _)| *p == path).cloned() else {
            return;
        };
        if test.steps.is_empty() {
            self.message = Some(("Record the test first.".into(), true));
            cx.notify();
            return;
        }
        self.message = None;
        self.zoomed = None;
        self.runs
            .update(cx, |runs, cx| runs.start(path, test, matrix, cx));
        cx.notify();
    }

    /// Makes the newest run's screens the baseline for each of its variants
    /// whose steps worked.
    fn accept_look(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        let Some(reports) = self.reports.get(path) else {
            return;
        };
        let mut saved = 0;
        for (dir, report) in reports.iter().filter(|(_, report)| report.steps_passed()) {
            match run::accept_look(self.project_id, dir, report) {
                Ok(()) => saved += 1,
                Err(err) => {
                    self.message = Some((format!("Could not save the look: {err:#}").into(), true));
                    cx.notify();
                    return;
                }
            }
        }
        self.message = Some((
            match saved {
                1 => "Saved as the baseline. Later runs compare their screens with it.".into(),
                n => format!("Saved {n} baselines. Later runs compare their screens with them.")
                    .into(),
            },
            false,
        ));
        cx.notify();
    }

    fn card(&self, path: &PathBuf, cx: &mut Context<Self>) {
        let (Some((_, test)), Some((dir, report))) = (
            self.tests.iter().find(|(p, _)| p == path),
            self.shown_report(path),
        ) else {
            return;
        };
        let on = report
            .variant
            .as_ref()
            .map(|variant| format!(" on {}", variant.label()))
            .unwrap_or_default();
        let what = match report.failure() {
            Some((ix, failure)) => format!(
                "Step {} (“{}”) failed{on}: {}",
                ix + 1,
                failure.step.describe(),
                failure.error.clone().unwrap_or_default()
            ),
            None => format!(
                "{} steps look different from the accepted screens{on}.",
                report.looks_different()
            ),
        };
        cx.emit(TestsPanelEvent::Card {
            test: model::slug(&test.name),
            title: format!("UI test failing: {}", test.name),
            description: format!(
                "{what}\n\nScenario: {}\nReport: {}",
                test.description,
                dir.display()
            ),
        });
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // A third of a narrow pane, at most 220 px, so the details keep room.
        v_flex()
            .id("tests-list")
            .w(gpui_kit::relative(0.34))
            .max_w(px(220.))
            .min_w(px(120.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(cx.theme().border)
            .p_1()
            .gap_0p5()
            .children(self.tests.iter().map(|(path, test)| {
                let selected = self.selected.as_ref() == Some(path);
                let passed = self
                    .reports
                    .get(path)
                    .map(|reports| reports.iter().all(|(_, report)| report.passed()));
                let (icon, color) = match passed {
                    Some(true) => (IconName::CircleCheck, cx.theme().success),
                    Some(false) => (IconName::CircleX, cx.theme().danger),
                    None => (IconName::Minus, cx.theme().muted_foreground),
                };
                let pick = path.clone();
                h_flex()
                    .id(SharedString::from(format!("test-{}", path.display())))
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .when(selected, |this| this.bg(cx.theme().list_active))
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(pick.clone());
                        this.zoomed = None;
                        this.shown = 0;
                        cx.notify();
                    }))
                    .child(Icon::new(icon).xsmall().text_color(color))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(test.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{} steps{}",
                                        test.steps.len(),
                                        if test.is_android() { " · Android" } else { "" }
                                    )),
                            ),
                    )
            }))
            .when(self.tests.is_empty(), |this| {
                this.child(
                    div()
                        .p_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "No tests yet. Make one: describe it, and an agent records the steps.",
                        ),
                )
            })
    }

    fn render_new(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .p_3()
            .child(div().text_sm().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("New UI test"))
            .child(Input::new(&self.name).small())
            .child(Input::new(&self.description).small())
            .child(Input::new(&self.target).small())
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(
                "An agent works through it once with the browser or device tools; its actions become the steps, replayed after that without AI.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("test-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.creating = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("test-create")
                            .small()
                            .primary()
                            .label("Create and record")
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    ),
            )
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((path, test)) = self.selected_test() else {
            return div().into_any_element();
        };
        let runs = self
            .reports
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let report = self.shown_report(path);
        let recording = record::current(cx).filter(|recording| &recording.path == path);
        let busy = recording.is_some() || self.runs.read(cx).running().is_some();
        let failed = runs.iter().any(|(_, report)| !report.passed());
        let broken = report.is_some_and(|(_, report)| report.failure().is_some());
        // Saving the look only makes sense for runs whose steps worked.
        let usable: Vec<&Report> = runs
            .iter()
            .map(|(_, report)| report)
            .filter(|report| report.steps_passed())
            .collect();
        let look = if usable.iter().any(|report| report.looks_different() > 0) {
            Some("Accept new look")
        } else if usable
            .iter()
            .any(|report| !run::has_baseline(self.project_id, report))
        {
            Some("Save as baseline")
        } else {
            None
        };
        let (matrix_path, look_path) = (path.clone(), path.clone());
        let muted = cx.theme().muted_foreground;
        let (run_path, record_path, repair_path, card_path, open_path) = (
            path.clone(),
            path.clone(),
            path.clone(),
            path.clone(),
            path.clone(),
        );
        let proposal = self
            .proposal
            .as_ref()
            .filter(|(p, _)| p == path)
            .map(|(_, steps)| steps.clone());

        let toolbar = h_flex()
            .gap_1()
            .flex_wrap()
            .child(
                Button::new("test-run")
                    .small()
                    .primary()
                    .icon(IconName::Play)
                    .label(if self.runs.read(cx).running() == Some(path) { "Running…" } else { "Run" })
                    .disabled(busy || test.steps.is_empty())
                    .on_click(cx.listener(move |this, _, _, cx| this.run(run_path.clone(), false, cx))),
            )
            .child(
                Button::new("test-run-matrix")
                    .small()
                    .ghost()
                    .label(if test.is_android() { "Run on every device" } else { "Run on every size" })
                    .tooltip(if test.is_android() {
                        "One run on each connected device"
                    } else {
                        "Desktop 1280×800, tablet 820×1180 and phone 390×844"
                    })
                    .disabled(busy || test.steps.is_empty())
                    .on_click(cx.listener(move |this, _, _, cx| this.run(matrix_path.clone(), true, cx))),
            )
            .when_some(look, |this, label| {
                this.child(
                    Button::new("test-look")
                        .small()
                        .ghost()
                        .label(label)
                        .tooltip("Later runs compare each step's screen with this run's")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.accept_look(&look_path, cx))),
                )
            })
            // A new test needs recording; a recorded one keeps it in More.
            .when(test.steps.is_empty(), |this| {
                let record_path = record_path.clone();
                this.child(
                    Button::new("test-record")
                        .small()
                        .ghost()
                        .label("Record with agent")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.record(record_path.clone(), cx))),
                )
            })
            .when(broken, |this| {
                this.child(
                    Button::new("test-repair")
                        .small()
                        .ghost()
                        .label("Repair with agent")
                        .disabled(busy)
                        .tooltip("The agent redoes the scenario from the broken step; you keep or discard what it records")
                        .on_click(cx.listener(move |this, _, _, cx| this.repair(repair_path.clone(), cx))),
                )
            })
            .when(failed, |this| {
                this.child(
                    Button::new("test-card")
                        .small()
                        .ghost()
                        .label("Make a card")
                        .on_click(cx.listener(move |this, _, _, cx| this.card(&card_path, cx))),
                )
            })
            .child({
                let panel = cx.weak_entity();
                let recorded = !test.steps.is_empty();
                Button::new("test-more")
                    .small()
                    .ghost()
                    .icon(IconName::Ellipsis)
                    .tooltip("More")
                    .accessibility_label("More test actions")
                    .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                        let (record, open) = (panel.clone(), panel.clone());
                        let (record_path, open_path) = (record_path.clone(), open_path.clone());
                        menu.when(recorded, |menu| {
                            menu.item(PopupMenuItem::new("Record again with agent").disabled(busy).on_click(
                                move |_, _, cx| {
                                    _ = record.update(cx, |this, cx| this.record(record_path.clone(), cx));
                                },
                            ))
                        })
                        .item(PopupMenuItem::new("Edit steps (JSON)").on_click(move |_, _, cx| {
                            _ = open.update(cx, |_, cx| cx.emit(TestsPanelEvent::OpenFile(open_path.clone())));
                        }))
                    })
            });

        let steps: Vec<(String, Option<&run::StepResult>)> = match report {
            Some((_, report)) => report
                .steps
                .iter()
                .map(|step| (step.step.describe(), Some(step)))
                .collect(),
            None => test
                .steps
                .iter()
                .map(|step| (step.describe(), None))
                .collect(),
        };
        let variants = (runs.len() > 1).then(|| {
            h_flex()
                .gap_1()
                .flex_wrap()
                .children(runs.iter().enumerate().map(|(ix, (_, run))| {
                    let label = run
                        .variant
                        .as_ref()
                        .map_or_else(|| "This device".to_string(), |variant| variant.label());
                    Button::new(("test-variant", ix))
                        .xsmall()
                        .when(ix == self.shown, |this| this.primary())
                        .when(ix != self.shown, |this| this.outline())
                        .icon(if run.passed() {
                            IconName::CircleCheck
                        } else {
                            IconName::CircleX
                        })
                        .label(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.shown = ix;
                            this.zoomed = None;
                            cx.notify();
                        }))
                }))
        });
        let rows =
            steps.into_iter().enumerate().map(|(ix, (text, result))| {
                let looks_different = result
                    .and_then(|result| result.visual.as_ref())
                    .filter(|visual| visual.differs())
                    .map(|visual| visual.changed);
                let (icon, color) = match result.map(|result| result.outcome.as_str()) {
                    Some("passed") if looks_different.is_some() => {
                        (IconName::TriangleAlert, cx.theme().warning)
                    }
                    Some("passed") => (IconName::Check, cx.theme().success),
                    Some("failed") => (IconName::CircleX, cx.theme().danger),
                    Some(_) => (IconName::Minus, muted),
                    None => (IconName::Minus, muted),
                };
                let has_shot = result.is_some_and(|result| result.screenshot.is_some());
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .id(("test-step", ix))
                            .gap_2()
                            .text_sm()
                            .when(has_shot, |this| {
                                this.cursor_pointer().on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.zoomed = if this.zoomed == Some(ix) {
                                            None
                                        } else {
                                            Some(ix)
                                        };
                                        cx.notify();
                                    },
                                ))
                            })
                            .child(Icon::new(icon).xsmall().text_color(color))
                            .child(
                                div()
                                    .text_color(muted)
                                    .w(px(22.))
                                    .child(format!("{}", ix + 1)),
                            )
                            .child(div().flex_1().min_w_0().truncate().child(text))
                            .when_some(result.map(|result| result.millis), |this, millis| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(format!("{:.1}s", millis as f64 / 1000.0)),
                                )
                            })
                            .when(has_shot, |this| {
                                this.child(Icon::new(Icons::Image).xsmall().text_color(muted))
                            }),
                    )
                    .when_some(looks_different, |this, changed| {
                        this.child(div().pl(px(44.)).text_xs().text_color(cx.theme().warning).child(
                        format!(
                            "Looks different from the baseline: {:.1}% of the screen changed",
                            changed * 100.0
                        ),
                    ))
                    })
                    .when_some(
                        result.and_then(|result| result.error.clone()),
                        |this, error| {
                            this.child(
                                div()
                                    .pl(px(44.))
                                    .text_xs()
                                    .text_color(cx.theme().danger)
                                    .child(error),
                            )
                        },
                    )
            });

        // The zoomed step's screenshot, and its changes from the baseline.
        let zoom = self.zoomed.and_then(|ix| {
            let (dir, report) = report?;
            let step = report.steps.get(ix)?;
            let shot = dir.join(step.screenshot.as_ref()?);
            let diff = step
                .visual
                .as_ref()
                .and_then(|visual| visual.diff.as_ref())
                .map(|diff| dir.join(diff));
            Some((shot, diff))
        });
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .p_3()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(test.name.clone()),
                    )
                    .child(div().text_sm().text_color(muted).child(match &test.target {
                        Target::Web { url } => format!("In the browser, from {url}"),
                        Target::Android { package } => format!("On Android, in {package}"),
                    }))
                    .when(!test.description.is_empty(), |this| {
                        this.child(div().text_sm().child(test.description.clone()))
                    }),
            )
            .child(toolbar)
            .children(variants)
            .when_some(
                recording.map(|recording| recording.steps.len()),
                |this, count| {
                    this.child(
                        v_flex()
                            .gap_1p5()
                            .px_3()
                            .py_2()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().danger.opacity(0.12))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Icon::new(Icons::Circle)
                                            .small()
                                            .text_color(cx.theme().danger),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_sm()
                                            .child(format!("Recording: {count} steps so far")),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("It stops when the agent finishes."),
                            )
                            .child(
                                Button::new("test-stop")
                                    .xsmall()
                                    .label("Stop and save")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.finish_recording(cx)),
                                    ),
                            ),
                    )
                },
            )
            .when_some(proposal, |this, steps| {
                this.child(
                    v_flex()
                        .gap_1()
                        .p_3()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().info.opacity(0.1))
                        .child(
                            div()
                                .text_sm()
                                .child(format!("Proposed fix: {} steps", steps.len())),
                        )
                        .children(steps.iter().enumerate().map(|(ix, step)| {
                            div()
                                .text_xs()
                                .child(format!("{}. {}", ix + 1, step.describe()))
                        }))
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_end()
                                .child(
                                    Button::new("proposal-discard")
                                        .xsmall()
                                        .ghost()
                                        .label("Discard")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.proposal = None;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("proposal-keep")
                                        .xsmall()
                                        .primary()
                                        .label("Keep")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.keep_proposal(cx)),
                                        ),
                                ),
                        ),
                )
            })
            .when_some(self.message.clone(), |this, (message, is_error)| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(if is_error {
                            cx.theme().danger
                        } else {
                            cx.theme().success
                        })
                        .child(message),
                )
            })
            .child(
                v_flex()
                    .id("test-steps")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_1()
                    .children(rows)
                    .when_some(zoom, |this, (shot, diff)| {
                        let picture = |path: PathBuf| {
                            // A fixed height: in the scrolling list an image
                            // without one shrinks to nothing.
                            img(path)
                                .flex_none()
                                .w_full()
                                .h(px(360.))
                                .object_fit(ObjectFit::Contain)
                                .rounded(cx.theme().radius)
                        };
                        this.child(picture(shot)).when_some(diff, |this, diff| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("What changed since the baseline (red):"),
                            )
                            .child(picture(diff))
                        })
                    }),
            )
            .into_any_element()
    }
}

impl Render for TestsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} tests in {}", self.tests.len(), model::FOLDER)),
                    )
                    .child(
                        Button::new("test-new")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Plus)
                            .label("New test")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.creating = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("tests-refresh")
                            .xsmall()
                            .ghost()
                            .icon(Icons::RotateCw)
                            .tooltip("Read the tests again")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.reload();
                                cx.notify();
                            })),
                    ),
            )
            .child(if self.creating {
                self.render_new(cx).into_any_element()
            } else {
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_list(cx))
                    .child(self.render_detail(cx))
                    .into_any_element()
            })
    }
}

/// A run's outcome in a line: passed, where it failed, or how many steps
/// look different.
pub(crate) fn summary(reports: &[Report]) -> (SharedString, bool) {
    let on = |report: &Report| {
        report
            .variant
            .as_ref()
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
                "The steps worked, but {different} look different from the baseline{}. Check them, then accept the new look or fix the app.",
                report.map(on).unwrap_or_default()
            )
            .into(),
            true,
        );
    }
    let compared = reports.iter().any(Report::compared);
    let text = match (reports.len(), compared) {
        (1, true) => "Passed, and every screen matches the baseline.".to_string(),
        (1, false) => {
            "Passed. Save it as the baseline to check how screens look next time.".to_string()
        }
        (n, true) => format!("Passed on all {n}, and every screen matches the baseline."),
        (n, false) => format!(
            "Passed on all {n}. Save them as the baseline to check how screens look next time."
        ),
    };
    (text.into(), false)
}
