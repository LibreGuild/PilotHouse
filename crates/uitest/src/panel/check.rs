//! What the Tests tab shows of a UI test: its steps in plain words with how
//! each went, the screen after the step that is picked, and the ways on
//! from there (run it, have an agent record or repair it, accept how it
//! looks).

use std::path::{Path, PathBuf};

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, img,
};
use ph_ui::{Icons, MenuButton as _};

use super::suites::{plural, result_line};
use super::{Picture, TestsPanel, TestsPanelEvent, took};
use crate::model::{self, Step, Target, TestCase};
use crate::record;
use crate::run::{self, Hint, Report, StepResult};
use crate::runner;
use crate::{export, secrets};

/// The share of the detail the screen takes beside the steps, and the most
/// it is given.
const SIDE_SHARE: f32 = 0.46;
const SIDE_MAX: f32 = 560.;
/// The tallest a screen is shown beside the steps, and under one of them.
const SIDE_TALLEST: f32 = 460.;
const UNDER_TALLEST: f32 = 300.;
/// What stands above the screen in the window: its bars, the tab's own
/// header, and a test's name with what goes with it.
const ABOVE_SCREEN: f32 = 380.;

/// Where what stands under a step starts: under the step's words, past its
/// mark and its number.
const UNDER_STEP: f32 = 56.;

/// From this width (at the default interface size) the screen stands beside
/// the steps; below it, under the step that is picked.
const SIDE_BY_SIDE: f32 = 640.;

/// What the agent is asked for when it records `test`.
fn record_prompt(test: &TestCase) -> String {
    let (start, tools, where_) = match &test.target {
        Target::Web { url } => (
            format!("Open {url} with browser_open."),
            "browser_*",
            "the site keeps nothing from before (it is signed out)",
        ),
        Target::Android { package } => (
            format!("Start {package} with device_launch."),
            "device_*",
            "the app starts at its first screen",
        ),
    };
    format!(
        "Record a UI test in {brand} named “{name}”.\n\nWhat it should check: {scenario}\n\n{start} Then do it step by step with the \
         {tools} tools. Each thing you click, tap or type becomes a step that names the element by what it says (the “Sign in” \
         button), so act on the real control: with a selector from browser_snapshot, or the middle of an element from device_ui, \
         rather than a point you guessed. After each result the user should see, call test_expect (kind text with words on the \
         screen, kind no_text, or kind url); where how the screen looks is what matters, call it with kind look. Every action \
         that works is kept, so avoid detours you would undo. Each run of the test starts where this recording does: {where_}, \
         so sign in as a part of it when it needs that. When done, reply with a one-line summary.",
        brand = ph_brand::NAME,
        name = test.name,
        scenario = if test.description.is_empty() {
            "(nothing was said: go through the main flow)"
        } else {
            &test.description
        },
    )
}

/// What the agent is asked for when `test` broke at step `failed` of `plan`.
fn repair_prompt(test: &TestCase, plan: &[Step], failed: usize, result: &StepResult) -> String {
    let remaining: Vec<String> = plan[failed..].iter().map(Step::describe).collect();
    format!(
        "The UI test “{}” broke at step {} (“{}”): {}\n\nThe steps before it still work and the {} is in the state they left. \
         Go on with what it should check from here, as the app is now: {}\n\nWhat it should check: {}\n\nUse the tools as when \
         recording (act on the real control; test_expect for the checks). {} records what you do as a proposed fix. If the \
         app is what is wrong, not the test, say so and change nothing. Reply with what changed in one line.",
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
        test.description,
        ph_brand::NAME,
    )
}

impl TestsPanel {
    /// The report shown for `path`: the chosen variant of its newest run.
    fn shown_report(&self, path: &PathBuf) -> Option<&(PathBuf, Report)> {
        let reports = self.reports.get(path)?;
        reports.get(self.shown).or_else(|| reports.first())
    }

