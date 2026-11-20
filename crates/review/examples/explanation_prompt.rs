//! Prints the prompt an agent gets when it is asked to explain a change, so
//! the wording can be tried on real agents and real changes without the app:
//!
//! ```sh
//! cargo run -p ph-review --example explanation_prompt -- <folder>
//! cargo run -p ph-review --example explanation_prompt -- <folder> --commit <sha>
//! cargo run -p ph-review --example explanation_prompt -- <folder> --asked record.json
//! ```
//!
//! `record.json` is what the chats said (a list of
//! `ph_review::explanation::prompt::Asked`). Give the output to an agent in
//! that folder (`claude -p "$(…)"`, `codex exec`, `gemini -p`), read what
//! comes back, and change `prompt.rs`. See docs/testing.md.

use std::path::PathBuf;

use ph_review::explanation::prompt::{Asked, Size, Subject, explanation_prompt};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let (mut commit, mut asked) = (None, Vec::<Asked>::new());
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--commit" => commit = args.next(),
            "--asked" => {
                let file = args.next().unwrap_or_default();
                asked = serde_json::from_str(&std::fs::read_to_string(file)?)?;
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let (subject, content, lines) = match commit {
        Some(sha) => {
            let subject = ph_git::git(&root, &["log", "-1", "--format=%s", &sha])?;
            let content = ph_git::content::of_commit(&root, &sha)?;
            let lines = ph_git::content::lines_of_commit(&root, &sha);
            (
                Subject::Commit {
                    sha,
                    subject: subject.trim().to_string(),
                },
                content,
                lines,
            )
        }
        None => (
            Subject::Changes,
            ph_git::content::pending(&root)?,
            ph_git::content::lines_pending(&root),
        ),
    };
    let files: Vec<String> = content
        .into_iter()
        .map(|(path, _)| path)
        .filter(|path| !ph_review::explanation::is_explanation_file(path))
        .collect();
    let size = Size {
        files: files.len(),
        lines,
    };
    print!("{}", explanation_prompt(&subject, &files, size, &asked));
    Ok(())
}
