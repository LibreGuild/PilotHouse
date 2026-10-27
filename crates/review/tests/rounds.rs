//! Review rounds end to end, on a real git repository: a first review, a
//! push that moves the code, a check of the fixes that follows each finding
//! to its new lines, and a fix the check confirms.

use std::path::Path;
use std::process::Command;

use ph_store::Store;
use ph_store::reviews::PullRef;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, text: &str, message: &str) -> String {
    std::fs::write(dir.join("lib.rs"), text).unwrap();
    git(dir, &["add", "lib.rs"]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

fn reply(json: &str) -> String {
    format!("Summary for you.\n\n```json pilothouse-review\n{json}\n```\n")
}

#[test]
fn findings_follow_the_code_across_rounds() {
    let dir = std::env::temp_dir().join(format!("ph-rounds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    let first = commit(
        &dir,
        "fn a() {}\nfn total(items: &[u32]) -> u32 {\n    items.iter().sum::<u32>() / 0\n}\n",
        "first",
    );

    let store = Store::open_in_memory().unwrap();
    let project = store.add_project(&dir).unwrap();
    let pull = PullRef {
        host: "github.com".into(),
        owner: "o".into(),
        repo: "r".into(),
        number: 7,
    };

    // Round 1: one real finding, one quoting code that is not there.
    let round1 = store
        .create_pr_run(project.id, &pull, "Sum", &first, "full", "Claude")
        .unwrap();
    let outcome = ph_review::rounds::finish(
        &store,
        &round1,
        &reply(
            r#"{"summary": "Divides by zero.", "findings": [
                {"path": "lib.rs", "line": 3, "title": "Division by zero", "impact": "crash", "likelihood": "common",
                 "details": "Always panics.", "evidence": "    items.iter().sum::<u32>() / 0", "suggestion": "Drop the division."},
                {"path": "lib.rs", "line": 1, "title": "Made up", "impact": "maintain", "likelihood": "rare",
                 "details": "x", "evidence": "fn nowhere() {}", "suggestion": "y"}
            ], "checks": []}"#,
        ),
        &dir,
        "Claude",
    )
    .unwrap()
    .unwrap();
    assert_eq!((outcome.new, outcome.open, outcome.unverified), (2, 2, 1));
    let found = store.pr_findings(round1.id).unwrap();
    let crash = found
        .iter()
        .find(|f| f.title == "Division by zero")
        .unwrap();
    assert_eq!(
        (crash.label.as_str(), crash.severity.as_str(), crash.line),
        ("F1", "critical", Some(3))
    );
    assert!(crash.verified);
    assert!(
        !found
            .iter()
            .find(|f| f.title == "Made up")
            .unwrap()
            .verified
    );

    // The author pushes code above it: the finding moves with its line.
    let second = commit(
        &dir,
        "use std::fmt;\n\nfn a() {}\nfn total(items: &[u32]) -> u32 {\n    items.iter().sum::<u32>() / 0\n}\n",
        "imports",
    );
    let round2 = store
        .create_pr_run(project.id, &pull, "Sum", &second, "followup", "Claude")
        .unwrap();
    assert_eq!(round2.round, 2);
    ph_review::rounds::finish(
        &store,
        &round2,
        &reply(r#"{"summary": "Still there.", "findings": [], "checks": [{"label": "F1", "fixed": false, "note": "still divides by zero"}]}"#),
        &dir,
        "Claude",
    )
    .unwrap()
    .unwrap();
    let moved = store.pr_findings(round2.id).unwrap();
    let crash = moved.iter().find(|f| f.label == "F1").unwrap();
    assert_eq!((crash.status.as_str(), crash.line), ("open", Some(5)));

    // Fixed: the check says so, and the round's verdict follows.
    let third = commit(
        &dir,
        "use std::fmt;\n\nfn a() {}\nfn total(items: &[u32]) -> u32 {\n    items.iter().sum::<u32>()\n}\n",
        "fix",
    );
    let round3 = store
        .create_pr_run(project.id, &pull, "Sum", &third, "followup", "Codex")
        .unwrap();
    let outcome = ph_review::rounds::finish(
        &store,
        &round3,
        &reply(r#"{"summary": "Fixed.", "findings": [], "checks": [{"label": "F1", "fixed": true, "note": "the division is gone"}]}"#),
        &dir,
        "Codex",
    )
    .unwrap()
    .unwrap();
    assert_eq!(outcome.fixed, 1);
    let fixed = store.pr_findings(round3.id).unwrap();
    assert_eq!(
        fixed.iter().find(|f| f.label == "F1").unwrap().status,
        "fixed"
    );
    assert_eq!(
        store.latest_pr_review(&pull, None).unwrap().unwrap().id,
        round3.id
    );

    // A reply that is no review changes nothing.
    let round4 = store
        .create_pr_run(project.id, &pull, "Sum", &third, "followup", "Claude")
        .unwrap();
    assert!(
        ph_review::rounds::finish(&store, &round4, "Looks fine to me.", &dir, "Claude")
            .unwrap()
            .is_none()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
