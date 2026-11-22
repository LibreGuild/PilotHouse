//! The project's own tests: the suites found in its folder (`cargo test`,
//! pytest, Vitest, Jest, Playwright, Gradle, Maven, `go test`…), and what a
//! run of one found.
//!
//! Nothing is executed to find them: detection reads `Cargo.toml`,
//! `package.json` and friends in the project folder and the folders right
//! under it (a `frontend` beside a `backend`). A suite PilotHouse knows how
//! to read is run so that it writes its results down (a JUnit file, JSON),
//! and those become one line per test, with the failure and where it is;
//! any other command is one line, passed or failed by how it exits, with
//! its output.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How a suite's results are read, and how a part of it is run again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Cargo,
    Pytest,
    Vitest,
    Jest,
    Playwright,
    Gradle,
    Maven,
    Go,
    /// Any other command: passed or failed by its exit code.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suite {
    /// Stable for the project: `cargo`, `frontend/vitest`, `own/make check`.
    pub id: String,
    /// What the user sees: `cargo test`, `frontend · Vitest`.
    pub name: String,
    pub kind: Kind,
    /// The command without what makes it write results down: what a user
    /// would type, shown under the name.
    pub command: String,
    /// The folder it runs in.
    pub cwd: PathBuf,
    /// Added by the user, rather than found.
    pub custom: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
}

/// One test of a suite, as its last run left it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    /// What it belongs to: a crate, a class, a file.
    pub group: String,
    pub name: String,
    pub outcome: Outcome,
    #[serde(default)]
    pub millis: Option<u64>,
    /// What went wrong, as the runner said it.
    #[serde(default)]
    pub message: Option<String>,
    /// Where: a file (relative to the suite's folder, or absolute) and a
    /// 1-based line.
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
}

/// What a run of a suite found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuiteResult {
    pub started_at: i64,
    pub millis: u64,
    /// The command's exit code; `None` when it was stopped or never started.
    pub exit: Option<i32>,
    #[serde(default)]
    pub stopped: bool,
    pub cases: Vec<Case>,
    /// The end of what the command printed.
    pub output: String,
    /// Only a part of the suite was run (the failed ones again, one test):
    /// the other cases are from the run before.
    #[serde(default)]
    pub partial: bool,
}

impl SuiteResult {
    pub fn count(&self, outcome: Outcome) -> usize {
        self.cases
            .iter()
            .filter(|case| case.outcome == outcome)
            .count()
    }

    /// The command ended well and no test failed.
    pub fn passed(&self) -> bool {
        !self.stopped && self.exit == Some(0) && self.count(Outcome::Failed) == 0
    }

    /// The command failed without any test saying so: it did not build, or
    /// is not installed.
    pub fn broke(&self) -> bool {
        !self.stopped && self.exit != Some(0) && self.count(Outcome::Failed) == 0
    }
}

impl Suite {
    fn found(dir: &Path, root: &Path, key: &str, name: &str, kind: Kind, command: String) -> Self {
        let under = dir
            .strip_prefix(root)
            .ok()
            .map(|under| under.to_string_lossy().replace('\\', "/"))
            .filter(|under| !under.is_empty());
        Self {
            id: match &under {
                Some(under) => format!("{under}/{key}"),
                None => key.to_string(),
            },
            name: match &under {
                Some(under) => format!("{under} · {name}"),
                None => name.to_string(),
            },
            kind,
            command,
            cwd: dir.to_path_buf(),
            custom: false,
        }
    }

    /// A command of the user's own, run in the project folder.
    pub fn custom(command: &str, root: &Path) -> Self {
        Self {
            id: format!("own/{command}"),
            name: command.to_string(),
            kind: Kind::Other,
            command: command.to_string(),
            cwd: root.to_path_buf(),
            custom: true,
        }
    }

