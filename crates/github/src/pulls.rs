//! Pull requests: list, read (files, diff, commits, conversation, reviews,
//! checks) and act (comment, reply, review, merge, create).

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::client::Client;
use crate::models::*;

/// `owner/name` on a GitHub host, checked so it is safe in a URL path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    pub fn new(owner: &str, name: &str) -> Result<Self> {
        let safe = |part: &str| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        if !safe(owner) || !safe(name) {
            bail!("`{owner}/{name}` is not a GitHub repository name");
        }
        Ok(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    pub(crate) fn path(&self, rest: &str) -> String {
        format!("repos/{}/{}/{rest}", self.owner, self.name)
    }
}

/// How a review ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewEvent {
    Approve,
    RequestChanges,
    Comment,
}

impl ReviewEvent {
    fn api(self) -> &'static str {
        match self {
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
            ReviewEvent::Comment => "COMMENT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    fn api(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }
}

pub fn me(client: &Client) -> Result<User> {
    client.get("user")
}

/// The repository itself: whether it exists for this token, its default
/// branch and where to clone it from.
pub fn repository(client: &Client, repo: &Repo) -> Result<Repository> {
    client.get(&format!("repos/{}/{}", repo.owner, repo.name))
}

/// Open pull requests, most recently updated first.
pub fn list(client: &Client, repo: &Repo) -> Result<Vec<PullRequest>> {
    client.get_all(&repo.path("pulls?state=open&sort=updated&direction=desc"))
}

pub fn get(client: &Client, repo: &Repo, number: u64) -> Result<PullRequest> {
    client.get(&repo.path(&format!("pulls/{number}")))
}

pub fn files(client: &Client, repo: &Repo, number: u64) -> Result<Vec<PullFile>> {
    client.get_all(&repo.path(&format!("pulls/{number}/files")))
}

/// The whole change as one unified diff.
pub fn diff(client: &Client, repo: &Repo, number: u64) -> Result<String> {
    client.get_text(
        &repo.path(&format!("pulls/{number}")),
        "application/vnd.github.diff",
    )
}

pub fn commits(client: &Client, repo: &Repo, number: u64) -> Result<Vec<Commit>> {
    client.get_all(&repo.path(&format!("pulls/{number}/commits")))
}

pub fn conversation(client: &Client, repo: &Repo, number: u64) -> Result<Vec<IssueComment>> {
    client.get_all(&repo.path(&format!("issues/{number}/comments")))
}

pub fn line_comments(client: &Client, repo: &Repo, number: u64) -> Result<Vec<ReviewComment>> {
    client.get_all(&repo.path(&format!("pulls/{number}/comments")))
}

/// The line comments that belong to one review.
pub fn review_comments(
    client: &Client,
    repo: &Repo,
    number: u64,
    review_id: u64,
) -> Result<Vec<ReviewComment>> {
    client.get_all(&repo.path(&format!("pulls/{number}/reviews/{review_id}/comments")))
}

pub fn reviews(client: &Client, repo: &Repo, number: u64) -> Result<Vec<Review>> {
    client.get_all(&repo.path(&format!("pulls/{number}/reviews")))
}

/// Check runs and commit statuses for `sha`, as one list.
pub fn checks(client: &Client, repo: &Repo, sha: &str) -> Result<Vec<Check>> {
    if !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("not a commit id: {sha}");
    }
    // The two lists are asked for at once: one after the other, the checks
    // took two round trips after the pull request itself had answered.
    let (runs, statuses) = std::thread::scope(|scope| {
        let runs = scope.spawn(|| {
            client.get::<CheckRuns>(&repo.path(&format!("commits/{sha}/check-runs?per_page=100")))
        });
        let statuses = client.get::<CombinedStatus>(&repo.path(&format!("commits/{sha}/status")));
        (runs.join(), statuses)
    });
    let runs = runs.map_err(|_| anyhow::anyhow!("reading the checks stopped"))??;
    let statuses = statuses?;
    let mut checks: Vec<Check> = runs
        .check_runs
        .into_iter()
        .map(|run| Check {
            outcome: match (run.status.as_str(), run.conclusion) {
                ("completed", Some(conclusion)) => conclusion,
                _ => "pending".into(),
            },
            name: run.name,
            url: run.html_url,
            id: Some(run.id).filter(|id| *id > 0),
            summary: [run.output.title, run.output.summary]
                .into_iter()
                .flatten()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"),
            actions: run.app.slug == "github-actions",
        })
        .collect();
    checks.extend(statuses.statuses.into_iter().map(|status| Check {
        name: status.context,
        outcome: match status.state.as_str() {
            "error" => "failure".into(),
            other => other.into(),
        },
        url: status.target_url.unwrap_or_default(),
        ..Check::default()
    }));
    Ok(checks)
}

