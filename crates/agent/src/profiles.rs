//! How each agent is run on this computer, as set in Settings → Agents:
//! whether its sessions open as a chat or as its CLI, the command its CLI
//! is started by and what is added to it, and its accounts.
//!
//! An **account** is a set of environment variables an agent is started
//! with. Agents keep their sign-in and their sessions in a folder they
//! read from one (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`), so a second account
//! is a second folder: what a shell alias such as
//! `alias claude-work='CLAUDE_CONFIG_DIR=~/.claude-work claude'` does.
//! Kept as variables, not as that alias: an alias exists only in an
//! interactive shell, so it would hold for the CLI and not for a chat.
//!
//! Stored as one JSON value (`agents.profiles`); what a build does not
//! know in it is kept.

use std::collections::BTreeMap;

use ph_store::Store;
use serde::{Deserialize, Serialize};

use crate::chat_panel::Form;

const KEY: &str = "agents.profiles";

/// Every agent's profile, and what holds for all of them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Profiles {
    /// How a new session opens, for an agent that does not say itself.
    #[serde(default, skip_serializing_if = "is_chat")]
    pub open_as: OpenAs,
    /// By agent id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, Profile>,
    #[serde(flatten)]
    other: BTreeMap<String, serde_json::Value>,
}

fn is_chat(open_as: &OpenAs) -> bool {
    *open_as == OpenAs::Chat
}

/// How a new session opens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAs {
    #[default]
    Chat,
    Cli,
}

impl OpenAs {
    pub fn form(self) -> Form {
        match self {
            OpenAs::Chat => Form::Chat,
            OpenAs::Cli => Form::Cli,
        }
    }
}

/// One agent's profile. All of it is optional: an agent nothing was set
/// for runs as it always did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// How its sessions open, when not as every agent's do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_as: Option<OpenAs>,
    /// The command its CLI is started by, when not the agent's usual one
    /// (a wrapper script, another build of it).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Arguments added when its CLI is started.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    /// Set for the agent, as a chat and as its CLI.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
    /// Its accounts beside its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<Account>,
    #[serde(flatten)]
    other: BTreeMap<String, serde_json::Value>,
}

/// One more account of an agent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Kept with each session in it; never shown.
    pub id: String,
    /// What the user calls it ("Work").
    pub name: String,
    /// What the agent is started with for it, over the agent's own
    /// environment.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

/// The profiles as last read or saved, for code that has no store at hand
/// (what counts usage from every account's files).
static KNOWN: std::sync::RwLock<Option<Profiles>> = std::sync::RwLock::new(None);

impl Profiles {
    pub fn load(store: &Store) -> Self {
        let profiles: Self = store.setting(KEY);
        profiles.note();
        profiles
    }

    pub fn save(&self, store: &Store) -> anyhow::Result<()> {
        store.set_setting(KEY, self)?;
        self.note();
        Ok(())
    }

    /// The profiles as last read or saved in this run, for code that
    /// draws (which must not read the store as it does).
    pub fn known() -> Self {
        KNOWN
            .read()
            .ok()
            .and_then(|known| known.clone())
            .unwrap_or_default()
    }

    fn note(&self) {
        if let Ok(mut known) = KNOWN.write() {
            *known = Some(self.clone());
        }
    }

