//! The list of the Tests tab: the project's suites with their tests under
//! them, then the UI tests. Every row is one line of one height, so a suite
//! of thousands of tests draws only the rows in view.

use std::ops::Range;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::Input;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, uniform_list,
};
use ph_ui::Icons;

use super::{Picked, TestsPanel, id_of};
use crate::record;
use crate::runner;
use crate::suite::{Case, Outcome};

/// A line of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Row {
    /// The name of a part of the list.
    Section(&'static str),
    /// A suite, by its place in the panel's suites.
    Suite(usize),
    /// What a suite's tests belong to (a crate, a class, a file), with how
    /// many of them failed and how many there are.
    Group {
        suite: usize,
        group: String,
        failed: usize,
        total: usize,
    },
    /// A test of a suite: the suite, and the test's place in its result.
    /// `deep` under a group, else right under the suite.
    Case {
        suite: usize,
        case: usize,
        deep: bool,
    },
    /// A UI test, by its place in the panel's tests.
    Check(usize),
    /// Nothing matches what is looked for.
    Nothing,
}

/// What a suite or one of its groups is known by among those shown open.
pub(crate) fn open_key(suite: &str, group: Option<&str>) -> String {
    match group {
        Some(group) => format!("{suite}\u{0}{group}"),
        None => suite.to_string(),
    }
}

/// How a row's state is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Passed,
    Failed,
    /// Never ran, or was skipped.
    None,
    Running,
    Waiting,
}

impl TestsPanel {
    /// A suite that just ran shows what failed in it: it is opened, and the
    /// groups with a failure in them; the others are closed.
    pub(super) fn open_failed(&mut self, suite: &str) {
        let Some(result) = self.results.get(suite) else {
            return;
        };
        let prefix = open_key(suite, Some(""));
        self.open.retain(|key| !key.starts_with(&prefix));
        let failed: Vec<&Case> = result
            .cases
            .iter()
            .filter(|case| case.outcome == Outcome::Failed)
            .collect();
        if failed.is_empty() {
            self.open.remove(suite);
            return;
        }
        self.open.insert(open_key(suite, None));
        for case in failed {
            self.open.insert(open_key(suite, Some(&case.group)));
        }
    }

    /// Makes the list's lines from what there is, what is open, what is
    /// looked for and whether only failures are wanted.
    pub(super) fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let wanted = self.search.read(cx).value().trim().to_lowercase();
        let searching = !wanted.is_empty();
        let says = |text: &str| text.to_lowercase().contains(&wanted);
        let mut rows = Vec::new();

        let mut suite_rows = Vec::new();
        for (ix, suite) in self.suites.iter().enumerate() {
            let result = self.results.get(&suite.id);
            let cases: &[Case] = result
                .map(|result| result.cases.as_slice())
                .unwrap_or_default();
            let suite_failed = result.is_some_and(|result| !result.passed());
            if self.only_failed && !suite_failed {
                continue;
            }
            // The tests of it that are shown: all, the failed, or those
            // found (all of a suite found by its own name).
            let named = !searching || says(&suite.name);
            let shown: Vec<usize> = cases
                .iter()
                .enumerate()
                .filter(|(_, case)| !self.only_failed || case.outcome == Outcome::Failed)
                .filter(|(_, case)| named || says(&case.name) || says(&case.group))
                .map(|(at, _)| at)
                .collect();
            if searching && !named && shown.is_empty() {
                continue;
            }
            suite_rows.push(Row::Suite(ix));
            // Found tests are shown without having to open what they are in.
            let open = (searching && !says(&suite.name)) || self.open.contains(&suite.id);
            if !open {
                continue;
            }
            let mut groups: Vec<&str> = Vec::new();
            for at in &shown {
                let group = cases[*at].group.as_str();
                if !groups.contains(&group) {
                    groups.push(group);
                }
            }
            // One group says nothing the suite does not: its tests stand
            // right under the suite.
            let flat = groups.len() <= 1;
            for group in groups {
                let of_group = |at: &&usize| cases[**at].group == group;
                if !flat {
                    suite_rows.push(Row::Group {
                        suite: ix,
                        group: group.to_string(),
                        failed: shown
                            .iter()
                            .filter(of_group)
                            .filter(|at| cases[**at].outcome == Outcome::Failed)
                            .count(),
                        total: shown.iter().filter(of_group).count(),
                    });
                    let open = searching || self.open.contains(&open_key(&suite.id, Some(group)));
                    if !open {
                        continue;
                    }
                }
                // What failed first: it is what is looked for.
                let mut of: Vec<usize> = shown.iter().filter(of_group).copied().collect();
                of.sort_by_key(|at| cases[*at].outcome != Outcome::Failed);
                suite_rows.extend(of.into_iter().map(|case| Row::Case {
                    suite: ix,
                    case,
                    deep: !flat,
                }));
            }
        }
        if !suite_rows.is_empty() {
            rows.push(Row::Section("Project tests"));
            rows.extend(suite_rows);
        }

