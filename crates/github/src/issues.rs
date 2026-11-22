//! Issues: a repository's list, one in full, who can be assigned, and
//! opening one. GitHub lists pull requests as issues too; they are left out.

use anyhow::Result;
use serde_json::json;

use crate::client::Client;
use crate::models::{Issue, IssueComment, User};
use crate::pulls::Repo;

/// Most issues one list shows (the most recently updated).
pub const PER_LIST: usize = 100;

/// Which issues a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Filter {
    #[default]
    Open,
    /// Open ones assigned to the account.
    Mine,
    Closed,
}

impl Filter {
    pub const ALL: [Filter; 3] = [Filter::Open, Filter::Mine, Filter::Closed];

    pub fn label(self) -> &'static str {
        match self {
            Filter::Open => "Open",
            Filter::Mine => "Assigned to me",
            Filter::Closed => "Closed",
        }
    }
}

/// The repository's issues in `filter`, most recently updated first (`me`:
/// the account's login).
pub fn list(client: &Client, repo: &Repo, filter: Filter, me: &str) -> Result<Vec<Issue>> {
    let mut query = format!(
        "issues?state={}&sort=updated&direction=desc&per_page={PER_LIST}",
        if filter == Filter::Closed {
            "closed"
        } else {
            "open"
        }
    );
    if filter == Filter::Mine {
        // A login is letters, digits and hyphens: safe in a query as it is.
        let login: String = me
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        query.push_str(&format!("&assignee={login}"));
    }
    let issues: Vec<Issue> = client.get(&repo.path(&query))?;
    Ok(issues
        .into_iter()
        .filter(|issue| issue.pull_request.is_none())
        .collect())
}

pub fn get(client: &Client, repo: &Repo, number: u64) -> Result<Issue> {
    client.get(&repo.path(&format!("issues/{number}")))
}

pub fn comments(client: &Client, repo: &Repo, number: u64) -> Result<Vec<IssueComment>> {
    client.get_all(&repo.path(&format!("issues/{number}/comments")))
}

/// The people an issue of the repository can be assigned to (the first
/// hundred, as GitHub orders them).
pub fn assignable(client: &Client, repo: &Repo) -> Result<Vec<User>> {
    client.get(&repo.path("assignees?per_page=100"))
}

/// What a new issue is given beside its text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extras<'a> {
    /// Added when the repository has them.
    pub labels: &'a [&'a str],
    /// Logins; each must be assignable.
    pub assignees: &'a [String],
}

/// Opens an issue. A label GitHub refuses (the repository does not have
/// it) is left off; an assignee it refuses is an error, since who it goes
/// to was chosen.
pub fn create(
    client: &Client,
    repo: &Repo,
    title: &str,
    body: &str,
    extras: &Extras,
) -> Result<Issue> {
    let path = repo.path("issues");
    let request = |labels: bool| {
        let mut request = json!({ "title": title, "body": body });
        if labels && !extras.labels.is_empty() {
            request["labels"] = json!(extras.labels);
        }
        if !extras.assignees.is_empty() {
            request["assignees"] = json!(extras.assignees);
        }
        request
    };
    if !extras.labels.is_empty()
        && let Ok(issue) = client.post::<Issue>(&path, &request(true))
    {
        return Ok(issue);
    }
    client.post(&path, &request(false))
}

/// Sets who issue `number` is assigned to.
pub fn assign(client: &Client, repo: &Repo, number: u64, logins: &[String]) -> Result<Issue> {
    client.patch(
        &repo.path(&format!("issues/{number}")),
        &json!({ "assignees": logins }),
    )
}

/// The issues of the same repository a pull request's text says it closes
/// (`Fixes #12`, `closes #3`, `Resolves: #7`), in the order named.
pub fn linked(text: &str) -> Vec<u64> {
    const WORDS: [&str; 9] = [
        "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
    ];
    let mut found = Vec::new();
    let words: Vec<&str> = text.split_whitespace().collect();
    for pair in words.windows(2) {
        let word = pair[0]
            .trim_matches(|c: char| !c.is_ascii_alphabetic())
            .to_ascii_lowercase();
        if !WORDS.contains(&word.as_str()) {
            continue;
        }
        let Some(digits) = pair[1].strip_prefix('#') else {
            continue;
        };
        let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(number) = digits.parse::<u64>()
            && !found.contains(&number)
        {
            found.push(number);
        }
    }
    found
}

/// An issue as Markdown an agent can be given: its title, who opened it,
/// its text and its comments.
pub fn as_markdown(issue: &Issue, comments: &[IssueComment]) -> String {
    let mut text = format!(
        "# {} (#{})\n\nOpened by {}",
        issue.title.trim(),
        issue.number,
        issue.user.login
    );
    if !issue.labels.is_empty() {
        let labels: Vec<&str> = issue.labels.iter().map(|l| l.name.as_str()).collect();
        text.push_str(&format!(" · {}", labels.join(", ")));
    }
    text.push_str(&format!("\n{}\n", issue.html_url));
    match issue.body.as_deref().map(str::trim) {
        Some(body) if !body.is_empty() => text.push_str(&format!("\n{body}\n")),
        _ => text.push_str("\n(No description.)\n"),
    }
    for comment in comments {
        text.push_str(&format!(
            "\n---\n\n**{}** commented:\n\n{}\n",
            comment.user.login,
            comment.body.trim()
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockServer;
    use serde_json::Value;

    #[test]
    fn finds_the_issues_a_pull_request_closes() {
        assert_eq!(
            linked("Fixes #12 and closes #3.\nSee #99. Resolves: #12, resolved #7"),
            [12, 3, 7]
        );
        assert!(linked("fix the #1 problem of prefix#2").is_empty());
    }

    #[test]
    fn lists_issues_without_pull_requests_and_assigns() {
        let server = MockServer::start(vec![
            (
                "GET",
                "/repos/acme/app/issues?state=open",
                200,
                r#"[{"number":7,"title":"Crash on empty cart","state":"open","user":{"login":"maria"},"comments":2},
                    {"number":8,"title":"A pull request","pull_request":{"url":"x"}}]"#,
            ),
            (
                "POST",
                "/repos/acme/app/issues",
                201,
                r#"{"number":9,"title":"t","state":"open","assignees":[{"login":"chen"}]}"#,
            ),
        ]);
        let client = Client::new(&server.base, "test-token");
        let repo = Repo::new("acme", "app").unwrap();
        let issues = list(&client, &repo, Filter::Mine, "me-1").unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 7);
        assert!(server.requests()[0].path.ends_with("&assignee=me-1"));

        let made = create(
            &client,
            &repo,
            "t",
            "b",
            &Extras {
                labels: &["bug"],
                assignees: &["chen".to_string()],
            },
        )
        .unwrap();
        assert_eq!(made.assignees[0].login, "chen");
        let sent: Value = serde_json::from_str(&server.requests()[1].body).unwrap();
        assert_eq!(sent["assignees"][0], "chen");
        assert_eq!(sent["labels"][0], "bug");
    }

    #[test]
    fn an_issue_reads_as_markdown() {
        let issue = Issue {
            number: 7,
            title: "Crash on empty cart".into(),
            body: Some("Steps: open the cart.".into()),
            user: User {
                login: "maria".into(),
                ..User::default()
            },
            ..Issue::default()
        };
        let text = as_markdown(&issue, &[]);
        assert!(text.starts_with("# Crash on empty cart (#7)\n\nOpened by maria"));
        assert!(text.contains("Steps: open the cart."));
    }
}
