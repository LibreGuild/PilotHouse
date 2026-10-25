//! One pull request, all in the app: description and conversation, changed
//! files with their line comments, commits, checks; comment, approve,
//! request changes, merge, check out, or hand it to an agent for review.

use std::collections::HashMap;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Textarea, TextareaState},
    notification::Notification,
    spinner::Spinner,
    text::{TextView, TextViewState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Hsla, InteractiveElement as _,
    IntoElement, ListAlignment, ListState, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window, div, list,
};
use ph_changes::Repo as LocalRepo;
use ph_github::models::{
    Check, Commit, IssueComment, PullFile, PullRequest, Review, ReviewComment,
};
use ph_github::pulls::{self, MergeMethod, ReviewEvent};
use ph_ui::Icons;

use crate::hosting::{self, Hosting};

#[derive(Debug, Clone)]
pub enum PullViewEvent {
    Back,
    /// Start an agent review of this pull request.
    ReviewWithAgent(Box<PullRequest>),
    /// Show the chat of an earlier review.
    OpenSession(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Files,
    Commits,
    Checks,
}

/// A file's diff is folded when longer than this (like GitHub's large
/// diffs); one click shows it.
const FOLD_LINES: usize = 400;

/// Everything about the pull request, loaded together.
#[derive(Default, Clone)]
struct Loaded {
    pull: PullRequest,
    files: Vec<PullFile>,
    /// Each file's patch, parsed once (in the background).
    changes: Vec<Option<ph_git::FileChange>>,
    commits: Vec<Commit>,
    conversation: Vec<IssueComment>,
    line_comments: Vec<ReviewComment>,
    reviews: Vec<Review>,
    checks: Vec<Check>,
}

pub struct PullView {
    store: ph_store::Store,
    /// The newest finished agent review: (session, open, fixed) counts.
    last_review: Option<(Option<i64>, usize, usize, String)>,
    hosting: Hosting,
    /// The workspace checkout to check the PR out in (none in the GitHub
    /// view, which reviews in its own clones).
    local: Option<Entity<LocalRepo>>,
    number: u64,
    data: Option<std::rc::Rc<Loaded>>,
    /// The Files tab draws only the files in sight.
    files_list: ListState,
    /// Large files whose diff was asked for.
    unfolded: std::collections::HashSet<usize>,
    error: Option<SharedString>,
    tab: Tab,
    /// Rendered markdown: the description, then each comment by id.
    description: Option<Entity<TextViewState>>,
    bodies: HashMap<u64, Entity<TextViewState>>,
    composer: Entity<TextareaState>,
    /// The line comment thread being replied to, and the reply.
    replying: Option<u64>,
    reply_box: Entity<TextareaState>,
    busy: Option<&'static str>,
    confirm_merge: bool,
    merge_method: MergeMethod,
    _load: Option<Task<()>>,
    _act: Option<Task<()>>,
}

impl EventEmitter<PullViewEvent> for PullView {}

impl PullView {
    pub fn new(
        store: ph_store::Store,
        hosting: Hosting,
        local: Option<Entity<LocalRepo>>,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Leave a comment (Markdown)")
                .rows(3)
        });
        let reply_box = cx.new(|cx| TextareaState::new(window, cx).placeholder("Reply").rows(2));
        let mut view = Self {
            store,
            last_review: None,
            hosting,
            local,
            replying: None,
            reply_box,
            number,
            data: None,
            files_list: ListState::new(0, ListAlignment::Top, gpui_kit::px(800.)),
            unfolded: std::collections::HashSet::new(),
            error: None,
            tab: Tab::Overview,
            description: None,
            bodies: HashMap::new(),
            composer,
            busy: None,
            confirm_merge: false,
            merge_method: MergeMethod::Squash,
            _load: None,
            _act: None,
        };
        view.reload(cx);
        view
    }

    /// An agent review of this pull request ended: its line in the header
    /// says what it found.
    pub fn review_changed(&mut self, cx: &mut Context<Self>) {
        self.read_last_review();
        cx.notify();
    }

    pub fn number(&self) -> u64 {
        self.number
    }

    /// What the newest agent review of this PR found.
    fn read_last_review(&mut self) {
        let pull = ph_store::reviews::PullRef {
            host: self.hosting.account.host.clone(),
            owner: self.hosting.repo.owner.clone(),
            repo: self.hosting.repo.name.clone(),
            number: self.number,
        };
        self.last_review = self
            .store
            .latest_pr_review(&pull, None)
            .ok()
            .flatten()
            .map(|review| {
                let findings = self.store.pr_findings(review.id).unwrap_or_default();
                let open = findings
                    .iter()
                    .filter(|f| matches!(f.status.as_str(), "new" | "open"))
                    .count();
                let fixed = findings.iter().filter(|f| f.status == "fixed").count();
                let at = review.head_sha[..review.head_sha.len().min(8)].to_string();
                (review.session_id, open, fixed, at)
            });
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.read_last_review();
        let (client, repo, number) = (
            self.hosting.client.clone(),
            self.hosting.repo.clone(),
            self.number,
        );
        let load = cx.background_spawn(async move {
            let pull = pulls::get(&client, &repo, number)?;
            let files = pulls::files(&client, &repo, number)?;
            let commits = pulls::commits(&client, &repo, number)?;
            let conversation = pulls::conversation(&client, &repo, number)?;
            let line_comments = pulls::line_comments(&client, &repo, number)?;
            let reviews = pulls::reviews(&client, &repo, number)?;
            // Checks are nice to have; a token without that scope still works.
            let checks = pulls::checks(&client, &repo, &pull.head.sha).unwrap_or_default();
            let changes = files.iter().map(file_change).collect();
            anyhow::Ok(Loaded {
                pull,
                changes,
                files,
                commits,
                conversation,
                line_comments,
                reviews,
                checks,
            })
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok(data) => {
                        let text = data.pull.body.clone().unwrap_or_default();
                        this.description = Some(cx.new(|cx| TextViewState::markdown(&text, cx)));
                        let mut bodies = HashMap::new();
                        for (id, body) in data
                            .conversation
                            .iter()
                            .map(|c| (c.id, c.body.clone()))
                            .chain(data.line_comments.iter().map(|c| (c.id, c.body.clone())))
                            .chain(
                                data.reviews
                                    .iter()
                                    .map(|r| (r.id, r.body.clone().unwrap_or_default())),
                            )
                        {
                            bodies.insert(id, cx.new(|cx| TextViewState::markdown(&body, cx)));
                        }
                        this.bodies = bodies;
                        // The same files keep their place (a reply was posted).
                        if this.files_list.item_count() == data.files.len() {
                            this.files_list.remeasure();
                        } else {
                            this.files_list.reset(data.files.len());
                        }
                        this.data = Some(std::rc::Rc::new(data));
                        this.error = None;
                    }
                    Err(err) => this.error = Some(format!("{err:#}").into()),
                }
                cx.notify();
            });
        }));
    }

    /// Runs a GitHub action in the background, then reloads.
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
                    Ok(()) => {
                        window.push_notification(Notification::success(done), cx);
                        this.composer
                            .update(cx, |input, cx| input.set_value("", window, cx));
                    }
                    Err(err) => window.push_notification(
                        Notification::error(format!("{err:#}")).title("GitHub"),
                        cx,
                    ),
                }
                this.reload(cx);
            });
        }));
    }

    fn comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.composer.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        let number = self.number;
        self.act(
            "Commenting",
            "Comment posted",
            move |h| pulls::comment(&h.client, &h.repo, number, &body).map(|_| ()),
            window,
            cx,
        );
    }

    fn reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.replying else {
            return;
        };
        let body = self.reply_box.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        self.replying = None;
        self.reply_box
            .update(cx, |input, cx| input.set_value("", window, cx));
        let number = self.number;
        self.act(
            "Replying",
            "Reply posted",
            move |h| pulls::reply(&h.client, &h.repo, number, thread, &body).map(|_| ()),
            window,
            cx,
        );
    }

    fn review(&mut self, event: ReviewEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(head) = self.data.as_ref().map(|data| data.pull.head.sha.clone()) else {
            return;
        };
        let body = self.composer.read(cx).value().trim().to_string();
        let number = self.number;
        let (label, done) = match event {
            ReviewEvent::Approve => ("Approving", "Approved"),
            ReviewEvent::RequestChanges => ("Requesting changes", "Changes requested"),
            ReviewEvent::Comment => ("Reviewing", "Review posted"),
        };
        self.act(
            label,
            done,
            move |h| {
                pulls::submit_review(&h.client, &h.repo, number, &head, event, &body, &[])
                    .map(|_| ())
            },
            window,
            cx,
        );
    }

    fn merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirm_merge {
            self.confirm_merge = true;
            cx.notify();
            return;
        }
        self.confirm_merge = false;
        let (number, method) = (self.number, self.merge_method);
        self.act(
            "Merging",
            "Merged",
            move |h| pulls::merge(&h.client, &h.repo, number, method).map(|_| ()),
            window,
            cx,
        );
    }

    /// Fetches the head and switches to a local `pr/<number>` branch.
    fn check_out(&mut self, cx: &mut Context<Self>) {
        let (hosting, number) = (self.hosting.clone(), self.number);
        let Some(local) = self.local.clone() else {
            return;
        };
        local.update(cx, |repo, cx| {
            repo.run(
                format!("Checking out #{number}"),
                move |root| {
                    let sha = hosting::fetch_head(root, &hosting, number)?;
                    let branch = format!("pr/{number}");
                    let exists = ph_git::git(
                        root,
                        &[
                            "rev-parse",
                            "--verify",
                            "--quiet",
                            &format!("refs/heads/{branch}"),
                        ],
                    )
                    .is_ok();
                    if exists {
                        ph_git::branches::switch(root, &branch, false)?;
                        ph_git::git(root, &["merge", "--ff-only", "--quiet", &sha])?;
                    } else {
                        ph_git::branches::create(root, &branch, Some(&sha))?;
                    }
                    Ok(format!("Checked out #{number} as {branch}"))
                },
                cx,
            )
        });
    }

    fn render_header(&self, data: &Loaded, cx: &mut Context<Self>) -> impl IntoElement {
        let pull = &data.pull;
        let (state, color) = state_badge(pull, cx);
        let url = pull.html_url.clone();
        let review_pull = pull.clone();
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
                        Button::new("pull-back")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ArrowLeft)
                            .accessibility_label("Back to pull requests")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PullViewEvent::Back))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .truncate()
                            .child(format!("{} #{}", pull.title, pull.number)),
                    )
                    .child(
                        Button::new("pull-open-web")
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
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(pill(state, color))
                    .child(format!(
                        "{} {} {} into {}",
                        pull.user.login,
                        if pull.merged {
                            "merged"
                        } else if pull.state == "closed" {
                            "proposed (closed)"
                        } else {
                            "wants to merge"
                        },
                        pull.head.name,
                        pull.base.name
                    ))
                    .child(format!("+{} −{}", pull.additions, pull.deletions)),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        Button::new("pull-review-agent")
                            .small()
                            .primary()
                            .icon(Icons::Sparkles)
                            .label("Review with agent")
                            .tooltip(
                                "Opens a review chat in the GitHub view, on its own copy of the PR",
                            )
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(PullViewEvent::ReviewWithAgent(Box::new(
                                    review_pull.clone(),
                                )))
                            })),
                    )
                    .when(self.local.is_some(), |this| {
                        this.child(
                            Button::new("pull-check-out")
                                .small()
                                .outline()
                                .icon(Icons::GitBranch)
                                .label("Check out")
                                .on_click(cx.listener(|this, _, _, cx| this.check_out(cx))),
                        )
                    })
                    .child(
                        Button::new("pull-refresh")
                            .small()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .accessibility_label("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
                    .when_some(self.busy, |this, busy| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .text_xs()
                                .child(Spinner::new().xsmall())
                                .child(busy),
                        )
                    }),
            )
            .when_some(
                self.last_review.clone(),
                |this, (session, open, fixed, at)| {
                    this.child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(Icons::Sparkles).xsmall())
                            .child(format!("Agent review at {at}: {open} open, {fixed} fixed"))
                            .when_some(session, |this, session| {
                                this.child(
                                    Button::new("pull-open-review")
                                        .xsmall()
                                        .ghost()
                                        .label("Open review chat")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(PullViewEvent::OpenSession(session))
                                        })),
                                )
                            }),
                    )
                },
            )
            .child(self.render_tabs(data, cx))
    }

    fn render_tabs(&self, data: &Loaded, cx: &mut Context<Self>) -> impl IntoElement {
        let failing = data
            .checks
            .iter()
            .filter(|c| c.outcome == "failure")
            .count();
        let tabs = [
            (Tab::Overview, "Overview".to_string()),
            (Tab::Files, format!("Files {}", data.files.len())),
            (Tab::Commits, format!("Commits {}", data.commits.len())),
            (
                Tab::Checks,
                if failing > 0 {
                    format!("Checks · {failing} failing")
                } else {
                    format!("Checks {}", data.checks.len())
                },
            ),
        ];
        h_flex()
            .gap_1()
            .pt_1()
            .children(tabs.into_iter().map(|(tab, label)| {
                let selected = self.tab == tab;
                let name = match tab {
                    Tab::Overview => "Overview",
                    Tab::Files => "Files",
                    Tab::Commits => "Commits",
                    Tab::Checks => "Checks",
                };
                div()
                    .id(SharedString::from(format!(
                        "pull-tab-{}",
                        name.to_lowercase()
                    )))
                    .role(gpui_kit::accesskit::Role::Tab)
                    .aria_label(name)
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
                        this.tab = tab;
                        cx.notify();
                    }))
                    .child(label)
                    .test_support()
            }))
    }

    fn render_overview(&self, data: &Loaded, cx: &mut Context<Self>) -> AnyElement {
        // The conversation in order: comments and reviews by time.
        enum Item<'a> {
            Comment(&'a IssueComment),
            Review(&'a Review),
        }
        let mut items: Vec<(String, Item)> = data
            .conversation
            .iter()
            .map(|c| (c.created_at.clone(), Item::Comment(c)))
            .chain(
                data.reviews
                    .iter()
                    // A reply on a line thread is a review with no text of its
                    // own: it shows in its thread under Files, not here.
                    .filter(|r| {
                        r.state != "PENDING"
                            && !(r.state == "COMMENTED"
                                && r.body.as_deref().unwrap_or_default().trim().is_empty())
                    })
                    .map(|r| (r.submitted_at.clone().unwrap_or_default(), Item::Review(r))),
            )
            .collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));
        let open = data.pull.state == "open" && !data.pull.merged;
        v_flex()
            .gap_3()
            .p_3()
            .child(card(
                format!("{} opened this", data.pull.user.login),
                &data.pull.created_at,
                self.description
                    .clone()
                    .map(|view| TextView::new(&view).into_any_element()),
                cx,
            ))
            .children(items.into_iter().map(|(_, item)| {
                match item {
                    Item::Comment(comment) => card(
                        comment.user.login.clone(),
                        &comment.created_at,
                        self.bodies
                            .get(&comment.id)
                            .map(|view| TextView::new(view).into_any_element()),
                        cx,
                    ),
                    Item::Review(review) => {
                        let (verb, color) = review_state(&review.state, cx);
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .child(pill(verb, color))
                                    .child(review.user.login.clone())
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(short_date(
                                                review.submitted_at.as_deref().unwrap_or_default(),
                                            )),
                                    ),
                            )
                            .when(
                                review
                                    .body
                                    .as_deref()
                                    .is_some_and(|body| !body.trim().is_empty()),
                                |this| {
                                    this.children(self.bodies.get(&review.id).map(|view| {
                                        div()
                                            .pl_3()
                                            .border_l_2()
                                            .border_color(color.opacity(0.5))
                                            .child(TextView::new(view))
                                    }))
                                },
                            )
                            .into_any_element()
                    }
                }
            }))
            .when(open, |this| this.child(self.render_actions(data, cx)))
            .into_any_element()
    }

    fn render_actions(&self, data: &Loaded, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.is_some();
        let mergeable = data.pull.mergeable != Some(false) && !data.pull.draft;
        let method_label = match self.merge_method {
            MergeMethod::Merge => "Merge commit",
            MergeMethod::Squash => "Squash and merge",
            MergeMethod::Rebase => "Rebase and merge",
        };
        v_flex()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .child(Textarea::new(&self.composer).small())
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        Button::new("pull-comment")
                            .small()
                            .outline()
                            .label("Comment")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.comment(window, cx))),
                    )
                    .child(
                        Button::new("pull-request-changes")
                            .small()
                            .outline()
                            .label("Request changes")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.review(ReviewEvent::RequestChanges, window, cx)
                            })),
                    )
                    .child(
                        Button::new("pull-approve")
                            .small()
                            .primary()
                            .icon(IconName::Check)
                            .label("Approve")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.review(ReviewEvent::Approve, window, cx)
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(match data.pull.mergeable {
                                _ if data.pull.draft => {
                                    "Draft: mark it ready on GitHub before merging."
                                }
                                Some(false) => "This branch has conflicts with the base.",
                                None => "GitHub is checking whether it can merge.",
                                Some(true) => "Ready to merge.",
                            }),
                    )
                    .child(
                        Button::new("pull-merge-method")
                            .small()
                            .ghost()
                            .label(method_label)
                            .tooltip("Change how it merges")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.merge_method = match this.merge_method {
                                    MergeMethod::Squash => MergeMethod::Merge,
                                    MergeMethod::Merge => MergeMethod::Rebase,
                                    MergeMethod::Rebase => MergeMethod::Squash,
                                };
                                this.confirm_merge = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("pull-merge")
                            .small()
                            .when(self.confirm_merge, |this| this.danger())
                            .when(!self.confirm_merge, |this| this.outline())
                            .label(if self.confirm_merge {
                                "Confirm merge"
                            } else {
                                "Merge"
                            })
                            .disabled(busy || !mergeable)
                            .on_click(cx.listener(|this, _, window, cx| this.merge(window, cx))),
                    ),
            )
            .into_any_element()
    }

    /// The Files tab: each file's diff and its line comment threads, drawn
    /// only while in sight (a pull request can change thousands of lines).
    fn render_files(&self, cx: &mut Context<Self>) -> AnyElement {
        list(
            self.files_list.clone(),
            cx.processor(|this, ix: usize, _, cx| this.render_file(ix, cx)),
        )
        .size_full()
        .into_any_element()
    }

    fn render_file(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(data) = self.data.clone() else {
            return div().into_any_element();
        };
        let (Some(file), Some(change)) = (data.files.get(ix), data.changes.get(ix)) else {
            return div().into_any_element();
        };
        // Line comment threads of this file, each root with its replies.
        let threads: Vec<(&ReviewComment, Vec<&ReviewComment>)> = data
            .line_comments
            .iter()
            .filter(|c| c.in_reply_to_id.is_none() && c.path == file.filename)
            .map(|root| {
                let replies = data
                    .line_comments
                    .iter()
                    .filter(|c| c.in_reply_to_id == Some(root.id))
                    .collect();
                (root, replies)
            })
            .collect();
        let lines: usize = change.as_ref().map_or(0, |change| {
            change.hunks.iter().map(|hunk| hunk.lines.len()).sum()
        });
        let folded = lines > FOLD_LINES && !self.unfolded.contains(&ix);
        let diff = match change {
            Some(change) if !folded => ph_changes::file_diff(change, cx),
            _ => h_flex()
                .w_full()
                .px_2()
                .py_1()
                .gap_2()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .text_sm()
                .child(div().flex_1().min_w_0().truncate().child(format!(
                    "{} · +{} −{}",
                    file.filename, file.additions, file.deletions
                )))
                .map(|this| {
                    if change.is_some() {
                        this.child(
                            Button::new(("show-diff", ix))
                                .xsmall()
                                .ghost()
                                .label(format!("Show diff ({lines} lines)"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.unfolded.insert(ix);
                                    this.files_list.remeasure_items(ix..ix + 1);
                                    cx.notify();
                                })),
                        )
                    } else {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} (no diff shown)", file.status)),
                        )
                    }
                })
                .into_any_element(),
        };
        // A list item sizes to its content: fill the column.
        v_flex()
            .w_full()
            .px_3()
            .pt_2()
            .when(ix + 1 == data.files.len(), |this| this.pb_3())
            .gap_1()
            .child(div().w_full().child(diff))
            .children(threads.into_iter().map(|(root, replies)| {
                v_flex()
                    .ml_3()
                    .gap_1()
                    .p_2()
                    .rounded(cx.theme().radius)
                    .border_l_2()
                    .border_color(cx.theme().info.opacity(0.5))
                    .bg(cx.theme().secondary)
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "Line {} · {}",
                                root.line
                                    .or(root.original_line)
                                    .map(|l| l.to_string())
                                    .unwrap_or("outdated".into()),
                                root.user.login
                            )),
                    )
                    .children(self.bodies.get(&root.id).map(TextView::new))
                    .children(replies.iter().map(|reply| {
                        v_flex()
                            .pt_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(reply.user.login.clone()),
                            )
                            .children(self.bodies.get(&reply.id).map(TextView::new))
                    }))
                    .map(|this| {
                        let thread = root.id;
                        if self.replying == Some(thread) {
                            this.child(
                                v_flex()
                                    .gap_1()
                                    .pt_1()
                                    .child(Textarea::new(&self.reply_box).small())
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .justify_end()
                                            .child(
                                                Button::new(("cancel-reply", thread))
                                                    .xsmall()
                                                    .ghost()
                                                    .label("Cancel")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.replying = None;
                                                        this.files_list.remeasure();
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                Button::new(("send-reply", thread))
                                                    .xsmall()
                                                    .primary()
                                                    .label("Reply")
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.reply(window, cx)
                                                        },
                                                    )),
                                            ),
                                    ),
                            )
                        } else {
                            this.child(
                                Button::new(("start-reply", thread))
                                    .xsmall()
                                    .ghost()
                                    .label("Reply")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.replying = Some(thread);
                                        this.files_list.remeasure();
                                        cx.notify();
                                    })),
                            )
                        }
                    })
            }))
            .into_any_element()
    }

    fn render_commits(&self, data: &Loaded, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .p_3()
            .gap_1()
            .children(data.commits.iter().map(|commit| {
                let subject = commit
                    .commit
                    .message
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                v_flex()
                    .px_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(div().text_sm().truncate().child(subject))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} · {} · {}",
                                &commit.sha[..commit.sha.len().min(8)],
                                commit.commit.author.name,
                                short_date(&commit.commit.author.date)
                            )),
                    )
            }))
            .into_any_element()
    }

    fn render_checks(&self, data: &Loaded, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .p_3()
            .gap_1()
            .when(data.checks.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No checks reported for the head commit."),
                )
            })
            .children(data.checks.iter().enumerate().map(|(ix, check)| {
                let (icon, color) = check_icon(&check.outcome, cx);
                let url = check.url.clone();
                h_flex()
                    .id(("check", ix))
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .when(!url.is_empty(), |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(cx.theme().list_hover))
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    })
                    .child(Icon::new(icon).small().text_color(color))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .truncate()
                            .child(check.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(color)
                            .child(check.outcome.clone()),
                    )
            }))
            .into_any_element()
    }
}

