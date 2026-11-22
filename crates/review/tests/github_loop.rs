//! The review loop against a local stand-in for GitHub: findings posted on
//! the code (with a one-click suggestion where it fits) and in one summary
//! comment, a fixed finding's thread resolved, then the author's ticks and
//! replies read back.

use ph_github::models::{IssueComment, PullFile, ReviewComment, User};
use ph_github::testing::MockServer;
use ph_github::{Client, Repo};
use ph_review::comment::{self, Summary};
use ph_review::post::{self, Target};
use ph_store::reviews::Finding;

fn finding(label: &str, line: u64, status: &str) -> Finding {
    Finding {
        id: line as i64,
        label: label.into(),
        severity: "major".into(),
        impact: "wrong".into(),
        likelihood: "common".into(),
        path: "src/lib.rs".into(),
        line: Some(line),
        end_line: Some(line),
        title: format!("Problem {label}"),
        body: "It breaks.".into(),
        status: status.into(),
        verified: true,
        first_round: 1,
        ..Finding::default()
    }
}

#[test]
fn posts_resolves_and_reads_back() {
    let server = MockServer::start(vec![
        (
            "POST",
            "/repos/o/r/pulls/7/reviews",
            200,
            r#"{"id": 99, "state": "COMMENTED"}"#,
        ),
        (
            "GET",
            "/repos/o/r/pulls/7/reviews/99/comments",
            200,
            r#"[{"id": 501, "body": "**F1 · Major: Problem F1**\n<!-- ph:F1 -->", "path": "src/lib.rs", "line": 11}]"#,
        ),
        (
            "POST",
            "/repos/o/r/issues/7/comments",
            200,
            r#"{"id": 700, "body": "summary"}"#,
        ),
        (
            "POST",
            "/graphql",
            200,
            r#"{"data": {"repository": {"pullRequest": {"reviewThreads": {"nodes": [
                {"id": "T1", "isResolved": false, "comments": {"nodes": [{"databaseId": 400}]}}
            ]}}}, "resolveReviewThread": {"thread": {"id": "T1"}}}}"#,
        ),
    ]);
    let client = Client::new(&server.base, "test-token");
    let repo = Repo::new("o", "r").unwrap();
    let target = Target {
        client: &client,
        repo: &repo,
        number: 7,
        head: "abc123",
    };
    let mut on_diff = finding("F1", 11, "open");
    on_diff.suggested_code = "let fixed = true;".into();
    let off_diff = finding("F2", 90, "open");
    let mut done = finding("F3", 12, "fixed");
    done.comment_id = Some(400);
    done.comment_kind = Some("line".into());
    let findings = vec![on_diff, off_diff, done];
    let files = vec![PullFile {
        filename: "src/lib.rs".into(),
        patch: Some("@@ -1,2 +10,4 @@\n ctx\n+one\n+two\n ctx\n".into()),
        ..PullFile::default()
    }];
    let summary = |findings: &[Finding]| {
        comment::render(&Summary {
            round: 1,
            sha: "abc123",
            verdict: ph_review::severity::verdict(findings),
            summary: "Two problems.",
            reviewer: "me",
            agents: "Claude",
            repo_url: "https://github.com/o/r",
            number: 7,
            findings,
        })
    };
    let published = post::publish(
        &target,
        &findings,
        &files,
        &post::Plan::default(),
        &summary,
        None,
    )
    .unwrap();

    assert_eq!(published.on_code, [("F1".to_string(), 501)]);
    assert_eq!(published.off_code, ["F2"], "line 90 is not in the diff");
    assert_eq!(published.summary_comment, Some(700));
    assert_eq!(published.resolved, ["F3"]);

    let requests = server.requests();
    let review = requests
        .iter()
        .find(|r| r.method == "POST" && r.path.starts_with("/repos/o/r/pulls/7/reviews"))
        .unwrap();
    assert!(
        review
            .body
            .contains("```suggestion\\nlet fixed = true;\\n```"),
        "{}",
        review.body
    );
    assert!(!review.body.contains("Problem F2"));
    let posted_summary = requests
        .iter()
        .find(|r| r.path.starts_with("/repos/o/r/issues/7/comments"))
        .unwrap();
    assert!(posted_summary.body.contains("pilothouse-review"));
    assert!(
        requests
            .iter()
            .any(|r| r.path == "/graphql" && r.body.contains("resolveReviewThread"))
    );

    // The author ticks F2 as fixed and answers F1 on its line.
    let body = summary(&findings);
    let ticked = body.replacen(
        "- [ ] Fixed <!-- ph:F2:fixed -->",
        "- [x] Fixed <!-- ph:F2:fixed -->",
        1,
    );
    assert_ne!(ticked, body, "the summary has a box for F2");
    let ticks = post::new_ticks(&ticked, &findings);
    assert_eq!(ticks.get("F2").map(String::as_str), Some("fixed"));
    let shared = comment::read_shared(&ticked).unwrap();
    assert_eq!(
        shared.findings.len(),
        3,
        "the author's PilotHouse reads the findings back"
    );

    let mut posted = findings.clone();
    posted[0].comment_id = Some(501);
    posted[0].comment_kind = Some("line".into());
    let author = User {
        login: "dev".into(),
        ..User::default()
    };
    let replies = vec![ReviewComment {
        id: 510,
        in_reply_to_id: Some(501),
        user: author.clone(),
        body: "Not a problem: the caller checks it first.".into(),
        ..ReviewComment::default()
    }];
    let conversation = vec![IssueComment {
        id: 520,
        user: author,
        body: "F2: fixed in the last commit".into(),
        ..IssueComment::default()
    }];
    let said = post::read_replies(&posted, &replies, &conversation, 0, "me");
    let answers: Vec<(&str, &str)> = said
        .iter()
        .filter_map(|s| Some((s.label.as_str(), s.answer.as_ref()?.status)))
        .collect();
    assert_eq!(answers, [("F1", "not_problem"), ("F2", "fixed")]);
}

