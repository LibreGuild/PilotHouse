//! What stands between a pull request and its merge, under the
//! conversation: who reviewed it and how, its checks, and the merge itself.
//!
//! A failing check opens to what it pointed at and the part of its log
//! around the first error, so the reason is read here; from there it is run
//! again, or fixed in a task. The merge says why it cannot happen yet (a
//! conflict, a missing review or check, a branch behind its base) instead
//! of offering a button GitHub will refuse.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::PopupMenuItem,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use ph_github::models::Check;
use ph_github::pulls::{self, CheckDetail, MergeMethod};
use ph_ui::{Icons, MenuButton as _};

use super::{Loaded, PullView, PullViewEvent, check_icon};

/// What is known of a failed check, once asked for.
pub(super) enum CheckState {
    Loading,
    Read(CheckDetail),
    Failed(SharedString),
}

/// Whether the pull request can be merged, and what to say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mergeable {
    Ready,
    /// Checks fail or still run, but GitHub would take the merge.
    Unstable,
    /// Its branch is behind the base.
    Behind,
    Draft,
    Conflicts,
    /// A required review or check is missing.
    Blocked,
    /// GitHub has not worked it out yet.
    Unknown,
}

impl Mergeable {
    pub(super) fn of(pull: &ph_github::models::PullRequest) -> Self {
        if pull.draft {
            return Mergeable::Draft;
        }
        match (pull.mergeable, pull.mergeable_state.as_str()) {
            (Some(false), _) | (_, "dirty") => Mergeable::Conflicts,
            (_, "blocked") => Mergeable::Blocked,
            (_, "behind") => Mergeable::Behind,
            (_, "unstable") => Mergeable::Unstable,
            (None, _) | (_, "unknown") => Mergeable::Unknown,
            _ => Mergeable::Ready,
        }
    }

    fn says(self, base: &str) -> String {
        match self {
            Mergeable::Ready => "Ready to merge.".into(),
            Mergeable::Unstable => "Checks are failing or still running.".into(),
            Mergeable::Behind => format!("Its branch is behind {base}."),
            Mergeable::Draft => "A draft: it is marked ready on GitHub before it is merged.".into(),
            Mergeable::Conflicts => {
                format!("It conflicts with {base}: that is resolved on its branch.")
            }
            Mergeable::Blocked => "A required review or check is missing.".into(),
            Mergeable::Unknown => "GitHub is working out whether it can be merged.".into(),
        }
    }

    fn can_merge(self) -> bool {
        matches!(
            self,
            Mergeable::Ready | Mergeable::Unstable | Mergeable::Behind
        )
    }
}

fn method_label(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "Merge commit",
        MergeMethod::Squash => "Squash and merge",
        MergeMethod::Rebase => "Rebase and merge",
    }
}

impl PullView {
    /// Opens or folds failed check `check`; what it says is read the first
    /// time.
    fn toggle_check(&mut self, check: &Check, cx: &mut Context<Self>) {
        let key = check_key(check);
        if !self.checks_open.remove(&key) {
            self.checks_open.insert(key);
            if let Some(id) = check.id
                && !self.check_details.contains_key(&id)
            {
                self.check_details.insert(id, CheckState::Loading);
                let (client, repo, check) = (
                    self.hosting.client.clone(),
                    self.hosting.repo.clone(),
                    check.clone(),
                );
                let read =
                    cx.background_spawn(async move { pulls::check_detail(&client, &repo, &check) });
                self._checks.push(cx.spawn(async move |this, cx| {
                    let detail = read.await;
                    _ = this.update(cx, |this, cx| {
                        this.check_details.insert(
                            id,
                            match detail {
                                Ok(detail) => CheckState::Read(detail),
                                Err(err) => CheckState::Failed(format!("{err:#}").into()),
                            },
                        );
                        cx.notify();
                    });
                }));
            }
        }
        cx.notify();
    }

