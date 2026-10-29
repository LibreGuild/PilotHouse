//! The pull request's one summary comment: what is open, with boxes the
//! author ticks (Fixed / Not a problem / Won't fix), what is done, and, in
//! a hidden block, the findings themselves, so anyone's PilotHouse that
//! opens the pull request (the author's too) shows the same findings.
//!
//! Only the comment's author (the reviewer) rewrites it; everyone else
//! ticks boxes or replies. Each box carries a marker naming the finding and
//! the status (`<!-- ph:F3:fixed -->`), so ticks are read back reliably and
//! the rest of the text can be edited freely.

use std::collections::BTreeMap;

use base64::Engine as _;
use ph_store::reviews::Finding;
use serde::{Deserialize, Serialize};

/// The first line of every summary comment.
pub const MARKER: &str = "<!-- pilothouse-review";
const DATA: &str = "<!-- pilothouse-data ";
/// GitHub refuses comments over 65,536 characters; room is left for edits.
const MAX: usize = 60_000;
/// Most of that for the text people read; the rest is the hidden block's.
const SHOWN_MAX: usize = 26_000;

/// The statuses an author can tick, in the order of the boxes.
pub const BOXES: [(&str, &str); 3] = [
    ("fixed", "Fixed"),
    ("not_problem", "Not a problem"),
    ("wont_fix", "Won't fix"),
];

/// A finding as the hidden block carries it (short keys keep it small).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Shared {
    #[serde(rename = "l")]
    pub label: String,
    #[serde(rename = "s")]
    pub severity: String,
    #[serde(rename = "i", default, skip_serializing_if = "String::is_empty")]
    pub impact: String,
    #[serde(rename = "h", default, skip_serializing_if = "String::is_empty")]
    pub likelihood: String,
    #[serde(rename = "f")]
    pub path: String,
    #[serde(rename = "a", default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    #[serde(rename = "b", default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u64>,
    #[serde(rename = "t")]
    pub title: String,
    #[serde(rename = "w", default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(rename = "e", default, skip_serializing_if = "String::is_empty")]
    pub evidence: String,
    #[serde(rename = "x", default, skip_serializing_if = "String::is_empty")]
    pub suggestion: String,
    #[serde(rename = "sc", default, skip_serializing_if = "String::is_empty")]
    pub suggested_code: String,
    #[serde(rename = "st")]
    pub status: String,
    #[serde(rename = "by", default, skip_serializing_if = "String::is_empty")]
    pub status_by: String,
    #[serde(rename = "r", default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(rename = "n", default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(rename = "fr", default)]
    pub first_round: u32,
    #[serde(rename = "late", default, skip_serializing_if = "std::ops::Not::not")]
    pub found_late: bool,
    #[serde(rename = "c", default, skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<u64>,
    #[serde(rename = "is", default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<u64>,
}

/// What the hidden block holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SharedReview {
    #[serde(rename = "v")]
    pub version: u32,
    pub round: u32,
    pub sha: String,
    pub verdict: String,
    #[serde(default)]
    pub summary: String,
    /// Who reviewed (their GitHub login).
    #[serde(default)]
    pub reviewer: String,
    pub findings: Vec<Shared>,
}

impl Shared {
    pub fn of(finding: &Finding) -> Self {
        Self {
            label: finding.label.clone(),
            severity: finding.severity.clone(),
            impact: finding.impact.clone(),
            likelihood: finding.likelihood.clone(),
            path: finding.path.clone(),
            line: finding.line,
            end_line: finding.end_line,
            title: finding.title.clone(),
            body: finding.body.clone(),
            evidence: cut(&finding.evidence, 1_200),
            suggestion: cut(&finding.suggestion, 1_200),
            suggested_code: cut(unfenced(&finding.suggested_code), 1_200),
            status: finding.status.clone(),
            status_by: finding.status_by.clone(),
            reason: finding.reason.clone(),
            note: finding.note.clone(),
            first_round: finding.first_round,
            found_late: finding.found_late,
            comment_id: finding.comment_id,
            issue: finding.issue_number,
        }
    }

    /// As a finding (of no stored review), for showing.
    pub fn to_finding(&self) -> Finding {
        Finding {
            label: self.label.clone(),
            key: self.label.clone(),
            severity: self.severity.clone(),
            impact: self.impact.clone(),
            likelihood: self.likelihood.clone(),
            path: self.path.clone(),
            line: self.line,
            end_line: self.end_line,
            title: self.title.clone(),
            body: self.body.clone(),
            evidence: self.evidence.clone(),
            suggestion: self.suggestion.clone(),
            suggested_code: self.suggested_code.clone(),
            status: self.status.clone(),
            status_by: self.status_by.clone(),
            reason: self.reason.clone(),
            note: self.note.clone(),
            first_round: self.first_round.max(1),
            found_late: self.found_late,
            verified: true,
            comment_id: self.comment_id,
            comment_kind: self.comment_id.map(|_| "line".into()),
            issue_number: self.issue,
            ..Finding::default()
        }
    }
}

