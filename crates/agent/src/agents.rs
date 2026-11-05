//! The coding agents PilotHouse knows how to start.
//!
//! Each agent is launched as an ACP server on stdio: Gemini CLI natively,
//! Claude Code and Codex through their official ACP adapters (run with `npx`,
//! which downloads them once), and Antigravity through PilotHouse's own
//! bridge (`pilothouse agy-acp`, see the `ph-agy` crate). The agents use the
//! user's own installs and logins; PilotHouse never sees credentials.

use std::path::PathBuf;

/// How to start an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Launch {
    /// The CLI speaks ACP itself: `program args…`.
    Native {
        program: &'static str,
        args: &'static [&'static str],
    },
    /// An ACP adapter package run with `npx -y <package>`. Versions are
    /// pinned: an unpinned name would run whatever the registry serves next,
    /// with the user's permissions. Bump them deliberately.
    Npx { package: &'static str },
    /// This binary run as a bridge: `pilothouse <subcommand>`.
    Bridge { subcommand: &'static str },
}

/// A supported agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentSpec {
    /// Stable id stored with sessions, e.g. `claude`.
    pub id: &'static str,
    pub name: &'static str,
    /// The user-facing CLI, needed for logging in.
    pub cli: &'static str,
    launch: Launch,
    /// What to run once to log in, shown when the agent reports no auth.
    pub login_hint: &'static str,
    /// How to install the CLI: (macOS and Linux, Windows) shell commands.
    pub install: (&'static str, &'static str),
    /// The agent's color (RGB), used for its glyph in pickers and chats.
    pub color: u32,
    /// A plain glyph for the agent, where its mark is not bundled.
    pub glyph: AgentGlyph,
    /// Where the models come from, shown under each model.
    pub provider: &'static str,
}

/// Generic glyphs that tell agents apart at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentGlyph {
    Asterisk,
    Hexagon,
    Sparkle,
    Orbit,
}

impl AgentSpec {
    pub fn icon(&self) -> ph_ui::Icons {
        match self.glyph {
            AgentGlyph::Asterisk => ph_ui::Icons::Asterisk,
            AgentGlyph::Hexagon => ph_ui::Icons::Hexagon,
            AgentGlyph::Sparkle => ph_ui::Icons::Sparkle,
            AgentGlyph::Orbit => ph_ui::Icons::Orbit,
        }
    }

    pub fn tint(&self) -> gpui_kit::Hsla {
        gpui_kit::rgb(self.color).into()
    }

    /// The agent's own mark (`ph_ui::agent_logo`), or its glyph in its
    /// colour, `size` across.
    pub fn mark(&self, size: gpui_kit::Pixels) -> gpui_kit::AnyElement {
        use gpui_kit::component::Sizable as _;
        use gpui_kit::{IntoElement as _, Styled as _};
        ph_ui::agent_logo(self.id, size).unwrap_or_else(|| {
            gpui_kit::component::Icon::new(self.icon())
                .with_size(size)
                .text_color(self.tint())
                .into_any_element()
        })
    }

    /// The name without "Code" or "CLI": Claude, Codex, Gemini.
    pub fn short_name(&self) -> &'static str {
        self.name
            .strip_suffix(" Code")
            .or_else(|| self.name.strip_suffix(" CLI"))
            .unwrap_or(self.name)
    }
}

pub const AGENTS: &[AgentSpec] = &[
    AgentSpec {
        id: "claude",
        name: "Claude Code",
        cli: "claude",
        launch: Launch::Npx {
            package: "@agentclientprotocol/claude-agent-acp@0.95.0",
        },
        login_hint: "Run `claude` in a terminal once and log in.",
        install: (
            "npm install -g @anthropic-ai/claude-code",
            "npm install -g @anthropic-ai/claude-code",
        ),
        color: 0xd97757,
        glyph: AgentGlyph::Asterisk,
        provider: "Anthropic",
    },
    AgentSpec {
        id: "codex",
        name: "Codex",
        cli: "codex",
        launch: Launch::Npx {
            package: "@agentclientprotocol/codex-acp@2.2.2",
        },
        login_hint: "Run `codex login` in a terminal once.",
        install: (
            "npm install -g @openai/codex",
            "npm install -g @openai/codex",
        ),
        color: 0x10a37f,
        glyph: AgentGlyph::Hexagon,
        provider: "OpenAI",
    },
    AgentSpec {
        id: "gemini",
        name: "Gemini CLI",
        cli: "gemini",
        launch: Launch::Native {
            program: "gemini",
            args: &["--acp"],
        },
        login_hint: "Run `gemini` in a terminal once and sign in.",
        install: (
            "npm install -g @google/gemini-cli",
            "npm install -g @google/gemini-cli",
        ),
        color: 0x4f8df7,
        glyph: AgentGlyph::Sparkle,
        provider: "Google",
    },
    AgentSpec {
        id: "antigravity",
        name: "Antigravity",
        cli: "agy",
        launch: Launch::Bridge {
            subcommand: "agy-acp",
        },
        login_hint: "Run `agy` in a terminal once and sign in.",
        install: (
            "curl -fsSL https://antigravity.google/cli/install.sh | bash",
            "irm https://antigravity.google/cli/install.ps1 | iex",
        ),
        color: 0x8b6cf6,
        glyph: AgentGlyph::Orbit,
        provider: "Google",
    },
];

