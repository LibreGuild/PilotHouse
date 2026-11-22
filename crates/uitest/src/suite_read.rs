//! Reading what a suite's run left behind as one [`Case`] per test: a JUnit
//! file (pytest, Vitest, Playwright, Gradle, Maven), Jest's JSON, the lines
//! `cargo test` prints, the events of `go test -json`.
//!
//! Each reader takes text (a file's, or the command's output) and is told
//! nothing else, so they are tested on what the real runners write.

use std::path::{Path, PathBuf};

use crate::suite::{Case, Kind, Outcome, Suite};

/// A failure's text kept per test, at most: a runner can print a whole
/// page's HTML into one.
const MESSAGE_MAX: usize = 6000;

/// The cases of a run of `suite`: from `results` (the file it was asked to
/// write) or `output` (what it printed), whichever its runner uses.
pub fn read(suite: &Suite, output: &str, results: &Path) -> Vec<Case> {
    let file = || std::fs::read_to_string(results).unwrap_or_default();
    let mut cases = match suite.kind {
        Kind::Cargo => libtest(output),
        Kind::Go => go_json(output),
        Kind::Pytest | Kind::Vitest | Kind::Playwright => junit(&file()),
        Kind::Jest => jest_json(&file(), &suite.cwd),
        Kind::Gradle => junit_files(&result_folders(
            &suite.cwd,
            &["build", "test-results", suite.gradle_task()],
        )),
        Kind::Maven => junit_files(&result_folders(&suite.cwd, &["target", "surefire-reports"])),
        Kind::Other => Vec::new(),
    };
    for case in &mut cases {
        if let Some(message) = &mut case.message
            && message.len() > MESSAGE_MAX
        {
            let mut end = MESSAGE_MAX;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            message.push_str("\n…");
        }
        // Where it failed, as its message says: the line that did not
        // hold, which is nearer than where the test starts (what a runner
        // says of a test as a whole).
        if case.outcome == Outcome::Failed
            && let Some(message) = &case.message
            && let Some((file, line)) = place_in(message, &suite.cwd)
        {
            case.file = Some(file);
            case.line = Some(line);
        }
    }
    cases
}

/// The folders ending in `tail` (`build/test-results/test`) under `root`:
/// one per module of a build. Dependencies and hidden folders are skipped.
fn result_folders(root: &Path, tail: &[&str]) -> Vec<PathBuf> {
    fn walk(dir: &Path, tail: &[&str], depth: usize, found: &mut Vec<PathBuf>) {
        let ends = tail
            .iter()
            .fold(dir.to_path_buf(), |path, part| path.join(part));
        if ends.is_dir() {
            found.push(ends);
        }
        if depth == 0 {
            return;
        }
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let skip = name.starts_with('.')
                || matches!(name.as_str(), "node_modules" | "build" | "target" | "src");
            if !skip && path.is_dir() {
                walk(&path, tail, depth - 1, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, tail, 3, &mut found);
    found.sort();
    found
}

fn junit_files(folders: &[PathBuf]) -> Vec<Case> {
    let mut files: Vec<PathBuf> = folders
        .iter()
        .flat_map(|folder| std::fs::read_dir(folder).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "xml"))
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|file| std::fs::read_to_string(file).ok())
        .flat_map(|text| junit(&text))
        .collect()
}

/// A JUnit XML file: `<testcase>`s in (nested) `<testsuite>`s, each failed
/// when it has a `<failure>` or `<error>` in it, skipped with `<skipped>`.
pub fn junit(xml: &str) -> Vec<Case> {
    let Ok(document) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let mut cases = Vec::new();
    for node in document
        .descendants()
        .filter(|node| node.has_tag_name("testcase"))
    {
        let attribute = |name: &str| node.attribute(name).unwrap_or_default().to_string();
        let child = |names: &[&str]| {
            node.children()
                .find(|child| names.iter().any(|name| child.has_tag_name(*name)))
        };
        let failure = child(&["failure", "error"]);
        let outcome = if failure.is_some() {
            Outcome::Failed
        } else if child(&["skipped"]).is_some() {
            Outcome::Skipped
        } else {
            Outcome::Passed
        };
        let message = failure.map(|failure| {
            // The short message, then the trace when it says more.
            let short = failure.attribute("message").unwrap_or_default().trim();
            let long = failure.text().unwrap_or_default().trim();
            if long.is_empty() || long.starts_with(short) {
                if long.is_empty() { short } else { long }.to_string()
            } else {
                format!("{short}\n\n{long}")
            }
        });
        // The class, else the suite the case is in (a file, for Vitest).
        let mut group = attribute("classname");
        if group.is_empty() {
            group = node
                .ancestors()
                .find(|parent| parent.has_tag_name("testsuite"))
                .and_then(|suite| suite.attribute("name"))
                .unwrap_or_default()
                .to_string();
        }
        let file = node
            .attribute("file")
            .or_else(|| {
                // Playwright and Vitest name the file as the class.
                let looks_like_file = group.contains('/') || group.contains('\\');
                looks_like_file.then_some(group.as_str())
            })
            .map(str::to_string);
        cases.push(Case {
            name: attribute("name"),
            outcome,
            millis: node
                .attribute("time")
                .and_then(|time| time.parse::<f64>().ok())
                .map(|seconds| (seconds * 1000.0).round() as u64),
            message: message.filter(|message| !message.is_empty()),
            line: node
                .attribute("line")
                .and_then(|line| line.parse::<u32>().ok())
                // pytest counts lines from 0.
                .map(|line| line + 1),
            file,
            group,
        });
    }
    cases
}

