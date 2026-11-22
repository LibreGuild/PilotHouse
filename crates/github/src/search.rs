//! Pull requests across every repository the account sees, as GitHub's
//! search finds them: those waiting for the user's review, those the user
//! reviewed and that wait for their author, and the user's own. The Reviews
//! view's lists.
//!
//! One GraphQL search per list brings what the rows show (size, checks,
//! reviews, who was asked), so nothing is asked per pull request. A server
//! that does not answer it (an old GitHub Enterprise) is asked the plain
//! search instead, and the rows show less.

use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::Client;
use crate::models::{Label, User};

/// Most pull requests one list shows.
const PER_LIST: usize = 50;

/// Which pull requests a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Inbox {
    /// Open ones the user (or a team of theirs) is asked to review.
    NeedsReview,
    /// Open ones the user reviewed: it is the author's turn, or the author
    /// answered.
    Waiting,
    /// Open ones the user opened.
    Mine,
}

impl Inbox {
    pub const ALL: [Inbox; 3] = [Inbox::NeedsReview, Inbox::Waiting, Inbox::Mine];

    fn query(self) -> &'static str {
        match self {
            Inbox::NeedsReview => "is:pr is:open archived:false review-requested:@me",
            // Asked again after a review: it is under Needs my review.
            Inbox::Waiting => {
                "is:pr is:open archived:false reviewed-by:@me -author:@me -review-requested:@me"
            }
            Inbox::Mine => "is:pr is:open archived:false author:@me",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Inbox::NeedsReview => "Needs my review",
            Inbox::Waiting => "Waiting for author",
            Inbox::Mine => "My pull requests",
        }
    }
}

/// What the reviewers decided, as GitHub sums it up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Decision {
    Approved,
    ChangesRequested,
    /// A review is required and missing.
    Required,
    /// Nothing decided (and nothing required).
    #[default]
    None,
}

/// The checks of the head commit, summed up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Checks {
    Passing,
    Failing,
    Running,
    #[default]
    None,
}

/// The account's own newest review of a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MyReview {
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED`.
    pub state: String,
    /// The commit it was made on.
    pub commit: String,
}

/// What the rows show beyond a title (absent from the plain search).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Detail {
    pub additions: u64,
    pub deletions: u64,
    pub head_sha: String,
    /// `None` while GitHub works it out.
    pub mergeable: Option<bool>,
    pub decision: Decision,
    pub checks: Checks,
    /// The account itself was asked to review (not only a team of theirs).
    pub asked_directly: bool,
    /// The teams asked to review.
    pub teams: Vec<String>,
    pub my_review: Option<MyReview>,
    /// The files changed (the first hundred).
    pub paths: Vec<String>,
}

/// A pull request as a list shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FoundPull {
    pub number: u64,
    pub title: String,
    pub user: User,
    pub html_url: String,
    pub updated_at: String,
    pub draft: bool,
    pub labels: Vec<Label>,
    pub owner: String,
    pub name: String,
    pub detail: Option<Detail>,
}

impl FoundPull {
    /// The repository's `(owner, name)`.
    pub fn repo(&self) -> Option<(String, String)> {
        (!self.owner.is_empty() && !self.name.is_empty())
            .then(|| (self.owner.clone(), self.name.clone()))
    }
}

/// A pull request as the plain search lists it.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Plain {
    number: u64,
    title: String,
    user: User,
    html_url: String,
    updated_at: String,
    draft: bool,
    labels: Vec<Label>,
    /// `https://API/repos/OWNER/NAME`.
    repository_url: String,
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    items: Vec<Plain>,
}

/// `(owner, name)` of `https://API/repos/OWNER/NAME`.
fn repo_of_url(url: &str) -> Option<(String, String)> {
    let mut parts = url.trim_end_matches('/').rsplit('/');
    let name = parts.next()?;
    let owner = parts.next()?;
    (parts.next() == Some("repos")).then(|| (owner.to_string(), name.to_string()))
}

