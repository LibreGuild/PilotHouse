//! Which pull request to review first: its size, what it touches that is
//! risky, its CI, and whether your last review is behind its head. Pull
//! requests sort small, green and risky first; ones you reviewed whose
//! author has pushed since come before new ones.

use ph_github::models::{Check, PullFile, PullRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Size {
    Small,
    Medium,
    Large,
}

impl Size {
    pub fn of(lines: u64) -> Self {
        match lines {
            0..=150 => Size::Small,
            151..=600 => Size::Medium,
            _ => Size::Large,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Medium => "medium",
            Size::Large => "large",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ci {
    Passing,
    Failing,
    Running,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Triage {
    pub size: Size,
    /// Lines added and removed.
    pub lines: u64,
    /// What it touches that deserves care (`auth`, `migration`…).
    pub risks: Vec<&'static str>,
    pub ci: Ci,
    /// You reviewed it, and the author pushed since.
    pub behind: bool,
    /// You reviewed it at its head: nothing new.
    pub reviewed: bool,
    /// The head commit it was assessed at.
    pub head: String,
}

/// Paths that make a change risky, by what they are.
const RISKY: &[(&str, &[&str])] = &[
    (
        "auth",
        &[
            "auth",
            "login",
            "session",
            "token",
            "password",
            "oauth",
            "permission",
        ],
    ),
    (
        "security",
        &["crypto", "secret", "security", "sanitiz", "csrf", "xss"],
    ),
    ("migration", &["migration", "migrations/", "schema", ".sql"]),
    ("payments", &["payment", "billing", "invoice", "stripe"]),
    (
        "CI",
        &[
            ".github/workflows",
            ".gitlab-ci",
            "jenkinsfile",
            "dockerfile",
        ],
    ),
    (
        "dependencies",
        &[
            "cargo.lock",
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock",
            "go.sum",
            "poetry.lock",
            "gemfile.lock",
        ],
    ),
];

/// Assesses `pull` from its files and checks; `reviewed_at` is the commit
/// your newest review read, if any.
pub fn assess(
    pull: &PullRequest,
    files: &[PullFile],
    checks: &[Check],
    reviewed_at: Option<&str>,
) -> Triage {
    let lines = pull.additions + pull.deletions;
    let mut risks = Vec::new();
    for (risk, words) in RISKY {
        let hit = files.iter().any(|file| {
            let path = file.filename.to_ascii_lowercase();
            words.iter().any(|word| path.contains(word))
        });
        if hit {
            risks.push(*risk);
        }
    }
    let ci = if checks.is_empty() {
        Ci::None
    } else if checks.iter().any(|c| {
        matches!(
            c.outcome.as_str(),
            "failure" | "cancelled" | "timed_out" | "action_required"
        )
    }) {
        Ci::Failing
    } else if checks.iter().any(|c| c.outcome == "pending") {
        Ci::Running
    } else {
        Ci::Passing
    };
    Triage {
        size: Size::of(lines),
        lines,
        risks,
        ci,
        behind: false,
        reviewed: false,
        head: pull.head.sha.clone(),
    }
    .with_review(reviewed_at)
}

impl Triage {
    /// The same, against your newest review (at commit `reviewed_at`).
    pub fn with_review(mut self, reviewed_at: Option<&str>) -> Self {
        self.behind = reviewed_at.is_some_and(|sha| sha != self.head);
        self.reviewed = reviewed_at == Some(self.head.as_str());
        self
    }

    /// Lower comes first: your reviews the author answered with commits,
    /// then CI not failing, risky before plain, small before large.
    pub fn rank(&self) -> (u8, u8, u8, Size) {
        (
            if self.behind {
                0
            } else if self.reviewed {
                3
            } else {
                1
            },
            matches!(self.ci, Ci::Failing) as u8,
            self.risks.is_empty() as u8,
            self.size,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_github::models::Branch;

    fn pull(lines: u64, head: &str) -> PullRequest {
        PullRequest {
            additions: lines,
            head: Branch {
                sha: head.into(),
                ..Branch::default()
            },
            ..PullRequest::default()
        }
    }

    fn file(path: &str) -> PullFile {
        PullFile {
            filename: path.into(),
            ..PullFile::default()
        }
    }

    #[test]
    fn sizes_risks_ci_and_stale_reviews() {
        let checks = [Check {
            outcome: "failure".into(),
            ..Check::default()
        }];
        let triage = assess(
            &pull(40, "b"),
            &[file("src/auth/login.rs"), file("Cargo.lock")],
            &checks,
            Some("a"),
        );
        assert_eq!(triage.size, Size::Small);
        assert_eq!(triage.risks, ["auth", "dependencies"]);
        assert_eq!(triage.ci, Ci::Failing);
        assert!(triage.behind && !triage.reviewed);
        let plain = assess(&pull(900, "c"), &[file("README.md")], &[], None);
        assert_eq!((plain.size, plain.ci), (Size::Large, Ci::None));
        assert!(
            triage.rank() < plain.rank(),
            "a review to finish comes first"
        );
    }
}
