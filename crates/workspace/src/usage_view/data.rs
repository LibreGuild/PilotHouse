//! The usage dashboard's numbers: every reply the agents' own files record
//! ([`ph_agent::usage_logs`]) and what agents reported in PilotHouse chats
//! (`usage_days`), put together per agent, day, model and place, then summed
//! over a period. Built off the UI thread.
//!
//! For an agent whose files are on this computer (Claude Code, Codex,
//! Gemini CLI), tokens come from those files, which hold every session
//! wherever it ran, PilotHouse's included; PilotHouse's own records then add
//! only the cost the agent reported (counting their tokens too would count
//! them twice). For the others (Antigravity), tokens and cost are what they
//! reported in PilotHouse chats.
//!
//! Each reply is also priced at its model's API list price
//! ([`super::prices`]), an estimate of what the tokens would cost without a
//! subscription; a model without a price adds its tokens to `unpriced`.
//!
//! A reply belongs to the workspace whose folder it worked in (the session's
//! folder, from the agent's file), or to its chat's workspace.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jiff::ToSpan as _;
use jiff::civil::Date;
use ph_agent::agents::{AGENTS, AgentSpec};
use ph_agent::usage_logs::Scan;
use ph_store::projects::{Project, ProjectId};
use ph_store::usage::UsageDay;

use super::prices::Prices;

/// How far back the dashboard reads: a year and the month it starts in.
pub const DAYS_READ: i64 = 400;

/// The period summed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Week,
    Month,
    Quarter,
    Year,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::Week, Range::Month, Range::Quarter, Range::Year];

    pub fn label(self) -> &'static str {
        match self {
            Range::Week => "Last 7 days",
            Range::Month => "Last 30 days",
            Range::Quarter => "Last 90 days",
            Range::Year => "Last 12 months",
        }
    }

    /// "in the last 30 days".
    pub fn phrase(self) -> String {
        format!("in the {}", self.label().to_lowercase())
    }

    /// The first day counted, ending today: whole months over a year.
    pub fn start(self, today: Date) -> Date {
        let back = |days: i64| today.checked_sub((days - 1).days()).unwrap_or(today);
        match self {
            Range::Week => back(7),
            Range::Month => back(30),
            Range::Quarter => back(90),
            Range::Year => today
                .first_of_month()
                .checked_sub(11.months())
                .unwrap_or(today),
        }
    }

    /// The bar a day falls in: the day itself, its week, or its month.
    fn bucket(self, day: Date, start: Date) -> Date {
        match self {
            Range::Week | Range::Month => day,
            Range::Quarter => week_start(day).max(start),
            Range::Year => day.first_of_month(),
        }
    }

    fn next(self, bucket: Date) -> Option<Date> {
        match self {
            Range::Week | Range::Month => bucket.checked_add(1.day()).ok(),
            Range::Quarter => week_start(bucket).checked_add(7.days()).ok(),
            Range::Year => bucket.checked_add(1.month()).ok(),
        }
    }

    /// What a bar is: "day", "week" or "month".
    pub fn unit(self) -> &'static str {
        match self {
            Range::Week | Range::Month => "day",
            Range::Quarter => "week",
            Range::Year => "month",
        }
    }
}

/// How the list groups the period's usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Agent,
    Workspace,
    Model,
}

/// Tokens and the cost reported for them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tally {
    /// Input not read from a cache.
    pub input: u64,
    /// Input read from a cache.
    pub cached: u64,
    pub output: u64,
    /// What agents reported as cost (in PilotHouse chats).
    pub cost: f64,
    pub currency: Option<String>,
    /// What the tokens would cost at API list prices, in US dollars.
    pub estimate: f64,
    /// Tokens of models without a known price, not in `estimate`.
    pub unpriced: u64,
}

impl Tally {
    pub fn add(&mut self, other: &Tally) {
        self.input += other.input;
        self.cached += other.cached;
        self.output += other.output;
        self.cost += other.cost;
        self.estimate += other.estimate;
        self.unpriced += other.unpriced;
        if self.currency.is_none() {
            self.currency.clone_from(&other.currency);
        }
    }

    /// Every token processed: input (cached or not) and output.
    pub fn tokens(&self) -> u64 {
        self.input + self.cached + self.output
    }