    /// The command line that runs the suite (or only `only` of it) and
    /// writes its results to `results` where the runner can.
    pub fn command_line(&self, only: &[&Case], results: &Path) -> String {
        let results = quoted(&results.to_string_lossy());
        let files = || {
            let mut files: Vec<String> = only
                .iter()
                .filter_map(|case| case.file.clone().or_else(|| Some(case.group.clone())))
                .filter(|file| !file.is_empty())
                .map(|file| quoted(&file))
                .collect();
            files.sort();
            files.dedup();
            files.join(" ")
        };
        let with = |extra: String| {
            if extra.is_empty() {
                String::new()
            } else {
                format!(" {extra}")
            }
        };
        match self.kind {
            Kind::Cargo => {
                // A doc test has no name a filter takes.
                let names: Vec<String> = only
                    .iter()
                    .filter(|case| !case.name.contains(" - "))
                    .map(|case| quoted(&case.name))
                    .collect();
                if names.is_empty() {
                    self.command.clone()
                } else {
                    format!("{} -- --exact {}", self.command, names.join(" "))
                }
            }
            Kind::Pytest => {
                let ids: Vec<String> = only.iter().map(|case| quoted(&pytest_id(case))).collect();
                format!(
                    "{}{} -o junit_family=xunit1 --junitxml={results}",
                    self.command,
                    with(ids.join(" "))
                )
            }
            Kind::Vitest => format!(
                "{}{} --reporter=default --reporter=junit --outputFile.junit={results}",
                self.command,
                with(files())
            ),
            Kind::Jest => format!(
                "{}{} --json --outputFile={results}",
                self.command,
                with(files())
            ),
            // Where the JUnit file goes is said in the environment
            // ([`Suite::environment`]).
            Kind::Playwright => format!("{}{} --reporter=list,junit", self.command, with(files())),
            Kind::Gradle => {
                let tests: Vec<String> = only
                    .iter()
                    .map(|case| format!("--tests {}", quoted(&jvm_test(case, "."))))
                    .collect();
                format!("{}{}", self.command, with(tests.join(" ")))
            }
            Kind::Maven => {
                let tests: Vec<String> = only.iter().map(|case| jvm_test(case, "#")).collect();
                if tests.is_empty() {
                    self.command.clone()
                } else {
                    format!("{} -Dtest={}", self.command, quoted(&tests.join("+")))
                }
            }
            Kind::Go => {
                let mut names: Vec<&str> = only
                    .iter()
                    .filter_map(|case| case.name.split('/').next())
                    .collect();
                names.sort();
                names.dedup();
                if names.is_empty() {
                    format!("{} -json ./...", self.command)
                } else {
                    format!(
                        "{} -json -run {} ./...",
                        self.command,
                        quoted(&format!("^({})$", names.join("|")))
                    )
                }
            }
            Kind::Other => self.command.clone(),
        }
    }

    /// What the command needs in its environment to write `results`.
    pub fn environment(&self, results: &Path) -> Vec<(&'static str, String)> {
        let results = results.to_string_lossy().into_owned();
        match self.kind {
            // The name changed between versions: both are said.
            Kind::Playwright => vec![
                ("PLAYWRIGHT_JUNIT_OUTPUT_FILE", results.clone()),
                ("PLAYWRIGHT_JUNIT_OUTPUT_NAME", results),
            ],
            _ => Vec::new(),
        }
    }

    /// Whether a part of the suite can be run alone (a test, the failed
    /// ones): where the runner takes names or files.
    pub fn runs_parts(&self) -> bool {
        self.kind != Kind::Other
    }

    /// Whether running `case` alone runs only it, or its whole file (the
    /// runners that take files, not names).
    pub fn runs_one(&self) -> bool {
        matches!(
            self.kind,
            Kind::Cargo | Kind::Pytest | Kind::Gradle | Kind::Maven | Kind::Go
        )
    }

    /// The name of the task whose results Gradle leaves in
    /// `build/test-results/<task>`.
    pub(crate) fn gradle_task(&self) -> &str {
        self.command.rsplit(' ').next().unwrap_or("test")
    }
}

/// `text` as one word of a command line: in single quotes for a POSIX
/// shell (where nothing inside them means anything), in double quotes for
/// `cmd`. What would end the quotes, or be read as a variable there, is
/// left out: these are paths and test names, which have none in practice.
fn quoted(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:=+,@".contains(c));
    if plain {
        return text.to_string();
    }
    if cfg!(windows) {
        let safe: String = text
            .chars()
            .filter(|c| !matches!(c, '"' | '%' | '!'))
            .collect();
        format!("\"{safe}\"")
    } else {
        format!("'{}'", text.replace('\'', ""))
    }
}