const QUERY: &str = "query($q: String!, $n: Int!) {
  search(query: $q, type: ISSUE, first: $n) {
    nodes {
      ... on PullRequest {
        number title url updatedAt isDraft
        author { login __typename avatarUrl }
        repository { nameWithOwner }
        additions deletions headRefOid mergeable reviewDecision
        labels(first: 10) { nodes { name color } }
        commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
        reviewRequests(first: 20) { nodes { requestedReviewer { __typename ... on User { login } ... on Team { name } } } }
        latestReviews(first: 30) { nodes { author { login } state commit { oid } } }
        files(first: 100) { nodes { path } }
      }
    }
  }
}";

/// The open pull requests in `inbox`, most recently updated first (`me`:
/// the account's login).
pub fn pulls(client: &Client, inbox: Inbox, me: &str) -> Result<Vec<FoundPull>> {
    let query = format!("{} sort:updated-desc", inbox.query());
    match client.graphql(QUERY, json!({ "q": query, "n": PER_LIST })) {
        Ok(data) => Ok(data["search"]["nodes"]
            .as_array()
            .map(|nodes| nodes.iter().filter_map(|node| detailed(node, me)).collect())
            .unwrap_or_default()),
        Err(err) => {
            tracing::info!("the detailed search failed ({err:#}); asking the plain one");
            plain(client, inbox)
        }
    }
}

fn plain(client: &Client, inbox: Inbox) -> Result<Vec<FoundPull>> {
    let reply: Reply = client.get(&format!(
        "search/issues?q={}&sort=updated&order=desc&per_page={PER_LIST}",
        encode(inbox.query())
    ))?;
    Ok(reply
        .items
        .into_iter()
        .map(|pull| {
            let (owner, name) = repo_of_url(&pull.repository_url).unwrap_or_default();
            FoundPull {
                number: pull.number,
                title: pull.title,
                user: pull.user,
                html_url: pull.html_url,
                updated_at: pull.updated_at,
                draft: pull.draft,
                labels: pull.labels,
                owner,
                name,
                detail: None,
            }
        })
        .collect())
}

