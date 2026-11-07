//! Cline's files on this computer, and what PilotHouse writes for it.
//!
//! Cline keeps everything under `~/.cline` (`CLINE_DIR`), its state in
//! `data/` (`CLINE_DATA_DIR`): sign-ins in `settings/providers.json`, MCP
//! servers in `settings/cline_mcp_settings.json`, and each session in
//! `sessions/<id>/` (`<id>.json`, and `<id>.messages.json` with each reply's
//! tokens). PilotHouse only reads them.
//!
//! Over ACP, Cline ignores the MCP servers a session is given and reads its
//! own settings file instead, so its process is pointed
//! (`CLINE_MCP_SETTINGS_PATH`) at a copy PilotHouse writes: the user's own
//! servers, with PilotHouse's and those from Settings → MCP & skills.

use std::path::{Path, PathBuf};

use agent_client_protocol_schema::v1::McpServer;
use anyhow::{Context as _, Result};
use serde_json::{Map, Value, json};

/// Where Cline's files are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub providers: PathBuf,
    pub mcp_settings: PathBuf,
    pub sessions: PathBuf,
}

impl Paths {
    /// As Cline finds them, from the login shell's environment.
    pub fn of_this_computer() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Self::under(&home, &|name| {
            ph_env::shell_env()
                .get(name)
                .cloned()
                .or_else(|| std::env::var(name).ok())
        })
    }

    /// [`Self::of_this_computer`] without waiting for the login shell's
    /// environment (the app's own until it is read): for what the UI
    /// shows, which must not wait seconds for a shell to start.
    fn of_this_computer_now() -> Self {
        Self::under(&dirs::home_dir().unwrap_or_default(), &var_now)
    }

    /// With `home` the user's home folder and `var` reading the environment.
    pub fn under(home: &Path, var: &dyn Fn(&str) -> Option<String>) -> Self {
        let var = |name: &str| {
            var(name)
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
        };
        let root = var("CLINE_DIR").unwrap_or_else(|| home.join(".cline"));
        let data = var("CLINE_DATA_DIR").unwrap_or_else(|| root.join("data"));
        Self {
            providers: var("CLINE_PROVIDER_SETTINGS_PATH")
                .unwrap_or_else(|| data.join("settings/providers.json")),
            mcp_settings: var("CLINE_MCP_SETTINGS_PATH")
                .unwrap_or_else(|| data.join("settings/cline_mcp_settings.json")),
            sessions: var("CLINE_SESSION_DATA_DIR").unwrap_or_else(|| data.join("sessions")),
        }
    }
}

/// The providers Cline signs in to for editors (ACP). Those `cline auth`
/// sets up with an API key or a cloud's own sign-in it uses in a terminal
/// only, unless told which to use (`CLINE_PROVIDER`, `CLINE_API_KEY`).
const ACCOUNTS: [&str; 3] = ["cline", "cline-pass", "openai-codex"];

/// Cline's `providers.json` (only whether secrets are there is looked at).
fn providers(paths: &Paths) -> Value {
    std::fs::read_to_string(&paths.providers)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn has_secret(settings: &Value) -> bool {
    let set = |value: &Value| value.as_str().is_some_and(|text| !text.is_empty());
    set(&settings["auth"]["accessToken"])
        || set(&settings["auth"]["apiKey"])
        || set(&settings["apiKey"])
}

/// A provider `cline auth` saved, other than the [`ACCOUNTS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalProvider {
    pub id: String,
    pub model: Option<String>,
    /// It signs in with a key `cline auth` saved, which PilotHouse does not
    /// read; one that needs none (Vertex AI on Google Cloud's own sign-in,
    /// a local Ollama) Cline can run on in PilotHouse too.
    pub needs_key: bool,
}

fn saved(providers: &Value, id: &str) -> Option<TerminalProvider> {
    if ACCOUNTS.contains(&id) {
        return None;
    }
    let settings = Value::Object(providers["providers"][id]["settings"].as_object()?.clone());
    Some(TerminalProvider {
        id: id.to_string(),
        model: settings["model"]
            .as_str()
            .filter(|model| !model.is_empty())
            .map(str::to_string),
        needs_key: has_secret(&settings),
    })
}