/// What Jest writes with `--json`: test files, each with its assertions.
pub fn jest_json(text: &str, cwd: &Path) -> Vec<Case> {
    let Ok(report) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let mut cases = Vec::new();
    for file in report["testResults"].as_array().into_iter().flatten() {
        let path = file["name"].as_str().unwrap_or_default();
        let relative = Path::new(path)
            .strip_prefix(cwd)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string());
        let assertions = file["assertionResults"].as_array();
        // A file that could not be run at all has no assertions: it is the
        // failure.
        if assertions.is_none_or(|assertions| assertions.is_empty()) && file["status"] == "failed" {
            cases.push(Case {
                group: relative.clone(),
                name: "The file could not be run".into(),
                outcome: Outcome::Failed,
                millis: None,
                message: file["message"].as_str().map(str::to_string),
                file: Some(relative.clone()),
                line: None,
            });
        }
        for assertion in assertions.into_iter().flatten() {
            let titles: Vec<&str> = assertion["ancestorTitles"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|title| title.as_str())
                .chain(assertion["title"].as_str())
                .collect();
            let messages: Vec<&str> = assertion["failureMessages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|message| message.as_str())
                .collect();
            cases.push(Case {
                group: relative.clone(),
                name: titles.join(" > "),
                outcome: match assertion["status"].as_str() {
                    Some("passed") => Outcome::Passed,
                    Some("failed") => Outcome::Failed,
                    _ => Outcome::Skipped,
                },
                millis: assertion["duration"].as_f64().map(|millis| millis as u64),
                message: (!messages.is_empty()).then(|| messages.join("\n\n")),
                file: Some(relative.clone()),
                line: assertion["location"]["line"]
                    .as_u64()
                    .map(|line| line as u32),
            });
        }
    }
    cases
}

