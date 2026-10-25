//! Pull requests across every repository the account sees, as GitHub's
//! search finds them: those waiting for the user's review, the user's own,
//! and those assigned to the user. The GitHub view's inbox lists.

use anyhow::Result;
use serde::Deserialize;

use crate::client::Client;
use crate::models::{Label, User};

/// Most pull requests one list shows.
const PER_LIST: usize = 50;

/// Which pull requests a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Inbox {
    /// Open ones the user is asked to review.
    ReviewRequested,
    /// Open ones the user opened.
    Mine,
    /// Open ones assigned to the user.
    Assigned,
}

impl Inbox {
    pub const ALL: [Inbox; 3] = [Inbox::ReviewRequested, Inbox::Mine, Inbox::Assigned];

    fn query(self) -> &'static str {
        match self {
            Inbox::ReviewRequested => "is:pr is:open archived:false review-requested:@me",
            Inbox::Mine => "is:pr is:open archived:false author:@me",
            Inbox::Assigned => "is:pr is:open archived:false assignee:@me",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Inbox::ReviewRequested => "Review requested",
            Inbox::Mine => "My pull requests",
            Inbox::Assigned => "Assigned to me",
        }
    }
}

/// A pull request as search lists it (no branches: open it for those).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct FoundPull {
    pub number: u64,
    pub title: String,
    pub user: User,
    pub html_url: String,
    pub updated_at: String,
    pub draft: bool,
    pub labels: Vec<Label>,
    /// `https://API/repos/OWNER/NAME`.
    pub repository_url: String,
}

impl FoundPull {
    /// The repository's `(owner, name)`.
    pub fn repo(&self) -> Option<(String, String)> {
        let mut parts = self.repository_url.trim_end_matches('/').rsplit('/');
        let name = parts.next()?;
        let owner = parts.next()?;
        (parts.next() == Some("repos")).then(|| (owner.to_string(), name.to_string()))
    }
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    items: Vec<FoundPull>,
}

/// The open pull requests in `inbox`, most recently updated first.
pub fn pulls(client: &Client, inbox: Inbox) -> Result<Vec<FoundPull>> {
    let reply: Reply = client.get(&format!(
        "search/issues?q={}&sort=updated&order=desc&per_page={PER_LIST}",
        encode(inbox.query())
    ))?;
    Ok(reply.items)
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
    fn inbox_lists_come_from_search() {
        let server = MockServer::start(vec![(
            "GET",
            "/search/issues?q=is%3Apr+is%3Aopen+archived%3Afalse+review-requested%3A%40me",
            200,
            r#"{"total_count":1,"items":[{"number":12,"title":"Add tunnels","user":{"login":"dev"},"repository_url":"https://api.github.com/repos/acme/app","updated_at":"2026-10-24T10:00:00Z"}]}"#,
        )]);
        let client = Client::new(&server.base, "test-token");
        let found = pulls(&client, Inbox::ReviewRequested).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].number, 12);
        assert_eq!(found[0].repo(), Some(("acme".into(), "app".into())));
        // Another list asks with its own query (none here: an error).
        assert!(pulls(&client, Inbox::Mine).is_err());
    }

    #[test]
    fn repository_urls_name_the_repository() {
        let found = |url: &str| FoundPull {
            repository_url: url.into(),
            ..Default::default()
        };
        assert_eq!(
            found("https://ghe.example.net/api/v3/repos/team/svc").repo(),
            Some(("team".into(), "svc".into()))
        );
        assert_eq!(found("https://api.github.com/acme/app").repo(), None);
    }
}
