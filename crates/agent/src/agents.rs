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
}

pub fn find(id: &str) -> Option<&'static AgentSpec> {
    AGENTS.iter().find(|agent| agent.id == id)
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
