//! Token usage the agents' CLIs keep on this computer, so the usage
//! dashboard counts every session, wherever it ran (a terminal, an IDE or
//! PilotHouse, which runs the same CLIs):
//!
//! - Claude Code: `<CLAUDE_CONFIG_DIR or ~/.claude>/projects/**/*.jsonl`
//!   (and `~/.config/claude/projects`), a row per model reply with its
//!   `usage`, `model` and `cwd`. A resumed session copies earlier replies
//!   into its new file, so a reply is counted once by its message and
//!   request ids.
//! - Codex: `<CODEX_HOME or ~/.codex>/sessions/**/rollout-*.jsonl` (and
//!   `archived_sessions`): a `token_usage_record` per response, or in older
//!   files the running totals of `token_count` events; the model from
//!   `turn_context`, the folder from `session_meta`.
//! - Gemini CLI: `<GEMINI_CLI_HOME or ~>/.gemini/tmp/*/chats/session-*`, a
//!   `tokens` object per reply (a reply is written again as it grows; the
//!   last one counts).
//! - Cline: `<CLINE_DIR or ~/.cline>/data/sessions/<id>/<id>.messages.json`,
//!   rewritten whole as the session goes, with `metrics` on each reply; the
//!   folder from `<id>.json` beside it.
//!
//! These files can add up to gigabytes, so: lines are matched as bytes
//! before any is parsed, only the fields needed are parsed, files are read
//! in parallel, and what a file held is kept (`<data dir>/usage-files.json`,
//! token counts only) so a later scan reads only files that changed, and of
//! a Claude Code file only what was added. Only read, off the UI thread;
//! nothing is sent anywhere. The files carry tokens, not prices.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead as _, BufReader, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One model reply's tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The agent id (`claude`, `codex`, `gemini`, `cline`).
    pub agent: &'static str,
    /// When it came (Unix milliseconds).
    pub at: i64,
    pub model: Arc<str>,
    pub session: Arc<str>,
    /// The folder the session worked in; empty when not recorded.
    pub cwd: Arc<str>,
    /// Input not read from a cache (writing to one included).
    pub input: u64,
    /// Of `input`, what was written to a cache (priced differently).
    pub cache_write: u64,
    /// Input read from a cache.
    pub cached: u64,
    /// Output, reasoning included.
    pub output: u64,
    /// Names the reply wherever it is copied; 0 when there is nothing to
    /// name it by.
    key: u64,
}

impl Entry {
    /// A reply with nothing to name it by: `(input, cached, output)` tokens.
    pub fn new(
        agent: &'static str,
        at: i64,
        model: &str,
        session: &str,
        cwd: &str,
        (input, cached, output): (u64, u64, u64),
    ) -> Self {
        Self {
            agent,
            at,
            model: model.into(),
            session: session.into(),
            cwd: cwd.into(),
            input,
            cache_write: 0,
            cached,
            output,
            key: 0,
        }
    }
}

/// What a scan found: the replies, and for each agent whose files were
/// there, where they were looked for.
#[derive(Debug, Default)]
pub struct Scan {
    pub entries: Vec<Entry>,
    pub sources: Vec<(&'static str, PathBuf)>,
}

/// How far a scan has got, for showing while it runs.
#[derive(Debug, Default)]
pub struct Progress {
    pub files: AtomicUsize,
    pub done: AtomicUsize,
}

const AGENTS: [&str; 4] = ["claude", "codex", "gemini", "cline"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Claude,
    Codex,
    Gemini,
    Cline,
}

impl Kind {
    fn agent(self) -> &'static str {
        AGENTS[self as usize]
    }
}

/// Where each agent keeps its files.
#[derive(Debug, Default, Clone)]
struct Roots {
    claude: Vec<PathBuf>,
    codex: Vec<PathBuf>,
    gemini: Vec<PathBuf>,
    cline: Vec<PathBuf>,
}