    /// The share of input read from a cache, 0 to 1.
    pub fn cache_share(&self) -> Option<f32> {
        let input = self.input + self.cached;
        (input > 0).then(|| self.cached as f32 / input as f32)
    }
}

/// Where an agent's numbers come from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// Its own files, in this folder.
    Files(PathBuf),
    /// What it reported in PilotHouse chats.
    PilotHouse,
}

/// One agent's state on this computer.
#[derive(Debug, Clone)]
pub struct AgentState {
    pub spec: &'static AgentSpec,
    pub installed: bool,
    pub source: Source,
}

/// Where a reply's work happened.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Place {
    Workspace(ProjectId),
    /// A folder no workspace holds.
    Folder(Arc<str>),
    /// Any folder in the system's temporary folders: scratch work, one line
    /// for all of it rather than one per throwaway folder.
    Temporary,
    /// Not recorded.
    Unknown,
}

/// Whether `path` is in one of the system's temporary folders.
fn temporary(path: &Path) -> bool {
    let temp = std::env::temp_dir();
    [
        Path::new("/tmp"),
        Path::new("/private/tmp"),
        Path::new("/var/tmp"),
        Path::new("/private/var/folders"),
        Path::new("/var/folders"),
        temp.as_path(),
    ]
    .iter()
    .any(|dir| !dir.as_os_str().is_empty() && path.starts_with(dir))
}

/// A workspace, as the list shows it.
#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub name: String,
    /// Its colour's index in the workspace palette.
    pub shade: usize,
}

/// What one session used one day with one model, somewhere.
#[derive(Debug, Clone)]
struct Row {
    agent: &'static str,
    day: Date,
    model: Arc<str>,
    place: Place,
    /// `None` for a cost-only row (tokens counted from the agent's files).
    session: Option<Arc<str>>,
    tally: Tally,
}

/// What a line of the list stands for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Agent(&'static str),
    Place(Place),
    Model(&'static str, Arc<str>),
}

/// A line of the list: its usage, split by agent for the bar.
#[derive(Debug, Clone)]
pub struct Line {
    pub key: Key,
    pub tally: Tally,
    pub sessions: usize,
    /// Tokens per agent, in [`AGENTS`] order.
    pub by_agent: Vec<u64>,
}

/// A bar of the chart: a day, week or month, tokens per agent.
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub start: Date,
    pub by_agent: Vec<u64>,
}

impl Bar {
    pub fn tokens(&self) -> u64 {
        self.by_agent.iter().sum()
    }
}

/// Sums over a period.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub total: Tally,
    pub sessions: usize,
    /// Most tokens first.
    pub lines: Vec<Line>,
    pub bars: Vec<Bar>,
}

/// One agent over a period, for its details.
#[derive(Debug, Clone, Default)]
pub struct AgentDetail {
    /// Most tokens first.
    pub models: Vec<(Arc<str>, Tally)>,
    /// Its busiest day: the day and its tokens.
    pub busiest: Option<(Date, u64)>,
    /// Days with any use.
    pub active_days: usize,
}

/// Everything read, ready to sum over any period.
#[derive(Debug, Clone)]
pub struct Dashboard {
    pub today: Date,
    /// When the prices were fetched (Unix milliseconds); 0 without any.
    pub prices_at: i64,
    pub agents: Vec<AgentState>,
    pub workspaces: HashMap<ProjectId, WorkspaceInfo>,
    rows: Vec<Row>,
}