/// The provider Cline last used in a terminal, when it is not one of the
/// [`ACCOUNTS`] editors sign in with.
pub fn terminal_provider(paths: &Paths) -> Option<TerminalProvider> {
    last_used(&providers(paths))
}

fn last_used(providers: &Value) -> Option<TerminalProvider> {
    saved(providers, providers["lastUsedProvider"].as_str()?)
}

/// The saved providers Cline can run on in PilotHouse besides the
/// [`ACCOUNTS`]: those that need no key, the one last used first.
pub fn keyless_providers(paths: &Paths) -> Vec<TerminalProvider> {
    keyless(&providers(paths))
}

fn keyless(providers: &Value) -> Vec<TerminalProvider> {
    let last = providers["lastUsedProvider"].as_str().unwrap_or_default();
    let mut found: Vec<TerminalProvider> = providers["providers"]
        .as_object()
        .into_iter()
        .flat_map(|all| all.keys())
        .filter_map(|id| saved(providers, id))
        .filter(|provider| !provider.needs_key)
        .collect();
    found.sort_by_key(|provider| (provider.id != last, provider.id.clone()));
    found
}

/// A provider's name as people know it.
pub fn provider_name(id: &str) -> String {
    match id {
        "cline" => "Cline".into(),
        "cline-pass" => "ClinePass".into(),
        "openai-codex" => "ChatGPT subscription".into(),
        "vertex" => "Vertex AI".into(),
        "ollama" => "Ollama".into(),
        "lmstudio" => "LM Studio".into(),
        "bedrock" => "Amazon Bedrock".into(),
        other => other.into(),
    }
}

/// What Cline's process gets in PilotHouse: the provider to run on, when
/// it is a saved one that needs no key (the one `chosen` in PilotHouse,
/// else the one Cline last used in a terminal), with its model and a
/// stand-in for the key Cline asks for (the provider's own settings sign it
/// in). Nothing for one of the [`ACCOUNTS`], which Cline picks over ACP
/// itself, or when the user's environment already says which provider or
/// key to use.
pub fn launch_env(
    paths: &Paths,
    var: &dyn Fn(&str) -> Option<String>,
    chosen: Option<&str>,
) -> Vec<(String, String)> {
    launch_env_with(&providers(paths), var, chosen)
}

/// [`launch_env`] with Cline's `providers.json` read already.
fn launch_env_with(
    providers: &Value,
    var: &dyn Fn(&str) -> Option<String>,
    chosen: Option<&str>,
) -> Vec<(String, String)> {
    let set = |name: &str| var(name).is_some_and(|value| !value.trim().is_empty());
    if set("CLINE_PROVIDER") || set("CLINE_API_KEY") {
        return Vec::new();
    }
    let keyless = |provider: &TerminalProvider| !provider.needs_key;
    let provider = match chosen {
        Some(id) if ACCOUNTS.contains(&id) => None,
        Some(id) => saved(providers, id)
            .filter(keyless)
            .or_else(|| last_used(providers).filter(keyless)),
        None => last_used(providers).filter(keyless),
    };
    let Some(provider) = provider else {
        return Vec::new();
    };
    let mut env = vec![
        ("CLINE_PROVIDER".to_string(), provider.id),
        ("CLINE_API_KEY".to_string(), "unused".to_string()),
    ];
    if let Some(model) = provider.model
        && !set("CLINE_MODEL")
    {
        env.push(("CLINE_MODEL".to_string(), model));
    }
    env
}

/// The provider chosen in PilotHouse (Cline's Provider option), if one was.
fn chosen(store: &ph_store::Store) -> Option<String> {
    crate::options::preferences(store, "cline").remove("provider")
}

fn shell_var(name: &str) -> Option<String> {
    ph_env::shell_env().get(name).cloned()
}

/// A variable of the login shell's environment once it was read, else of
/// the app's own.
fn var_now(name: &str) -> Option<String> {
    ph_env::shell_env_if_ready()
        .and_then(|env| env.get(name).cloned())
        .or_else(|| std::env::var(name).ok())
}

/// [`launch_env`] on this computer, with the provider chosen in PilotHouse.
pub fn env_for(store: &ph_store::Store) -> Vec<(String, String)> {
    launch_env(
        &Paths::of_this_computer(),
        &shell_var,
        chosen(store).as_deref(),
    )
}

