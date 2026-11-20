//! Explanations end to end, on a real git repository: one written for work
//! not committed yet is found from the commit that carries that work, also
//! after history is rewritten; a teammate's later change takes its lines;
//! and one shared as a file is found the same way by whoever has the
//! repository.

use std::path::{Path, PathBuf};
use std::process::Command;

use ph_review::explanation::file::{self, Shared};
use ph_review::explanation::find::{self, Source};
use ph_store::Store;

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

fn repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ph-explanations-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("src/auth.rs"), "fn sign_in() {}\n").unwrap();
    std::fs::write(dir.join("src/mail.rs"), "fn send() {}\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "first"]);
    dir
}

/// Explains the work not committed in `dir` as an agent would have.
fn explain(store: &Store, dir: &Path, title: &str) -> i64 {
    let content = ph_git::content::pending(dir).unwrap();
    let id = store
        .create_explanation(
            &dir.to_string_lossy(),
            "changes",
            "Claude Code",
            &content,
            "[]",
        )
        .unwrap();
    store
        .finish_explanation(
            id,
            title,
            &format!("# {title}\n\n## In short\n\nIt works."),
            None,
        )
        .unwrap();
    id
}

#[test]
fn an_explanation_follows_its_content_through_history() {
    let dir = repo("history");
    let store = Store::open_in_memory().unwrap();

    // Work that is not committed is explained, then committed.
    std::fs::write(dir.join("src/auth.rs"), "fn sign_in() {}\nfn reset() {}\n").unwrap();
    std::fs::write(dir.join("src/reset.rs"), "fn link() {}\n").unwrap();
    let reset = explain(&store, &dir, "Reset emails");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "reset emails"]);
    let commit = git(&dir, &["rev-parse", "HEAD"]);

    let found = find::of_commit(&store, &dir, &commit, None).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title, "Reset emails");
    assert_eq!(
        found[0].source,
        Source::Local {
            id: reset,
            session: None
        }
    );
    assert!(find::changed_since(&dir, &found[0]).is_empty());
    // The first commit was never explained.
    let first = git(&dir, &["rev-list", "--max-parents=0", "HEAD"]);
    assert!(
        find::of_commit(&store, &dir, &first, None)
            .unwrap()
            .is_empty()
    );

    // History rewritten (the commit gets a new id): still its explanation.
    git(
        &dir,
        &["commit", "-q", "--amend", "-m", "reset emails, reworded"],
    );
    let amended = git(&dir, &["rev-parse", "HEAD"]);
    assert_ne!(amended, commit);
    assert_eq!(
        find::of_commit(&store, &dir, &amended, None).unwrap()[0].title,
        "Reset emails"
    );

    // A teammate changes one of the files and explains nothing.
    std::fs::write(
        dir.join("src/auth.rs"),
        "fn sign_in() {}\nfn reset() { expire(); }\n",
    )
    .unwrap();
    git(&dir, &["commit", "-q", "-am", "links expire"]);
    let theirs = git(&dir, &["rev-parse", "HEAD"]);
    assert!(
        find::of_commit(&store, &dir, &theirs, None)
            .unwrap()
            .is_empty()
    );
    // The line they rewrote is theirs, the one they left is still the
    // first commit's.
    let line = ph_git::content::line_origin(&dir, "src/auth.rs", 2)
        .unwrap()
        .unwrap();
    assert_eq!(line.commit, theirs);
    // What there is to offer for that file: the earlier explanation, which
    // says it is behind.
    let earlier = find::naming(&store, &dir, "src/auth.rs");
    assert_eq!(earlier[0].title, "Reset emails");
    assert_eq!(find::changed_since(&dir, &earlier[0]), ["src/auth.rs"]);
    assert!(find::newer(&store, &dir, &earlier[0]).is_none());
    // The file they did not touch is still explained as it stands.
    let line = ph_git::content::line_origin(&dir, "src/reset.rs", 1)
        .unwrap()
        .unwrap();
    assert_eq!(line.commit, amended);
    assert_eq!(
        find::of_commit(&store, &dir, &line.commit, Some("src/reset.rs")).unwrap()[0].title,
        "Reset emails"
    );

    // Someone explains the newer state: that one is the newer to read.
    std::fs::write(
        dir.join("src/auth.rs"),
        "fn sign_in() {}\nfn reset() { expire_after(HOUR); }\n",
    )
    .unwrap();
    explain(&store, &dir, "Links last an hour");
    git(&dir, &["commit", "-q", "-am", "an hour"]);
    assert_eq!(
        find::newer(&store, &dir, &earlier[0]).unwrap().title,
        "Links last an hour"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_shared_explanation_is_found_by_whoever_has_the_repository() {
    let dir = repo("shared");
    let mine = Store::open_in_memory().unwrap();

    std::fs::write(dir.join("src/mail.rs"), "fn send() { retry(3); }\n").unwrap();
    let id = explain(&mine, &dir, "Mail is sent three times");
    // Shared: the file rides in the same commit as the work it explains.
    let local = mine.explanation(id).unwrap().unwrap();
    let path = file::write(
        &dir,
        &Shared {
            markdown: local.markdown.clone(),
            written: local.created_at / 1000,
            author: "Test".into(),
            agent: local.agent.clone(),
            files: local.files.clone(),
        },
    )
    .unwrap();
    // Its own file is not part of the work it explains.
    assert!(ph_review::explanation::is_explanation_file(&path));
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "retry mail"]);
    let commit = git(&dir, &["rev-parse", "HEAD"]);

    // A teammate, with a store that never heard of it, finds the file.
    let theirs = Store::open_in_memory().unwrap();
    let found = find::of_commit(&theirs, &dir, &commit, Some("src/mail.rs")).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title, "Mail is sent three times");
    assert_eq!(found[0].author, "Test");
    assert_eq!(found[0].source, Source::Shared { path: path.clone() });
    // Where it was written, the file stands for the one kept there.
    let found = find::of_commit(&mine, &dir, &commit, None).unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].is_shared());

    // Two explanations ride in one commit (a squash): the one that explains
    // the file asked about comes first.
    std::fs::write(dir.join("src/auth.rs"), "fn sign_in() { audit(); }\n").unwrap();
    let auth = ph_git::content::pending(&dir).unwrap();
    file::write(
        &dir,
        &Shared {
            markdown: "# Sign-ins are logged".into(),
            written: 2_000_000_000,
            author: "A".into(),
            agent: "Codex".into(),
            files: auth,
        },
    )
    .unwrap();
    std::fs::write(dir.join("src/mail.rs"), "fn send() { retry(5); }\n").unwrap();
    let mail: Vec<_> = ph_git::content::pending(&dir)
        .unwrap()
        .into_iter()
        .filter(|(path, _)| path == "src/mail.rs")
        .collect();
    file::write(
        &dir,
        &Shared {
            markdown: "# Mail is sent five times".into(),
            written: 2_000_000_100,
            author: "B".into(),
            agent: "Codex".into(),
            files: mail,
        },
    )
    .unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "two things"]);
    let squash = git(&dir, &["rev-parse", "HEAD"]);
    let for_auth = find::of_commit(&theirs, &dir, &squash, Some("src/auth.rs")).unwrap();
    assert_eq!(for_auth.len(), 2);
    assert_eq!(for_auth[0].title, "Sign-ins are logged");
    let for_mail = find::of_commit(&theirs, &dir, &squash, Some("src/mail.rs")).unwrap();
    assert_eq!(for_mail[0].title, "Mail is sent five times");
    // The first explanation of the mail file says it is behind, and which
    // one is newer.
    let old = find::of_commit(&theirs, &dir, &commit, None)
        .unwrap()
        .remove(0);
    assert_eq!(find::changed_since(&dir, &old), ["src/mail.rs"]);
    assert_eq!(
        find::newer(&theirs, &dir, &old).unwrap().title,
        "Mail is sent five times"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
