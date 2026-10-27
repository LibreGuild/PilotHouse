//! How serious a finding is: worked out by PilotHouse from two plain
//! answers the agent gives (what goes wrong, and how often), not picked by
//! the agent, so the same problem gets the same severity on every run. The
//! verdict follows from what is still open.

use ph_store::reviews::Finding;

/// What goes wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Impact {
    /// A security hole, data lost or corrupted.
    Security,
    /// A crash, an outage, a hang.
    Crash,
    /// A wrong result or behaviour.
    Wrong,
    /// Slow or wasteful.
    Slow,
    /// Harder to maintain, missing tests.
    Maintain,
    /// An optional improvement.
    Improve,
}

/// How often it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Likelihood {
    /// In normal use.
    Common,
    /// In specific, realistic conditions.
    Specific,
    /// Rarely.
    Rare,
}

impl Impact {
    /// Reads an agent's answer leniently (`security`, `data loss`, `bug`…).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|word| text.contains(word));
        Some(
            if has(&["secur", "data", "corrupt", "leak", "inject", "auth"]) {
                Impact::Security
            } else if has(&["crash", "outage", "hang", "panic", "deadlock"]) {
                Impact::Crash
            } else if has(&["wrong", "incorrect", "bug", "behav", "logic"]) {
                Impact::Wrong
            } else if has(&["slow", "perf", "wasteful", "memory"]) {
                Impact::Slow
            } else if has(&["maintain", "readab", "test", "style", "clarity"]) {
                Impact::Maintain
            } else if has(&["improve", "suggest", "optional", "nit"]) {
                Impact::Improve
            } else {
                return None;
            },
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Impact::Security => "security",
            Impact::Crash => "crash",
            Impact::Wrong => "wrong",
            Impact::Slow => "slow",
            Impact::Maintain => "maintain",
            Impact::Improve => "improve",
        }
    }

    /// For people: "Security or data".
    pub fn label(self) -> &'static str {
        match self {
            Impact::Security => "Security or data",
            Impact::Crash => "Crash or hang",
            Impact::Wrong => "Wrong result",
            Impact::Slow => "Slow",
            Impact::Maintain => "Maintainability",
            Impact::Improve => "Improvement",
        }
    }
}

impl Likelihood {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        Some(
            if text.contains("common") || text.contains("normal") || text.contains("always") {
                Likelihood::Common
            } else if text.contains("specific")
                || text.contains("condition")
                || text.contains("some")
            {
                Likelihood::Specific
            } else if text.contains("rare") || text.contains("unlikely") || text.contains("edge") {
                Likelihood::Rare
            } else {
                return None;
            },
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Likelihood::Common => "common",
            Likelihood::Specific => "specific",
            Likelihood::Rare => "rare",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Likelihood::Common => "in normal use",
            Likelihood::Specific => "in specific conditions",
            Likelihood::Rare => "rarely",
        }
    }
}

/// The severity of `impact` happening `likelihood`.
pub fn severity_for(impact: Impact, likelihood: Likelihood) -> &'static str {
    use Impact::*;
    use Likelihood::*;
    match (impact, likelihood) {
        (Security, Rare) => "major",
        (Security, _) => "critical",
        (Crash, Common) => "critical",
        (Crash, Specific) => "major",
        (Crash, Rare) => "minor",
        (Wrong, Rare) => "minor",
        (Wrong, _) => "major",
        (Slow, Common) => "major",
        (Slow, _) => "minor",
        (Maintain, _) => "minor",
        (Improve, _) => "nit",
    }
}

/// Why a finding has its severity, for people: "Wrong result, in specific
/// conditions". Empty for findings without the two answers.
pub fn reasoning(finding: &Finding) -> String {
    match (
        Impact::parse(&finding.impact),
        Likelihood::parse(&finding.likelihood),
    ) {
        (Some(impact), Some(likelihood)) => format!("{}, {}", impact.label(), likelihood.label()),
        _ => String::new(),
    }
}

/// The verdict the open findings call for: changes for any critical or
/// major one, a comment for a minor one, else approval.
pub fn verdict(findings: &[Finding]) -> &'static str {
    let open = findings.iter().filter(|f| f.status == "open");
    let mut verdict = "approve";
    for finding in open {
        match finding.severity.as_str() {
            "critical" | "major" => return "request_changes",
            "minor" => verdict = "comment",
            _ => {}
        }
    }
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_comes_from_the_two_answers() {
        let sev = |impact: &str, likelihood: &str| {
            severity_for(
                Impact::parse(impact).unwrap(),
                Likelihood::parse(likelihood).unwrap(),
            )
        };
        assert_eq!(sev("security", "rare"), "major");
        assert_eq!(sev("data loss", "common"), "critical");
        assert_eq!(sev("crash", "specific conditions"), "major");
        assert_eq!(sev("wrong result", "normal use"), "major");
        assert_eq!(sev("slow", "rarely"), "minor");
        assert_eq!(sev("missing tests", "common"), "minor");
        assert_eq!(sev("optional improvement", "common"), "nit");
    }

    #[test]
    fn the_verdict_follows_what_is_open() {
        let finding = |severity: &str, status: &str| Finding {
            severity: severity.into(),
            status: status.into(),
            ..Finding::default()
        };
        assert_eq!(verdict(&[]), "approve");
        assert_eq!(
            verdict(&[finding("minor", "open"), finding("nit", "open")]),
            "comment"
        );
        assert_eq!(
            verdict(&[finding("major", "fixed"), finding("nit", "open")]),
            "approve"
        );
        assert_eq!(verdict(&[finding("critical", "open")]), "request_changes");
    }
}
