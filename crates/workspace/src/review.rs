//! Agent reviews of pull requests, from the workspace's side: getting a
//! round ready (checkout, prompt). The GitHub side of the findings is
//! [`ph_review::github`]. Blocking (network, git): run it off the UI thread.

use std::path::PathBuf;

use anyhow::{Result, bail};
use gpui_kit::App;
use ph_agent::review_card::ReviewService;
use ph_github::models::PullRequest;
use ph_github::pulls;
use ph_pulls::Hosting;
use ph_review::github::learned_rules;
use ph_review::prompt::{Guidance, PullInfo, RoundKind, round_prompt};
use ph_review::templates::Template;
use ph_store::Store;
use ph_store::projects::ProjectId;
use ph_store::reviews::{Finding, PullRef};

/// Sets up what review cards need.
pub fn install(store: Store, cx: &mut App) {
    cx.set_global(ReviewService { store });
}

/// What the chat needs to start a review round.
pub struct Prepared {
    pub review_id: i64,
    pub worktree: PathBuf,
    pub title: String,
    pub display: String,
    pub prompt: String,
}

/// The pull request as the prompt names it.
fn info<'a>(hosting: &'a Hosting, pull: &'a PullRequest, sha: &'a str) -> PullInfo<'a> {
    PullInfo {
        number: pull.number,
        title: &pull.title,
        body: pull.body.as_deref().unwrap_or_default(),
        author: &pull.user.login,
        owner: &hosting.repo.owner,
        repo: &hosting.repo.name,
        base: &pull.base.name,
        head: &pull.head.name,
        head_sha: sha,
    }
}

pub fn pull_ref(hosting: &Hosting, number: u64) -> PullRef {
    PullRef {
        host: hosting.account.host.clone(),
        owner: hosting.repo.owner.clone(),
        repo: hosting.repo.name.clone(),
        number,
    }
}