/// `tests/test_cart.py::TestCart::test_add` from what pytest's JUnit file
/// says of a case: its file, and its class as a dotted path that starts
/// with the file's.
fn pytest_id(case: &Case) -> String {
    let Some(file) = &case.file else {
        return case.name.clone();
    };
    let module = file.trim_end_matches(".py").replace(['/', '\\'], ".");
    let classes = case
        .group
        .strip_prefix(&module)
        .unwrap_or_default()
        .trim_start_matches('.');
    let mut id = file.clone();
    for class in classes.split('.').filter(|class| !class.is_empty()) {
        id.push_str("::");
        id.push_str(class);
    }
    format!("{id}::{}", case.name)
}

/// `com.app.CartTest.adds` (Gradle, `between` a dot) or `CartTest#adds`
/// (Maven): the class, and the method when its name is one (a display name
/// with spaces is not something the runner can be asked for).
fn jvm_test(case: &Case, between: &str) -> String {
    let method = case.name.trim_end_matches("()");
    let named = !method.is_empty()
        && method
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$');
    if named {
        format!("{}{between}{method}", case.group)
    } else {
        case.group.clone()
    }
}

/// Folders right under the project that are never a part of it with tests
/// of its own.
const NOT_PARTS: &[&str] = &[
    "node_modules",
    "target",
    "build",
    "dist",
    "out",
    "vendor",
    "venv",
    "coverage",
    "__pycache__",
];
/// Folders right under the project looked into, at most.
const MAX_PARTS: usize = 24;

/// The suites of the project at `root`, and of the folders right under it.
/// Reads a few small files: call it off the UI thread.
pub fn detect(root: &Path) -> Vec<Suite> {
    let mut suites = detect_in(root, root, &[]);
    let at_root: Vec<Kind> = suites.iter().map(|suite| suite.kind).collect();
    let mut parts: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                !name.starts_with('.') && !NOT_PARTS.contains(&name.as_ref())
            })
        })
        .collect();
    parts.sort();
    for part in parts.into_iter().take(MAX_PARTS) {
        suites.extend(detect_in(&part, root, &at_root));
    }
    suites
}

