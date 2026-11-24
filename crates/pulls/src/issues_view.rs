//! A repository's issues, beside its pull requests: the list (open,
//! assigned to you, closed), and one issue in full with its comments.
//!
//! An issue is work waiting to be done, so what the page offers first is
//! doing it: **Fix in a task** starts a task whose agent is given the issue
//! and its comments, and **Copy for an agent** puts the same text on the
//! clipboard for a chat that is already open. A new issue is written here
//! too, with who it is assigned to.

use std::collections::HashMap;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    menu::PopupMenuItem,
    notification::Notification,
    text::{TextView, TextViewState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClipboardItem, Context, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
};
use ph_github::issues::{self, Filter};
use ph_github::models::{Issue, IssueComment};
use ph_ui::{Icons, MenuButton as _};

use crate::AgentRequest;
use crate::hosting::Hosting;
use crate::issue_form::{IssueForm, IssueFormEvent};

#[derive(Debug, Clone)]
pub enum IssuesEvent {
    /// Work for an agent about an issue.
    Agent(AgentRequest),
}

/// The issue shown in full.
struct Open {
    issue: Issue,
    comments: Vec<IssueComment>,
    /// The comments are still being read.
    loading: bool,
    body: Entity<TextViewState>,
    bodies: HashMap<u64, Entity<TextViewState>>,
}

pub struct IssuesView {
    hosting: Hosting,
    filter: Filter,
    issues: Vec<Issue>,
    loading: bool,
    /// The list was read at least once (it is read when first shown).
    read: bool,
    error: Option<SharedString>,
    search: Entity<InputState>,
    open: Option<Open>,
    composer: Entity<TextareaState>,
    busy: Option<&'static str>,
    form: Option<(Entity<IssueForm>, Subscription)>,
    _load: Option<Task<()>>,
    _open: Option<Task<()>>,
    _act: Option<Task<()>>,
    _search: Subscription,
}

impl EventEmitter<IssuesEvent> for IssuesView {}

fn markdown(text: &str, cx: &mut gpui_kit::App) -> Entity<TextViewState> {
    // (Text nested too deeply to parse is shown plain.)
    let text = ph_ui::markdown::guarded(text);
    cx.new(|cx| TextViewState::markdown(&text, cx))
}

