//! What chats used: tokens, cost and the context window, per session, and
//! totals by agent or project.
//!
//! Agents report running totals for their own session (ACP `usage_update`
//! and the prompt reply's `usage`). A chat can span several agent sessions
//! (restarts), and a resumed one reports totals that include turns already
//! counted, so each report is compared with the last one from the same
//! agent session and only the difference is added.
//!
//! What each report adds is also kept by local day and model
//! (`usage_days`), for the usage dashboard's charts and breakdowns.

use anyhow::Result;
use rusqlite::{OptionalExtension as _, params};

use crate::Store;
use crate::sessions::SessionId;
use crate::time::now_millis;

/// A chat's usage so far.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cost: f64,
    pub currency: Option<String>,
    /// Tokens in the agent's context and its window, as last reported.
    pub context: Option<(u64, u64)>,
}

/// One report from an agent: running totals for `agent_session`, each
/// `None` when this report does not carry it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageReport {
    pub agent_session: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cost: Option<(f64, String)>,
    pub context: Option<(u64, u64)>,
    /// The model the chat used when the report came, when known.
    pub model: Option<String>,
}

/// What one chat used on one (local) day with one model.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageDay {
    pub session_id: SessionId,
    /// The workspace the chat is in.
    pub project_id: crate::projects::ProjectId,
    /// The agent id the chat runs.
    pub agent: String,
    /// `YYYY-MM-DD`, local time.
    pub day: String,
    /// Empty when not known.
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cost: f64,
    pub currency: Option<String>,
}

/// Totals for one agent or project.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageTotal {
    /// The agent id, or the project's name.
    pub key: String,
    pub chats: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    pub currency: Option<String>,
}

/// How [`Store::usage_totals`] groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageBy {
    Agent,
    Project,
}

/// A stored row: the totals and the last report they were counted from.
#[derive(Debug, Default)]
struct Row {
    usage: SessionUsage,
    base_session: Option<String>,
    base: (u64, u64, u64, f64),
}

/// How much a running total grew since `last`; a smaller one means the
/// agent started counting again.
fn grew(now: u64, last: u64) -> u64 {
    if now >= last { now - last } else { now }
}