/// A finding's fix as Markdown: what to do (a paragraph of its own when it
/// runs over several lines), then the replacement code, fenced, with `kind`
/// after the fence (`suggestion` makes it GitHub's one-click suggestion).
pub fn fix_markdown(finding: &Finding, kind: &str) -> String {
    let mut md = String::new();
    let text = finding.suggestion.trim();
    if text.contains('\n') {
        md.push_str(&format!("**Fix:**\n\n{text}"));
    } else if !text.is_empty() {
        md.push_str(&format!("**Fix:** {text}"));
    }
    let code = unfenced(&finding.suggested_code);
    if !code.trim().is_empty() {
        let fence = fence_for(code);
        if !md.is_empty() {
            md.push_str("\n\n");
        }
        md.push_str(&format!("{fence}{kind}\n{}\n{fence}", code.trim_end()));
    }
    md
}

/// `code` without the Markdown fence an agent may have put around it (the
/// replacement goes in a fence of ours: one inside it would become code).
pub fn unfenced(code: &str) -> &str {
    let lines = code.trim_matches(|c| c == '\n' || c == '\r');
    let trimmed = lines.trim();
    for fence in ["```", "~~~"] {
        if trimmed.starts_with(fence)
            && trimmed.ends_with(fence)
            && let Some(newline) = trimmed.find('\n')
            && newline < trimmed.len() - fence.len()
        {
            return trimmed[newline + 1..trimmed.len() - fence.len()].trim_end();
        }
    }
    lines
}

/// A code fence longer than any run of backticks in `code`.
fn fence_for(code: &str) -> String {
    let longest = code.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat((longest + 1).max(3))
}

/// `text` with every line after the first indented by `indent`, to stay in
/// a list item.
fn indented(text: &str, indent: &str) -> String {
    text.replace('\n', &format!("\n{indent}"))
}

fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// What the summary comment says, and the context it needs.
pub struct Summary<'a> {
    pub round: u32,
    pub sha: &'a str,
    pub verdict: &'a str,
    pub summary: &'a str,
    pub reviewer: &'a str,
    /// The agents that reviewed (names), for the footer.
    pub agents: &'a str,
    /// `https://<host>/<owner>/<repo>`, for links to lines.
    pub repo_url: &'a str,
    pub number: u64,
    pub findings: &'a [Finding],
}

