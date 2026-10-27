//! Work for agents about pull requests, asked from their views: review
//! rounds, checking an author's answer, walkthroughs, questions about
//! lines, and fixes (in a task, through the window).

use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::{AppContext as _, Context, Window};
use ph_agent::ReviewTurn;
use ph_agent::agents::{AGENTS, AgentSpec, Availability};
use ph_github::models::PullRequest;
use ph_pulls::clones::RepoId;
use ph_pulls::{AgentRequest, Hosting};
use ph_review::templates::Template;

use super::GithubView;

impl GithubView {
    /// Does `request` about a pull request of `hosting`'s repository.
    pub fn agent(
        &mut self,
        hosting: Hosting,
        request: AgentRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match request {
            AgentRequest::Review { pull, kind } => {
                let template = ph_review::templates::default_review(&self.store);
                self.review(hosting, *pull, template, kind, window, cx)
            }
            AgentRequest::Claim { finding_id } => self.check_claim(hosting, finding_id, window, cx),
            AgentRequest::Walkthrough { pull } => self.walkthrough(hosting, *pull, window, cx),
            AgentRequest::AskLines {
                pull,
                path,
                start,
                end,
                question,
            } => self.ask_lines(hosting, *pull, path, start, end, question, window, cx),
            request @ (AgentRequest::Fix { .. } | AgentRequest::Address { .. }) => {
                // Fixes are made in a task: a workspace on the PR's branch.
                let workspace = self.workspace.clone();
                window.defer(cx, move |window, cx| {
                    _ = workspace.update(cx, |workspace, cx| {
                        workspace.fix_pull(hosting, request, window, cx)
                    });
                });
            }
        }
    }

