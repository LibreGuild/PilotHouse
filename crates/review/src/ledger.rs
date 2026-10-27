//! A pull request's findings across review rounds (its ledger).
//!
//! Each round starts from the previous round's findings and:
//!
//! - moves them to where their lines are now (git's line map, not the
//!   agent's memory), keeping their label, text and severity: only people
//!   change those;
//! - applies the agent's checks: a check of the fixes says which are fixed
//!   (and reopens a "fixed" someone only claimed), a double-check which do
//!   not hold up;
//! - checks each new finding's quoted code is in the file, and moves it to
//!   the lines quoted;
//! - drops a new finding that is a recorded one again (adding the agent to
//!   it, or reopening it when it had been fixed), labels the rest `F<n>`,
//!   and marks those in code that had not changed since the last round as
//!   found late.

use std::collections::HashSet;

use ph_store::reviews::Finding;

use crate::grounding;
use crate::linemap::{LineMap, Moved};
use crate::parse::{Report, ReportedFinding};

/// What a round is built from.
pub struct Round<'a> {
    pub review_id: i64,
    pub round: u32,
    /// `full`, `followup`, `second`, `double_check` or `self`.
    pub kind: &'a str,
    /// The agent that ran it (its name).
    pub agent: &'a str,
    /// The pull request's findings before this round.
    pub previous: &'a [Finding],
    pub report: &'a Report,
    /// How lines moved since the last round's commit (none on the same
    /// commit, or the first round).
    pub lines: Option<&'a LineMap>,
    /// A file of the reviewed commit, by path.
    pub read_file: &'a dyn Fn(&str) -> Option<String>,
}

