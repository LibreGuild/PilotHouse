//! The GitHub side of a pull request's findings: publishing them,
//! reading back ticks and replies, replying, statuses set by the reviewer,
//! issues, and the review rules a repository learns. Blocking (network):
//! run it off the UI thread.

use anyhow::{Context as _, Result, bail};
use ph_github::{Client, Repo, accounts, pulls};
use ph_store::Store;
use ph_store::reviews::{Finding, PrReview, PullRef};

use crate::post::{self, PostMode};

/// A client for the review's host, its repository, and the account's login.
pub fn client_for(store: &Store, pull: &PullRef) -> Result<(Client, Repo, String)> {
    let account = accounts::for_host(store, &pull.host)
        .with_context(|| format!("No GitHub account for {} (Settings → GitHub).", pull.host))?;
    Ok((
        account.client()?,
        Repo::new(&pull.owner, &pull.repo)?,
        account.login.clone(),
    ))
}

/// The settings key of a repository's learned review rules.
fn rules_key(pull: &PullRef) -> String {
    format!("review.learned.{}/{}/{}", pull.host, pull.owner, pull.repo)
}

/// The rules learned for `pull`'s repository from findings marked not a
/// problem (see [`learn`]).
pub fn learned_rules(store: &Store, pull: &PullRef) -> Vec<String> {
    store.setting(&rules_key(pull))
}

/// Remembers `rule` for `pull`'s repository: every later review is told.
pub fn learn(store: &Store, pull: &PullRef, rule: &str) -> Result<()> {
    let rule = rule.trim();
    if rule.is_empty() {
        return Ok(());
    }
    let mut rules = learned_rules(store, pull);
    if !rules.iter().any(|known| known == rule) {
        rules.push(rule.to_string());
    }
    store.set_setting(&rules_key(pull), &rules)
}

/// Forgets learned rule `ix` of `pull`'s repository.
pub fn forget(store: &Store, pull: &PullRef, ix: usize) -> Result<()> {
    let mut rules = learned_rules(store, pull);
    if ix < rules.len() {
        rules.remove(ix);
    }
    store.set_setting(&rules_key(pull), &rules)
}

/// The pull request's findings now: its newest round's.
pub fn current(store: &Store, pull: &PullRef) -> Result<Option<(PrReview, Vec<Finding>)>> {
    let Some(round) = store.latest_pr_review(pull, None)? else {
        return Ok(None);
    };
    let findings = store.pr_findings(round.id)?;
    Ok(Some((round, findings)))
}

/// What reading GitHub back found.
#[derive(Debug, Default)]
pub struct Synced {
    /// Statuses changed by ticks or replies: (label, status, who, reason).
    pub changed: Vec<(String, String, String, String)>,
    /// Replies that claim nothing (conversation), as (label, who).
    pub talked: Vec<(String, String)>,
}