impl Store {
    fn usage_row(&self, session_id: SessionId) -> Result<Option<Row>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT input_tokens, output_tokens, cached_tokens, cost, currency, context_used, context_size,
                        base_session, base_input, base_output, base_cached, base_cost
                 FROM session_usage WHERE session_id = ?1",
                params![session_id],
                |row| {
                    let number = |ix| row.get::<_, i64>(ix).map(|n| n.max(0) as u64);
                    let context = match (row.get::<_, Option<i64>>(5)?, row.get::<_, Option<i64>>(6)?) {
                        (Some(used), Some(size)) => Some((used.max(0) as u64, size.max(0) as u64)),
                        _ => None,
                    };
                    Ok(Row {
                        usage: SessionUsage {
                            input_tokens: number(0)?,
                            output_tokens: number(1)?,
                            cached_tokens: number(2)?,
                            cost: row.get(3)?,
                            currency: row.get(4)?,
                            context,
                        },
                        base_session: row.get(7)?,
                        base: (number(8)?, number(9)?, number(10)?, row.get(11)?),
                    })
                },
            )
            .optional()?)
    }

    pub fn session_usage(&self, session_id: SessionId) -> Result<Option<SessionUsage>> {
        Ok(self.usage_row(session_id)?.map(|row| row.usage))
    }

    /// Counts an agent's report into a chat's usage; returns the new totals.
    pub fn record_usage(
        &self,
        session_id: SessionId,
        report: &UsageReport,
    ) -> Result<SessionUsage> {
        let mut row = self.usage_row(session_id)?.unwrap_or_default();
        // A different agent session counts from zero.
        if row.base_session.as_deref() != Some(report.agent_session.as_str()) {
            row.base_session = Some(report.agent_session.clone());
            row.base = (0, 0, 0, 0.0);
        }
        let (base_input, base_output, base_cached, base_cost) = &mut row.base;
        let usage = &mut row.usage;
        // What this report adds, for the day's row.
        let mut added = (0u64, 0u64, 0u64, 0f64);
        if let Some(input) = report.input_tokens {
            added.0 = grew(input, *base_input);
            usage.input_tokens += added.0;
            *base_input = input;
        }
        if let Some(output) = report.output_tokens {
            added.1 = grew(output, *base_output);
            usage.output_tokens += added.1;
            *base_output = output;
        }
        if let Some(cached) = report.cached_tokens {
            added.2 = grew(cached, *base_cached);
            usage.cached_tokens += added.2;
            *base_cached = cached;
        }
        if let Some((cost, currency)) = &report.cost {
            added.3 = if *cost >= *base_cost {
                cost - *base_cost
            } else {
                *cost
            };
            usage.cost += added.3;
            *base_cost = *cost;
            usage.currency = Some(currency.clone());
        }
        if added != (0, 0, 0, 0.0) {
            self.conn().execute(
                "INSERT INTO usage_days
                    (session_id, day, model, input_tokens, output_tokens, cached_tokens, cost, currency)
                 VALUES (?1, date('now', 'localtime'), ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (session_id, day, model) DO UPDATE SET
                    input_tokens = input_tokens + excluded.input_tokens,
                    output_tokens = output_tokens + excluded.output_tokens,
                    cached_tokens = cached_tokens + excluded.cached_tokens,
                    cost = cost + excluded.cost,
                    currency = coalesce(excluded.currency, currency)",
                params![
                    session_id,
                    report.model.as_deref().unwrap_or_default(),
                    added.0 as i64,
                    added.1 as i64,
                    added.2 as i64,
                    added.3,
                    report.cost.as_ref().map(|(_, currency)| currency.as_str()),
                ],
            )?;
        }
        if report.context.is_some() {
            usage.context = report.context;
        }
        let (used, size) = usage
            .context
            .map(|(used, size)| (Some(used as i64), Some(size as i64)))
            .unwrap_or((None, None));
        self.conn().execute(
            "INSERT OR REPLACE INTO session_usage
                (session_id, input_tokens, output_tokens, cached_tokens, cost, currency, context_used, context_size,
                 base_session, base_input, base_output, base_cached, base_cost, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                session_id,
                usage.input_tokens as i64,
                usage.output_tokens as i64,
                usage.cached_tokens as i64,
                usage.cost,
                usage.currency,
                used,
                size,
                row.base_session,
                row.base.0 as i64,
                row.base.1 as i64,
                row.base.2 as i64,
                row.base.3,
                now_millis()
            ],
        )?;
        Ok(row.usage)
    }

    /// Usage of chats active since `since` (milliseconds), by agent or
    /// project, most expensive (then most tokens) first.
    pub fn usage_totals(&self, by: UsageBy, since: i64) -> Result<Vec<UsageTotal>> {
        let key = match by {
            UsageBy::Agent => "s.agent_id",
            UsageBy::Project => "p.name",
        };
        let conn = self.conn();
        let mut statement = conn.prepare(&format!(
            "SELECT {key}, count(*), sum(u.input_tokens), sum(u.output_tokens), sum(u.cost), max(u.currency)
             FROM session_usage u
             JOIN sessions s ON s.id = u.session_id
             JOIN projects p ON p.id = s.project_id
             WHERE u.updated_at >= ?1
             GROUP BY {key}
             ORDER BY sum(u.cost) DESC, sum(u.input_tokens + u.output_tokens) DESC"
        ))?;
        let totals = statement
            .query_map(params![since], |row| {
                Ok(UsageTotal {
                    key: row.get(0)?,
                    chats: row.get::<_, i64>(1)? as u64,
                    input_tokens: row.get::<_, i64>(2)? as u64,
                    output_tokens: row.get::<_, i64>(3)? as u64,
                    cost: row.get(4)?,
                    currency: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(totals)
    }

    /// Every chat's usage by day and model from `since` (`YYYY-MM-DD`, local)
    /// on, oldest first.
    pub fn usage_days(&self, since: &str) -> Result<Vec<UsageDay>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT d.session_id, s.agent_id, d.day, d.model, d.input_tokens, d.output_tokens,
                    d.cached_tokens, d.cost, d.currency, s.project_id
             FROM usage_days d
             JOIN sessions s ON s.id = d.session_id
             WHERE d.day >= ?1
             ORDER BY d.day, d.session_id",
        )?;
        let days = statement
            .query_map(params![since], |row| {
                let number = |ix| row.get::<_, i64>(ix).map(|n| n.max(0) as u64);
                Ok(UsageDay {
                    session_id: row.get(0)?,
                    project_id: row.get(9)?,
                    agent: row.get(1)?,
                    day: row.get(2)?,
                    model: row.get(3)?,
                    input_tokens: number(4)?,
                    output_tokens: number(5)?,
                    cached_tokens: number(6)?,
                    cost: row.get(7)?,
                    currency: row.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(agent_session: &str, input: u64, cost: f64) -> UsageReport {
        UsageReport {
            agent_session: agent_session.into(),
            input_tokens: Some(input),
            cost: Some((cost, "USD".into())),
            ..Default::default()
        }
    }

    #[test]
    fn running_totals_are_counted_once() {
        let store = Store::open_in_memory().unwrap();
        let folder = std::env::temp_dir().join(format!("ph-usage-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let project = store.add_project(&folder).unwrap();
        let chat = store.create_session(project.id, "One", "claude").unwrap();
        assert_eq!(store.session_usage(chat.id).unwrap(), None);

        store
            .record_usage(chat.id, &report("a", 1_000, 0.01))
            .unwrap();
        let usage = store
            .record_usage(chat.id, &report("a", 1_500, 0.03))
            .unwrap();
        assert_eq!(
            usage.input_tokens, 1_500,
            "a running total, not added twice"
        );
        assert!((usage.cost - 0.03).abs() < 1e-9);

        // The agent restarted: a new session counts from zero, added on.
        let usage = store
            .record_usage(chat.id, &report("b", 200, 0.002))
            .unwrap();
        assert_eq!(usage.input_tokens, 1_700);
        // A report without tokens keeps them; context comes and stays.
        let usage = store
            .record_usage(
                chat.id,
                &UsageReport {
                    agent_session: "b".into(),
                    context: Some((9_000, 200_000)),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            (usage.input_tokens, usage.context),
            (1_700, Some((9_000, 200_000)))
        );
        // Resuming "b" after a PilotHouse restart: same running totals, nothing new.
        let usage = store
            .record_usage(chat.id, &report("b", 200, 0.002))
            .unwrap();
        assert_eq!(usage.input_tokens, 1_700);
    }

    #[test]
    fn what_each_report_adds_is_kept_by_day_and_model() {
        let store = Store::open_in_memory().unwrap();
        let folder = std::env::temp_dir().join(format!("ph-usage-days-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let project = store.add_project(&folder).unwrap();
        let chat = store.create_session(project.id, "One", "claude").unwrap();
        let with_model = |input, cost, model: &str| UsageReport {
            model: Some(model.into()),
            ..report("a", input, cost)
        };
        store
            .record_usage(chat.id, &with_model(1_000, 0.10, "opus"))
            .unwrap();
        store
            .record_usage(chat.id, &with_model(1_500, 0.15, "opus"))
            .unwrap();
        store
            .record_usage(chat.id, &with_model(1_800, 0.20, "sonnet"))
            .unwrap();
        // Nothing new: no row is touched.
        store
            .record_usage(chat.id, &with_model(1_800, 0.20, "haiku"))
            .unwrap();

        let days = store.usage_days("2000-01-01").unwrap();
        assert_eq!(days.len(), 2, "{days:?}");
        let opus = days.iter().find(|day| day.model == "opus").unwrap();
        assert_eq!((opus.agent.as_str(), opus.input_tokens), ("claude", 1_500));
        assert!((opus.cost - 0.15).abs() < 1e-9);
        let sonnet = days.iter().find(|day| day.model == "sonnet").unwrap();
        assert_eq!(sonnet.input_tokens, 300);
        assert_eq!(opus.day.len(), 10, "a local date: {}", opus.day);
        assert!(store.usage_days("9999-01-01").unwrap().is_empty());
    }

    #[test]
    fn totals_by_agent_and_project() {
        let store = Store::open_in_memory().unwrap();
        let folder = std::env::temp_dir().join(format!("ph-usage-totals-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let project = store.add_project(&folder).unwrap();
        let claude = store.create_session(project.id, "One", "claude").unwrap();
        let codex = store.create_session(project.id, "Two", "codex").unwrap();
        store
            .record_usage(claude.id, &report("a", 1_000, 0.5))
            .unwrap();
        store
            .record_usage(codex.id, &report("c", 4_000, 0.1))
            .unwrap();

        let by_agent = store.usage_totals(UsageBy::Agent, 0).unwrap();
        assert_eq!(
            by_agent
                .iter()
                .map(|total| total.key.as_str())
                .collect::<Vec<_>>(),
            ["claude", "codex"],
            "the more expensive first"
        );
        let by_project = store.usage_totals(UsageBy::Project, 0).unwrap();
        assert_eq!(
            (by_project[0].chats, by_project[0].input_tokens),
            (2, 5_000)
        );
        assert!(
            store
                .usage_totals(UsageBy::Agent, i64::MAX)
                .unwrap()
                .is_empty()
        );
    }
}