impl Dashboard {
    /// Puts together what the agents' files and PilotHouse recorded.
    /// `installed` says whether an agent's CLI is on this computer.
    pub fn build(
        today: Date,
        tz: &jiff::tz::TimeZone,
        scan: Scan,
        recorded: Vec<UsageDay>,
        projects: &[Project],
        prices: &Prices,
        installed: impl Fn(&AgentSpec) -> bool,
    ) -> Self {
        let workspaces: Vec<&Project> = projects
            .iter()
            .filter(|project| project.is_workspace())
            .collect();
        let palette = ph_ui::WORKSPACE_COLORS.len();
        // A folder's workspace: the one with the deepest folder holding it.
        let mut places: HashMap<Arc<str>, Place> = HashMap::new();
        let mut place_of = |cwd: &Arc<str>| -> Place {
            if cwd.is_empty() {
                return Place::Unknown;
            }
            places
                .entry(cwd.clone())
                .or_insert_with(|| {
                    let path = Path::new(&**cwd);
                    workspaces
                        .iter()
                        .flat_map(|project| {
                            project
                                .folders()
                                .into_iter()
                                .filter(|folder| path.starts_with(folder))
                                .map(move |folder| (folder.components().count(), project.id))
                        })
                        .max()
                        .map_or_else(
                            || {
                                if temporary(path) {
                                    Place::Temporary
                                } else {
                                    Place::Folder(cwd.clone())
                                }
                            },
                            |(_, id)| Place::Workspace(id),
                        )
                })
                .clone()
        };
        let from_files: HashSet<&str> = scan.sources.iter().map(|(agent, _)| *agent).collect();
        let mut rows = Vec::new();
        for entry in scan.entries {
            let Ok(at) = jiff::Timestamp::from_millisecond(entry.at) else {
                continue;
            };
            rows.push(Row {
                agent: entry.agent,
                day: at.to_zoned(tz.clone()).date(),
                place: place_of(&entry.cwd),
                model: entry.model.clone(),
                session: Some(Arc::from(format!("{}:{}", entry.agent, entry.session))),
                tally: priced(
                    prices,
                    &entry.model,
                    Tally {
                        input: entry.input,
                        cached: entry.cached,
                        output: entry.output,
                        ..Tally::default()
                    },
                    entry.cache_write,
                ),
            });
        }
        let workspace_ids: HashSet<ProjectId> =
            workspaces.iter().map(|project| project.id).collect();
        for day in recorded {
            let Some(spec) = ph_agent::agents::find(&day.agent) else {
                continue;
            };
            let Ok(date) = day.day.parse::<Date>() else {
                continue;
            };
            let files = from_files.contains(spec.id);
            if files && day.cost <= 0.0 {
                continue;
            }
            rows.push(Row {
                agent: spec.id,
                day: date,
                model: Arc::from(day.model.as_str()),
                place: if workspace_ids.contains(&day.project_id) {
                    Place::Workspace(day.project_id)
                } else {
                    Place::Unknown
                },
                session: (!files).then(|| Arc::from(format!("pilothouse:{}", day.session_id))),
                tally: if files {
                    Tally {
                        cost: day.cost,
                        currency: day.currency,
                        ..Tally::default()
                    }
                } else {
                    priced(
                        prices,
                        &day.model,
                        Tally {
                            input: day.input_tokens,
                            cached: day.cached_tokens,
                            output: day.output_tokens,
                            cost: day.cost,
                            currency: day.currency,
                            ..Tally::default()
                        },
                        0,
                    )
                },
            });
        }
        let agents = AGENTS
            .iter()
            .map(|spec| AgentState {
                spec,
                installed: installed(spec),
                source: scan
                    .sources
                    .iter()
                    .find(|(agent, _)| *agent == spec.id)
                    .map(|(_, dir)| Source::Files(dir.clone()))
                    .unwrap_or(Source::PilotHouse),
            })
            .collect();
        Self {
            today,
            prices_at: prices.fetched_at,
            agents,
            workspaces: workspaces
                .iter()
                .map(|project| {
                    (
                        project.id,
                        WorkspaceInfo {
                            name: project.name.clone(),
                            shade: project.shade(palette),
                        },
                    )
                })
                .collect(),
            rows,
        }
    }

    pub fn agent(&self, id: &str) -> Option<&AgentState> {
        self.agents.iter().find(|agent| agent.spec.id == id)
    }

    fn rows(&self, range: Range) -> impl Iterator<Item = &Row> {
        let start = range.start(self.today);
        self.rows
            .iter()
            .filter(move |row| row.day >= start && row.day <= self.today)
    }