/// The suites of one folder. `covered` are the kinds the project folder
/// already has: a workspace's `cargo test`, or a root `gradlew test`, runs
/// its parts' tests too, so those are not listed twice.
fn detect_in(dir: &Path, root: &Path, covered: &[Kind]) -> Vec<Suite> {
    let has = |name: &str| dir.join(name).exists();
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
    let mut suites = Vec::new();
    let mut add = |key: &str, name: &str, kind: Kind, command: String| {
        let js = |kind| matches!(kind, Kind::Vitest | Kind::Jest | Kind::Playwright);
        let dup = covered.contains(&kind)
            && (matches!(kind, Kind::Cargo | Kind::Gradle | Kind::Maven) || js(kind));
        if !dup {
            suites.push(Suite::found(dir, root, key, name, kind, command));
        }
    };

    if has("Cargo.toml") {
        // Every crate of a workspace, and all of them even when one fails.
        add(
            "cargo",
            "cargo test",
            Kind::Cargo,
            "cargo test --workspace --no-fail-fast".into(),
        );
    }

    if let Ok(package) = serde_json::from_str::<serde_json::Value>(&read("package.json")) {
        let exec = if has("pnpm-lock.yaml") {
            "pnpm exec"
        } else if has("yarn.lock") {
            "yarn"
        } else if has("bun.lockb") || has("bun.lock") {
            "bunx"
        } else {
            "npx"
        };
        let uses = |name: &str| {
            ["dependencies", "devDependencies"]
                .iter()
                .any(|deps| package[deps].get(name).is_some())
        };
        let mut known = false;
        if uses("vitest") {
            known = true;
            add(
                "vitest",
                "Vitest",
                Kind::Vitest,
                format!("{exec} vitest run"),
            );
        }
        if uses("jest") {
            known = true;
            add("jest", "Jest", Kind::Jest, format!("{exec} jest"));
        }
        if uses("@playwright/test") {
            known = true;
            add(
                "playwright",
                "Playwright",
                Kind::Playwright,
                format!("{exec} playwright test"),
            );
        }
        // Another runner behind `npm test`: one line, by how it exits. Not
        // the script `npm init` writes, which only says there are none.
        let script = package["scripts"]["test"].as_str().unwrap_or_default();
        if !known && !script.is_empty() && !script.contains("no test specified") {
            let run = match exec {
                "pnpm exec" => "pnpm test",
                "yarn" => "yarn test",
                "bunx" => "bun run test",
                _ => "npm test",
            };
            add("npm", run, Kind::Other, run.into());
        }
    }

    let python_tests = || {
        std::fs::read_dir(dir.join("tests"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py"))
            })
    };
    if has("pytest.ini")
        || has("conftest.py")
        || read("pyproject.toml").contains("pytest")
        || read("setup.cfg").contains("[tool:pytest]")
        || python_tests()
    {
        let python = if has("uv.lock") {
            "uv run python".to_string()
        } else if has("poetry.lock") {
            "poetry run python".to_string()
        } else if has(".venv/bin/python") {
            ".venv/bin/python".to_string()
        } else if has(".venv/Scripts/python.exe") {
            ".venv\\Scripts\\python.exe".to_string()
        } else if cfg!(windows) {
            "python".to_string()
        } else {
            "python3".to_string()
        };
        add(
            "pytest",
            "pytest",
            Kind::Pytest,
            format!("{python} -m pytest"),
        );
    }

    let gradle_files = [
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ];
    if gradle_files.iter().any(|file| has(file)) {
        let gradle = if cfg!(windows) && has("gradlew.bat") {
            "gradlew.bat"
        } else if !cfg!(windows) && has("gradlew") {
            "./gradlew"
        } else {
            "gradle"
        };
        // An Android app's unit tests are a task per build variant.
        let android = ["build.gradle", "build.gradle.kts"]
            .iter()
            .flat_map(|file| [read(file), read(&format!("app/{file}"))])
            .any(|text| text.contains("com.android.") || text.contains("android {"));
        let task = if android { "testDebugUnitTest" } else { "test" };
        add(
            "gradle",
            "Gradle tests",
            Kind::Gradle,
            format!("{gradle} {task}"),
        );
    } else if has("pom.xml") {
        let maven = if cfg!(windows) && has("mvnw.cmd") {
            "mvnw.cmd"
        } else if !cfg!(windows) && has("mvnw") {
            "./mvnw"
        } else {
            "mvn"
        };
        add("maven", "Maven tests", Kind::Maven, format!("{maven} test"));
    }

    if has("go.mod") {
        add("go", "go test", Kind::Go, "go test".into());
    }

    // Runners whose results are not read yet: one line each, by exit code.
    let files_ending = |ending: &str| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().ends_with(ending))
    };
    if files_ending(".sln") || files_ending(".csproj") || files_ending(".fsproj") {
        add("dotnet", "dotnet test", Kind::Other, "dotnet test".into());
    }
    if has("pubspec.yaml") {
        let flutter = read("pubspec.yaml").contains("flutter:");
        let run = if flutter { "flutter test" } else { "dart test" };
        if has("test") {
            add("dart", run, Kind::Other, run.into());
        }
    }
    if has("deno.json") || has("deno.jsonc") {
        add("deno", "deno test", Kind::Other, "deno test".into());
    }
    if has("mix.exs") {
        add("mix", "mix test", Kind::Other, "mix test".into());
    }
    if has("Gemfile") && (has("spec") || has("test")) {
        let run = if has("spec") {
            "bundle exec rspec"
        } else {
            "bundle exec rake test"
        };
        add("ruby", run, Kind::Other, run.into());
    }
    suites
}

/// A suite's id as a folder name.
pub fn folder_name(id: &str) -> String {
    let name: String = id
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    // Two commands that differ only in punctuation keep their own.
    let hash = id.bytes().fold(0x811c9dc5u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x01000193)
    });
    format!("{}-{hash:08x}", name.chars().take(40).collect::<String>())
}

