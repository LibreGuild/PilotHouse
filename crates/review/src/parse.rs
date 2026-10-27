//! Reading the agent's review out of its reply.

use serde::Deserialize;
use serde_json::Value;

/// What the agent reported.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    pub summary: String,
    /// `approve`, `comment` or `request_changes`.
    pub verdict: String,
    pub findings: Vec<ReportedFinding>,
    /// Keys of previous findings the agent checked and found fixed (older
    /// prompts; see `checks`).
    pub resolved: Vec<String>,
    /// The agent's word on each earlier finding it was asked about.
    pub checks: Vec<Check>,
}

/// One earlier finding checked again: still there or not (a check of the
/// fixes), or still holding up or not (a double-check).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Check {
    /// `F3`.
    pub label: String,
    /// Fixed (a check of the fixes) or refuted (a double-check): the
    /// finding no longer stands.
    pub gone: bool,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct ReportedFinding {
    pub key: String,
    pub severity: String,
    pub path: String,
    pub line: Option<u64>,
    pub end_line: Option<u64>,
    pub title: String,
    pub details: String,
    pub evidence: String,
    pub suggestion: String,
    /// What goes wrong and how often (see [`crate::severity`]).
    pub impact: String,
    pub likelihood: String,
    /// Replacement code for exactly the quoted lines.
    pub suggested_code: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Raw {
    summary: String,
    verdict: String,
    findings: Vec<Value>,
    resolved: Vec<Value>,
    checks: Vec<Value>,
}

/// Finds the review block in `reply` (the last fenced JSON block with
/// `findings`), and tidies what it holds. `None` when there is none.
pub fn parse(reply: &str) -> Option<Report> {
    let raw = json_blocks(reply)
        .into_iter()
        .rev()
        .filter_map(|block| serde_json::from_str::<Raw>(&tolerant(block)).ok())
        .find(|raw| {
            !raw.findings.is_empty() || !raw.summary.is_empty() || !raw.checks.is_empty()
        })?;
    let mut findings: Vec<ReportedFinding> = raw
        .findings
        .into_iter()
        .filter_map(|value| finding(&value))
        .collect();
    findings.sort_by_key(|f| crate::severity_rank(&f.severity));
    Some(Report {
        summary: raw.summary.trim().to_string(),
        verdict: match raw
            .verdict
            .to_ascii_lowercase()
            .replace([' ', '-'], "_")
            .as_str()
        {
            "approve" | "approved" | "lgtm" => "approve".into(),
            "request_changes" | "changes_requested" | "reject" => "request_changes".into(),
            _ => "comment".into(),
        },
        findings,
        resolved: raw
            .resolved
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        checks: raw.checks.iter().filter_map(check).collect(),
    })
}

/// `{"label": "F3", "fixed": true, "note": …}`, or `holds`, or a `status`
/// word (`fixed`, `not_fixed`, `refuted`…).
fn check(value: &Value) -> Option<Check> {
    let object = value.as_object()?;
    let label = ["label", "id", "key"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))?
        .trim()
        .to_ascii_uppercase();
    let word = |key: &str| object.get(key).and_then(Value::as_bool);
    let gone = word("fixed")
        .or_else(|| word("holds").map(|holds| !holds))
        .or_else(|| {
            let status = object.get("status")?.as_str()?.to_ascii_lowercase();
            Some(matches!(
                status.as_str(),
                "fixed" | "resolved" | "gone" | "refuted" | "false_positive" | "not_a_problem"
            ))
        })?;
    let note = ["note", "reason", "why"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
        .to_string();
    Some(Check { label, gone, note })
}

fn finding(value: &Value) -> Option<ReportedFinding> {
    // Agents write line numbers as numbers, strings, or a range ("10-14"):
    // read those by hand.
    let mut fields = value.as_object()?.clone();
    let line = fields.remove("line");
    let end_line = fields.remove("end_line");
    let mut finding: ReportedFinding = serde_json::from_value(Value::Object(fields)).ok()?;
    let (start, range_end) = line_range(line.as_ref());
    finding.line = start;
    finding.end_line = line_range(end_line.as_ref()).0.or(range_end);
    if finding.details.is_empty() {
        finding.details = ["description", "body", "explanation"]
            .iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
    }
    // The severity is read from what goes wrong and how often, when given.
    finding.severity = match (
        crate::severity::Impact::parse(&finding.impact),
        crate::severity::Likelihood::parse(&finding.likelihood),
    ) {
        (Some(impact), Some(likelihood)) => {
            finding.impact = impact.as_str().into();
            finding.likelihood = likelihood.as_str().into();
            crate::severity::severity_for(impact, likelihood).into()
        }
        _ => normalize_severity(&finding.severity),
    };
    finding.path = finding.path.trim().trim_start_matches("./").to_string();
    finding.title = finding.title.trim().to_string();
    if finding.title.is_empty() && finding.details.is_empty() {
        return None;
    }
    if finding.title.is_empty() {
        finding.title = finding
            .details
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(80)
            .collect();
    }
    if finding.key.trim().is_empty() {
        finding.key = fingerprint(&finding.path, &finding.title);
    }
    Some(finding)
}

/// `42`, `"42"` or `"10-14"` as (start, end).
fn line_range(value: Option<&Value>) -> (Option<u64>, Option<u64>) {
    match value {
        Some(Value::Number(number)) => (number.as_u64(), None),
        Some(Value::String(text)) => {
            let mut parts = text
                .split(['-', '–', ':'])
                .map(|part| part.trim().parse::<u64>().ok());
            (parts.next().flatten(), parts.next().flatten())
        }
        _ => (None, None),
    }
}

fn normalize_severity(severity: &str) -> String {
    match severity.trim().to_ascii_lowercase().as_str() {
        "critical" | "blocker" | "severe" => "critical",
        "major" | "high" | "important" | "error" => "major",
        "minor" | "medium" | "low" | "warning" => "minor",
        _ => "nit",
    }
    .into()
}

/// A key from the file and title, for findings without one.
pub fn fingerprint(path: &str, title: &str) -> String {
    let words: String = title
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    let file = path.rsplit('/').next().unwrap_or(path);
    format!("{file}:{words}")
}

/// The JSON values of `text`'s fenced blocks (see [`json_blocks`]).
pub(crate) fn json_values(text: &str) -> Vec<Value> {
    json_blocks(text)
        .into_iter()
        .filter_map(|block| serde_json::from_str::<Value>(&tolerant(block)).ok())
        .collect()
}

/// `block` without trailing commas before `}` or `]` (agents write them).
fn tolerant(block: &str) -> String {
    let mut out = String::with_capacity(block.len());
    let chars: Vec<char> = block.chars().collect();
    let mut in_string = false;
    let mut escaped = false;
    for (at, c) in chars.iter().enumerate() {
        if in_string {
            out.push(*c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        if *c == '"' {
            in_string = true;
        }
        if *c == ',' {
            let next = chars[at + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        out.push(*c);
    }
    out
}

/// The contents of fenced code blocks tagged json (or holding JSON).
fn json_blocks(text: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let Some(newline) = after.find('\n') else {
            break;
        };
        let info = after[..newline].trim().to_ascii_lowercase();
        let body = &after[newline + 1..];
        let Some(end) = body.find("```") else { break };
        if info.is_empty()
            || info.starts_with("json")
            || info.contains("review")
            || info.contains("pilothouse")
        {
            blocks.push(body[..end].trim());
        }
        rest = &body[end + 3..];
    }
    // A reply that is only the JSON, without a fence.
    if blocks.is_empty() && text.trim_start().starts_with('{') {
        blocks.push(text.trim());
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"I read the diff and the call sites.

```json pilothouse-review
{
  "summary": "The retry loop can spin forever.",
  "verdict": "Request changes",
  "findings": [
    {"key": "retry-forever", "severity": "nit", "path": "./src/app.py", "line": 3, "title": "Name the constant", "details": "3 is a magic number."},
    {"severity": "High", "path": "src/app.py", "line": "10-14", "title": "Unbounded retry", "description": "No backoff."},
    {"severity": "critical", "path": "src/auth.py", "title": "", "details": "Token logged in plain text.\nSee line 40."}
  ],
  "resolved": ["old-key"]
}
```"#;

    #[test]
    fn reads_and_tidies_the_review_block() {
        let report = parse(REPLY).unwrap();
        assert_eq!(report.verdict, "request_changes");
        assert_eq!(report.resolved, ["old-key"]);
        let severities: Vec<_> = report
            .findings
            .iter()
            .map(|f| f.severity.as_str())
            .collect();
        assert_eq!(severities, ["critical", "major", "nit"]);
        let critical = &report.findings[0];
        assert_eq!(critical.title, "Token logged in plain text.");
        assert_eq!(critical.key, "auth.py:token-logged-in-plain-text");
        let major = &report.findings[1];
        assert_eq!((major.line, major.end_line), (Some(10), Some(14)));
        assert_eq!(major.details, "No backoff.");
        assert_eq!(report.findings[2].path, "src/app.py");
    }

    #[test]
    fn ignores_replies_without_a_review() {
        assert_eq!(parse("Just chatting, no review here."), None);
        assert_eq!(parse("```rust\nfn main() {}\n```"), None);
        let bare = parse(r#"{"summary": "Fine", "verdict": "approve", "findings": []}"#).unwrap();
        assert_eq!((bare.verdict.as_str(), bare.findings.len()), ("approve", 0));
    }

    #[test]
    fn reads_checks_and_works_out_the_severity() {
        let reply = r#"```json pilothouse-review
{"summary": "s", "findings": [
  {"path": "a.rs", "line": 3, "title": "Leaks the token", "impact": "security", "likelihood": "rare", "severity": "nit"}
 ],
 "checks": [{"label": "f1", "fixed": true, "note": "Now checked"}, {"label": "F2", "holds": false},
            {"id": "F3", "status": "not_fixed"}]}
```"#;
        let report = parse(reply).unwrap();
        assert_eq!(
            report.findings[0].severity, "major",
            "from the two answers, not the agent's"
        );
        assert_eq!(
            report.checks,
            vec![
                Check {
                    label: "F1".into(),
                    gone: true,
                    note: "Now checked".into()
                },
                Check {
                    label: "F2".into(),
                    gone: true,
                    note: String::new()
                },
                Check {
                    label: "F3".into(),
                    gone: false,
                    note: String::new()
                },
            ]
        );
    }
}
