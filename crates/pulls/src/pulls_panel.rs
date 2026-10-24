//! The Pull requests tab: the project's open PRs (all, mine, waiting for my
//! review) with CI state, a form to open one for the current branch, and
//! the PR view.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    notification::Notification,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, div,
};
use ph_changes::{Repo as LocalRepo, Writer, clean_message, cut};
use ph_github::models::{Check, PullRequest};
use ph_github::pulls;
use ph_store::Store;
use ph_ui::{EmptyState, Icons};

use crate::hosting::{self, Hosting, Missing};
use crate::pull_view::{self, PullView, PullViewEvent};

/// CI is read for this many PRs at the top of the list.
const CHECKS_FOR: usize = 20;

#[derive(Clone)]
pub enum PullsPanelEvent {
    OpenSettings,
    ReviewWithAgent {
        hosting: Box<Hosting>,
        pull: Box<PullRequest>,
    },
    OpenSession(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Mine,
    ToReview,
}

enum State {
    Loading,
    Missing(Missing),
    Ready(Hosting),
}

pub struct PullsPanel {
    store: Store,
    local: Entity<LocalRepo>,
    root: PathBuf,
    state: State,
    me: Option<String>,
    pulls: Vec<PullRequest>,
    checks: HashMap<u64, Vec<Check>>,
    filter: Filter,
    loading: bool,
    error: Option<SharedString>,
    open: Option<(Entity<PullView>, Subscription)>,
    create: Option<CreateForm>,
    writer: Option<Writer>,
    _load: Option<Task<()>>,
}

/// The "new pull request" form.
struct CreateForm {
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    base: Entity<InputState>,
    draft: bool,
    busy: Option<&'static str>,
    _task: Option<Task<()>>,
}

impl EventEmitter<PullsPanelEvent> for PullsPanel {}

impl PullsPanel {
    pub fn new(store: Store, local: Entity<LocalRepo>, cx: &mut Context<Self>) -> Self {
        let root = local.read(cx).root().to_path_buf();
        let mut panel = Self {
            store,
            local,
            root,
            state: State::Loading,
            me: None,
            pulls: Vec::new(),
            checks: HashMap::new(),
            filter: Filter::All,
            loading: false,
            error: None,
            open: None,
            create: None,
            writer: None,
            _load: None,
        };
        panel.reload(cx);
        panel
    }

    pub fn set_writer(&mut self, writer: Writer) {
        self.writer = Some(writer);
    }

    /// Re-reads the remote and account, then the pull requests.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let (root, store) = (self.root.clone(), self.store.clone());
        self.loading = true;
        cx.notify();
        let load = cx.background_spawn(async move {
            let hosting = hosting::resolve(&root, &store)?;
            let me = pulls::me(&hosting.client).ok().map(|user| user.login);
            let list = pulls::list(&hosting.client, &hosting.repo);
            let checks: HashMap<u64, Vec<Check>> = list
                .as_ref()
                .map(|list| {
                    list.iter()
                        .take(CHECKS_FOR)
                        .filter_map(|pull| {
                            pulls::checks(&hosting.client, &hosting.repo, &pull.head.sha)
                                .ok()
                                .map(|checks| (pull.number, checks))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok((hosting, me, list, checks))
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result: Result<_, Missing> = load.await;
            _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((hosting, me, list, checks)) => {
                        this.state = State::Ready(hosting);
                        this.me = me;
                        match list {
                            Ok(list) => {
                                this.pulls = list;
                                this.error = None;
                            }
                            Err(err) => this.error = Some(format!("{err:#}").into()),
                        }
                        this.checks = checks;
                    }
                    Err(missing) => this.state = State::Missing(missing),
                }
                cx.notify();
            });
        }));
    }

    /// The pull requests last loaded.
    pub fn list(&self) -> &[PullRequest] {
        &self.pulls
    }

    fn hosting(&self) -> Option<&Hosting> {
        match &self.state {
            State::Ready(hosting) => Some(hosting),
            _ => None,
        }
    }