impl IssuesView {
    pub fn new(hosting: Hosting, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find an issue"));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Leave a comment (Markdown)")
                .auto_grow(3, 12)
        });
        Self {
            hosting,
            filter: Filter::Open,
            issues: Vec::new(),
            loading: false,
            read: false,
            error: None,
            _search: cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            search,
            open: None,
            composer,
            busy: None,
            form: None,
            _load: None,
            _open: None,
            _act: None,
        }
    }

    /// Reads the list the first time it is shown.
    pub fn shown(&mut self, cx: &mut Context<Self>) {
        if !self.read && !self.loading {
            self.reload(cx);
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();
        let (client, repo) = (self.hosting.client.clone(), self.hosting.repo.clone());
        let (filter, me) = (self.filter, self.hosting.account.login.clone());
        let load = cx.background_spawn(async move { issues::list(&client, &repo, filter, &me) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.read = true;
                match result {
                    Ok(issues) => {
                        this.issues = issues;
                        this.error = None;
                    }
                    Err(err) => this.error = Some(format!("{err:#}").into()),
                }
                cx.notify();
            });
        }));
    }

    fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        if self.filter != filter {
            self.filter = filter;
            self.issues.clear();
            self.reload(cx);
        }
    }

    /// Shows `issue` in full, and reads its comments.
    fn open_issue(&mut self, issue: Issue, cx: &mut Context<Self>) {
        let number = issue.number;
        let body = markdown(issue.body.as_deref().unwrap_or_default(), cx);
        self.open = Some(Open {
            loading: issue.comments > 0,
            issue,
            comments: Vec::new(),
            body,
            bodies: HashMap::new(),
        });
        self.read_open(number, cx);
        cx.notify();
    }

    /// Reads the open issue again (itself and its comments).
    fn read_open(&mut self, number: u64, cx: &mut Context<Self>) {
        let (client, repo) = (self.hosting.client.clone(), self.hosting.repo.clone());
        let load = cx.background_spawn(async move {
            let issue = issues::get(&client, &repo, number)?;
            let comments = issues::comments(&client, &repo, number)?;
            anyhow::Ok((issue, comments))
        });
        self._open = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            _ = this.update(cx, |this, cx| {
                let Some(open) = this
                    .open
                    .as_mut()
                    .filter(|open| open.issue.number == number)
                else {
                    return;
                };
                open.loading = false;
                match result {
                    Ok((issue, comments)) => {
                        open.body = markdown(issue.body.as_deref().unwrap_or_default(), cx);
                        open.bodies = comments
                            .iter()
                            .map(|comment| (comment.id, markdown(&comment.body, cx)))
                            .collect();
                        open.issue = issue;
                        open.comments = comments;
                    }
                    Err(err) => this.error = Some(format!("{err:#}").into()),
                }
                cx.notify();
            });
        }));
    }

    /// Runs GitHub work on the open issue in the background, then reads it
    /// again.
    fn act(
        &mut self,
        label: &'static str,
        done: &'static str,
        work: impl FnOnce(&Hosting) -> anyhow::Result<()> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label);
        cx.notify();
        let hosting = self.hosting.clone();
        let task = cx.background_spawn(async move { work(&hosting) });
        self._act = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(()) => window.push_notification(Notification::success(done), cx),
                    Err(err) => window.push_notification(
                        Notification::error(format!("{err:#}")).title("GitHub"),
                        cx,
                    ),
                }
                if let Some(number) = this.open.as_ref().map(|open| open.issue.number) {
                    this.read_open(number, cx);
                }
                cx.notify();
            });
        }));
    }

    fn comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(number) = self.open.as_ref().map(|open| open.issue.number) else {
            return;
        };
        let body = self.composer.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.act(
            "Commenting",
            "Comment posted",
            move |h| ph_github::pulls::comment(&h.client, &h.repo, number, &body).map(|_| ()),
            window,
            cx,
        );
    }

    /// Assigns the open issue to the account, or takes it off.
    fn assign_me(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &self.open else {
            return;
        };
        let me = self.hosting.account.login.clone();
        let mut logins: Vec<String> = open
            .issue
            .assignees
            .iter()
            .map(|user| user.login.clone())
            .filter(|login| !login.eq_ignore_ascii_case(&me))
            .collect();
        if on {
            logins.push(me);
        }
        let number = open.issue.number;
        self.act(
            "Assigning",
            if on {
                "Assigned to you"
            } else {
                "No longer assigned to you"
            },
            move |h| issues::assign(&h.client, &h.repo, number, &logins).map(|_| ()),
            window,
            cx,
        );
    }

    /// The open issue as Markdown, with its comments.
    fn as_text(&self) -> Option<(Issue, String)> {
        let open = self.open.as_ref()?;
        Some((
            open.issue.clone(),
            issues::as_markdown(&open.issue, &open.comments),
        ))
    }

    fn fix(&mut self, cx: &mut Context<Self>) {
        if let Some((issue, text)) = self.as_text() {
            cx.emit(IssuesEvent::Agent(AgentRequest::FixIssue {
                issue: Box::new(issue),
                text,
            }));
        }
    }

    fn copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((issue, text)) = self.as_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            window.push_notification(
                Notification::success(format!(
                    "Copied issue #{} with its comments: paste it into a chat",
                    issue.number
                )),
                cx,
            );
        }
    }

    /// Writes a new issue in a dialog.
    fn new_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hosting = self.hosting.clone();
        let form = cx.new(|cx| IssueForm::new(&hosting, "", "", None, window, cx));
        let submitted = cx.subscribe_in(
            &form,
            window,
            |this, _, event: &IssueFormEvent, window, cx| {
                let IssueFormEvent::Submit {
                    title,
                    body,
                    assignees,
                } = event.clone();
                window.close_dialog(cx);
                this.form = None;
                let hosting = this.hosting.clone();
                let work = cx.background_spawn(async move {
                    issues::create(
                        &hosting.client,
                        &hosting.repo,
                        &title,
                        &body,
                        &issues::Extras {
                            labels: &[],
                            assignees: &assignees,
                        },
                    )
                });
                this._act = Some(cx.spawn_in(window, async move |this, cx| {
                    let result = work.await;
                    _ = this.update_in(cx, |this, window, cx| match result {
                        Ok(issue) => {
                            window.push_notification(
                                Notification::success(format!("Opened issue #{}", issue.number)),
                                cx,
                            );
                            this.reload(cx);
                            this.open_issue(issue, cx);
                        }
                        Err(err) => window.push_notification(
                            Notification::error(format!("{err:#}")).title("GitHub"),
                            cx,
                        ),
                    });
                }));
            },
        );
        IssueForm::open(&form, "New issue", window, cx);
        self.form = Some((form, submitted));
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let wanted = self.search.read(cx).value().trim().to_lowercase();
        let visible: Vec<&Issue> = self
            .issues
            .iter()
            .filter(|issue| {
                wanted.is_empty()
                    || issue.title.to_lowercase().contains(&wanted)
                    || format!("#{}", issue.number).contains(&wanted)
                    || issue
                        .labels
                        .iter()
                        .any(|label| label.name.to_lowercase().contains(&wanted))
            })
            .collect();
        let filter = |value: Filter, cx: &mut Context<Self>| {
            let selected = self.filter == value;
            div()
                .id(SharedString::from(format!(
                    "issues-filter-{}",
                    value.label()
                )))
                .role(gpui_kit::accesskit::Role::Tab)
                .aria_label(value.label())
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
                .on_click(cx.listener(move |this, _, _, cx| this.set_filter(value, cx)))
                .child(value.label())
                .test_support()
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_2()
                    .pt_2()
                    .gap_1()
                    .flex_wrap()
                    .children(Filter::ALL.map(|value| filter(value, cx)))
                    .child(div().flex_1())
                    .when(self.loading, |this| {
                        this.child(ph_ui::Loader::new().xsmall())
                    })
                    .child(
                        Button::new("issue-new")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Plus)
                            .label("New")
                            .tooltip("Open an issue in this repository")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.new_issue(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("issues-refresh")
                            .xsmall()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .accessibility_label("Refresh issues")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    ),
            )
            // A search once the list is longer than a glance takes in.
            .when(self.issues.len() > 8, |this| {
                this.child(
                    div().px_2().pt_2().child(
                        Input::new(&self.search)
                            .small()
                            .prefix(Icon::new(IconName::Search).xsmall()),
                    ),
                )
            })
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
                    .id("issues-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_2()
                    .gap_1()
                    .when(visible.is_empty() && !self.loading, |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(match (self.filter, wanted.is_empty()) {
                                    (_, false) => "No issue matches.",
                                    (Filter::Open, _) => "No open issues here.",
                                    (Filter::Mine, _) => "No open issue is assigned to you.",
                                    (Filter::Closed, _) => "No closed issues here.",
                                }),
                        )
                    })
                    .children(visible.into_iter().map(|issue| self.render_row(issue, cx)))
                    .when(
                        self.issues.len() >= issues::PER_LIST && wanted.is_empty(),
                        |this| {
                            this.child(
                                div()
                                    .p_2()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "The {} most recently updated. Older ones are on GitHub.",
                                        issues::PER_LIST
                                    )),
                            )
                        },
                    ),
            )
            .into_any_element()
    }

    fn render_row(&self, issue: &Issue, cx: &mut Context<Self>) -> AnyElement {
        let number = issue.number;
        let (icon, color) = state_icon(issue, cx);
        let opened = issue.clone();
        let assigned: Vec<&str> = issue
            .assignees
            .iter()
            .map(|user| user.login.as_str())
            .collect();
        v_flex()
            .id(("issue", number))
            .aria_label(SharedString::from(format!("#{number} {}", issue.title)))
            .flex_none()
            .gap_0p5()
            .px_2p5()
            .py_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().list_hover))
            .on_click(cx.listener(move |this, _, _, cx| this.open_issue(opened.clone(), cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(icon.small().text_color(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(issue.title.clone()),
                    ),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .flex_wrap()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "#{number} · {} · {}",
                        issue.user.login,
                        ph_ui::time::ago_of(&issue.updated_at)
                    ))
                    .when(issue.comments > 0, |this| {
                        this.child(match issue.comments {
                            1 => "1 comment".to_string(),
                            n => format!("{n} comments"),
                        })
                    })
                    .when(!assigned.is_empty(), |this| {
                        this.child(format!("→ {}", assigned.join(", ")))
                    })
                    .children(
                        issue
                            .labels
                            .iter()
                            .take(3)
                            .map(|label| label_chip(label, cx)),
                    ),
            )
            .test_support()
            .into_any_element()
    }

    fn render_open(&self, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let issue = &open.issue;
        let (_, color) = state_icon(issue, cx);
        let is_open = issue.state == "open";
        let url = issue.html_url.clone();
        let me = &self.hosting.account.login;
        let mine = issue
            .assignees
            .iter()
            .any(|user| user.login.eq_ignore_ascii_case(me));
        let assigned: Vec<&str> = issue
            .assignees
            .iter()
            .map(|user| user.login.as_str())
            .collect();
        let view = cx.weak_entity();
        let number = issue.number;
        let card = |who: String, when: &str, body: Option<AnyElement>, cx: &gpui_kit::App| {
            v_flex()
                .rounded(cx.theme().radius_lg)
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .gap_2()
                        .px_3()
                        .py_1p5()
                        .bg(cx.theme().secondary)
                        .text_xs()
                        .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child(who))
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(ph_ui::time::ago_of(when)),
                        ),
                )
                .child(div().px_3().py_2().text_sm().children(body))
        };
        let text = |view: &Entity<TextViewState>| {
            TextView::new(view)
                .image_source(ph_ui::images_safe::web_only)
                .into_any_element()
        };
        v_flex()
            .size_full()
            .child(
                v_flex()
                    .gap_1p5()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("issue-back")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ArrowLeft)
                                    .accessibility_label("Back to the issues")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(format!("{} #{}", issue.title, issue.number)),
                            )
                            .child(
                                Button::new("issue-open-web")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ExternalLink)
                                    .tooltip("Open on GitHub")
                                    .accessibility_label("Open on GitHub")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                div()
                                    .px_2()
                                    .rounded_full()
                                    .bg(color.opacity(0.15))
                                    .text_color(color)
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .child(if is_open { "Open" } else { "Closed" }),
                            )
                            .child(match assigned.as_slice() {
                                [] => "Assigned to no one".to_string(),
                                people => format!("Assigned to {}", people.join(", ")),
                            })
                            .children(issue.labels.iter().take(4).map(|label| label_chip(label, cx))),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .flex_wrap()
                            .when(is_open, |this| {
                                this.child(
                                    Button::new("issue-fix")
                                        .small()
                                        .primary()
                                        .icon(Icons::GitBranch)
                                        .label("Fix in a task")
                                        .tooltip(ph_ui::tip("A task on a branch of its own; its agent is given the issue and its comments"))
                                        .disabled(open.loading)
                                        .on_click(cx.listener(|this, _, _, cx| this.fix(cx))),
                                )
                            })
                            .child(
                                Button::new("issue-copy")
                                    .small()
                                    .outline()
                                    .icon(IconName::Copy)
                                    .label("Copy for an agent")
                                    .tooltip(ph_ui::tip("The issue and its comments as Markdown, to paste into a chat"))
                                    .disabled(open.loading)
                                    .on_click(cx.listener(|this, _, window, cx| this.copy(window, cx))),
                            )
                            .child(
                                Button::new("issue-more")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Ellipsis)
                                    .accessibility_label("More for this issue")
                                    .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                                        let (assign, refresh) = (view.clone(), view.clone());
                                        menu.item(
                                            PopupMenuItem::new(if mine {
                                                "Take it off me"
                                            } else {
                                                "Assign it to me"
                                            })
                                            .icon(Icons::User)
                                            .on_click(move |_, window, cx| {
                                                _ = assign.update(cx, |this, cx| {
                                                    this.assign_me(!mine, window, cx)
                                                });
                                            }),
                                        )
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("Refresh")
                                                .icon(IconName::RefreshCw)
                                                .on_click(move |_, _, cx| {
                                                    _ = refresh.update(cx, |this, cx| {
                                                        this.read_open(number, cx)
                                                    });
                                                }),
                                        )
                                    }),
                            )
                            .when_some(self.busy, |this, busy| {
                                this.child(
                                    h_flex()
                                        .gap_1()
                                        .text_xs()
                                        .child(ph_ui::Loader::new().xsmall())
                                        .child(busy),
                                )
                            }),
                    ),
            )
            .child(
                v_flex()
                    .id("issue-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .gap_3()
                    .p_3()
                    .child(card(
                        format!("{} opened this", issue.user.login),
                        &issue.created_at,
                        Some(if issue.body.as_deref().unwrap_or_default().trim().is_empty() {
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child("No description.")
                                .into_any_element()
                        } else {
                            text(&open.body)
                        }),
                        cx,
                    ))
                    .children(open.comments.iter().map(|comment| {
                        card(
                            comment.user.login.clone(),
                            &comment.created_at,
                            open.bodies.get(&comment.id).map(text),
                            cx,
                        )
                    }))
                    .when(open.loading, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(ph_ui::Loader::new().xsmall())
                                .child("Reading the comments…"),
                        )
                    })
                    .child(
                        v_flex()
                            .gap_2()
                            .child(Textarea::new(&self.composer).small())
                            .child(
                                h_flex().justify_end().child(
                                    Button::new("issue-comment")
                                        .small()
                                        .outline()
                                        .label("Comment")
                                        .disabled(self.busy.is_some())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.comment(window, cx)
                                        })),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for IssuesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.open {
            Some(open) => self.render_open(open, cx),
            None => self.render_list(cx),
        }
    }
}