impl Render for PullView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(data) = self.data.clone() else {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .map(|this| match &self.error {
                    Some(error) => this.child(error.clone()).child(
                        Button::new("pull-retry")
                            .small()
                            .label("Try again")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    ),
                    None => this
                        .child(Spinner::new())
                        .child(format!("Loading #{}…", self.number)),
                })
                .into_any_element();
        };
        let body = match self.tab {
            // A list that scrolls itself, absolutely filled so it gets an
            // exact height (a percentage inside a flex item resolves to the
            // content's, and then nothing scrolls).
            Tab::Files => div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(div().absolute().inset_0().child(self.render_files(cx)))
                .into_any_element(),
            tab => div()
                .id("pull-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(match tab {
                    Tab::Overview => self.render_overview(&data, cx),
                    Tab::Commits => self.render_commits(&data, cx),
                    _ => self.render_checks(&data, cx),
                })
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .child(self.render_header(&data, cx))
            .child(body)
            .into_any_element()
    }
}

/// A PR file's patch as a [`ph_git::FileChange`] for the diff view.
fn file_change(file: &PullFile) -> Option<ph_git::FileChange> {
    let patch = file.patch.as_ref()?;
    let name = &file.filename;
    let text = format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n{patch}\n");
    ph_git::parse_diff(&text).into_iter().next()
}