/// The pull request's findings after `round`.
pub fn build(round: Round) -> Vec<Finding> {
    let mut findings: Vec<Finding> = round
        .previous
        .iter()
        .cloned()
        .map(|mut finding| {
            finding.id = 0;
            finding.review_id = round.review_id;
            if let (Some(lines), Some(start)) = (round.lines, finding.line) {
                match lines.relocate(&finding.path, start, finding.end_line.unwrap_or(start)) {
                    Moved::At {
                        path, start, end, ..
                    } => {
                        finding.path = path;
                        finding.line = Some(start);
                        finding.end_line = (end > start).then_some(end);
                    }
                    Moved::Deleted if finding.status == "open" => {
                        set(&mut finding, "fixed", "agent", round.agent, "");
                        finding.note = "Its file was deleted.".into();
                    }
                    Moved::Deleted => {}
                }
            }
            finding
        })
        .collect();

    // The agent's word on earlier findings.
    for check in &round.report.checks {
        let Some(finding) = findings.iter_mut().find(|f| f.label == check.label) else {
            continue;
        };
        apply_check(finding, round.kind, round.agent, check.gone, &check.note);
    }
    for key in &round.report.resolved {
        if let Some(finding) = findings
            .iter_mut()
            .find(|f| &f.key == key && f.status == "open")
        {
            apply_check(finding, "followup", round.agent, true, "");
        }
    }

    let mut next = findings
        .iter()
        .filter_map(|f| f.label.strip_prefix('F')?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    for reported in &round.report.findings {
        let grounded = ground(reported, round.read_file);
        if let Some(known) = findings.iter_mut().find(|f| same_problem(f, &grounded)) {
            add_agent(known, round.agent);
            if known.status == "fixed" && round.kind != "double_check" {
                // It came back.
                set(known, "open", "agent", round.agent, "");
                known.note = "Reported again after it was fixed.".into();
            }
            continue;
        }
        if round.kind == "double_check" {
            // A double-check only judges what is there.
            continue;
        }
        next += 1;
        let start = grounded.line.unwrap_or(0);
        let found_late = round.round > 1
            && matches!(round.kind, "full" | "followup")
            && round.lines.is_some_and(|lines| {
                start > 0
                    && !lines.touched(&grounded.path, start, grounded.end_line.unwrap_or(start))
            });
        findings.push(Finding {
            review_id: round.review_id,
            key: grounded.key.clone(),
            label: format!("F{next}"),
            severity: grounded.severity.clone(),
            impact: grounded.impact.clone(),
            likelihood: grounded.likelihood.clone(),
            path: grounded.path.clone(),
            line: grounded.line,
            end_line: grounded.end_line,
            title: grounded.title.clone(),
            body: grounded.details.clone(),
            evidence: grounded.evidence.clone(),
            suggestion: grounded.suggestion.clone(),
            suggested_code: grounded.suggested_code.clone(),
            status: "open".into(),
            status_source: "agent".into(),
            status_by: round.agent.into(),
            first_round: round.round,
            found_late,
            verified: grounded.verified,
            agents: round.agent.into(),
            ..Finding::default()
        });
    }
    sort(&mut findings);
    findings
}

/// A reported finding, moved to the lines its quote is on.
struct Grounded {
    verified: bool,
    key: String,
    severity: String,
    impact: String,
    likelihood: String,
    path: String,
    line: Option<u64>,
    end_line: Option<u64>,
    title: String,
    details: String,
    evidence: String,
    suggestion: String,
    suggested_code: String,
}

fn ground(reported: &ReportedFinding, read_file: &dyn Fn(&str) -> Option<String>) -> Grounded {
    let place = (!reported.evidence.trim().is_empty())
        .then(|| read_file(&reported.path))
        .flatten()
        .and_then(|text| grounding::locate(&text, &reported.evidence, reported.line));
    let (line, end_line) = match place {
        Some((start, end)) => (Some(start), (end > start).then_some(end)),
        None => (reported.line, reported.end_line),
    };
    Grounded {
        verified: place.is_some(),
        key: reported.key.clone(),
        severity: reported.severity.clone(),
        impact: reported.impact.clone(),
        likelihood: reported.likelihood.clone(),
        path: reported.path.clone(),
        line,
        end_line,
        title: reported.title.clone(),
        details: reported.details.clone(),
        evidence: reported.evidence.clone(),
        suggestion: reported.suggestion.clone(),
        suggested_code: reported.suggested_code.clone(),
    }
}

fn apply_check(finding: &mut Finding, kind: &str, agent: &str, gone: bool, note: &str) {
    let note = note.trim();
    if kind == "double_check" {
        if finding.status != "open" {
            return;
        }
        if gone {
            set(finding, "not_problem", "double_check", agent, note);
            finding.note = format!("Did not hold up: {note}");
        } else {
            finding.note = format!("Double-checked by {agent}: it holds.");
        }
        return;
    }
    if gone {
        // Fixed: an open finding, or one someone only said was fixed.
        if finding.status == "open"
            || (finding.status == "fixed" && finding.status_source != "agent")
        {
            set(finding, "fixed", "agent", agent, "");
            finding.note = if note.is_empty() {
                "Checked: fixed.".into()
            } else {
                format!("Checked: {note}")
            };
        }
    } else if finding.status == "open" || finding.status == "fixed" {
        let claimed = finding.status == "fixed";
        set(finding, "open", "agent", agent, "");
        finding.note = match (claimed, note.is_empty()) {
            (true, true) => "Said to be fixed, but it is still there.".into(),
            (true, false) => format!("Said to be fixed, but it is still there: {note}"),
            (false, true) => "Still there.".into(),
            (false, false) => format!("Still there: {note}"),
        };
    }
}

fn set(finding: &mut Finding, status: &str, source: &str, by: &str, reason: &str) {
    finding.status = status.into();
    finding.status_source = source.into();
    finding.status_by = by.into();
    finding.reason = reason.into();
}

fn add_agent(finding: &mut Finding, agent: &str) {
    if agent.is_empty() {
        return;
    }
    let known: Vec<&str> = finding
        .agents
        .split(", ")
        .filter(|a| !a.is_empty())
        .collect();
    if !known.contains(&agent) {
        finding.agents = known
            .into_iter()
            .chain([agent])
            .collect::<Vec<_>>()
            .join(", ");
    }
}

/// Whether `new` is `known` reported again: the same key, or the same file
/// with lines close by and much the same words.
fn same_problem(known: &Finding, new: &Grounded) -> bool {
    if known.path != new.path {
        return false;
    }
    if !known.key.is_empty() && known.key == new.key {
        return true;
    }
    let words = |text: &str| -> HashSet<String> {
        text.to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| word.len() > 2)
            .map(str::to_string)
            .collect()
    };
    let similarity = |a: &str, b: &str| {
        let (a, b) = (words(a), words(b));
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        a.intersection(&b).count() as f64 / a.union(&b).count() as f64
    };
    let near = match (known.line, new.line) {
        (Some(a), Some(b)) => {
            let (a_end, b_end) = (known.end_line.unwrap_or(a), new.end_line.unwrap_or(b));
            a <= b_end + 3 && b <= a_end + 3
        }
        _ => false,
    };
    let title = similarity(&known.title, &new.title);
    (near && (title >= 0.3 || similarity(&known.evidence, &new.evidence) >= 0.6)) || title >= 0.7
}