    /// Sums `range`, listed by `group`. Grouped by agent, every installed
    /// agent is listed, used or not.
    pub fn summary(&self, range: Range, group: Group) -> Summary {
        let start = range.start(self.today);
        let mut total = Tally::default();
        let mut sessions = HashSet::new();
        let mut lines: BTreeMap<Key, (Tally, HashSet<&str>, Vec<u64>)> = BTreeMap::new();
        let mut bars: BTreeMap<Date, Vec<u64>> = BTreeMap::new();
        if group == Group::Agent {
            for agent in self.agents.iter().filter(|agent| agent.installed) {
                lines
                    .entry(Key::Agent(agent.spec.id))
                    .or_insert_with(|| (Tally::default(), HashSet::new(), vec![0; AGENTS.len()]));
            }
        }
        for row in self.rows(range) {
            let tokens = row.tally.tokens();
            let agent_ix = AGENTS.iter().position(|spec| spec.id == row.agent);
            total.add(&row.tally);
            if let Some(session) = &row.session {
                sessions.insert(&**session);
            }
            let key = match group {
                Group::Agent => Key::Agent(row.agent),
                Group::Workspace => Key::Place(row.place.clone()),
                // Cost alone adds no line for a model.
                Group::Model if tokens == 0 => Key::Agent(row.agent),
                Group::Model => Key::Model(row.agent, row.model.clone()),
            };
            if group != Group::Model || matches!(key, Key::Model(..)) {
                let (tally, line_sessions, by_agent) = lines
                    .entry(key)
                    .or_insert_with(|| (Tally::default(), HashSet::new(), vec![0; AGENTS.len()]));
                tally.add(&row.tally);
                if let Some(session) = &row.session {
                    line_sessions.insert(&**session);
                }
                if let Some(ix) = agent_ix {
                    by_agent[ix] += tokens;
                }
            }
            let bar = bars
                .entry(range.bucket(row.day, start))
                .or_insert_with(|| vec![0; AGENTS.len()]);
            if let Some(ix) = agent_ix {
                bar[ix] += tokens;
            }
        }
        let mut lines: Vec<Line> = lines
            .into_iter()
            .map(|(key, (tally, sessions, by_agent))| Line {
                key,
                sessions: sessions.len(),
                tally,
                by_agent,
            })
            .collect();
        lines.sort_by(|a, b| {
            b.tally
                .tokens()
                .cmp(&a.tally.tokens())
                .then(b.tally.cost.total_cmp(&a.tally.cost))
                .then(a.key.cmp(&b.key))
        });
        // Every bar of the period, empty ones too.
        let mut all = Vec::new();
        let mut bar = range.bucket(start, start);
        while bar <= self.today {
            all.push(Bar {
                start: bar,
                by_agent: bars.remove(&bar).unwrap_or_else(|| vec![0; AGENTS.len()]),
            });
            match range.next(bar) {
                Some(next) if next > bar => bar = next,
                _ => break,
            }
        }
        Summary {
            total,
            sessions: sessions.len(),
            lines,
            bars: all,
        }
    }

    /// `agent`'s models, busiest day and active days over `range`.
    pub fn detail(&self, agent: &str, range: Range) -> AgentDetail {
        let mut models: BTreeMap<Arc<str>, Tally> = BTreeMap::new();
        let mut days: BTreeMap<Date, u64> = BTreeMap::new();
        for row in self.rows(range).filter(|row| row.agent == agent) {
            let tokens = row.tally.tokens();
            if tokens == 0 {
                continue;
            }
            models.entry(row.model.clone()).or_default().add(&row.tally);
            *days.entry(row.day).or_default() += tokens;
        }
        let mut models: Vec<_> = models.into_iter().collect();
        models.sort_by_key(|(_, tally)| std::cmp::Reverse(tally.tokens()));
        AgentDetail {
            models,
            busiest: days
                .iter()
                .max_by_key(|(day, tokens)| (**tokens, std::cmp::Reverse(**day)))
                .map(|(day, tokens)| (*day, *tokens)),
            active_days: days.len(),
        }
    }
}

/// `tally` with its estimate at `model`'s price (`cache_write` of its
/// input written to a cache), or its tokens unpriced.
fn priced(prices: &Prices, model: &str, mut tally: Tally, cache_write: u64) -> Tally {
    match prices.find(model) {
        Some(price) => {
            tally.estimate = price.cost(tally.input, cache_write, tally.cached, tally.output);
        }
        None => tally.unpriced = tally.tokens(),
    }
    tally
}

