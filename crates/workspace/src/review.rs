//! Agent reviews of pull requests, from the workspace's side.
//!
//! - [`install`] gives review cards their GitHub actions (post findings,
//!   mark them fixed), which need the user's account.
//! - [`prepare`] checks the PR out in its own worktree in PilotHouse's data
//!   folder, records the review run and builds the prompt; the chat panel
//!   then opens a review chat there and starts it.

use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context as _, Result, bail};
use gpui_kit::{App, AppContext as _};
use ph_agent::review_card::ReviewService;
use ph_github::models::PullRequest;
use ph_github::{Client, Repo, accounts, pulls};
use ph_pulls::Hosting;
use ph_review::post::{self, PostMode};
use ph_review::prompt::{PullInfo, review_prompt};
use ph_store::Store;
use ph_store::projects::ProjectId;
use ph_store::reviews::{PrReview, PullRef};

/// Sets up the review card actions for the whole app.
pub fn install(store: Store, cx: &mut App) {
    let (post_store, announce_store) = (store.clone(), store.clone());
    cx.set_global(ReviewService {
        store,
        post: Rc::new(move |review_id, ids, mode, cx| {
            let store = post_store.clone();
            cx.background_spawn(async move { post_findings(&store, review_id, &ids, mode) })
        }),
        announce_fixed: Rc::new(move |review_id, cx| {
            let store = announce_store.clone();
            cx.background_spawn(async move { announce_fixed(&store, review_id) })
        }),
    });
}

/// A client for the review's host, with its repository.
fn client_for(store: &Store, review: &PrReview) -> Result<(Client, Repo)> {
    let account = accounts::for_host(store, &review.pull.host).with_context(|| {
        format!(
            "No GitHub account for {} (Settings → GitHub).",
            review.pull.host
        )
    })?;
    Ok((
        account.client()?,
        Repo::new(&review.pull.owner, &review.pull.repo)?,
    ))
}

fn post_findings(store: &Store, review_id: i64, ids: &[i64], mode: PostMode) -> Result<String> {
    let review = store.pr_review(review_id)?.context("review not found")?;
    let findings: Vec<_> = store
        .pr_findings(review_id)?
        .into_iter()
        .filter(|finding| ids.contains(&finding.id))
        .collect();
    if findings.is_empty() {
        bail!("Pick findings to post first.");
    }
    let (client, repo) = client_for(store, &review)?;
    let files = pulls::files(&client, &repo, review.pull.number)?;
    let target = post::Target {
        client: &client,
        repo: &repo,
        number: review.pull.number,
        head: &review.head_sha,
    };
    let posted = post::post(&target, &review.summary, &findings, &files, mode)?;
    for item in &posted {
        store.set_finding_comment(item.finding_id, item.comment_id, item.kind)?;
    }
    let lines = posted.iter().filter(|item| item.kind == "line").count();
    Ok(match mode {
        PostMode::Inline => format!(
            "Posted {} finding{} to #{} ({lines} on lines)",
            posted.len(),
            if posted.len() == 1 { "" } else { "s" },
            review.pull.number
        ),
        PostMode::Conversation => format!(
            "Posted one comment with {} findings to #{}",
            posted.len(),
            review.pull.number
        ),
    })
}

fn announce_fixed(store: &Store, review_id: i64) -> Result<String> {
    let review = store.pr_review(review_id)?.context("review not found")?;
    let fixed: Vec<_> = store
        .pr_findings(review_id)?
        .into_iter()
        .filter(|f| f.status == "fixed" && f.comment_id.is_some() && !f.fixed_notified)
        .collect();
    let (client, repo) = client_for(store, &review)?;
    let target = post::Target {
        client: &client,
        repo: &repo,
        number: review.pull.number,
        head: &review.head_sha,
    };
    let done = post::announce_fixed(&target, &fixed)?;
    for id in &done {
        store.set_finding_notified(*id)?;
    }
    Ok(format!(
        "Marked {} finding{} fixed on GitHub",
        done.len(),
        if done.len() == 1 { "" } else { "s" }
    ))
}

/// What the chat needs to start a review.
pub struct Prepared {
    pub review_id: i64,
    pub worktree: PathBuf,
    pub title: String,
    pub display: String,
    pub prompt: String,
}

/// Checks the PR out in its worktree, records the run and writes the prompt
/// (blocking: run off the UI thread).
pub fn prepare(
    store: &Store,
    project_id: ProjectId,
    root: &std::path::Path,
    hosting: &Hosting,
    pull: &PullRequest,
) -> Result<Prepared> {
    // The latest head: the PR may have moved since the list was read.
    let pull =
        pulls::get(&hosting.client, &hosting.repo, pull.number).unwrap_or_else(|_| pull.clone());
    let sha = ph_pulls::hosting::fetch_head(root, hosting, pull.number)?;
    // The base, so the agent can diff against it.
    let _ = ph_git::worktree::fetch_ref(root, &hosting.remote, &pull.base.name);
    let worktree = worktree_path(project_id, pull.number);
    if worktree.join(".git").exists() {
        ph_git::git(&worktree, &["checkout", "--quiet", "--detach", &sha])?;
    } else {
        if let Some(parent) = worktree.parent() {
            ph_store::paths::ensure_private_dir(parent)?;
        }
        ph_git::worktree::add_detached(root, &worktree, &sha)?;
    }

    let pull_ref = PullRef {
        host: hosting.account.host.clone(),
        owner: hosting.repo.owner.clone(),
        repo: hosting.repo.name.clone(),
        number: pull.number,
    };
    let review = store.create_pr_review(project_id, &pull_ref, &pull.title, &sha)?;
    let previous = store
        .latest_pr_review(&pull_ref, Some(review.id))?
        .map(|earlier| store.pr_findings(earlier.id))
        .transpose()?
        .unwrap_or_default();
    let info = PullInfo {
        number: pull.number,
        title: &pull.title,
        body: pull.body.as_deref().unwrap_or_default(),
        author: &pull.user.login,
        owner: &hosting.repo.owner,
        repo: &hosting.repo.name,
        base: &pull.base.name,
        head: &pull.head.name,
        head_sha: &sha,
    };
    let again = if previous.is_empty() { "" } else { " again" };
    Ok(Prepared {
        review_id: review.id,
        worktree,
        title: format!("Review #{}: {}", pull.number, pull.title),
        display: format!(
            "Review pull request #{} “{}”{again} ({} → {}, at {}).",
            pull.number,
            pull.title,
            pull.head.name,
            pull.base.name,
            &sha[..sha.len().min(8)]
        ),
        prompt: review_prompt(&info, &previous),
    })
}

/// `<data dir>/worktrees/<project>-pr-<number>`.
pub fn worktree_path(project_id: ProjectId, number: u64) -> PathBuf {
    ph_store::paths::data_dir()
        .join("worktrees")
        .join(format!("{project_id}-pr-{number}"))
}
