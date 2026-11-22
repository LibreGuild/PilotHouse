//! A UI test: what it checks, in plain words, and the steps that do it.
//!
//! Tests are JSON files in the project's `.pilothouse/tests/`, so they are
//! reviewed and shared with the code like any other file.
//!
//! A step names what it acts on the way a person would: a page's element by
//! what it is and what it says (`"role": "button", "name": "Sign in"`), a
//! phone's by its words or id. Where an element stands (a CSS path, a point
//! on the screen) is kept beside that and only used when there is no name
//! to go by: a place moves when a row is added above it, and differs from
//! phone to phone.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Where tests live, under the project root.
pub const FOLDER: &str = ".pilothouse/tests";

/// How long a step waits for its element to show and be usable.
pub const ELEMENT_WAIT_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestCase {
    pub name: String,
    /// The scenario in plain words: what the agent explores and records.
    pub description: String,
    /// Where it starts: a URL for the browser, or an Android app package.
    pub target: Target,
    /// Whether a run starts with what the site (or the app) keeps erased:
    /// see [`TestCase::starts_clean`] for what it is when not said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean: Option<bool>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    Web { url: String },
    Android { package: String },
}

/// An element of a page, as a step names it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Element {
    /// What it is: `button`, `link`, `textbox`, `checkbox`…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// What it says: the words on a button, a field's label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Words of the row, card or section around it, when several elements
    /// are named the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<String>,
    /// Which of them (from 0), when no words tell them apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nth: Option<usize>,
    /// Its CSS selector when recorded: what finds it when it has no name,
    /// or has a hook made for tests (`data-testid`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

impl Element {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.selector.is_none()
    }

    /// From what a browser call says it acted on (`element` in its result).
    pub fn from_json(value: &Value) -> Option<Self> {
        let element: Self = serde_json::from_value(value.clone()).ok()?;
        (!element.is_empty()).then_some(element)
    }

    /// “Sign in” button, “Add” button by “Coffee”, `#go`.
    fn say(&self) -> String {
        let Some(name) = &self.name else {
            return self.selector.clone().unwrap_or_default();
        };
        let mut said = format!("“{name}”");
        // A role that reads as a word; the rest say enough by their name.
        if let Some(role) = self.role.as_deref().and_then(role_word) {
            said = format!("the {said} {role}");
        }
        if let Some(within) = &self.within {
            said.push_str(&format!(" by “{within}”"));
        } else if let Some(nth) = self.nth {
            said.push_str(&format!(" (no. {})", nth + 1));
        }
        said
    }
}

fn role_word(role: &str) -> Option<&'static str> {
    Some(match role {
        "button" => "button",
        "link" => "link",
        "checkbox" => "checkbox",
        "radio" => "option",
        "tab" => "tab",
        "menuitem" => "menu item",
        "switch" => "switch",
        "combobox" => "list",
        "heading" => "heading",
        _ => return None,
    })
}

/// An element of a phone's screen, as a recorded tap names it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub desc: String,
}

impl Mark {
    pub fn is_empty(&self) -> bool {
        self.id.is_empty() && self.text.is_empty() && self.desc.is_empty()
    }

    fn say(&self) -> Option<String> {
        [&self.text, &self.desc]
            .into_iter()
            .find(|said| !said.is_empty())
            .map(|said| format!("“{said}”"))
            // `com.app:id/sign_in` → sign_in.
            .or_else(|| {
                (!self.id.is_empty()).then(|| {
                    self.id
                        .rsplit_once('/')
                        .map_or(self.id.as_str(), |(_, name)| name)
                        .to_string()
                })
            })
    }
}

/// One recorded step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Step {
    Open {
        url: String,
    },
    Click {
        #[serde(flatten)]
        element: Element,
    },
    ClickAt {
        x: f64,
        y: f64,
    },
    Type {
        text: String,
        /// The field; none of it for typing where the focus is.
        #[serde(flatten)]
        element: Element,
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
        /// The element tapped, when the screen said: it is found again by
        /// that, and the point is only where it was.
        #[serde(flatten)]
        mark: Mark,
        /// The size of the screen the point was recorded on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screen: Option<(i64, i64)>,
    },
    Swipe {
        x1: i64,
        y1: i64,
        x2: i64,
        y2: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screen: Option<(i64, i64)>,
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
    /// The screen looks as it did when this check was last accepted
    /// ([`crate::visual`]). `id` names its accepted picture, so that steps
    /// added before it do not change which one that is; `ignore` are the
    /// areas (x, y, width, height in the picture's pixels) that may differ:
    /// a clock, an advertisement.
    ExpectLook {
        id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ignore: Vec<[u32; 4]>,
    },
}