/// Open findings first, by severity and label; then the rest.
pub fn sort(findings: &mut [Finding]) {
    let number = |f: &Finding| -> u32 {
        f.label
            .strip_prefix('F')
            .and_then(|n| n.parse().ok())
            .unwrap_or(u32::MAX)
    };
    findings.sort_by_key(|f| {
        (
            f.status != "open",
            crate::severity_rank(&f.severity),
            number(f),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Check;

    fn reported(path: &str, line: u64, title: &str, evidence: &str) -> ReportedFinding {
        ReportedFinding {
            key: crate::parse::fingerprint(path, title),
            severity: "major".into(),
            path: path.into(),
            line: Some(line),
            title: title.into(),
            details: "details".into(),
            evidence: evidence.into(),
            ..ReportedFinding::default()
        }
    }

    const FILE: &str = "a\nb\nlet token = read();\nlog(token);\ne\nf\n";

    #[test]
    fn rounds_keep_labels_and_check_fixes() {
        let read = |path: &str| (path == "src/a.rs").then(|| FILE.to_string());
        let first = build(Round {
            review_id: 1,
            round: 1,
            kind: "full",
            agent: "Claude Code",
            previous: &[],
            report: &Report {
                findings: vec![
                    reported("src/a.rs", 9, "Token is logged", "log(token);"),
                    reported("src/a.rs", 1, "Nothing here", "not in the file at all"),
                ],
                ..Report::default()
            },
            lines: None,
            read_file: &read,
        });
        assert_eq!(first[0].label, "F1");
        assert_eq!(
            (first[0].line, first[0].verified),
            (Some(4), true),
            "moved to the quoted line"
        );
        assert!(!first[1].verified, "its code is not in the file");

        // Round 2: F1 fixed, F2 still there, the same token problem again
        // (dropped) and a new one in unchanged code (found late).
        let lines = LineMap::parse(
            "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,0 +2,1 @@\n+new\n",
        );
        let second = build(Round {
            review_id: 2,
            round: 2,
            kind: "followup",
            agent: "Codex",
            previous: &first,
            report: &Report {
                findings: vec![
                    reported("src/a.rs", 5, "Token is logged", "log(token);"),
                    reported("src/a.rs", 6, "Missing tests for read", ""),
                ],
                checks: vec![
                    Check {
                        label: "F1".into(),
                        gone: true,
                        note: String::new(),
                    },
                    Check {
                        label: "F2".into(),
                        gone: false,
                        note: "same".into(),
                    },
                ],
                ..Report::default()
            },
            lines: Some(&lines),
            read_file: &read,
        });
        let by_label = |label: &str| second.iter().find(|f| f.label == label).unwrap();
        assert_eq!(
            by_label("F1").status,
            "open",
            "reported again after the fix: reopened"
        );
        assert_eq!(by_label("F1").line, Some(5), "moved by the line map");
        assert_eq!(by_label("F2").note, "Still there: same");
        let late = by_label("F3");
        assert!(late.found_late, "line 6 did not change");
        assert_eq!(second.len(), 3, "the repeat was not added");
        assert_eq!(by_label("F1").agents, "Claude Code, Codex");
    }

    #[test]
    fn a_claimed_fix_is_checked_and_a_double_check_can_refute() {
        let read = |_: &str| None;
        let mut claimed = Finding {
            label: "F1".into(),
            path: "a.rs".into(),
            status: "fixed".into(),
            status_source: "author".into(),
            ..Finding::default()
        };
        let open = Finding {
            label: "F2".into(),
            path: "a.rs".into(),
            status: "open".into(),
            ..Finding::default()
        };
        let after = build(Round {
            review_id: 3,
            round: 3,
            kind: "followup",
            agent: "Codex",
            previous: &[claimed.clone(), open.clone()],
            report: &Report {
                checks: vec![Check {
                    label: "F1".into(),
                    gone: false,
                    note: String::new(),
                }],
                ..Report::default()
            },
            lines: None,
            read_file: &read,
        });
        let f1 = after.iter().find(|f| f.label == "F1").unwrap();
        assert_eq!(f1.status, "open", "the claim did not hold");

        claimed.status = "open".into();
        let checked = build(Round {
            review_id: 4,
            round: 4,
            kind: "double_check",
            agent: "Gemini",
            previous: &[claimed, open],
            report: &Report {
                checks: vec![Check {
                    label: "F2".into(),
                    gone: true,
                    note: "validated upstream".into(),
                }],
                findings: vec![reported("a.rs", 1, "Brand new", "")],
                ..Report::default()
            },
            lines: None,
            read_file: &read,
        });
        let f2 = checked.iter().find(|f| f.label == "F2").unwrap();
        assert_eq!(
            (f2.status.as_str(), f2.status_source.as_str()),
            ("not_problem", "double_check")
        );
        assert_eq!(checked.len(), 2, "a double-check adds no findings");
    }
}