impl Roots {
    fn of_this_computer() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let var = |name: &str| {
            ph_env::shell_env()
                .get(name)
                .cloned()
                .or_else(|| std::env::var(name).ok())
                .filter(|value| !value.is_empty())
        };
        let mut claude: Vec<PathBuf> = match var("CLAUDE_CONFIG_DIR") {
            // A list, as Claude Code allows.
            Some(dirs) => dirs
                .split(',')
                .map(|dir| PathBuf::from(dir.trim()).join("projects"))
                .collect(),
            None => vec![
                home.join(".claude/projects"),
                home.join(".config/claude/projects"),
            ],
        };
        claude.dedup();
        let codex_home = var("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let gemini_home = var("GEMINI_CLI_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.clone());
        Self {
            claude,
            codex: vec![
                codex_home.join("sessions"),
                codex_home.join("archived_sessions"),
            ],
            gemini: vec![gemini_home.join(".gemini/tmp")],
            cline: vec![crate::cline::Paths::under(&home, &var).sessions],
        }
    }
}

/// What one file held: its replies, with the strings they repeat kept
/// once. Saved between runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileUsage {
    /// The file's length and time (Unix milliseconds) when read.
    len: u64,
    modified: i64,
    /// How far it was read: the end of its last whole line.
    read_to: u64,
    names: Vec<String>,
    rows: Vec<SavedRow>,
}

/// `(at, model, session, cwd, input, cache_write, cached, output, key)`, the
/// strings as indexes into [`FileUsage::names`].
type SavedRow = (i64, u32, u32, u32, u64, u64, u64, u64, u64);

impl FileUsage {
    fn name(&mut self, index: &mut HashMap<String, u32>, name: &str) -> u32 {
        if let Some(ix) = index.get(name) {
            return *ix;
        }
        let ix = self.names.len() as u32;
        self.names.push(name.to_string());
        index.insert(name.to_string(), ix);
        ix
    }

    fn push(&mut self, index: &mut HashMap<String, u32>, row: Row<'_>) {
        let model = self.name(index, row.model);
        let session = self.name(index, row.session);
        let cwd = self.name(index, row.cwd);
        self.rows.push((
            row.at,
            model,
            session,
            cwd,
            row.input,
            row.cache_write,
            row.cached,
            row.output,
            row.key,
        ));
    }
}

/// A reply as a file's parser finds it.
struct Row<'a> {
    at: i64,
    model: &'a str,
    session: &'a str,
    cwd: &'a str,
    input: u64,
    cache_write: u64,
    cached: u64,
    output: u64,
    key: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    version: u32,
    files: HashMap<PathBuf, FileUsage>,
}

/// Bumped when what is kept per file changes: older ones are read again.
const SAVED_VERSION: u32 = 2;

struct Cache {
    loaded: bool,
    files: HashMap<PathBuf, FileUsage>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| {
    Mutex::new(Cache {
        loaded: false,
        files: HashMap::new(),
    })
});

fn saved_path() -> PathBuf {
    ph_store::paths::data_dir().join("usage-files.json")
}

/// Every reply since `since` (Unix milliseconds) the agents' files on this
/// computer record, each counted once, with `progress` kept up to date.
/// Blocking: run it off the UI thread.
pub fn scan(since: i64, progress: &Progress) -> Scan {
    scan_roots(
        &Roots::of_this_computer(),
        since,
        progress,
        Some(&saved_path()),
    )
}

