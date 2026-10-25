//! Pull request reviews by an agent, and their findings. Each run is a row;
//! findings carry a stable `key` so the next run can say which are fixed,
//! and the GitHub comment they were posted as, so it can be updated.

use anyhow::Result;
use rusqlite::{OptionalExtension as _, Row, params};

use crate::Store;
use crate::projects::ProjectId;
use crate::sessions::SessionId;
use crate::time::now_millis;

/// Which pull request a review is of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRef {
    pub host: String,
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrReview {
    pub id: i64,
    pub project_id: ProjectId,
    pub pull: PullRef,
    pub title: String,
    pub head_sha: String,
    pub session_id: Option<SessionId>,
    pub summary: String,
    /// `approve`, `comment` or `request_changes`, as the agent judged it.
    pub verdict: String,
    pub finished: bool,
    pub created_at: i64,
}

/// One finding of a review.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub id: i64,
    pub review_id: i64,
    /// Stable across runs (the agent's id, or one made from path and title).
    pub key: String,
    /// `critical`, `major`, `minor` or `nit`.
    pub severity: String,
    pub path: String,
    pub line: Option<u64>,
    pub end_line: Option<u64>,
    pub title: String,
    /// What is wrong and why it matters.
    pub body: String,
    /// What shows it: the code, a call path, a failing case.
    pub evidence: String,
    /// How to fix it.
    pub suggestion: String,
    /// `new`, `open` (seen before, still there), `fixed` or `dismissed`.
    pub status: String,
    /// The GitHub comment it was posted as, and whether that is a `line`
    /// comment or one `conversation` comment.
    pub comment_id: Option<u64>,
    pub comment_kind: Option<String>,
    /// Whether GitHub was told it is fixed.
    pub fixed_notified: bool,
}

impl PrReview {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            project_id: row.get("project_id")?,
            pull: PullRef {
                host: row.get("host")?,
                owner: row.get("owner")?,
                repo: row.get("repo")?,
                number: row.get::<_, i64>("number")? as u64,
            },
            title: row.get("title")?,
            head_sha: row.get("head_sha")?,
            session_id: row.get("session_id")?,
            summary: row.get("summary")?,
            verdict: row.get("verdict")?,
            finished: row.get::<_, i64>("finished")? != 0,
            created_at: row.get("created_at")?,
        })
    }
}

impl Finding {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let number = |name: &str| {
            row.get::<_, Option<i64>>(name)
                .map(|value| value.map(|v| v as u64))
        };
        Ok(Self {
            id: row.get("id")?,
            review_id: row.get("review_id")?,
            key: row.get("key")?,
            severity: row.get("severity")?,
            path: row.get("path")?,
            line: number("line")?,
            end_line: number("end_line")?,
            title: row.get("title")?,
            body: row.get("body")?,
            evidence: row.get("evidence")?,
            suggestion: row.get("suggestion")?,
            status: row.get("status")?,
            comment_id: number("comment_id")?,
            comment_kind: row.get("comment_kind")?,
            fixed_notified: row.get::<_, i64>("fixed_notified")? != 0,
        })
    }
}

impl Store {
    /// Starts a review run (its findings come with [`Store::finish_pr_review`]).
    pub fn create_pr_review(
        &self,
        project_id: ProjectId,
        pull: &PullRef,
        title: &str,
        head_sha: &str,
    ) -> Result<PrReview> {
        let now = now_millis();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO pr_reviews (project_id, host, owner, repo, number, title, head_sha, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![project_id, pull.host, pull.owner, pull.repo, pull.number as i64, title, head_sha, now],
        )?;
        let id = conn.last_insert_rowid();
        drop(conn);
        Ok(self.pr_review(id)?.expect("just inserted"))
    }