/// What `cargo test` prints: a line per test under the target it belongs
/// to, then what each failed test wrote.
pub fn libtest(output: &str) -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    let mut group = String::new();
    // The failed test whose output is being read.
    let mut telling: Option<String> = None;
    let mut told: Vec<(String, String)> = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim_start();
        // `Running unittests src/lib.rs (target/debug/deps/cart-1a2b3c)`,
        // `Running tests/api.rs (…)`, `Doc-tests cart`.
        if let Some(rest) = trimmed.strip_prefix("Running ") {
            telling = None;
            let source = rest.split(" (").next().unwrap_or(rest);
            let binary = rest
                .rsplit_once('(')
                .map(|(_, path)| path.trim_end_matches(')'))
                .and_then(|path| path.rsplit(['/', '\\']).next())
                .and_then(|name| name.rsplit_once('-').map(|(name, _)| name))
                .unwrap_or_default();
            group = match source.strip_prefix("unittests ") {
                Some(_) => binary.to_string(),
                // An integration test's file says more than its binary.
                None if !binary.is_empty() => format!("{binary} ({source})"),
                None => source.to_string(),
            };
            continue;
        }
        if let Some(name) = trimmed.strip_prefix("Doc-tests ") {
            telling = None;
            group = format!("{name} (docs)");
            continue;
        }
        if let Some(rest) = line.strip_prefix("test ")
            && let Some((name, outcome)) = rest.rsplit_once(" ... ")
        {
            let outcome = match outcome.split_whitespace().next() {
                Some("ok") => Outcome::Passed,
                Some("FAILED") => Outcome::Failed,
                Some("ignored") => Outcome::Skipped,
                // `test result: …`, or a line a test printed that looks alike.
                _ => continue,
            };
            // `name - should panic` and such say how it was run.
            let name = name.split(" - should panic").next().unwrap_or(name);
            cases.push(Case {
                group: group.clone(),
                name: name.to_string(),
                outcome,
                millis: None,
                message: None,
                file: None,
                line: None,
            });
            continue;
        }
        // `---- tests::adds stdout ----` opens what a failed test wrote; the
        // list of failures (or the next test's) closes it.
        if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|rest| rest.strip_suffix(" ----"))
        {
            let name = name.rsplit_once(' ').map_or(name, |(name, _)| name);
            telling = Some(name.to_string());
            told.push((name.to_string(), String::new()));
            continue;
        }
        if line == "failures:" || line.starts_with("test result:") {
            telling = None;
            continue;
        }
        if telling.is_some()
            && let Some((_, text)) = told.last_mut()
        {
            text.push_str(line);
            text.push('\n');
        }
    }
    for (name, text) in told {
        // The newest case of that name: the same name can be in two crates,
        // and each one's output follows its own run.
        if let Some(case) = cases
            .iter_mut()
            .rev()
            .find(|case| case.name == name && case.message.is_none())
        {
            let text = text.trim();
            // `thread 'tests::adds' panicked at src/lib.rs:12:9:`.
            if let Some((file, line)) = text
                .split("panicked at ")
                .nth(1)
                .and_then(|rest| rest.lines().next())
                .and_then(file_and_line)
            {
                case.file = Some(file);
                case.line = Some(line);
            }
            case.message = Some(text.to_string());
        }
    }
    cases
}

/// `src/lib.rs:12:9:` → (`src/lib.rs`, 12).
fn file_and_line(text: &str) -> Option<(String, u32)> {
    let mut parts = text.trim().trim_end_matches(':').split(':');
    let file = parts.next()?.trim();
    // A drive letter is a part of the file's name, not a line.
    let (file, line) = match parts.next()? {
        rest if file.len() == 1 && rest.starts_with(['\\', '/']) => {
            (format!("{file}:{rest}"), parts.next()?)
        }
        line => (file.to_string(), line),
    };
    Some((file, line.trim().parse().ok()?))
}

/// The events of `go test -json`: a line of JSON per event, with the
/// package, the test (none for the package's own events) and what happened.
pub fn go_json(output: &str) -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    // What each test printed, and each package (which says why it did not
    // build).
    let mut printed: std::collections::HashMap<(String, String), String> = Default::default();
    for line in output.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let package = event["Package"].as_str().unwrap_or_default().to_string();
        let test = event["Test"].as_str().unwrap_or_default().to_string();
        let action = event["Action"].as_str().unwrap_or_default();
        if action == "output" {
            printed
                .entry((package, test))
                .or_default()
                .push_str(event["Output"].as_str().unwrap_or_default());
            continue;
        }
        let outcome = match action {
            "pass" => Outcome::Passed,
            "fail" => Outcome::Failed,
            "skip" => Outcome::Skipped,
            _ => continue,
        };
        let text = printed.remove(&(package.clone(), test.clone()));
        if test.is_empty() {
            // A package fails as a whole when it does not build, or a test
            // of it failed (then that test says so already).
            let said = cases
                .iter()
                .any(|case| case.group == package && case.outcome == Outcome::Failed);
            if outcome == Outcome::Failed && !said {
                cases.push(Case {
                    group: package,
                    name: "The package could not be tested".into(),
                    outcome,
                    millis: None,
                    message: text.map(|text| text.trim().to_string()),
                    file: None,
                    line: None,
                });
            }
            continue;
        }
        let message = text
            .map(|text| {
                // Without the lines `go test` frames each test with.
                text.lines()
                    .filter(|line| {
                        let line = line.trim_start();
                        ![
                            "=== RUN",
                            "=== PAUSE",
                            "=== CONT",
                            "--- FAIL",
                            "--- PASS",
                            "--- SKIP",
                        ]
                        .iter()
                        .any(|frame| line.starts_with(frame))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string()
            })
            .filter(|text| !text.is_empty() && outcome == Outcome::Failed);
        // `    cart_test.go:12: got 1, want 2`.
        let place = message
            .as_deref()
            .and_then(|message| message.lines().find_map(file_and_line))
            .filter(|(file, _)| file.ends_with(".go"));
        cases.push(Case {
            group: package,
            name: test,
            outcome,
            millis: event["Elapsed"]
                .as_f64()
                .map(|seconds| (seconds * 1000.0).round() as u64),
            message,
            file: place.as_ref().map(|(file, _)| file.clone()),
            line: place.map(|(_, line)| line),
        });
    }
    cases
}