    /// Has an agent do what the test should check while its steps are
    /// recorded. The recording starts where each run will: with what the
    /// site keeps erased, or the app at its first screen.
    pub(super) fn record(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(test) = self.test(&path).cloned() else {
            return;
        };
        if record::current(cx).is_some() {
            self.message = Some((
                "A test is already being recorded: stop it first.".into(),
                true,
            ));
            cx.notify();
            return;
        }
        let begin = record::begin(self.project_id, path, &test, cx);
        self.message = Some(("Getting ready to record…".into(), false));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let begun = begin.await;
            _ = this.update(cx, |this, cx| {
                if let Err(err) = begun {
                    this.message = Some((err.to_string().into(), true));
                    cx.notify();
                    return;
                }
                this.repairing = false;
                this.message = None;
                this.watch(cx);
                cx.emit(TestsPanelEvent::Agent {
                    title: format!("Record test: {}", test.name),
                    prompt: record_prompt(&test),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Has an agent redo a failing test from the step that broke; what it
    /// records is offered as a fix.
    fn repair(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let (Some(test), Some((_, report))) =
            (self.test(&path).cloned(), self.shown_report(&path).cloned())
        else {
            return;
        };
        let Some((failed, result)) = report.failure() else {
            return;
        };
        let plan = run::plan(&test);
        // The report can be of an older version of the test.
        if plan.get(failed) != Some(&result.step) {
            self.message = Some((
                "That failure is from an older version of the test: run it again first.".into(),
                true,
            ));
            cx.notify();
            return;
        }
        // What is kept: the test's own steps before the broken one (the
        // opening step a run adds is not one of them).
        let added = plan.len() - test.steps.len();
        let kept = test.steps[..failed.saturating_sub(added)].to_vec();
        if let Err(err) = record::start(self.project_id, path, kept, cx) {
            self.message = Some((err.to_string().into(), true));
            cx.notify();
            return;
        }
        self.repairing = true;
        self.watch(cx);
        cx.emit(TestsPanelEvent::Agent {
            title: format!("Repair test: {}", test.name),
            prompt: repair_prompt(&test, &plan, failed, result),
        });
        cx.notify();
    }

    /// Ends the recording: a new one is saved, a repair becomes a proposal.
    pub fn finish_recording(&mut self, cx: &mut Context<Self>) {
        // Another workspace's recording is its panel's to end.
        if !record::current(cx).is_some_and(|recording| recording.project_id == self.project_id) {
            return;
        }
        let Some(recording) = record::stop(cx) else {
            return;
        };
        let path = recording.path.clone();
        let repairing = std::mem::take(&mut self.repairing);
        cx.spawn(async move |this, cx| {
            // The passwords typed go to the credential store, and a new
            // recording into its file: off the UI thread. A repair is only
            // offered: its steps are kept or thrown away by the user.
            let steps = cx
                .background_spawn(async move {
                    if repairing {
                        recording.keep_passwords().map(|(steps, _)| steps)
                    } else {
                        record::write_down(recording).map(|(_, steps)| steps)
                    }
                })
                .await;
            _ = this.update(cx, |this, cx| {
                let steps = match steps {
                    Ok(steps) => steps,
                    Err(err) => {
                        this.message = Some((
                            format!("The recording could not be kept: {err:#}").into(),
                            true,
                        ));
                        cx.notify();
                        return;
                    }
                };
                let count = steps.len();
                this.message = Some(if repairing {
                    this.proposal = Some((path, steps));
                    (
                        format!(
                            "The agent proposes {}: keep or discard them below.",
                            plural(count, "step")
                        )
                        .into(),
                        false,
                    )
                } else if count == 0 {
                    (
                        "Nothing was recorded: the agent did not act in the browser or on the phone."
                            .into(),
                        true,
                    )
                } else {
                    (format!("Recorded {}.", plural(count, "step")).into(), false)
                });
                this.step = None;
                this.reload(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn keep_proposal(&mut self, cx: &mut Context<Self>) {
        let Some((path, steps)) = self.proposal.take() else {
            return;
        };
        self.change_test(&path, |test| test.steps = steps, cx);
        self.message = Some(("Kept the fix. Run the test to check it.".into(), false));
        cx.notify();
    }

    /// Makes the newest run's screens the accepted look of its look checks
    /// (all of them, in every variant whose steps worked; or only the one
    /// at step `only` of the variant shown).
    fn accept_looks(&mut self, path: &PathBuf, only: Option<usize>, cx: &mut Context<Self>) {
        let Some(reports) = self.reports.get(path) else {
            return;
        };
        let wanted: Vec<(PathBuf, Report)> = match only {
            Some(_) => self.shown_report(path).cloned().into_iter().collect(),
            None => reports
                .iter()
                .filter(|(_, report)| report.steps_passed())
                .cloned()
                .collect(),
        };
        let project_id = self.project_id;
        cx.spawn(async move |this, cx| {
            // Whole pictures are copied: off the UI thread.
            let accepted = cx
                .background_spawn(async move {
                    let mut accepted = 0;
                    for (dir, report) in &wanted {
                        accepted += run::accept_looks(project_id, dir, report, only)?;
                    }
                    anyhow::Ok(accepted)
                })
                .await;
            _ = this.update(cx, |this, cx| {
                this.message = Some(match accepted {
                    Ok(0) => ("There was no screen to accept.".into(), true),
                    Ok(_) => (
                        "Accepted. Later runs compare the screen with this look.".into(),
                        false,
                    ),
                    Err(err) => (format!("Could not accept the look: {err:#}").into(), true),
                });
                this.picture = Picture::Now;
                this.reload(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// From now on, look check `step` (of the test's own steps) leaves out
    /// the `areas` that differed in the newest run.
    fn ignore_changes(
        &mut self,
        path: &PathBuf,
        step: usize,
        areas: Vec<[u32; 4]>,
        cx: &mut Context<Self>,
    ) {
        self.change_test(
            path,
            |test| {
                if let Some(Step::ExpectLook { ignore, .. }) = test.steps.get_mut(step) {
                    ignore.extend(areas);
                }
            },
            cx,
        );
        self.message = Some((
            "Those areas are left out from now on. Run the test to see that it passes.".into(),
            false,
        ));
    }

    /// The field for a saved password that is missing, or being changed.
    fn ask_secret(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("The password this test types")
                .masked(true)
        });
        self._subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save_secret(window, cx);
                }
            },
        ));
        input.update(cx, |input, cx| input.focus(window, cx));
        self.secret = Some((name, input));
        cx.notify();
    }

    fn save_secret(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some((name, input)) = self.secret.take() else {
            return;
        };
        let value = input.read(cx).value().to_string();
        if value.is_empty() {
            cx.notify();
            return;
        }
        let project_id = self.project_id;
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_spawn(async move { secrets::set(project_id, &name, &value) })
                .await;
            _ = this.update(cx, |this, cx| {
                this.message = Some(match saved {
                    Ok(()) => ("Saved on this computer. Run the test again.".into(), false),
                    Err(err) => (format!("Could not save it: {err:#}").into(), true),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn export(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        let Some(test) = self.test(path).cloned() else {
            return;
        };
        let (root, source) = (self.root.clone(), path.clone());
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_spawn(async move { export::write(&root, &test, &source) })
                .await;
            _ = this.update(cx, |this, cx| match written {
                Ok(path) => {
                    this.message = Some((
                        format!(
                            "Written to {}.",
                            path.strip_prefix(&this.root).unwrap_or(&path).display()
                        )
                        .into(),
                        false,
                    ));
                    cx.emit(TestsPanelEvent::Open { path, line: None });
                    cx.notify();
                }
                Err(err) => {
                    this.message = Some((format!("{err:#}").into(), true));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn delete_test(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        if let Err(err) = std::fs::remove_file(path) {
            self.message = Some((format!("Could not delete it: {err}").into(), true));
        } else {
            self.message = None;
            self.picked = None;
            self.inside = false;
        }
        self.reload(cx);
        cx.notify();
    }

    pub(super) fn render_check(
        &mut self,
        path: &PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(test) = self.test(path).cloned() else {
            return div().into_any_element();
        };
        let (width, probe) = ph_ui::measure::width_of("ui-test-room", window, cx);
        let width = width.map(|width| ph_ui::measure::design_px(width, window));
        let beside = width.is_none_or(|width| width >= SIDE_BY_SIDE);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let runs = self.runs.read(cx);
        let running = runs.running() == Some(path);
        let waiting = runs.waits(Some(path), None);
        let recording = record::current(cx).filter(|recording| &recording.path == path);
        let busy = self.busy(cx);
        let reports: &[(PathBuf, Report)] = self
            .reports
            .get(path)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let report = self.shown_report(path);
        let plan = run::plan(&test);
        // A run opens the page (or starts the app) first when the
        // recording did not: that step is not one of the test's own.
        let added = plan.len() - test.steps.len();
        let live = if running { runs.live_steps() } else { &[] };
        // How each step went: as the run going on has it, else as the
        // newest report does, where it is of this step.
        let result_of = |at: usize| -> Option<(&StepResult, &Path)> {
            if running {
                return live
                    .iter()
                    .find(|progress| progress.index == at)
                    .map(|progress| (&progress.result, progress.dir.as_path()));
            }
            let (dir, report) = report?;
            report
                .steps
                .get(at)
                .filter(|result| Some(&result.step) == plan.get(at))
                .map(|result| (result, dir.as_path()))
        };
        let stale = !running
            && report.is_some_and(|(_, report)| {
                report.steps.len() != plan.len()
                    || report
                        .steps
                        .iter()
                        .zip(&plan)
                        .any(|(result, step)| &result.step != step)
            });
        // The step whose screen shows: the one picked, else where the run
        // is, where it failed, the first look that differs, the last.
        let follows = || {
            let with_shot =
                |at: &usize| result_of(*at).is_some_and(|(result, _)| result.screenshot.is_some());
            let all = 0..plan.len();
            if running {
                return all.rev().find(with_shot);
            }
            all.clone()
                .find(|at| result_of(*at).is_some_and(|(result, _)| result.outcome == "failed"))
                .or_else(|| {
                    all.clone().find(|at| {
                        result_of(*at).is_some_and(|(result, _)| {
                            result
                                .visual
                                .as_ref()
                                .is_some_and(|visual| visual.differs())
                        })
                    })
                })
                .or_else(|| all.rev().find(with_shot))
        };
        let at_step = self.step.filter(|at| *at < plan.len()).or_else(follows);
        // A failure an agent can repair: not one the user gets past
        // themselves (the app is not running, a password is missing).
        let broken = !stale
            && report.is_some_and(|(_, report)| {
                report
                    .failure()
                    .is_some_and(|(_, result)| result.hint.is_none())
            });
        // One button for all the looks there are to accept, where there are
        // several (each has its own under its step).
        let looks_to_accept = !running
            && reports
                .iter()
                .filter(|(_, report)| report.steps_passed())
                .map(|(_, report)| report.looks_different() + report.looks_new())
                .sum::<usize>()
                > 1;

        // ---- Its name, and what can be done with it ----
        let (run_path, record_path, repair_path, look_path) =
            (path.clone(), path.clone(), path.clone(), path.clone());
        let unrecorded = test.steps.is_empty();
        let panel = cx.weak_entity();
        let menu_path = path.clone();
        let (android, clean) = (test.is_android(), test.starts_clean());
        let actions = h_flex()
            .gap_1()
            .flex_wrap()
            .map(|this| {
                if unrecorded {
                    this.child(
                        Button::new("test-record")
                            .small()
                            .primary()
                            .icon(Icons::Circle)
                            .label("Record with agent")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.record(record_path.clone(), cx)
                            })),
                    )
                } else {
                    this.child(
                        Button::new("test-run")
                            .small()
                            .primary()
                            .icon(IconName::Play)
                            .label(if running {
                                "Running…"
                            } else if waiting {
                                "Waiting…"
                            } else {
                                "Run"
                            })
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_check(&run_path, false, cx)
                            })),
                    )
                }
            })
            .when(broken, |this| {
                this.child(
                    Button::new("test-repair")
                        .small()
                        .ghost()
                        .icon(Icons::Wrench)
                        .label("Repair with agent")
                        .disabled(busy)
                        .tooltip(ph_ui::tip(
                            "The agent goes on from the broken step; you keep or discard what it records",
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.repair(repair_path.clone(), cx)
                        })),
                )
            })
            .when(looks_to_accept, |this| {
                this.child(
                    Button::new("test-look")
                        .small()
                        .ghost()
                        .label("Accept every look")
                        .tooltip(ph_ui::tip(
                            "Later runs compare each look check's screen with this run's",
                        ))
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.accept_looks(&look_path, None, cx)
                        })),
                )
            })
            .child(
                Button::new("test-more")
                    .small()
                    .ghost()
                    .icon(IconName::Ellipsis)
                    .tooltip("More")
                    .accessibility_label("More test actions")
                    .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                        let on = |act: fn(&mut TestsPanel, &PathBuf, &mut Context<TestsPanel>)| {
                            let (panel, path) = (panel.clone(), menu_path.clone());
                            move |_: &gpui_kit::ClickEvent,
                                  _: &mut Window,
                                  cx: &mut gpui_kit::App| {
                                _ = panel.update(cx, |this, cx| act(this, &path, cx));
                            }
                        };
                        menu.item(
                            PopupMenuItem::new(if android {
                                "Run on every device"
                            } else {
                                "Run on every size"
                            })
                            .disabled(busy || unrecorded)
                            .on_click(on(|this, path, cx| this.run_check(path, true, cx))),
                        )
                        .when(!unrecorded, |menu| {
                            menu.item(
                                PopupMenuItem::new("Record again with agent")
                                    .disabled(busy)
                                    .on_click(on(|this, path, cx| this.record(path.clone(), cx))),
                            )
                        })
                        .separator()
                        .item(
                            PopupMenuItem::new(if android {
                                "Erase the app's data before each run"
                            } else {
                                "Start signed out, with nothing kept"
                            })
                            .checked(clean)
                            .on_click(on(|this, path, cx| {
                                this.change_test(
                                    path,
                                    |test| test.clean = Some(!test.starts_clean()),
                                    cx,
                                )
                            })),
                        )
                        .item(
                            PopupMenuItem::new(if android {
                                "Export as a Maestro flow"
                            } else {
                                "Export as a Playwright test"
                            })
                            .disabled(unrecorded)
                            .on_click(on(|this, path, cx| this.export(path, cx))),
                        )
                        .item(PopupMenuItem::new("Edit steps (JSON)").on_click(on(
                            |_, path, cx| {
                                cx.emit(TestsPanelEvent::Open {
                                    path: path.clone(),
                                    line: None,
                                })
                            },
                        )))
                        .separator()
                        .item(
                            PopupMenuItem::new("Delete this test")
                                .icon(Icon::new(Icons::Trash))
                                .disabled(busy)
                                .on_click(on(|this, path, cx| this.delete_test(path, cx))),
                        )
                    }),
            );
        let place = match &test.target {
            Target::Web { url } => {
                let short = url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches('/');
                format!("In the browser, from {short}")
            }
            Target::Android { package } => format!("On a phone, in {package}"),
        };
        let heading = v_flex()
            .gap_1()
            .child(
                div()
                    .text_base()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(test.name.clone()),
            )
            .when(!test.description.is_empty(), |this| {
                // A few lines: the steps under it need the room.
                this.child(
                    div()
                        .text_sm()
                        .line_clamp(3)
                        .child(test.description.clone()),
                )
            })
            .child(div().text_xs().text_color(muted).child(place));

        // ---- The sizes or devices of a matrix run ----
        let variants = (reports.len() > 1 && !running).then(|| {
            h_flex()
                .gap_1()
                .flex_wrap()
                .children(reports.iter().enumerate().map(|(ix, (_, run))| {
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
                            this.step = None;
                            this.picture = Picture::Now;
                            cx.notify();
                        }))
                }))
        });

        // ---- How it went ----
        let status: Option<AnyElement> = if let Some(recording) = recording {
            let count = recording.steps.len();
            Some(
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded(theme.radius)
                    .bg(theme.danger.opacity(0.12))
                    .child(
                        Icon::new(Icons::Circle)
                            .small()
                            .flex_none()
                            .text_color(theme.danger),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .child(format!("Recording: {} so far", plural(count, "step"))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("It stops when the agent finishes."),
                            ),
                    )
                    .child(
                        Button::new("test-stop")
                            .xsmall()
                            .outline()
                            .label("Stop and save")
                            .on_click(cx.listener(|this, _, _, cx| this.finish_recording(cx))),
                    )
                    .into_any_element(),
            )
        } else if running {
            let done = live.len();
            Some(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(muted)
                    .child(ph_ui::Loader::new().small())
                    .child(div().flex_1().min_w_0().child(format!(
                        "Running · step {} of {}",
                        (done + 1).min(plan.len()),
                        plan.len()
                    )))
                    .child(
                        Button::new("test-watch")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Eye)
                            .label("Watch")
                            .tooltip(if android {
                                "Show the phone"
                            } else {
                                "Show the browser"
                            })
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(TestsPanelEvent::ShowPreview { phone: android })
                            })),
                    )
                    .into_any_element(),
            )
        } else if waiting {
            Some(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Waits for its turn.")
                    .into_any_element(),
            )
        } else if unrecorded {
            Some(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(
                        "Not recorded yet. An agent does it once in the browser or on the phone; what it does becomes the steps.",
                    )
                    .into_any_element(),
            )
        } else if let Some((_, shown)) = report {
            let all: Vec<Report> = reports.iter().map(|(_, report)| report.clone()).collect();
            let (said, bad) = runner::summary(&all);
            let (icon, color) = if !bad {
                (IconName::CircleCheck, theme.success)
            } else if all.iter().all(Report::steps_passed) {
                (IconName::TriangleAlert, theme.warning)
            } else {
                (IconName::CircleX, theme.danger)
            };
            Some(
                result_line(
                    icon,
                    color,
                    said,
                    Some(format!(
                        "{} · {}",
                        took(shown.millis()),
                        ph_ui::time::ago(shown.started_at)
                    )),
                    cx,
                )
                .into_any_element(),
            )
        } else {
            Some(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Not run yet.")
                    .into_any_element(),
            )
        };
        // What was just done, when the line above does not say it already.
        let message = self
            .message
            .clone()
            .filter(|_| !running)
            .filter(|(message, _)| {
                let all: Vec<Report> = reports.iter().map(|(_, report)| report.clone()).collect();
                all.is_empty() || runner::summary(&all).0 != *message
            })
            .map(|(message, bad)| {
                div()
                    .text_xs()
                    .text_color(if bad { theme.danger } else { muted })
                    .child(message)
            });
        let stale_note = stale.then(|| {
            div()
                .text_xs()
                .text_color(muted)
                .child("The test changed since it last ran.")
        });

        // ---- A fix an agent proposed ----
        let proposal = self
            .proposal
            .as_ref()
            .filter(|(proposed, _)| proposed == path)
            .map(|(_, steps)| {
                v_flex()
                    .flex_none()
                    .gap_1()
                    .p_3()
                    .rounded(theme.radius)
                    .bg(theme.info.opacity(0.1))
                    .child(
                        div()
                            .text_sm()
                            .child(format!("Proposed fix: {}", plural(steps.len(), "step"))),
                    )
                    .child(
                        // However many steps it has, the test's own stay
                        // in view under it.
                        v_flex()
                            .id("test-proposal")
                            .max_h(ph_ui::rems_from_px(140.))
                            .overflow_y_scroll()
                            .children(steps.iter().enumerate().map(|(ix, step)| {
                                div()
                                    .text_xs()
                                    .child(format!("{}. {}", ix + 1, step.describe()))
                            })),
                    )
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
                                    .on_click(cx.listener(|this, _, _, cx| this.keep_proposal(cx))),
                            ),
                    )
            });

