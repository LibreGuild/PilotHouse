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

/// Commands that run tests: a program, or a program and what it is told.
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

/// Programs that run the command named after them (and after their own
/// options): `npx vitest`, `uv run pytest`, `python -m pytest`.
const WRAPPERS: &[&[&str]] = &[
    &["pnpm", "exec"],
    &["pnpm", "dlx"],
    &["pnpm", "run"],
    &["npm", "exec"],
    &["npm", "run"],
    &["yarn", "exec"],
    &["yarn", "dlx"],
    &["yarn", "run"],
    &["bun", "run"],
    &["bun", "x"],
    &["uv", "run"],
    &["poetry", "run"],
    &["pipenv", "run"],
    &["pdm", "run"],
    &["hatch", "run"],
    &["rye", "run"],
    &["bundle", "exec"],
    &["python", "-m"],
    &["python3", "-m"],
    &["py", "-m"],
    &["doppler", "run"],
    &["mise", "exec"],
    &["op", "run"],
    &["docker", "compose", "exec"],
    &["docker", "compose", "run"],
    &["docker", "exec"],
    &["docker", "run"],
    &["docker-compose", "exec"],
    &["docker-compose", "run"],
    &["sudo"],
    &["doas"],
    &["time"],
    &["env"],
    &["nice"],
    &["nohup"],
    &["command"],
    &["exec"],
    &["npx"],
    &["bunx"],
    &["pnpx"],
    &["uvx"],
    &["dotenv"],
    &["xvfb-run"],
    &["timeout"],
    &["gtimeout"],
    // A package's own program: `pnpm vitest`, `yarn jest`.
    &["pnpm"],
    &["yarn"],
];

/// Wrappers that take one word of their own before the command: how
/// long, or where it runs.
const WRAPPERS_WITH_A_WORD: &[&str] = &["timeout", "gtimeout", "docker", "docker-compose"];

/// Shells, which run the script after `-c`.
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "fish"];

/// The commands of a shell line, each as its words: split where one
/// command ends and the next starts (`&&`, `||`, `;`, `|`, brackets, a
/// line end), quotes taken off. What is in quotes stays one word, so a
/// command that only names a test tool in a message or a pattern is not
/// taken apart there.
fn commands_in(line: &str) -> Vec<Vec<String>> {
    let mut commands = vec![Vec::new()];
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(open) if c == open => quote = None,
            Some('"') if c == '\\' => word.extend(chars.next()),
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c == '\\' => {
                word.extend(chars.next());
                in_word = true;
            }
            None if c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '{' | '}') => {
                if std::mem::take(&mut in_word)
                    && let Some(command) = commands.last_mut()
                {
                    command.push(std::mem::take(&mut word));
                }
                if c != ' ' && c != '\t' {
                    commands.push(Vec::new());
                }
            }
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word && let Some(command) = commands.last_mut() {
        command.push(word);
    }
    commands
}

/// Whether the command `words` runs tests: by the program it starts, not
/// by what it mentions (`pip install pytest`, `cat jest.config.js` and
/// `git commit -m "make test green"` run none).
fn runs_tests(mut words: &[String]) -> bool {
    loop {
        // Variables set for the command, and the options of a wrapper
        // taken off before.
        while let Some((first, rest)) = words.split_first()
            && (first.starts_with('-') || is_assignment(first))
        {
            words = rest;
        }
        let Some((program, rest)) = words.split_first() else {
            return false;
        };
        // By its name, wherever it is started from (`./gradlew`,
        // `node_modules/.bin/jest`).
        let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
        if TEST_COMMANDS
            .iter()
            .any(|test| is_command(test, name, rest))
        {
            return true;
        }
        if SHELLS.contains(&name) {
            let script = rest
                .iter()
                .skip_while(|word| !(word.starts_with('-') && word.ends_with('c')))
                .nth(1);
            return script.is_some_and(|script| is_test_command(script));
        }
        let Some(wrapper) = WRAPPERS.iter().find(|wrapper| {
            wrapper[0] == name
                && rest.len() + 1 >= wrapper.len()
                && wrapper[1..] == rest[..wrapper.len() - 1]
        }) else {
            return false;
        };
        words = &words[wrapper.len()..];
        if WRAPPERS_WITH_A_WORD.contains(&wrapper[0]) {
            while let Some((first, rest)) = words.split_first()
                && (first.starts_with('-') || is_assignment(first))
            {
                words = rest;
            }
            words = words.get(1..).unwrap_or_default();
        }
    }
}