fn scan_roots(roots: &Roots, since: i64, progress: &Progress, saved: Option<&Path>) -> Scan {
    let mut cache = CACHE.lock().unwrap_or_else(|err| err.into_inner());
    if !cache.loaded {
        cache.loaded = true;
        if let Some(saved) = saved
            && let Ok(bytes) = std::fs::read(saved)
            && let Ok(saved) = serde_json::from_slice::<Saved>(&bytes)
            && saved.version == SAVED_VERSION
        {
            cache.files = saved.files;
        }
    }
    let mut sources = Vec::new();
    let mut files: Vec<(PathBuf, Kind)> = Vec::new();
    for (dirs, kind) in [
        (&roots.claude, Kind::Claude),
        (&roots.codex, Kind::Codex),
        (&roots.gemini, Kind::Gemini),
        (&roots.cline, Kind::Cline),
    ] {
        let found: Vec<&PathBuf> = dirs.iter().filter(|dir| dir.is_dir()).collect();
        if let Some(first) = found.first() {
            sources.push((kind.agent(), (*first).clone()));
        }
        for dir in found {
            files.extend(list(dir, kind, since).into_iter().map(|file| (file, kind)));
        }
    }
    progress.files.store(files.len(), Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);

    // What changed is read again, in parallel.
    let mut todo = Vec::new();
    let mut fresh = HashMap::new();
    for (path, kind) in &files {
        let (len, modified) = stat(path);
        match cache.files.remove(path) {
            Some(usage) if usage.len == len && usage.modified == modified => {
                progress.done.fetch_add(1, Ordering::Relaxed);
                fresh.insert(path.clone(), usage);
            }
            before => todo.push((path.clone(), *kind, before, len, modified)),
        }
    }
    let changed = !todo.is_empty() || !cache.files.is_empty();
    let read: Mutex<Vec<(PathBuf, FileUsage)>> = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8);
    std::thread::scope(|scope| {
        for _ in 0..workers.min(todo.len()) {
            scope.spawn(|| {
                loop {
                    let ix = next.fetch_add(1, Ordering::Relaxed);
                    let Some((path, kind, before, len, modified)) = todo.get(ix) else {
                        break;
                    };
                    let usage = read_file(path, *kind, before.clone(), *len, *modified);
                    progress.done.fetch_add(1, Ordering::Relaxed);
                    read.lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .push((path.clone(), usage));
                }
            });
        }
    });
    fresh.extend(read.into_inner().unwrap_or_else(|err| err.into_inner()));
    // Files gone (or now too old to read) are forgotten.
    cache.files = fresh;
    if changed && let Some(saved) = saved {
        save(saved, &cache.files);
    }

    // Each reply once, newest scan order: Claude, then Codex, then Gemini.
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for (path, kind) in &files {
        let Some(usage) = cache.files.get(path) else {
            continue;
        };
        let names: Vec<Arc<str>> = usage
            .names
            .iter()
            .map(|name| Arc::from(name.as_str()))
            .collect();
        let name = |ix: u32| {
            names
                .get(ix as usize)
                .cloned()
                .unwrap_or_else(|| Arc::from(""))
        };
        for &(at, model, session, cwd, input, cache_write, cached, output, key) in &usage.rows {
            if at < since || (key != 0 && !seen.insert(key)) {
                continue;
            }
            entries.push(Entry {
                agent: kind.agent(),
                at,
                model: name(model),
                session: name(session),
                cwd: name(cwd),
                input,
                cache_write,
                cached,
                output,
                key,
            });
        }
    }
    Scan { entries, sources }
}

fn save(path: &Path, files: &HashMap<PathBuf, FileUsage>) {
    #[derive(Serialize)]
    struct SavedRef<'a> {
        version: u32,
        files: &'a HashMap<PathBuf, FileUsage>,
    }
    let Ok(bytes) = serde_json::to_vec(&SavedRef {
        version: SAVED_VERSION,
        files,
    }) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let partial = path.with_extension("json.part");
    if std::fs::write(&partial, bytes).is_ok() {
        let _ = std::fs::rename(&partial, path);
    }
}

fn stat(path: &Path) -> (u64, i64) {
    let meta = std::fs::metadata(path).ok();
    let len = meta.as_ref().map_or(0, |meta| meta.len());
    let modified = meta
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis() as i64);
    (len, modified)
}

/// The session files under `dir` changed since `since` (an older one holds
/// nothing newer).
fn list(dir: &Path, kind: Kind, since: i64) -> Vec<PathBuf> {
    let since = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(since.max(0) as u64);
    let mut found = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if depth < 5 {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let ext = path.extension().and_then(|ext| ext.to_str());
            let wanted = match kind {
                Kind::Claude | Kind::Codex => ext == Some("jsonl"),
                Kind::Gemini => {
                    matches!(ext, Some("jsonl" | "json"))
                        && dir.file_name().is_some_and(|name| name == "chats")
                }
                Kind::Cline => path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".messages.json")),
            };
            if wanted && meta.modified().is_ok_and(|modified| modified >= since) {
                found.push(path);
            }
        }
    }
    found
}

