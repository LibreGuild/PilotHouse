//! Pull request reviews by an agent, and their findings. Each run is a row
//! (a *round*: a full review, a check of the fixes, a second opinion…);
//! the newest finished round holds the pull request's findings (its
//! ledger), each with a label (`F3`) kept across rounds, a status saying
//! who set it and why, and the GitHub comment it was posted as. A
//! walkthrough is a row too (kind `walkthrough`), outside the rounds.

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
    /// `approve`, `comment` or `request_changes`, worked out from the open
    /// findings.
    pub verdict: String,
    pub finished: bool,
    pub created_at: i64,
    /// 1 for the first round of the pull request (0: not a round).
    pub round: u32,
    /// `full`, `followup` (check the fixes), `second` (another agent),
    /// `double_check`, `self` (a branch before its pull request) or
    /// `walkthrough`.
    pub kind: String,
    /// The agent that ran it (its id).
    pub agent: String,
    /// The pull request's summary comment, once posted.
    pub summary_comment_id: Option<u64>,
    /// Kind-specific JSON (a walkthrough's content).
    pub data: String,
}

/// The kinds of review run that make up a pull request's rounds.
pub const ROUND_KINDS: &str = "('full', 'followup', 'second', 'double_check', 'self')";

/// One finding of a review.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Finding {
    pub id: i64,
    pub review_id: i64,
    /// The agent's id for it, or one made from path and title.
    pub key: String,
    /// `F1`, `F2`…: the pull request's name for it, kept across rounds.
    pub label: String,
    /// `critical`, `major`, `minor` or `nit`.
    pub severity: String,
    /// What goes wrong and how often, which the severity is read from
    /// (empty for findings from before).
    pub impact: String,
    pub likelihood: String,
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
    /// Replacement code for exactly the quoted lines, when the agent gave
    /// one (posted as a one-click GitHub suggestion).
    pub suggested_code: String,
    /// `open`, `fixed`, `not_problem` or `wont_fix`.
    pub status: String,
    /// Who set the status: `agent`, `me`, `author` (a reply or tick on
    /// GitHub), or `double_check`; and their name, and why.
    pub status_source: String,
    pub status_by: String,
    pub reason: String,
    /// The round it was first found in.
    pub first_round: u32,
    /// Found in code that had not changed since the round before (an
    /// earlier round missed it).
    pub found_late: bool,
    /// Whether the code it quotes was found in the file.
    pub verified: bool,
    /// The agents that found it, comma-separated.
    pub agents: String,
    /// The last check's word on it ("still there: …", "refuted: …").
    pub note: String,
    /// A GitHub issue tracking it.
    pub issue_number: Option<u64>,
    /// The newest reply on GitHub already taken into account.
    pub last_reply_id: Option<u64>,
    /// An agent's view of the author's last answer: `agree` or `disagree`
    /// (empty when not asked), and the reply it drafted for them.
    pub claim: String,
    pub draft_reply: String,
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
            round: row.get::<_, i64>("round")?.max(0) as u32,
            kind: row.get("kind")?,
            agent: row.get("agent")?,
            summary_comment_id: row
                .get::<_, Option<i64>>("summary_comment_id")?
                .map(|id| id as u64),
            data: row.get("data")?,
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
            label: row.get("label")?,
            severity: row.get("severity")?,
            impact: row.get("impact")?,
            likelihood: row.get("likelihood")?,
            path: row.get("path")?,
            line: number("line")?,
            end_line: number("end_line")?,
            title: row.get("title")?,
            body: row.get("body")?,
            evidence: row.get("evidence")?,
            suggestion: row.get("suggestion")?,
            suggested_code: row.get("suggested_code")?,
            status: row.get("status")?,
            status_source: row.get("status_source")?,
            status_by: row.get("status_by")?,
            reason: row.get("reason")?,
            first_round: row.get::<_, i64>("first_round")?.max(1) as u32,
            found_late: row.get::<_, i64>("found_late")? != 0,
            verified: row.get::<_, i64>("verified")? != 0,
            agents: row.get("agents")?,
            note: row.get("note")?,
            issue_number: number("issue_number")?,
            last_reply_id: number("last_reply_id")?,
            claim: row.get("claim")?,
            draft_reply: row.get("draft_reply")?,
            comment_id: number("comment_id")?,
            comment_kind: row.get("comment_kind")?,
            fixed_notified: row.get::<_, i64>("fixed_notified")? != 0,
        })
    }
}