/// `NAME=value`, as set before a command.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether the program `name` with the words `rest` after it is the test
/// command `test` (`cargo test`): its words in their order, with options
/// allowed before the last (`cargo -q test`). The last may name one of
/// several scripts (`npm run test:unit`).
fn is_command(test: &str, name: &str, rest: &[String]) -> bool {
    let mut wanted = test.split(' ');
    if wanted.next() != Some(name) {
        return false;
    }
    let mut rest = rest.iter().filter(|word| !word.starts_with(['-', '+']));
    wanted.all(|wanted| {
        rest.next().is_some_and(|word| {
            word.strip_prefix(wanted)
                .is_some_and(|after| after.is_empty() || after.starts_with(':'))
        })
    })
}

fn is_test_command(command: &str) -> bool {
    commands_in(&command.to_lowercase())
        .iter()
        .any(|words| runs_tests(words))
}

/// Whether the call took a screenshot: a tool named for it, not a command
/// or a file read that has the word in it (`ls screenshots/`), unless it
/// gave a picture back.
fn took_screenshot(tool: &ToolEntry) -> bool {
    tool.title.to_lowercase().contains("screenshot")
        && (!tool.images.is_empty()
            || !matches!(
                tool.kind,
                ToolKindLabel::Execute
                    | ToolKindLabel::Read
                    | ToolKindLabel::Search
                    | ToolKindLabel::Edit
                    | ToolKindLabel::Delete
                    | ToolKindLabel::Move
            ))
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
        if took_screenshot(tool) {
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
            images: Vec::new(),
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
    fn a_command_that_only_names_a_test_tool_ran_no_tests() {
        for command in [
            "cargo test -p shop",
            "cd crates/shop && RUST_BACKTRACE=1 cargo test 2>&1 | tail -20",
            "cargo -q nextest run",
            "npm run test:unit",
            "pnpm vitest run",
            "npx --yes jest src",
            "yarn test --watch=false",
            "python3 -m pytest -q",
            "uv run --frozen pytest",
            ".venv/bin/pytest tests",
            "./gradlew test",
            "bundle exec rspec",
            "timeout 300 go test ./...",
            "bash -lc 'cargo test'",
            "docker compose exec web pytest",
            "(cd web; npm test)",
            "make -j4 check",
        ] {
            assert!(is_test_command(command), "{command}");
        }
        for command in [
            "pip install pytest",
            "npm install -D vitest",
            "pnpm add jest",
            "cat jest.config.js",
            "rg \"cargo test\" .github",
            "git commit -m \"make test green\"",
            "git commit -m 'fix; pytest now passes'",
            "cat docs/perspective.md",
            "echo \"then run: npm test\"",
            "ls tests && cargo build",
            "cargo testing",
            "which pytest",
        ] {
            assert!(!is_test_command(command), "{command}");
        }
        // Such a command is a command run, not a test that passed.
        let evidence = of(&[run("pip install pytest", ToolStatus::Done, "")]);
        assert!(evidence.tests.is_empty());
        assert_eq!(evidence.commands, 1);
    }

    #[test]
    fn a_screenshot_is_a_tool_that_took_one() {
        let tool = |title: &str, kind, image: bool| {
            let Entry::Tool(mut tool) = run(title, ToolStatus::Done, "") else {
                unreachable!()
            };
            tool.kind = kind;
            if image {
                tool.images.push("shot.png".into());
            }
            Entry::Tool(tool)
        };
        let entries = [
            tool("browser_screenshot", ToolKindLabel::Other, false),
            tool("ls screenshots/", ToolKindLabel::Execute, false),
            tool("Read docs/screenshot.png", ToolKindLabel::Read, false),
            tool(
                "adb exec-out screencap # screenshot",
                ToolKindLabel::Execute,
                true,
            ),
        ];
        let evidence = of(&entries);
        assert_eq!(evidence.screenshots, 2);
        assert_eq!(evidence.commands, 1);
    }

    #[test]
    fn a_call_still_running_shows_nothing() {
        let entries = [run("npm test", ToolStatus::Running, "")];
        assert!(of(&entries).is_empty());
    }
}