impl AgentSpec {
    /// The install command for this platform.
    pub fn install_command(&self) -> &'static str {
        if cfg!(windows) {
            self.install.1
        } else {
            self.install.0
        }
    }

    /// The npm adapter package (`name@version`), for agents run with `npx`.
    pub fn adapter(&self) -> Option<&'static str> {
        match self.launch {
            Launch::Npx { package } => Some(package),
            _ => None,
        }
    }

    /// The CLI's own page: its npm package for those installed with npm,
    /// else the site its installer comes from.
    pub fn home_page(&self) -> &'static str {
        match self.id {
            "claude" => "https://www.npmjs.com/package/@anthropic-ai/claude-code",
            "codex" => "https://www.npmjs.com/package/@openai/codex",
            "gemini" => "https://www.npmjs.com/package/@google/gemini-cli",
            _ => "https://antigravity.google",
        }
    }
}

pub fn find(id: &str) -> Option<&'static AgentSpec> {
    AGENTS.iter().find(|agent| agent.id == id)
}

/// The agents found ready to start by the last [`detect`], by id; `None`
/// until it first ran (it waits for the login shell's environment).
static INSTALLED: std::sync::RwLock<Option<Vec<&'static str>>> = std::sync::RwLock::new(None);

/// Finds which agents can start on this machine (their CLI, and what their
/// launch needs, on the login shell's `PATH`), in [`AGENTS`] order, and
/// remembers it for [`installed`]. Blocking: it may wait seconds for the
/// login shell, so call it off the UI thread.
pub fn detect() -> Vec<&'static AgentSpec> {
    let found: Vec<&'static AgentSpec> = AGENTS
        .iter()
        .filter(|spec| matches!(spec.availability(), Availability::Ready { .. }))
        .collect();
    if let Ok(mut installed) = INSTALLED.write() {
        *installed = Some(found.iter().map(|spec| spec.id).collect());
    }
    found
}

/// What the last [`detect`] found, in [`AGENTS`] order; `None` before it ran.
pub fn installed() -> Option<Vec<&'static AgentSpec>> {
    let installed = INSTALLED.read().ok()?;
    installed
        .as_ref()
        .map(|ids| ids.iter().filter_map(|id| find(id)).collect())
}

/// The agent new chats start with: the one the user chose while it is
/// installed, else the first installed one, else (none installed, or not
/// looked for yet) the one chosen, else the first known.
pub fn pick_default(
    chosen: Option<&'static AgentSpec>,
    installed: Option<&[&'static AgentSpec]>,
) -> &'static AgentSpec {
    match installed {
        Some(installed) => chosen
            .filter(|chosen| installed.iter().any(|spec| spec.id == chosen.id))
            .or_else(|| installed.first().copied())
            .or(chosen)
            .unwrap_or(&AGENTS[0]),
        None => chosen.unwrap_or(&AGENTS[0]),
    }
}

/// The command to start an agent, or why it cannot start on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Ready { program: PathBuf, args: Vec<String> },
    Missing(String),
}

impl AgentSpec {
    /// Resolves the launch command against the user's login-shell `PATH`.
    pub fn availability(&self) -> Availability {
        if ph_env::which(self.cli).is_none() {
            return Availability::Missing(format!(
                "{} is not installed (no `{}` on your PATH).",
                self.name, self.cli
            ));
        }
        match self.launch {
            Launch::Native { program, args } => match ph_env::which(program) {
                Some(program) => Availability::Ready {
                    program,
                    args: args.iter().map(|arg| arg.to_string()).collect(),
                },
                None => Availability::Missing(format!("`{program}` is not on your PATH.")),
            },
            Launch::Npx { package } => match ph_env::which("npx") {
                Some(npx) => Availability::Ready {
                    program: npx,
                    args: vec!["-y".into(), package.into()],
                },
                None => Availability::Missing(format!(
                    "{} needs Node.js: `npx` was not found on your PATH.",
                    self.name
                )),
            },
            Launch::Bridge { subcommand } => match std::env::current_exe() {
                Ok(program) => Availability::Ready {
                    program,
                    args: vec![subcommand.into()],
                },
                Err(err) => Availability::Missing(format!("PilotHouse cannot find itself: {err}")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_chats_start_with_an_installed_agent() {
        let (claude, codex, gemini) = (
            find("claude").unwrap(),
            find("codex").unwrap(),
            find("gemini").unwrap(),
        );
        // Not looked for yet: the choice, else the first known.
        assert_eq!(pick_default(None, None).id, "claude");
        assert_eq!(pick_default(Some(codex), None).id, "codex");
        // The choice while it is installed.
        assert_eq!(
            pick_default(Some(codex), Some(&[claude, codex])).id,
            "codex"
        );
        // Nothing chosen: the first installed, not the first known.
        assert_eq!(pick_default(None, Some(&[codex, gemini])).id, "codex");
        // The chosen one went away: the first installed instead.
        assert_eq!(pick_default(Some(claude), Some(&[gemini])).id, "gemini");
        // Nothing installed: the choice still names the chat's agent (its
        // setup card says how to install it), else the first known.
        assert_eq!(pick_default(Some(codex), Some(&[])).id, "codex");
        assert_eq!(pick_default(None, Some(&[])).id, "claude");
    }

    #[test]
    fn every_agent_has_a_page() {
        for spec in AGENTS {
            assert!(spec.home_page().starts_with("https://"), "{}", spec.id);
        }
    }
}