impl Store {
    /// Starts a review run of kind `kind` by agent `agent` (its findings come
    /// with [`Store::finish_pr_review`]). A round is numbered after the
    /// pull request's newest one; a walkthrough is round 0.
    pub fn create_pr_run(
        &self,
        project_id: ProjectId,
        pull: &PullRef,
        title: &str,
        head_sha: &str,
        kind: &str,
        agent: &str,
    ) -> Result<PrReview> {
        let round = if kind == "walkthrough" {
            0
        } else {
            self.latest_pr_review(pull, None)?
                .map_or(1, |latest| latest.round + 1)
        };
        let now = now_millis();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO pr_reviews (project_id, host, owner, repo, number, title, head_sha,
                created_at, round, kind, agent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                project_id,
                pull.host,
                pull.owner,
                pull.repo,
                pull.number as i64,
                title,
                head_sha,
                now,
                round,
                kind,
                agent
            ],
        )?;
        let id = conn.last_insert_rowid();
        drop(conn);
        Ok(self.pr_review(id)?.expect("just inserted"))
    }

    /// A full review round by an unnamed agent (see [`Store::create_pr_run`]).
    pub fn create_pr_review(
        &self,
        project_id: ProjectId,
        pull: &PullRef,
        title: &str,
        head_sha: &str,
    ) -> Result<PrReview> {
        self.create_pr_run(project_id, pull, title, head_sha, "full", "")
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

    /// The newest finished round of `pull` (before `before`, if given): it
    /// holds the pull request's findings.
    pub fn latest_pr_review(
        &self,
        pull: &PullRef,
        before: Option<i64>,
    ) -> Result<Option<PrReview>> {
        Ok(self
            .conn()
            .query_row(
                &format!(
                    "SELECT * FROM pr_reviews
                     WHERE host = ?1 AND owner = ?2 AND repo = ?3 AND number = ?4
                       AND finished = 1 AND id < ?5 AND kind IN {ROUND_KINDS}
                     ORDER BY id DESC LIMIT 1"
                ),
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

    /// The newest finished run of `kind` (a walkthrough) of `pull`.
    pub fn latest_pr_run(&self, pull: &PullRef, kind: &str) -> Result<Option<PrReview>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT * FROM pr_reviews
                 WHERE host = ?1 AND owner = ?2 AND repo = ?3 AND number = ?4
                   AND finished = 1 AND kind = ?5
                 ORDER BY id DESC LIMIT 1",
                params![pull.host, pull.owner, pull.repo, pull.number as i64, kind],
                PrReview::from_row,
            )
            .optional()?)
    }

    /// Every finished round of every pull request of `host/owner/repo`, the
    /// newest round of each pull request only (for the stats).
    pub fn latest_rounds_of_repo(
        &self,
        host: &str,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<PrReview>> {
        let conn = self.conn();
        let mut statement = conn.prepare(&format!(
            "SELECT * FROM pr_reviews r
             WHERE host = ?1 AND owner = ?2 AND repo = ?3 AND finished = 1
               AND kind IN {ROUND_KINDS}
               AND id = (SELECT max(id) FROM pr_reviews o
                         WHERE o.host = r.host AND o.owner = r.owner AND o.repo = r.repo
                           AND o.number = r.number AND o.finished = 1
                           AND o.kind IN {ROUND_KINDS})
             ORDER BY id DESC"
        ))?;
        let reviews = statement
            .query_map(params![host, owner, repo], PrReview::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(reviews)
    }

    /// The repositories with reviews, as (host, owner, repo).
    pub fn reviewed_repos(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT DISTINCT host, owner, repo FROM pr_reviews WHERE finished = 1 AND host != 'local'
             ORDER BY owner, repo",
        )?;
        let repos = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(repos)
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
                    suggestion, status, comment_id, comment_kind, fixed_notified, evidence,
                    label, impact, likelihood, status_source, status_by, reason, first_round,
                    found_late, verified, agents, note, issue_number, last_reply_id, suggested_code,
                    claim, draft_reply)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
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
                    finding.label,
                    finding.impact,
                    finding.likelihood,
                    finding.status_source,
                    finding.status_by,
                    finding.reason,
                    finding.first_round,
                    finding.found_late as i64,
                    finding.verified as i64,
                    finding.agents,
                    finding.note,
                    finding.issue_number.map(|v| v as i64),
                    finding.last_reply_id.map(|v| v as i64),
                    finding.suggested_code,
                    finding.claim,
                    finding.draft_reply,
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

    /// Finishes a run that has no findings (a walkthrough) with `data`.
    pub fn finish_pr_run(&self, id: i64, summary: &str, data: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE pr_reviews SET summary = ?2, data = ?3, finished = 1 WHERE id = ?1",
            params![id, summary, data],
        )?;
        Ok(())
    }

    /// Changes the verdict of round `id` (its findings changed).
    pub fn set_pr_review_verdict(&self, id: i64, verdict: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE pr_reviews SET verdict = ?2 WHERE id = ?1",
            params![id, verdict],
        )?;
        Ok(())
    }

    /// The pull request's summary comment, posted from round `id`.
    pub fn set_summary_comment(&self, id: i64, comment_id: u64) -> Result<()> {
        self.conn().execute(
            "UPDATE pr_reviews SET summary_comment_id = ?2 WHERE id = ?1",
            params![id, comment_id as i64],
        )?;
        Ok(())
    }

    /// The summary comment of `pull`, from any round.
    pub fn summary_comment_of(&self, pull: &PullRef) -> Result<Option<u64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT summary_comment_id FROM pr_reviews
                 WHERE host = ?1 AND owner = ?2 AND repo = ?3 AND number = ?4
                   AND summary_comment_id IS NOT NULL
                 ORDER BY id DESC LIMIT 1",
                params![pull.host, pull.owner, pull.repo, pull.number as i64],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(|id| id as u64))
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

    pub fn pr_finding(&self, id: i64) -> Result<Option<Finding>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT * FROM pr_findings WHERE id = ?1",
                params![id],
                Finding::from_row,
            )
            .optional()?)
    }

    /// Saves the parts of a finding that change after its round: status
    /// (with who and why), severity, note, posting, issue, replies read.
    pub fn update_finding(&self, finding: &Finding) -> Result<()> {
        self.conn().execute(
            "UPDATE pr_findings SET status = ?2, status_source = ?3, status_by = ?4, reason = ?5,
                severity = ?6, note = ?7, comment_id = ?8, comment_kind = ?9, fixed_notified = ?10,
                issue_number = ?11, last_reply_id = ?12, verified = ?13, claim = ?14,
                draft_reply = ?15
             WHERE id = ?1",
            params![
                finding.id,
                finding.status,
                finding.status_source,
                finding.status_by,
                finding.reason,
                finding.severity,
                finding.note,
                finding.comment_id.map(|v| v as i64),
                finding.comment_kind,
                finding.fixed_notified as i64,
                finding.issue_number.map(|v| v as i64),
                finding.last_reply_id.map(|v| v as i64),
                finding.verified as i64,
                finding.claim,
                finding.draft_reply,
            ],
        )?;
        Ok(())
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
            ..Finding::default()
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
                &[finding("a", "open"), finding("b", "open")],
            )
            .unwrap();
        let findings = store.pr_findings(first.id).unwrap();
        assert_eq!(findings.len(), 2);
        store
            .set_finding_comment(findings[0].id, 55, "line")
            .unwrap();
        store
            .set_finding_status(findings[1].id, "not_problem")
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
        assert_eq!(again[1].status, "not_problem");
        assert_eq!((first.round, second.round), (1, 2), "rounds are numbered");

        // Every field a round carries on is kept, and read back as written.
        let mut carried = again[1].clone();
        carried.claim = "agree".into();
        carried.draft_reply = "Thanks, you are right.".into();
        carried.status_source = "author".into();
        carried.reason = "checked upstream".into();
        carried.suggested_code = "x = 1".into();
        carried.found_late = true;
        carried.verified = true;
        carried.issue_number = Some(9);
        carried.last_reply_id = Some(77);
        store
            .finish_pr_review(second.id, "", "comment", std::slice::from_ref(&carried))
            .unwrap();
        let read = &store.pr_findings(second.id).unwrap()[0];
        assert_eq!(
            Finding {
                id: read.id,
                review_id: read.review_id,
                ..carried
            },
            *read
        );
    }
}