    /// A task on the pull request's branch whose agent makes `check` pass,
    /// given what the check said.
    fn fix_check(&mut self, check: &Check, cx: &mut Context<Self>) {
        let Some(data) = self.data.clone() else {
            return;
        };
        let (annotations, log) = match check.id.and_then(|id| self.check_details.get(&id)) {
            Some(CheckState::Read(detail)) => (
                detail
                    .annotations
                    .iter()
                    .map(|note| {
                        (
                            format!("{}:{}", note.path, note.start_line),
                            match &note.title {
                                Some(title) if !title.is_empty() => {
                                    format!("{title}: {}", note.message)
                                }
                                _ => note.message.clone(),
                            },
                        )
                    })
                    .collect(),
                detail.log.clone(),
            ),
            _ => (Vec::new(), check.summary.clone()),
        };
        cx.emit(PullViewEvent::Agent(crate::AgentRequest::FixCheck {
            pull: Box::new(data.pull.clone()),
            name: check.name.clone(),
            annotations,
            log,
        }));
    }

    fn update_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(head) = self.data.as_ref().map(|data| data.pull.head.sha.clone()) else {
            return;
        };
        let number = self.number;
        self.act(
            "Updating the branch",
            "The base was merged into the branch",
            None,
            move |h| pulls::update_branch(&h.client, &h.repo, number, &head),
            window,
            cx,
        );
    }

    /// The pull request's commits, folded to one line until asked for.
    pub(super) fn render_commits(&self, data: &Loaded, cx: &mut Context<Self>) -> AnyElement {
        let count = data.commits.len();
        if count == 0 {
            return div().into_any_element();
        }
        v_flex()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .id("pull-commits")
                    .role(gpui_kit::accesskit::Role::Button)
                    .aria_label(SharedString::from(format!("{count} commits")))
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .cursor_pointer()
                    .text_sm()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.commits_open = !this.commits_open;
                        cx.notify();
                    }))
                    .child(
                        Icon::new(Icons::GitCompareArrows)
                            .small()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().flex_1().child(match count {
                        1 => "1 commit".to_string(),
                        count => format!("{count} commits"),
                    }))
                    .child(
                        Icon::new(if self.commits_open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .test_support(),
            )
            .when(self.commits_open, |this| {
                this.children(data.commits.iter().map(|commit| {
                    let subject = commit
                        .commit
                        .message
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    h_flex()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(cx.theme().muted_foreground)
                                .child(commit.sha[..commit.sha.len().min(7)].to_string()),
                        )
                        .child(div().flex_1().min_w_0().truncate().text_sm().child(subject))
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} · {}",
                                    commit.commit.author.name,
                                    ph_ui::time::ago_of(&commit.commit.author.date)
                                )),
                        )
                }))
            })
            .into_any_element()
    }

    /// Who decided what: each reviewer's newest word, and who is still
    /// asked.
    fn render_reviewers(&self, data: &Loaded, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mut latest: Vec<(&str, &str)> = Vec::new();
        for review in &data.reviews {
            let state = review.state.as_str();
            if !matches!(state, "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED") {
                continue;
            }
            let login = review.user.login.as_str();
            latest.retain(|(known, _)| *known != login);
            if state != "DISMISSED" {
                latest.push((login, state));
            }
        }
        let me = &self.hosting.account.login;
        let asked: Vec<String> = data
            .pull
            .requested_reviewers
            .iter()
            .map(|user| {
                if user.login.eq_ignore_ascii_case(me) {
                    "you".to_string()
                } else {
                    user.login.clone()
                }
            })
            .chain(
                data.pull
                    .requested_teams
                    .iter()
                    .map(|team| team.name.clone()),
            )
            .collect();
        if latest.is_empty() && asked.is_empty() {
            return None;
        }
        let row = |icon: IconName, color: gpui_kit::Hsla, text: String| {
            h_flex()
                .gap_2()
                .text_sm()
                .child(Icon::new(icon).small().text_color(color))
                .child(div().flex_1().min_w_0().child(text))
        };
        Some(
            v_flex()
                .gap_1()
                .children(latest.into_iter().map(|(login, state)| {
                    if state == "APPROVED" {
                        row(
                            IconName::CircleCheck,
                            cx.theme().success,
                            format!("{login} approved"),
                        )
                    } else {
                        row(
                            IconName::CircleX,
                            cx.theme().danger,
                            format!("{login} asked for changes"),
                        )
                    }
                }))
                .when(!asked.is_empty(), |this| {
                    this.child(row(
                        IconName::Eye,
                        cx.theme().muted_foreground,
                        format!("Waiting for {}", asked.join(", ")),
                    ))
                })
                .into_any_element(),
        )
    }

    /// One check: its outcome; a failed one opens to why.
    fn render_check(&self, ix: usize, check: &Check, cx: &mut Context<Self>) -> AnyElement {
        let (icon, color) = check_icon(&check.outcome, cx);
        let failed = check.failed();
        let open = self.checks_open.contains(&check_key(check));
        let url = check.url.clone();
        let outcome = match check.outcome.as_str() {
            "success" => "passed",
            "failure" => "failed",
            "timed_out" => "timed out",
            "action_required" => "needs someone",
            "in_progress" | "queued" | "pending" => "running",
            other => other,
        }
        .replace('_', " ");
        v_flex()
            .child(
                h_flex()
                    .id(("check", ix))
                    .gap_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    // A failed check opens here; any other goes to its page.
                    .when(failed || !url.is_empty(), |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(cx.theme().list_hover))
                    })
                    .map(|this| {
                        if failed {
                            let check = check.clone();
                            this.on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_check(&check, cx)),
                            )
                        } else if !url.is_empty() {
                            let url = url.clone();
                            this.on_click(move |_, _, cx| cx.open_url(&url))
                        } else {
                            this
                        }
                    })
                    .child(Icon::new(icon).small().text_color(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(check.name.clone()),
                    )
                    .child(div().flex_none().text_xs().text_color(color).child(outcome))
                    .when(failed, |this| {
                        this.child(
                            Icon::new(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                        )
                    })
                    .test_support(),
            )
            .when(failed && open, |this| {
                this.child(self.render_check_detail(ix, check, cx))
            })
            .into_any_element()
    }

    /// Why a check failed, and what can be done about it.
    fn render_check_detail(&self, ix: usize, check: &Check, cx: &mut Context<Self>) -> AnyElement {
        // A log keeps its place by its check, not by its row: the rows
        // change places when the checks are read again.
        let log_id = |name: &'static str| match check.id {
            Some(id) => gpui_kit::ElementId::from((name, id)),
            None => gpui_kit::ElementId::from((name, ix)),
        };
        let state = check.id.and_then(|id| self.check_details.get(&id));
        let muted = cx.theme().muted_foreground;
        let url = check.url.clone();
        let busy = self.busy.is_some();
        v_flex()
            .gap_2()
            .ml_6()
            .mb_2()
            .map(|this| match state {
                Some(CheckState::Loading) => this.child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(muted)
                        .child(ph_ui::Loader::new().xsmall())
                        .child("Reading what it said…"),
                ),
                Some(CheckState::Failed(why)) => {
                    this.child(div().text_xs().text_color(muted).child(why.clone()))
                }
                Some(CheckState::Read(detail)) => {
                    let nothing = detail.annotations.is_empty()
                        && detail.log.trim().is_empty()
                        && check.summary.trim().is_empty();
                    this.when(!check.summary.trim().is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .child(check.summary.chars().take(600).collect::<String>()),
                        )
                    })
                    .children(detail.annotations.iter().take(8).map(|note| {
                        v_flex()
                            .text_xs()
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(cx.theme().link)
                                    .child(format!("{}:{}", note.path, note.start_line)),
                            )
                            .child(div().child(note.message.chars().take(400).collect::<String>()))
                    }))
                    .when(!detail.log.trim().is_empty(), |this| {
                        this.child(
                            div()
                                .id(log_id("check-log"))
                                .max_h(ph_ui::rems_from_px(260.))
                                // A log's lines are long: both bars, each
                                // check's log with its own place.
                                .overflow_scrollbar()
                                .id(log_id("check-log-scroll"))
                                .p_2()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().secondary)
                                .text_xs()
                                .font_family(cx.theme().mono_font_family.clone())
                                .whitespace_nowrap()
                                .children(
                                    detail
                                        .log
                                        .lines()
                                        .map(|line| div().child(line.to_string())),
                                ),
                        )
                    })
                    .when(nothing, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("It said nothing that can be read here: its page has the rest."),
                        )
                    })
                }
                // A commit status: only a link.
                None => this.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("This check reports only a link: its page says why."),
                ),
            })
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new(("check-fix", ix))
                            .xsmall()
                            .outline()
                            .icon(Icons::GitBranch)
                            .label("Fix in a task")
                            .tooltip(ph_ui::tip("A task on the pull request's branch; its agent is given what the check said"))
                            .on_click(cx.listener({
                                let check = check.clone();
                                move |this, _, _, cx| this.fix_check(&check, cx)
                            })),
                    )
                    .when(check.id.is_some(), |this| {
                        this.child(
                            Button::new(("check-rerun", ix))
                                .xsmall()
                                .ghost()
                                .icon(IconName::RefreshCw)
                                .label("Run again")
                                .disabled(busy)
                                .on_click(cx.listener({
                                    let check = check.clone();
                                    move |this, _, window, cx| {
                                        let check = check.clone();
                                        this.act(
                                            "Asking for another run",
                                            "GitHub runs it again",
                                            None,
                                            move |h| pulls::rerun_check(&h.client, &h.repo, &check),
                                            window,
                                            cx,
                                        )
                                    }
                                })),
                        )
                    })
                    .when(!url.is_empty(), |this| {
                        this.child(
                            Button::new(("check-open", ix))
                                .xsmall()
                                .ghost()
                                .icon(IconName::ExternalLink)
                                .label("Its page")
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The box under the conversation: reviewers, checks, the merge.
    pub(super) fn render_status(&self, data: &Loaded, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.is_some();
        let mergeable = Mergeable::of(&data.pull);
        let open = data.pull.state == "open" && !data.pull.merged;
        let failing = data.checks.iter().filter(|check| check.failed()).count();
        let running = data
            .checks
            .iter()
            .filter(|check| matches!(check.outcome.as_str(), "pending" | "queued" | "in_progress"))
            .count();
        // What needs a look first.
        let mut checks: Vec<(usize, &Check)> = data.checks.iter().enumerate().collect();
        checks.sort_by_key(|(_, check)| (!check.failed(), check.outcome == "success"));
        let muted = cx.theme().muted_foreground;
        let part = move |title: String| {
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(muted)
                .child(title)
        };
        let view = cx.weak_entity();
        let method = self.merge_method;
        v_flex()
            .id("pull-status")
            .gap_3()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .children(self.render_reviewers(data, cx).map(|reviewers| {
                v_flex()
                    .gap_1()
                    .child(part("Reviews".into()))
                    .child(reviewers)
            }))
            .child(
                v_flex()
                    .gap_0p5()
                    .child(part(match (data.checks.len(), failing, running) {
                        (0, ..) => "Checks".to_string(),
                        (_, 0, 0) => "Checks · all passed".to_string(),
                        (_, 0, running) => format!("Checks · {running} running"),
                        (_, failing, _) => format!("Checks · {failing} failing"),
                    }))
                    .when(data.checks.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("None reported for the newest commit."),
                        )
                    })
                    .children(
                        checks
                            .into_iter()
                            .map(|(ix, check)| self.render_check(ix, check, cx)),
                    ),
            )
            .when(open, |this| {
                this.child(
                    h_flex()
                        .gap_1p5()
                        .flex_wrap()
                        .justify_end()
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                // A line of its own where the buttons do
                                // not fit beside it.
                                .flex_grow(1.)
                                .min_w(ph_ui::rems_from_px(200.))
                                .text_sm()
                                .text_color(match mergeable {
                                    Mergeable::Ready => cx.theme().success,
                                    Mergeable::Conflicts | Mergeable::Blocked => cx.theme().danger,
                                    _ => cx.theme().muted_foreground,
                                })
                                .child(mergeable.says(&data.pull.base.name)),
                        )
                        .when(mergeable == Mergeable::Behind, |this| {
                            this.child(
                                Button::new("pull-update-branch")
                                    .small()
                                    .outline()
                                    .label("Update branch")
                                    .tooltip(ph_ui::tip(
                                        "Merges the base into the pull request's branch on GitHub",
                                    ))
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.update_branch(window, cx)
                                    })),
                            )
                        })
                        .when(mergeable.can_merge(), |this| {
                            this.child(
                                Button::new("pull-merge-method")
                                    .small()
                                    .ghost()
                                    .label(method_label(method))
                                    .dropdown_caret(true)
                                    .tooltip("How it is merged")
                                    .menu(gpui_kit::Anchor::BottomRight, move |menu, _, _| {
                                        let mut menu = menu;
                                        for choice in [
                                            MergeMethod::Squash,
                                            MergeMethod::Merge,
                                            MergeMethod::Rebase,
                                        ] {
                                            let view = view.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(method_label(choice))
                                                    .checked(choice == method)
                                                    .on_click(move |_, _, cx| {
                                                        _ = view.update(cx, |this, cx| {
                                                            this.merge_method = choice;
                                                            this.confirm_merge = false;
                                                            cx.notify();
                                                        });
                                                    }),
                                            );
                                        }
                                        menu
                                    }),
                            )
                            .child(
                                Button::new("pull-merge")
                                    .small()
                                    .when(self.confirm_merge, |this| this.danger())
                                    .when(!self.confirm_merge, |this| this.outline())
                                    .label(match (self.confirm_merge, mergeable) {
                                        (true, Mergeable::Unstable) => "Merge with failing checks",
                                        (true, _) => "Confirm merge",
                                        (false, _) => "Merge",
                                    })
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.merge(window, cx)),
                                    ),
                            )
                        }),
                )
            })
            .test_support()
            .into_any_element()
    }
}