/// `{{secret:NAME}}` or `{{env:NAME}}` in a step's typed text: something
/// kept out of the test file ([`crate::secrets`]).
pub fn placeholder(text: &str) -> Option<(&str, &str)> {
    let inner = text.strip_prefix("{{")?.strip_suffix("}}")?;
    let (kind, name) = inner.split_once(':')?;
    matches!(kind, "secret" | "env").then_some((kind, name))
}

impl Step {
    /// What the step does, in plain words: for the list of steps, a report
    /// and an agent.
    pub fn describe(&self) -> String {
        let typed = |text: &str| match placeholder(text) {
            Some(("secret", _)) => "the saved password".to_string(),
            Some((_, name)) => format!("the value of {name}"),
            None => format!("“{text}”"),
        };
        match self {
            Step::Open { url } => format!("Open {}", short_url(url)),
            Step::Click { element } => format!("Click {}", element.say()),
            Step::ClickAt { x, y } => format!("Click at {x:.0}, {y:.0}"),
            Step::Type {
                text,
                element,
                submit,
            } => format!(
                "Type {}{}{}",
                typed(text),
                if element.is_empty() {
                    String::new()
                } else {
                    // A field is named by its label, without "the … textbox".
                    format!(
                        " into {}",
                        element
                            .name
                            .as_ref()
                            .map(|name| format!("“{name}”"))
                            .unwrap_or_else(|| element.say())
                    )
                },
                if *submit { ", then press Enter" } else { "" }
            ),
            Step::Press { key } => format!("Press {key}"),
            Step::Scroll { dy } => {
                if *dy < 0.0 {
                    "Scroll up".to_string()
                } else {
                    "Scroll down".to_string()
                }
            }
            Step::Launch { package } => format!("Start {package}"),
            Step::Tap { x, y, mark, .. } => match mark.say() {
                Some(said) => format!("Tap {said}"),
                None => format!("Tap at {x}, {y}"),
            },
            Step::Swipe { x1, y1, x2, y2, .. } => {
                let (dx, dy) = (x2 - x1, y2 - y1);
                if dx == 0 && dy == 0 {
                    "Press and hold".to_string()
                } else if dy.abs() >= dx.abs() {
                    // The finger's way: up scrolls the page down.
                    if dy < 0 { "Swipe up" } else { "Swipe down" }.to_string()
                } else if dx < 0 {
                    "Swipe left".to_string()
                } else {
                    "Swipe right".to_string()
                }
            }
            Step::TypeText { text } => format!("Type {}", typed(text)),
            Step::Key { key } => format!("Press {key}"),
            Step::ExpectText { text } => format!("Check that “{text}” shows"),
            Step::ExpectNoText { text } => format!("Check that “{text}” does not show"),
            Step::ExpectUrl { contains } => format!("Check that the address has “{contains}”"),
            Step::ExpectLook { .. } => "Check that the screen looks as accepted".to_string(),
        }
    }

    /// The step only looks (a check), where the others act.
    pub fn is_check(&self) -> bool {
        matches!(
            self,
            Step::ExpectText { .. }
                | Step::ExpectNoText { .. }
                | Step::ExpectUrl { .. }
                | Step::ExpectLook { .. }
        )
    }

    /// The control method and parameters that perform an action step;
    /// `None` for checks (the runner reads the screen for those).
    pub fn call(&self, project_id: i64, serial: Option<&str>) -> Option<(&'static str, Value)> {
        // A replay shows itself in the Tests tab: the pane stays where it is.
        let web = |method, mut params: Value| {
            params["project_id"] = json!(project_id);
            params["quiet"] = json!(true);
            Some((method, params))
        };
        let device = |method, mut params: Value| {
            params["serial"] = json!(serial);
            Some((method, params))
        };
        match self {
            Step::Open { url } => web("browser.open", json!({ "url": url })),
            Step::Click { element } => web(
                "browser.click",
                json!({ "target": element, "wait_ms": ELEMENT_WAIT_MS }),
            ),
            Step::ClickAt { x, y } => web("browser.click", json!({ "x": x, "y": y })),
            Step::Type {
                text,
                element,
                submit,
            } => {
                let mut params = json!({ "text": text, "submit": submit });
                if !element.is_empty() {
                    params["target"] = json!(element);
                    params["wait_ms"] = json!(ELEMENT_WAIT_MS);
                }
                web("browser.type", params)
            }
            Step::Press { key } => web("browser.press", json!({ "key": key })),
            Step::Scroll { dy } => web("browser.scroll", json!({ "dy": dy })),
            Step::Launch { package } => device("device.launch", json!({ "package": package })),
            Step::Tap { x, y, mark, screen } => {
                let mut params = json!({ "x": x, "y": y, "screen": screen });
                if !mark.is_empty() {
                    params["id"] = json!(mark.id);
                    params["text"] = json!(mark.text);
                    params["desc"] = json!(mark.desc);
                    params["wait_ms"] = json!(ELEMENT_WAIT_MS);
                }
                device("device.tap", params)
            }
            Step::Swipe {
                x1,
                y1,
                x2,
                y2,
                screen,
            } => device(
                "device.swipe",
                json!({ "x1": x1, "y1": y1, "x2": x2, "y2": y2, "screen": screen }),
            ),
            // Typed once a field has the focus.
            Step::TypeText { text } => device(
                "device.type",
                json!({ "text": text, "wait_ms": ELEMENT_WAIT_MS }),
            ),
            Step::Key { key } => device("device.key", json!({ "key": key })),
            Step::ExpectText { .. }
            | Step::ExpectNoText { .. }
            | Step::ExpectUrl { .. }
            | Step::ExpectLook { .. } => None,
        }
    }
}

