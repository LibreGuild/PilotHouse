//! A UI test: what it checks, in plain words, and the steps that do it.
//!
//! Tests are JSON files in the project's `.pilothouse/tests/`, so they are
//! reviewed and shared with the code like any other file.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Where tests live, under the project root.
pub const FOLDER: &str = ".pilothouse/tests";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestCase {
    pub name: String,
    /// The scenario in plain words: what the agent explores and records.
    pub description: String,
    /// Where it starts: a URL for the browser, or an Android app package.
    pub target: Target,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Web { url: String },
    Android { package: String },
}

/// One recorded step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Step {
    Open {
        url: String,
    },
    Click {
        selector: String,
    },
    ClickAt {
        x: f64,
        y: f64,
    },
    Type {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        submit: bool,
    },
    Press {
        key: String,
    },
    Scroll {
        dy: f64,
    },
    /// Android.
    Launch {
        package: String,
    },
    Tap {
        x: i64,
        y: i64,
    },
    Swipe {
        x1: i64,
        y1: i64,
        x2: i64,
        y2: i64,
    },
    TypeText {
        text: String,
    },
    Key {
        key: String,
    },
    /// Checks: the screen shows (or does not show) `text`, or the page's URL
    /// contains it. Each waits up to a few seconds for the screen to settle.
    ExpectText {
        text: String,
    },
    ExpectNoText {
        text: String,
    },
    ExpectUrl {
        contains: String,
    },
}

impl Step {
    /// What the step does, for a report.
    pub fn describe(&self) -> String {
        match self {
            Step::Open { url } => format!("Open {url}"),
            Step::Click { selector } => format!("Click {selector}"),
            Step::ClickAt { x, y } => format!("Click at {x:.0}, {y:.0}"),
            Step::Type {
                text,
                selector,
                submit,
            } => format!(
                "Type “{text}”{}{}",
                selector
                    .as_ref()
                    .map(|s| format!(" into {s}"))
                    .unwrap_or_default(),
                if *submit { " and submit" } else { "" }
            ),
            Step::Press { key } => format!("Press {key}"),
            Step::Scroll { dy } => format!("Scroll {dy:.0}"),
            Step::Launch { package } => format!("Launch {package}"),
            Step::Tap { x, y } => format!("Tap {x}, {y}"),
            Step::Swipe { x1, y1, x2, y2 } => format!("Swipe {x1},{y1} → {x2},{y2}"),
            Step::TypeText { text } => format!("Type “{text}”"),
            Step::Key { key } => format!("Key {key}"),
            Step::ExpectText { text } => format!("Expect “{text}”"),
            Step::ExpectNoText { text } => format!("Expect no “{text}”"),
            Step::ExpectUrl { contains } => format!("Expect the URL to contain “{contains}”"),
        }
    }

    pub fn is_check(&self) -> bool {
        matches!(
            self,
            Step::ExpectText { .. } | Step::ExpectNoText { .. } | Step::ExpectUrl { .. }
        )
    }

    /// The control method and parameters that perform an action step;
    /// `None` for checks (the runner reads the screen for those).
    pub fn call(&self, project_id: i64, serial: Option<&str>) -> Option<(&'static str, Value)> {
        let web = |method, mut params: Value| {
            params["project_id"] = json!(project_id);
            Some((method, params))
        };
        let device = |method, mut params: Value| {
            params["serial"] = json!(serial);
            Some((method, params))
        };
        match self {
            Step::Open { url } => web("browser.open", json!({ "url": url })),
            Step::Click { selector } => web("browser.click", json!({ "selector": selector })),
            Step::ClickAt { x, y } => web("browser.click", json!({ "x": x, "y": y })),
            Step::Type {
                text,
                selector,
                submit,
            } => web(
                "browser.type",
                json!({ "text": text, "selector": selector, "submit": submit }),
            ),
            Step::Press { key } => web("browser.press", json!({ "key": key })),
            Step::Scroll { dy } => web("browser.scroll", json!({ "dy": dy })),
            Step::Launch { package } => device("device.launch", json!({ "package": package })),
            Step::Tap { x, y } => device("device.tap", json!({ "x": x, "y": y })),
            Step::Swipe { x1, y1, x2, y2 } => device(
                "device.swipe",
                json!({ "x1": x1, "y1": y1, "x2": x2, "y2": y2 }),
            ),
            Step::TypeText { text } => device("device.type", json!({ "text": text })),
            Step::Key { key } => device("device.key", json!({ "key": key })),
            Step::ExpectText { .. } | Step::ExpectNoText { .. } | Step::ExpectUrl { .. } => None,
        }
    }
}