/// Reads ticks in the summary comment and replies on findings, and applies
/// them to the newest round's findings (an author's word is a claim the
/// next check verifies).
pub fn sync(store: &Store, pull: &PullRef) -> Result<Synced> {
    let mut synced = Synced::default();
    let Some((round, mut findings)) = current(store, pull)? else {
        return Ok(synced);
    };
    let (client, repo, me) = client_for(store, pull)?;
    let target = post::Target {
        client: &client,
        repo: &repo,
        number: pull.number,
        head: &round.head_sha,
    };
    let mut touched = Vec::new();
    if let Some(comment) = store
        .summary_comment_of(pull)?
        .and_then(|id| post::read_summary(&target, id))
    {
        for (label, status) in post::new_ticks(&comment.body, &findings) {
            if let Some(finding) = findings.iter_mut().find(|f| f.label == label) {
                // A tick on a finding someone already moved is old news, and
                // the reviewer's own word (a reopen) wins over a tick made
                // before it (the boxes are fresh on the next post).
                if finding.status == "open" && finding.status_source != "me" {
                    finding.status = status.clone();
                    finding.status_source = "author".into();
                    finding.status_by = String::new();
                    finding.reason = String::new();
                    touched.push(finding.id);
                    synced
                        .changed
                        .push((label, status, String::new(), String::new()));
                }
            }
        }
    }
    let line_comments = pulls::line_comments(&client, &repo, pull.number).unwrap_or_default();
    let conversation = pulls::conversation(&client, &repo, pull.number).unwrap_or_default();
    let mut seen: std::collections::BTreeMap<String, u64> = store.setting(CONVERSATION_SEEN_KEY);
    let key = watch_key(pull);
    let conversation_after = seen.get(&key).copied().unwrap_or(0);
    for said in post::read_replies(
        &findings,
        &line_comments,
        &conversation,
        conversation_after,
        &me,
    ) {
        let Some(finding) = findings.iter_mut().find(|f| f.label == said.label) else {
            continue;
        };
        if said.place == "line" {
            finding.last_reply_id = Some(said.id);
        }
        match &said.answer {
            Some(answer) if finding.status != answer.status => {
                finding.status = answer.status.into();
                finding.status_source = "author".into();
                finding.status_by = said.who.clone();
                finding.reason = answer.reason.clone();
                finding.claim = String::new();
                finding.draft_reply = String::new();
                synced.changed.push((
                    said.label.clone(),
                    answer.status.into(),
                    said.who.clone(),
                    answer.reason.clone(),
                ));
            }
            Some(_) => {}
            None => synced.talked.push((said.label.clone(), said.who.clone())),
        }
        touched.push(finding.id);
    }
    if let Some(newest) = conversation.iter().map(|c| c.id).max()
        && newest > conversation_after
    {
        seen.insert(key, newest);
        store.set_setting(CONVERSATION_SEEN_KEY, &seen)?;
    }
    touched.sort_unstable();
    touched.dedup();
    for finding in findings.iter().filter(|f| touched.contains(&f.id)) {
        store.update_finding(finding)?;
    }
    if !touched.is_empty() {
        store.set_pr_review_verdict(round.id, crate::severity::verdict(&findings))?;
    }
    Ok(synced)
}

/// Publishes the newest round: line comments (when `mode` is inline), the
/// summary comment (rewritten in place), threads of findings dealt with
/// resolved. Ticks and replies are read first, so none is overwritten.
pub fn publish(store: &Store, pull: &PullRef, mode: PostMode) -> Result<String> {
    sync(store, pull)?;
    let Some((round, findings)) = current(store, pull)? else {
        bail!("Nothing to post: review the pull request first.");
    };
    let (client, repo, me) = client_for(store, pull)?;
    let files = pulls::files(&client, &repo, pull.number)?;
    let target = post::Target {
        client: &client,
        repo: &repo,
        number: pull.number,
        head: &round.head_sha,
    };
    let repo_url = format!("https://{}/{}/{}", pull.host, pull.owner, pull.repo);
    let agents: Vec<String> = {
        let mut agents: Vec<String> = findings
            .iter()
            .flat_map(|f| f.agents.split(", ").map(str::to_string))
            .filter(|a| !a.is_empty())
            .collect();
        agents.sort();
        agents.dedup();
        agents
    };
    let agents = agents.join(", ");
    let summary = |findings: &[Finding]| {
        crate::comment::render(&crate::comment::Summary {
            round: round.round,
            sha: &round.head_sha,
            verdict: crate::severity::verdict(findings),
            summary: &round.summary,
            reviewer: &me,
            agents: &agents,
            repo_url: &repo_url,
            number: pull.number,
            findings,
        })
    };
    let existing = store.summary_comment_of(pull)?;
    let published = post::publish(&target, &findings, &files, mode, &summary, existing)?;
    store.set_summary_comment(round.id, published.summary_comment)?;
    for mut finding in findings {
        let mut changed = false;
        if let Some((_, id)) = published.on_code.iter().find(|(l, _)| *l == finding.label) {
            finding.comment_id = Some(*id);
            finding.comment_kind = Some("line".into());
            changed = true;
        }
        if published.resolved.contains(&finding.label) {
            finding.fixed_notified = true;
            changed = true;
        }
        if changed {
            store.update_finding(&finding)?;
        }
    }
    let mut said = format!("Posted the review of #{}", pull.number);
    if !published.on_code.is_empty() {
        said.push_str(&format!(", {} on the code", published.on_code.len()));
    }
    if !published.resolved.is_empty() {
        said.push_str(&format!(", {} threads resolved", published.resolved.len()));
    }
    if !published.off_code.is_empty() {
        said.push_str(&format!(
            " ({} not on changed lines: in the summary)",
            published.off_code.len()
        ));
    }
    Ok(said)
}