    /// Where agent `id`'s accounts point `variable` (a folder the agent
    /// keeps its files in, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`), as far as
    /// the profiles were read in this run.
    pub fn account_folders(id: &str, variable: &str) -> Vec<std::path::PathBuf> {
        let Ok(known) = KNOWN.read() else {
            return Vec::new();
        };
        known
            .as_ref()
            .map(|profiles| profiles.of(id))
            .map(|profile| {
                profile
                    .accounts
                    .iter()
                    .filter_map(|account| account.environment.get(variable))
                    .filter(|folder| !folder.is_empty())
                    .map(|folder| std::path::PathBuf::from(home_expanded(folder)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The profile of agent `id` (an empty one when nothing was set).
    pub fn of(&self, id: &str) -> Profile {
        self.agents.get(id).cloned().unwrap_or_default()
    }

    /// Changes agent `id`'s profile; one left empty is not kept.
    pub fn update(&mut self, id: &str, change: impl FnOnce(&mut Profile)) {
        let mut profile = self.of(id);
        change(&mut profile);
        if profile == Profile::default() {
            self.agents.remove(id);
        } else {
            self.agents.insert(id.to_string(), profile);
        }
    }

    /// How a new session with agent `id` opens.
    pub fn opens_as(&self, id: &str) -> Form {
        self.agents
            .get(id)
            .and_then(|profile| profile.open_as)
            .unwrap_or(self.open_as)
            .form()
    }
}

impl Profile {
    /// The account with id `id`; `None` for the agent's own (an empty id),
    /// and for one that was removed since.
    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|account| account.id == id)
    }

    /// What the agent is started with in `account`: the agent's own
    /// environment from its profile, then the account's over it. Values
    /// may start with `~` for the home folder, as they would in a shell.
    pub fn environment_for(&self, account: &str) -> Vec<(String, String)> {
        let mut environment = self.environment.clone();
        if let Some(account) = self.account(account) {
            environment.extend(account.environment.clone());
        }
        environment
            .into_iter()
            .map(|(name, value)| (name, home_expanded(&value)))
            .collect()
    }

    /// Adds an account named `name`, with an id of its own, and returns
    /// the id.
    pub fn add_account(&mut self, name: &str) -> String {
        let base: String = name
            .chars()
            .filter_map(|c| match c {
                c if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
                ' ' | '-' | '_' => Some('-'),
                _ => None,
            })
            .collect();
        let base = if base.trim_matches('-').is_empty() {
            "account".to_string()
        } else {
            base.trim_matches('-').to_string()
        };
        let mut id = base.clone();
        let mut n = 2;
        while self.account(&id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        self.accounts.push(Account {
            id: id.clone(),
            name: name.trim().to_string(),
            environment: BTreeMap::new(),
        });
        id
    }
}

/// `value` with a leading `~` as the home folder (`~`, `~/x`): a variable
/// set from Settings is not read by a shell, which would do this.
pub fn home_expanded(value: &str) -> String {
    let rest = match value.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => rest,
        _ => return value.to_string(),
    };
    match dirs::home_dir() {
        Some(home) => format!("{}{rest}", home.display()),
        None => value.to_string(),
    }
}

/// `text` as environment variables: `NAME=value` a line (or separated by
/// spaces, as typed before a command), a value in quotes when it has
/// spaces. Lines that are not that are left out.
pub fn parse_environment(text: &str) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    for word in words(text) {
        let Some((name, value)) = word.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let named = !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit());
        if named {
            environment.insert(name.to_string(), value.to_string());
        }
    }
    environment
}

/// Environment variables as text to edit: `NAME=value`, one a line.
pub fn environment_text(environment: &BTreeMap<String, String>) -> String {
    environment
        .iter()
        .map(|(name, value)| {
            if value.is_empty() {
                format!("{name}=")
            } else {
                format!("{name}={}", quoted(value))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `word` as text that [`words`] reads back as it is: in quotes when it
/// has spaces or a quote character, in the kind of quote it does not hold.
/// One that holds both kinds is written in pieces, as in a shell
/// (`'it'"'"'s'`): the text then needs no escape character, so a backslash
/// stays a backslash (a Windows path).
pub fn quoted(word: &str) -> String {
    let plain =
        !word.is_empty() && !word.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'');
    if plain {
        word.to_string()
    } else if !word.contains('"') {
        format!("\"{word}\"")
    } else if !word.contains('\'') {
        format!("'{word}'")
    } else {
        word.split('\'')
            .map(|piece| match piece {
                "" => String::new(),
                piece => format!("'{piece}'"),
            })
            .collect::<Vec<_>>()
            .join("\"'\"")
    }
}

/// `text` as the words of a command line: split at spaces and line ends,
/// with quotes (single or double) keeping what is in them together.
pub fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in text.chars() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            None => word.push(c),
        }
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_opens_as_its_agent_says_or_as_all_do() {
        let mut profiles = Profiles::default();
        assert_eq!(profiles.opens_as("claude"), Form::Chat);
        profiles.open_as = OpenAs::Cli;
        assert_eq!(profiles.opens_as("claude"), Form::Cli);
        profiles.update("codex", |codex| codex.open_as = Some(OpenAs::Chat));
        assert_eq!(profiles.opens_as("codex"), Form::Chat);
        assert_eq!(profiles.opens_as("claude"), Form::Cli);
        // Back to "as all agents": nothing is kept for it.
        profiles.update("codex", |codex| codex.open_as = None);
        assert!(profiles.agents.is_empty());
    }

    #[test]
    fn an_account_is_what_the_agent_is_started_with() {
        let mut claude = Profile::default();
        claude
            .environment
            .insert("ANTHROPIC_LOG".into(), "info".into());
        let work = claude.add_account("Work");
        assert_eq!(work, "work");
        claude.accounts[0]
            .environment
            .insert("CLAUDE_CONFIG_DIR".into(), "~/.claude-work".into());
        // The agent's own: only what is set for the agent.
        assert_eq!(
            claude.environment_for(""),
            vec![("ANTHROPIC_LOG".to_string(), "info".to_string())]
        );
        // An account: its variables over the agent's, `~` as the home.
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            claude.environment_for("work"),
            vec![
                ("ANTHROPIC_LOG".to_string(), "info".to_string()),
                (
                    "CLAUDE_CONFIG_DIR".to_string(),
                    format!("{}/.claude-work", home.display())
                ),
            ]
        );
        // One removed since: the agent's own, not an error.
        assert_eq!(claude.environment_for("gone").len(), 1);
        // Ids are each account's own, whatever it is named.
        assert_eq!(claude.add_account("Work"), "work-2");
        assert_eq!(claude.add_account("客户 !"), "account");
        assert_eq!(home_expanded("~user/x"), "~user/x");
        assert_eq!(home_expanded("/a/~"), "/a/~");
    }

    #[test]
    fn profiles_are_stored_and_keep_what_is_not_known() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(Profiles::load(&store), Profiles::default());
        // A newer build's field is kept through a save here.
        store
            .set_setting(
                KEY,
                &serde_json::json!({
                    "open_as": "cli",
                    "later": true,
                    "agents": { "claude": { "command": "claude-beta", "sandbox": "strict" } }
                }),
            )
            .unwrap();
        let mut profiles = Profiles::load(&store);
        assert_eq!(profiles.open_as, OpenAs::Cli);
        assert_eq!(profiles.of("claude").command, "claude-beta");
        profiles.update("claude", |claude| {
            claude.arguments = vec!["--verbose".into()]
        });
        profiles.save(&store).unwrap();
        let raw: serde_json::Value = store.setting(KEY);
        assert_eq!(raw["later"], true);
        assert_eq!(raw["agents"]["claude"]["sandbox"], "strict");
        assert_eq!(raw["agents"]["claude"]["arguments"][0], "--verbose");
    }