/// The provider PilotHouse runs Cline on ([`env_for`]), which Cline then
/// will not change; `None` when Cline picks among its accounts.
pub fn pinned(store: &ph_store::Store) -> Option<String> {
    pinned_with(
        &providers(&Paths::of_this_computer_now()),
        chosen(store).as_deref(),
    )
}

/// [`pinned`] from `providers` read already, without waiting for the
/// login shell (its choices are on screen).
fn pinned_with(providers: &Value, chosen: Option<&str>) -> Option<String> {
    launch_env_with(providers, &var_now, chosen)
        .into_iter()
        .find(|(name, _)| name == "CLINE_PROVIDER")
        .map(|(_, value)| value)
}

/// Cline's Provider option as PilotHouse offers it: the accounts Cline
/// offers, and the saved providers that need no key (which Cline does not
/// list over ACP), the current one the provider it runs on.
pub fn adjust_options(options: &mut Vec<crate::options::AgentOption>, store: &ph_store::Store) {
    use crate::options::{AgentOption, OptionCategory, OptionChoice, OptionKind, OptionSource};
    // Read once: this runs as options change, on the UI thread.
    let providers = providers(&Paths::of_this_computer_now());
    let chosen = chosen(store);
    // Before Cline says (options kept from before it starts), its accounts.
    if !options.iter().any(|option| option.id == "provider") {
        let current = chosen.clone().unwrap_or_else(|| "cline".into());
        options.push(AgentOption {
            id: "provider".into(),
            name: "Provider".into(),
            category: OptionCategory::Other,
            current,
            kind: OptionKind::Select(
                ["cline", "openai-codex"]
                    .iter()
                    .map(|id| OptionChoice {
                        value: id.to_string(),
                        name: provider_name(id),
                        description: None,
                    })
                    .collect(),
            ),
            source: OptionSource::ConfigOption,
        });
    }
    let Some(option) = options.iter_mut().find(|option| option.id == "provider") else {
        return;
    };
    let OptionKind::Select(choices) = &mut option.kind else {
        return;
    };
    for provider in keyless(&providers) {
        if !choices.iter().any(|choice| choice.value == provider.id) {
            choices.push(OptionChoice {
                name: provider_name(&provider.id),
                value: provider.id,
                description: Some("Set up with `cline auth`, as in your terminal".into()),
            });
        }
    }
    if let Some(pinned) = pinned_with(&providers, chosen.as_deref()) {
        option.current = pinned;
    }
}

/// Whether choosing `value` for Cline's Provider (now `before`, from
/// [`pinned`]) needs Cline started again: when what it runs on is set at
/// its start rather than changed over ACP.
pub fn needs_restart(before: Option<&str>, after: Option<&str>) -> bool {
    before != after
}

/// How Cline is signed in in PilotHouse, if it is: with `CLINE_API_KEY`,
/// the provider it uses in a terminal (see [`launch_env`]), or a Cline
/// account, ClinePass or ChatGPT subscription saved by `cline auth`.
/// Nothing secret is read out: only whether a sign-in is there.
pub fn signed_in(paths: &Paths, var: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if var("CLINE_API_KEY").is_some_and(|key| !key.trim().is_empty()) {
        return Some("With CLINE_API_KEY.".into());
    }
    if let Some(provider) = terminal_provider(paths).filter(|provider| !provider.needs_key) {
        return Some(format!(
            "With {}, as in your terminal (`cline auth`).",
            provider.id
        ));
    }
    let providers = providers(paths);
    let has = |id: &str| has_secret(&providers["providers"][id]["settings"]);
    if has("cline") {
        Some("With your Cline account.".into())
    } else if has("cline-pass") {
        Some("With ClinePass.".into())
    } else if has("openai-codex") {
        Some("With your ChatGPT subscription.".into())
    } else {
        None
    }
}

/// Writes the MCP settings Cline reads in PilotHouse, named by `key` (one
/// file per workspace): the user's own servers from `paths`, and `servers`
/// over them by name. Returns its path. The user's servers may carry keys
/// in their environment, so the folder is private.
pub fn write_mcp_settings(paths: &Paths, servers: &[McpServer], key: &str) -> Result<PathBuf> {
    let folder = ph_store::paths::data_dir().join("agents").join("cline");
    ph_store::paths::ensure_private_dir(&folder).context("making a folder for Cline's settings")?;
    let path = folder.join(format!("mcp-{:016x}.json", fnv(key)));
    let text = serde_json::to_string_pretty(&mcp_settings(paths, servers))?;
    // Written whole and then moved in, so Cline never reads half of it.
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, text).context("writing Cline's MCP settings")?;
    std::fs::rename(&partial, &path).context("writing Cline's MCP settings")?;
    Ok(path)
}

