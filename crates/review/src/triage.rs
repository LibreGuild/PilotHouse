//! Which pull request to review first: its size, what it touches that is
//! risky, its CI, and whether your last review is behind its head. Pull
//! requests sort small, green and risky first; ones you reviewed whose
//! author has pushed since come before new ones.
//!
//! And whose turn it is: where one of your own pull requests stands
//! ([`Mine`]), and one you reviewed ([`Turn`]).

use ph_github::search::{Checks, Decision, FoundPull};

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

/// Assesses `pull` from what the list read of it; `reviewed_at` is the
/// commit your newest review read (an agent's here, else yours on GitHub).
/// `None` when the list came without detail.
pub fn assess(pull: &FoundPull, reviewed_at: Option<&str>) -> Option<Triage> {
    let detail = pull.detail.as_ref()?;
    let lines = detail.additions + detail.deletions;
    let mut risks = Vec::new();
    for (risk, words) in RISKY {
        let hit = detail.paths.iter().any(|path| {
            let path = path.to_ascii_lowercase();
            words.iter().any(|word| path.contains(word))
        });
        if hit {
            risks.push(*risk);
        }
    }
    let reviewed_at = reviewed_at.or(detail
        .my_review
        .as_ref()
        .map(|review| review.commit.as_str())
        .filter(|commit| !commit.is_empty()));
    Some(
        Triage {
            size: Size::of(lines),
            lines,
            risks,
            ci: match detail.checks {
                Checks::Passing => Ci::Passing,
                Checks::Failing => Ci::Failing,
                Checks::Running => Ci::Running,
                Checks::None => Ci::None,
            },
            behind: false,
            reviewed: false,
            head: detail.head_sha.clone(),
        }
        .with_review(reviewed_at),
    )
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

/// Where one of your own pull requests stands, what needs you first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mine {
    /// A reviewer asked for changes.
    ChangesRequested,
    ChecksFailing,
    /// It conflicts with its base.
    Conflicts,
    /// Approved, nothing failing: it can be merged.
    Ready,
    /// Nobody has decided yet.
    Waiting,
    Draft,
}

impl Mine {
    pub const ALL: [Mine; 6] = [
        Mine::ChangesRequested,
        Mine::ChecksFailing,
        Mine::Conflicts,
        Mine::Ready,
        Mine::Waiting,
        Mine::Draft,
    ];

    pub fn of(pull: &FoundPull) -> Self {
        if pull.draft {
            return Mine::Draft;
        }
        let Some(detail) = &pull.detail else {
            return Mine::Waiting;
        };
        if detail.decision == Decision::ChangesRequested {
            Mine::ChangesRequested
        } else if detail.checks == Checks::Failing {
            Mine::ChecksFailing
        } else if detail.mergeable == Some(false) {
            Mine::Conflicts
        } else if detail.decision == Decision::Approved && detail.checks != Checks::Running {
            Mine::Ready
        } else {
            Mine::Waiting
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Mine::ChangesRequested => "Changes requested",
            Mine::ChecksFailing => "Checks failing",
            Mine::Conflicts => "Conflicts with the base",
            Mine::Ready => "Ready to merge",
            Mine::Waiting => "Waiting for review",
            Mine::Draft => "Drafts",
        }
    }
}

/// Whose turn it is on a pull request you reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Turn {
    /// The author pushed since your review: look again.
    Yours,
    /// You approved it at its head: nothing left for you.
    Approved,
    /// The author has not answered yet.
    Authors,
}

impl Turn {
    pub const ALL: [Turn; 3] = [Turn::Yours, Turn::Authors, Turn::Approved];

    /// `reviewed_at`: the commit an agent's review here read, if any.
    pub fn of(pull: &FoundPull, reviewed_at: Option<&str>) -> Self {
        let Some(detail) = &pull.detail else {
            return Turn::Authors;
        };
        let mine = detail.my_review.as_ref();
        let seen = reviewed_at.or(mine.map(|review| review.commit.as_str()));
        if seen.is_some_and(|sha| !sha.is_empty() && sha != detail.head_sha) {
            Turn::Yours
        } else if mine.is_some_and(|review| review.state == "APPROVED") {
            Turn::Approved
        } else {
            Turn::Authors
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Turn::Yours => "New commits since your review",
            Turn::Authors => "Waiting for the author",
            Turn::Approved => "Approved by you",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ph_github::search::{Detail, MyReview};

    fn pull(lines: u64, head: &str, paths: &[&str], checks: Checks) -> FoundPull {
        FoundPull {
            detail: Some(Detail {
                additions: lines,
                head_sha: head.into(),
                checks,
                paths: paths.iter().map(|path| path.to_string()).collect(),
                ..Detail::default()
            }),
            ..FoundPull::default()
        }
    }

    #[test]
    fn sizes_risks_ci_and_stale_reviews() {
        let triage = assess(
            &pull(
                40,
                "b",
                &["src/auth/login.rs", "Cargo.lock"],
                Checks::Failing,
            ),
            Some("a"),
        )
        .unwrap();
        assert_eq!(triage.size, Size::Small);
        assert_eq!(triage.risks, ["auth", "dependencies"]);
        assert_eq!(triage.ci, Ci::Failing);
        assert!(triage.behind && !triage.reviewed);
        let plain = assess(&pull(900, "c", &["README.md"], Checks::None), None).unwrap();
        assert_eq!((plain.size, plain.ci), (Size::Large, Ci::None));
        assert!(
            triage.rank() < plain.rank(),
            "a review to finish comes first"
        );
        assert_eq!(assess(&FoundPull::default(), None), None);
    }

    #[test]
    fn says_whose_turn_it_is() {
        let mut mine = pull(10, "b", &[], Checks::Passing);
        assert_eq!(Mine::of(&mine), Mine::Waiting);
        let detail = mine.detail.as_mut().unwrap();
        detail.decision = Decision::Approved;
        assert_eq!(Mine::of(&mine), Mine::Ready);
        mine.detail.as_mut().unwrap().mergeable = Some(false);
        assert_eq!(Mine::of(&mine), Mine::Conflicts);
        mine.detail.as_mut().unwrap().checks = Checks::Failing;
        assert_eq!(Mine::of(&mine), Mine::ChecksFailing);
        mine.detail.as_mut().unwrap().decision = Decision::ChangesRequested;
        assert_eq!(Mine::of(&mine), Mine::ChangesRequested);
        mine.draft = true;
        assert_eq!(Mine::of(&mine), Mine::Draft);

        let mut reviewed = pull(10, "b", &[], Checks::Passing);
        reviewed.detail.as_mut().unwrap().my_review = Some(MyReview {
            state: "APPROVED".into(),
            commit: "b".into(),
        });
        assert_eq!(Turn::of(&reviewed, None), Turn::Approved);
        // An agent's review here read an older commit: the author pushed.
        assert_eq!(Turn::of(&reviewed, Some("a")), Turn::Yours);
        reviewed.detail.as_mut().unwrap().my_review = Some(MyReview {
            state: "COMMENTED".into(),
            commit: "b".into(),
        });
        assert_eq!(Turn::of(&reviewed, None), Turn::Authors);
    }
}