/// Adds a comment to the conversation.
pub fn comment(client: &Client, repo: &Repo, number: u64, body: &str) -> Result<IssueComment> {
    client.post(
        &repo.path(&format!("issues/{number}/comments")),
        &json!({ "body": body }),
    )
}

/// Replies in the thread of line comment `comment_id`.
pub fn reply(
    client: &Client,
    repo: &Repo,
    number: u64,
    comment_id: u64,
    body: &str,
) -> Result<ReviewComment> {
    client.post(
        &repo.path(&format!("pulls/{number}/comments/{comment_id}/replies")),
        &json!({ "body": body }),
    )
}

pub fn edit_comment(
    client: &Client,
    repo: &Repo,
    comment_id: u64,
    body: &str,
) -> Result<IssueComment> {
    client.patch(
        &repo.path(&format!("issues/comments/{comment_id}")),
        &json!({ "body": body }),
    )
}

/// Submits a review with line comments, on `commit` (the head it was made
/// against, so comments land on the right lines).
pub fn submit_review(
    client: &Client,
    repo: &Repo,
    number: u64,
    commit: &str,
    event: ReviewEvent,
    body: &str,
    comments: &[DraftComment],
) -> Result<Review> {
    let mut request = json!({
        "commit_id": commit,
        "event": event.api(),
        "comments": comments,
    });
    if !body.trim().is_empty() {
        request["body"] = Value::String(body.to_string());
    }
    client.post(&repo.path(&format!("pulls/{number}/reviews")), &request)
}

/// One conversation comment by id.
pub fn issue_comment(client: &Client, repo: &Repo, comment_id: u64) -> Result<IssueComment> {
    client.get(&repo.path(&format!("issues/comments/{comment_id}")))
}

/// A line comment of its own (outside a review), on `line` (or lines
/// `start_line..=line`) of the new version of `path` at `commit`.
pub fn line_comment(
    client: &Client,
    repo: &Repo,
    number: u64,
    commit: &str,
    draft: &DraftComment,
) -> Result<ReviewComment> {
    let mut request = json!({
        "body": draft.body,
        "commit_id": commit,
        "path": draft.path,
        "line": draft.line,
        "side": draft.side,
    });
    if let Some(start) = draft.start_line {
        request["start_line"] = json!(start);
        request["start_side"] = json!(draft.start_side.as_deref().unwrap_or("RIGHT"));
    }
    client.post(&repo.path(&format!("pulls/{number}/comments")), &request)
}

/// The pull request's line-comment threads (the first 100), to resolve
/// them by their first comment.
pub fn threads(client: &Client, repo: &Repo, number: u64) -> Result<Vec<Thread>> {
    let data = client.graphql(
        "query($owner: String!, $name: String!, $number: Int!) {
           repository(owner: $owner, name: $name) {
             pullRequest(number: $number) {
               reviewThreads(first: 100) {
                 nodes { id isResolved comments(first: 1) { nodes { databaseId } } }
               }
             }
           }
         }",
        json!({ "owner": repo.owner, "name": repo.name, "number": number }),
    )?;
    let nodes = data["repository"]["pullRequest"]["reviewThreads"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(nodes
        .iter()
        .filter_map(|node| {
            Some(Thread {
                id: node["id"].as_str()?.to_string(),
                resolved: node["isResolved"].as_bool().unwrap_or(false),
                first_comment: node["comments"]["nodes"][0]["databaseId"].as_u64()?,
            })
        })
        .collect())
}

/// Marks thread `id` (GraphQL) resolved.
pub fn resolve_thread(client: &Client, id: &str) -> Result<()> {
    client.graphql(
        "mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { thread { id } } }",
        json!({ "id": id }),
    )?;
    Ok(())
}