/// The settings [`write_mcp_settings`] writes.
fn mcp_settings(paths: &Paths, servers: &[McpServer]) -> Value {
    let mut all: Map<String, Value> = std::fs::read_to_string(&paths.mcp_settings)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| {
            settings
                .get("mcpServers")
                .and_then(Value::as_object)
                .cloned()
        })
        .unwrap_or_default();
    for server in servers {
        if let Some((name, entry)) = stdio_entry(server) {
            all.insert(name, entry);
        }
    }
    json!({ "mcpServers": all })
}

/// An ACP stdio server as Cline writes one: `{"transport": {"type":
/// "stdio", "command", "args", "env"}}`. Only stdio servers are handed over.
fn stdio_entry(server: &McpServer) -> Option<(String, Value)> {
    let McpServer::Stdio(stdio) = server else {
        return None;
    };
    let env: Map<String, Value> = stdio
        .env
        .iter()
        .map(|var| (var.name.clone(), Value::String(var.value.clone())))
        .collect();
    Some((
        stdio.name.clone(),
        json!({
            "transport": {
                "type": "stdio",
                "command": stdio.command.to_string_lossy(),
                "args": stdio.args,
                "env": env,
            }
        }),
    ))
}

/// A stable 64-bit hash (FNV-1a), for file names.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol_schema::v1::{EnvVariable, McpServerStdio};

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ph-cline-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn paths_follow_clines_variables() {
        let home = Path::new("/home/me");
        let none = |_: &str| None;
        let paths = Paths::under(home, &none);
        assert_eq!(
            paths.providers,
            home.join(".cline/data/settings/providers.json")
        );
        assert_eq!(paths.sessions, home.join(".cline/data/sessions"));
        let set = |name: &str| match name {
            "CLINE_DIR" => Some("/elsewhere".to_string()),
            "CLINE_MCP_SETTINGS_PATH" => Some("/mcp.json".to_string()),
            _ => None,
        };
        let paths = Paths::under(home, &set);
        assert_eq!(paths.sessions, Path::new("/elsewhere/data/sessions"));
        assert_eq!(paths.mcp_settings, Path::new("/mcp.json"));
    }

    #[test]
    fn signed_in_as_editors_or_the_terminal_use_it() {
        let home = folder("auth");
        let paths = Paths::under(&home, &|_| None);
        let none = |_: &str| None;
        assert_eq!(signed_in(&paths, &none), None);
        std::fs::create_dir_all(paths.providers.parent().unwrap()).unwrap();
        // An API key for another provider: a terminal sign-in PilotHouse
        // cannot use without reading the key.
        std::fs::write(
            &paths.providers,
            r#"{"lastUsedProvider":"anthropic","providers":{"anthropic":{"settings":{"apiKey":"k","model":"m"}}}}"#,
        )
        .unwrap();
        assert_eq!(signed_in(&paths, &none), None);
        assert!(terminal_provider(&paths).unwrap().needs_key);
        assert!(launch_env(&paths, &none, None).is_empty());
        let key = |name: &str| (name == "CLINE_API_KEY").then(|| "k".to_string());
        assert_eq!(
            signed_in(&paths, &key).as_deref(),
            Some("With CLINE_API_KEY.")
        );
        // Vertex AI on Google Cloud's own sign-in: Cline runs on it here too,
        // even with a Cline account saved.
        std::fs::write(
            &paths.providers,
            r#"{"lastUsedProvider":"vertex","providers":{
                "vertex":{"settings":{"model":"gemini-3.1-flash-lite","gcp":{"projectId":"p","region":"r"}}},
                "cline":{"settings":{"auth":{"accessToken":"t"}}}}}"#,
        )
        .unwrap();
        assert!(signed_in(&paths, &none).unwrap().starts_with("With vertex"));
        assert_eq!(
            launch_env(&paths, &none, None),
            [
                ("CLINE_PROVIDER".to_string(), "vertex".to_string()),
                ("CLINE_API_KEY".to_string(), "unused".to_string()),
                (
                    "CLINE_MODEL".to_string(),
                    "gemini-3.1-flash-lite".to_string()
                ),
            ]
        );
        // The user's own choice of provider wins.
        let chosen = |name: &str| (name == "CLINE_PROVIDER").then(|| "cline".to_string());
        assert!(launch_env(&paths, &chosen, None).is_empty());
        // A Cline account last used: editors' own way.
        std::fs::write(
            &paths.providers,
            r#"{"lastUsedProvider":"cline","providers":{"cline":{"settings":{"auth":{"accessToken":"t"}}}}}"#,
        )
        .unwrap();
        assert_eq!(
            signed_in(&paths, &none).as_deref(),
            Some("With your Cline account.")
        );
        assert_eq!(terminal_provider(&paths), None);
        assert!(launch_env(&paths, &none, None).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_provider_chosen_in_pilothouse() {
        let home = folder("choose");
        let paths = Paths::under(&home, &|_| None);
        let none = |_: &str| None;
        std::fs::create_dir_all(paths.providers.parent().unwrap()).unwrap();
        std::fs::write(
            &paths.providers,
            r#"{"lastUsedProvider":"vertex","providers":{
                "vertex":{"settings":{"model":"gemini","gcp":{"projectId":"p"}}},
                "ollama":{"settings":{"model":"llama","baseUrl":"http://localhost:11434"}},
                "anthropic":{"settings":{"apiKey":"k"}},
                "cline":{"settings":{"auth":{"accessToken":"t"}}}}}"#,
        )
        .unwrap();
        // Those it can run on here: no keys, the last used first.
        let ids: Vec<String> = keyless_providers(&paths)
            .into_iter()
            .map(|provider| provider.id)
            .collect();
        assert_eq!(ids, ["vertex", "ollama"]);
        let pinned = |chosen| {
            launch_env(&paths, &none, chosen)
                .into_iter()
                .find(|(name, _)| name == "CLINE_PROVIDER")
                .map(|(_, value)| value)
        };
        assert_eq!(pinned(None).as_deref(), Some("vertex"));
        assert_eq!(pinned(Some("ollama")).as_deref(), Some("ollama"));
        // An account: Cline picks it over ACP itself.
        assert_eq!(pinned(Some("cline")), None);
        // One that needs a key, or is gone: as in the terminal.
        assert_eq!(pinned(Some("anthropic")).as_deref(), Some("vertex"));
        assert_eq!(pinned(Some("gone")).as_deref(), Some("vertex"));
        assert!(needs_restart(Some("vertex"), None));
        assert!(!needs_restart(None, None));
        assert_eq!(provider_name("vertex"), "Vertex AI");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn mcp_settings_keep_the_users_servers_and_add_ours() {
        let home = folder("mcp");
        let paths = Paths::under(&home, &|_| None);
        let ours = McpServer::Stdio(
            McpServerStdio::new("pilothouse", "/app/pilothouse")
                .args(vec!["mcp".into()])
                .env(vec![EnvVariable::new("A", "1")]),
        );
        // No file of the user's: only ours.
        let settings = mcp_settings(&paths, std::slice::from_ref(&ours));
        assert_eq!(
            settings["mcpServers"]["pilothouse"]["transport"],
            json!({ "type": "stdio", "command": "/app/pilothouse", "args": ["mcp"], "env": { "A": "1" } })
        );
        std::fs::create_dir_all(paths.mcp_settings.parent().unwrap()).unwrap();
        std::fs::write(
            &paths.mcp_settings,
            r#"{"mcpServers":{"git":{"command":"uvx","args":["mcp-git"],"disabled":true},"pilothouse":{"command":"old"}}}"#,
        )
        .unwrap();
        let settings = mcp_settings(&paths, &[ours]);
        // Theirs as they wrote it; ours wins a name both use.
        assert_eq!(
            settings["mcpServers"]["git"],
            json!({ "command": "uvx", "args": ["mcp-git"], "disabled": true })
        );
        assert_eq!(
            settings["mcpServers"]["pilothouse"]["transport"]["command"],
            "/app/pilothouse"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