    /// Shows pull request `number`.
    pub fn open_pull(&mut self, number: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hosting) = self.hosting().cloned() else {
            return;
        };
        let (local, store) = (self.local.clone(), self.store.clone());
        let view = cx.new(|cx| PullView::new(store, hosting.clone(), local, number, window, cx));
        let subscription = cx.subscribe(
            &view,
            move |this, _, event: &PullViewEvent, cx| match event {
                PullViewEvent::Back => {
                    this.open = None;
                    this.reload(cx);
                }
                PullViewEvent::ReviewWithAgent(pull) => cx.emit(PullsPanelEvent::ReviewWithAgent {
                    hosting: Box::new(hosting.clone()),
                    pull: pull.clone(),
                }),
                PullViewEvent::OpenSession(session) => {
                    cx.emit(PullsPanelEvent::OpenSession(*session))
                }
            },
        );
        self.open = Some((view, subscription));
        cx.notify();
    }

    fn show_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let branch = self.local.read(cx).branch().unwrap_or_default().to_string();
        let title = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Title");
            input.set_value(branch.replace(['-', '_', '/'], " "), window, cx);
            input
        });
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Description (Markdown)")
                .rows(6)
        });
        let base = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Base branch");
            input.set_value(default_base(&self.root), window, cx);
            input
        });
        self.create = Some(CreateForm {
            title,
            body,
            base,
            draft: false,
            busy: None,
            _task: None,
        });
        cx.notify();
    }

    /// The agent writes the description from the branch's commits and diff.
    fn write_description(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(writer), Some(form)) = (self.writer.clone(), self.create.as_mut()) else {
            return;
        };
        let base = form.base.read(cx).value().trim().to_string();
        form.busy = Some("Writing…");
        cx.notify();
        let root = self.root.clone();
        let collect = cx.background_spawn({
            let root = root.clone();
            let base = base.clone();
            async move {
                let range = format!("origin/{base}..HEAD");
                let commits = ph_git::log::summaries(&root, &range)?;
                let stat = ph_git::git(&root, &["diff", "--stat", "--no-ext-diff", "--no-textconv", &format!("origin/{base}...HEAD")])?;
                let diff = ph_git::git(&root, &["diff", "--no-color", "--no-ext-diff", "--no-textconv", &format!("origin/{base}...HEAD")])?;
                anyhow::Ok(format!(
                    "Write a pull request description in Markdown for these changes: a short \\
                     summary, then a bullet list of what changed and why, then how to test \\
                     it. Reply with the description only and do not use tools.\\n\\nCommits:\\n{commits}\\n\\
                     Files:\\n{stat}\\n\\nDiff:\\n```diff\\n{}\\n```",
                    cut(&diff, 14_000)
                ))
            }
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let prompt = match collect.await {
                Ok(prompt) => prompt,
                Err(err) => {
                    _ = this.update_in(cx, |this, window, cx| {
                        if let Some(form) = &mut this.create {
                            form.busy = None;
                        }
                        window.push_notification(Notification::error(format!("{err:#}")), cx);
                    });
                    return;
                }
            };
            let Ok(write) = this.update(cx, |_, cx| writer(prompt, root, cx)) else {
                return;
            };
            let result = write.await;
            _ = this.update_in(cx, |this, window, cx| {
                let Some(form) = &mut this.create else { return };
                form.busy = None;
                match result {
                    Ok(text) => form.body.update(cx, |body, cx| {
                        body.set_value(clean_message(&text), window, cx)
                    }),
                    Err(err) => {
                        window.push_notification(Notification::error(format!("{err:#}")), cx)
                    }
                }
                cx.notify();
            });
        });
        if let Some(form) = &mut self.create {
            form._task = Some(task);
        }
    }

    /// Pushes the branch if it has no upstream, then opens the PR.
    fn create_pull(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(hosting), Some(form)) = (self.hosting().cloned(), self.create.as_mut()) else {
            return;
        };
        let title = form.title.read(cx).value().trim().to_string();
        let body = form.body.read(cx).value().to_string();
        let base = form.base.read(cx).value().trim().to_string();
        let draft = form.draft;
        let status = self.local.read(cx).status().cloned().unwrap_or_default();
        let Some(head) = status.branch.clone() else {
            window.push_notification(Notification::warning("Switch to a branch first."), cx);
            return;
        };
        form.busy = Some("Opening…");
        cx.notify();
        let root = self.root.clone();
        let work = cx.background_spawn(async move {
            if status.upstream.is_none() || status.ahead > 0 {
                ph_git::branches::push(&root, status.upstream.is_some())?;
            }
            pulls::create(
                &hosting.client,
                &hosting.repo,
                &head,
                &base,
                &title,
                &body,
                draft,
            )
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(pull) => {
                        this.create = None;
                        window.push_notification(
                            Notification::success(format!("Opened #{}", pull.number)),
                            cx,
                        );
                        this.reload(cx);
                        this.open_pull(pull.number, window, cx);
                    }
                    Err(err) => {
                        if let Some(form) = &mut this.create {
                            form.busy = None;
                        }
                        window.push_notification(
                            Notification::error(format!("{err:#}")).title("GitHub"),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        });
        if let Some(form) = &mut self.create {
            form._task = Some(task);
        }
    }

    fn render_missing(&self, missing: &Missing, cx: &mut Context<Self>) -> AnyElement {
        let (title, description, settings) = match missing {
            Missing::NotRepository => ("Not a git repository", "Pull requests need a git repository with a GitHub remote.".to_string(), false),
            Missing::NoRemote => ("No GitHub remote", "Add a remote (git remote add origin …) that points at GitHub or GitHub Enterprise.".to_string(), false),
            Missing::NoAccount { host } => ("Connect GitHub", format!("Add a token for {host} in Settings → GitHub to see and review pull requests here."), true),
            Missing::Failed(reason) => ("Could not connect", reason.clone(), true),
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                EmptyState::new(title)
                    .icon(Icons::GitPullRequest)
                    .description(description),
            )
            .when(settings, |this| {
                this.child(
                    Button::new("pulls-open-settings")
                        .small()
                        .primary()
                        .label("Open Settings")
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(PullsPanelEvent::OpenSettings)),
                        ),
                )
            })
            .child(
                Button::new("pulls-retry")
                    .small()
                    .ghost()
                    .label("Check again")
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .into_any_element()
    }

    fn render_create(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.create.as_ref()?;
        let busy = form.busy;
        Some(
            v_flex()
                .gap_2()
                .m_3()
                .p_3()
                .rounded(cx.theme().radius_lg)
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(format!(
                            "New pull request from {}",
                            self.local.read(cx).branch().unwrap_or("this branch")
                        )),
                )
                .child(Input::new(&form.title).small())
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().text_xs().child("Into"))
                        .child(div().flex_1().child(Input::new(&form.base).small())),
                )
                .child(Textarea::new(&form.body).small())
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Checkbox::new("pull-draft")
                                .label("Draft")
                                .checked(form.draft)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    if let Some(form) = &mut this.create {
                                        form.draft = *checked;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(div().flex_1())
                        .when(self.writer.is_some(), |this| {
                            this.child(
                                Button::new("pull-write-description")
                                    .small()
                                    .ghost()
                                    .icon(Icons::Sparkles)
                                    .label("Write description")
                                    .disabled(busy.is_some())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.write_description(window, cx)
                                    })),
                            )
                        })
                        .child(
                            Button::new("pull-create-cancel")
                                .small()
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.create = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("pull-create")
                                .small()
                                .primary()
                                .label(busy.unwrap_or("Open pull request"))
                                .disabled(busy.is_some())
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.create_pull(window, cx)),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_list(&self, hosting: &Hosting, cx: &mut Context<Self>) -> AnyElement {
        let me = self.me.clone().unwrap_or_default();
        let visible: Vec<&PullRequest> = self
            .pulls
            .iter()
            .filter(|pull| match self.filter {
                Filter::All => true,
                Filter::Mine => pull.user.login == me,
                Filter::ToReview => pull.requested_reviewers.iter().any(|user| user.login == me),
            })
            .collect();
        let branch = self.local.read(cx).branch().map(str::to_string);
        let has_pr = branch
            .as_ref()
            .is_some_and(|branch| self.pulls.iter().any(|pull| &pull.head.name == branch));
        let filter = |value: Filter, label: String, cx: &mut Context<Self>| {
            let selected = self.filter == value;
            div()
                .id(SharedString::from(format!("pulls-filter-{label}")))
                .px_2p5()
                .py_1()
                .rounded(cx.theme().radius)
                .text_sm()
                .cursor_pointer()
                .text_color(if selected {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .when(selected, |this| {
                    this.bg(cx.theme().secondary)
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.filter = value;
                    cx.notify();
                }))
                .child(label)
        };
        let to_review = self
            .pulls
            .iter()
            .filter(|pull| pull.requested_reviewers.iter().any(|u| u.login == me))
            .count();
        let web = hosting.web_url();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_2()
                    .pt_2()
                    .gap_1()
                    .child(filter(
                        Filter::All,
                        format!("Open {}", self.pulls.len()),
                        cx,
                    ))
                    .child(filter(Filter::Mine, "Mine".into(), cx))
                    .child(filter(
                        Filter::ToReview,
                        if to_review > 0 {
                            format!("To review {to_review}")
                        } else {
                            "To review".into()
                        },
                        cx,
                    ))
                    .child(div().flex_1())
                    .when(self.loading, |this| this.child(Spinner::new().xsmall()))
                    .when(
                        !has_pr && branch.is_some() && self.create.is_none(),
                        |this| {
                            this.child(
                                Button::new("pull-new")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Plus)
                                    .label("New")
                                    .tooltip("Open a pull request for the current branch")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_create(window, cx)
                                    })),
                            )
                        },
                    )
                    .child(
                        Button::new("pulls-web")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ExternalLink)
                            .tooltip(format!(
                                "{}/{} on GitHub",
                                hosting.repo.owner, hosting.repo.name
                            ))
                            .accessibility_label("Open the repository on GitHub")
                            .on_click(move |_, _, cx| cx.open_url(&format!("{web}/pulls"))),
                    )
                    .child(
                        Button::new("pulls-refresh")
                            .xsmall()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .accessibility_label("Refresh pull requests")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    ),
            )
            .children(self.render_create(cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                v_flex()
                    .id("pulls-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .gap_1()
                    .when(visible.is_empty() && !self.loading, |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No open pull requests here."),
                        )
                    })
                    .children(
                        visible
                            .into_iter()
                            .map(|pull| self.render_row(pull, &me, cx)),
                    ),
            )
            .into_any_element()
    }

    fn render_row(&self, pull: &PullRequest, me: &str, cx: &mut Context<Self>) -> AnyElement {
        let number = pull.number;
        let (state, color) = pull_view::state_badge(pull, cx);
        let checks = self.checks.get(&number);
        let ci = checks.map(|checks| {
            let outcome = if checks.iter().any(|c| c.outcome == "failure") {
                "failure"
            } else if checks.iter().any(|c| c.outcome == "pending") {
                "pending"
            } else if checks.is_empty() {
                "none"
            } else {
                "success"
            };
            pull_view::check_icon(outcome, cx)
        });
        let review_me = pull.requested_reviewers.iter().any(|user| user.login == me);
        v_flex()
            .id(("pull", number))
            .flex_none()
            .gap_0p5()
            .px_2p5()
            .py_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().list_hover))
            .on_click(cx.listener(move |this, _, window, cx| this.open_pull(number, window, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(Icons::GitPullRequest).small().text_color(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(pull.title.clone()),
                    )
                    .when_some(ci, |this, (icon, color)| {
                        this.child(Icon::new(icon).small().text_color(color))
                    }),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "#{number} · {} · {} → {}",
                        pull.user.login, pull.head.name, pull.base.name
                    ))
                    .when(state != "Open", |this| {
                        this.child(div().text_color(color).child(state))
                    })
                    .when(review_me, |this| {
                        this.child(
                            div()
                                .text_color(cx.theme().warning)
                                .child("Review requested"),
                        )
                    })
                    .children(pull.labels.iter().take(3).map(|label| {
                        let tint: gpui_kit::Hsla = u32::from_str_radix(&label.color, 16)
                            .map(|hex| gpui_kit::rgb(hex).into())
                            .unwrap_or(cx.theme().muted_foreground);
                        div()
                            .px_1p5()
                            .rounded_full()
                            .border_1()
                            .border_color(tint.opacity(0.6))
                            .text_color(tint)
                            .child(label.name.clone())
                    })),
            )
            .into_any_element()
    }
}

/// `main` unless the remote's default branch says otherwise.
fn default_base(root: &std::path::Path) -> String {
    ph_git::git(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .ok()
    .and_then(|name| {
        name.trim()
            .split_once('/')
            .map(|(_, branch)| branch.to_string())
    })
    .unwrap_or_else(|| "main".into())
}

impl Render for PullsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((view, _)) = &self.open {
            return view.clone().into_any_element();
        }
        match &self.state {
            State::Loading => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element(),
            State::Missing(missing) => {
                let missing = missing.clone();
                self.render_missing(&missing, cx)
            }
            State::Ready(hosting) => {
                let hosting = hosting.clone();
                self.render_list(&hosting, cx)
            }
        }
    }
}