/// The comment's Markdown.
pub fn render(summary: &Summary) -> String {
    let short = &summary.sha[..summary.sha.len().min(7)];
    let open: Vec<&Finding> = summary
        .findings
        .iter()
        .filter(|f| {
            f.status == "open" && !(f.found_late && matches!(f.severity.as_str(), "minor" | "nit"))
        })
        .collect();
    let done: Vec<&Finding> = summary
        .findings
        .iter()
        .filter(|f| f.status != "open")
        .collect();
    let mut md = format!(
        "{MARKER} v1 sha={} round={} -->\n",
        summary.sha, summary.round
    );
    md.push_str(&format!(
        "### PilotHouse review · round {} · `{short}`\n\n",
        summary.round
    ));
    let mut counts: Vec<String> = crate::SEVERITIES
        .iter()
        .filter_map(|severity| {
            let n = open.iter().filter(|f| f.severity == *severity).count();
            (n > 0).then(|| format!("{n} {severity}"))
        })
        .collect();
    if counts.is_empty() {
        counts.push("nothing open".into());
    }
    let fixed = done.iter().filter(|f| f.status == "fixed").count();
    if fixed > 0 {
        counts.push(format!("{fixed} fixed"));
    }
    if done.len() > fixed {
        counts.push(format!("{} not fixing", done.len() - fixed));
    }
    md.push_str(&format!(
        "**{}** · {}\n\n",
        verdict_label(summary.verdict),
        counts.join(" · ")
    ));
    if !summary.summary.trim().is_empty() {
        md.push_str(summary.summary.trim());
        md.push_str("\n\n");
    }
    if !open.is_empty() {
        md.push_str(
            "> When you have dealt with a finding, tick one of its boxes, or reply on its line \
             starting with *Fixed*, *Not a problem* or *Won't fix* and why. The next review \
             checks it.\n\n",
        );
    }
    // The text people read stays well under GitHub's limit, leaving the
    // rest to the hidden block: past a point, details stay on the lines,
    // then the rest is only counted.
    let mut left_out = 0;
    for severity in crate::SEVERITIES {
        let group: Vec<&&Finding> = open.iter().filter(|f| f.severity == severity).collect();
        if group.is_empty() {
            continue;
        }
        md.push_str(&format!("#### {}\n\n", capitalize(severity)));
        for finding in group {
            if md.len() > SHOWN_MAX {
                left_out += 1;
                continue;
            }
            let brief = md.len() > SHOWN_MAX / 2;
            md.push_str(&render_open(finding, summary, brief));
        }
    }
    if left_out > 0 {
        md.push_str(&format!(
            "*And {left_out} more open, too many for one comment: see them in PilotHouse or on their lines.*\n\n"
        ));
    }
    if !done.is_empty() {
        md.push_str("#### Done\n\n");
        let mut done_left = 0;
        for finding in &done {
            if md.len() > SHOWN_MAX + 4_000 {
                done_left += 1;
                continue;
            }
            let why = match (finding.status.as_str(), finding.reason.trim()) {
                ("fixed", _) => "fixed".to_string(),
                (status, "") => status_label(status).to_lowercase(),
                (status, reason) => format!("{}: {reason}", status_label(status).to_lowercase()),
            };
            md.push_str(&format!(
                "- **{}** {} · {}\n",
                finding.label,
                finding.title.trim(),
                why
            ));
        }
        if done_left > 0 {
            md.push_str(&format!("- and {done_left} more\n"));
        }
        md.push('\n');
    }
    md.push_str(&format!(
        "<sub>Reviewed `{short}` with {}. Findings are AI-generated; check them before acting. Posted with PilotHouse.</sub>\n",
        if summary.agents.is_empty() { "an agent" } else { summary.agents }
    ));
    md.push_str(&data_block(summary, &md));
    md
}

fn render_open(finding: &Finding, summary: &Summary, brief: bool) -> String {
    let place = match (finding.line, finding.end_line) {
        (Some(start), Some(end)) if end > start => format!(
            "[`{}:{start}-{end}`]({}/blob/{}/{}#L{start}-L{end})",
            finding.path, summary.repo_url, summary.sha, finding.path
        ),
        (Some(start), _) => format!(
            "[`{}:{start}`]({}/blob/{}/{}#L{start})",
            finding.path, summary.repo_url, summary.sha, finding.path
        ),
        _ if !finding.path.is_empty() => format!("`{}`", finding.path),
        _ => String::new(),
    };
    let mut md = format!("**{}** · {} · {place}", finding.label, finding.title.trim());
    if finding.first_round == summary.round && summary.round > 1 {
        md.push_str(" · *new*");
    }
    md.push_str("\n\n");
    if let Some(comment) = finding
        .comment_id
        .filter(|_| finding.comment_kind.as_deref() == Some("line"))
    {
        md.push_str(&format!(
            "Details on the line: [comment]({}/pull/{}#discussion_r{comment}).\n\n",
            summary.repo_url, summary.number
        ));
    } else if !brief {
        if !finding.body.trim().is_empty() {
            md.push_str(&cut(finding.body.trim(), 1_500));
            md.push_str("\n\n");
        }
        let fix = fix_markdown(finding, "");
        if !fix.is_empty() {
            md.push_str(&cut(&fix, 2_500));
            md.push_str("\n\n");
        }
    }
    if !finding.note.is_empty() && summary.round > 1 {
        md.push_str(&format!("*{}*\n\n", finding.note.trim()));
    }
    for (status, label) in BOXES {
        md.push_str(&format!(
            "- [ ] {label} <!-- ph:{}:{status} -->\n",
            finding.label
        ));
    }
    md.push('\n');
    md
}

