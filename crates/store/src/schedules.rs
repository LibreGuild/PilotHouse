//! Schedules: work PilotHouse starts on its own, per workspace: a prompt for
//! an agent (a morning summary, a nightly check), or a run of the
//! workspace's UI tests. It starts at set times, or when something happens
//! ([`When`]): files change, a commit, a pull request, failing checks, or
//! another schedule finishing (a chain).
//!
//! They run while PilotHouse is open. One that came due while it was closed
//! runs once when it opens, not once per missed time.

use anyhow::{Context as _, Result, bail};
use jiff::{Timestamp, ToSpan as _, Zoned, tz::TimeZone};
use rusqlite::{OptionalExtension as _, Row, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::projects::ProjectId;
use crate::sessions::SessionId;
use crate::time::now_millis;

pub type ScheduleId = i64;

/// What a schedule does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Job {
    /// Starts a chat with `prompt` for the default agent.
    Agent { prompt: String },
    /// Runs every UI test of the workspace.
    Tests,
}

/// When a schedule runs: at times (local time), or on an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "when", rename_all = "snake_case")]
pub enum When {
    /// Every `minutes` minutes (at least 15).
    Minutes { minutes: u32 },
    /// Each day at `hour:minute`.
    Daily { hour: u8, minute: u8 },
    /// Monday to Friday at `hour:minute`.
    Weekdays { hour: u8, minute: u8 },
    /// Each week on `weekday` (1 Monday … 7 Sunday) at `hour:minute`.
    Weekly { weekday: u8, hour: u8, minute: u8 },
    /// When files of the workspace change (and then stay unchanged a few
    /// seconds); with `pattern`, only paths containing it count.
    FilesChange {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
    },
    /// When a folder's Git HEAD moves to another commit.
    Commit,
    /// When a pull request opens on the workspace's GitHub repository.
    PullRequest,
    /// When checks fail on the commit the workspace has checked out.
    ChecksFail,
    /// When schedule `schedule` finishes (with `on_failure`, only when it
    /// fails; a test run fails when a test does).
    After {
        schedule: ScheduleId,
        #[serde(default)]
        on_failure: bool,
    },
}

/// An event schedule's next run time: never, it waits for its event.
pub const NEVER: i64 = i64::MAX;

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

impl When {
    /// "Every day at 09:00", "When a pull request opens". `name_of` names
    /// the schedule an `After` waits for.
    pub fn describe(&self, name_of: impl Fn(ScheduleId) -> Option<String>) -> String {
        match self {
            When::Minutes { minutes: 60 } => "Every hour".into(),
            When::Minutes { minutes } => format!("Every {minutes} minutes"),
            When::Daily { hour, minute } => format!("Every day at {hour:02}:{minute:02}"),
            When::Weekdays { hour, minute } => format!("Weekdays at {hour:02}:{minute:02}"),
            When::Weekly {
                weekday,
                hour,
                minute,
            } => format!(
                "Every {} at {hour:02}:{minute:02}",
                WEEKDAYS[((*weekday).clamp(1, 7) - 1) as usize]
            ),
            When::FilesChange { pattern: None } => "When files change".into(),
            When::FilesChange {
                pattern: Some(pattern),
            } => format!("When files in “{pattern}” change"),
            When::Commit => "After each new commit".into(),
            When::PullRequest => "When a pull request opens".into(),
            When::ChecksFail => "When checks fail".into(),
            When::After {
                schedule,
                on_failure,
            } => {
                let name = name_of(*schedule).unwrap_or_else(|| "a deleted schedule".into());
                if *on_failure {
                    format!("When “{name}” fails")
                } else {
                    format!("After “{name}” runs")
                }
            }
        }
    }

    /// Runs on an event rather than at times.
    pub fn is_event(&self) -> bool {
        !matches!(
            self,
            When::Minutes { .. } | When::Daily { .. } | When::Weekdays { .. } | When::Weekly { .. }
        )
    }