        let checks: Vec<Row> = self
            .tests
            .iter()
            .enumerate()
            .filter(|(_, (path, test))| {
                let failed = self
                    .reports
                    .get(path)
                    .is_some_and(|reports| reports.iter().any(|(_, report)| !report.passed()));
                (!self.only_failed || failed) && (!searching || says(&test.name))
            })
            .map(|(ix, _)| Row::Check(ix))
            .collect();
        if !checks.is_empty() {
            rows.push(Row::Section("UI tests"));
            rows.extend(checks);
        }
        if rows.is_empty() && (searching || self.only_failed) {
            rows.push(Row::Nothing);
        }
        self.rows = rows;
    }

    /// What a row picks, if it picks anything.
    fn picks(&self, row: &Row) -> Option<Picked> {
        Some(match row {
            Row::Suite(ix) => Picked::Suite(self.suites.get(*ix)?.id.clone()),
            Row::Case { suite, case, .. } => {
                let suite = self.suites.get(*suite)?;
                let case = self.results.get(&suite.id)?.cases.get(*case)?;
                Picked::Case {
                    suite: suite.id.clone(),
                    group: case.group.clone(),
                    name: case.name.clone(),
                }
            }
            Row::Check(ix) => Picked::Check(self.tests.get(*ix)?.0.clone()),
            Row::Section(_) | Row::Group { .. } | Row::Nothing => return None,
        })
    }

    /// Picks the row `by` rows after (or before) the one picked: the arrow
    /// keys, in the list.
    fn step_pick(&mut self, by: isize, cx: &mut Context<Self>) {
        let pickable: Vec<(usize, Picked)> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(at, row)| Some((at, self.picks(row)?)))
            .collect();
        if pickable.is_empty() {
            return;
        }
        let now = pickable
            .iter()
            .position(|(_, picked)| Some(picked) == self.picked.as_ref());
        let next = match now {
            Some(now) => (now as isize + by).clamp(0, pickable.len() as isize - 1) as usize,
            None => 0,
        };
        let (at, picked) = pickable[next].clone();
        self.list_scroll
            .scroll_to_item(at, gpui_kit::ScrollStrategy::Center);
        // The keys move in the list: the pane does not leave it.
        let inside = self.inside;
        self.pick(picked, cx);
        self.inside = inside;
    }

    fn toggle(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.open.remove(&key) {
            self.open.insert(key);
        }
        self.rebuild_rows(cx);
        cx.notify();
    }

    pub(super) fn render_list(&self, alone: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let count = self.rows.len();
        let list = uniform_list(
            "tests-rows",
            count,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .filter_map(|at| Some(this.render_row(at, this.rows.get(at)?.clone(), cx)))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.list_scroll)
        .size_full();
        v_flex()
            .id("tests-list")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "down" => this.step_pick(1, cx),
                    "up" => this.step_pick(-1, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .h_full()
            .map(|this| {
                if alone {
                    this.w_full()
                } else {
                    // A third of the pane, within what a name needs and
                    // what the detail can spare.
                    this.flex_none()
                        .w(gpui_kit::relative(0.36))
                        .min_w(ph_ui::rems_from_px(200.))
                        .max_w(ph_ui::rems_from_px(320.))
                        .border_r_1()
                        .border_color(theme.border)
                }
            })
            .child(
                div().flex_none().p_2().child(
                    Input::new(&self.search).small().cleanable(true).prefix(
                        Icon::new(IconName::Search)
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    ),
                ),
            )
            .child(div().flex_1().min_h_0().px_1().pb_1().child(list))
    }

    fn render_row(&self, at: usize, row: Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let runs = self.runs.read(cx);
        let busy = self.busy(cx);
        // What every line shares: its height, and its place by depth.
        let line = |id: gpui_kit::SharedString, depth: f32| {
            h_flex()
                .id(id)
                .h_7()
                .w_full()
                .gap_1p5()
                .pr_1()
                .pl(ph_ui::rems_from_px(6. + depth * 14.))
                .rounded(theme.radius)
                .text_sm()
        };
        let mark = |state: State| -> AnyElement {
            let (icon, color) = match state {
                State::Running => {
                    return div()
                        .flex_none()
                        .child(ph_ui::Loader::new().xsmall())
                        .into_any_element();
                }
                State::Passed => (Icon::new(IconName::CircleCheck), theme.success),
                State::Failed => (Icon::new(IconName::CircleX), theme.danger),
                State::Waiting => (Icon::new(Icons::Clock), muted),
                State::None => (Icon::new(IconName::Minus), muted),
            };
            icon.xsmall()
                .flex_none()
                .text_color(color)
                .into_any_element()
        };
        let chevron = |open: bool| {
            Icon::new(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall()
            .flex_none()
            .text_color(muted)
        };
        let picked = self
            .picks(&row)
            .filter(|picked| Some(picked) == self.picked.as_ref());
        let pickable = |element: gpui_kit::Stateful<gpui_kit::Div>, picks: Picked| {
            let selected = picked.is_some();
            element
                .cursor_pointer()
                .when(selected, |this| this.bg(theme.list_active))
                .when(!selected, |this| {
                    this.hover(|style| style.bg(theme.list_hover))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.pick(picks.clone(), cx);
                }))
        };
        match row {
            Row::Section(title) => h_flex()
                .h_7()
                .px_2()
                .when(at > 0, |this| this.mt_1())
                .text_xs()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(muted)
                .child(title)
                .into_any_element(),
            Row::Nothing => h_flex()
                .h_7()
                .px_2()
                .text_sm()
                .text_color(muted)
                .child(if self.only_failed {
                    "Nothing failed."
                } else {
                    "No test of that name."
                })
                .into_any_element(),
            Row::Suite(ix) => {
                let Some(suite) = self.suites.get(ix) else {
                    return div().into_any_element();
                };
                let result = self.results.get(&suite.id);
                let state = if runs.running_suite() == Some(suite.id.as_str()) {
                    State::Running
                } else if runs.waits(None, Some(&suite.id)) {
                    State::Waiting
                } else {
                    match result {
                        Some(result) if result.passed() => State::Passed,
                        Some(result) if !result.stopped => State::Failed,
                        _ => State::None,
                    }
                };
                let has_tests = result.is_some_and(|result| !result.cases.is_empty());
                let key = open_key(&suite.id, None);
                let open = self.open.contains(&key);
                let badge = result.map(runner::suite_badge).unwrap_or_default();
                let bad = state == State::Failed;
                let (id, name) = (suite.id.clone(), suite.name.clone());
                pickable(
                    line(id_of("suite", &suite.id), 0.),
                    Picked::Suite(suite.id.clone()),
                )
                .group("test-row")
                .child(
                    div()
                        .id(id_of("suite-open", &suite.id))
                        .flex_none()
                        .when(!has_tests, |this| this.invisible())
                        .when(has_tests, |this| {
                            this.cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle(key.clone(), cx);
                                }))
                        })
                        .child(chevron(open)),
                )
                .child(mark(state))
                .child(div().flex_1().min_w_0().truncate().child(name))
                .child(trailing(
                    div()
                        .text_xs()
                        .text_color(if bad { theme.danger } else { muted })
                        .child(badge)
                        .into_any_element(),
                    Button::new(id_of("suite-run", &suite.id))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Play)
                        .tooltip("Run")
                        .accessibility_label("Run this suite")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.run_suite(&id, Vec::new(), cx);
                        })),
                ))
                .test_support()
                .into_any_element()
            }
            Row::Group {
                suite,
                group,
                failed,
                total,
            } => {
                let Some(suite) = self.suites.get(suite) else {
                    return div().into_any_element();
                };
                let key = open_key(&suite.id, Some(&group));
                let open = self.open.contains(&key);
                line(id_of("test-group", &format!("{}-{group}", suite.id)), 1.)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.list_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(key.clone(), cx)))
                    .child(chevron(open))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(short_group(&group).to_string()),
                    )
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(group.clone()).build(window, cx)
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(if failed > 0 { theme.danger } else { muted })
                            .child(if failed > 0 {
                                format!("{failed} failed")
                            } else {
                                total.to_string()
                            }),
                    )
                    .test_support()
                    .into_any_element()
            }
            Row::Case { suite, case, deep } => {
                let found = self
                    .suites
                    .get(suite)
                    .and_then(|suite| Some((suite, self.results.get(&suite.id)?.cases.get(case)?)));
                let Some((suite, case)) = found else {
                    return div().into_any_element();
                };
                let state = match case.outcome {
                    Outcome::Passed => State::Passed,
                    Outcome::Failed => State::Failed,
                    Outcome::Skipped => State::None,
                };
                pickable(
                    line(
                        id_of(
                            "test-case",
                            &format!("{}-{}-{}", suite.id, case.group, case.name),
                        ),
                        if deep { 2. } else { 1. },
                    ),
                    Picked::Case {
                        suite: suite.id.clone(),
                        group: case.group.clone(),
                        name: case.name.clone(),
                    },
                )
                .child(mark(state))
                .child(div().flex_1().min_w_0().truncate().child(case.name.clone()))
                .tooltip({
                    // A long name is cut: all of it, under the pointer.
                    let name = gpui_kit::SharedString::from(case.name.clone());
                    move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(name.clone()).build(window, cx)
                    }
                })
                .test_support()
                .into_any_element()
            }
            Row::Check(ix) => {
                let Some((path, test)) = self.tests.get(ix) else {
                    return div().into_any_element();
                };
                let recording =
                    record::current(cx).is_some_and(|recording| &recording.path == path);
                let state = if runs.running() == Some(path) || recording {
                    State::Running
                } else if runs.waits(Some(path), None) {
                    State::Waiting
                } else {
                    match self.reports.get(path) {
                        Some(reports) if reports.iter().all(|(_, report)| report.passed()) => {
                            State::Passed
                        }
                        Some(_) => State::Failed,
                        None => State::None,
                    }
                };
                let run = path.clone();
                let unrecorded = test.steps.is_empty();
                pickable(
                    line(id_of("ui-test", &crate::model::slug(&test.name)), 0.),
                    Picked::Check(path.clone()),
                )
                .group("test-row")
                // Where a suite has its arrow: the names line up.
                .child(div().flex_none().invisible().child(chevron(false)))
                .child(mark(state))
                .child(div().flex_1().min_w_0().truncate().child(test.name.clone()))
                .child(trailing(
                    Icon::new(if test.is_android() {
                        Icons::Smartphone
                    } else {
                        Icons::Globe
                    })
                    .xsmall()
                    .text_color(muted)
                    .into_any_element(),
                    Button::new(id_of("ui-test-run", &crate::model::slug(&test.name)))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Play)
                        .tooltip("Run")
                        .accessibility_label("Run this test")
                        .disabled(busy || unrecorded)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.run_check(&run, false, cx);
                        })),
                ))
                .test_support()
                .into_any_element()
            }
        }
    }
}

