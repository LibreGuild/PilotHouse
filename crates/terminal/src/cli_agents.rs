//! Coding-agent CLIs that PilotHouse has no chat for (they do not speak ACP,
//! or PilotHouse has no adapter for them yet). They run in a Terminal tab,
//! in the workspace's folder, like the user would start them by hand.

/// A CLI the Terminal tab offers to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliAgent {
    pub name: &'static str,
    /// The program looked up on `PATH`.
    pub program: &'static str,
    /// What is typed to start it.
    pub command: &'static str,
}

pub const CLI_AGENTS: &[CliAgent] = &[
    CliAgent {
        name: "Aider",
        program: "aider",
        command: "aider",
    },
    CliAgent {
        name: "Amp",
        program: "amp",
        command: "amp",
    },
    CliAgent {
        name: "Crush",
        program: "crush",
        command: "crush",
    },
    CliAgent {
        name: "Cursor Agent",
        program: "cursor-agent",
        command: "cursor-agent",
    },
    CliAgent {
        name: "GitHub Copilot CLI",
        program: "copilot",
        command: "copilot",
    },
    CliAgent {
        name: "Goose",
        program: "goose",
        command: "goose session",
    },
    CliAgent {
        name: "OpenCode",
        program: "opencode",
        command: "opencode",
    },
    CliAgent {
        name: "Qwen Code",
        program: "qwen",
        command: "qwen",
    },
];

/// The CLIs installed on this machine (found on the shell's `PATH`).
pub fn installed() -> Vec<&'static CliAgent> {
    CLI_AGENTS
        .iter()
        .filter(|agent| ph_env::which(agent.program).is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_cli_is_listed_once_and_starts_with_its_program() {
        for (ix, agent) in CLI_AGENTS.iter().enumerate() {
            assert!(agent.command.starts_with(agent.program), "{}", agent.name);
            assert!(
                CLI_AGENTS[ix + 1..]
                    .iter()
                    .all(|other| other.program != agent.program),
                "{} is listed twice",
                agent.program
            );
        }
    }
}
