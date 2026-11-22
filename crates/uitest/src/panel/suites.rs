//! What the Tests tab shows of a suite of the project, and of one of its
//! tests: how it went, what it printed or why it failed, and the ways on
//! from there (run it again, have an agent fix it, open the file).

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div,
};
use ph_ui::{Icons, MenuButton as _};

use super::{TestsPanel, TestsPanelEvent, mono, took};
use crate::suite::{Case, Outcome, Suite, SuiteResult};
use crate::suite_run;

/// The lines of output shown: its end, where a run says how it went.
const OUTPUT_LINES: usize = 400;
/// Failures named to an agent at most; it finds the rest by running.
const FAILURES_TOLD: usize = 12;

/// What an agent is asked to fix: the command, and what failed with why.
fn fix_prompt(suite: &Suite, failed: &[&Case], result: &SuiteResult) -> String {
    let mut prompt = format!(
        "The project's tests fail. They are run with `{}` in `{}`.\n\n",
        suite.command,
        suite.cwd.display()
    );
    if failed.is_empty() {
        // It did not get as far as a test: what it printed says why.
        let end: Vec<&str> = result.output.lines().rev().take(60).collect();
        prompt.push_str(
            "The command failed before any test said why. The end of what it printed:\n\n```\n",
        );
        prompt.push_str(&end.into_iter().rev().collect::<Vec<_>>().join("\n"));
        prompt.push_str("\n```\n\n");
    } else {
        prompt.push_str(&format!("{} failed:\n\n", plural(failed.len(), "test")));
        for case in failed.iter().take(FAILURES_TOLD) {
            prompt.push_str(&format!("- `{}`", case.name));
            if !case.group.is_empty() {
                prompt.push_str(&format!(" ({})", case.group));
            }
            if let (Some(file), Some(line)) = (&case.file, case.line) {
                prompt.push_str(&format!(" at {file}:{line}"));
            }
            prompt.push('\n');
            if let Some(message) = &case.message {
                let short: String = message.chars().take(1200).collect();
                prompt.push_str(&format!("\n```\n{short}\n```\n\n"));
            }
        }
        if failed.len() > FAILURES_TOLD {
            prompt.push_str(&format!("…and {} more.\n\n", failed.len() - FAILURES_TOLD));
        }
    }
    prompt.push_str(
        "Find out why: read the tests and the code they check before changing either. Fix the code when it is wrong, and the test only when it is the test that is wrong (say which, and why). Then run the tests again and say how they went.",
    );
    prompt
}

/// `1 test`, `3 tests`.
pub(super) fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// The line under a name that says how a run went: its mark in the colour
/// of how it went, the words as text.
pub(super) fn result_line(
    icon: IconName,
    color: gpui_kit::Hsla,
    text: impl Into<SharedString>,
    aside: Option<String>,
    cx: &gpui_kit::App,
) -> gpui_kit::Div {
    h_flex()
        .gap_2()
        .text_sm()
        .child(Icon::new(icon).small().flex_none().text_color(color))
        .child(div().flex_1().min_w_0().child(text.into()))
        .children(aside.map(|aside| {
            div()
                .flex_none()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(aside)
        }))
}

/// A boxed block of fixed-width text: as tall as what it says, and
/// scrolling where that is more than the room there is.
pub(super) fn text_box(id: &'static str, text: &str, cx: &gpui_kit::App) -> impl IntoElement {
    div()
        .id(id)
        .flex_shrink(1.)
        .min_h(ph_ui::rems_from_px(48.))
        .overflow_y_scroll()
        .p_2()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted.opacity(0.4))
        .child(mono(text, OUTPUT_LINES, cx))
}

impl TestsPanel {
    fn fix_with_agent(
        &mut self,
        suite: &str,
        only: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let (Some(suite), Some(result)) = (self.suite(suite), self.results.get(suite)) else {
            return;
        };
        let failed: Vec<&Case> = result
            .cases
            .iter()
            .filter(|case| case.outcome == Outcome::Failed)
            .filter(|case| {
                only.as_ref()
                    .is_none_or(|(group, name)| &case.group == group && &case.name == name)
            })
            .collect();
        cx.emit(TestsPanelEvent::Agent {
            title: format!("Fix {}", suite.name),
            prompt: fix_prompt(suite, &failed, result),
        });
    }