/// A group's name as a row has room for: the end of a dotted one, which is
/// what tells it from its neighbours (`tests.test_cart.TestCart` →
/// `test_cart.TestCart`). A file's path, or a short name, stays whole.
fn short_group(group: &str) -> &str {
    if group.contains(['/', '\\', ' ']) {
        return group;
    }
    let mut dots = group.rmatch_indices('.').map(|(at, _)| at);
    match (dots.next(), dots.next()) {
        (Some(_), Some(second)) => &group[second + 1..],
        _ => group,
    }
}

/// The end of a row: what it says at rest (a count, where it runs), and in
/// its place, under the pointer, the button that runs it.
fn trailing(at_rest: AnyElement, run: Button) -> impl IntoElement {
    h_flex()
        .relative()
        .flex_none()
        .justify_end()
        .min_w(ph_ui::rems_from_px(24.))
        .child(
            div()
                .group_hover("test-row", |style| style.invisible())
                .child(at_rest),
        )
        .child(
            h_flex()
                .absolute()
                .top_0()
                .right_0()
                .h_full()
                .invisible()
                .group_hover("test-row", |style| style.visible())
                .child(run),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_group_is_said_by_its_end() {
        assert_eq!(
            short_group("tests.test_cart.TestCart"),
            "test_cart.TestCart"
        );
        assert_eq!(short_group("shop.StockTest"), "shop.StockTest");
        assert_eq!(short_group("src/cart.test.js"), "src/cart.test.js");
        assert_eq!(short_group("api (tests/api.rs)"), "api (tests/api.rs)");
        assert_eq!(open_key("cargo", Some("shop")), "cargo\u{0}shop");
    }
}