#[test]
fn a_finished_review_goes_out_once_with_what_was_picked() {
    let server = MockServer::start(vec![
        (
            "POST",
            "/repos/o/r/pulls/7/reviews",
            200,
            r#"{"id": 99, "state": "CHANGES_REQUESTED"}"#,
        ),
        (
            "GET",
            "/repos/o/r/pulls/7/reviews/99/comments",
            200,
            r#"[{"id": 501, "body": "**F1 · Major: Problem F1**\n<!-- ph:F1 -->", "path": "src/lib.rs", "line": 11},
                {"id": 502, "body": "Mine: rename this.", "path": "src/lib.rs", "line": 12}]"#,
        ),
        (
            "POST",
            "/repos/o/r/issues/7/comments",
            200,
            r#"{"id": 700, "body": "summary"}"#,
        ),
    ]);
    let client = Client::new(&server.base, "test-token");
    let repo = Repo::new("o", "r").unwrap();
    let target = Target {
        client: &client,
        repo: &repo,
        number: 7,
        head: "abc123",
    };
    // F1 is sent; F2 (also on the diff) is held back.
    let findings = vec![finding("F1", 11, "open"), finding("F2", 12, "open")];
    let files = vec![PullFile {
        filename: "src/lib.rs".into(),
        patch: Some("@@ -1,2 +10,4 @@\n ctx\n+one\n+two\n ctx\n".into()),
        ..PullFile::default()
    }];
    let own = [ph_github::models::DraftComment {
        path: "src/lib.rs".into(),
        line: 12,
        side: "RIGHT".into(),
        body: "Mine: rename this.".into(),
        start_line: None,
        start_side: None,
    }];
    let picked = ["F1".to_string()];
    let said: std::cell::RefCell<Vec<String>> = Default::default();
    let summary = |findings: &[Finding]| {
        *said.borrow_mut() = findings.iter().map(|f| f.label.clone()).collect();
        "summary".to_string()
    };
    let published = post::publish(
        &target,
        &findings,
        &files,
        &post::Plan {
            picked: Some(&picked),
            own: &own,
            event: ph_github::pulls::ReviewEvent::RequestChanges,
            ..post::Plan::default()
        },
        &summary,
        None,
    )
    .unwrap();
    assert_eq!(published.on_code, [("F1".to_string(), 501)]);
    assert_eq!(published.own, 1);
    assert!(published.off_code.is_empty());
    assert_eq!(*said.borrow(), ["F1"], "a finding held back is not told");

    let requests = server.requests();
    let reviews: Vec<_> = requests
        .iter()
        .filter(|r| r.method == "POST" && r.path.starts_with("/repos/o/r/pulls/7/reviews"))
        .collect();
    assert_eq!(reviews.len(), 1, "one review, so the author is told once");
    let sent: serde_json::Value = serde_json::from_str(&reviews[0].body).unwrap();
    assert_eq!(sent["event"], "REQUEST_CHANGES");
    assert_eq!(sent["comments"].as_array().unwrap().len(), 2);
    // GitHub wants words with a request for changes.
    assert!(sent["body"].as_str().unwrap().contains("2 comments"));
}