/// A file of branch `branch`: its text and blob sha; `None` when missing.
pub fn file_at(
    client: &Client,
    repo: &Repo,
    path: &str,
    branch: &str,
) -> Result<Option<(String, String)>> {
    use base64::Engine as _;
    let value: Value = match client.get(&repo.path(&format!("contents/{path}?ref={branch}"))) {
        Ok(value) => value,
        // Missing; anything else (no network, a bad token) is an error, not
        // a reason to write the file afresh.
        Err(err) if err.to_string().contains("(404)") => return Ok(None),
        Err(err) => return Err(err),
    };
    let sha = value["sha"].as_str().unwrap_or_default().to_string();
    let encoded: String = value["content"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    Ok(Some((String::from_utf8_lossy(&bytes).into_owned(), sha)))
}

/// Creates branch `name` at commit `sha`.
pub fn create_branch(client: &Client, repo: &Repo, name: &str, sha: &str) -> Result<()> {
    client.post::<Value>(
        &repo.path("git/refs"),
        &json!({ "ref": format!("refs/heads/{name}"), "sha": sha }),
    )?;
    Ok(())
}

/// Writes `text` to `path` on `branch` as a commit (`sha`: the file's blob
/// when it exists).
pub fn put_file(
    client: &Client,
    repo: &Repo,
    path: &str,
    branch: &str,
    message: &str,
    text: &str,
    sha: Option<&str>,
) -> Result<()> {
    use base64::Engine as _;
    let mut request = json!({
        "message": message,
        "content": base64::engine::general_purpose::STANDARD.encode(text),
        "branch": branch,
    });
    if let Some(sha) = sha {
        request["sha"] = json!(sha);
    }
    client.put::<Value>(&repo.path(&format!("contents/{path}")), &request)?;
    Ok(())
}

/// The commit branch `name` points at.
pub fn branch_head(client: &Client, repo: &Repo, name: &str) -> Result<String> {
    let value: Value = client.get(&repo.path(&format!("git/ref/heads/{name}")))?;
    value["object"]["sha"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("branch {name} not found"))
}

/// What a failed check says: the places it points at, and for a GitHub
/// Actions job the part of its log around the first error.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CheckDetail {
    pub annotations: Vec<Annotation>,
    pub log: String,
}

/// Reads [`CheckDetail`] of check run `check` (whatever can be read: a
/// check by another app has no log here).
pub fn check_detail(client: &Client, repo: &Repo, check: &Check) -> Result<CheckDetail> {
    let Some(id) = check.id else {
        bail!("{} reports only a link", check.name);
    };
    let annotations: Vec<Annotation> = client
        .get(&repo.path(&format!("check-runs/{id}/annotations?per_page=50")))
        .unwrap_or_default();
    let log = if check.actions {
        client
            .download_text(&repo.path(&format!("actions/jobs/{id}/logs")), LOG_LIMIT)
            .map(|log| log_excerpt(&log))
            .unwrap_or_default()
    } else {
        String::new()
    };
    Ok(CheckDetail {
        // The noise first out: a notice says nothing about the failure.
        annotations: annotations
            .into_iter()
            .filter(|note| note.annotation_level != "notice")
            .collect(),
        log,
    })
}

/// Most of a job's log read.
const LOG_LIMIT: u64 = 24 * 1024 * 1024;
/// Lines of a log shown.
const LOG_LINES: usize = 60;

