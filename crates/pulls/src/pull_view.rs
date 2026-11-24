//! One pull request, all in the app. Its page reads in the order a review
//! goes: what an agent found, a briefing on the change, the files (with
//! what is said on their lines, [`files`]), then the conversation, with
//! what stands before the merge under it ([`status`]).
//!
//! A review leaves the computer in one step, **Finish review**
//! ([`crate::finish_view`]): the findings picked, the reviewer's own
//! comments, a note, and how it ends.

use std::collections::{HashMap, HashSet};

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Textarea, TextareaState},
    notification::Notification,
    scroll::ScrollableElement as _,
    text::{TextView, TextViewState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Hsla, InteractiveElement as _,
    IntoElement, ListAlignment, ListState, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
};
use ph_changes::Repo as LocalRepo;
use ph_github::models::{
    Check, Commit, IssueComment, PullFile, PullRequest, Review, ReviewComment,
};
use ph_github::pulls::{self, MergeMethod};
use ph_ui::{Icons, MenuButton as _};

use crate::findings_view::{FindingsEvent, FindingsView};
use crate::finish_view::{Draft, FinishEvent, FinishView};
use crate::hosting::{self, Hosting};
use crate::walkthrough_view::{WalkthroughEvent, WalkthroughView};

mod files;
mod status;

