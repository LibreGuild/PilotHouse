//! Past sessions of an agent CLI in a folder, from the agent's own files
//! (only read), to resume in a terminal:
//!
//! - Claude Code: one JSONL file per session in
//!   `<CLAUDE_CONFIG_DIR or ~/.claude>/projects/<folder, other characters as ->/`.
//! - Codex: `<CODEX_HOME or ~/.codex>/sessions/<y>/<m>/<d>/rollout-*.jsonl`,
//!   whose first row names the folder.
//! - Gemini CLI: `~/.gemini/tmp/<name>/chats/session-*.jsonl`, the name from
//!   `~/.gemini/projects.json`.
//! - Antigravity: `~/.gemini/antigravity-cli/conversation_summaries.db`.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// How much of each end of a session file is read for its name.
const EDGE: u64 = 64 * 1024;
/// Most sessions listed.
const MOST: usize = 50;

/// One past session.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PastSession {
    pub id: String,
    pub title: String,
    /// When it last changed (Unix milliseconds).
    pub updated_at: i64,
    /// Its session file, for agents that keep one.
    pub path: Option<PathBuf>,
}

/// Agent `agent`'s sessions in `cwd`, newest first.
pub(crate) fn sessions(agent: &str, cwd: &Path) -> Vec<PastSession> {
    let mut sessions = match agent {
        "claude" => claude_sessions(cwd),
        "codex" => codex_sessions(cwd),
        "gemini" => gemini_sessions(cwd),
        "antigravity" => antigravity_sessions(cwd),
        _ => Vec::new(),
    };
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    sessions.truncate(MOST);
    sessions
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// An environment variable as the user's shell sets it.
fn shell_var(name: &str) -> Option<PathBuf> {
    ph_env::shell_env()
        .get(name)
        .cloned()
        .or_else(|| std::env::var(name).ok())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The path a `file://` URI names (`%20` and the like decoded; a Windows
/// one as `C:/…`).
fn file_uri_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |byte: u8| (byte as char).to_digit(16);
        match (bytes[at], bytes.get(at + 1), bytes.get(at + 2)) {
            (b'%', Some(&high), Some(&low)) if hex(high).is_some() && hex(low).is_some() => {
                decoded.push((hex(high)? * 16 + hex(low)?) as u8);
                at += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    let path = String::from_utf8(decoded).ok()?;
    // `/C:/repo` on Windows.
    let windows = path.as_bytes().get(2) == Some(&b':') && path.starts_with('/');
    Some(if windows { path[1..].to_string() } else { path })
}

fn modified_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as i64,
    )
}

/// The first `limit` bytes of a file, as text.
fn head(path: &Path, limit: u64) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// What the user typed, not context an agent put in a user message.
fn typed(text: &str) -> Option<String> {
    let mut text = text.trim_start();
    // PilotHouse's own note ahead of a chat's first message.
    if text.starts_with("(Workspace note from PilotHouse") {
        text = text.split_once("\n\n")?.1.trim_start();
    }
    if text.starts_with('<')
        || text.starts_with("# AGENTS.md")
        || text.starts_with("Here is a list of")
    {
        return None;
    }
    one_line(text)
}

/// A folder's entries, newest name first.
fn newest_first(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    entries.sort();
    entries.reverse();
    entries
}

/// Codex's sessions in `cwd`: the newest rollout files whose first row
/// names that folder.
fn codex_sessions(cwd: &Path) -> Vec<PastSession> {
    let root = shell_var("CODEX_HOME")
        .unwrap_or_else(|| home().join(".codex"))
        .join("sessions");
    let mut files = Vec::new();
    'walk: for year in newest_first(&root) {
        for month in newest_first(&year) {
            for day in newest_first(&month) {
                files.extend(
                    newest_first(&day)
                        .into_iter()
                        .filter(|file| file.extension().is_some_and(|ext| ext == "jsonl")),
                );
                if files.len() >= 300 {
                    break 'walk;
                }
            }
        }
    }
    files
        .into_iter()
        .filter_map(|file| {
            use std::io::BufRead as _;
            // The first row names the folder (and carries the agent's
            // instructions, so it can be long); other folders stop there.
            let mut reader = std::io::BufReader::new(std::fs::File::open(&file).ok()?);
            let mut first = String::new();
            (&mut reader).take(1024 * 1024).read_line(&mut first).ok()?;
            let meta: Value = serde_json::from_str(&first).ok()?;
            let payload = &meta["payload"];
            if meta["type"] != "session_meta" || Path::new(payload["cwd"].as_str()?) != cwd {
                return None;
            }
            let id = payload["id"].as_str()?.to_string();
            // Bytes, then lenient text: the cut can fall inside a character.
            let mut rest = Vec::new();
            reader.take(1024 * 1024).read_to_end(&mut rest).ok()?;
            let rest = String::from_utf8_lossy(&rest);
            let rows: Vec<Value> = rest
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect();
            // What the user typed (`user_message`), else the first user
            // message that is not context.
            let title = rows
                .iter()
                .find_map(|row| {
                    let payload = &row["payload"];
                    (payload["type"] == "user_message")
                        .then(|| payload["message"].as_str().and_then(typed))
                        .flatten()
                })
                .or_else(|| {
                    rows.iter().find_map(|row| {
                        let payload = &row["payload"];
                        if row["type"] != "response_item" || payload["role"] != "user" {
                            return None;
                        }
                        payload["content"]
                            .as_array()?
                            .iter()
                            .find_map(|block| block["text"].as_str().and_then(typed))
                    })
                })?;
            Some(PastSession {
                id,
                title,
                updated_at: modified_ms(&file)?,
                path: Some(file),
            })
        })
        .collect()
}

/// Gemini CLI's sessions in `cwd`.
fn gemini_sessions(cwd: &Path) -> Vec<PastSession> {
    let gemini = shell_var("GEMINI_CLI_HOME")
        .unwrap_or_else(home)
        .join(".gemini");
    let Some(projects) = std::fs::read_to_string(gemini.join("projects.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return Vec::new();
    };
    let Some(name) = projects["projects"][cwd.to_string_lossy().as_ref()].as_str() else {
        return Vec::new();
    };
    if name.contains(['/', '\\']) || name.starts_with('.') {
        return Vec::new();
    }
    let chats = gemini.join("tmp").join(name).join("chats");
    newest_first(&chats)
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "jsonl" || ext == "json")
        })
        .filter_map(|path| {
            let text = head(&path, 4 * EDGE)?;
            let rows: Vec<Value> = text
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            let id = rows
                .iter()
                .find_map(|row| row["sessionId"].as_str())?
                .to_string();
            let title = rows
                .iter()
                .flat_map(|row| {
                    row["$set"]["messages"]
                        .as_array()
                        .or_else(|| row["messages"].as_array())
                        .cloned()
                        .unwrap_or_default()
                })
                .filter(|message| message["type"] == "user")
                .find_map(|message| {
                    let content = &message["content"];
                    match content.as_str() {
                        Some(text) => typed(text),
                        None => content
                            .as_array()?
                            .iter()
                            .find_map(|part| part["text"].as_str().and_then(typed)),
                    }
                })?;
            Some(PastSession {
                id,
                title,
                updated_at: modified_ms(&path)?,
                path: Some(path),
            })
        })
        .collect()
}

