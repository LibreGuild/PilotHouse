//! Carrying findings from one review run to the next.

use ph_store::reviews::Finding;

use crate::parse::Report;

/// The findings of a new run: what the agent reported, matched by key to
/// the previous run.
///
/// - Reported again: `open` (or still `dismissed`), keeping its GitHub
///   comment.
/// - Reported for the first time: `new`.
/// - Previous ones the agent says are resolved: `fixed`.
/// - Previous ones it did not mention: kept as they were (not re-checked).
pub fn merge(review_id: i64, report: &Report, previous: &[Finding]) -> Vec<Finding> {
    let mut findings: Vec<Finding> = report
        .findings
        .iter()
        .map(|reported| {
            let before = previous.iter().find(|old| old.key == reported.key);
            let status = match before.map(|old| old.status.as_str()) {
                Some("dismissed") => "dismissed",
                Some(_) => "open",
                None => "new",
            };
            Finding {
                id: 0,
                review_id,
                key: reported.key.clone(),
                severity: reported.severity.clone(),
                path: reported.path.clone(),
                line: reported.line,
                end_line: reported.end_line,
                title: reported.title.clone(),
                body: reported.details.clone(),
                evidence: reported.evidence.clone(),
                suggestion: reported.suggestion.clone(),
                status: status.into(),
                comment_id: before.and_then(|old| old.comment_id),
                comment_kind: before.and_then(|old| old.comment_kind.clone()),
                fixed_notified: false,
            }
        })
        .collect();
    for old in previous {
        if findings.iter().any(|finding| finding.key == old.key) {
            continue;
        }
        let mut carried = old.clone();
        carried.id = 0;
        carried.review_id = review_id;
        if report.resolved.contains(&old.key) {
            carried.status = "fixed".into();
            // A newly fixed finding has not been announced yet.
            if old.status != "fixed" {
                carried.fixed_notified = false;
            }
        }
        findings.push(carried);
    }
    findings.sort_by_key(|f| (f.status == "fixed", crate::severity_rank(&f.severity)));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::ReportedFinding;

    fn old(key: &str, status: &str, comment: Option<u64>) -> Finding {
        Finding {
            id: 9,
            review_id: 1,
            key: key.into(),
            severity: "major".into(),
            path: "a.rs".into(),
            line: Some(1),
            end_line: None,
            title: key.into(),
            body: String::new(),
            evidence: String::new(),
            suggestion: String::new(),
            status: status.into(),
            comment_id: comment,
            comment_kind: comment.map(|_| "line".into()),
            fixed_notified: false,
        }
    }

    fn reported(key: &str) -> ReportedFinding {
        ReportedFinding {
            key: key.into(),
            severity: "minor".into(),
            path: "a.rs".into(),
            title: key.into(),
            ..ReportedFinding::default()
        }
    }

    #[test]
    fn marks_new_open_fixed_and_dismissed() {
        let previous = [
            old("still", "new", Some(11)),
            old("fixed", "open", Some(12)),
            old("ignored", "dismissed", None),
            old("unchecked", "new", None),
        ];
        let report = Report {
            findings: vec![
                reported("still"),
                reported("brand-new"),
                reported("ignored"),
            ],
            resolved: vec!["fixed".into()],
            ..Report::default()
        };
        let merged = merge(2, &report, &previous);
        let status = |key: &str| {
            merged
                .iter()
                .find(|f| f.key == key)
                .map(|f| f.status.clone())
                .unwrap()
        };
        assert_eq!(status("still"), "open");
        assert_eq!(status("brand-new"), "new");
        assert_eq!(status("ignored"), "dismissed");
        assert_eq!(status("fixed"), "fixed");
        assert_eq!(status("unchecked"), "new");
        // Comments travel with the finding; everything belongs to run 2.
        assert_eq!(
            merged.iter().find(|f| f.key == "still").unwrap().comment_id,
            Some(11)
        );
        assert!(merged.iter().all(|f| f.review_id == 2 && f.id == 0));
        // Fixed ones sort last.
        assert_eq!(merged.last().unwrap().key, "fixed");
    }
}