    pub(super) fn render_suite(&mut self, id: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(suite) = self.suite(id).cloned() else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let result = self.results.get(id);
        let runs = self.runs.read(cx);
        let running = runs.running_suite() == Some(id);
        let waiting = runs.waits(None, Some(id));
        let live = if running {
            runs.live_output(OUTPUT_LINES)
        } else {
            None
        };
        let busy = self.busy(cx);
        let failed: Vec<Case> = result
            .map(|result| {
                result
                    .cases
                    .iter()
                    .filter(|case| case.outcome == Outcome::Failed)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let bad = result.is_some_and(|result| !result.passed() && !result.stopped);
        let (run_id, failed_id, fix_id, remove_id) = (
            id.to_string(),
            id.to_string(),
            id.to_string(),
            id.to_string(),
        );
        let custom = suite.custom;
        let panel = cx.weak_entity();

        let actions = h_flex()
            .gap_1()
            .flex_wrap()
            .child(
                Button::new("suite-run")
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
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.run_suite(&run_id, Vec::new(), cx)),
                    ),
            )
            .when(!failed.is_empty() && suite.runs_parts(), |this| {
                let failed = failed.clone();
                this.child(
                    Button::new("suite-run-failed")
                        .small()
                        .ghost()
                        .label("Run failed")
                        .tooltip(ph_ui::tip(if suite.runs_one() {
                            "Only the tests that failed"
                        } else {
                            "Only the files with a test that failed"
                        }))
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.run_suite(&failed_id, failed.clone(), cx)
                        })),
                )
            })
            .when(bad, |this| {
                this.child(
                    Button::new("suite-fix")
                        .small()
                        .ghost()
                        .icon(Icons::Wrench)
                        .label("Fix with agent")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.fix_with_agent(&fix_id, None, cx)
                        })),
                )
            })
            .when(custom, |this| {
                this.child(
                    Button::new("suite-more")
                        .small()
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .tooltip("More")
                        .accessibility_label("More about this command")
                        .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                            let (panel, id) = (panel.clone(), remove_id.clone());
                            menu.item(
                                PopupMenuItem::new("Remove this command")
                                    .icon(Icon::new(Icons::Trash))
                                    .on_click(move |_, _, cx| {
                                        _ = panel
                                            .update(cx, |this, cx| this.remove_command(&id, cx));
                                    }),
                            )
                        }),
                )
            });

        let status = match (&live, result) {
            (Some((_, elapsed)), _) => Some(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(muted)
                    .child(ph_ui::Loader::new().small())
                    .child(format!("Running · {}", took(elapsed.as_millis() as u64))),
            ),
            (None, Some(result)) => {
                let (icon, color) = if result.stopped {
                    (IconName::Minus, muted)
                } else if result.passed() {
                    (IconName::CircleCheck, theme.success)
                } else {
                    (IconName::CircleX, theme.danger)
                };
                let mut said = suite_run::summary(result);
                if result.partial {
                    said.push_str(" (after running a part of it again)");
                }
                Some(result_line(
                    icon,
                    color,
                    said,
                    Some(format!(
                        "{} · {}",
                        took(result.millis),
                        ph_ui::time::ago(result.started_at)
                    )),
                    cx,
                ))
            }
            (None, None) => None,
        };
        let output = match (&live, result) {
            (Some((output, _)), _) => Some(output.clone()),
            (None, Some(result)) => Some(result.output.clone()),
            (None, None) => None,
        };
        // A runner whose results are read says so by having tests; another
        // gets told what it is shown by.
        let unread = result.is_some_and(|result| result.cases.is_empty() && !result.stopped)
            && !suite.runs_parts();
        v_flex()
            .size_full()
            .p_4()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(suite.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(muted)
                            .child(suite.command.clone()),
                    ),
            )
            .child(actions)
            .children(status)
            .when(unread, |this| {
                this.child(div().text_xs().text_color(muted).child(
                    "It passes when the command ends without an error: its tests are not told apart.",
                ))
            })
            .map(|this| match output {
                Some(output) if !output.trim().is_empty() => this
                    .child(
                        h_flex()
                            .flex_none()
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(muted)
                                    .child("Output"),
                            )
                            .child(
                                ph_ui::CopyButton::new("suite-output-copy", output.clone())
                                    .xsmall()
                                    .tooltip("Copy the output"),
                            ),
                    )
                    .child(text_box("suite-output", &output, cx)),
                Some(_) => this,
                None => this.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("Not run yet."),
                ),
            })
            .into_any_element()
    }

    pub(super) fn render_case(
        &mut self,
        suite_id: &str,
        group: &str,
        name: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let found = self.suite(suite_id).cloned().zip(
            self.results
                .get(suite_id)
                .and_then(|result| {
                    result
                        .cases
                        .iter()
                        .find(|case| case.group == group && case.name == name)
                })
                .cloned(),
        );
        let Some((suite, case)) = found else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let busy = self.busy(cx);
        let (icon, color, said) = match case.outcome {
            Outcome::Passed => (IconName::CircleCheck, theme.success, "Passed"),
            Outcome::Failed => (IconName::CircleX, theme.danger, "Failed"),
            Outcome::Skipped => (IconName::Minus, muted, "Skipped"),
        };
        // Where it is: in the suite's folder, unless it says the whole way.
        let place = case.file.as_ref().map(|file| {
            let path = PathBuf::from(file);
            let path = if path.is_absolute() {
                path
            } else {
                suite.cwd.join(path)
            };
            let label = match case.line {
                Some(line) => format!("{file}:{line}"),
                None => file.clone(),
            };
            (path, label)
        });
        let (run_id, fix_id) = (suite_id.to_string(), suite_id.to_string());
        let (again, fix) = (case.clone(), (case.group.clone(), case.name.clone()));
        let line = case.line;
        v_flex()
            .size_full()
            .p_4()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(case.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(if case.group.is_empty() {
                                suite.name.clone()
                            } else {
                                format!("{} · {}", case.group, suite.name)
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .when(suite.runs_parts(), |this| {
                        this.child(
                            Button::new("case-run")
                                .small()
                                .primary()
                                .icon(IconName::Play)
                                .label(if suite.runs_one() {
                                    "Run again"
                                } else {
                                    "Run its file again"
                                })
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.run_suite(&run_id, vec![again.clone()], cx)
                                })),
                        )
                    })
                    .when(case.outcome == Outcome::Failed, |this| {
                        this.child(
                            Button::new("case-fix")
                                .small()
                                .ghost()
                                .icon(Icons::Wrench)
                                .label("Fix with agent")
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.fix_with_agent(&fix_id, Some(fix.clone()), cx)
                                })),
                        )
                    })
                    .children(place.map(|(path, label)| {
                        Button::new("case-open")
                            .small()
                            .ghost()
                            .icon(Icons::FileCode)
                            .label(label)
                            .tooltip("Open it")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(TestsPanelEvent::Open {
                                    path: path.clone(),
                                    line,
                                })
                            }))
                    })),
            )
            .child(result_line(icon, color, said, case.millis.map(took), cx))
            .children(case.message.as_ref().map(|message| {
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .flex_none()
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(muted)
                                    .child("Why"),
                            )
                            .child(
                                ph_ui::CopyButton::new("case-message-copy", message.clone())
                                    .xsmall()
                                    .tooltip("Copy"),
                            ),
                    )
                    .child(text_box("case-message", message, cx))
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suite::Kind;

    #[test]
    fn an_agent_is_told_what_failed_and_why() {
        let suite = Suite {
            id: "cargo".into(),
            name: "cargo test".into(),
            kind: Kind::Cargo,
            command: "cargo test --workspace".into(),
            cwd: "/p".into(),
            custom: false,
        };
        let case = Case {
            group: "cart".into(),
            name: "tests::totals".into(),
            outcome: Outcome::Failed,
            millis: None,
            message: Some("left: 3\nright: 4".into()),
            file: Some("src/lib.rs".into()),
            line: Some(42),
        };
        let result = SuiteResult {
            started_at: 0,
            millis: 0,
            exit: Some(101),
            stopped: false,
            cases: vec![case.clone()],
            output: "error[E0308]: mismatched types\n".into(),
            partial: false,
        };
        let prompt = fix_prompt(&suite, &[&case], &result);
        assert!(
            prompt.contains("`cargo test --workspace` in `/p`"),
            "{prompt}"
        );
        assert!(prompt.contains("1 test failed"));
        assert!(prompt.contains("- `tests::totals` (cart) at src/lib.rs:42"));
        assert!(prompt.contains("left: 3"));
        // Nothing failed by name: it did not build, and the output says why.
        let prompt = fix_prompt(&suite, &[], &result);
        assert!(prompt.contains("before any test said why") && prompt.contains("E0308"));
        assert_eq!(plural(1, "test"), "1 test");
        assert_eq!(plural(3, "place"), "3 places");
    }
}
