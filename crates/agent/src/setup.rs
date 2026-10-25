//! What an agent needs before it can run, checked on this machine, so the
//! chat can say exactly what is missing and how to fix it.
//!
//! Checks are plain file-system lookups (the CLI on `PATH`, `npx`, the
//! adapter in npm's cache); nothing is installed or downloaded here.

use std::path::{Path, PathBuf};

use crate::agents::AgentSpec;

/// Where to get Node.js (opened in the user's browser on request).
pub const NODE_DOWNLOAD: &str = "https://nodejs.org/en/download";

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub title: String,
    pub state: CheckState,
    pub detail: String,
    pub action: Option<Fix>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Ok,
    Missing,
    /// Not checkable ahead of time, or handled automatically (shown as info).
    Note,
}

/// How the user can fix a check.
#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    /// A command to run in a terminal (offered with a Copy button).
    Command(String),
    /// A web page to open in the browser.
    Link {
        label: &'static str,
        url: &'static str,
    },
}

/// Everything `spec` needs, in the order to fix it.
pub fn checks(spec: &AgentSpec) -> Vec<Check> {
    let mut checks = vec![match ph_env::which(spec.cli) {
        Some(path) => Check {
            title: format!("{} is installed", spec.name),
            state: CheckState::Ok,
            detail: path.display().to_string(),
            action: None,
        },
        None => Check {
            title: format!("Install {}", spec.name),
            state: CheckState::Missing,
            detail: format!("No `{}` on your PATH. Run this in a terminal:", spec.cli),
            action: Some(Fix::Command(spec.install_command().to_string())),
        },
    }];
    if let Some(package) = spec.adapter() {
        let has_npx = ph_env::which("npx").is_some();
        checks.push(if has_npx {
            Check {
                title: "Node.js is installed".into(),
                state: CheckState::Ok,
                detail: "Runs the agent's ACP adapter.".into(),
                action: None,
            }
        } else {
            Check {
                title: "Install Node.js".into(),
                state: CheckState::Missing,
                detail: format!(
                    "{} runs through an adapter that needs Node.js (npx).",
                    spec.name
                ),
                action: Some(Fix::Link {
                    label: "Download Node.js",
                    url: NODE_DOWNLOAD,
                }),
            }
        });
        let (name, version) = split_package(package);
        checks.push(if adapter_cached(name, version) {
            Check {
                title: "Adapter downloaded".into(),
                state: CheckState::Ok,
                detail: format!("{name} {version}"),
                action: None,
            }
        } else {
            Check {
                title: "Adapter downloads on first start".into(),
                state: CheckState::Note,
                detail: format!(
                    "The first chat downloads {name} {version} from npm once (needs internet)."
                ),
                action: None,
            }
        });
    }
    // Shown only when the agent's own sign-in record is found: without one
    // it may still be signed in some other way (a keychain), and the chat
    // offers its sign-in methods if it asks.
    if let Some(how) = dirs::home_dir().and_then(|home| signed_in(spec.id, &home, &env_var)) {
        checks.push(Check {
            title: "Signed in".into(),
            state: CheckState::Ok,
            detail: how.into(),
            action: None,
        });
    }
    checks
}

