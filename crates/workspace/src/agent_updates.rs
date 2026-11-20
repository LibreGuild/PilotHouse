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
//! Once an update was started from the pop-up, that CLI is asked its
//! version every few seconds for a few minutes: the updater runs in a
//! terminal and does not say when it is done, and the status bar should
//! not go on saying *1 agent update* for a day. That asks the CLI only
//! (the newest version is known already), so nothing goes over the
//! network for it. The pop-up also has *Check again*, which looks at
//! everything anew.
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
/// After an update was started: how often its CLI is asked its version,
/// and for how long (an updater that downloads a new build takes a while;
/// one that needs an answer in its terminal, longer).
const WATCH_EVERY: Duration = Duration::from_secs(4);
const WATCH_FOR: Duration = Duration::from_secs(5 * 60);

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
    /// A look is under way.
    looking: bool,
    /// The agents whose update was started and is watched, each with the
    /// watch.
    watched: Vec<(&'static str, Task<()>)>,
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
        updates.watched.clear();
        updates.looking = false;
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

/// Whether a look is under way (the pop-up's *Check again* turns).
pub(crate) fn looking(cx: &App) -> bool {
    cx.try_global::<AgentUpdates>()
        .is_some_and(|updates| updates.looking)
}

/// Whether agent `id`'s update was started and is being watched.
pub(crate) fn updating(id: &str, cx: &App) -> bool {
    cx.try_global::<AgentUpdates>()
        .is_some_and(|updates| updates.watched.iter().any(|(agent, _)| *agent == id))
}

/// Agent `id`'s updater was started: its CLI is asked its version every
/// few seconds until it is no longer behind (it then leaves the list), or
/// for [`WATCH_FOR`]. Only the CLI is asked; the newest version is known.
pub(crate) fn watch(id: &'static str, cx: &mut App) {
    let Some(update) = behind(cx)
        .iter()
        .find(|update| update.agent.id == id)
        .cloned()
    else {
        return;
    };
    let cli = update.agent.cli;
    let watch = cx.spawn(async move |cx| {
        let started = std::time::Instant::now();
        while started.elapsed() < WATCH_FOR {
            cx.background_executor().timer(WATCH_EVERY).await;
            // Mid-update the CLI may be gone or not answer: asked again.
            let Some(installed) = cx
                .background_spawn(async move { installed_version(cli) })
                .await
            else {
                continue;
            };
            let changed = cx.update(|cx| {
                seen_at(
                    &mut cx.default_global::<AgentUpdates>().behind,
                    id,
                    &installed,
                )
            });
            if changed {
                cx.update(|cx| cx.refresh_windows());
            }
            let gone = cx.update(|cx| !behind(cx).iter().any(|update| update.agent.id == id));
            if gone {
                break;
            }
        }
        cx.update(|cx| {
            cx.default_global::<AgentUpdates>()
                .watched
                .retain(|(agent, _)| *agent != id);
            cx.refresh_windows();
        });
    });
    let updates = cx.default_global::<AgentUpdates>();
    // Pressed again: watched from now, once.
    updates.watched.retain(|(agent, _)| *agent != id);
    updates.watched.push((id, watch));
    cx.refresh_windows();
}

/// Agent `id`'s CLI now says it is version `installed`: it leaves `behind`
/// when that is the newest, and says the version it is at when that
/// changed on the way. Whether the list changed.
fn seen_at(behind: &mut Vec<AgentUpdate>, id: &str, installed: &str) -> bool {
    let Some(at) = behind.iter().position(|update| update.agent.id == id) else {
        return false;
    };
    if !newer(&behind[at].latest, installed) {
        behind.remove(at);
    } else if behind[at].installed != installed {
        behind[at].installed = installed.to_string();
    } else {
        return false;
    }
    true
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
            updates.looking = false;
            updates.behind = behind;
            // An agent no longer behind is no longer watched.
            let behind: Vec<&str> = updates
                .behind
                .iter()
                .map(|update| update.agent.id)
                .collect();
            updates.watched.retain(|(agent, _)| behind.contains(agent));
            cx.refresh_windows();
        });
    });
    let updates = cx.default_global::<AgentUpdates>();
    updates.looking = true;
    updates._look = Some(look);
    cx.refresh_windows();
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
    fn an_updated_cli_leaves_the_list() {
        let update = |id: &str, installed: &str, latest: &str| AgentUpdate {
            agent: ph_agent::agents::find(id).unwrap(),
            installed: installed.into(),
            latest: latest.into(),
        };
        let mut behind = vec![
            update("claude", "2.1.3", "2.2.0"),
            update("codex", "0.164.0", "0.171.0"),
        ];
        // Still the version it was: nothing to draw again.
        assert!(!seen_at(&mut behind, "codex", "0.164.0"));
        // A step on the way: still behind, at the version it now says.
        assert!(seen_at(&mut behind, "codex", "0.170.0"));
        assert_eq!(behind[1].installed, "0.170.0");
        // At the newest (or past it): off the list, the other stays.
        assert!(seen_at(&mut behind, "codex", "0.171.0"));
        assert_eq!(behind.len(), 1);
        assert_eq!(behind[0].agent.id, "claude");
        assert!(seen_at(&mut behind, "claude", "2.3.1"));
        assert!(behind.is_empty());
        // Off the list already (a look took it): nothing changes.
        assert!(!seen_at(&mut behind, "claude", "2.3.1"));
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