/// The Sunday on or before `day`.
pub fn week_start(day: Date) -> Date {
    let back = day.weekday().to_sunday_zero_offset() as i64;
    day.checked_sub(back.days()).unwrap_or(day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::{Weekday, date};
    use ph_agent::usage_logs::Entry;

    fn scan(entries: Vec<Entry>, sources: &[&'static str]) -> Scan {
        Scan {
            entries,
            sources: sources
                .iter()
                .map(|agent| (*agent, PathBuf::from("/logs")))
                .collect(),
        }
    }

    fn at(day: Date) -> i64 {
        day.at(12, 0, 0, 0)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap()
            .timestamp()
            .as_millisecond()
    }

    fn entry(
        agent: &'static str,
        day: Date,
        model: &str,
        cwd: &str,
        session: &str,
        tokens: u64,
    ) -> Entry {
        Entry::new(
            agent,
            at(day),
            model,
            session,
            cwd,
            (tokens, tokens, tokens),
        )
    }

    fn recorded(
        agent: &str,
        day: &str,
        session: i64,
        project: i64,
        tokens: u64,
        cost: f64,
    ) -> UsageDay {
        UsageDay {
            session_id: session,
            project_id: project,
            agent: agent.into(),
            day: day.into(),
            model: "Opus".into(),
            input_tokens: tokens,
            output_tokens: 0,
            cached_tokens: 0,
            cost,
            currency: Some("USD".into()),
        }
    }

    fn harbor() -> Vec<Project> {
        let store = ph_store::Store::open_in_memory().unwrap();
        let folder = std::env::temp_dir()
            .join(format!("ph-usage-data-{}", std::process::id()))
            .join("harbor");
        std::fs::create_dir_all(&folder).unwrap();
        store.add_project(&folder).unwrap();
        store
            .projects()
            .unwrap()
            .into_iter()
            .filter(|project| project.is_workspace())
            .collect()
    }

    fn dashboard(today: Date, projects: &[Project]) -> Dashboard {
        let harbor = projects[0].path.to_string_lossy().into_owned();
        Dashboard::build(
            today,
            &jiff::tz::TimeZone::UTC,
            scan(
                vec![
                    entry(
                        "claude",
                        date(2026, 11, 4),
                        "claude-opus-5-5",
                        &format!("{harbor}/src"),
                        "s1",
                        100,
                    ),
                    entry(
                        "claude",
                        date(2026, 11, 5),
                        "claude-opus-5-5",
                        "/elsewhere",
                        "s2",
                        10,
                    ),
                    entry("codex", date(2026, 10, 1), "gpt-6-luna", "", "c1", 1),
                ],
                &["claude", "codex"],
            ),
            vec![
                // Its tokens are in Claude's files already: only the cost counts.
                recorded("claude", "2026-11-05", 7, projects[0].id, 999, 1.25),
                // Antigravity has no files: all of it counts.
                recorded("antigravity", "2026-11-03", 8, projects[0].id, 50, 0.0),
            ],
            projects,
            &opus_priced(),
            |spec| spec.id != "gemini",
        )
    }

    fn opus_priced() -> Prices {
        let mut prices = Prices::default();
        prices.models.insert(
            "claude-opus-5-5".into(),
            super::super::prices::Price {
                input: 0.000005,
                output: 0.000025,
                cache_read: Some(0.0000005),
                cache_write: None,
            },
        );
        prices
    }

    #[test]
    fn files_give_tokens_and_pilothouse_gives_cost() {
        let today = date(2026, 11, 5);
        let projects = harbor();
        let dashboard = dashboard(today, &projects);
        let week = dashboard.summary(Range::Week, Group::Agent);
        assert_eq!(week.total.tokens(), 300 + 30 + 50);
        assert!((week.total.cost - 1.25).abs() < 1e-9);
        assert_eq!(week.sessions, 3, "s1, s2 and the Antigravity chat");
        assert_eq!(week.lines[0].key, Key::Agent("claude"));
        assert_eq!(week.lines[0].sessions, 2);
        // Installed agents are listed even when unused: Codex used nothing this week.
        assert!(
            week.lines
                .iter()
                .any(|line| line.key == Key::Agent("codex") && line.tally.tokens() == 0)
        );
        assert!(
            week.lines
                .iter()
                .all(|line| line.key != Key::Agent("gemini"))
        );
        assert_eq!(week.bars.len(), 7);
        assert_eq!(week.bars.last().unwrap().start, today);
        let models = dashboard.summary(Range::Week, Group::Model);
        assert_eq!(
            models
                .lines
                .iter()
                .map(|line| line.key.clone())
                .collect::<Vec<_>>(),
            [
                Key::Model("claude", Arc::from("claude-opus-5-5")),
                Key::Model("antigravity", Arc::from("Opus")),
            ],
            "no model line for cost alone"
        );
        let month = dashboard.summary(Range::Month, Group::Agent);
        assert_eq!(
            month
                .lines
                .iter()
                .find(|line| line.key == Key::Agent("codex"))
                .unwrap()
                .tally
                .tokens(),
            0,
            "1 Oct is 35 days back"
        );
        let year = dashboard.summary(Range::Year, Group::Agent);
        assert_eq!(year.total.tokens(), 383);
        assert_eq!(year.bars.len(), 12, "a bar per month");
        assert_eq!(year.bars[0].start, date(2025, 12, 1));
        let quarter = dashboard.summary(Range::Quarter, Group::Agent);
        assert!(
            quarter.bars.len() == 13 || quarter.bars.len() == 14,
            "{}",
            quarter.bars.len()
        );
        assert_eq!(quarter.bars.iter().map(Bar::tokens).sum::<u64>(), 383);
    }

    #[test]
    fn scratch_folders_count_as_one() {
        assert!(temporary(Path::new("/private/tmp/ph-test/repo")));
        assert!(temporary(Path::new("/var/folders/rn/x/T/scratch")));
        assert!(temporary(&std::env::temp_dir().join("demo")));
        assert!(!temporary(Path::new("/Users/me/code/app")));
        assert!(
            !temporary(Path::new("/tmpfiles/app")),
            "a name that only starts like it"
        );
    }

    #[test]
    fn work_belongs_to_the_workspace_whose_folder_it_was_in() {
        let projects = harbor();
        let dashboard = dashboard(date(2026, 11, 5), &projects);
        let places = dashboard.summary(Range::Week, Group::Workspace);
        let tokens = |place: Place| {
            places
                .lines
                .iter()
                .find(|line| line.key == Key::Place(place.clone()))
                .map(|line| line.tally.tokens())
        };
        assert_eq!(tokens(Place::Workspace(projects[0].id)), Some(300 + 50));
        assert_eq!(tokens(Place::Folder(Arc::from("/elsewhere"))), Some(30));
        assert!(dashboard.workspaces.contains_key(&projects[0].id));
    }

    #[test]
    fn details_name_the_busiest_day() {
        let projects = harbor();
        let dashboard = dashboard(date(2026, 11, 5), &projects);
        let detail = dashboard.detail("claude", Range::Month);
        assert_eq!(detail.models.len(), 1);
        assert_eq!(detail.busiest, Some((date(2026, 11, 4), 300)));
        assert_eq!(detail.active_days, 2);
    }

    #[test]
    fn replies_are_priced_by_their_model() {
        let projects = harbor();
        let dashboard = dashboard(date(2026, 11, 5), &projects);
        let week = dashboard.summary(Range::Week, Group::Agent);
        // Opus: 110 in, 110 cached, 110 out; the rest has no price.
        let expected = 110. * 0.000005 + 110. * 0.0000005 + 110. * 0.000025;
        assert!(
            (week.total.estimate - expected).abs() < 1e-12,
            "{}",
            week.total.estimate
        );
        assert_eq!(week.total.unpriced, 50, "Antigravity's model has no price");
        assert!(
            (week.total.cost - 1.25).abs() < 1e-9,
            "reported cost is kept apart"
        );
    }

    #[test]
    fn ranges_start_and_bars_follow() {
        assert_eq!(week_start(date(2026, 11, 5)), date(2026, 11, 1));
        assert_eq!(date(2026, 11, 1).weekday(), Weekday::Sunday);
        assert_eq!(Range::Month.start(date(2026, 11, 30)), date(2026, 11, 1));
        assert_eq!(Range::Year.start(date(2026, 11, 30)), date(2025, 12, 1));
        assert_eq!(
            Range::Quarter.bucket(date(2026, 11, 5), date(2026, 8, 8)),
            date(2026, 11, 1)
        );
        assert_eq!(
            Range::Quarter.bucket(date(2026, 8, 10), date(2026, 8, 8)),
            date(2026, 8, 9)
        );
        assert_eq!(
            Range::Quarter.bucket(date(2026, 8, 8), date(2026, 8, 8)),
            date(2026, 8, 8)
        );
    }
}