/// `http://localhost:3000/login?next=1` → `localhost:3000/login?next=1`.
fn short_url(url: &str) -> &str {
    let url = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    url.strip_suffix('/').unwrap_or(url)
}

impl TestCase {
    pub fn is_android(&self) -> bool {
        matches!(self.target, Target::Android { .. })
    }

    /// Whether a run starts with what the site keeps (sign-ins, storage) or
    /// the app's data erased. A page does unless the test says otherwise:
    /// a test that signs in finds itself signed in the second time, and
    /// fails. An app's data is only erased when the test asks: it may be
    /// the one build on the user's own phone. (It always starts stopped.)
    pub fn starts_clean(&self) -> bool {
        self.clean.unwrap_or(!self.is_android())
    }

    /// The secrets its steps type: `password` of `{{secret:password}}`.
    pub fn secrets(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .steps
            .iter()
            .filter_map(|step| match step {
                Step::Type { text, .. } | Step::TypeText { text } => placeholder(text),
                _ => None,
            })
            .filter(|(kind, _)| *kind == "secret")
            .map(|(_, name)| name.to_string())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The ids of its look checks: what its accepted pictures are named by.
    pub fn looks(&self) -> Vec<&str> {
        self.steps
            .iter()
            .filter_map(|step| match step {
                Step::ExpectLook { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// An id for a new look check of `test`: short, and not one it has.
pub fn new_look_id(test: &TestCase) -> String {
    let taken = test.looks();
    (1..)
        .map(|n| format!("look-{n}"))
        .find(|id| !taken.contains(&id.as_str()))
        .unwrap_or_default()
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

/// A name for a test from what it should check: its first words.
/// `Sign in as Ada, then add coffee.` → `Sign in as Ada`.
pub fn name_from(scenario: &str) -> String {
    let first = scenario
        .split(['.', ',', ';', ':', '\n'])
        .map(str::trim)
        .find(|part| !part.is_empty())
        .unwrap_or_default();
    let mut name = String::new();
    for word in first.split_whitespace().take(7) {
        if name.len() + word.len() > 48 {
            break;
        }
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(word);
    }
    name
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

    fn button(name: &str) -> Element {
        Element {
            role: Some("button".into()),
            name: Some(name.into()),
            ..Element::default()
        }
    }

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
    fn a_test_is_named_by_the_first_words_of_what_it_checks() {
        assert_eq!(
            name_from("Sign in as Ada, then add coffee."),
            "Sign in as Ada"
        );
        assert_eq!(name_from("  \n"), "");
        assert_eq!(
            name_from("The cart keeps what was added when the page is opened again later on"),
            "The cart keeps what was added when"
        );
    }

    #[test]
    fn steps_round_trip_as_readable_json() {
        let step = Step::Type {
            text: "ada".into(),
            element: Element {
                role: Some("textbox".into()),
                name: Some("Name".into()),
                selector: Some("#user".into()),
                ..Element::default()
            },
            submit: false,
        };
        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(
            json,
            json!({ "do": "type", "text": "ada", "role": "textbox", "name": "Name", "selector": "#user" })
        );
        assert_eq!(serde_json::from_value::<Step>(json).unwrap(), step);
        assert_eq!(step.describe(), "Type “ada” into “Name”");
        let tap = Step::Tap {
            x: 5,
            y: 6,
            mark: Mark {
                text: "Sign in".into(),
                ..Mark::default()
            },
            screen: Some((1080, 2400)),
        };
        let json = serde_json::to_value(&tap).unwrap();
        assert_eq!(
            json,
            json!({ "do": "tap", "x": 5, "y": 6, "text": "Sign in", "screen": [1080, 2400] })
        );
        assert_eq!(serde_json::from_value::<Step>(json).unwrap(), tap);
    }

    #[test]
    fn tests_recorded_before_elements_had_names_still_load() {
        let old = json!({
            "name": "Old", "description": "", "target": { "kind": "web", "url": "http://x" },
            "steps": [
                { "do": "click", "selector": "#go" },
                { "do": "type", "text": "a", "selector": "#q", "submit": true },
                { "do": "type", "text": "b" },
                { "do": "tap", "x": 1, "y": 2 },
                { "do": "swipe", "x1": 1, "y1": 2, "x2": 3, "y2": 4 },
            ]
        });
        let test: TestCase = serde_json::from_value(old).unwrap();
        assert_eq!(
            test.steps[0],
            Step::Click {
                element: Element {
                    selector: Some("#go".into()),
                    ..Element::default()
                }
            }
        );
        assert_eq!(test.steps[0].describe(), "Click #go");
        assert_eq!(
            test.steps[1].describe(),
            "Type “a” into #q, then press Enter"
        );
        assert_eq!(test.steps[2].describe(), "Type “b”");
        assert_eq!(test.steps[3].describe(), "Tap at 1, 2");
        assert!(test.starts_clean(), "a page starts clean unless it says");
    }

    #[test]
    fn steps_say_what_they_do_in_plain_words() {
        let say = |step: Step| step.describe();
        assert_eq!(
            say(Step::Open {
                url: "http://localhost:3000/".into()
            }),
            "Open localhost:3000"
        );
        assert_eq!(
            say(Step::Click {
                element: button("Sign in")
            }),
            "Click the “Sign in” button"
        );
        assert_eq!(
            say(Step::Click {
                element: Element {
                    within: Some("Coffee".into()),
                    ..button("Add")
                }
            }),
            "Click the “Add” button by “Coffee”"
        );
        assert_eq!(
            say(Step::Click {
                element: Element {
                    nth: Some(1),
                    ..button("Add")
                }
            }),
            "Click the “Add” button (no. 2)"
        );
        assert_eq!(
            say(Step::Type {
                text: "{{secret:password}}".into(),
                element: Element {
                    name: Some("Password".into()),
                    ..Element::default()
                },
                submit: false
            }),
            "Type the saved password into “Password”"
        );
        assert_eq!(
            say(Step::Tap {
                x: 1,
                y: 2,
                mark: Mark {
                    id: "com.app:id/sign_in".into(),
                    ..Mark::default()
                },
                screen: None
            }),
            "Tap sign_in"
        );
        assert_eq!(
            say(Step::Swipe {
                x1: 500,
                y1: 1800,
                x2: 500,
                y2: 600,
                screen: None
            }),
            "Swipe up"
        );
        assert_eq!(
            say(Step::ExpectNoText {
                text: "Error".into()
            }),
            "Check that “Error” does not show"
        );
    }

    #[test]
    fn actions_map_to_control_calls() {
        let (method, params) = Step::Click {
            element: button("Go"),
        }
        .call(3, None)
        .unwrap();
        assert_eq!(method, "browser.click");
        assert_eq!(params["project_id"], 3);
        assert_eq!(params["target"], json!({ "role": "button", "name": "Go" }));
        assert_eq!(params["wait_ms"], ELEMENT_WAIT_MS);
        assert_eq!(params["quiet"], true, "a replay leaves the pane alone");
        // Typing where the focus is names no field, and waits for none.
        let (_, params) = Step::Type {
            text: "a".into(),
            element: Element::default(),
            submit: false,
        }
        .call(3, None)
        .unwrap();
        assert!(params.get("target").is_none() && params.get("wait_ms").is_none());
        let (method, params) = Step::Tap {
            x: 10,
            y: 20,
            mark: Mark {
                text: "OK".into(),
                ..Mark::default()
            },
            screen: Some((1080, 2400)),
        }
        .call(3, Some("emulator-5554"))
        .unwrap();
        assert_eq!(
            (method, params["serial"].as_str(), params["text"].as_str()),
            ("device.tap", Some("emulator-5554"), Some("OK"))
        );
        assert_eq!(params["screen"], json!([1080, 2400]));
        assert!(
            Step::ExpectText { text: "Hi".into() }
                .call(3, None)
                .is_none()
        );
    }

    #[test]
    fn a_test_says_what_it_keeps_out_of_its_file() {
        let mut test = TestCase {
            name: "t".into(),
            description: String::new(),
            target: Target::Android {
                package: "com.app".into(),
            },
            clean: None,
            steps: vec![
                Step::TypeText {
                    text: "{{secret:password}}".into(),
                },
                Step::TypeText {
                    text: "{{env:E2E_USER}}".into(),
                },
                Step::ExpectLook {
                    id: "look-1".into(),
                    ignore: Vec::new(),
                },
            ],
        };
        assert_eq!(test.secrets(), ["password"]);
        assert_eq!(new_look_id(&test), "look-2");
        assert!(!test.starts_clean(), "an app's data stays unless asked");
        test.clean = Some(true);
        assert!(test.starts_clean());
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
            clean: None,
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
