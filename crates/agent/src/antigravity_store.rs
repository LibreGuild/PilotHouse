//! Antigravity's own record of a conversation, read for what was said.
//!
//! The other agents keep a session as lines of JSON. Antigravity keeps one
//! SQLite database a conversation
//! (`<GEMINI_CLI_HOME or ~>/.gemini/antigravity-cli/conversations/<id>.db`),
//! whose `steps` table holds each step as a protobuf message. Its layout is
//! Antigravity's own and written down nowhere; what is read here is the
//! little that was found to hold steady: a step's kind (`step_type`), and
//! the text of what the user typed and of what the agent replied.
//!
//! | `step_type` | What | Where its text is |
//! |---|---|---|
//! | 14 | the user's message | field 19, in it field 2 |
//! | 15 | the agent's reply | field 20, in it field 1 (empty for a turn that only called tools) |
//!
//! Tool steps (132), notices (101) and the agent's thoughts are passed
//! over: their layout differs by tool, and a made-up line would be worse
//! than none. Should the layout change, nothing is read, and the chat says
//! what was said in the CLI cannot be shown; nothing is guessed.
//!
//! Only read, with the database opened read-only.

use std::path::{Path, PathBuf};

use crate::cli_transcript::Step;

const USER_INPUT: i64 = 14;
const REPLY: i64 = 15;

/// Where Antigravity keeps conversation `id`, when started with
/// `environment` (an account's) over the user's own.
fn database(id: &str, environment: &[(String, String)]) -> Option<PathBuf> {
    // An id names a file here: nothing of a path.
    let plain = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !plain {
        return None;
    }
    let home = environment
        .iter()
        .find(|(name, _)| name == "GEMINI_CLI_HOME")
        .map(|(_, value)| value.clone())
        .or_else(|| ph_env::shell_env().get("GEMINI_CLI_HOME").cloned())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)?;
    Some(
        home.join(".gemini/antigravity-cli/conversations")
            .join(format!("{id}.db")),
    )
}

/// What was said in conversation `id`, oldest first; `None` when its
/// record is not there or is not laid out as this knows it.
pub(crate) fn steps(id: &str, environment: &[(String, String)]) -> Option<Vec<Step>> {
    steps_in(&database(id, environment)?)
}

fn steps_in(database: &Path) -> Option<Vec<Step>> {
    use rusqlite::{Connection, OpenFlags};
    if !database.is_file() {
        return None;
    }
    let connection =
        Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let mut statement = connection
        .prepare("SELECT step_type, step_payload FROM steps ORDER BY idx")
        .ok()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?.unwrap_or_default(),
            ))
        })
        .ok()?;
    let mut steps = Vec::new();
    for (kind, payload) in rows.flatten() {
        let text = |outer: u64, inner: u64| {
            field(&payload, outer)
                .and_then(|message| field(message, inner))
                .and_then(|text| std::str::from_utf8(text).ok())
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        match kind {
            USER_INPUT => steps.extend(text(19, 2).map(Step::User)),
            REPLY => steps.extend(text(20, 1).map(Step::Agent)),
            _ => {}
        }
    }
    Some(steps)
}

/// The first length-delimited field numbered `number` of protobuf message
/// `bytes`; `None` when there is none, or `bytes` is not a message.
fn field(bytes: &[u8], number: u64) -> Option<&[u8]> {
    let mut at = 0;
    while at < bytes.len() {
        let key = varint(bytes, &mut at)?;
        match key & 7 {
            0 => {
                varint(bytes, &mut at)?;
            }
            1 => at = at.checked_add(8)?,
            5 => at = at.checked_add(4)?,
            2 => {
                let length = usize::try_from(varint(bytes, &mut at)?).ok()?;
                let end = at.checked_add(length)?;
                let value = bytes.get(at..end)?;
                if key >> 3 == number {
                    return Some(value);
                }
                at = end;
            }
            // Groups, or not a message at all.
            _ => return None,
        }
    }
    None
}