/// The part of a job's log worth reading: around its first error (GitHub
/// Actions marks it `##[error]`), else its end. Timestamps are dropped.
pub fn log_excerpt(log: &str) -> String {
    let lines: Vec<&str> = log
        .lines()
        .map(|line| {
            // `2026-11-18T10:00:00.1234567Z text`
            match line.split_once(' ') {
                Some((stamp, rest))
                    if stamp.len() >= 20 && stamp.ends_with('Z') && stamp.contains('T') =>
                {
                    rest
                }
                _ => line,
            }
        })
        .collect();
    let first_error = lines.iter().position(|line| line.contains("##[error]"));
    let (start, end) = match first_error {
        Some(at) => (
            at.saturating_sub(LOG_LINES - 12),
            (at + 12).min(lines.len()),
        ),
        None => (lines.len().saturating_sub(LOG_LINES), lines.len()),
    };
    lines[start..end]
        .iter()
        // Folding marks say nothing here.
        .filter(|line| !line.starts_with("##[group]") && !line.starts_with("##[endgroup]"))
        .map(|line| line.replace("##[error]", "error: "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs check run `check` again: the job for GitHub Actions, else a
/// request to the app that made it.
pub fn rerun_check(client: &Client, repo: &Repo, check: &Check) -> Result<()> {
    let Some(id) = check.id else {
        bail!("{} cannot be run again from here", check.name);
    };
    if check.actions {
        return client.act("POST", &repo.path(&format!("actions/jobs/{id}/rerun")));
    }
    client.act("POST", &repo.path(&format!("check-runs/{id}/rerequest")))
}

/// What changed between commits `base` and `head` (three dots: since their
/// common ancestor), as a pull request's files are listed.
pub fn compare(client: &Client, repo: &Repo, base: &str, head: &str) -> Result<Vec<PullFile>> {
    let hex = |sha: &str| !sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit());
    if !hex(base) || !hex(head) {
        bail!("not commit ids: {base}, {head}");
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Compared {
        files: Vec<PullFile>,
    }
    let compared: Compared = client.get(&repo.path(&format!("compare/{base}...{head}")))?;
    Ok(compared.files)
}

/// Brings pull request `number`'s branch up to date with its base (a merge
/// of the base into it), if its head is still `sha`.
pub fn update_branch(client: &Client, repo: &Repo, number: u64, sha: &str) -> Result<()> {
    client
        .put::<Value>(
            &repo.path(&format!("pulls/{number}/update-branch")),
            &json!({ "expected_head_sha": sha }),
        )
        .map(|_| ())
}

/// Merges pull request `number` if its head is still `sha`, the commit the
/// person saw: one pushed since is never merged unseen.
pub fn merge(
    client: &Client,
    repo: &Repo,
    number: u64,
    sha: &str,
    method: MergeMethod,
) -> Result<Value> {
    client
        .put(
            &repo.path(&format!("pulls/{number}/merge")),
            &json!({ "merge_method": method.api(), "sha": sha }),
        )
        .map_err(|err| {
            if err.to_string().starts_with("GitHub returned 409") {
                anyhow::anyhow!(
                    "Not merged: commits were pushed to #{number} since this page was loaded. Look at them, then merge again."
                )
            } else {
                err
            }
        })
}

pub fn create(
    client: &Client,
    repo: &Repo,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<PullRequest> {
    client.post(
        &repo.path("pulls"),
        &json!({ "head": head, "base": base, "title": title, "body": body, "draft": draft }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockServer;

    #[test]
    fn repo_names_are_checked() {
        assert!(Repo::new("acme", "app.rs").is_ok());
        assert!(Repo::new("acme", "../etc").is_err());
        assert!(Repo::new("a b", "app").is_err());
        assert!(Repo::new("acme", "").is_err());
    }

    #[test]
    fn lists_reads_and_reviews_against_a_mock_server() {
        let server = MockServer::start(vec![
            (
                "GET",
                "/repos/acme/app/pulls?state=open",
                200,
                r#"[{"number":7,"title":"Fix login","user":{"login":"dev"},"head":{"ref":"fix","sha":"abc"},"base":{"ref":"main"},"labels":[{"name":"bug","color":"d73a4a"}]}]"#,
            ),
            (
                "GET",
                "/repos/acme/app/pulls/7/files",
                200,
                r#"[{"filename":"src/a.rs","status":"modified","additions":3,"deletions":1,"patch":"@@ -1 +1 @@\n-a\n+b"}]"#,
            ),
            (
                "GET",
                "/repos/acme/app/commits/abc/check-runs",
                200,
                r#"{"check_runs":[{"name":"build","status":"completed","conclusion":"success","html_url":"u"},{"name":"test","status":"in_progress"}]}"#,
            ),
            (
                "GET",
                "/repos/acme/app/commits/abc/status",
                200,
                r#"{"statuses":[{"context":"ci/lint","state":"error"}]}"#,
            ),
            (
                "POST",
                "/repos/acme/app/pulls/7/reviews",
                200,
                r#"{"id":99,"state":"COMMENTED","user":{"login":"me"}}"#,
            ),
            (
                "GET",
                "/repos/acme/app/pulls/8",
                404,
                r#"{"message":"Not Found"}"#,
            ),
        ]);
        let client = Client::new(&server.base, "test-token");
        let repo = Repo::new("acme", "app").unwrap();

        let pulls = list(&client, &repo).unwrap();
        assert_eq!(pulls[0].number, 7);
        assert_eq!(pulls[0].labels[0].name, "bug");
        let changed = files(&client, &repo, 7).unwrap();
        assert_eq!(changed[0].additions, 3);
        let ci = checks(&client, &repo, "abc").unwrap();
        let outcomes: Vec<_> = ci.iter().map(|c| c.outcome.as_str()).collect();
        assert_eq!(outcomes, ["success", "pending", "failure"]);

        let comments = [DraftComment {
            path: "src/a.rs".into(),
            line: 1,
            side: "RIGHT".into(),
            body: "Nit".into(),
            start_line: None,
            start_side: None,
        }];
        let review = submit_review(
            &client,
            &repo,
            7,
            "abc",
            ReviewEvent::Comment,
            "Looks fine",
            &comments,
        )
        .unwrap();
        assert_eq!(review.id, 99);
        let sent = server.requests();
        let post = sent.iter().find(|r| r.method == "POST").unwrap();
        assert_eq!(post.path, "/repos/acme/app/pulls/7/reviews");
        assert!(
            post.headers
                .iter()
                .any(|(k, v)| k == "authorization" && v == "Bearer test-token")
        );
        let body: Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(body["event"], "COMMENT");
        assert_eq!(body["comments"][0]["line"], 1);

        let missing = get(&client, &repo, 8).unwrap_err().to_string();
        assert!(missing.contains("404"), "{missing}");
    }
}