        // ---- A saved password being entered ----
        let secret = self.secret.as_ref().map(|(name, input)| {
            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("“{name}”")),
                )
                .child(div().flex_1().min_w_0().child(Input::new(input).small()))
                .child(
                    Button::new("test-secret-save")
                        .small()
                        .primary()
                        .label("Save")
                        .on_click(cx.listener(|this, _, window, cx| this.save_secret(window, cx))),
                )
                .child(
                    Button::new("test-secret-cancel")
                        .small()
                        .ghost()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.secret = None;
                            cx.notify();
                        })),
                )
        });
        // What it types that is not in its file, with the way to change it.
        let kept = (self.secret.is_none() && !test.secrets().is_empty()).then(|| {
            h_flex()
                .gap_1()
                .flex_wrap()
                .text_xs()
                .text_color(muted)
                .child(Icon::new(Icons::KeyRound).xsmall())
                .child("Types a password kept on this computer.")
                .children(test.secrets().into_iter().map(|name| {
                    let asked = name.clone();
                    Button::new(SharedString::from(format!("test-secret-{name}")))
                        .xsmall()
                        .ghost()
                        .label(if test.secrets().len() > 1 {
                            format!("Change “{name}”")
                        } else {
                            "Change".to_string()
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.ask_secret(asked.clone(), window, cx)
                        }))
                }))
        });

        // ---- The screen after the step that is picked ----
        let picture = self.picture;
        // How tall the screenshots are for their width (a page is wide, a
        // phone tall): as the run says, a page's size where it does not.
        let shape = if running {
            None
        } else {
            report.and_then(|(_, report)| match (&report.picture, &report.variant) {
                (Some((width, height)), _) => Some(*height as f32 / *width as f32),
                (None, Some(run::Variant::Screen { width, height, .. })) if *width > 0 => {
                    Some(*height as f32 / *width as f32)
                }
                _ => None,
            })
        };
        // The picture, as large as fits in `room` (width, height): given
        // its size in so many words, since a picture left to fill a box
        // takes the box's width whatever its shape.
        let screen = |at: usize, room: (f32, f32)| -> Option<AnyElement> {
            let fits = shape.map(|shape| {
                let width = room.0.min(room.1 / shape);
                (width, width * shape)
            });
            let (result, dir) = result_of(at)?;
            let shot = dir.join(result.screenshot.as_ref()?);
            let differs = result.visual.as_ref().filter(|visual| visual.differs());
            let accepted = differs
                .and_then(|visual| visual.accepted.as_ref())
                .map(|name| dir.join(name));
            let changes = differs
                .and_then(|visual| visual.diff.as_ref())
                .map(|name| dir.join(name));
            // The pictures there are to switch between, for a look that
            // differs.
            let mut choices = vec![(Picture::Now, "Now", shot.clone())];
            if let Some(accepted) = accepted {
                choices.push((Picture::Accepted, "Accepted", accepted));
            }
            if let Some(changes) = changes {
                choices.push((Picture::Changes, "Changes", changes));
            }
            let chosen = choices
                .iter()
                .position(|(kind, _, _)| *kind == picture)
                .unwrap_or(0);
            let kinds: Vec<Picture> = choices.iter().map(|(kind, _, _)| *kind).collect();
            Some(
                v_flex()
                    .gap_2()
                    .w_full()
                    .when(choices.len() > 1, |this| {
                        this.child(
                            h_flex().flex_none().child(
                                TabBar::new("test-pictures")
                                    .segmented()
                                    .xsmall()
                                    .selected_index(chosen)
                                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                                        this.picture =
                                            kinds.get(*ix).copied().unwrap_or(Picture::Now);
                                        cx.notify();
                                    }))
                                    .children(
                                        choices
                                            .iter()
                                            .map(|(_, label, _)| Tab::new().label(*label)),
                                    ),
                            ),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .map(|this| match fits {
                                Some((width, height)) => this
                                    .w(ph_ui::rems_from_px(width))
                                    .h(ph_ui::rems_from_px(height)),
                                // A run still going has not said: a wide
                                // box, which a page's picture fills.
                                None => this.w_full().h(ph_ui::rems_from_px(room.1.min(260.))),
                            })
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.muted.opacity(0.4))
                            .overflow_hidden()
                            .child(
                                img(choices[chosen].2.clone())
                                    .size_full()
                                    .object_fit(ObjectFit::Contain),
                            ),
                    )
                    .into_any_element(),
            )
        };

        // ---- The steps ----
        // The room a screen has under its step, in a narrow pane.
        let under = (
            width.map_or(320., |width| (width - 32. - UNDER_STEP - 8.).max(120.)),
            UNDER_TALLEST,
        );
        let editable = !busy;
        let rows = plan.iter().enumerate().map(|(at, step)| {
            let result = result_of(at).map(|(result, _)| result);
            let differs = result
                .and_then(|result| result.visual.as_ref())
                .filter(|visual| visual.differs());
            let is_look = matches!(step, Step::ExpectLook { .. });
            let unaccepted = is_look
                && result.is_some_and(|result| result.outcome == "passed" && result.visual.is_none());
            let (icon, color) = match result.map(|result| result.outcome.as_str()) {
                Some("passed") if differs.is_some() => (IconName::TriangleAlert, theme.warning),
                Some("passed") => (IconName::Check, theme.success),
                Some("failed") => (IconName::CircleX, theme.danger),
                _ => (IconName::Minus, muted),
            };
            // The step among the test's own (none for the one a run adds).
            let own = at.checked_sub(added);
            let picked = at_step == Some(at);
            let has_shot = result.is_some_and(|result| result.screenshot.is_some());
            let next_is_look = matches!(plan.get(at + 1), Some(Step::ExpectLook { .. }));
            let ignores = matches!(step, Step::ExpectLook { ignore, .. } if !ignore.is_empty());
            let (menu_panel, menu_path) = (cx.weak_entity(), path.clone());
            let line = h_flex()
                .id(("test-step", at))
                .group("test-step")
                .gap_2()
                .px_2()
                .min_h_7()
                .rounded(theme.radius)
                .text_sm()
                .when(picked, |this| this.bg(theme.list_active))
                .when(has_shot, |this| {
                    this.cursor_pointer()
                        .when(!picked, |this| this.hover(|style| style.bg(theme.list_hover)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.step = Some(at);
                            this.picture = Picture::Now;
                            cx.notify();
                        }))
                })
                .child(Icon::new(icon).xsmall().flex_none().text_color(color))
                .child(
                    div()
                        .flex_none()
                        .w(ph_ui::rems_from_px(18.))
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{}", at + 1)),
                )
                .child(div().flex_1().min_w_0().py_1().child(step.describe()))
                .children(
                    result
                        .filter(|result| result.outcome != "skipped")
                        .map(|result| {
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .child(took(result.millis))
                        }),
                )
                .when_some(own.filter(|_| editable), |this, own| {
                    this.child(
                        div()
                            .flex_none()
                            .invisible()
                            .group_hover("test-step", |style| style.visible())
                            .child(
                                Button::new(("test-step-more", at))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Ellipsis)
                                    .tooltip("More")
                                    .accessibility_label("More about this step")
                                    .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                                        let change = |act: fn(&mut TestCase, usize)| {
                                            let (panel, path) =
                                                (menu_panel.clone(), menu_path.clone());
                                            move |_: &gpui_kit::ClickEvent,
                                                  _: &mut Window,
                                                  cx: &mut gpui_kit::App| {
                                                _ = panel.update(cx, |this, cx| {
                                                    this.change_test(
                                                        &path,
                                                        |test| act(test, own),
                                                        cx,
                                                    );
                                                    this.step = None;
                                                });
                                            }
                                        };
                                        menu.when(!is_look && !next_is_look, |menu| {
                                            menu.item(
                                                PopupMenuItem::new(
                                                    "Check how the screen looks after this",
                                                )
                                                .on_click(change(|test, own| {
                                                    let id = model::new_look_id(test);
                                                    test.steps.insert(
                                                        own + 1,
                                                        Step::ExpectLook {
                                                            id,
                                                            ignore: Vec::new(),
                                                        },
                                                    );
                                                })),
                                            )
                                        })
                                        .when(ignores, |menu| {
                                            menu.item(
                                                PopupMenuItem::new("Compare the whole screen again")
                                                    .on_click(change(|test, own| {
                                                        if let Some(Step::ExpectLook {
                                                            ignore, ..
                                                        }) = test.steps.get_mut(own)
                                                        {
                                                            ignore.clear();
                                                        }
                                                    })),
                                            )
                                        })
                                        .item(
                                            PopupMenuItem::new("Remove this step")
                                                .icon(Icon::new(Icons::Trash))
                                                .on_click(change(|test, own| {
                                                    if own < test.steps.len() {
                                                        test.steps.remove(own);
                                                    }
                                                })),
                                        )
                                    }),
                            ),
                    )
                })
                .test_support();
            // What a failed step says, and what would get past it.
            let failure = result.and_then(|result| {
                let error = result.error.clone()?;
                let hint = result.hint.clone().map(|hint| match hint {
                    Hint::StartApp { .. } => Button::new("test-hint-run")
                        .xsmall()
                        .outline()
                        .icon(IconName::Play)
                        .label("Open Run")
                        .tooltip("Where the app is started")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(TestsPanelEvent::ShowRun))),
                    Hint::Secret { name } => Button::new("test-hint-secret")
                        .xsmall()
                        .outline()
                        .icon(Icons::KeyRound)
                        .label("Enter it")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.ask_secret(name.clone(), window, cx)
                        })),
                });
                Some(
                    v_flex()
                        .pl(ph_ui::rems_from_px(UNDER_STEP))
                        .pr_2()
                        .pb_1()
                        .gap_1()
                        .items_start()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.danger)
                                .line_clamp(6)
                                .child(error),
                        )
                        .children(hint),
                )
            });
            // A look check: nothing accepted yet, or how it differs.
            let look = if let Some(visual) = differs {
                let areas = if visual.ignorable.is_empty() {
                    visual.areas.clone()
                } else {
                    visual.ignorable.clone()
                };
                let (accept_path, ignore_path) = (path.clone(), path.clone());
                let said = if visual.resized {
                    "The screen has another size than the accepted look.".to_string()
                } else {
                    format!(
                        "Looks different: {} changed, in {}.",
                        plural(visual.pixels as usize, "pixel"),
                        plural(visual.areas.len().max(1), "place")
                    )
                };
                Some(
                    v_flex()
                        .pl(ph_ui::rems_from_px(UNDER_STEP))
                        .pr_2()
                        .pb_1()
                        .gap_1()
                        .child(div().text_xs().text_color(theme.warning).child(said))
                        .child(
                            h_flex()
                                .gap_1()
                                .flex_wrap()
                                .child(
                                    Button::new(("test-look-accept", at))
                                        .xsmall()
                                        .outline()
                                        .label("Accept new look")
                                        .disabled(busy)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.accept_looks(&accept_path, Some(at), cx)
                                        })),
                                )
                                .when_some(
                                    own.filter(|_| !areas.is_empty() && editable),
                                    |this, own| {
                                        this.child(
                                            Button::new(("test-look-ignore", at))
                                                .xsmall()
                                                .ghost()
                                                .label("Ignore what changed")
                                                .tooltip(ph_ui::tip(
                                                    "For what changes by itself, like a clock: those areas are left out from now on",
                                                ))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.ignore_changes(
                                                        &ignore_path,
                                                        own,
                                                        areas.clone(),
                                                        cx,
                                                    )
                                                })),
                                        )
                                    },
                                ),
                        ),
                )
            } else if unaccepted {
                let accept_path = path.clone();
                Some(
                    v_flex()
                        .pl(ph_ui::rems_from_px(UNDER_STEP))
                        .pr_2()
                        .pb_1()
                        .gap_1()
                        .items_start()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("No look is accepted yet: there is nothing to compare with."),
                        )
                        .child(
                            Button::new(("test-look-accept", at))
                                .xsmall()
                                .outline()
                                .label("Accept this look")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.accept_looks(&accept_path, Some(at), cx)
                                })),
                        ),
                )
            } else {
                None
            };
            v_flex()
                .child(line)
                .children(failure)
                .children(look)
                // In a narrow pane the screen opens under its step.
                .children((!beside && picked).then(|| screen(at, under)).flatten().map(
                    |screen| {
                        div()
                            .flex_none()
                            .pl(ph_ui::rems_from_px(UNDER_STEP))
                            .pr_2()
                            .pb_2()
                            .child(screen)
                    },
                ))
        });
        // As tall as the room under the test's name, whatever the steps
        // need: a column left to its own height never has more than fits,
        // and so nothing to scroll.
        let steps = v_flex()
            .id("test-steps")
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .overflow_y_scroll()
            .gap_0p5()
            .children(rows);
        // Beside the steps the screen has the room of its share of the
        // pane, and a height a pane of any size can show.
        let side_width = width.map_or(SIDE_MAX, |width| ((width - 44.) * SIDE_SHARE).min(SIDE_MAX));
        // No taller than the window leaves for it under the test's name
        // and what goes with it.
        let tallest = (ph_ui::measure::design_px(window.viewport_size().height, window)
            - ABOVE_SCREEN)
            .clamp(160., SIDE_TALLEST);
        let side = (beside && !plan.is_empty())
            .then(|| at_step.and_then(|at| screen(at, (side_width, tallest))))
            .flatten();

        // The steps scroll in the room under the test's name and what goes
        // with it (which is kept short: a few lines of what it checks, a
        // proposed fix that scrolls by itself), and the screen stays beside
        // them meanwhile.
        v_flex()
            .id("ui-test")
            .relative()
            .size_full()
            .p_4()
            .gap_3()
            .child(probe)
            .child(heading)
            .child(actions)
            .children(kept)
            .children(secret)
            .children(variants)
            .children(status)
            .children(message)
            .children(stale_note)
            .children(proposal)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .items_start()
                    .child(steps)
                    .children(side.map(|side| {
                        div()
                            .flex_none()
                            .w(ph_ui::rems_from_px(side_width))
                            .child(side)
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Element;

    fn test_case(target: Target) -> TestCase {
        TestCase {
            name: "Sign in".into(),
            description: "Sign in as Ada".into(),
            target,
            clean: None,
            steps: Vec::new(),
        }
    }

    #[test]
    fn the_agent_is_told_how_to_record() {
        let web = record_prompt(&test_case(Target::Web {
            url: "http://localhost:3000".into(),
        }));
        assert!(web.contains("named “Sign in”") && web.contains("Sign in as Ada"));
        assert!(web.contains("Open http://localhost:3000 with browser_open."));
        assert!(web.contains("kind look") && web.contains("signed out"));
        let phone = record_prompt(&test_case(Target::Android {
            package: "com.app".into(),
        }));
        assert!(phone.contains("Start com.app with device_launch.") && phone.contains("device_*"));
    }

    #[test]
    fn a_repair_goes_on_from_the_broken_step() {
        let mut test = test_case(Target::Web {
            url: "http://x".into(),
        });
        let click = Step::Click {
            element: Element {
                role: Some("button".into()),
                name: Some("Sign in".into()),
                ..Element::default()
            },
        };
        test.steps = vec![click.clone(), Step::ExpectText { text: "Hi".into() }];
        let plan = run::plan(&test);
        let result = StepResult {
            step: click,
            outcome: "failed".into(),
            error: Some("No button “Sign in” shows.".into()),
            hint: None,
            millis: 0,
            screenshot: None,
            visual: None,
        };
        let prompt = repair_prompt(&test, &plan, 1, &result);
        assert!(
            prompt.contains("broke at step 2 (“Click the “Sign in” button”)"),
            "{prompt}"
        );
        assert!(prompt.contains("Click the “Sign in” button; Check that “Hi” shows"));
        assert!(prompt.contains("the browser is in the state"));
    }
}