/// Reads `path` (`len` bytes, modified at `modified`). A Claude Code file
/// that only grew is read from where `before` stopped; any other change
/// reads it all.
fn read_file(
    path: &Path,
    kind: Kind,
    before: Option<FileUsage>,
    len: u64,
    modified: i64,
) -> FileUsage {
    let grown = before.filter(|before| kind == Kind::Claude && len >= before.read_to);
    let mut usage = grown.unwrap_or_default();
    let mut index: HashMap<String, u32> = usage
        .names
        .iter()
        .enumerate()
        .map(|(ix, name)| (name.clone(), ix as u32))
        .collect();
    usage.read_to = match kind {
        Kind::Claude => claude_file(path, usage.read_to, &mut usage, &mut index),
        Kind::Codex => codex_file(path, &mut usage, &mut index),
        Kind::Gemini => gemini_file(path, &mut usage, &mut index),
        Kind::Cline => cline_file(path, &mut usage, &mut index),
    };
    usage.len = len;
    usage.modified = modified;
    usage
}

/// Calls `line` with each whole line of `path` from `from` on that holds
/// every one of `needles`; returns where the last whole line ended.
fn lines(path: &Path, from: u64, needles: &[&[u8]], mut line: impl FnMut(&[u8])) -> u64 {
    let Ok(mut file) = std::fs::File::open(path) else {
        return from;
    };
    if file.seek(SeekFrom::Start(from)).is_err() {
        return from;
    }
    let finders: Vec<_> = needles.iter().map(memchr::memmem::Finder::new).collect();
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut read_to = from;
    loop {
        buffer.clear();
        let Ok(read) = reader.read_until(b'\n', &mut buffer) else {
            break;
        };
        // The end, or a line still being written: left for the next read.
        if read == 0 || buffer.last() != Some(&b'\n') {
            break;
        }
        read_to += read as u64;
        if finders.iter().all(|finder| finder.find(&buffer).is_some()) {
            line(&buffer);
        }
    }
    read_to
}

fn millis(at: &str) -> Option<i64> {
    at.parse::<jiff::Timestamp>()
        .ok()
        .map(|at| at.as_millisecond())
}

/// A stable 64-bit name for a reply (FNV-1a), never 0.
fn key(parts: &[&str]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.bytes().chain([0xff]) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash.max(1)
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct ClaudeRow<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(borrow)]
    timestamp: Option<Cow<'a, str>>,
    #[serde(rename = "sessionId", borrow)]
    session: Option<Cow<'a, str>>,
    #[serde(rename = "requestId", borrow)]
    request: Option<Cow<'a, str>>,
    #[serde(borrow)]
    cwd: Option<Cow<'a, str>>,
    #[serde(borrow)]
    message: Option<ClaudeMessage<'a>>,
}

#[derive(Deserialize)]
struct ClaudeMessage<'a> {
    #[serde(borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    model: Option<Cow<'a, str>>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClaudeUsage {
    input_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    output_tokens: u64,
}