/// The variable-length number at `at`, which moves past it.
fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A length-delimited protobuf field.
    fn bytes_field(number: u64, value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut push = |mut n: u64| loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        };
        push(number << 3 | 2);
        push(value.len() as u64);
        out.extend_from_slice(value);
        out
    }

    /// A step as Antigravity writes one: its kind and some other fields
    /// first (numbers, a nested message), then its own part.
    fn step(kind: u8, part: u64, text_field: u64, text: &str) -> Vec<u8> {
        let mut payload = vec![0x08, kind, 0x20, 0x03];
        payload.extend(bytes_field(5, &bytes_field(12, b"an-id")));
        let mut own = bytes_field(text_field, text.as_bytes());
        own.extend(bytes_field(12, &bytes_field(1, &[0x08, 0xa7, 0x0a])));
        payload.extend(bytes_field(part, &own));
        payload
    }

    #[test]
    fn what_was_said_is_read_from_a_conversations_database() {
        let dir = std::env::temp_dir().join(format!("ph-agy-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE steps (idx integer primary key, step_type integer NOT NULL DEFAULT 0, \
                 status integer NOT NULL DEFAULT 0, step_payload blob);",
            )
            .unwrap();
        let rows: Vec<(i64, i64, Vec<u8>)> = vec![
            (0, 14, step(14, 19, 2, "Start the arrivals board")),
            // A turn that only called tools has no reply text.
            (1, 15, step(15, 20, 3, "thinking it through")),
            (2, 132, step(132, 31, 2, "ls -la")),
            (3, 101, step(101, 114, 1, "[Notice] subagents done")),
            (4, 15, step(15, 20, 1, "The board is up.\n")),
            (5, 14, step(14, 19, 2, "ship it — ✓")),
            // Not a message at all: passed over, not a failure.
            (6, 15, vec![0xff, 0xff, 0xff]),
            (7, 14, Vec::new()),
        ];
        for (idx, kind, payload) in rows {
            connection
                .execute(
                    "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, ?2, ?3)",
                    rusqlite::params![idx, kind, payload],
                )
                .unwrap();
        }
        drop(connection);
        assert_eq!(
            steps_in(&path),
            Some(vec![
                Step::User("Start the arrivals board".into()),
                Step::Agent("The board is up.".into()),
                Step::User("ship it — ✓".into()),
            ])
        );
        // No record, or one laid out another way: nothing is read.
        assert_eq!(steps_in(&dir.join("none.db")), None);
        let other = dir.join("other.db");
        rusqlite::Connection::open(&other)
            .unwrap()
            .execute_batch("CREATE TABLE turns (id integer);")
            .unwrap();
        assert_eq!(steps_in(&other), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A conversation that went on in Antigravity's CLI comes back to its
    /// chat: what was said there is read from its database, in the account
    /// it runs in, and only what came after the chat left.
    #[test]
    fn what_was_said_in_antigravitys_cli_is_taken_into_the_chat() {
        use crate::cli_transcript::Record;
        let home = std::env::temp_dir().join(format!("ph-agy-record-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let conversations = home.join(".gemini/antigravity-cli/conversations");
        std::fs::create_dir_all(&conversations).unwrap();
        let id = "9f4355b6-762e-4ce4-bb43-c47f6aed6895";
        let connection =
            rusqlite::Connection::open(conversations.join(format!("{id}.db"))).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE steps (idx integer primary key, step_type integer NOT NULL DEFAULT 0, \
                 step_payload blob);",
            )
            .unwrap();
        let add = |idx: i64, kind: i64, payload: Vec<u8>| {
            connection
                .execute(
                    "INSERT INTO steps (idx, step_type, step_payload) VALUES (?1, ?2, ?3)",
                    rusqlite::params![idx, kind, payload],
                )
                .unwrap();
        };
        // Said in the chat, before it went to the CLI.
        add(0, 14, step(14, 19, 2, "hi"));
        add(1, 15, step(15, 20, 1, "Hello! How can I help?"));
        let account = vec![(
            "GEMINI_CLI_HOME".to_string(),
            home.to_string_lossy().into_owned(),
        )];
        let record = Record {
            agent: "antigravity",
            id,
            cwd: &home,
            environment: &account,
            file: None,
        };
        let mark = record.step_count().unwrap();
        assert_eq!(mark, 2);
        // Said in the CLI.
        add(2, 14, step(14, 19, 2, "why?"));
        add(3, 132, step(132, 31, 2, "a tool"));
        add(4, 15, step(15, 20, 1, "Could you clarify?"));

        let store = ph_store::Store::open_in_memory().unwrap();
        let place = store.add_project(&home).unwrap();
        let chat = store.create_session(place.id, "hi", "antigravity").unwrap();
        assert_eq!(record.take_in(&store, chat.id, mark).unwrap(), Some(2));
        let said: Vec<(String, serde_json::Value)> = store
            .messages(chat.id)
            .unwrap()
            .into_iter()
            .map(|message| (message.role, serde_json::from_str(&message.body).unwrap()))
            .collect();
        assert_eq!(said.len(), 2);
        assert_eq!(
            (said[0].0.as_str(), &said[0].1["text"]),
            ("user", &"why?".into())
        );
        assert_eq!(
            (said[1].0.as_str(), &said[1].1["text"]),
            ("agent", &"Could you clarify?".into())
        );
        // A conversation whose record is not there: nothing is known, and
        // nothing is added (the chat then says it cannot show it).
        let gone = Record {
            id: "0e1140d8-c545-4d94-af9c-35e256227817",
            ..record
        };
        assert_eq!(gone.step_count(), None);
        assert_eq!(gone.take_in(&store, chat.id, 0).unwrap(), None);
        assert_eq!(store.messages(chat.id).unwrap().len(), 2);
        drop(connection);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The layout is Antigravity's own and may change with a version of
    /// it. To check this reader against a real conversation:
    /// `PH_AGY_CONVERSATION=<a conversations/*.db> cargo test -p ph-agent
    /// a_real_conversation -- --ignored --nocapture` prints how many
    /// messages and replies it reads, and how each starts.
    #[test]
    #[ignore = "reads the conversation database named in PH_AGY_CONVERSATION"]
    fn a_real_conversation_is_read() {
        let path = std::env::var("PH_AGY_CONVERSATION").expect("PH_AGY_CONVERSATION");
        let steps = steps_in(Path::new(&path)).expect("a conversation database");
        for step in &steps {
            let (who, text) = match step {
                Step::User(text) => ("user ", text),
                Step::Agent(text) => ("agent", text),
                Step::Tool(text) => ("tool ", text),
            };
            let start: String = text.chars().take(16).collect();
            println!("{who} {} chars: {start:?}…", text.chars().count());
        }
        assert!(!steps.is_empty(), "nothing was read");
    }

    #[test]
    fn a_conversation_is_looked_for_where_its_account_keeps_it() {
        let account = vec![("GEMINI_CLI_HOME".to_string(), "/accounts/work".to_string())];
        assert_eq!(
            database("9f4355b6-762e-4ce4", &account),
            Some(PathBuf::from(
                "/accounts/work/.gemini/antigravity-cli/conversations/9f4355b6-762e-4ce4.db"
            ))
        );
        // An id is a file's name, nothing more.
        assert_eq!(database("../../etc/passwd", &account), None);
        assert_eq!(database("", &account), None);
    }

    #[test]
    fn a_field_is_found_among_others_or_not_at_all() {
        let message = [
            vec![0x08, 0x96, 0x01],
            bytes_field(3, b"three"),
            vec![0x3d, 1, 2, 3, 4],
            bytes_field(19, b"nineteen"),
        ]
        .concat();
        assert_eq!(field(&message, 19), Some(&b"nineteen"[..]));
        assert_eq!(field(&message, 3), Some(&b"three"[..]));
        assert_eq!(field(&message, 4), None);
        // A length that runs past the end is no field.
        assert_eq!(field(&[0x9a, 0x01, 0x7f, b'x'], 19), None);
        assert_eq!(field(&[], 1), None);
    }
}
