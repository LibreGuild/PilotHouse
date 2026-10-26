//! How close each agent subscription is to its limits, as the agents report
//! it while they work. Nothing is fetched for this; a window shows once its
//! agent has said something about it.
//!
//! - Claude Code's 5-hour and weekly windows come with its usage updates
//!   (`_meta["_claude/rateLimit"]`).
//! - Codex writes its windows into its session files
//!   (`$CODEX_HOME/sessions/…/rollout-*.jsonl`, `rate_limits`), whatever ran
//!   it (PilotHouse, its terminal, the IDE); the newest one is read every
//!   minute.

use std::collections::BTreeMap;

use gpui_kit::{App, Global};
use serde_json::Value;

/// One limit window of one agent.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    /// `5h`, `week`, `week (Opus)`…
    pub window: String,
    /// How much of it is used, 0 to 1, when the agent says.
    pub used: Option<f32>,
    /// When it resets (Unix seconds).
    pub resets_at: Option<i64>,
    pub state: LimitState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LimitState {
    Fine,
    /// Close to the limit.
    Warning,
    /// The limit was hit.
    Reached,
}

/// Every agent's limits, by agent name then window.
#[derive(Default)]
pub struct RateLimits(BTreeMap<&'static str, BTreeMap<String, Limit>>);

impl Global for RateLimits {}

impl RateLimits {
    /// The agents' limits so far: agent name and its windows.
    pub fn all(cx: &App) -> Vec<(&'static str, Vec<Limit>)> {
        cx.try_global::<Self>()
            .map(|limits| {
                limits
                    .0
                    .iter()
                    .map(|(agent, windows)| (*agent, windows.values().cloned().collect()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Records what `agent` reported (a Claude `rateLimit` object).
    pub(crate) fn record(agent: &'static str, info: &Value, cx: &mut App) {
        let Some(limit) = parse(info) else { return };
        cx.default_global::<Self>()
            .0
            .entry(agent)
            .or_default()
            .insert(limit.window.clone(), limit);
    }

    /// Starts reading Codex's windows from its session files, once a
    /// minute, for the app's life.
    pub fn watch_codex(cx: &mut App) {
        let Some(sessions) = codex_sessions_dir().filter(|dir| dir.is_dir()) else {
            return;
        };
        cx.spawn(async move |cx| {
            loop {
                let dir = sessions.clone();
                let limits = cx
                    .background_executor()
                    .spawn(async move { codex_limits(&dir) })
                    .await;
                if !limits.is_empty() {
                    cx.update(|cx| {
                        let windows = cx.default_global::<Self>().0.entry("Codex").or_default();
                        windows.clear();
                        for limit in limits {
                            windows.insert(limit.window.clone(), limit);
                        }
                        cx.refresh_windows();
                    });
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(60))
                    .await;
            }
        })
        .detach();
    }
}

/// `$CODEX_HOME/sessions`, else `~/.codex/sessions`.
fn codex_sessions_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))?;
    Some(home.join("sessions"))
}

/// The newest file in the newest folder, down `depth` levels of
/// `year/month/day` folders.
fn newest_file(dir: &std::path::Path, depth: usize) -> Option<std::path::PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    if depth == 0 {
        return entries
            .into_iter()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            .max_by_key(|entry| entry.metadata().and_then(|meta| meta.modified()).ok())
            .map(|entry| entry.path());
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
    entries
        .into_iter()
        .filter(|entry| entry.path().is_dir())
        .find_map(|entry| newest_file(&entry.path(), depth - 1))
}

/// Codex's windows as its newest session file last recorded them.
fn codex_limits(sessions: &std::path::Path) -> Vec<Limit> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let Some(path) = newest_file(sessions, 3) else {
        return Vec::new();
    };
    let Ok(mut file) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(length.saturating_sub(512 * 1024)));
    // Bytes: the seek can land inside a character.
    let mut tail = Vec::new();
    if file.read_to_end(&mut tail).is_err() {
        return Vec::new();
    }
    String::from_utf8_lossy(&tail)
        .lines()
        .rev()
        .filter(|line| line.contains("\"rate_limits\""))
        .find_map(|line| {
            let row: Value = serde_json::from_str(line).ok()?;
            let limits = find_key(&row, "rate_limits")?;
            let windows: Vec<Limit> = ["primary", "secondary"]
                .iter()
                .filter_map(|key| codex_window(&limits[*key]))
                .collect();
            (!windows.is_empty()).then_some(windows)
        })
        .unwrap_or_default()
}

fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map
            .get(key)
            .filter(|found| found.is_object())
            .or_else(|| map.values().find_map(|inner| find_key(inner, key))),
        _ => None,
    }
}