/// Claude Code: `{"type":"assistant","sessionId","timestamp","requestId",
/// "cwd","message":{"id","model","usage":{…}}}`, from byte `from` on.
fn claude_file(
    path: &Path,
    from: u64,
    usage: &mut FileUsage,
    index: &mut HashMap<String, u32>,
) -> u64 {
    let fallback = stem(path);
    lines(path, from, &[b"\"assistant\"", b"\"usage\""], |line| {
        let Ok(row) = serde_json::from_slice::<ClaudeRow>(line) else {
            return;
        };
        let (Some("assistant"), Some(message)) = (row.kind.as_deref(), row.message) else {
            return;
        };
        let Some(tokens) = message.usage else { return };
        let model = message.model.as_deref().unwrap_or_default();
        // Claude Code's own notes, not a model's reply.
        if model == "<synthetic>" {
            return;
        }
        let Some(at) = row.timestamp.as_deref().and_then(millis) else {
            return;
        };
        let key = match (message.id.as_deref(), row.request.as_deref()) {
            (Some(id), request) => self::key(&["claude", id, request.unwrap_or_default()]),
            _ => 0,
        };
        usage.push(
            index,
            Row {
                at,
                model,
                session: row.session.as_deref().unwrap_or(&fallback),
                cwd: row.cwd.as_deref().unwrap_or_default(),
                input: tokens.input_tokens + tokens.cache_creation_input_tokens,
                cache_write: tokens.cache_creation_input_tokens,
                cached: tokens.cache_read_input_tokens,
                output: tokens.output_tokens,
                key,
            },
        );
    })
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Codex: per response (`token_usage_record`), else the growth of the
/// running totals (`token_count`). Its `input_tokens` include the cached.
fn codex_file(path: &Path, usage: &mut FileUsage, index: &mut HashMap<String, u32>) -> u64 {
    let mut model = String::new();
    let mut session = stem(path);
    let mut cwd = String::new();
    let mut records: Vec<(i64, String, u64, u64, u64, u64, u64)> = Vec::new();
    let mut counts: Vec<(i64, String, u64, u64, u64)> = Vec::new();
    let mut last_total: Option<(u64, u64, u64)> = None;
    let read_to = lines(path, 0, &[], |line| {
        let Ok(row) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        let payload = &row["payload"];
        let at = || row["timestamp"].as_str().and_then(millis);
        match (row["type"].as_str(), payload["type"].as_str()) {
            (Some("session_meta"), _) => {
                if let Some(id) = payload["id"].as_str() {
                    session = id.to_string();
                }
                if let Some(folder) = payload["cwd"].as_str() {
                    cwd = folder.to_string();
                }
            }
            (Some("turn_context"), _) => {
                if let Some(name) = payload["model"].as_str() {
                    model = name.to_string();
                }
            }
            (Some("token_usage_record"), _) => {
                let tokens = &payload["usage"];
                let Some(at) = at() else { return };
                let input = number(tokens, "input_tokens");
                let cached = number(tokens, "cached_input_tokens");
                records.push((
                    at,
                    payload["model"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| model.clone()),
                    input.saturating_sub(cached),
                    number(tokens, "cache_write_input_tokens"),
                    cached,
                    number(tokens, "output_tokens"),
                    payload["response_id"]
                        .as_str()
                        .map_or(0, |id| key(&["codex", id])),
                ));
            }
            (Some("event_msg"), Some("token_count")) => {
                let total = &payload["info"]["total_token_usage"];
                if !total.is_object() {
                    return;
                }
                let Some(at) = at() else { return };
                let now = (
                    number(total, "input_tokens"),
                    number(total, "cached_input_tokens"),
                    number(total, "output_tokens"),
                );
                let before = last_total.unwrap_or((0, 0, 0));
                last_total = Some(now);
                let (input, cached, output) = (
                    now.0.saturating_sub(before.0),
                    now.1.saturating_sub(before.1),
                    now.2.saturating_sub(before.2),
                );
                if input + output > 0 {
                    counts.push((
                        at,
                        model.clone(),
                        input.saturating_sub(cached),
                        cached,
                        output,
                    ));
                }
            }
            _ => {}
        }
    });
    if records.is_empty() {
        for (at, model, input, cached, output) in counts {
            let row = Row {
                at,
                model: &model,
                session: &session,
                cwd: &cwd,
                input,
                cache_write: 0,
                cached,
                output,
                key: 0,
            };
            usage.push(index, row);
        }
    } else {
        for (at, model, input, cache_write, cached, output, key) in records {
            let row = Row {
                at,
                model: &model,
                session: &session,
                cwd: &cwd,
                input,
                cache_write: cache_write.min(input),
                cached,
                output,
                key,
            };
            usage.push(index, row);
        }
    }
    read_to
}

/// Gemini CLI: `{"id","type":"gemini","timestamp","model","tokens":{input,
/// output, cached, thoughts}}` rows (JSONL), or one JSON file with
/// `messages` in older versions. Its `input` includes the cached.
fn gemini_file(path: &Path, usage: &mut FileUsage, index: &mut HashMap<String, u32>) -> u64 {
    let session = stem(path);
    let mut messages: Vec<Value> = Vec::new();
    let read_to = if path.extension().is_some_and(|ext| ext == "json") {
        let file = std::fs::read(path).unwrap_or_default();
        if let Ok(file) = serde_json::from_slice::<Value>(&file)
            && let Some(list) = file.get("messages").and_then(Value::as_array)
        {
            messages.extend(list.iter().cloned());
        }
        file.len() as u64
    } else {
        lines(path, 0, &[b"\"tokens\""], |line| {
            if let Ok(row) = serde_json::from_slice::<Value>(line) {
                messages.push(row);
            }
        })
    };
    // A reply is written again as it grows: the last one counts.
    let mut order: Vec<String> = Vec::new();
    let mut by_id: HashMap<String, (i64, String, u64, u64, u64)> = HashMap::new();
    for message in &messages {
        let Some(tokens) = message.get("tokens").filter(|tokens| tokens.is_object()) else {
            continue;
        };
        let Some(at) = message["timestamp"].as_str().and_then(millis) else {
            continue;
        };
        // A reply without an id is its own (only an id says two are one).
        let id = message["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map_or_else(|| format!("\0{}", order.len()), str::to_string);
        let input = number(tokens, "input");
        let cached = number(tokens, "cached");
        let reply = (
            at,
            message["model"].as_str().unwrap_or_default().to_string(),
            input.saturating_sub(cached),
            cached,
            number(tokens, "output") + number(tokens, "thoughts"),
        );
        if by_id.insert(id.clone(), reply).is_none() {
            order.push(id);
        }
    }
    for id in order {
        let Some((at, model, input, cached, output)) = by_id.remove(&id) else {
            continue;
        };
        let row = Row {
            at,
            model: &model,
            session: &session,
            cwd: "",
            input,
            cache_write: 0,
            cached,
            output,
            key: if id.starts_with('\0') {
                0
            } else {
                key(&["gemini", &session, &id])
            },
        };
        usage.push(index, row);
    }
    read_to
}

/// Cline: `{"sessionId", "messages": [{"id", "role": "assistant", "ts",
/// "modelInfo": {"id"}, "metrics": {inputTokens, outputTokens,
/// cacheReadTokens, cacheWriteTokens}}]}`, rewritten whole; the folder the
/// session worked in is in `<id>.json` beside it. Its `inputTokens` leave
/// out the cache.
fn cline_file(path: &Path, usage: &mut FileUsage, index: &mut HashMap<String, u32>) -> u64 {
    let bytes = std::fs::read(path).unwrap_or_default();
    let Ok(file) = serde_json::from_slice::<Value>(&bytes) else {
        return bytes.len() as u64;
    };
    let session = file["sessionId"].as_str().map_or_else(
        || stem(path).trim_end_matches(".messages").to_string(),
        str::to_string,
    );
    let cwd = path
        .parent()
        .map(|dir| dir.join(format!("{session}.json")))
        .and_then(|meta| std::fs::read(meta).ok())
        .and_then(|meta| serde_json::from_slice::<Value>(&meta).ok())
        .and_then(|meta| meta["cwd"].as_str().map(str::to_string))
        .unwrap_or_default();
    for message in file["messages"].as_array().into_iter().flatten() {
        let metrics = &message["metrics"];
        if message["role"].as_str() != Some("assistant") || !metrics.is_object() {
            continue;
        }
        let Some(at) = message["ts"].as_i64() else {
            continue;
        };
        let cache_write = number(metrics, "cacheWriteTokens");
        let id = message["id"].as_str().unwrap_or_default();
        let row = Row {
            at,
            model: message["modelInfo"]["id"].as_str().unwrap_or_default(),
            session: &session,
            cwd: &cwd,
            input: number(metrics, "inputTokens") + cache_write,
            cache_write,
            cached: number(metrics, "cacheReadTokens"),
            output: number(metrics, "outputTokens"),
            key: if id.is_empty() {
                0
            } else {
                key(&["cline", &session, id])
            },
        };
        usage.push(index, row);
    }
    bytes.len() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ph-usage-logs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn scan(roots: &Roots, since: i64) -> Scan {
        scan_roots(roots, since, &Progress::default(), None)
    }

    #[test]
    fn claude_replies_count_once_across_resumed_files() {
        let root = folder("claude");
        let project = root.join("-Users-me-harbor");
        std::fs::create_dir_all(&project).unwrap();
        let reply = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","cwd":"/Users/me/harbor","timestamp":"2026-11-02T10:00:00Z","message":{"id":"m1","model":"claude-opus-5-5","content":[{"type":"text","text":"with \"usage\" and \"assistant\" inside"}],"usage":{"input_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":900,"output_tokens":40}}}"#;
        let synthetic = r#"{"type":"assistant","sessionId":"s1","timestamp":"2026-11-02T10:01:00Z","message":{"id":"m2","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#;
        let user = r#"{"type":"user","sessionId":"s1","timestamp":"2026-11-02T09:59:00Z","message":{"content":"has \"usage\" and \"assistant\" in it"}}"#;
        std::fs::write(
            project.join("s1.jsonl"),
            [user, reply, reply, synthetic, ""].join("\n"),
        )
        .unwrap();
        // Resumed: the same reply copied into a new session file.
        std::fs::write(project.join("s2.jsonl"), format!("{reply}\n")).unwrap();
        let roots = Roots {
            claude: vec![root.clone()],
            ..Default::default()
        };
        let found = scan(&roots, 0);
        assert_eq!(found.entries.len(), 1, "{:?}", found.entries);
        let entry = &found.entries[0];
        assert_eq!(
            (
                &*entry.model,
                &*entry.cwd,
                entry.input,
                entry.cached,
                entry.output
            ),
            ("claude-opus-5-5", "/Users/me/harbor", 102, 900, 40)
        );
        assert_eq!(entry.cache_write, 100, "of the input, written to the cache");
        assert_eq!(found.sources, [("claude", root.clone())]);
        // Before `since`: not counted.
        let later = millis("2026-11-03T00:00:00Z").unwrap();
        assert!(scan(&roots, later).entries.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_reads_responses_or_running_totals() {
        let root = folder("codex");
        let day = root.join("2026/11/02");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-new.jsonl"),
            [
                r#"{"timestamp":"2026-11-02T10:00:00Z","type":"session_meta","payload":{"id":"c1","cwd":"/work/tide"}}"#,
                r#"{"timestamp":"2026-11-02T10:00:01Z","type":"turn_context","payload":{"model":"gpt-6-luna"}}"#,
                r#"{"timestamp":"2026-11-02T10:00:02Z","type":"token_usage_record","payload":{"response_id":"resp1","usage":{"input_tokens":1000,"cached_input_tokens":600,"output_tokens":50}}}"#,
                r#"{"timestamp":"2026-11-02T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":600,"output_tokens":50}}}}"#,
                "",
            ]
            .join("\n"),
        )
        .unwrap();
        std::fs::write(
            day.join("rollout-old.jsonl"),
            [
                r#"{"timestamp":"2026-11-02T11:00:00Z","type":"turn_context","payload":{"model":"gpt-6-terra"}}"#,
                r#"{"timestamp":"2026-11-02T11:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":500,"cached_input_tokens":100,"output_tokens":20}}}}"#,
                r#"{"timestamp":"2026-11-02T11:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":500,"cached_input_tokens":100,"output_tokens":20}}}}"#,
                r#"{"timestamp":"2026-11-02T11:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":800,"cached_input_tokens":300,"output_tokens":30}}}}"#,
                "",
            ]
            .join("\n"),
        )
        .unwrap();
        let roots = Roots {
            codex: vec![root.clone()],
            ..Default::default()
        };
        let mut entries = scan(&roots, 0).entries;
        entries.sort_by_key(|entry| entry.at);
        let tokens: Vec<_> = entries
            .iter()
            .map(|entry| (&*entry.model, entry.input, entry.cached, entry.output))
            .collect();
        assert_eq!(
            tokens,
            [
                ("gpt-6-luna", 400, 600, 50),
                ("gpt-6-terra", 400, 100, 20),
                ("gpt-6-terra", 100, 200, 10),
            ],
            "a repeated total adds nothing"
        );
        assert_eq!(
            (&*entries[0].session, &*entries[0].cwd),
            ("c1", "/work/tide")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gemini_counts_the_last_write_of_each_reply() {
        let root = folder("gemini");
        let chats = root.join("harbor/chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(
            chats.join("session-1.jsonl"),
            [
                r#"{"id":"1","type":"user","timestamp":"2026-11-02T10:00:00Z","content":"hi"}"#,
                r#"{"id":"2","type":"gemini","timestamp":"2026-11-02T10:00:01Z","model":"gemini-3.8-flash","tokens":{"input":100,"output":1,"cached":20,"thoughts":0}}"#,
                r#"{"id":"2","type":"gemini","timestamp":"2026-11-02T10:00:02Z","model":"gemini-3.8-flash","tokens":{"input":100,"output":9,"cached":20,"thoughts":5}}"#,
                "",
            ]
            .join("\n"),
        )
        .unwrap();
        // Not a chat file: ignored.
        std::fs::write(root.join("harbor/logs.json"), r#"{"tokens":{}}"#).unwrap();
        let roots = Roots {
            gemini: vec![root.clone()],
            ..Default::default()
        };
        let entries = scan(&roots, 0).entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(
            (entries[0].input, entries[0].cached, entries[0].output),
            (80, 20, 14)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cline_replies_with_their_folder() {
        let root = folder("cline");
        let dir = root.join("1790000000000_abcde");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("1790000000000_abcde.json"),
            r#"{"session_id":"1790000000000_abcde","cwd":"/work/app"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("1790000000000_abcde.messages.json"),
            r#"{"sessionId":"1790000000000_abcde","messages":[
                {"id":"u1","role":"user","content":"hi","ts":1790000000100},
                {"id":"a1","role":"assistant","ts":1790000000200,"modelInfo":{"id":"anthropic/claude-sonnet-4.6"},
                 "metrics":{"inputTokens":100,"outputTokens":7,"cacheReadTokens":30,"cacheWriteTokens":5,"cost":0.01}}]}"#,
        )
        .unwrap();
        // The session's own file is not a messages file.
        let roots = Roots {
            cline: vec![root.clone()],
            ..Default::default()
        };
        let found = scan(&roots, 0);
        assert_eq!(found.sources, [("cline", root.clone())]);
        let entry = &found.entries[0];
        assert_eq!(found.entries.len(), 1);
        assert_eq!(
            (entry.agent, &*entry.cwd, &*entry.model),
            ("cline", "/work/app", "anthropic/claude-sonnet-4.6")
        );
        assert_eq!(
            (entry.input, entry.cache_write, entry.cached, entry.output),
            (105, 5, 30, 7)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_grown_file_is_read_from_where_it_stopped_and_kept() {
        let root = folder("grow");
        let project = root.join("p");
        std::fs::create_dir_all(&project).unwrap();
        let line = |id: &str| {
            format!(
                r#"{{"type":"assistant","sessionId":"s","requestId":"{id}","timestamp":"2026-11-02T10:00:00Z","message":{{"id":"{id}","model":"m","usage":{{"input_tokens":1,"output_tokens":1}}}}}}"#
            )
        };
        let file = project.join("s.jsonl");
        // The second line is still being written: not read yet.
        std::fs::write(&file, format!("{}\n{}", line("a"), &line("b")[..20])).unwrap();
        let first = read_file(&file, Kind::Claude, None, 1, 1);
        assert_eq!(first.rows.len(), 1);
        assert_eq!(first.read_to, line("a").len() as u64 + 1);
        std::fs::write(&file, format!("{}\n{}\n", line("a"), line("b"))).unwrap();
        let grown = read_file(&file, Kind::Claude, Some(first.clone()), 2, 2);
        assert_eq!(grown.rows.len(), 2, "a, kept, and b, read now");
        assert_eq!(grown.names, first.names, "strings kept once");

        // Saved between runs and read back.
        let saved = root.join("usage-files.json");
        let mut files = HashMap::new();
        files.insert(file.clone(), grown);
        save(&saved, &files);
        let back: Saved = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
        assert_eq!(
            (back.version, back.files[&file].rows.len()),
            (SAVED_VERSION, 2)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn keys_tell_replies_apart() {
        assert_ne!(key(&["claude", "m1", "r1"]), key(&["claude", "m1", "r2"]));
        assert_ne!(key(&["ab", "c"]), key(&["a", "bc"]));
        assert_ne!(key(&[]), 0);
    }
}