fn card(author: String, when: &str, body: Option<AnyElement>, cx: &App) -> AnyElement {
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
                .child(
                    div()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .child(author),
                )
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child(short_date(when)),
                ),
        )
        .child(div().px_3().py_2().text_sm().children(body))
        .into_any_element()
}

fn pill(text: &'static str, color: Hsla) -> AnyElement {
    div()
        .px_2()
        .rounded_full()
        .bg(color.opacity(0.15))
        .text_color(color)
        .text_xs()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .child(text)
        .into_any_element()
}

pub fn state_badge(pull: &PullRequest, cx: &App) -> (&'static str, Hsla) {
    if pull.merged {
        ("Merged", gpui_kit::rgb(0xa074c4).into())
    } else if pull.state == "closed" {
        ("Closed", cx.theme().danger)
    } else if pull.draft {
        ("Draft", cx.theme().muted_foreground)
    } else {
        ("Open", cx.theme().success)
    }
}

fn review_state(state: &str, cx: &App) -> (&'static str, Hsla) {
    match state {
        "APPROVED" => ("Approved", cx.theme().success),
        "CHANGES_REQUESTED" => ("Changes requested", cx.theme().danger),
        "DISMISSED" => ("Dismissed", cx.theme().muted_foreground),
        _ => ("Reviewed", cx.theme().info),
    }
}

pub fn check_icon(outcome: &str, cx: &App) -> (IconName, Hsla) {
    match outcome {
        "success" => (IconName::CircleCheck, cx.theme().success),
        "failure" | "timed_out" | "action_required" => (IconName::CircleX, cx.theme().danger),
        "pending" | "queued" | "in_progress" => (IconName::Loader, cx.theme().warning),
        _ => (IconName::Minus, cx.theme().muted_foreground),
    }
}

/// `2026-10-24T08:12:00Z` → `2026-10-24 08:12`.
pub fn short_date(stamp: &str) -> String {
    stamp
        .get(..16)
        .map(|s| s.replace('T', " "))
        .unwrap_or_else(|| stamp.to_string())
}