    pub fn pr_review(&self, id: i64) -> Result<Option<PrReview>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT * FROM pr_reviews WHERE id = ?1",
                params![id],
                PrReview::from_row,
            )
            .optional()?)
    }

    pub fn pr_review_for_session(&self, session_id: SessionId) -> Result<Option<PrReview>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT * FROM pr_reviews WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
                params![session_id],
                PrReview::from_row,
            )
            .optional()?)
    }

    pub fn set_pr_review_session(&self, id: i64, session_id: SessionId) -> Result<()> {
        self.conn().execute(
            "UPDATE pr_reviews SET session_id = ?2 WHERE id = ?1",
            params![id, session_id],
        )?;
        Ok(())
    }

    /// The newest finished review of `pull` (before `before`, if given).
    pub fn latest_pr_review(
        &self,
        pull: &PullRef,
        before: Option<i64>,
    ) -> Result<Option<PrReview>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT * FROM pr_reviews
                 WHERE host = ?1 AND owner = ?2 AND repo = ?3 AND number = ?4
                   AND finished = 1 AND id < ?5
                 ORDER BY id DESC LIMIT 1",
                params![
                    pull.host,
                    pull.owner,
                    pull.repo,
                    pull.number as i64,
                    before.unwrap_or(i64::MAX)
                ],
                PrReview::from_row,
            )
            .optional()?)
    }

    /// Saves what the agent found and marks the run finished.
    pub fn finish_pr_review(
        &self,
        id: i64,
        summary: &str,
        verdict: &str,
        findings: &[Finding],
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM pr_findings WHERE review_id = ?1", params![id])?;
        for finding in findings {
            tx.execute(
                "INSERT INTO pr_findings (review_id, key, severity, path, line, end_line, title, body,
                    suggestion, status, comment_id, comment_kind, fixed_notified, evidence)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    id,
                    finding.key,
                    finding.severity,
                    finding.path,
                    finding.line.map(|v| v as i64),
                    finding.end_line.map(|v| v as i64),
                    finding.title,
                    finding.body,
                    finding.suggestion,
                    finding.status,
                    finding.comment_id.map(|v| v as i64),
                    finding.comment_kind,
                    finding.fixed_notified as i64,
                    finding.evidence,
                ],
            )?;
        }
        tx.execute(
            "UPDATE pr_reviews SET summary = ?2, verdict = ?3, finished = 1 WHERE id = ?1",
            params![id, summary, verdict],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn pr_findings(&self, review_id: i64) -> Result<Vec<Finding>> {
        let conn = self.conn();
        let mut statement =
            conn.prepare("SELECT * FROM pr_findings WHERE review_id = ?1 ORDER BY id")?;
        let findings = statement
            .query_map(params![review_id], Finding::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(findings)
    }

    pub fn set_finding_status(&self, id: i64, status: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE pr_findings SET status = ?2 WHERE id = ?1",
            params![id, status],
        )?;
        Ok(())
    }

    pub fn set_finding_comment(&self, id: i64, comment_id: u64, kind: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE pr_findings SET comment_id = ?2, comment_kind = ?3 WHERE id = ?1",
            params![id, comment_id as i64, kind],
        )?;
        Ok(())
    }

    pub fn set_finding_notified(&self, id: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE pr_findings SET fixed_notified = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(key: &str, status: &str) -> Finding {
        Finding {
            id: 0,
            review_id: 0,
            key: key.into(),
            severity: "major".into(),
            path: "src/a.rs".into(),
            line: Some(3),
            end_line: None,
            title: format!("Problem {key}"),
            body: "Details".into(),
            evidence: String::new(),
            suggestion: String::new(),
            status: status.into(),
            comment_id: None,
            comment_kind: None,
            fixed_notified: false,
        }
    }

    #[test]
    fn reviews_keep_findings_per_run() {
        let store = Store::open_in_memory().unwrap();
        let folder = std::env::temp_dir().join(format!("ph-review-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let project = store.add_project(&folder).unwrap();
        let pull = PullRef {
            host: "github.com".into(),
            owner: "acme".into(),
            repo: "app".into(),
            number: 7,
        };
        let first = store
            .create_pr_review(project.id, &pull, "Fix", "abc")
            .unwrap();
        assert!(
            store.latest_pr_review(&pull, None).unwrap().is_none(),
            "unfinished runs do not count"
        );
        store
            .finish_pr_review(
                first.id,
                "Two issues",
                "request_changes",
                &[finding("a", "new"), finding("b", "new")],
            )
            .unwrap();
        let findings = store.pr_findings(first.id).unwrap();
        assert_eq!(findings.len(), 2);
        store
            .set_finding_comment(findings[0].id, 55, "line")
            .unwrap();
        store
            .set_finding_status(findings[1].id, "dismissed")
            .unwrap();

        let second = store
            .create_pr_review(project.id, &pull, "Fix", "def")
            .unwrap();
        let latest = store
            .latest_pr_review(&pull, Some(second.id))
            .unwrap()
            .unwrap();
        assert_eq!(
            (latest.id, latest.verdict.as_str()),
            (first.id, "request_changes")
        );
        let again = store.pr_findings(first.id).unwrap();
        assert_eq!(again[0].comment_id, Some(55));
        assert_eq!(again[1].status, "dismissed");
    }
}