    fn check(&self) -> Result<()> {
        let time_ok = |hour: u8, minute: u8| hour < 24 && minute < 60;
        let ok = match *self {
            When::Minutes { minutes } => minutes >= 15,
            When::Daily { hour, minute } | When::Weekdays { hour, minute } => time_ok(hour, minute),
            When::Weekly {
                weekday,
                hour,
                minute,
            } => (1..=7).contains(&weekday) && time_ok(hour, minute),
            _ => true,
        };
        if !ok {
            bail!(
                "{} is not a time a schedule can run at",
                self.describe(|_| None)
            );
        }
        Ok(())
    }

    /// The first time after `after` (milliseconds since 1970) it runs;
    /// [`NEVER`] for an event.
    pub fn next_after(&self, after: i64) -> Result<i64> {
        self.next_in(after, &TimeZone::system())
    }

    fn next_in(&self, after: i64, zone: &TimeZone) -> Result<i64> {
        let now = Timestamp::from_millisecond(after)?.to_zoned(zone.clone());
        let next = match *self {
            When::Minutes { minutes } => now.checked_add((minutes as i64).minutes())?,
            When::Daily { hour, minute } => first_day(&now, hour, minute, |_| true)?,
            When::Weekdays { hour, minute } => first_day(&now, hour, minute, |day| {
                day.weekday().to_monday_one_offset() <= 5
            })?,
            When::Weekly {
                weekday,
                hour,
                minute,
            } => first_day(&now, hour, minute, |day| {
                day.weekday().to_monday_one_offset() == weekday as i8
            })?,
            _ => return Ok(NEVER),
        };
        Ok(next.timestamp().as_millisecond())
    }
}

/// The first `hour:minute` after `now` on a day `fits` accepts (looking a
/// week ahead).
fn first_day(now: &Zoned, hour: u8, minute: u8, fits: impl Fn(&Zoned) -> bool) -> Result<Zoned> {
    for days in 0..=7 {
        let day = now.checked_add(days.days())?;
        let at = day
            .with()
            .hour(hour as i8)
            .minute(minute as i8)
            .second(0)
            .subsec_nanosecond(0)
            .build()?;
        if at > *now && fits(&at) {
            return Ok(at);
        }
    }
    bail!("no day in the next week fits")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub id: ScheduleId,
    pub project_id: ProjectId,
    pub name: String,
    pub job: Job,
    pub timing: When,
    pub enabled: bool,
    pub next_run: i64,
    pub last_run: Option<i64>,
    /// How the last run went, in a line.
    pub last_result: Option<String>,
    /// The chat the last agent run started.
    pub last_session: Option<SessionId>,
}

impl Schedule {
    fn from_row(row: &Row) -> rusqlite::Result<Self> {
        let job: String = row.get("job")?;
        let timing: String = row.get("timing")?;
        let bad = |err: serde_json::Error| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        };
        Ok(Self {
            id: row.get("id")?,
            project_id: row.get("project_id")?,
            name: row.get("name")?,
            job: serde_json::from_str(&job).map_err(bad)?,
            timing: serde_json::from_str(&timing).map_err(bad)?,
            enabled: row.get("enabled")?,
            next_run: row.get("next_run")?,
            last_run: row.get("last_run")?,
            last_result: row.get("last_result")?,
            last_session: row.get("last_session")?,
        })
    }
}

const COLUMNS: &str =
    "id, project_id, name, job, timing, enabled, next_run, last_run, last_result, last_session";

