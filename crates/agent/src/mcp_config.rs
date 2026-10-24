//! MCP servers PilotHouse hands to every agent session, besides its own
//! (board, browser, devices): set up once here instead of in each agent's
//! settings. The servers agents already have in their own settings are
//! found too, to see them and copy them over.
//!
//! Only local (stdio) servers: PilotHouse starts nothing on the network.

use std::path::{Path, PathBuf};

use agent_client_protocol_schema::v1::{EnvVariable, McpServer, McpServerStdio};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use ph_store::Store;

/// Where the list is kept.
const SETTING: &str = "mcp.servers";
/// PilotHouse's own server; no other may take the name.
const RESERVED: &str = "pilothouse";

/// One server PilotHouse starts for agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

impl McpServerConfig {
    /// As ACP passes it to an agent.
    pub fn to_acp(&self) -> McpServer {
        McpServer::Stdio(
            McpServerStdio::new(self.name.clone(), PathBuf::from(&self.command))
                .args(self.args.clone())
                .env(
                    self.env
                        .iter()
                        .map(|(key, value)| EnvVariable::new(key.clone(), value.clone()))
                        .collect(),
                ),
        )
    }

    /// `npx -y @scope/server --flag`, for display.
    pub fn command_line(&self) -> String {
        std::iter::once(self.command.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(|part| {
                if part.contains(' ') {
                    format!("\"{part}\"")
                } else {
                    part.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn load(store: &Store) -> Vec<McpServerConfig> {
    store.setting(SETTING)
}

pub fn save(store: &Store, servers: &[McpServerConfig]) -> Result<()> {
    store.set_setting(SETTING, &servers.to_vec())
}

/// The enabled servers, for a new agent session.
pub fn for_session(store: &Store) -> Vec<McpServer> {
    load(store)
        .iter()
        .filter(|server| server.enabled)
        .map(McpServerConfig::to_acp)
        .collect()
}

/// Checks a new server against the list.
pub fn validate(server: &McpServerConfig, existing: &[McpServerConfig]) -> Result<()> {
    let name = server.name.trim();
    if name.is_empty() || server.command.trim().is_empty() {
        bail!("A server needs a name and a command.");
    }
    if name.eq_ignore_ascii_case(RESERVED) {
        bail!("“{RESERVED}” is PilotHouse's own server.");
    }
    if existing
        .iter()
        .any(|other| other.name.eq_ignore_ascii_case(name))
    {
        bail!("There is already a server named “{name}”.");
    }
    if server
        .env
        .iter()
        .any(|(key, _)| key.is_empty() || key.contains(['=', '\0']))
    {
        bail!("Environment lines look like KEY=value.");
    }
    Ok(())
}

/// Splits a command line into words: spaces separate, quotes group, a
/// backslash escapes the next character.
pub fn split_command(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    in_word = true;
                }
            }
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// `KEY=value` lines; blank lines and `#` comments are skipped.
pub fn parse_env(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| match line.split_once('=') {
            Some((key, value)) => (key.trim().to_string(), value.trim().to_string()),
            None => (line.to_string(), String::new()),
        })
        .collect()
}

/// A server found in an agent's own settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundServer {
    /// The agent's name, e.g. `Claude Code`.
    pub agent: &'static str,
    pub source: PathBuf,
    pub server: McpServerConfig,
}

/// The local servers configured in Claude Code, Codex and Gemini CLI, for
/// everyone and for `project`.
pub fn discover(home: &Path, project: &Path) -> Vec<FoundServer> {
    let mut found = Vec::new();
    let mut from_json = |agent, path: PathBuf, pick: &dyn Fn(&Value) -> Option<Value>| {
        let Some(json) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            return;
        };
        if let Some(servers) = pick(&json) {
            found.extend(
                json_servers(&servers)
                    .into_iter()
                    .map(|server| FoundServer {
                        agent,
                        source: path.clone(),
                        server,
                    }),
            );
        }
    };
    let project_key = project.to_string_lossy().to_string();
    from_json("Claude Code", home.join(".claude.json"), &|json| {
        json.get("mcpServers").cloned()
    });
    from_json("Claude Code", home.join(".claude.json"), &|json| {
        json.get("projects")?
            .get(&project_key)?
            .get("mcpServers")
            .cloned()
    });
    from_json("Claude Code", project.join(".mcp.json"), &|json| {
        json.get("mcpServers").cloned()
    });
    from_json("Gemini CLI", home.join(".gemini/settings.json"), &|json| {
        json.get("mcpServers").cloned()
    });
    from_json(
        "Gemini CLI",
        project.join(".gemini/settings.json"),
        &|json| json.get("mcpServers").cloned(),
    );
    let codex = home.join(".codex/config.toml");
    if let Some(table) = std::fs::read_to_string(&codex)
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
    {
        found.extend(toml_servers(&table).into_iter().map(|server| FoundServer {
            agent: "Codex",
            source: codex.clone(),
            server,
        }));
    }
    found
}

/// `{"name": {"command": …, "args": […], "env": {…}}}`; servers with only a
/// URL are left out.
fn json_servers(servers: &Value) -> Vec<McpServerConfig> {
    let Some(servers) = servers.as_object() else {
        return Vec::new();
    };
    servers
        .iter()
        .filter_map(|(name, server)| {
            let command = server.get("command")?.as_str()?.to_string();
            let strings = |value: Option<&Value>| -> Vec<String> {
                value
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let env = server
                .get("env")
                .and_then(Value::as_object)
                .map(|env| {
                    env.iter()
                        .map(|(key, value)| {
                            (key.clone(), value.as_str().unwrap_or_default().to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(McpServerConfig {
                name: name.clone(),
                command,
                args: strings(server.get("args")),
                env,
                enabled: true,
            })
        })
        .collect()
}

/// Codex's `[mcp_servers.name]` tables.
fn toml_servers(config: &toml::Table) -> Vec<McpServerConfig> {
    let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    servers
        .iter()
        .filter_map(|(name, server)| {
            let command = server.get("command")?.as_str()?.to_string();
            let args = server
                .get("args")
                .and_then(toml::Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(|arg| arg.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let env = server
                .get("env")
                .and_then(toml::Value::as_table)
                .map(|env| {
                    env.iter()
                        .map(|(key, value)| {
                            (key.clone(), value.as_str().unwrap_or_default().to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(McpServerConfig {
                name: name.clone(),
                command,
                args,
                env,
                enabled: true,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-memory".into()],
            env: Vec::new(),
            enabled: true,
        }
    }

    #[test]
    fn command_lines_split_like_a_shell() {
        assert_eq!(
            split_command(r#"npx -y "@scope/server name" --root='/my dir' a\ b"#),
            ["npx", "-y", "@scope/server name", "--root=/my dir", "a b"]
        );
        assert_eq!(split_command("  "), Vec::<String>::new());
        assert_eq!(
            split_command(r#"run """#),
            ["run", ""],
            "an empty quoted word is a word"
        );
        assert_eq!(
            parse_env("TOKEN=abc\n# note\n\nURL = http://x=y\nFLAG"),
            [
                ("TOKEN".into(), "abc".into()),
                ("URL".into(), "http://x=y".into()),
                ("FLAG".into(), String::new())
            ]
        );
    }

    #[test]
    fn new_servers_are_checked() {
        let list = vec![server("memory")];
        assert!(validate(&server("files"), &list).is_ok());
        assert!(
            validate(&server("Memory"), &list).is_err(),
            "names are unique, any case"
        );
        assert!(validate(&server("pilothouse"), &list).is_err());
        let mut blank = server("x");
        blank.command = " ".into();
        assert!(validate(&blank, &list).is_err());
        assert_eq!(
            server("m").command_line(),
            "npx -y @modelcontextprotocol/server-memory"
        );
    }

    #[test]
    fn the_list_is_stored_and_only_enabled_servers_go_to_sessions() {
        let store = Store::open_in_memory().unwrap();
        assert!(load(&store).is_empty());
        let mut off = server("off");
        off.enabled = false;
        save(&store, &[server("memory"), off]).unwrap();
        assert_eq!(load(&store).len(), 2);
        assert_eq!(for_session(&store).len(), 1);
    }

    #[test]
    fn finds_servers_in_each_agents_settings() {
        let home = std::env::temp_dir().join(format!("ph-mcp-home-{}", std::process::id()));
        let project = home.join("work/app");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let project_key = project.to_string_lossy().to_string();
        std::fs::write(
            home.join(".claude.json"),
            serde_json::json!({
                "mcpServers": { "memory": { "command": "npx", "args": ["-y", "mem"], "env": { "K": "v" } },
                                "remote": { "type": "http", "url": "https://example.com/mcp" } },
                "projects": { project_key: { "mcpServers": { "db": { "command": "db-mcp" } } } }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            home.join(".codex/config.toml"),
            "[mcp_servers.search]\ncommand = \"search-mcp\"\nargs = [\"--fast\"]\n[mcp_servers.search.env]\nKEY = \"1\"\n",
        )
        .unwrap();
        std::fs::write(
            home.join(".gemini/settings.json"),
            r#"{"mcpServers": {"git": {"command": "uvx", "args": ["mcp-server-git"]}}}"#,
        )
        .unwrap();
        std::fs::write(
            project.join(".mcp.json"),
            r#"{"mcpServers": {"local": {"command": "./tool"}}}"#,
        )
        .unwrap();

        let found = discover(&home, &project);
        let names: Vec<_> = found
            .iter()
            .map(|found| (found.agent, found.server.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("Claude Code", "memory"),
                ("Claude Code", "db"),
                ("Claude Code", "local"),
                ("Gemini CLI", "git"),
                ("Codex", "search")
            ],
            "remote servers are left out"
        );
        assert_eq!(found[0].server.env, [("K".to_string(), "v".to_string())]);
        assert_eq!(found[4].server.args, ["--fast"]);
        assert_eq!(found[4].server.env, [("KEY".to_string(), "1".to_string())]);
    }
}