/// Sets finding `id`'s status by the user (with why), as the reviewer.
pub fn set_status(store: &Store, id: i64, status: &str, reason: &str) -> Result<()> {
    let mut finding = store.pr_finding(id)?.context("finding not found")?;
    finding.status = status.into();
    finding.status_source = "me".into();
    finding.status_by = String::new();
    finding.reason = reason.trim().into();
    finding.claim = String::new();
    finding.draft_reply = String::new();
    store.update_finding(&finding)?;
    let review = store
        .pr_review(finding.review_id)?
        .context("review not found")?;
    let findings = store.pr_findings(review.id)?;
    store.set_pr_review_verdict(review.id, crate::severity::verdict(&findings))
}

/// Replies `text` on finding `finding` on GitHub (its thread, or the
/// conversation). Used by the reviewer and by the author's PilotHouse.
pub fn reply(store: &Store, pull: &PullRef, finding: &Finding, text: &str) -> Result<()> {
    let (client, repo, _) = client_for(store, pull)?;
    let target = post::Target {
        client: &client,
        repo: &repo,
        number: pull.number,
        head: "",
    };
    post::reply_to(&target, finding, text)?;
    // The drafted reply is posted. (Our own replies are never read back as
    // answers: only others' are.)
    if finding.id != 0
        && let Ok(Some(mut stored)) = store.pr_finding(finding.id)
        && !stored.draft_reply.is_empty()
    {
        stored.draft_reply = String::new();
        store.update_finding(&stored)?;
    }
    Ok(())
}

/// The review the pull request's summary comment carries (for the author's
/// PilotHouse, or a reviewer on another computer), with the comment's id.
pub fn shared(
    store: &Store,
    pull: &PullRef,
) -> Result<Option<(u64, crate::comment::SharedReview)>> {
    let (client, repo, _) = client_for(store, pull)?;
    let conversation = pulls::conversation(&client, &repo, pull.number)?;
    Ok(conversation
        .iter()
        .rev()
        .find_map(|comment| crate::comment::read_shared(&comment.body).map(|s| (comment.id, s))))
}

/// Opens an issue for finding `finding_id` (labelled `pilothouse`).
pub fn create_issue(store: &Store, finding_id: i64, title: &str, body: &str) -> Result<u64> {
    let mut finding = store.pr_finding(finding_id)?.context("finding not found")?;
    let review = store
        .pr_review(finding.review_id)?
        .context("review not found")?;
    let (client, repo, _) = client_for(store, &review.pull)?;
    let issue = pulls::create_issue(&client, &repo, title, body, &["pilothouse"])?;
    finding.issue_number = Some(issue.number);
    store.update_finding(&finding)?;
    Ok(issue.number)
}