#[derive(Debug, Clone)]
pub enum PullViewEvent {
    Back,
    /// Work for an agent about this pull request.
    Agent(crate::AgentRequest),
    /// Show the chat of an earlier review.
    OpenSession(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Findings,
    Walkthrough,
    Files,
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
    /// The agent a review asked for here runs with (the one chats start
    /// with): named on the button, so it is known before it is pressed.
    agent: &'static ph_agent::agents::AgentSpec,
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
    /// The user asked to hear about new commits and answers.
    watched: bool,
    reply_box: Entity<TextareaState>,
    /// The reviewer's own comments on lines, not sent yet.
    drafts: Vec<Draft>,
    /// The lines a comment is being written on, and its box.
    composing: Option<files::Composing>,
    comment_box: Entity<TextareaState>,
    /// The files ticked as read (path at content).
    viewed: HashSet<String>,
    /// Only what changed since the reviewer's last review is shown.
    since: files::SinceState,
    /// The diff is drawn side by side (else in one column).
    split: bool,
    /// Lines are being picked: the button is still down.
    picking: bool,
    /// The commits are listed (they fold to one line).
    commits_open: bool,
    /// The conversation's scroll, so the checks under it can be gone to.
    conversation_scroll: gpui_kit::ScrollHandle,
    focus_handle: gpui_kit::FocusHandle,
    /// The file the keyboard is on in the Files tab.
    file_cursor: usize,
    /// Failed checks opened, and what each said.
    checks_open: HashSet<String>,
    check_details: HashMap<u64, status::CheckState>,
    /// The sheet a review is finished in, while open.
    finish: Option<(Entity<FinishView>, Subscription)>,
    busy: Option<&'static str>,
    confirm_merge: bool,
    merge_method: MergeMethod,
    findings: Entity<FindingsView>,
    walkthrough: Entity<WalkthroughView>,
    _load: Option<Task<()>>,
    _act: Option<Task<()>>,
    _since: Option<Task<()>>,
    _checks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
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
                .auto_grow(3, 12)
        });
        let reply_box = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Reply")
                .auto_grow(2, 8)
        });
        let comment_box = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Comment on these lines, or ask about them (Markdown)")
                .auto_grow(2, 12)
        });
        let findings =
            cx.new(|cx| FindingsView::new(store.clone(), hosting.clone(), number, window, cx));
        let walkthrough =
            cx.new(|cx| WalkthroughView::new(store.clone(), hosting.clone(), number, cx));
        let subscriptions = vec![
            cx.subscribe(
                &findings,
                |this, _, event: &FindingsEvent, cx| match event {
                    FindingsEvent::Agent(request) => cx.emit(PullViewEvent::Agent(request.clone())),
                    FindingsEvent::ShowLine { path, line } => this.show_line(path, *line, cx),
                    FindingsEvent::Changed => {
                        this.read_last_review();
                        cx.notify();
                    }
                    FindingsEvent::OpenSession(session) => {
                        cx.emit(PullViewEvent::OpenSession(*session))
                    }
                },
            ),
            cx.subscribe(
                &walkthrough,
                |this, _, event: &WalkthroughEvent, cx| match event {
                    WalkthroughEvent::Agent(request) => {
                        cx.emit(PullViewEvent::Agent(request.clone()))
                    }
                    WalkthroughEvent::ShowLine { path, line } => this.show_line(path, *line, cx),
                },
            ),
        ];
        // ⌘↵ (Ctrl+Enter) in a box does what its button does.
        let on_enter =
            |cx: &mut Context<Self>,
             input: &Entity<TextareaState>,
             act: fn(&mut Self, &mut Window, &mut Context<Self>)| {
                cx.subscribe_in(
                    input,
                    window,
                    move |this, _, event: &gpui_kit::component::input::InputEvent, window, cx| {
                        if matches!(
                            event,
                            gpui_kit::component::input::InputEvent::PressEnter {
                                secondary: true,
                                ..
                            }
                        ) {
                            act(this, window, cx)
                        }
                    },
                )
            };
        let mut subscriptions = subscriptions;
        subscriptions.push(on_enter(cx, &composer, Self::comment));
        subscriptions.push(on_enter(cx, &reply_box, Self::reply));
        subscriptions.push(on_enter(cx, &comment_box, Self::add_draft));
        let agent = ph_agent::chats::default_agent(&store);
        let mut view = Self {
            store,
            last_review: None,
            agent,
            hosting,
            local,
            replying: None,
            reply_box,
            number,
            data: None,
            files_list: ListState::new(0, ListAlignment::Top, gpui_kit::px(800.)),
            unfolded: std::collections::HashSet::new(),
            error: None,
            // The page opens as the host's does, on the conversation;
            // whoever comes for the findings (a row of the queue that has
            // some) is taken to them (`show_findings`).
            tab: Tab::Overview,
            description: None,
            bodies: HashMap::new(),
            composer,
            busy: None,
            confirm_merge: false,
            merge_method: MergeMethod::Squash,
            watched: false,
            findings,
            walkthrough,
            drafts: Vec::new(),
            composing: None,
            comment_box,
            viewed: HashSet::new(),
            since: files::SinceState::Off,
            split: false,
            picking: false,
            commits_open: false,
            conversation_scroll: gpui_kit::ScrollHandle::default(),
            focus_handle: cx.focus_handle(),
            file_cursor: 0,
            checks_open: HashSet::new(),
            check_details: HashMap::new(),
            finish: None,
            _load: None,
            _act: None,
            _since: None,
            _checks: Vec::new(),
            _subscriptions: subscriptions,
        };
        view.watched = ph_review::github::is_watched(&view.store, &view.pull_ref());
        view.drafts = crate::finish_view::drafts(&view.store, &view.pull_ref());
        view.read_viewed();
        view.split = view
            .store
            .setting::<Option<bool>>(files::SPLIT_KEY)
            .unwrap_or(false);
        view.reload(cx);
        view
    }

    fn pull_ref(&self) -> ph_store::reviews::PullRef {
        ph_store::reviews::PullRef {
            host: self.hosting.account.host.clone(),
            owner: self.hosting.repo.owner.clone(),
            repo: self.hosting.repo.name.clone(),
            number: self.number,
        }
    }

    fn toggle_watch(&mut self, cx: &mut Context<Self>) {
        let on = !self.watched;
        if ph_review::github::set_watched(&self.store, &self.pull_ref(), on).is_ok() {
            self.watched = on;
        }
        cx.notify();
    }

    /// Shows the Findings tab.
    pub fn show_findings(&mut self, cx: &mut Context<Self>) {
        self.tab = Tab::Findings;
        cx.notify();
    }

    /// Shows `path` in the Files tab, unfolded (a finding's place may be in
    /// a file ticked as read).
    fn show_line(&mut self, path: &str, _line: Option<u64>, cx: &mut Context<Self>) {
        self.tab = Tab::Files;
        // The whole change: the place may not have moved since the review.
        if !matches!(self.since, files::SinceState::Off)
            && let Some(data) = &self.data
        {
            self.since = files::SinceState::Off;
            self.files_list.reset(data.files.len());
        }
        if let Some(ix) = self
            .data
            .as_ref()
            .and_then(|data| data.files.iter().position(|f| f.filename == path))
        {
            self.unfolded.insert(ix);
            self.files_list.scroll_to_reveal_item(ix);
        }
        cx.notify();
    }

    /// Whether the keyboard is in one of the page's boxes.
    fn typing(&self, window: &Window, cx: &App) -> bool {
        use gpui_kit::Focusable as _;
        [&self.composer, &self.reply_box, &self.comment_box]
            .iter()
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
            || self.findings.read(cx).typing(window, cx)
    }

    /// j and k: the next finding or file, and the one before. v: the file
    /// is read. Only outside a box, and without a modifier.
    fn key_down(
        &mut self,
        event: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt || self.typing(window, cx) {
            return;
        }
        let forward = match keystroke.key.as_str() {
            "j" => true,
            "k" => false,
            "v" if self.tab == Tab::Files => {
                self.view_current(cx);
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        match self.tab {
            Tab::Findings => self
                .findings
                .update(cx, |findings, cx| findings.step(forward, cx)),
            Tab::Files => self.step_file(forward, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Opens the sheet a review is finished in: what goes to GitHub, and
    /// how the review ends.
    fn finish_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.data.clone() else {
            return;
        };
        let own = data
            .pull
            .user
            .login
            .eq_ignore_ascii_case(&self.hosting.account.login);
        let (store, pull) = (self.store.clone(), self.pull_ref());
        let (findings, reviewed) = {
            let findings = self.findings.read(cx);
            (findings.sendable(), findings.still_open())
        };
        let drafts = self.drafts.clone();
        let view =
            cx.new(|cx| FinishView::new(store, pull, own, reviewed, findings, drafts, window, cx));
        let events =
            cx.subscribe_in(
                &view,
                window,
                |this, _, event: &FinishEvent, window, cx| match event {
                    FinishEvent::Sent(said) => {
                        window.close_dialog(cx);
                        window.push_notification(Notification::success(said.clone()), cx);
                        this.finish = None;
                        this.drafts.clear();
                        this.composing = None;
                        // Watched from now on: the author's answer is news.
                        this.watched = ph_review::github::is_watched(&this.store, &this.pull_ref());
                        this.findings.update(cx, |findings, cx| findings.reload(cx));
                        this.reload(cx);
                    }
                    FinishEvent::DraftsChanged(drafts) => {
                        this.drafts = drafts.clone();
                        this.files_list.remeasure();
                        cx.notify();
                    }
                },
            );
        FinishView::open(&view, window, cx);
        self.finish = Some((view, events));
    }

    /// An agent review of this pull request ended: its line in the header
    /// says what it found.
    pub fn review_changed(&mut self, cx: &mut Context<Self>) {
        self.read_last_review();
        self.findings.update(cx, |findings, cx| findings.reload(cx));
        let changed = self.changed_paths();
        self.walkthrough
            .update(cx, |walkthrough, cx| walkthrough.reload(&changed, cx));
        cx.notify();
    }

    fn changed_paths(&self) -> Vec<String> {
        self.data
            .as_ref()
            .map(|data| data.files.iter().map(|f| f.filename.clone()).collect())
            .unwrap_or_default()
    }

    pub fn number(&self) -> u64 {
        self.number
    }

    /// What the newest agent review of this PR found (and which agent the
    /// next one runs with: read here, never while drawing).
    fn read_last_review(&mut self) {
        self.agent = ph_agent::chats::default_agent(&self.store);
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
                let open = findings.iter().filter(|f| f.status == "open").count();
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
            // The calls are independent: made at once, the page waits for
            // the slowest instead of the sum of all.
            let (client, repo) = (&client, &repo);
            std::thread::scope(|scope| {
                let files = scope.spawn(|| pulls::files(client, repo, number));
                let commits = scope.spawn(|| pulls::commits(client, repo, number));
                let conversation = scope.spawn(|| pulls::conversation(client, repo, number));
                let line_comments = scope.spawn(|| pulls::line_comments(client, repo, number));
                let reviews = scope.spawn(|| pulls::reviews(client, repo, number));
                let pull = pulls::get(client, repo, number)?;
                // Checks are nice to have; a token without that scope still works.
                let checks = pulls::checks(client, repo, &pull.head.sha).unwrap_or_default();
                let files = crate::joined(files)?;
                let commits = crate::joined(commits)?;
                let conversation = crate::joined(conversation)?;
                let line_comments = crate::joined(line_comments)?;
                let reviews = crate::joined(reviews)?;
                anyhow::Ok((
                    pull,
                    files,
                    commits,
                    conversation,
                    line_comments,
                    reviews,
                    checks,
                ))
            })
            .map(
                |(pull, files, commits, conversation, line_comments, reviews, checks)| {
                    let changes = files.iter().map(file_change).collect();
                    Loaded {
                        pull,
                        changes,
                        files,
                        commits,
                        conversation,
                        line_comments,
                        reviews,
                        checks,
                    }
                },
            )
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            _ = this.update(cx, |this, cx| {
                match result {
                    Ok(data) => {
                        let text = data.pull.body.clone().unwrap_or_default();
                        // (Text nested too deeply to parse is shown plain.)
                        let text = ph_ui::markdown::guarded(&text);
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
                            let body = ph_ui::markdown::guarded(&body);
                            bodies.insert(id, cx.new(|cx| TextViewState::markdown(&body, cx)));
                        }
                        this.bodies = bodies;
                        let (pull, changed): (PullRequest, Vec<String>) = (
                            data.pull.clone(),
                            data.files.iter().map(|f| f.filename.clone()).collect(),
                        );
                        this.data = Some(std::rc::Rc::new(data));
                        this.error = None;
                        // The same files keep their place (a reply was posted).
                        let shown = changed.len();
                        if this.files_list.item_count() == shown {
                            this.files_list.remeasure();
                        } else {
                            this.files_list.reset(shown);
                        }
                        this.findings
                            .update(cx, |findings, cx| findings.set_pull(pull.clone(), cx));
                        this.walkthrough.update(cx, |walkthrough, cx| {
                            walkthrough.set_pull(pull, &changed, cx)
                        });
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
        sent: Option<String>,
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
                        // Only the text that went out: a reply or a merge
                        // leaves a comment being written alone, and so does
                        // what was typed while this one was sent.
                        if sent.is_some_and(|sent| this.composer.read(cx).value().trim() == sent) {
                            this.composer
                                .update(cx, |input, cx| input.set_value("", window, cx));
                        }
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
            Some(body.clone()),
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
            None,
            move |h| pulls::reply(&h.client, &h.repo, number, thread, &body).map(|_| ()),
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
        let Some(head) = self.data.as_ref().map(|data| data.pull.head.sha.clone()) else {
            return;
        };
        let (number, method) = (self.number, self.merge_method);
        self.act(
            "Merging",
            "Merged",
            None,
            move |h| pulls::merge(&h.client, &h.repo, number, &head, method).map(|_| ()),
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
        let open = pull.state == "open" && !pull.merged;
        let mine = pull
            .user
            .login
            .eq_ignore_ascii_case(&self.hosting.account.login);
        let (has_review, to_send, unposted) = {
            let findings = self.findings.read(cx);
            (
                findings.has_review(),
                findings.sendable().len() + self.drafts.len(),
                // An agent reviewed it here and GitHub has nothing of that
                // yet: also when it found nothing, which is worth saying.
                findings.has_own_review() && !findings.posted(),
            )
        };
        // What comes next is the one filled button: an agent's review
        // first, then finishing it while there is something to send.
        let failing = data.checks.iter().filter(|check| check.failed()).count();
        let running = data
            .checks
            .iter()
            .any(|check| matches!(check.outcome.as_str(), "pending" | "queued" | "in_progress"));
        // Reviewers said something on the user's own pull request.
        let to_address = mine
            && open
            && (data
                .line_comments
                .iter()
                .any(|comment| !comment.user.login.eq_ignore_ascii_case(&pull.user.login))
                || data
                    .reviews
                    .iter()
                    .any(|review| review.state == "CHANGES_REQUESTED"));
        let review_pull = pull.clone();
        let agent = self.agent;
        let review_button = |primary: bool, cx: &mut Context<Self>| {
            Button::new("pull-review-agent")
                .small()
                .map(|this| if primary { this.primary() } else { this.outline() })
                .icon(Icons::Sparkles)
                .label(format!("Review with {}", agent.short_name()))
                .tooltip(ph_ui::tip(format!(
                    "{} reads the change on its own copy of the pull request. Nothing is sent to GitHub. (It is the agent chats start with: pick another in a chat's model list.)",
                    agent.name
                )))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(PullViewEvent::Agent(crate::AgentRequest::Review {
                        pull: Box::new(review_pull.clone()),
                        kind: "full",
                    }))
                }))
        };
        let finish_button = |primary: bool, cx: &mut Context<Self>| {
            Button::new("pull-finish")
                .small()
                .map(|this| if primary { this.primary() } else { this.outline() })
                .label(match to_send {
                    0 => "Finish review…".to_string(),
                    n => format!("Finish review · {n}"),
                })
                .tooltip(ph_ui::tip(if mine {
                    "Post what the agent found (or that it found nothing) and your comments on the code, in one post. On your own pull request a review can only comment."
                } else {
                    "Choose what is sent and how it ends: comment, approve or request changes. A review that found nothing can be posted too. One post."
                }))
                .on_click(cx.listener(|this, _, window, cx| this.finish_review(window, cx)))
        };
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
                    // Gives way in a narrow pane; the counts stay whole.
                    .child(div().min_w_0().truncate().child(format!(
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
                    )))
                    .child(
                        div()
                            .flex_none()
                            .child(format!("+{} −{}", pull.additions, pull.deletions)),
                    )
                    // The checks in a word; a click goes to them.
                    .when(open && (failing > 0 || running), |this| {
                        this.child(
                            div()
                                .id("pull-checks-chip")
                                .flex_none()
                                .cursor_pointer()
                                .text_color(if failing > 0 {
                                    cx.theme().danger
                                } else {
                                    cx.theme().warning
                                })
                                .child(match failing {
                                    0 => "checks running".to_string(),
                                    1 => "1 check failing".to_string(),
                                    n => format!("{n} checks failing"),
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.tab = Tab::Overview;
                                    this.conversation_scroll.scroll_to_bottom();
                                    cx.notify();
                                }))
                                .test_support(),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .flex_wrap()
                    .when(open, |this| {
                        // Finishing is always in reach, on one's own pull
                        // request too (where it can only comment): a
                        // review that found nothing is posted with it, and
                        // so is a note without any agent's review.
                        if to_send > 0 {
                            this.child(finish_button(true, cx))
                                .when(!has_review, |this| this.child(review_button(false, cx)))
                        } else if has_review {
                            // Nothing waits on the code: what the agent
                            // made of it can still be posted, and the
                            // review ended (approved) later.
                            this.child(finish_button(unposted, cx))
                        } else {
                            this.child(review_button(true, cx))
                                .child(finish_button(false, cx))
                        }
                    })
                    .when(to_address, |this| {
                        let mine = pull.clone();
                        this.child(
                            Button::new("pull-address")
                                .small()
                                .outline()
                                .icon(Icons::GitBranch)
                                .label("Address comments")
                                .tooltip(ph_ui::tip("A task on the branch: its agent works through the reviewers' comments not yet resolved, and drafts a reply for each"))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(PullViewEvent::Agent(crate::AgentRequest::Address {
                                        pull: Box::new(mine.clone()),
                                    }))
                                })),
                        )
                    })
                    // Watching shows while it is on, and turns off here.
                    .when(self.watched, |this| {
                        this.child(
                            Button::new("pull-watching")
                                .small()
                                .ghost()
                                .icon(IconName::Eye)
                                .label("Watching")
                                .selected(true)
                                .tooltip(ph_ui::tip("You are told when the author pushes or answers a finding. Click to stop."))
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_watch(cx))),
                        )
                    })
                    // The rest, used now and then, behind one button.
                    .child({
                        let view = cx.weak_entity();
                        let again = (open && has_review).then(|| pull.clone());
                        let (watched, local) = (self.watched, self.local.is_some());
                        Button::new("pull-more")
                            .small()
                            .ghost()
                            .icon(IconName::Ellipsis)
                            .accessibility_label("More for this pull request")
                            .tooltip("Review again, watch, check out, refresh")
                            .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                                use gpui_kit::component::menu::PopupMenuItem;
                                let mut menu = menu;
                                if let Some(again) = again.clone() {
                                    let view = view.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(format!(
                                            "Review again with {}",
                                            agent.short_name()
                                        ))
                                            .icon(Icons::Sparkles)
                                            .on_click(move |_, _, cx| {
                                                let pull = again.clone();
                                                _ = view.update(cx, |_, cx| {
                                                    cx.emit(PullViewEvent::Agent(
                                                        crate::AgentRequest::Review {
                                                            pull: Box::new(pull),
                                                            kind: "full",
                                                        },
                                                    ))
                                                });
                                            }),
                                    );
                                }
                                let watch = view.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(if watched {
                                        "Stop watching"
                                    } else {
                                        "Watch: tell me when the author pushes or answers"
                                    })
                                    .icon(if watched {
                                        IconName::EyeOff
                                    } else {
                                        IconName::Eye
                                    })
                                    .on_click(move |_, _, cx| {
                                        _ = watch.update(cx, |this, cx| this.toggle_watch(cx));
                                    }),
                                );
                                if local {
                                    let view = view.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new("Check out this branch")
                                            .icon(Icons::GitBranch)
                                            .on_click(move |_, _, cx| {
                                                _ = view.update(cx, |this, cx| this.check_out(cx));
                                            }),
                                    );
                                }
                                let view = view.clone();
                                menu.separator().item(
                                    PopupMenuItem::new("Refresh")
                                        .icon(IconName::RefreshCw)
                                        .on_click(move |_, _, cx| {
                                            _ = view.update(cx, |this, cx| this.reload(cx));
                                        }),
                                )
                            })
                    })
                    .when_some(self.busy, |this, busy| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .text_xs()
                                .child(ph_ui::Loader::new().xsmall())
                                .child(busy),
                        )
                    }),
            )
            .child(self.render_tabs(data, cx))
    }

    fn render_tabs(&self, data: &Loaded, cx: &mut Context<Self>) -> impl IntoElement {
        let findings = self.findings.read(cx);
        let findings_label = match (findings.has_review(), findings.open_count()) {
            (false, _) => "Findings".to_string(),
            (true, 0) => "Findings ✓".to_string(),
            (true, open) => format!("Findings {open}"),
        };
        // As the host's own page begins: the conversation, then what an
        // agent found, a briefing on the change, and the files. Four names
        // fit beside a chat; in a pane narrower still the ones not in
        // front fold to their icons.
        let tabs = [
            (Tab::Overview, "Conversation".to_string()),
            (Tab::Findings, findings_label),
            (Tab::Walkthrough, "Walkthrough".to_string()),
            (Tab::Files, format!("Files {}", data.files.len())),
        ];
        let view = cx.entity().downgrade();
        div().pt_1().child(ph_ui::PillTabs::new(
            "pull-tabs",
            tabs.into_iter()
                .map(|(tab, label)| {
                    let (name, icon) = match tab {
                        Tab::Overview => ("Conversation", Icons::ScrollText),
                        Tab::Findings => ("Findings", Icons::ShieldCheck),
                        Tab::Walkthrough => ("Walkthrough", Icons::ListOrdered),
                        Tab::Files => ("Files", Icons::Files),
                    };
                    let view = view.clone();
                    ph_ui::PillTab::new(
                        format!("pull-tab-{}", name.to_lowercase()),
                        label,
                        move |window, cx| {
                            _ = view.update(cx, |this, cx| {
                                this.tab = tab;
                                if tab == Tab::Findings {
                                    this.findings
                                        .update(cx, |findings, cx| findings.sync(window, cx));
                                }
                                cx.notify();
                            });
                        },
                    )
                    .name(name)
                    .icon(icon)
                    .selected(self.tab == tab)
                })
                .collect(),
        ))
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
                self.description.clone().map(|view| {
                    TextView::new(&view)
                        .image_source(ph_ui::images_safe::web_only)
                        .into_any_element()
                }),
                cx,
            ))
            .children(items.into_iter().map(|(_, item)| match item {
                Item::Comment(comment) => card(
                    comment.user.login.clone(),
                    &comment.created_at,
                    self.bodies.get(&comment.id).map(|view| {
                        TextView::new(view)
                            .image_source(ph_ui::images_safe::web_only)
                            .into_any_element()
                    }),
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
                                        .child(ph_ui::time::ago_of(
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
                                        .child(
                                            TextView::new(view)
                                                .image_source(ph_ui::images_safe::web_only),
                                        )
                                }))
                            },
                        )
                        .into_any_element()
                }
            }))
            .child(self.render_commits(data, cx))
            .child(self.render_status(data, cx))
            .when(open, |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(Textarea::new(&self.composer).small())
                        .child(
                            h_flex().justify_end().child(
                                Button::new("pull-comment")
                                    .small()
                                    .outline()
                                    .label("Comment")
                                    .disabled(self.busy.is_some())
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.comment(window, cx)),
                                    ),
                            ),
                        ),
                )
            })
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
                        .child(ph_ui::Loader::new())
                        .child(format!("Loading #{}…", self.number)),
                })
                .into_any_element();
        };
        let body = match self.tab {
            // A list that scrolls itself (inside, absolutely filled so it
            // gets an exact height).
            Tab::Files => v_flex()
                .flex_1()
                .min_h_0()
                .child(self.render_files(&data, cx))
                .into_any_element(),
            // (It scrolls itself, to bring a finding into sight.)
            Tab::Findings => div()
                .id("pull-findings")
                .flex_1()
                .min_h_0()
                .child(self.findings.clone())
                .into_any_element(),
            Tab::Walkthrough => div()
                .id("pull-walkthrough")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(self.walkthrough.clone())
                .into_any_element(),
            // (Its scroll is the page's own, so the checks under the
            // conversation can be gone to; the bar is laid over it.)
            Tab::Overview => div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("pull-body")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.conversation_scroll)
                        .child(self.render_overview(&data, cx)),
                )
                .vertical_scrollbar(&self.conversation_scroll)
                .into_any_element(),
        };
        v_flex()
            .id("pull-page")
            .size_full()
            // j and k move through the findings or the files, v ticks a
            // file as read: a click anywhere on the page gives it the keys.
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            // Wherever a pick of lines ends, on the page or off it.
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.end_pick(window, cx)),
            )
            .on_mouse_up_out(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.end_pick(window, cx)),
            )
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
                        .child(ph_ui::time::ago_of(when)),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
    use gpui_kit::{MouseButton, TestAppContext, VisualTestContext};
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
                owner: "o".into(),
                name: "r".into(),
            },
            remote: "origin".into(),
            client: Arc::new(Client::new(base, "t")),
        }
    }

    /// GitHub's answers for pull request 7 of `o/r`: one file, one failing
    /// check, nobody's review yet. (The specific paths first: the first
    /// route that matches answers.)
    fn server() -> MockServer {
        MockServer::start(vec![
            (
                "GET",
                "/repos/o/r/pulls/7/files",
                200,
                r#"[{"filename":"src/a.rs","status":"modified","additions":2,"deletions":0,"sha":"f1","patch":"@@ -1,2 +1,4 @@\n one\n+two\n+three\n four\n"}]"#,
            ),
            ("GET", "/repos/o/r/pulls/7/commits", 200, "[]"),
            ("GET", "/repos/o/r/pulls/7/comments", 200, "[]"),
            ("GET", "/repos/o/r/pulls/7/reviews", 200, "[]"),
            ("GET", "/repos/o/r/issues/7/comments", 200, "[]"),
            (
                "GET",
                "/repos/o/r/commits/abc123/check-runs",
                200,
                r#"{"check_runs":[{"id":2,"name":"test","status":"completed","conclusion":"failure","app":{"slug":"github-actions"}}]}"#,
            ),
            (
                "GET",
                "/repos/o/r/commits/abc123/status",
                200,
                r#"{"statuses":[]}"#,
            ),
            (
                "GET",
                "/repos/o/r/pulls/7",
                200,
                r#"{"number":7,"title":"Add two","state":"open","user":{"login":"maria"},"head":{"ref":"two","sha":"abc123"},"base":{"ref":"main"},"mergeable":true,"mergeable_state":"blocked"}"#,
            ),
        ])
    }

    fn shows(id: &'static str, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    }

    fn click(id: &'static str, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.render_frame(cx);
            // Near its left edge: a diff's rows are as wide as its longest
            // line, so the middle of one may be out of sight.
            let bounds = window.find(id).bounds();
            let at = gpui_kit::point(bounds.origin.x + gpui_kit::px(12.), bounds.center().y);
            window.pointer_down(at, MouseButton::Left, cx);
            window.pointer_up(at, MouseButton::Left, cx);
        });
        cx.run_until_parked();
    }

    /// A comment written on a line is kept, not sent: the page counts it
    /// on Finish review, and it is still there when the page is opened
    /// again. The merge says what stands in its way instead of offering a
    /// button GitHub would refuse.
    #[gpui_kit::test]
    fn a_line_comment_waits_for_the_review_to_be_finished(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            ph_theme::init(cx);
        });
        let server = server();
        let store = ph_store::Store::open_in_memory().unwrap();
        let (view, cx) = cx.add_window_view({
            let (store, hosting) = (store.clone(), hosting(&server.base));
            |window, cx| PullView::new(store, hosting, None, 7, window, cx)
        });
        cx.run_until_parked();
        assert!(
            view.read_with(cx, |view, _| view.data.is_some()),
            "the page is read"
        );

        // Nobody reviewed it: an agent's review is what comes first, and a
        // person's own is one click away.
        assert!(shows("pull-review-agent", cx) && shows("pull-finish", cx));
        // A required review is missing: no Merge to be refused.
        assert!(shows("pull-status", cx));
        assert!(!shows("pull-merge", cx));
        assert_eq!(
            view.read_with(cx, |view, _| status::Mergeable::of(
                &view.data.as_ref().unwrap().pull
            )),
            status::Mergeable::Blocked
        );

        click("pull-tab-files", cx);
        // The second line of the hunk: `+two`, line 2 of the new file.
        click("line-0-0-1", cx);
        view.update_in(cx, |view, window, cx| {
            let composing = view.composing.clone().expect("a click starts a comment");
            assert_eq!((composing.start, composing.end), (2, 2));
            view.comment_box
                .update(cx, |input, cx| input.set_value("Why two?", window, cx));
            view.add_draft(window, cx);
            assert!(view.composing.is_none());
            assert_eq!(view.drafts.len(), 1);
        });
        let kept =
            crate::finish_view::drafts(&store, &view.read_with(cx, |view, _| view.pull_ref()));
        assert_eq!(kept[0].body, "Why two?");
        assert_eq!((kept[0].path.as_str(), kept[0].line), ("src/a.rs", 2));

        // Pressing on a line and dragging over the next picks both; the
        // box waits for the button to be let go, so it does not push the
        // lines from under the pointer.
        let edge = |id: &'static str, window: &mut Window| {
            let bounds = window.find(id).bounds();
            gpui_kit::point(bounds.origin.x + gpui_kit::px(12.), bounds.center().y)
        };
        let to = cx.update(|window, cx| {
            window.render_frame(cx);
            let (from, to) = (edge("line-0-0-1", window), edge("line-0-0-2", window));
            window.pointer_down(from, MouseButton::Left, cx);
            window.pointer_move(to, Some(MouseButton::Left), cx);
            to
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let composing = view.composing.as_ref().unwrap();
            assert_eq!((composing.start, composing.end), (2, 3));
            assert!(view.picking);
        });
        assert!(
            !shows("line-composer", cx),
            "not while the lines are picked"
        );
        cx.update(|window, cx| window.pointer_up(to, MouseButton::Left, cx));
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| !view.picking));
        assert!(shows("line-composer", cx));
        // What a right click on them offers is about both lines.
        assert_eq!(
            view.read_with(cx, |view, _| view.lines_text("src/a.rs", 2, 3)),
            ["two", "three"]
        );

        // Side by side, the lines keep their names and what is said of them.
        view.update(cx, |view, cx| {
            view.composing = None;
            view.set_split(true, cx)
        });
        assert!(shows("line-0-0-1", cx));
        view.update(cx, |view, cx| view.set_split(false, cx));

        // Read: the file folds, and its tick is kept by its content.
        click("file-viewed-src/a.rs", cx);
        assert!(view.read_with(cx, |view, _| view.viewed.contains("src/a.rs@f1")));
        assert!(!shows("line-0-0-1", cx), "a file that was read is folded");
    }
}