    /// The agent a round of kind `kind` runs with: the default, or for a
    /// second opinion one that has not reviewed the pull request yet.
    fn reviewer(
        &self,
        hosting: &Hosting,
        number: u64,
        kind: &str,
    ) -> Result<&'static AgentSpec, String> {
        let default = ph_agent::chats::default_agent(&self.store);
        if kind != "second" {
            return Ok(default);
        }
        let pull = crate::review::pull_ref(hosting, number);
        let used: Vec<String> = ph_review::github::current(&self.store, &pull)
            .ok()
            .flatten()
            .map(|(_, findings)| {
                findings
                    .iter()
                    .flat_map(|f| f.agents.split(", ").map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        AGENTS
            .iter()
            .filter(|agent| !used.iter().any(|name| name == agent.name))
            .find(|agent| matches!(agent.availability(), Availability::Ready { .. }))
            .ok_or_else(|| {
                "A second opinion needs another agent installed: every installed agent has reviewed this pull request.".to_string()
            })
    }

    /// Starts a review round of kind `kind` (`full`, `followup`, `second`,
    /// `double_check`) of `pull` with review `template`: checks it out
    /// beside the repository's clone (made the first time), then opens a
    /// review chat there that starts by itself.
    pub fn review(
        &mut self,
        hosting: Hosting,
        pull: PullRequest,
        template: Template,
        kind: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let agent = match self.reviewer(&hosting, pull.number, kind) {
            Ok(agent) => agent,
            Err(why) => {
                window.push_notification(Notification::warning(why), cx);
                return;
            }
        };
        let id = RepoId::of(&hosting);
        let number = pull.number;
        self.show_pull(id.clone(), number, window, cx);
        let project = match self.space(&id) {
            Ok(project) => project,
            Err(err) => {
                window.push_notification(Notification::error(format!("{err:#}")), cx);
                return;
            }
        };
        let chat = self.chat_for(&id, project.clone(), window, cx);
        let first = !id.clone_dir().join(".git").exists();
        chat.update(cx, |chat, cx| {
            chat.set_busy(
                Some(if first {
                    format!("Cloning {} for the review of #{number}…", id.full_name())
                } else {
                    format!("Getting #{number} ready for the review…")
                }),
                cx,
            )
        });
        let store = self.store.clone();
        let work = cx.background_spawn(async move {
            crate::review::prepare(
                &store, project.id, &hosting, &pull, &template, kind, agent.name,
            )
        });
        self._work.push(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |_, window, cx| {
                chat.update(cx, |chat, cx| match result {
                    Ok(prepared) => chat.start_review_turn(
                        Some(prepared.review_id),
                        Some(ReviewTurn::Round),
                        Some(agent),
                        prepared.worktree,
                        prepared.title,
                        prepared.display,
                        prepared.prompt,
                        window,
                        cx,
                    ),
                    Err(err) => chat.set_failed(format!("Could not start the review: {err:#}"), cx),
                });
                cx.notify();
            });
        }));
    }

    /// Checks the fixes for review `review_id`: a new round on its pull
    /// request's latest head, about each open finding and what changed.
    pub fn review_again(&mut self, review_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.with_pull(review_id, window, cx, |this, hosting, pull, window, cx| {
            let template = ph_review::templates::default_review(&this.store);
            this.review(hosting, pull, template, "followup", window, cx)
        });
    }

    /// Shows the findings of the pull request review `review_id` is about.
    pub fn open_findings(&mut self, review_id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(review) = self.store.pr_review(review_id).ok().flatten() else {
            return;
        };
        let id = RepoId {
            host: review.pull.host.clone(),
            owner: review.pull.owner.clone(),
            name: review.pull.repo.clone(),
        };
        self.show_pull(id.clone(), review.pull.number, window, cx);
        if let Some(panel) = self.panel_of(&id).cloned() {
            panel.update(cx, |panel, cx| panel.show_findings(cx));
        }
    }

    /// Finds review `review_id`'s repository and pull request (as it is
    /// now), then runs `then` with them.
    fn with_pull(
        &mut self,
        review_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Hosting, PullRequest, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let store = self.store.clone();
        let find = cx.background_spawn(async move {
            let review = store
                .pr_review(review_id)?
                .ok_or_else(|| anyhow::anyhow!("review not found"))?;
            let id = RepoId {
                host: review.pull.host.clone(),
                owner: review.pull.owner.clone(),
                name: review.pull.repo.clone(),
            };
            let hosting = ph_pulls::hosting::for_repo(&store, &id)
                .map_err(|missing| anyhow::anyhow!("{}", missing.explain()))?;
            let pull = ph_github::pulls::get(&hosting.client, &hosting.repo, review.pull.number)?;
            anyhow::Ok((hosting, pull))
        });
        self._work.push(cx.spawn_in(window, async move |this, cx| {
            let result = find.await;
            _ = this.update_in(cx, |this, window, cx| match result {
                Ok((hosting, pull)) => then(this, hosting, pull, window, cx),
                Err(err) => window.push_notification(Notification::error(format!("{err:#}")), cx),
            });
        }));
    }

    /// Asks an agent whether the author's answer on finding `finding_id`
    /// holds up, in the pull request's checkout; the verdict and a drafted
    /// reply land on the finding.
    fn check_claim(
        &mut self,
        hosting: Hosting,
        finding_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(finding) = self.store.pr_finding(finding_id).ok().flatten() else {
            return;
        };
        let Some(review) = self.store.pr_review(finding.review_id).ok().flatten() else {
            return;
        };
        let number = review.pull.number;
        let (id, title) = (
            RepoId::of(&hosting),
            format!("{} on #{number}", finding.label),
        );
        let Some(chat) = self.chat_ready(&id, number, window, cx) else {
            return;
        };
        let work = cx.background_spawn(async move {
            let (worktree, _, _) = ph_pulls::clones::check_out_pull(&hosting, number)?;
            anyhow::Ok(worktree)
        });
        let who = if finding.status_by.is_empty() {
            "The author".to_string()
        } else {
            finding.status_by.clone()
        };
        let claim = format!(
            "{}: {}",
            ph_review::comment::status_label(&finding.status),
            finding.reason
        );
        let prompt = ph_review::prompt::claim_prompt(&finding, &who, &claim);
        let display = format!(
            "Is {who} right about {} (“{}”)? They said: {claim}",
            finding.label, finding.title
        );
        self._work.push(cx.spawn_in(window, async move |_, cx| {
            let result = work.await;
            _ = chat.update_in(cx, |chat, window, cx| match result {
                Ok(worktree) => chat.start_review_turn(
                    None,
                    Some(ReviewTurn::Claim { finding_id }),
                    None,
                    worktree,
                    title,
                    display,
                    prompt,
                    window,
                    cx,
                ),
                Err(err) => chat.set_failed(format!("{err:#}"), cx),
            });
        }));
    }

    /// Asks an agent for a walkthrough of `pull`, shown in its Walkthrough
    /// tab when the turn ends.
    fn walkthrough(
        &mut self,
        hosting: Hosting,
        pull: PullRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = RepoId::of(&hosting);
        let number = pull.number;
        let Some(chat) = self.chat_ready(&id, number, window, cx) else {
            return;
        };
        let Ok(project) = self.space(&id) else {
            return;
        };
        let (store, agent) = (
            self.store.clone(),
            ph_agent::chats::default_agent(&self.store),
        );
        let work = cx.background_spawn(async move {
            let (worktree, sha, _) = ph_pulls::clones::check_out_pull(&hosting, number)?;
            let info = ph_review::prompt::PullInfo {
                number,
                title: &pull.title,
                body: pull.body.as_deref().unwrap_or_default(),
                author: &pull.user.login,
                owner: &hosting.repo.owner,
                repo: &hosting.repo.name,
                base: &pull.base.name,
                head: &pull.head.name,
                head_sha: &sha,
            };
            let prompt = ph_review::prompt::walkthrough_prompt(&info);
            let run = store.create_pr_run(
                project.id,
                &crate::review::pull_ref(&hosting, number),
                &pull.title,
                &sha,
                "walkthrough",
                agent.name,
            )?;
            anyhow::Ok((
                run.id,
                worktree,
                format!("Walkthrough #{number}: {}", pull.title),
                format!("Walk me through #{number} “{}”.", pull.title),
                prompt,
            ))
        });
        self._work.push(cx.spawn_in(window, async move |_, cx| {
            let result = work.await;
            _ = chat.update_in(cx, |chat, window, cx| match result {
                Ok((run_id, worktree, title, display, prompt)) => chat.start_review_turn(
                    None,
                    Some(ReviewTurn::Walkthrough { run_id }),
                    Some(agent),
                    worktree,
                    title,
                    display,
                    prompt,
                    window,
                    cx,
                ),
                Err(err) => chat.set_failed(format!("{err:#}"), cx),
            });
        }));
    }

    /// Asks `question` about lines `start..=end` of `path` in `pull`, in a
    /// chat beside it.
    #[allow(clippy::too_many_arguments)]
    fn ask_lines(
        &mut self,
        hosting: Hosting,
        pull: PullRequest,
        path: String,
        start: u64,
        end: u64,
        question: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = RepoId::of(&hosting);
        let number = pull.number;
        let Some(chat) = self.chat_ready(&id, number, window, cx) else {
            return;
        };
        let work = cx.background_spawn(async move {
            let (worktree, sha, _) = ph_pulls::clones::check_out_pull(&hosting, number)?;
            let info = ph_review::prompt::PullInfo {
                number,
                title: &pull.title,
                body: pull.body.as_deref().unwrap_or_default(),
                author: &pull.user.login,
                owner: &hosting.repo.owner,
                repo: &hosting.repo.name,
                base: &pull.base.name,
                head: &pull.head.name,
                head_sha: &sha,
            };
            let prompt = ph_review::prompt::lines_prompt(&info, &path, start, end, &question);
            anyhow::Ok((
                worktree,
                format!("{path}:{start} in #{number}"),
                format!("About `{path}` lines {start}-{end}: {}", question.trim()),
                prompt,
            ))
        });
        self._work.push(cx.spawn_in(window, async move |_, cx| {
            let result = work.await;
            _ = chat.update_in(cx, |chat, window, cx| match result {
                Ok((worktree, title, display, prompt)) => {
                    chat.start_question(worktree, title, display, prompt, window, cx)
                }
                Err(err) => chat.set_failed(format!("{err:#}"), cx),
            });
        }));
    }

    /// The chat beside pull request `number` of repository `id`, shown and
    /// busy getting it ready.
    fn chat_ready(
        &mut self,
        id: &RepoId,
        number: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::Entity<ph_agent::ChatPanel>> {
        self.show_pull(id.clone(), number, window, cx);
        let project = match self.space(id) {
            Ok(project) => project,
            Err(err) => {
                window.push_notification(Notification::error(format!("{err:#}")), cx);
                return None;
            }
        };
        let chat = self.chat_for(id, project, window, cx);
        chat.update(cx, |chat, cx| {
            chat.set_busy(Some(format!("Getting #{number} ready…")), cx)
        });
        Some(chat)
    }
}