/// The draft issue for a finding: its place at the reviewed commit, the
/// code, what is wrong and the fix.
pub fn issue_draft(finding: &Finding, review: &PrReview) -> (String, String) {
    let pull = &review.pull;
    let link = finding.line.map_or_else(
        || {
            format!(
                "https://{}/{}/{}/blob/{}/{}",
                pull.host, pull.owner, pull.repo, review.head_sha, finding.path
            )
        },
        |line| {
            format!(
                "https://{}/{}/{}/blob/{}/{}#L{line}{}",
                pull.host,
                pull.owner,
                pull.repo,
                review.head_sha,
                finding.path,
                finding
                    .end_line
                    .filter(|end| *end > line)
                    .map(|end| format!("-L{end}"))
                    .unwrap_or_default()
            )
        },
    );
    let mut body = format!(
        "From the review of #{} ({} · {}, {}).\n\n{link}\n",
        pull.number,
        finding.label,
        finding.severity,
        crate::comment::status_label(&finding.status).to_lowercase()
    );
    if !finding.evidence.trim().is_empty() {
        body.push_str(&format!("\n```\n{}\n```\n", finding.evidence.trim()));
    }
    if !finding.body.trim().is_empty() {
        body.push_str(&format!("\n**What's wrong:** {}\n", finding.body.trim()));
    }
    let fix = crate::comment::fix_markdown(finding, "");
    if !fix.is_empty() {
        body.push_str(&format!("\n{fix}\n"));
    }
    if !finding.reason.trim().is_empty() {
        body.push_str(&format!(
            "\n**Why not fixed in the PR:** {}\n",
            finding.reason.trim()
        ));
    }
    (finding.title.clone(), body)
}

/// Whether the repository is public (a security issue there discloses it).
pub fn is_public(store: &Store, pull: &PullRef) -> Result<bool> {
    let (client, repo, _) = client_for(store, pull)?;
    Ok(!pulls::repository(&client, &repo)?.private)
}

/// Opens a pull request that adds `rule` to the repository's shared review
/// rules (`.pilothouse/review.md`) on its default branch; its link.
pub fn propose_rule(store: &Store, pull: &PullRef, rule: &str) -> Result<String> {
    let (client, repo, _) = client_for(store, pull)?;
    let base = pulls::repository(&client, &repo)?.default_branch;
    let path = crate::templates::TEAM_RULES;
    let (text, sha) = match pulls::file_at(&client, &repo, path, &base)? {
        Some((text, sha)) => (text, Some(sha)),
        None => (
            "# Review rules\n\nRead by PilotHouse's reviews.\n".to_string(),
            None,
        ),
    };
    let mut text = text.trim_end().to_string();
    if !text.contains("## Not problems here") {
        text.push_str("\n\n## Not problems here\n");
    }
    text.push_str(&format!("\n- {}\n", rule.trim()));
    let branch = format!("pilothouse/review-rule-{}", ph_store::now_millis() / 1000);
    let head = pulls::branch_head(&client, &repo, &base)?;
    pulls::create_branch(&client, &repo, &branch, &head)?;
    pulls::put_file(
        &client,
        &repo,
        path,
        &branch,
        "Review rules: a finding that is not a problem here",
        &text,
        sha.as_deref(),
    )?;
    let created = pulls::create(
        &client,
        &repo,
        &branch,
        &base,
        "Review rules: a finding that is not a problem here",
        &format!(
            "Adds to `{path}`, which PilotHouse's reviews read:\n\n> {}\n",
            rule.trim()
        ),
        false,
    )?;
    Ok(created.html_url)
}

/// Comments by others on `pull` still to deal with: line comments in
/// threads not resolved (each thread's comments), and conversation
/// comments; as (who, where, text), oldest first.
pub fn comments_to_address(store: &Store, pull: &PullRef) -> Result<Vec<(String, String, String)>> {
    let (client, repo, me) = client_for(store, pull)?;
    let threads = pulls::threads(&client, &repo, pull.number).unwrap_or_default();
    let line_comments = pulls::line_comments(&client, &repo, pull.number)?;
    let resolved: Vec<u64> = threads
        .iter()
        .filter(|thread| thread.resolved)
        .map(|thread| thread.first_comment)
        .collect();
    let mut comments = Vec::new();
    for comment in &line_comments {
        let root = comment.in_reply_to_id.unwrap_or(comment.id);
        if comment.user.login == me || resolved.contains(&root) {
            continue;
        }
        let line = comment.line.or(comment.original_line).unwrap_or(0);
        comments.push((
            comment.created_at.clone(),
            comment.user.login.clone(),
            format!("{}:{line}", comment.path),
            comment.body.clone(),
        ));
    }
    for comment in pulls::conversation(&client, &repo, pull.number)? {
        if comment.user.login != me {
            comments.push((
                comment.created_at.clone(),
                comment.user.login.clone(),
                "the conversation".into(),
                comment.body.clone(),
            ));
        }
    }
    comments.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(comments
        .into_iter()
        .map(|(_, who, place, text)| (who, place, text))
        .collect())
}