/// One search result (a pull request node) with its detail.
fn detailed(node: &Value, me: &str) -> Option<FoundPull> {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let (owner, name) = node["repository"]["nameWithOwner"]
        .as_str()?
        .split_once('/')?;
    let nodes = |value: &Value| value["nodes"].as_array().cloned().unwrap_or_default();
    let mut asked_directly = false;
    let mut teams = Vec::new();
    for request in nodes(&node["reviewRequests"]) {
        let reviewer = &request["requestedReviewer"];
        match reviewer["__typename"].as_str() {
            Some("User") => {
                asked_directly |= reviewer["login"]
                    .as_str()
                    .is_some_and(|login| login.eq_ignore_ascii_case(me))
            }
            Some("Team") => teams.push(text(&reviewer["name"])),
            _ => {}
        }
    }
    let my_review = nodes(&node["latestReviews"])
        .iter()
        .find(|review| {
            review["author"]["login"]
                .as_str()
                .is_some_and(|login| login.eq_ignore_ascii_case(me))
        })
        .map(|review| MyReview {
            state: text(&review["state"]),
            commit: text(&review["commit"]["oid"]),
        });
    let rollup = node["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["state"]
        .as_str()
        .unwrap_or_default();
    Some(FoundPull {
        number: node["number"].as_u64()?,
        title: text(&node["title"]),
        user: User {
            login: text(&node["author"]["login"]),
            name: None,
            avatar_url: text(&node["author"]["avatarUrl"]),
            kind: text(&node["author"]["__typename"]),
        },
        html_url: text(&node["url"]),
        updated_at: text(&node["updatedAt"]),
        draft: node["isDraft"].as_bool().unwrap_or(false),
        labels: nodes(&node["labels"])
            .iter()
            .map(|label| Label {
                name: text(&label["name"]),
                color: text(&label["color"]),
            })
            .collect(),
        owner: owner.to_string(),
        name: name.to_string(),
        detail: Some(Detail {
            additions: node["additions"].as_u64().unwrap_or(0),
            deletions: node["deletions"].as_u64().unwrap_or(0),
            head_sha: text(&node["headRefOid"]),
            mergeable: match node["mergeable"].as_str() {
                Some("MERGEABLE") => Some(true),
                Some("CONFLICTING") => Some(false),
                _ => None,
            },
            decision: match node["reviewDecision"].as_str() {
                Some("APPROVED") => Decision::Approved,
                Some("CHANGES_REQUESTED") => Decision::ChangesRequested,
                Some("REVIEW_REQUIRED") => Decision::Required,
                _ => Decision::None,
            },
            checks: match rollup {
                "SUCCESS" => Checks::Passing,
                "FAILURE" | "ERROR" => Checks::Failing,
                "PENDING" | "EXPECTED" => Checks::Running,
                _ => Checks::None,
            },
            asked_directly,
            teams,
            my_review,
            paths: nodes(&node["files"])
                .iter()
                .map(|file| text(&file["path"]))
                .collect(),
        }),
    })
}

/// `text` as a URL query value.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockServer;

    #[test]
    fn queries_are_encoded() {
        assert_eq!(
            encode("is:pr review-requested:@me"),
            "is%3Apr+review-requested%3A%40me"
        );
    }

    #[test]
    fn a_list_comes_with_its_detail() {
        let server = MockServer::start(vec![(
            "POST",
            "/graphql",
            200,
            r#"{"data":{"search":{"nodes":[{},{"number":12,"title":"Add tunnels","url":"https://github.com/acme/app/pull/12",
              "updatedAt":"2026-10-24T10:00:00Z","isDraft":false,"author":{"login":"dependabot","__typename":"Bot"},
              "repository":{"nameWithOwner":"acme/app"},"additions":40,"deletions":2,"headRefOid":"abc","mergeable":"CONFLICTING",
              "reviewDecision":"CHANGES_REQUESTED","labels":{"nodes":[{"name":"bug","color":"d73a4a"}]},
              "commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"FAILURE"}}}]},
              "reviewRequests":{"nodes":[{"requestedReviewer":{"__typename":"Team","name":"Core"}},{"requestedReviewer":{"__typename":"User","login":"Me"}}]},
              "latestReviews":{"nodes":[{"author":{"login":"other"},"state":"APPROVED","commit":{"oid":"000"}},{"author":{"login":"me"},"state":"COMMENTED","commit":{"oid":"aaa"}}]},
              "files":{"nodes":[{"path":"src/auth.rs"}]}}]}}}"#,
        )]);
        let client = Client::new(&server.base, "test-token");
        let found = pulls(&client, Inbox::NeedsReview, "me").unwrap();
        // The empty node (an issue the search matched) is left out.
        assert_eq!(found.len(), 1);
        let pull = &found[0];
        assert_eq!(pull.repo(), Some(("acme".into(), "app".into())));
        assert!(pull.user.is_bot());
        let detail = pull.detail.as_ref().unwrap();
        assert_eq!(detail.checks, Checks::Failing);
        assert_eq!(detail.decision, Decision::ChangesRequested);
        assert_eq!(detail.mergeable, Some(false));
        assert!(detail.asked_directly);
        assert_eq!(detail.teams, ["Core"]);
        assert_eq!(detail.my_review.as_ref().unwrap().commit, "aaa");
        assert_eq!(detail.paths, ["src/auth.rs"]);
    }

    #[test]
    fn a_server_without_the_detailed_search_gets_the_plain_one() {
        let server = MockServer::start(vec![(
            "GET",
            "/search/issues?q=is%3Apr+is%3Aopen+archived%3Afalse+author%3A%40me",
            200,
            r#"{"total_count":1,"items":[{"number":12,"title":"Add tunnels","user":{"login":"dev"},"repository_url":"https://api.github.com/repos/acme/app","updated_at":"2026-10-24T10:00:00Z"}]}"#,
        )]);
        let client = Client::new(&server.base, "test-token");
        let found = pulls(&client, Inbox::Mine, "me").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].repo(), Some(("acme".into(), "app".into())));
        assert_eq!(found[0].detail, None);
        // Another list asks with its own query (none here: an error).
        assert!(pulls(&client, Inbox::Waiting, "me").is_err());
    }

    #[test]
    fn repository_urls_name_the_repository() {
        assert_eq!(
            repo_of_url("https://ghe.example.net/api/v3/repos/team/svc"),
            Some(("team".into(), "svc".into()))
        );
        assert_eq!(repo_of_url("https://api.github.com/acme/app"), None);
    }
}