/// The hidden block, made smaller until the whole comment fits.
fn data_block(summary: &Summary, shown: &str) -> String {
    let mut shared = SharedReview {
        version: 1,
        round: summary.round,
        sha: summary.sha.into(),
        verdict: summary.verdict.into(),
        summary: cut(summary.summary, 2_000),
        reviewer: summary.reviewer.into(),
        findings: summary.findings.iter().map(Shared::of).collect(),
    };
    loop {
        let json = serde_json::to_vec(&shared).unwrap_or_default();
        let block = format!(
            "{DATA}{} -->\n",
            base64::engine::general_purpose::STANDARD.encode(json)
        );
        if shown.len() + block.len() <= MAX {
            return block;
        }
        // Shorter: done findings' details first, then everyone's quotes.
        if let Some(finding) = shared
            .findings
            .iter_mut()
            .find(|f| f.status != "open" && (!f.body.is_empty() || !f.evidence.is_empty()))
        {
            finding.body.clear();
            finding.evidence.clear();
            finding.suggestion.clear();
            finding.suggested_code.clear();
        } else if let Some(finding) = shared.findings.iter_mut().find(|f| {
            !f.evidence.is_empty() || !f.suggestion.is_empty() || !f.suggested_code.is_empty()
        }) {
            finding.evidence.clear();
            finding.suggestion.clear();
            finding.suggested_code.clear();
        } else if shared.findings.iter().any(|f| f.status != "open") {
            let at = shared
                .findings
                .iter()
                .rposition(|f| f.status != "open")
                .unwrap_or(0);
            shared.findings.remove(at);
        } else if let Some(finding) = shared
            .findings
            .iter_mut()
            .find(|f| !f.body.is_empty() || !f.note.is_empty())
        {
            finding.body.clear();
            finding.note.clear();
        } else if let Some(at) = shared
            .findings
            .iter()
            .rposition(|f| matches!(f.severity.as_str(), "nit" | "minor"))
            .or_else(|| shared.findings.len().checked_sub(1))
        {
            // Too many even as titles: the least serious go (they are still
            // in the text and on their lines).
            shared.findings.remove(at);
        } else {
            return String::new();
        }
    }
}

/// The findings in a summary comment, `None` when it is not one.
pub fn read_shared(body: &str) -> Option<SharedReview> {
    if !body.trim_start().starts_with(MARKER) {
        return None;
    }
    let start = body.find(DATA)? + DATA.len();
    let end = body[start..].find("-->")? + start;
    let json = base64::engine::general_purpose::STANDARD
        .decode(body[start..end].trim())
        .ok()?;
    serde_json::from_slice(&json).ok()
}

/// The boxes ticked in a summary comment: finding label → status ticked.
pub fn read_ticks(body: &str) -> BTreeMap<String, String> {
    let mut ticks = BTreeMap::new();
    for line in body.lines() {
        let line = line.trim();
        let ticked = line.starts_with("- [x]") || line.starts_with("- [X]");
        if !ticked {
            continue;
        }
        let Some(start) = line.find("<!-- ph:") else {
            continue;
        };
        let tag = &line[start + "<!-- ph:".len()..];
        let Some(end) = tag.find("-->") else { continue };
        let mut parts = tag[..end].trim().split(':');
        let (Some(label), Some(status)) = (parts.next(), parts.next()) else {
            continue;
        };
        if BOXES.iter().any(|(known, _)| *known == status) {
            ticks.insert(label.to_string(), status.to_string());
        }
    }
    ticks
}

/// The findings as plain Markdown, to paste anywhere (no boxes, no hidden
/// block): open ones first by severity, then the rest.
pub fn markdown(title: &str, summary: &str, findings: &[Finding]) -> String {
    let mut md = format!("## {title}\n\n");
    if !summary.trim().is_empty() {
        md.push_str(&format!("{}\n\n", summary.trim()));
    }
    let mut sorted: Vec<&Finding> = findings.iter().collect();
    sorted.sort_by_key(|f| {
        (
            f.status != "open",
            crate::SEVERITIES
                .iter()
                .position(|s| *s == f.severity)
                .unwrap_or(9),
        )
    });
    for finding in sorted {
        let place = match finding.line {
            Some(line) => format!("`{}:{line}`", finding.path),
            None if finding.path.is_empty() => String::new(),
            None => format!("`{}`", finding.path),
        };
        md.push_str(&format!(
            "- **{} · {}** {}{}{}\n",
            finding.label,
            capitalize(&finding.severity),
            finding.title.trim(),
            if place.is_empty() {
                String::new()
            } else {
                format!(" ({place})")
            },
            if finding.status == "open" {
                String::new()
            } else {
                format!(" · *{}*", status_label(&finding.status))
            }
        ));
        if !finding.body.trim().is_empty() {
            md.push_str(&format!(
                "  {}\n",
                finding.body.trim().replace('\n', "\n  ")
            ));
        }
        let fix = fix_markdown(finding, "");
        if !fix.is_empty() {
            md.push_str(&format!("\n  {}\n", indented(&fix, "  ")));
        }
    }
    md
}

pub fn verdict_label(verdict: &str) -> &'static str {
    match verdict {
        "request_changes" => "Changes requested",
        "approve" => "Looks good",
        _ => "Comments",
    }
}