/// How reviews of `pull`'s repository went (each pull request's newest
/// round).
pub fn repo_stats(store: &Store, pull: &PullRef) -> Result<crate::stats::Stats> {
    let rounds = store.latest_rounds_of_repo(&pull.host, &pull.owner, &pull.repo)?;
    let mut pulls = Vec::new();
    for round in rounds {
        pulls.push((round.round, store.pr_findings(round.id)?));
    }
    Ok(crate::stats::of(&pulls))
}

/// Settings key: per pull request, the newest conversation comment read.
const CONVERSATION_SEEN_KEY: &str = "review.conversation_seen";

/// Settings key: the pull requests watched (`host/owner/name#number`).
const WATCHED_KEY: &str = "review.watched";
/// Settings key: each watched pull request's head when last looked at.
const SEEN_KEY: &str = "review.watch.seen";

fn watch_key(pull: &PullRef) -> String {
    format!("{}/{}/{}#{}", pull.host, pull.owner, pull.repo, pull.number)
}

/// The pull requests watched.
pub fn watched(store: &Store) -> Vec<PullRef> {
    let keys: Vec<String> = store.setting(WATCHED_KEY);
    keys.iter()
        .filter_map(|key| {
            let (repo, number) = key.rsplit_once('#')?;
            let mut parts = repo.splitn(3, '/');
            Some(PullRef {
                host: parts.next()?.into(),
                owner: parts.next()?.into(),
                repo: parts.next()?.into(),
                number: number.parse().ok()?,
            })
        })
        .collect()
}

pub fn is_watched(store: &Store, pull: &PullRef) -> bool {
    let keys: Vec<String> = store.setting(WATCHED_KEY);
    keys.contains(&watch_key(pull))
}

/// Watches `pull` (or stops): new commits and replies are announced.
pub fn set_watched(store: &Store, pull: &PullRef, on: bool) -> Result<()> {
    let key = watch_key(pull);
    let mut keys: Vec<String> = store.setting(WATCHED_KEY);
    keys.retain(|known| *known != key);
    if on {
        keys.push(key);
    }
    store.set_setting(WATCHED_KEY, &keys)
}

/// What changed on a watched pull request since last looked at.
#[derive(Debug, Default, PartialEq)]
pub struct News {
    /// The author pushed: the new head.
    pub pushed: Option<String>,
    /// The pull request was merged or closed.
    pub closed: bool,
    /// Statuses changed by ticks and replies: (label, status, who).
    pub answers: Vec<(String, String, String)>,
    /// Replies that claim nothing: (label, who).
    pub talk: Vec<(String, String)>,
}

/// Looks at watched pull request `pull`: its head (against the last look),
/// and, when it has a posted review, the replies (applied, as [`sync`]).
pub fn check_watched(store: &Store, pull: &PullRef) -> Result<News> {
    let (client, repo, _) = client_for(store, pull)?;
    let now = pulls::get(&client, &repo, pull.number)?;
    let mut seen: std::collections::BTreeMap<String, String> = store.setting(SEEN_KEY);
    let key = watch_key(pull);
    let mut news = News {
        closed: now.state != "open" || now.merged,
        ..News::default()
    };
    match seen.get(&key) {
        Some(head) if *head != now.head.sha => news.pushed = Some(now.head.sha.clone()),
        _ => {}
    }
    seen.insert(key, now.head.sha.clone());
    store.set_setting(SEEN_KEY, &seen)?;
    if store.summary_comment_of(pull)?.is_some() {
        let synced = sync(store, pull)?;
        news.answers = synced
            .changed
            .into_iter()
            .map(|(label, status, who, _)| (label, status, who))
            .collect();
        news.talk = synced.talked;
    }
    Ok(news)
}