/// File endings that are sources a failure can point into.
const SOURCES: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "svelte", "kt", "java", "go", "rb",
    "swift", "cs", "dart", "php",
];

/// The first place (`file:line`) named in `message` that is a file of the
/// project: a stack trace's own frame, past the runner's and the
/// libraries'. A JVM trace names only the file (`(CartTest.kt:12)`), which
/// is then looked for under `cwd`.
fn place_in(message: &str, cwd: &Path) -> Option<(String, u32)> {
    let is_name = |c: char| c.is_alphanumeric() || "_-./\\@+[]$".contains(c);
    for (at, _) in message.match_indices(':') {
        let before = &message[..at];
        let start = before.rfind(|c: char| !is_name(c)).map_or(0, |start| {
            start + before[start..].chars().next().map_or(1, char::len_utf8)
        });
        let file = &before[start..];
        let digits: String = message[at + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let Ok(line) = digits.parse::<u32>() else {
            continue;
        };
        let known = file
            .rsplit_once('.')
            .is_some_and(|(_, ending)| SOURCES.contains(&ending));
        if !known || file.contains("node_modules") {
            continue;
        }
        let path = Path::new(file);
        if path.is_absolute() {
            // Said from the project folder, like the others.
            if let Ok(inside) = path.strip_prefix(cwd)
                && path.is_file()
            {
                return Some((inside.to_string_lossy().replace('\\', "/"), line));
            }
        } else if cwd.join(path).is_file() {
            return Some((file.to_string(), line));
        } else if !file.contains(['/', '\\'])
            && let Some(found) = find_file(cwd, file)
        {
            return Some((found, line));
        }
    }
    None
}

/// A file named `name` somewhere under `root` (not in what is built or
/// fetched), relative to it. Gives up after a few thousand entries.
fn find_file(root: &Path, name: &str) -> Option<String> {
    let mut left = 5000usize;
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            left = left.checked_sub(1)?;
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                let skip = file_name.starts_with('.')
                    || matches!(
                        file_name.as_str(),
                        "node_modules" | "build" | "target" | "dist" | "out"
                    );
                if !skip {
                    folders.push(path);
                }
            } else if file_name == name {
                return path
                    .strip_prefix(root)
                    .ok()
                    .map(|path| path.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargos_lines_become_cases_under_their_crate() {
        let output = "\
   Compiling cart v0.1.0 (/p)
     Running unittests src/lib.rs (target/debug/deps/cart-1a2b3c4d5e6f7a8b)

running 3 tests
test tests::adds ... ok
test tests::later ... ignored
test tests::totals ... FAILED

failures:

---- tests::totals stdout ----

thread 'tests::totals' (123) panicked at src/lib.rs:42:9:
assertion `left == right` failed
  left: 3
 right: 4
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    tests::totals

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/api.rs (target/debug/deps/api-0011223344556677)

running 1 test
test serves ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests cart

running 1 test
test src/lib.rs - add (line 3) ... ok
";
        let cases = libtest(output);
        let seen: Vec<(&str, &str, Outcome)> = cases
            .iter()
            .map(|case| (case.group.as_str(), case.name.as_str(), case.outcome))
            .collect();
        assert_eq!(
            seen,
            [
                ("cart", "tests::adds", Outcome::Passed),
                ("cart", "tests::later", Outcome::Skipped),
                ("cart", "tests::totals", Outcome::Failed),
                ("api (tests/api.rs)", "serves", Outcome::Passed),
                ("cart (docs)", "src/lib.rs - add (line 3)", Outcome::Passed),
            ]
        );
        let failed = &cases[2];
        assert_eq!(
            (failed.file.as_deref(), failed.line),
            (Some("src/lib.rs"), Some(42))
        );
        let message = failed.message.as_deref().unwrap();
        assert!(
            message.contains("left: 3") && !message.contains("failures:"),
            "{message}"
        );
        assert!(cases[0].message.is_none());
    }

    #[test]
    fn a_junit_file_becomes_cases() {
        // As pytest writes it (xunit1): the file, and lines from 0.
        let pytest = r#"<?xml version="1.0" encoding="utf-8"?><testsuites><testsuite name="pytest" errors="0" failures="1" skipped="1" tests="3" time="0.021">
<testcase classname="tests.test_cart.TestCart" name="test_add" file="tests/test_cart.py" line="4" time="0.001"></testcase>
<testcase classname="tests.test_cart" name="test_total" file="tests/test_cart.py" line="9" time="0.002"><failure message="assert 3 == 4">def test_total():
&gt;       assert total([1, 2]) == 4
E       assert 3 == 4

tests/test_cart.py:11: AssertionError</failure></testcase>
<testcase classname="tests.test_cart" name="test_later" file="tests/test_cart.py" line="13" time="0.000"><skipped type="pytest.skip" message="not yet">tests/test_cart.py:14: not yet</skipped></testcase>
</testsuite></testsuites>"#;
        let cases = junit(pytest);
        assert_eq!(cases.len(), 3);
        assert_eq!(
            (
                cases[0].group.as_str(),
                cases[0].name.as_str(),
                cases[0].outcome
            ),
            ("tests.test_cart.TestCart", "test_add", Outcome::Passed)
        );
        assert_eq!(cases[0].millis, Some(1));
        let failed = &cases[1];
        assert_eq!(failed.outcome, Outcome::Failed);
        assert_eq!(
            (failed.file.as_deref(), failed.line),
            (Some("tests/test_cart.py"), Some(10))
        );
        let message = failed.message.as_deref().unwrap();
        assert!(
            message.starts_with("assert 3 == 4\n\ndef test_total()"),
            "{message}"
        );
        assert!(
            message.contains(">       assert total"),
            "entities are read"
        );
        assert_eq!(cases[2].outcome, Outcome::Skipped);

        // As Vitest and Playwright write it: the file is the class.
        let vitest = r#"<testsuites name="vitest tests"><testsuite name="src/cart.test.ts" tests="2">
<testcase classname="src/cart.test.ts" name="cart &gt; adds" time="0.0012"></testcase>
<testcase classname="src/cart.test.ts" name="cart &gt; totals" time="0.004"><failure message="expected 3 to be 4" type="AssertionError"><![CDATA[AssertionError: expected 3 to be 4
 ❯ src/cart.test.ts:9:22]]></failure></testcase>
</testsuite></testsuites>"#;
        let cases = junit(vitest);
        assert_eq!(cases[1].name, "cart > totals");
        assert_eq!(cases[1].file.as_deref(), Some("src/cart.test.ts"));
        assert!(
            cases[1]
                .message
                .as_deref()
                .unwrap()
                .contains("❯ src/cart.test.ts:9:22")
        );

        // As Gradle writes it: no class file, an error in place of a failure.
        let gradle = r#"<testsuite name="com.app.CartTest" tests="1"><testcase name="adds()" classname="com.app.CartTest" time="0.01"><error message="boom" type="java.lang.IllegalStateException">java.lang.IllegalStateException: boom
	at com.app.CartTest.adds(CartTest.kt:12)</error></testcase></testsuite>"#;
        let cases = junit(gradle);
        assert_eq!(
            (cases[0].outcome, cases[0].file.as_deref()),
            (Outcome::Failed, None)
        );
        assert!(junit("not xml").is_empty());
    }

    #[test]
    fn jests_json_becomes_cases() {
        let json = r#"{"numFailedTests":1,"testResults":[
          {"name":"/p/src/cart.test.js","status":"failed","message":"","assertionResults":[
            {"ancestorTitles":["cart"],"title":"adds","fullName":"cart adds","status":"passed","duration":2,"failureMessages":[],"location":null},
            {"ancestorTitles":["cart"],"title":"totals","fullName":"cart totals","status":"failed","duration":5,"failureMessages":["Error: expect(received).toBe(expected)\n    at Object.<anonymous> (/p/src/cart.test.js:9:22)"],"location":{"line":8,"column":3}},
            {"ancestorTitles":[],"title":"later","status":"pending","failureMessages":[]}]},
          {"name":"/p/src/broken.test.js","status":"failed","message":"SyntaxError: Unexpected token","assertionResults":[]}]}"#;
        let cases = jest_json(json, Path::new("/p"));
        let seen: Vec<(&str, &str, Outcome)> = cases
            .iter()
            .map(|case| (case.group.as_str(), case.name.as_str(), case.outcome))
            .collect();
        assert_eq!(
            seen,
            [
                ("src/cart.test.js", "cart > adds", Outcome::Passed),
                ("src/cart.test.js", "cart > totals", Outcome::Failed),
                ("src/cart.test.js", "later", Outcome::Skipped),
                (
                    "src/broken.test.js",
                    "The file could not be run",
                    Outcome::Failed
                ),
            ]
        );
        assert_eq!(cases[1].line, Some(8));
        assert_eq!(
            cases[3].message.as_deref(),
            Some("SyntaxError: Unexpected token")
        );
        assert!(jest_json("", Path::new("/p")).is_empty());
    }

    #[test]
    fn gos_events_become_cases() {
        let output = r##"{"Action":"start","Package":"example.com/cart"}
{"Action":"run","Package":"example.com/cart","Test":"TestAdds"}
{"Action":"output","Package":"example.com/cart","Test":"TestAdds","Output":"=== RUN   TestAdds\n"}
{"Action":"output","Package":"example.com/cart","Test":"TestAdds","Output":"--- PASS: TestAdds (0.00s)\n"}
{"Action":"pass","Package":"example.com/cart","Test":"TestAdds","Elapsed":0}
{"Action":"run","Package":"example.com/cart","Test":"TestTotals"}
{"Action":"output","Package":"example.com/cart","Test":"TestTotals","Output":"=== RUN   TestTotals\n"}
{"Action":"output","Package":"example.com/cart","Test":"TestTotals","Output":"    cart_test.go:12: got 3, want 4\n"}
{"Action":"output","Package":"example.com/cart","Test":"TestTotals","Output":"--- FAIL: TestTotals (0.01s)\n"}
{"Action":"fail","Package":"example.com/cart","Test":"TestTotals","Elapsed":0.01}
{"Action":"output","Package":"example.com/cart","Output":"FAIL\n"}
{"Action":"fail","Package":"example.com/cart","Elapsed":0.2}
{"Action":"output","Package":"example.com/api","Output":"# example.com/api\n./api.go:3:1: syntax error\n"}
{"Action":"fail","Package":"example.com/api","Elapsed":0}
not json at all
"##;
        let cases = go_json(output);
        let seen: Vec<(&str, &str, Outcome)> = cases
            .iter()
            .map(|case| (case.group.as_str(), case.name.as_str(), case.outcome))
            .collect();
        assert_eq!(
            seen,
            [
                ("example.com/cart", "TestAdds", Outcome::Passed),
                ("example.com/cart", "TestTotals", Outcome::Failed),
                (
                    "example.com/api",
                    "The package could not be tested",
                    Outcome::Failed
                ),
            ]
        );
        assert_eq!(
            cases[1].message.as_deref(),
            Some("cart_test.go:12: got 3, want 4")
        );
        assert_eq!(
            (cases[1].file.as_deref(), cases[1].line),
            (Some("cart_test.go"), Some(12))
        );
        assert_eq!(cases[1].millis, Some(10));
        assert!(
            cases[2]
                .message
                .as_deref()
                .unwrap()
                .contains("syntax error")
        );
    }

    #[test]
    fn a_failure_points_into_the_projects_own_file() {
        let root = std::env::temp_dir().join(format!("ph-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for file in [
            "src/cart.test.ts",
            "app/src/test/kotlin/com/app/CartTest.kt",
        ] {
            std::fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
            std::fs::write(root.join(file), "").unwrap();
        }
        // Past the library's frame, to the project's.
        let trace = "AssertionError: expected 3 to be 4\n ❯ node_modules/vitest/dist/x.js:10:2\n ❯ src/cart.test.ts:9:22";
        assert_eq!(place_in(trace, &root), Some(("src/cart.test.ts".into(), 9)));
        // A JVM trace names the file alone: it is found in the project.
        let jvm = "java.lang.IllegalStateException: boom\n\tat org.junit.Assert.fail(Assert.java:89)\n\tat com.app.CartTest.adds(CartTest.kt:12)";
        assert_eq!(
            place_in(jvm, &root),
            Some(("app/src/test/kotlin/com/app/CartTest.kt".into(), 12))
        );
        assert_eq!(place_in("at 10:30 nothing ran: see x.md:3", &root), None);
        assert_eq!(
            file_and_line("src/lib.rs:12:9:"),
            Some(("src/lib.rs".into(), 12))
        );
        assert_eq!(
            file_and_line("C:\\p\\src\\lib.rs:12:9"),
            Some(("C:\\p\\src\\lib.rs".into(), 12))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