/// Antigravity's conversations in `cwd`, from its summaries database.
fn antigravity_sessions(cwd: &Path) -> Vec<PastSession> {
    use rusqlite::{Connection, OpenFlags};
    let db = shell_var("GEMINI_CLI_HOME")
        .unwrap_or_else(home)
        .join(".gemini/antigravity-cli/conversation_summaries.db");
    if !db.is_file() {
        return Vec::new();
    }
    let Ok(connection) = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return Vec::new();
    };
    let folder = cwd.to_string_lossy().replace('\\', "/");
    let folder = folder.trim_end_matches('/');
    let Ok(mut statement) = connection.prepare(
        "SELECT conversation_id, title, workspace_uris, last_modified_time \
         FROM conversation_summaries WHERE parent_conversation_id = '' \
         ORDER BY last_modified_time DESC LIMIT 500",
    ) else {
        return Vec::new();
    };
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    rows.flatten()
        .filter(|(_, _, uris, _)| {
            serde_json::from_str::<Vec<String>>(uris).is_ok_and(|uris| {
                uris.iter()
                    .filter_map(|uri| file_uri_path(uri))
                    .any(|path| path.trim_end_matches('/') == folder)
            })
        })
        .filter_map(|(id, title, _, modified)| {
            Some(PastSession {
                id,
                title: one_line(&title)?,
                updated_at: sqlite_time_ms(&modified)?,
                path: None,
            })
        })
        .collect()
}