impl Store {
    pub fn schedules(&self, project_id: ProjectId) -> Result<Vec<Schedule>> {
        let conn = self.conn();
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM schedules WHERE project_id = ?1 ORDER BY name, id"
        ))?;
        let schedules = statement
            .query_map(params![project_id], Schedule::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(schedules)
    }

    pub fn schedule(&self, id: ScheduleId) -> Result<Option<Schedule>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM schedules WHERE id = ?1"),
                params![id],
                Schedule::from_row,
            )
            .optional()?)
    }

    /// Enabled schedules that wait for an event, in every workspace.
    pub fn event_schedules(&self) -> Result<Vec<Schedule>> {
        let conn = self.conn();
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM schedules WHERE enabled = 1 AND next_run = ?1"
        ))?;
        let schedules = statement
            .query_map(params![NEVER], Schedule::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(schedules
            .into_iter()
            .filter(|schedule| schedule.timing.is_event())
            .collect())
    }

    /// Enabled schedules of every workspace whose time has come by `now`.
    pub fn due_schedules(&self, now: i64) -> Result<Vec<Schedule>> {
        let conn = self.conn();
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM schedules WHERE enabled = 1 AND next_run <= ?1 ORDER BY next_run"
        ))?;
        let schedules = statement
            .query_map(params![now], Schedule::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(schedules)
    }

    pub fn add_schedule(
        &self,
        project_id: ProjectId,
        name: &str,
        job: &Job,
        timing: When,
    ) -> Result<Schedule> {
        let name = checked(name, job, &timing)?;
        let now = now_millis();
        // One lock for both: another thread's insert in between would
        // change the last row id.
        let id = {
            let conn = self.conn();
            conn.execute(
                "INSERT INTO schedules (project_id, name, job, timing, enabled, next_run, created_at)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
                params![
                    project_id,
                    name,
                    serde_json::to_string(job)?,
                    serde_json::to_string(&timing)?,
                    timing.next_after(now)?,
                    now
                ],
            )?;
            conn.last_insert_rowid()
        };
        self.schedule(id)?.context("the schedule vanished")
    }

    /// Changes a schedule; its next run is worked out again.
    pub fn update_schedule(
        &self,
        id: ScheduleId,
        name: &str,
        job: &Job,
        timing: When,
        enabled: bool,
    ) -> Result<Schedule> {
        let name = checked(name, job, &timing)?;
        self.conn().execute(
            "UPDATE schedules SET name = ?2, job = ?3, timing = ?4, enabled = ?5, next_run = ?6 WHERE id = ?1",
            params![
                id,
                name,
                serde_json::to_string(job)?,
                serde_json::to_string(&timing)?,
                enabled,
                timing.next_after(now_millis())?
            ],
        )?;
        self.schedule(id)?.context("no such schedule")
    }

    pub fn delete_schedule(&self, id: ScheduleId) -> Result<()> {
        self.conn()
            .execute("DELETE FROM schedules WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Notes that a schedule started at `now` and when it runs next (so it
    /// does not start twice).
    pub fn schedule_started(&self, id: ScheduleId, now: i64) -> Result<()> {
        let schedule = self.schedule(id)?.context("no such schedule")?;
        self.conn().execute(
            "UPDATE schedules SET last_run = ?2, next_run = ?3, last_result = 'Running…', last_session = NULL
             WHERE id = ?1",
            params![id, now, schedule.timing.next_after(now)?],
        )?;
        Ok(())
    }

    /// Records how a run went (and the chat it started).
    pub fn schedule_finished(
        &self,
        id: ScheduleId,
        result: &str,
        session: Option<SessionId>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE schedules SET last_result = ?2, last_session = COALESCE(?3, last_session) WHERE id = ?1",
            params![id, result, session],
        )?;
        Ok(())
    }

    /// Runs that finished after `since` (milliseconds), in every workspace,
    /// newest first: for the inbox.
    pub fn finished_since(&self, since: i64) -> Result<Vec<Schedule>> {
        let conn = self.conn();
        let mut statement = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM schedules
             WHERE last_run > ?1 AND last_result IS NOT NULL AND last_result != 'Running…'
             ORDER BY last_run DESC"
        ))?;
        let schedules = statement
            .query_map(params![since], Schedule::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(schedules)
    }

    /// The schedule whose last run started chat `session`.
    pub fn schedule_of_session(&self, session: SessionId) -> Result<Option<Schedule>> {
        Ok(self
            .conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM schedules WHERE last_session = ?1"),
                params![session],
                Schedule::from_row,
            )
            .optional()?)
    }
}