/// An issue's mark and its colour: open, or closed.
fn state_icon(issue: &Issue, cx: &gpui_kit::App) -> (Icon, gpui_kit::Hsla) {
    if issue.state == "open" {
        (Icon::new(Icons::CircleDot), cx.theme().success)
    } else {
        (
            Icon::new(IconName::CircleCheck),
            cx.theme().muted_foreground,
        )
    }
}

fn label_chip(label: &ph_github::models::Label, cx: &gpui_kit::App) -> gpui_kit::Div {
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
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::TestAppContext;
    use ph_github::testing::MockServer;
    use ph_github::{Account, Client, Repo};

    use super::*;

    fn hosting(base: &str) -> Hosting {
        Hosting {
            account: Account {
                id: "test".into(),
                host: "github.com".into(),
                login: "me".into(),
                api_url: None,
            },
            repo: Repo {
                owner: "acme".into(),
                name: "app".into(),
            },
            remote: "origin".into(),
            client: Arc::new(Client::new(base, "t")),
        }
    }

    /// The list leaves out pull requests (GitHub lists them as issues), an
    /// issue opens with its comments, and fixing it hands an agent the
    /// issue and what was said about it.
    #[gpui_kit::test]
    fn lists_issues_and_hands_one_to_an_agent(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            ph_theme::init(cx);
        });
        let server = MockServer::start(vec![
            (
                "GET",
                "/repos/acme/app/issues/7/comments",
                200,
                r#"[{"id":1,"user":{"login":"chen"},"body":"I can reproduce it."}]"#,
            ),
            (
                "GET",
                "/repos/acme/app/issues/7",
                200,
                r#"{"number":7,"title":"Crash on an empty cart","state":"open","user":{"login":"maria"},"body":"Open the cart.","comments":1}"#,
            ),
            (
                "GET",
                "/repos/acme/app/issues?state=open",
                200,
                r#"[{"number":7,"title":"Crash on an empty cart","state":"open","user":{"login":"maria"},"comments":1},
                    {"number":8,"title":"A pull request","state":"open","pull_request":{"url":"x"}}]"#,
            ),
        ]);
        let hosting = hosting(&server.base);
        let (view, cx) = cx.add_window_view(|window, cx| IssuesView::new(hosting, window, cx));
        let asked = std::rc::Rc::new(std::cell::RefCell::new(None));
        let heard = asked.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &IssuesEvent, _| {
                *heard.borrow_mut() = Some(event.clone());
            })
            .detach();
        });
        view.update(cx, |view, cx| view.shown(cx));
        cx.run_until_parked();
        let first = view.read_with(cx, |view, _| {
            assert_eq!(view.issues.len(), 1, "the pull request is left out");
            view.issues[0].clone()
        });
        view.update(cx, |view, cx| view.open_issue(first, cx));
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            let open = view.open.as_ref().unwrap();
            assert!(!open.loading);
            assert_eq!(open.comments.len(), 1);
            view.fix(cx);
        });
        let asked = asked.borrow().clone();
        let Some(IssuesEvent::Agent(AgentRequest::FixIssue { issue, text })) = asked else {
            panic!("fixing an issue asks for a task");
        };
        assert_eq!(issue.number, 7);
        assert!(text.contains("Open the cart.") && text.contains("I can reproduce it."));
    }
}