    #[test]
    fn environment_and_arguments_are_read_as_typed() {
        let environment = parse_environment(
            "CLAUDE_CONFIG_DIR=~/.claude-work\nNAME=\"two words\" A=1\nnot a variable\n9X=1\n=x",
        );
        assert_eq!(
            environment_text(&environment),
            "A=1\nCLAUDE_CONFIG_DIR=~/.claude-work\nNAME=\"two words\""
        );
        assert_eq!(
            words("--model opus  --append-system-prompt 'be brief' \"\""),
            vec!["--model", "opus", "--append-system-prompt", "be brief", ""]
        );
        assert!(words("  \n ").is_empty());
    }

    #[test]
    fn values_with_quotes_come_back_as_they_were() {
        let mut environment = BTreeMap::new();
        for (name, value) in [
            ("NAME", "O'Brien"),
            ("EXTRA", r#"{"a": 1}"#),
            ("BOTH", r#"it's "quoted""#),
            ("QUOTES", r#"'""'"#),
            ("WINDOWS", r"C:\Program Files\tool\"),
            ("EMPTY", ""),
            ("OTHER", "1"),
        ] {
            environment.insert(name.to_string(), value.to_string());
        }
        let text = environment_text(&environment);
        // Every variable is still there, each on a line of its own.
        assert_eq!(text.lines().count(), environment.len());
        assert_eq!(parse_environment(&text), environment);
        assert!(text.contains("NAME=\"O'Brien\""), "{text}");
        assert!(text.contains(r#"EXTRA='{"a": 1}'"#), "{text}");
        // Arguments the same way, an empty one included.
        let arguments = ["say \"hi\" now", "", "don't", r#"a'b"c d"#, "--plain"];
        let line = arguments.map(quoted).join(" ");
        assert_eq!(words(&line), arguments);
    }
}