/// `2026-10-25 09:50:44.869866+00:00` in Unix milliseconds.
fn sqlite_time_ms(text: &str) -> Option<i64> {
    let iso = text.trim().replacen(' ', "T", 1);
    Some(iso.parse::<jiff::Timestamp>().ok()?.as_millisecond())
}

/// Claude Code's sessions in `cwd`, newest first.
fn claude_sessions(cwd: &Path) -> Vec<PastSession> {
    let dir = claude_config_dir()
        .join("projects")
        .join(project_dir_name(cwd));
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, i64)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            let millis = modified
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_millis() as i64;
            Some((entry.path(), millis))
        })
        .collect();
    files.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
    files
        .into_iter()
        .take(MOST)
        .filter_map(|(path, updated_at)| {
            let id = path.file_stem()?.to_string_lossy().into_owned();
            let title = title_of(&path)?;
            Some(PastSession {
                id,
                title,
                updated_at,
                path: Some(path),
            })
        })
        .collect()
}

/// Where Claude Code keeps its files: `CLAUDE_CONFIG_DIR` (as the user's
/// shell sets it), else `~/.claude`.
fn claude_config_dir() -> PathBuf {
    shell_var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home().join(".claude"))
}

/// The folder's name under `projects/`: every character but ASCII letters
/// and digits becomes `-`.
fn project_dir_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A session's name: Claude's own title for it, else the last thing
/// asked, else the first. None for a session with nothing said.
fn title_of(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut head = vec![0; EDGE.min(length) as usize];
    file.read_exact(&mut head).ok()?;
    let mut tail = Vec::new();
    if length > EDGE {
        file.seek(SeekFrom::Start(length.saturating_sub(EDGE).max(EDGE)))
            .ok()?;
        // Bytes: the seek can land inside a character.
        file.read_to_end(&mut tail).ok()?;
    }
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&head),
        String::from_utf8_lossy(&tail)
    );
    let rows: Vec<Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let named = |key: &str, field: &str| {
        rows.iter()
            .rev()
            .filter(|row| row["type"] == key)
            .find_map(|row| row[field].as_str().and_then(one_line))
    };
    named("ai-title", "aiTitle")
        .or_else(|| named("custom-title", "customTitle"))
        .or_else(|| named("last-prompt", "lastPrompt"))
        .or_else(|| {
            rows.iter()
                .filter(|row| row["type"] == "user")
                .find_map(|row| match &row["message"]["content"] {
                    Value::String(text) => one_line(text),
                    Value::Array(blocks) => blocks
                        .iter()
                        .find(|block| block["type"] == "text")
                        .and_then(|block| block["text"].as_str())
                        .and_then(one_line),
                    _ => None,
                })
        })
}

/// The first line that is not markup, cut to a title's length.
fn one_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('<'))
        .map(|line| line.chars().take(120).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_file_uris() {
        assert_eq!(
            file_uri_path("file:///Users/ada/My%20App").as_deref(),
            Some("/Users/ada/My App")
        );
        assert_eq!(file_uri_path("file:///C:/repo").as_deref(), Some("C:/repo"));
        assert_eq!(file_uri_path("https://x"), None);
    }

    #[test]
    fn reads_antigravity_times() {
        assert_eq!(
            sqlite_time_ms("2026-10-25 09:50:44.869866+00:00"),
            "2026-10-25T09:50:44.869866Z"
                .parse::<jiff::Timestamp>()
                .ok()
                .map(|at| at.as_millisecond())
        );
    }

    #[test]
    fn names_folders_like_claude() {
        assert_eq!(
            project_dir_name(Path::new("/Users/ada/my.app")),
            "-Users-ada-my-app"
        );
    }

    #[test]
    fn titles_sessions() {
        let dir = std::env::temp_dir().join(format!("ph-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let with_title = dir.join("a.jsonl");
        std::fs::write(
            &with_title,
            [
                r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#,
                r#"{"type":"user","message":{"content":[{"type":"text","text":"Fix the login bug"}]}}"#,
                r#"{"type":"ai-title","aiTitle":"Login bug fix"}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        assert_eq!(title_of(&with_title).as_deref(), Some("Login bug fix"));
        let plain = dir.join("b.jsonl");
        std::fs::write(
            &plain,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"Fix the login bug"}]}}"#,
        )
        .unwrap();
        assert_eq!(title_of(&plain).as_deref(), Some("Fix the login bug"));
        let empty = dir.join("c.jsonl");
        std::fs::write(&empty, r#"{"type":"mode","mode":"normal"}"#).unwrap();
        assert_eq!(title_of(&empty), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