impl TestCase {
    pub fn is_android(&self) -> bool {
        matches!(self.target, Target::Android { .. })
    }
}

/// `Sign in works` → `sign-in-works`, `ログイン` → `ログイン`. A test's
/// baselines and reports are kept under it, so names in any script keep
/// theirs apart; one with no letters or digits at all (`🎉`) is named by a
/// hash of it.
pub fn slug(name: &str) -> String {
    let slug = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if !slug.is_empty() || name.trim().is_empty() {
        return slug;
    }
    // FNV-1a: the same on every computer and Rust version.
    let hash = name.bytes().fold(0x811c9dc5u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x01000193)
    });
    format!("test-{hash:08x}")
}

/// The project's tests, by name, each with its file.
pub fn list(root: &Path) -> Vec<(PathBuf, TestCase)> {
    let mut tests: Vec<_> = std::fs::read_dir(root.join(FOLDER))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| Some((path.clone(), load(&path).ok()?)))
        .collect();
    tests.sort_by_key(|(_, test)| test.name.to_lowercase());
    tests
}

pub fn load(path: &Path) -> Result<TestCase> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not a test", path.display()))
}

pub fn save(path: &Path, test: &TestCase) -> Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(test)? + "\n")
        .with_context(|| format!("writing {}", path.display()))
}

/// Makes a new test file; refuses a name already taken.
pub fn create(root: &Path, test: &TestCase) -> Result<PathBuf> {
    let slug = slug(&test.name);
    if slug.is_empty() {
        bail!("Give the test a name.");
    }
    let path = root.join(FOLDER).join(format!("{slug}.json"));
    if path.exists() {
        bail!("There is already a test named “{}”.", test.name);
    }
    save(&path, test)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_in_any_script_keep_their_own_folder() {
        assert_eq!(slug("Sign in works!"), "sign-in-works");
        assert_eq!(slug("ログイン"), "ログイン");
        assert_ne!(slug("ログイン"), slug("検索"));
        assert_ne!(slug("🎉"), slug("🚀"));
        assert!(slug("🎉").starts_with("test-"));
        assert_eq!(slug("  "), "");
    }

    #[test]
    fn steps_round_trip_as_readable_json() {
        let step = Step::Type {
            text: "ada".into(),
            selector: Some("#user".into()),
            submit: false,
        };
        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(
            json,
            json!({ "do": "type", "text": "ada", "selector": "#user" })
        );
        assert_eq!(serde_json::from_value::<Step>(json).unwrap(), step);
        assert_eq!(step.describe(), "Type “ada” into #user");
    }

    #[test]
    fn actions_map_to_control_calls() {
        let (method, params) = Step::Click {
            selector: "#go".into(),
        }
        .call(3, None)
        .unwrap();
        assert_eq!(
            (
                method,
                params["project_id"].as_i64(),
                params["selector"].as_str()
            ),
            ("browser.click", Some(3), Some("#go"))
        );
        let (method, params) = Step::Tap { x: 10, y: 20 }
            .call(3, Some("emulator-5554"))
            .unwrap();
        assert_eq!(
            (method, params["serial"].as_str()),
            ("device.tap", Some("emulator-5554"))
        );
        assert!(
            Step::ExpectText { text: "Hi".into() }
                .call(3, None)
                .is_none()
        );
    }

    #[test]
    fn tests_are_files_in_the_project() {
        let root = std::env::temp_dir().join(format!("ph-uitest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let test = TestCase {
            name: "Sign in works!".into(),
            description: "Sign in with the demo account".into(),
            target: Target::Web {
                url: "http://localhost:3000".into(),
            },
            steps: vec![Step::Open {
                url: "http://localhost:3000".into(),
            }],
        };
        let path = create(&root, &test).unwrap();
        assert!(path.ends_with(".pilothouse/tests/sign-in-works.json"));
        assert!(create(&root, &test).is_err(), "names are unique");
        assert_eq!(list(&root), vec![(path, test)]);
    }
}