/// `<data dir>/test-runs/<project>/_suites/<suite>`: its last result, and
/// the results file its runner writes.
pub fn suite_dir(project_id: i64, id: &str) -> PathBuf {
    ph_store::paths::data_dir()
        .join("test-runs")
        .join(project_id.to_string())
        .join("_suites")
        .join(folder_name(id))
}

/// What the suite's last run found, if it ever ran.
pub fn last_result(project_id: i64, id: &str) -> Option<SuiteResult> {
    let text = std::fs::read_to_string(suite_dir(project_id, id).join("result.json")).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_result(project_id: i64, id: &str, result: &SuiteResult) -> anyhow::Result<()> {
    let dir = suite_dir(project_id, id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("result.json"), serde_json::to_string(result)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ph-suites-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (file, text) in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        root
    }

    fn case(group: &str, name: &str, file: Option<&str>) -> Case {
        Case {
            group: group.into(),
            name: name.into(),
            outcome: Outcome::Failed,
            millis: None,
            message: None,
            file: file.map(str::to_string),
            line: None,
        }
    }

    #[test]
    fn suites_are_found_in_the_project_and_right_under_it() {
        let root = project(
            "found",
            &[
                ("Cargo.toml", "[workspace]"),
                ("crates/a/Cargo.toml", "[package]"),
                (
                    "web/package.json",
                    r#"{"devDependencies":{"vitest":"^3","@playwright/test":"^1"},"scripts":{"test":"vitest"}}"#,
                ),
                ("web/pnpm-lock.yaml", ""),
                ("api/tests/test_cart.py", ""),
                (
                    "node_modules/x/package.json",
                    r#"{"devDependencies":{"jest":"1"}}"#,
                ),
                (
                    "old/package.json",
                    r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
                ),
                (
                    "tool/package.json",
                    r#"{"scripts":{"test":"node check.js"}}"#,
                ),
            ],
        );
        let suites = detect(&root);
        let found: Vec<(&str, &str, &str)> = suites
            .iter()
            .map(|suite| {
                (
                    suite.id.as_str(),
                    suite.name.as_str(),
                    suite.command.as_str(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (
                    "cargo",
                    "cargo test",
                    "cargo test --workspace --no-fail-fast"
                ),
                ("api/pytest", "api · pytest", "python3 -m pytest"),
                ("tool/npm", "tool · npm test", "npm test"),
                ("web/vitest", "web · Vitest", "pnpm exec vitest run"),
                (
                    "web/playwright",
                    "web · Playwright",
                    "pnpm exec playwright test"
                ),
            ]
        );
        assert_eq!(suites[1].cwd, root.join("api"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_workspaces_parts_are_not_listed_twice() {
        let root = project(
            "covered",
            &[
                ("settings.gradle.kts", ""),
                (
                    "app/build.gradle.kts",
                    "plugins { id(\"com.android.application\") }",
                ),
                ("gradlew", ""),
                ("Cargo.toml", ""),
                ("cli/Cargo.toml", ""),
                ("svc/go.mod", ""),
            ],
        );
        let found: Vec<(String, String)> = detect(&root)
            .into_iter()
            .map(|suite| (suite.id, suite.command))
            .collect();
        let gradle = if cfg!(windows) { "gradle" } else { "./gradlew" };
        assert_eq!(
            found,
            [
                (
                    "cargo".to_string(),
                    "cargo test --workspace --no-fail-fast".to_string()
                ),
                ("gradle".to_string(), format!("{gradle} testDebugUnitTest")),
                ("svc/go".to_string(), "go test".to_string()),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // The words of a command line are quoted as the system's shell reads
    // them: these say how a POSIX one does.
    #[cfg(unix)]
    #[test]
    fn a_suite_is_asked_to_write_its_results_down() {
        let root = Path::new("/p");
        let results = Path::new("/data dir/results.xml");
        let suite = |kind, command: &str| Suite {
            id: "x".into(),
            name: "x".into(),
            kind,
            command: command.into(),
            cwd: root.into(),
            custom: false,
        };
        assert_eq!(
            suite(Kind::Pytest, "python3 -m pytest").command_line(&[], results),
            "python3 -m pytest -o junit_family=xunit1 --junitxml='/data dir/results.xml'"
        );
        assert_eq!(
            suite(Kind::Vitest, "npx vitest run").command_line(&[], results),
            "npx vitest run --reporter=default --reporter=junit --outputFile.junit='/data dir/results.xml'"
        );
        assert_eq!(
            suite(Kind::Go, "go test").command_line(&[], results),
            "go test -json ./..."
        );
        let playwright = suite(Kind::Playwright, "npx playwright test");
        assert_eq!(
            playwright.command_line(&[], results),
            "npx playwright test --reporter=list,junit"
        );
        assert_eq!(
            playwright.environment(results)[0],
            (
                "PLAYWRIGHT_JUNIT_OUTPUT_FILE",
                "/data dir/results.xml".into()
            )
        );
        assert_eq!(
            suite(Kind::Other, "make check").command_line(&[], results),
            "make check"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_part_of_a_suite_is_run_by_name_or_by_file() {
        let results = Path::new("/r.xml");
        let suite = |kind, command: &str| Suite {
            id: "x".into(),
            name: "x".into(),
            kind,
            command: command.into(),
            cwd: "/p".into(),
            custom: false,
        };
        let unit = case("cart", "tests::adds", None);
        let doc = case("cart", "src/lib.rs - add (line 3)", None);
        assert_eq!(
            suite(Kind::Cargo, "cargo test").command_line(&[&unit, &doc], results),
            "cargo test -- --exact tests::adds"
        );
        let method = case(
            "tests.test_cart.TestCart",
            "test_add[1-2]",
            Some("tests/test_cart.py"),
        );
        let function = case("tests.test_cart", "test_empty", Some("tests/test_cart.py"));
        assert_eq!(
            suite(Kind::Pytest, "pytest").command_line(&[&method, &function], results),
            "pytest 'tests/test_cart.py::TestCart::test_add[1-2]' tests/test_cart.py::test_empty -o junit_family=xunit1 --junitxml=/r.xml"
        );
        let a = case("src/cart.test.ts", "cart > adds", None);
        let b = case("src/cart.test.ts", "cart > removes", None);
        assert_eq!(
            suite(Kind::Jest, "npx jest").command_line(&[&a, &b], results),
            "npx jest src/cart.test.ts --json --outputFile=/r.xml"
        );
        let jvm = case("com.app.CartTest", "adds()", None);
        let shown = case("com.app.CartTest", "adds two items", None);
        assert_eq!(
            suite(Kind::Gradle, "./gradlew test").command_line(&[&jvm, &shown], results),
            "./gradlew test --tests com.app.CartTest.adds --tests com.app.CartTest"
        );
        assert_eq!(
            suite(Kind::Maven, "mvn test").command_line(&[&jvm], results),
            "mvn test -Dtest='com.app.CartTest#adds'"
        );
        let sub = case("svc", "TestCart/adds", None);
        assert_eq!(
            suite(Kind::Go, "go test").command_line(&[&sub], results),
            "go test -json -run '^(TestCart)$' ./..."
        );
        // A test's name never ends the quotes it is put in.
        assert_eq!(quoted("a'; rm -rf $HOME"), "'a; rm -rf $HOME'");
    }

    #[test]
    fn a_result_says_whether_the_suite_passed_or_broke() {
        let mut result = SuiteResult {
            started_at: 0,
            millis: 0,
            exit: Some(0),
            stopped: false,
            cases: vec![case("g", "a", None)],
            output: String::new(),
            partial: false,
        };
        assert!(!result.passed() && !result.broke(), "a test failed");
        result.cases[0].outcome = Outcome::Passed;
        assert!(result.passed());
        result.exit = Some(101);
        assert!(result.broke(), "it failed and no test says why");
        result.stopped = true;
        assert!(!result.passed() && !result.broke());
        assert_ne!(folder_name("own/make a"), folder_name("own/make-a"));
    }
}
