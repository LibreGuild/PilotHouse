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
    checks.push(Check {
        title: "Signed in".into(),
        state: CheckState::Note,
        detail: spec.login_hint.into(),
        action: None,
    });
    checks
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
