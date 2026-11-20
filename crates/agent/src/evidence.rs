//! What a chat did to check its own work: the tests and commands it ran and
//! the screenshots it took. It is shown over the changes the chat made, so
//! whoever reviews them sees what stands behind them before reading a line.
//!
//! All of it is read off the chat's own tool calls. Nothing is run again,
//! and nothing is claimed that the transcript does not show: a test
//! command's result is the one its call ended with.

use crate::transcript::{Entry, ToolEntry, ToolKindLabel, ToolStatus};

/// The last run of a test command.
#[derive(Debug, Clone, PartialEq)]
pub struct TestRun {
    pub command: String,
    pub ok: bool,
    /// How many tests passed and failed, where the output says.
    pub passed: Option<usize>,
    pub failed: Option<usize>,
}

/// What a chat did to check its work.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evidence {
    /// Each test command's last run (a run that failed and was put right
    /// shows as it ended).
    pub tests: Vec<TestRun>,
    /// Other commands run, and how many of them failed.
    pub commands: usize,
    pub failed_commands: usize,
    pub screenshots: usize,
}

impl Evidence {
    pub fn is_empty(&self) -> bool {
        self.tests.is_empty() && self.commands == 0 && self.screenshots == 0
    }
}

/// Parts of a command line that say it runs tests.
const TEST_COMMANDS: &[&str] = &[
    "cargo test",
    "cargo nextest",
    "npm test",
    "npm run test",
    "pnpm test",
    "pnpm run test",
    "yarn test",
    "bun test",
    "deno test",
    "pytest",
    "go test",
    "jest",
    "vitest",
    "mocha",
    "mvn test",
    "gradle test",
    "gradlew test",
    "dotnet test",
    "swift test",
    "flutter test",
    "rspec",
    "phpunit",
    "ctest",
    "make test",
    "make check",
    "xcodebuild test",
];

fn is_test_command(command: &str) -> bool {
    let command = command.to_lowercase();
    TEST_COMMANDS.iter().any(|test| command.contains(test))
}

/// The number before each `word` in `text` ("14 passed", "0 failed;"),
/// added up; `None` when it never stands there. Lines that count files or
/// suites, not tests, are left out.
fn count(text: &str, word: &str) -> Option<usize> {
    let mut total = None;
    for line in text.lines() {
        let start = line.trim_start();
        if start.starts_with("Test Suites:") || start.starts_with("Test Files") {
            continue;
        }
        let mut before: Option<usize> = None;
        for part in line.split_whitespace() {
            if part.starts_with(word)
                && let Some(number) = before
            {
                total = Some(total.unwrap_or(0) + number);
            }
            before = part.parse().ok();
        }
    }
    total
}

/// For a call that ran a test command: how many tests its output says
/// passed (`None` inside when it does not say). `None` for any other call.
pub(crate) fn tests_passed(tool: &ToolEntry) -> Option<Option<usize>> {
    (tool.kind == ToolKindLabel::Execute && is_test_command(&tool.title))
        .then(|| count(&tool.output, "passed"))
}

/// A tool call and the calls of the subagent it started.
fn with_children(tool: &ToolEntry) -> impl Iterator<Item = &ToolEntry> {
    std::iter::once(tool).chain(tool.children.iter())
}

/// What the chat with `entries` did to check its work.
pub fn of(entries: &[Entry]) -> Evidence {
    let mut evidence = Evidence::default();
    let tools = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool(tool) => Some(tool),
            _ => None,
        })
        .flat_map(with_children);
    for tool in tools {
        // Still going: it has shown nothing yet.
        if matches!(tool.status, ToolStatus::Pending | ToolStatus::Running) {
            continue;
        }
        if tool.title.to_lowercase().contains("screenshot") {
            evidence.screenshots += 1;
            continue;
        }
        if tool.kind != ToolKindLabel::Execute {
            continue;
        }
        let failed = tool.status == ToolStatus::Failed;
        if !is_test_command(&tool.title) {
            evidence.commands += 1;
            evidence.failed_commands += usize::from(failed);
            continue;
        }
        let run = TestRun {
            command: tool.title.clone(),
            ok: !failed,
            passed: count(&tool.output, "passed"),
            failed: count(&tool.output, "failed"),
        };
        match evidence
            .tests
            .iter_mut()
            .find(|known| known.command == run.command)
        {
            Some(known) => *known = run,
            None => evidence.tests.push(run),
        }
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(title: &str, status: ToolStatus, output: &str) -> Entry {
        Entry::Tool(ToolEntry {
            id: title.to_string(),
            title: title.to_string(),
            kind: ToolKindLabel::Execute,
            status,
            output: output.to_string(),
            diffs: Vec::new(),
            children: Vec::new(),
            agent_text: String::new(),
            run: Default::default(),
            expanded: false,
            started: None,
            touched: None,
        })
    }

    #[test]
    fn a_test_run_shows_as_it_ended() {
        let entries = [
            run(
                "cargo test -p shop",
                ToolStatus::Failed,
                "test result: FAILED. 12 passed; 2 failed; 0 ignored",
            ),
            run("cargo build", ToolStatus::Done, ""),
            run(
                "cargo test -p shop",
                ToolStatus::Done,
                "test result: ok. 10 passed; 0 failed;\ntest result: ok. 4 passed; 0 failed;",
            ),
            run("ls missing", ToolStatus::Failed, "No such file"),
        ];
        let evidence = of(&entries);
        assert_eq!(
            evidence.tests,
            vec![TestRun {
                command: "cargo test -p shop".into(),
                ok: true,
                passed: Some(14),
                failed: Some(0),
            }]
        );
        assert_eq!((evidence.commands, evidence.failed_commands), (2, 1));
    }

    #[test]
    fn suites_are_not_counted_as_tests() {
        let jest =
            "Test Suites: 1 failed, 2 passed, 3 total\nTests:       1 failed, 14 passed, 15 total";
        assert_eq!(count(jest, "passed"), Some(14));
        assert_eq!(count(jest, "failed"), Some(1));
        assert_eq!(count("ok  \tshop/cart\t0.31s", "passed"), None);
    }

    #[test]
    fn a_call_still_running_shows_nothing() {
        let entries = [run("npm test", ToolStatus::Running, "")];
        assert!(of(&entries).is_empty());
    }
}