/// Gets a round of kind `kind` (`full`, `followup`, `second`,
/// `double_check`) by agent `agent` ready: checks the pull request's head
/// out beside the repository's managed clone (never the user's checkout),
/// takes out what would act on the agent, records the round and writes the
/// prompt from review `template` and the repository's guidance.
pub fn prepare(
    store: &Store,
    project_id: ProjectId,
    hosting: &Hosting,
    pull: &PullRequest,
    template: &Template,
    kind: &str,
    agent: &str,
) -> Result<Prepared> {
    // The latest head: the PR may have moved since the list was read.
    let pull =
        pulls::get(&hosting.client, &hosting.repo, pull.number).unwrap_or_else(|_| pull.clone());
    let (worktree, sha, removed) = ph_pulls::clones::check_out_pull(hosting, pull.number)?;
    let clone = ph_pulls::clones::RepoId::of(hosting).clone_dir();
    let pull_ref = pull_ref(hosting, pull.number);
    let guidance = Guidance {
        rules: ph_pulls::clones::team_rules(&clone, &pull.base.name),
        learned: learned_rules(store, &pull_ref),
        notes: ph_pulls::clones::agent_notes(&clone, &pull.base.name),
    };
    let latest = store.latest_pr_review(&pull_ref, None)?;
    let open: Vec<Finding> = match &latest {
        Some(round) => store
            .pr_findings(round.id)?
            .into_iter()
            .filter(|f| f.status == "open")
            .collect(),
        None => Vec::new(),
    };
    // A check of the fixes needs an earlier round on another commit.
    let kind = match (kind, &latest) {
        ("followup", Some(round)) if round.head_sha != sha => "followup",
        ("followup", _) => "full",
        ("double_check", _) if open.is_empty() => {
            bail!("There are no open findings to double-check.")
        }
        (kind, _) => kind,
    };
    let round_kind = match kind {
        "followup" => RoundKind::CheckFixes {
            since: latest.as_ref().map_or("", |round| round.head_sha.as_str()),
        },
        "second" => RoundKind::Second,
        "double_check" => RoundKind::DoubleCheck,
        _ => RoundKind::Full,
    };
    let prompt = round_prompt(
        &info(hosting, &pull, &sha),
        &round_kind,
        &template.instructions,
        &guidance,
        &open,
    );
    let review = store.create_pr_run(project_id, &pull_ref, &pull.title, &sha, kind, agent)?;
    let what = match kind {
        "followup" => "Check of the fixes".to_string(),
        "second" => "Second opinion".to_string(),
        "double_check" => "Double-check".to_string(),
        _ if template.id == ph_review::templates::REVIEW => "Review".to_string(),
        _ => template.name.clone(),
    };
    let mut notes = Vec::new();
    if guidance.rules.is_some() {
        notes.push(format!(
            "the repository's rules ({})",
            ph_review::templates::TEAM_RULES
        ));
    }
    if !guidance.learned.is_empty() {
        notes.push(match guidance.learned.len() {
            1 => "1 learned rule".to_string(),
            n => format!("{n} learned rules"),
        });
    }
    if !guidance.notes.is_empty() {
        notes.push(
            guidance
                .notes
                .iter()
                .map(|(file, _)| file.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    let mut display = format!(
        "{what} of #{} “{}” (round {}, {} → {}, at {}).",
        pull.number,
        pull.title,
        review.round,
        pull.head.name,
        pull.base.name,
        &sha[..sha.len().min(8)]
    );
    if !notes.is_empty() {
        display.push_str(&format!(" With {}.", notes.join(", ")));
    }
    if !removed.is_empty() {
        display.push_str(&format!(
            " Took out the pull request's agent settings so they cannot act on the agent: {}.",
            removed.join(", ")
        ));
    }
    Ok(Prepared {
        review_id: review.id,
        worktree,
        title: format!("{what} #{}: {}", pull.number, pull.title),
        display,
        prompt,
    })
}

/// Gets a review of the user's own changes in checkout `root` ready, before
/// they push: what is not pushed yet (commits ahead of the upstream, and
/// work not committed). Recorded as a `self` round of the branch, so the
/// next review carries its findings on.
pub fn prepare_self(
    store: &Store,
    project_id: ProjectId,
    root: &std::path::Path,
    template: &Template,
    agent: &str,
) -> Result<Prepared> {
    let branch = ph_git::git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|name| name.trim().to_string())
        .unwrap_or_default();
    let sha = ph_git::git(root, &["rev-parse", "HEAD"])?
        .trim()
        .to_string();
    let upstream = ph_git::git(
        root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .map(|name| name.trim().to_string())
    .ok()
    .filter(|name| !name.is_empty());
    let (what, diff) = match &upstream {
        Some(upstream) => (
            format!(
                "the changes on `{branch}` not pushed to `{upstream}` yet (commits and work not committed)"
            ),
            "git diff @{upstream}".to_string(),
        ),
        None => (
            "the work not committed yet".to_string(),
            "git diff HEAD (and `git status` for new files)".to_string(),
        ),
    };
    // The rules the repository's reviews use, and those learned for it.
    let rules = std::fs::read_to_string(root.join(ph_review::templates::TEAM_RULES))
        .ok()
        .filter(|text| !text.trim().is_empty())
        .map(|text| text.chars().take(20_000).collect());
    let learned = ph_git::remote::hosted_repo(root)
        .map(|hosted| {
            learned_rules(
                store,
                &PullRef {
                    host: hosted.host.to_ascii_lowercase(),
                    owner: hosted.owner,
                    repo: hosted.name,
                    number: 0,
                },
            )
        })
        .unwrap_or_default();
    let guidance = Guidance {
        rules,
        learned,
        notes: Vec::new(),
    };
    let prompt =
        ph_review::prompt::self_review_prompt(&what, &diff, &template.instructions, &guidance);
    let pull = PullRef {
        host: "local".into(),
        owner: project_id.to_string(),
        repo: if branch.is_empty() {
            "HEAD".into()
        } else {
            branch.clone()
        },
        number: 0,
    };
    let title = format!("Your changes on {branch}");
    let review = store.create_pr_run(project_id, &pull, &title, &sha, "self", agent)?;
    Ok(Prepared {
        review_id: review.id,
        worktree: root.to_path_buf(),
        display: format!("Review {what} (round {}).", review.round),
        title,
        prompt,
    })
}