pub fn status_label(status: &str) -> &'static str {
    match status {
        "fixed" => "Fixed",
        "not_problem" => "Not a problem",
        "wont_fix" => "Won't fix",
        _ => "Open",
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(label: &str, severity: &str, status: &str) -> Finding {
        Finding {
            label: label.into(),
            severity: severity.into(),
            status: status.into(),
            path: "src/a.rs".into(),
            line: Some(4),
            title: format!("Problem {label}"),
            body: "What is wrong.".into(),
            evidence: "log(token);".into(),
            first_round: 1,
            ..Finding::default()
        }
    }

    #[test]
    fn the_fix_is_posted_with_its_code_and_keeps_lists_whole() {
        let mut open = finding("F1", "major", "open");
        open.suggestion = "Check the index first.\nThen return early.".into();
        open.suggested_code = "```rust\n    if i >= n {\n        return;\n    }\n```".into();
        // An agent's own fence is dropped; the indentation stays.
        assert_eq!(
            unfenced(&open.suggested_code),
            "    if i >= n {\n        return;\n    }"
        );
        assert_eq!(unfenced("\nlet a = 1;\n"), "let a = 1;");
        let fix = fix_markdown(&open, "suggestion");
        assert!(fix.starts_with("**Fix:**\n\nCheck the index first.\nThen return early."));
        assert!(fix.ends_with("```suggestion\n    if i >= n {\n        return;\n    }\n```"));
        // A finding not on a line shows its fix in the summary.
        let md = render(&Summary {
            round: 1,
            sha: "0123456789abcdef",
            verdict: "request_changes",
            summary: "",
            reviewer: "ada",
            agents: "Claude Code",
            repo_url: "https://github.com/acme/app",
            number: 7,
            findings: std::slice::from_ref(&open),
        });
        assert!(md.contains("Then return early."), "{md}");
        assert!(md.contains("\n```\n    if i >= n {"), "{md}");
        // In the plain list, every line of the fix stays in its item.
        let list = markdown("Review", "", std::slice::from_ref(&open));
        assert!(list.contains("\n  Then return early."), "{list}");
        assert!(list.contains("\n  ```\n      if i >= n {"), "{list}");
        // The hidden block carries the code for other people's PilotHouse.
        let shared = read_shared(&md).unwrap();
        assert!(shared.findings[0].suggested_code.contains("return;"));
    }

    #[test]
    fn renders_boxes_and_reads_back_ticks_and_findings() {
        let mut fixed = finding("F2", "minor", "fixed");
        fixed.reason = String::new();
        let findings = [finding("F1", "major", "open"), fixed];
        let md = render(&Summary {
            round: 2,
            sha: "0123456789abcdef",
            verdict: "request_changes",
            summary: "One open.",
            reviewer: "ada",
            agents: "Claude Code",
            repo_url: "https://github.com/acme/app",
            number: 7,
            findings: &findings,
        });
        assert!(md.starts_with("<!-- pilothouse-review v1 sha=0123456789abcdef round=2 -->"));
        assert!(md.contains("- [ ] Not a problem <!-- ph:F1:not_problem -->"));
        assert!(md.contains("**F2** Problem F2 · fixed"));
        assert!(!md.contains("ph:F2:"), "done findings have no boxes");
        let shared = read_shared(&md).unwrap();
        assert_eq!(shared.reviewer, "ada");
        assert_eq!(shared.findings.len(), 2);
        assert_eq!(shared.findings[0].to_finding().label, "F1");

        let ticked = md.replace(
            "- [ ] Won't fix <!-- ph:F1:wont_fix -->",
            "- [x] Won't fix <!-- ph:F1:wont_fix -->",
        );
        assert_eq!(
            read_ticks(&ticked).get("F1").map(String::as_str),
            Some("wont_fix")
        );
        assert!(read_ticks("- [x] plain task\n- [x] x <!-- ph:F9:nonsense -->").is_empty());
        assert_eq!(read_shared("A comment"), None);
    }

    #[test]
    fn a_big_review_still_fits_in_one_comment() {
        let findings: Vec<Finding> = (1..400)
            .map(|n| {
                let mut f = finding(
                    &format!("F{n}"),
                    "minor",
                    if n % 2 == 0 { "fixed" } else { "open" },
                );
                f.body = "x".repeat(300);
                f.evidence = "y".repeat(600);
                f
            })
            .collect();
        let md = render(&Summary {
            round: 1,
            sha: "abc",
            verdict: "comment",
            summary: "",
            reviewer: "",
            agents: "",
            repo_url: "https://github.com/a/b",
            number: 1,
            findings: &findings,
        });
        assert!(md.len() < 65_536, "{}", md.len());
        assert!(
            read_shared(&md).is_some(),
            "the findings are still there, shorter"
        );
    }
}
