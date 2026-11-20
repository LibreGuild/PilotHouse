//! Newer versions of the agents' CLIs installed here.
//!
//! Once a day (and a minute after the app starts) each installed CLI is
//! asked its version (`claude --version`), and the npm registry is asked
//! the newest version of the package that carries its releases (one small
//! request each, naming only the package). A CLI that is behind shows in
//! the status bar (*2 agent updates*), and its pop-up updates it with the
//! agent's own updater, in a terminal where it is seen to run.
//!
//! Only the registry is asked, nothing about this computer is sent, and
//! the look can be switched off (Settings → Agents). A build whose brand
//! turns updates off never looks.
//!
//! This is about the CLI on the user's `PATH`: what a session's CLI form
//! runs. A chat runs on the adapter this version of the app was built
//! with, which a CLI's update does not change.

use std::time::Duration;

use gpui_kit::{App, AppContext as _, Global, Task};
use ph_agent::agents::{AGENTS, AgentSpec};

/// Whether to look at all: the user's choice, on unless switched off.
const CHECK: &str = "agents.check_updates";
const FIRST_LOOK: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// The longest a CLI gets to say its version.
const VERSION_WAIT: Duration = Duration::from_secs(10);

/// An installed CLI that is behind its newest release.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AgentUpdate {
    pub agent: &'static AgentSpec,
    pub installed: String,
    pub latest: String,
}

#[derive(Default)]
pub(crate) struct AgentUpdates {
    behind: Vec<AgentUpdate>,
    _look: Option<Task<()>>,
    _schedule: Option<Task<()>>,
}

impl Global for AgentUpdates {}

/// Where each agent's releases are published, and how its CLI is updated.
/// An agent with no entry is not looked at (Antigravity publishes nowhere
/// this can read a version from).
fn release(agent: &str) -> Option<(&'static str, &'static str)> {
    match agent {
        "claude" => Some(("@anthropic-ai/claude-code", "claude update")),
        "codex" => Some(("@openai/codex", "codex update")),
        "gemini" => Some((
            "@google/gemini-cli",
            "npm install -g @google/gemini-cli@latest",
        )),
        "cline" => Some(("cline", "cline update")),
        _ => None,
    }
}

/// The command that updates agent `id`'s CLI: its own updater where it has
/// one, else its package's install.
pub(crate) fn update_command(id: &str) -> Option<&'static str> {
    release(id).map(|(_, command)| command)
}

/// Whether this build and this user look for newer CLIs.
pub(crate) fn enabled(cx: &App) -> bool {
    ph_brand::UPDATES_ENABLED
        && crate::AppState::store(cx)
            .setting::<Option<bool>>(CHECK)
            .unwrap_or(true)
}

/// Switches the look on or off; off, what was found is forgotten.
pub(crate) fn set_enabled(on: bool, cx: &mut App) {
    let _ = crate::AppState::store(cx).set_setting(CHECK, &Some(on));
    if on {
        look(cx);
    } else {
        let updates = cx.default_global::<AgentUpdates>();
        updates.behind.clear();
        updates._look = None;
        cx.refresh_windows();
    }
}

/// The CLIs found behind, as of the last look.
pub(crate) fn behind(cx: &App) -> &[AgentUpdate] {
    cx.try_global::<AgentUpdates>()
        .map(|updates| updates.behind.as_slice())
        .unwrap_or_default()
}

/// Starts the daily look.
pub(crate) fn install(cx: &mut App) {
    if cx.has_global::<AgentUpdates>() || !ph_brand::UPDATES_ENABLED {
        return;
    }
    let schedule = cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_LOOK).await;
        loop {
            cx.update(look);
            cx.background_executor().timer(EVERY).await;
        }
    });
    cx.set_global(AgentUpdates {
        _schedule: Some(schedule),
        ..Default::default()
    });
}

/// Looks now (off the UI thread), unless switched off.
pub(crate) fn look(cx: &mut App) {
    if !enabled(cx) {
        return;
    }
    let found = cx.background_spawn(async move {
        AGENTS
            .iter()
            .filter_map(|agent| {
                let (package, _) = release(agent.id)?;
                let installed = installed_version(agent.cli)?;
                let latest = match latest_version(package) {
                    Ok(latest) => latest,
                    Err(err) => {
                        tracing::debug!("the newest version of {package}: {err:#}");
                        return None;
                    }
                };
                newer(&latest, &installed).then_some(AgentUpdate {
                    agent,
                    installed,
                    latest,
                })
            })
            .collect::<Vec<_>>()
    });
    let look = cx.spawn(async move |cx| {
        let behind = found.await;
        cx.update(|cx| {
            let updates = cx.default_global::<AgentUpdates>();
            if updates.behind != behind {
                updates.behind = behind;
                cx.refresh_windows();
            }
        });
    });
    cx.default_global::<AgentUpdates>()._look = Some(look);
}

