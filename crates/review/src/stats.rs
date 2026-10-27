//! How reviews of a repository went: what was found, what came of it, and
//! how often each agent was wrong (its findings marked not a problem).

use std::collections::BTreeMap;

use ph_store::reviews::Finding;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// Pull requests reviewed.
    pub pulls: usize,
    pub findings: usize,
    pub open: usize,
    pub fixed: usize,
    pub not_problem: usize,
    pub wont_fix: usize,
    /// Findings an earlier round missed.
    pub found_late: usize,
    /// Rounds per pull request, on average.
    pub rounds: f32,
    /// By agent: (findings, marked not a problem).
    pub agents: BTreeMap<String, (usize, usize)>,
}

impl Stats {
    /// Of the findings someone decided on, the share that were not a
    /// problem (0–100).
    pub fn false_positive_percent(&self) -> Option<u32> {
        let decided = self.fixed + self.not_problem + self.wont_fix;
        (decided > 0).then(|| (self.not_problem * 100 / decided) as u32)
    }
}

/// Stats from each pull request's newest round (its round number, and its
/// findings).
pub fn of(pulls: &[(u32, Vec<Finding>)]) -> Stats {
    let mut stats = Stats {
        pulls: pulls.len(),
        ..Stats::default()
    };
    let mut rounds = 0;
    for (round, findings) in pulls {
        rounds += round;
        for finding in findings {
            stats.findings += 1;
            match finding.status.as_str() {
                "fixed" => stats.fixed += 1,
                "not_problem" => stats.not_problem += 1,
                "wont_fix" => stats.wont_fix += 1,
                _ => stats.open += 1,
            }
            if finding.found_late {
                stats.found_late += 1;
            }
            for agent in finding.agents.split(", ").filter(|a| !a.is_empty()) {
                let entry = stats.agents.entry(agent.to_string()).or_default();
                entry.0 += 1;
                if finding.status == "not_problem" {
                    entry.1 += 1;
                }
            }
        }
    }
    if !pulls.is_empty() {
        stats.rounds = rounds as f32 / pulls.len() as f32;
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_outcomes_and_wrong_calls_by_agent() {
        let finding = |status: &str, agents: &str| Finding {
            status: status.into(),
            agents: agents.into(),
            ..Finding::default()
        };
        let stats = of(&[
            (
                2,
                vec![finding("fixed", "Claude"), finding("not_problem", "Codex")],
            ),
            (
                1,
                vec![
                    finding("open", "Claude, Codex"),
                    finding("wont_fix", "Claude"),
                ],
            ),
        ]);
        assert_eq!((stats.pulls, stats.findings, stats.open), (2, 4, 1));
        assert_eq!(stats.false_positive_percent(), Some(33));
        assert_eq!(stats.agents["Codex"], (2, 1));
        assert_eq!(stats.rounds, 1.5);
    }
}