fn env_var(name: &str) -> Option<String> {
    ph_env::var(name)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

/// How agent `id` is signed in on this machine, if its own record says so
/// (`home` is the user's home folder, `var` reads the login environment).
/// Nothing secret is read: only whether the record is there.
fn signed_in(id: &str, home: &Path, var: &dyn Fn(&str) -> Option<String>) -> Option<&'static str> {
    let json = |path: PathBuf| -> Option<serde_json::Value> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    };
    match id {
        "claude" => {
            let account = json(home.join(".claude.json"))
                .is_some_and(|config| config.get("oauthAccount").is_some_and(|a| !a.is_null()));
            if account || home.join(".claude/.credentials.json").is_file() {
                Some("With your Claude account.")
            } else {
                var("ANTHROPIC_API_KEY").map(|_| "With ANTHROPIC_API_KEY.")
            }
        }
        "codex" => {
            let auth = json(home.join(".codex/auth.json")).is_some_and(|auth| {
                ["tokens", "OPENAI_API_KEY"]
                    .iter()
                    .any(|key| auth.get(key).is_some_and(|value| !value.is_null()))
            });
            if auth {
                Some("With `codex login`.")
            } else {
                var("OPENAI_API_KEY").map(|_| "With OPENAI_API_KEY.")
            }
        }
        "gemini" => {
            let google = json(home.join(".gemini/google_accounts.json"))
                .is_some_and(|accounts| accounts.get("active").is_some_and(|a| !a.is_null()))
                || home.join(".gemini/oauth_creds.json").is_file();
            if google {
                Some("With your Google account.")
            } else {
                var("GEMINI_API_KEY").map(|_| "With GEMINI_API_KEY.")
            }
        }
        _ => None,
    }
}

/// Whether nothing blocks starting the agent.
pub fn ready(checks: &[Check]) -> bool {
    checks
        .iter()
        .all(|check| check.state != CheckState::Missing)
}

/// `@scope/name@1.2.3` into its name and version.
fn split_package(package: &str) -> (&str, &str) {
    match package[1..].rfind('@') {
        Some(at) => (&package[..at + 1], &package[at + 2..]),
        None => (package, ""),
    }
}

/// Whether npx has this exact adapter version in its cache.
fn adapter_cached(name: &str, version: &str) -> bool {
    let Some(cache) = npm_cache() else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(cache.join("_npx")) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let manifest = entry
            .path()
            .join("node_modules")
            .join(name)
            .join("package.json");
        has_version(&manifest, version)
    })
}

fn has_version(manifest: &Path, version: &str) -> bool {
    std::fs::read_to_string(manifest)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| json.get("version")?.as_str().map(|found| found == version))
        .unwrap_or(false)
}

/// npm's cache folder: `npm_config_cache`, else the platform default.
fn npm_cache() -> Option<PathBuf> {
    if let Some(dir) = ph_env::var("npm_config_cache") {
        return Some(PathBuf::from(dir));
    }
    if cfg!(windows) {
        dirs::data_local_dir().map(|dir| dir.join("npm-cache"))
    } else {
        dirs::home_dir().map(|dir| dir.join(".npm"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_records_are_recognised() {
        let home = std::env::temp_dir().join(format!("ph-signin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        let none = |_: &str| None;
        assert_eq!(signed_in("claude", &home, &none), None);
        std::fs::write(home.join(".claude.json"), r#"{"oauthAccount":{"x":1}}"#).unwrap();
        assert!(signed_in("claude", &home, &none).is_some());
        std::fs::write(home.join(".codex/auth.json"), r#"{"tokens":null}"#).unwrap();
        assert_eq!(signed_in("codex", &home, &none), None);
        let key = |name: &str| (name == "OPENAI_API_KEY").then(|| "k".to_string());
        assert!(signed_in("codex", &home, &key).is_some());
        std::fs::write(home.join(".gemini/google_accounts.json"), r#"{"active":"a"}"#).unwrap();
        assert!(signed_in("gemini", &home, &none).is_some());
        assert_eq!(signed_in("antigravity", &home, &none), None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn splits_scoped_packages() {
        assert_eq!(
            split_package("@agentclientprotocol/claude-agent-acp@0.95.0"),
            ("@agentclientprotocol/claude-agent-acp", "0.95.0")
        );
        assert_eq!(split_package("plain@1.0.0"), ("plain", "1.0.0"));
    }

    #[test]
    fn finds_a_cached_adapter_version() {
        let dir = std::env::temp_dir().join(format!("ph-setup-{}", std::process::id()));
        let manifest = dir.join("pkg.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&manifest, r#"{"name":"x","version":"0.95.0"}"#).unwrap();
        assert!(has_version(&manifest, "0.95.0"));
        assert!(!has_version(&manifest, "0.96.0"));
        assert!(!has_version(&dir.join("missing.json"), "0.95.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