/// The version `cli` says it is (`--version`), when it is installed and
/// answers in time. Blocking.
fn installed_version(cli: &str) -> Option<String> {
    let program = ph_env::which(cli)?;
    let mut child = ph_env::command(program)
        .arg("--version")
        .envs(ph_env::shell_env())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    // A CLI that hangs (waits for a terminal, a network) is not waited for.
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < VERSION_WAIT => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut said = String::new();
    use std::io::Read as _;
    child.stdout.take()?.read_to_string(&mut said).ok()?;
    version_in(&said)
}

/// The first version (`1.2.3`) in what a CLI printed: `2.1.3 (Claude
/// Code)`, `codex-cli 0.164.0`, or a line of its own after other lines.
fn version_in(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+'))
        .find_map(|word| {
            let word = word.trim_start_matches('v');
            let core = word.split(['-', '+']).next().unwrap_or_default();
            let parts: Vec<&str> = core.split('.').collect();
            (parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())))
            .then(|| core.to_string())
        })
}

/// Whether version `latest` is after `installed`, number by number. Text
/// that is not a version is never "newer".
fn newer(latest: &str, installed: &str) -> bool {
    let numbers = |version: &str| -> Option<Vec<u64>> {
        version.split('.').map(|part| part.parse().ok()).collect()
    };
    match (numbers(latest), numbers(installed)) {
        (Some(latest), Some(installed)) => latest > installed,
        _ => false,
    }
}

/// The newest released version of npm package `package`, from the public
/// registry. Blocking. The request names the package and nothing else.
fn latest_version(package: &str) -> anyhow::Result<String> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(20)))
        .build()
        .into();
    let body = agent
        .get(&format!("https://registry.npmjs.org/{package}/latest"))
        .header("accept", "application/json")
        .call()?
        .body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_vec()?;
    let release: serde_json::Value = serde_json::from_slice(&body)?;
    release["version"]
        .as_str()
        .and_then(version_in)
        .ok_or_else(|| anyhow::anyhow!("the registry named no version"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_read_from_what_a_cli_prints() {
        assert_eq!(
            version_in("2.1.329 (Claude Code)").as_deref(),
            Some("2.1.329")
        );
        assert_eq!(
            version_in("codex-cli 0.164.0\n").as_deref(),
            Some("0.164.0")
        );
        assert_eq!(version_in("0.66.0").as_deref(), Some("0.66.0"));
        assert_eq!(version_in("v3.0.46").as_deref(), Some("3.0.46"));
        // A line of its own after a warning; a pre-release is its release.
        assert_eq!(
            version_in("(node:1) Warning: x 1.2\nCline 3.1.0-beta.2\n").as_deref(),
            Some("3.1.0")
        );
        assert_eq!(version_in("no version here 12.5"), None);
        assert_eq!(version_in(""), None);
    }

    #[test]
    fn newer_is_by_number_not_by_text() {
        assert!(newer("0.171.0", "0.164.0"));
        assert!(newer("2.10.0", "2.9.9"));
        assert!(newer("3.0.0", "2.99.99"));
        assert!(!newer("0.164.0", "0.164.0"));
        // Ahead of the registry (a build of one's own): nothing to say.
        assert!(!newer("1.5.0", "1.6.0"));
        assert!(!newer("latest", "1.0.0"));
        assert!(!newer("1.0.0", ""));
    }

    #[test]
    fn each_cli_has_its_own_way_to_update() {
        assert_eq!(update_command("claude"), Some("claude update"));
        assert_eq!(update_command("codex"), Some("codex update"));
        assert_eq!(
            update_command("gemini"),
            Some("npm install -g @google/gemini-cli@latest")
        );
        assert_eq!(update_command("cline"), Some("cline update"));
        // Nowhere to read its newest version from: not looked at.
        assert_eq!(update_command("antigravity"), None);
        for agent in AGENTS.iter() {
            if let Some((package, command)) = release(agent.id) {
                assert!(!package.contains(' ') && !command.is_empty());
            }
        }
    }
}