/// What names a check among the others of a commit.
fn check_key(check: &Check) -> String {
    match check.id {
        Some(id) => id.to_string(),
        None => check.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_github::models::PullRequest;

    #[test]
    fn says_whether_it_can_be_merged() {
        let pull = |mergeable: Option<bool>, state: &str, draft: bool| PullRequest {
            mergeable,
            mergeable_state: state.into(),
            draft,
            ..PullRequest::default()
        };
        assert_eq!(
            Mergeable::of(&pull(Some(true), "clean", false)),
            Mergeable::Ready
        );
        assert_eq!(
            Mergeable::of(&pull(Some(true), "blocked", false)),
            Mergeable::Blocked
        );
        assert_eq!(
            Mergeable::of(&pull(Some(true), "behind", false)),
            Mergeable::Behind
        );
        assert_eq!(
            Mergeable::of(&pull(Some(true), "unstable", false)),
            Mergeable::Unstable
        );
        assert_eq!(
            Mergeable::of(&pull(Some(false), "dirty", false)),
            Mergeable::Conflicts
        );
        assert_eq!(Mergeable::of(&pull(None, "", false)), Mergeable::Unknown);
        assert_eq!(
            Mergeable::of(&pull(Some(true), "clean", true)),
            Mergeable::Draft
        );
        assert!(!Mergeable::Blocked.can_merge() && Mergeable::Unstable.can_merge());
    }
}