/// One Codex window: `{used_percent, window_minutes, resets_at}`.
fn codex_window(window: &Value) -> Option<Limit> {
    let minutes = window["window_minutes"].as_u64()?;
    let resets_at = window["resets_at"].as_i64();
    // The newest session can be old: a window that has reset since is unused.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    let reset = resets_at.is_some_and(|at| at <= now);
    let used = window["used_percent"].as_f64().map(|used| {
        if reset {
            0.0
        } else {
            (used / 100.0).clamp(0.0, 1.0) as f32
        }
    });
    let name = match minutes {
        300 => "5h".to_string(),
        10_080 => "week".to_string(),
        43_200 => "month".to_string(),
        minutes if minutes % 1440 == 0 => format!("{}d", minutes / 1440),
        minutes => format!("{}h", minutes.div_ceil(60)),
    };
    let state = match used {
        Some(used) if used >= 1.0 => LimitState::Reached,
        Some(used) if used >= 0.8 => LimitState::Warning,
        _ => LimitState::Fine,
    };
    Some(Limit {
        window: name,
        used,
        resets_at: resets_at.filter(|_| !reset),
        state,
    })
}

/// A Claude `SDKRateLimitInfo`: `{status, rateLimitType, utilization,
/// resetsAt}`.
fn parse(info: &Value) -> Option<Limit> {
    let window = match info["rateLimitType"].as_str()? {
        "five_hour" => "5h".to_string(),
        "seven_day" => "week".to_string(),
        "seven_day_opus" => "week (Opus)".to_string(),
        "seven_day_sonnet" => "week (Sonnet)".to_string(),
        other => other.replace('_', " "),
    };
    let state = match info["status"].as_str() {
        Some("allowed_warning") => LimitState::Warning,
        Some("rejected") => LimitState::Reached,
        _ => LimitState::Fine,
    };
    // A fraction, or a percentage from some versions.
    let used = info["utilization"]
        .as_f64()
        .map(|used| if used > 1.0 { used / 100.0 } else { used })
        .map(|used| used.clamp(0.0, 1.0) as f32);
    Some(Limit {
        window,
        used,
        resets_at: info["resetsAt"].as_i64(),
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claude_limit_info() {
        let limit = parse(&serde_json::json!({
            "status": "allowed_warning", "rateLimitType": "five_hour",
            "utilization": 0.82, "resetsAt": 1_800_000_000
        }))
        .unwrap();
        assert_eq!(limit.window, "5h");
        assert_eq!(limit.state, LimitState::Warning);
        assert_eq!(limit.used, Some(0.82));
        let percent =
            parse(&serde_json::json!({ "rateLimitType": "seven_day", "utilization": 41 })).unwrap();
        assert_eq!(
            (percent.window.as_str(), percent.used),
            ("week", Some(0.41))
        );
        assert!(parse(&serde_json::json!({ "status": "allowed" })).is_none());
    }

    #[test]
    fn reads_codex_session_files() {
        let dir = std::env::temp_dir().join(format!("ph-codex-limits-{}", std::process::id()));
        let day = dir.join("2026/10/25");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-a.jsonl"),
            [
                r#"{"type":"session_meta","payload":{"id":"x"}}"#,
                r#"{"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":85.0,"window_minutes":300,"resets_at":99999999999},"secondary":{"used_percent":90.0,"window_minutes":10080,"resets_at":2}}}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let limits = codex_limits(&dir);
        assert_eq!(limits.len(), 2);
        assert_eq!(
            (limits[0].window.as_str(), limits[0].state),
            ("5h", LimitState::Warning)
        );
        assert_eq!(limits[1].window, "week");
        assert_eq!(
            (limits[1].used, limits[1].state),
            (Some(0.0), LimitState::Fine),
            "reset since it was recorded"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