/// The trimmed name, once the schedule makes sense.
fn checked<'a>(name: &'a str, job: &Job, timing: &When) -> Result<&'a str> {
    let name = name.trim();
    if name.is_empty() {
        bail!("Give the schedule a name.");
    }
    if let Job::Agent { prompt } = job
        && prompt.trim().is_empty()
    {
        bail!("Say what the agent should do.");
    }
    timing.check()?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-21 (a Wednesday) at `hour:minute` UTC, in milliseconds.
    fn at(day: i8, hour: i8, minute: i8) -> i64 {
        jiff::civil::date(2026, 10, day)
            .at(hour, minute, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap()
            .timestamp()
            .as_millisecond()
    }

    fn next(timing: impl std::borrow::Borrow<When>, after: i64) -> i64 {
        timing.borrow().next_in(after, &TimeZone::UTC).unwrap()
    }

    #[test]
    fn works_out_the_next_run() {
        let wednesday_8 = at(21, 8, 0);
        let daily = When::Daily { hour: 9, minute: 0 };
        assert_eq!(next(&daily, wednesday_8), at(21, 9, 0), "later today");
        assert_eq!(
            next(&daily, at(21, 9, 0)),
            at(22, 9, 0),
            "not the same minute again"
        );
        assert_eq!(
            next(When::Minutes { minutes: 30 }, wednesday_8),
            at(21, 8, 30)
        );
        let friday_10 = at(23, 10, 0);
        assert_eq!(
            next(When::Weekdays { hour: 9, minute: 0 }, friday_10),
            at(26, 9, 0),
            "skips the weekend"
        );
        assert_eq!(
            next(
                When::Weekly {
                    weekday: 1,
                    hour: 7,
                    minute: 30
                },
                wednesday_8
            ),
            at(26, 7, 30),
            "next Monday"
        );
        assert_eq!(daily.describe(|_| None), "Every day at 09:00");
        assert!(When::Minutes { minutes: 5 }.check().is_err(), "too often");
        assert_eq!(
            next(When::Commit, wednesday_8),
            NEVER,
            "events have no time"
        );
        let after = When::After {
            schedule: 3,
            on_failure: true,
        };
        assert_eq!(
            after.describe(|_| Some("Nightly".into())),
            "When “Nightly” fails"
        );
    }

    #[test]
    fn due_schedules_run_once_then_wait() {
        let store = Store::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("ph-schedules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let project = store.add_project(&dir).unwrap();
        let job = Job::Agent {
            prompt: "Summarize the day".into(),
        };
        let schedule = store
            .add_schedule(project.id, "Summary", &job, When::Minutes { minutes: 15 })
            .unwrap();
        assert!(store.due_schedules(now_millis()).unwrap().is_empty());
        let later = schedule.next_run + 1;
        assert_eq!(store.due_schedules(later).unwrap().len(), 1);
        store.schedule_started(schedule.id, later).unwrap();
        assert!(store.due_schedules(later).unwrap().is_empty(), "not twice");
        store
            .schedule_finished(schedule.id, "Done", Some(7))
            .unwrap();
        assert_eq!(
            store.schedule_of_session(7).unwrap().unwrap().id,
            schedule.id
        );
        assert!(
            store
                .add_schedule(project.id, " ", &job, When::Minutes { minutes: 15 })
                .is_err()
        );
        assert!(
            store
                .add_schedule(
                    project.id,
                    "x",
                    &Job::Agent { prompt: "".into() },
                    When::Minutes { minutes: 15 }
                )
                .is_err()
        );
    }
}
