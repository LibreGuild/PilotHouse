//! A UI test as a file another runner takes: a Playwright spec for a page,
//! a Maestro flow for a phone. For a team that runs its tests where
//! PilotHouse is not (a build server): the test stays here as it is, and
//! the file says the same steps in that runner's words.
//!
//! The file is written again from the test each time it is exported, and
//! says so in its first line; a file that does not start with that line is
//! someone's own and is left alone.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::model::{self, Element, Mark, Step, Target, TestCase, placeholder};

/// How an exported file starts. One that starts like this is replaced by
/// the next export.
const MARK: &str = "Exported from a UI test in ";

/// Where the file of `test` goes, under the project.
pub fn file_of(test: &TestCase) -> String {
    let slug = model::slug(&test.name);
    match test.target {
        Target::Web { .. } => format!("e2e/{slug}.spec.ts"),
        Target::Android { .. } => format!(".maestro/{slug}.yaml"),
    }
}

/// Writes `test` (kept in `source`) as a Playwright spec or a Maestro flow
/// into the project at `root`, and says where.
pub fn write(root: &Path, test: &TestCase, source: &Path) -> Result<PathBuf> {
    let path = root.join(file_of(test));
    let source = source
        .strip_prefix(root)
        .unwrap_or(source)
        .to_string_lossy()
        .replace('\\', "/");
    let text = match test.target {
        Target::Web { .. } => playwright(test, &source),
        Target::Android { .. } => maestro(test, &source),
    };
    if let Ok(there) = std::fs::read_to_string(&path)
        && !there
            .lines()
            .next()
            .is_some_and(|first| first.contains(MARK))
    {
        bail!(
            "{} is already there and was not exported from here: move it away first.",
            file_of(test)
        );
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(&path, text)?;
    Ok(path)
}

/// The environment variable a runner elsewhere reads a secret from:
/// `password` → `TEST_PASSWORD`, `{{env:E2E_USER}}` → `E2E_USER`.
fn variable(kind: &str, name: &str) -> String {
    if kind == "env" {
        return name.to_string();
    }
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("TEST_{name}")
}

/// The variables the steps of `test` read, in order, each once.
fn variables(test: &TestCase) -> Vec<String> {
    let mut names = Vec::new();
    for step in &test.steps {
        if let Step::Type { text, .. } | Step::TypeText { text } = step
            && let Some((kind, name)) = placeholder(text)
        {
            let name = variable(kind, name);
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// `text` as a JavaScript string in single quotes.
fn js(text: &str) -> String {
    let mut quoted = String::from("'");
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '\'' => quoted.push_str("\\'"),
            '\n' => quoted.push_str("\\n"),
            '\r' => {}
            c => quoted.push(c),
        }
    }
    quoted.push('\'');
    quoted
}

/// `text` as a regular expression that matches exactly it.
fn js_regex(text: &str) -> String {
    let mut regex = String::from("/");
    for c in text.chars() {
        if "\\^$.*+?()[]{}|/".contains(c) {
            regex.push('\\');
        }
        regex.push(c);
    }
    regex.push('/');
    regex
}

/// The Playwright locator of `element`. A field that hides what is typed
/// has no role Playwright knows it by: it is found by its label.
fn locator(element: &Element, hidden: bool) -> String {
    let hook = element
        .selector
        .as_deref()
        .filter(|selector| selector.starts_with("[data-"));
    let (Some(name), None) = (&element.name, hook) else {
        return format!(
            "page.locator({})",
            js(element.selector.as_deref().unwrap_or_default())
        );
    };
    let exact = format!("{}, {{ exact: true }}", js(name));
    // The same locator from the page, or from the row it is in.
    let named = |from: &str| match element.role.as_deref() {
        _ if hidden => format!("{from}.getByLabel({exact}).or({from}.getByPlaceholder({exact}))"),
        Some(role) => format!(
            "{from}.getByRole({}, {{ name: {}, exact: true }})",
            js(role),
            js(name)
        ),
        None => format!("{from}.getByText({exact})"),
    };
    let mut found = match &element.within {
        // The smallest thing that says those words and has the element.
        Some(within) => {
            let around = format!(
                "page.locator('*').filter({{ hasText: {} }}).filter({{ has: {} }}).last()",
                js(within),
                named("page")
            );
            named(&around)
        }
        None => named("page"),
    };
    if let Some(nth) = element.nth {
        found.push_str(&format!(".nth({nth})"));
    }
    found
}

/// What a step types, as JavaScript: the text, or the variable it is kept
/// in elsewhere.
fn typed_js(text: &str) -> (String, bool) {
    match placeholder(text) {
        Some((kind, name)) => (
            format!("process.env.{} ?? ''", variable(kind, name)),
            kind == "secret",
        ),
        None => (js(text), false),
    }
}

/// `test` as a Playwright spec.
pub fn playwright(test: &TestCase, source: &str) -> String {
    let mut lines = vec![
        format!("// {MARK}{} ({source}).", ph_brand::NAME),
        "// Exporting it again replaces this file: change the test there.".to_string(),
    ];
    let variables = variables(test);
    if !variables.is_empty() {
        lines.push(format!(
            "// It types what is kept out of the test: set {} where it runs.",
            variables.join(", ")
        ));
    }
    lines.push("import { test, expect } from '@playwright/test';".to_string());
    lines.push(String::new());
    // The size a run here shows the page at: its looks, and the areas they
    // leave out, are of that size.
    if let Some(crate::run::Variant::Screen { width, height, .. }) = crate::run::screens().first() {
        lines.push(format!(
            "test.use({{ viewport: {{ width: {width}, height: {height} }} }});"
        ));
        lines.push(String::new());
    }
    for line in test
        .description
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        lines.push(format!("// {}", line.trim()));
    }
    lines.push(format!("test({}, async ({{ page }}) => {{", js(&test.name)));
    let mut step = |code: String| lines.push(format!("  {code}"));
    for planned in crate::run::plan(test) {
        match &planned {
            Step::Open { url } => step(format!("await page.goto({});", js(url))),
            Step::Click { element } => step(format!("await {}.click();", locator(element, false))),
            Step::ClickAt { x, y } => step(format!("await page.mouse.click({x:.0}, {y:.0});")),
            Step::Type {
                text,
                element,
                submit,
            } => {
                let (value, hidden) = typed_js(text);
                if element.is_empty() {
                    step(format!("await page.keyboard.insertText({value});"));
                    if *submit {
                        step("await page.keyboard.press('Enter');".to_string());
                    }
                } else {
                    let field = locator(element, hidden);
                    step(format!("await {field}.fill({value});"));
                    if *submit {
                        step(format!("await {field}.press('Enter');"));
                    }
                }
            }
            Step::Press { key } => step(format!("await page.keyboard.press({});", js(key))),
            Step::Scroll { dy } => step(format!("await page.mouse.wheel(0, {dy:.0});")),
            Step::ExpectText { text } => step(format!(
                "await expect(page.locator('body')).toContainText({}, {{ ignoreCase: true }});",
                js(text)
            )),
            Step::ExpectNoText { text } => step(format!(
                "await expect(page.locator('body')).not.toContainText({}, {{ ignoreCase: true }});",
                js(text)
            )),
            Step::ExpectUrl { contains } => step(format!(
                "await expect(page).toHaveURL({});",
                js_regex(contains)
            )),
            // Playwright keeps its own accepted pictures: the first run
            // writes them (`--update-snapshots`). What the check leaves out
            // is covered for the picture, since it takes no areas.
            Step::ExpectLook { id, ignore } => {
                if !ignore.is_empty() {
                    step(format!(
                        "await page.evaluate((areas) => {{ for (const [x, y, w, h] of areas) {{ const cover = document.createElement('div'); cover.className = 'test-left-out'; cover.style.cssText = `position:fixed;left:${{x}}px;top:${{y}}px;width:${{w}}px;height:${{h}}px;background:#808080;z-index:2147483647`; document.body.appendChild(cover); }} }}, {});",
                        serde_json::json!(ignore)
                    ));
                }
                step(format!(
                    "await expect(page).toHaveScreenshot({});",
                    js(&format!("{id}.png"))
                ));
                if !ignore.is_empty() {
                    step("await page.evaluate(() => document.querySelectorAll('.test-left-out').forEach((cover) => cover.remove()));".to_string());
                }
            }
            other => step(format!("// Not a step of a page: {}", other.describe())),
        }
    }
    lines.push("});".to_string());
    lines.join("\n") + "\n"
}

/// `text` as a YAML string in double quotes.
fn yaml(text: &str) -> String {
    let mut quoted = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => {}
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// A point of a screen of size `screen` as Maestro takes it: in percent,
/// so that it is the same place on another phone; in pixels when the size
/// is not known.
fn point(x: i64, y: i64, screen: Option<(i64, i64)>) -> String {
    match screen {
        Some((width, height)) if width > 0 && height > 0 => {
            format!("{}%,{}%", x * 100 / width, y * 100 / height)
        }
        _ => format!("{x},{y}"),
    }
}

/// What a tap lands on, as Maestro finds it: by its words when it has
/// some, else by its id (without the app's, which differs between builds).
fn tap(mark: &Mark, x: i64, y: i64, screen: Option<(i64, i64)>) -> String {
    let said = [&mark.text, &mark.desc]
        .into_iter()
        .find(|said| !said.is_empty());
    if let Some(said) = said {
        return format!("- tapOn: {}", yaml(said));
    }
    if !mark.id.is_empty() {
        let id = mark
            .id
            .rsplit_once(":id/")
            .map_or(mark.id.as_str(), |(_, id)| id);
        return format!("- tapOn:\n    id: {}", yaml(id));
    }
    format!("- tapOn:\n    point: {}", yaml(&point(x, y, screen)))
}

/// The name Maestro has for a key.
fn maestro_key(key: &str) -> Option<&'static str> {
    Some(match key.to_ascii_lowercase().as_str() {
        "back" => "Back",
        "home" => "Home",
        "enter" => "Enter",
        "delete" | "backspace" => "Backspace",
        "tab" => "Tab",
        "volume_up" => "Volume Up",
        "volume_down" => "Volume Down",
        "power" => "Power",
        _ => return None,
    })
}

/// `test` as a Maestro flow.
pub fn maestro(test: &TestCase, source: &str) -> String {
    let package = match &test.target {
        Target::Android { package } => package.as_str(),
        Target::Web { .. } => "",
    };
    let mut lines = vec![
        format!("# {MARK}{} ({source}).", ph_brand::NAME),
        "# Exporting it again replaces this file: change the test there.".to_string(),
    ];
    let variables = variables(test);
    if !variables.is_empty() {
        lines.push(format!(
            "# It types what is kept out of the test: run it with {}.",
            variables
                .iter()
                .map(|name| format!("-e {name}=…"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    for line in test
        .description
        .lines()
        .filter(|line| !line.trim().is_empty())
    {
        lines.push(format!("# {}", line.trim()));
    }
    lines.push(format!("appId: {package}"));
    lines.push("---".to_string());
    let mut launched = false;
    for planned in crate::run::plan(test) {
        lines.push(match &planned {
            Step::Launch { package: app } => {
                let mut launch = vec!["- launchApp:".to_string()];
                if app != package {
                    launch.push(format!("    appId: {}", yaml(app)));
                }
                // As a run here starts it: stopped, and with its data
                // erased when the test asks.
                let first = !std::mem::replace(&mut launched, true);
                launch.push(format!("    clearState: {}", first && test.starts_clean()));
                launch.join("\n")
            }
            Step::Tap { x, y, mark, screen } => tap(mark, *x, *y, *screen),
            Step::Swipe {
                x1,
                y1,
                x2,
                y2,
                screen,
            } if (x1, y1) == (x2, y2) => format!(
                "- longPressOn:\n    point: {}",
                yaml(&point(*x1, *y1, *screen))
            ),
            Step::Swipe {
                x1,
                y1,
                x2,
                y2,
                screen,
            } => format!(
                "- swipe:\n    start: {}\n    end: {}",
                yaml(&point(*x1, *y1, *screen)),
                yaml(&point(*x2, *y2, *screen))
            ),
            Step::TypeText { text } => match placeholder(text) {
                Some((kind, name)) => format!("- inputText: ${{{}}}", variable(kind, name)),
                None => format!("- inputText: {}", yaml(text)),
            },
            Step::Key { key } => match maestro_key(key) {
                Some(key) => format!("- pressKey: {key}"),
                None => format!("# No key of that name in a flow: {key}"),
            },
            Step::ExpectText { text } => format!("- assertVisible: {}", yaml(&contains(text))),
            Step::ExpectNoText { text } => {
                format!("- assertNotVisible: {}", yaml(&contains(text)))
            }
            // A flow has no accepted pictures to compare with: the screen
            // is kept, for whoever reads the run.
            Step::ExpectLook { id, .. } => format!("- takeScreenshot: {id}"),
            other => format!("# Not a step of a phone: {}", other.describe()),
        });
    }
    lines.join("\n") + "\n"
}

/// `text` as the pattern of a screen that has it somewhere: Maestro matches
/// an element's whole text, and a check here asks only that it is shown.
fn contains(text: &str) -> String {
    let mut pattern = String::from(".*");
    for c in text.chars() {
        if "\\^$.*+?()[]{}|".contains(c) {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push_str(".*");
    pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(role: Option<&str>, name: &str) -> Element {
        Element {
            role: role.map(str::to_string),
            name: Some(name.into()),
            ..Element::default()
        }
    }

    #[test]
    fn a_page_test_reads_as_a_playwright_spec() {
        let test = TestCase {
            name: "Sign in and add coffee".into(),
            description: "Sign in as Ada; the cart should say Coffee.".into(),
            target: Target::Web {
                url: "http://localhost:3000".into(),
            },
            clean: None,
            steps: vec![
                Step::Type {
                    text: "Ada".into(),
                    element: named(Some("textbox"), "Name"),
                    submit: false,
                },
                Step::Type {
                    text: "{{secret:password}}".into(),
                    element: named(Some("textbox"), "Password"),
                    submit: true,
                },
                Step::Click {
                    element: Element {
                        within: Some("Coffee".into()),
                        ..named(Some("button"), "Add")
                    },
                },
                Step::Click {
                    element: Element {
                        selector: Some("[data-testid=\"pay\"]".into()),
                        ..named(Some("button"), "Pay")
                    },
                },
                Step::Click {
                    element: Element {
                        nth: Some(1),
                        ..named(None, "It's here")
                    },
                },
                Step::ExpectText {
                    text: "Cart: Coffee".into(),
                },
                Step::ExpectUrl {
                    contains: "/cart?id=1".into(),
                },
                Step::ExpectLook {
                    id: "look-1".into(),
                    ignore: vec![[72, 416, 176, 64]],
                },
            ],
        };
        let spec = playwright(&test, ".pilothouse/tests/sign-in-and-add-coffee.json");
        let expected = format!(
            "\
// Exported from a UI test in {brand} (.pilothouse/tests/sign-in-and-add-coffee.json).
// Exporting it again replaces this file: change the test there.
// It types what is kept out of the test: set TEST_PASSWORD where it runs.
import {{ test, expect }} from '@playwright/test';

test.use({{ viewport: {{ width: 1280, height: 800 }} }});

// Sign in as Ada; the cart should say Coffee.
test('Sign in and add coffee', async ({{ page }}) => {{
  await page.goto('http://localhost:3000');
  await page.getByRole('textbox', {{ name: 'Name', exact: true }}).fill('Ada');
  await page.getByLabel('Password', {{ exact: true }}).or(page.getByPlaceholder('Password', {{ exact: true }})).fill(process.env.TEST_PASSWORD ?? '');
  await page.getByLabel('Password', {{ exact: true }}).or(page.getByPlaceholder('Password', {{ exact: true }})).press('Enter');
  await page.locator('*').filter({{ hasText: 'Coffee' }}).filter({{ has: page.getByRole('button', {{ name: 'Add', exact: true }}) }}).last().getByRole('button', {{ name: 'Add', exact: true }}).click();
  await page.locator('[data-testid=\"pay\"]').click();
  await page.getByText('It\\'s here', {{ exact: true }}).nth(1).click();
  await expect(page.locator('body')).toContainText('Cart: Coffee', {{ ignoreCase: true }});
  await expect(page).toHaveURL(/\\/cart\\?id=1/);
  await page.evaluate((areas) => {{ for (const [x, y, w, h] of areas) {{ const cover = document.createElement('div'); cover.className = 'test-left-out'; cover.style.cssText = `position:fixed;left:${{x}}px;top:${{y}}px;width:${{w}}px;height:${{h}}px;background:#808080;z-index:2147483647`; document.body.appendChild(cover); }} }}, [[72,416,176,64]]);
  await expect(page).toHaveScreenshot('look-1.png');
  await page.evaluate(() => document.querySelectorAll('.test-left-out').forEach((cover) => cover.remove()));
}});
",
            brand = ph_brand::NAME
        );
        assert_eq!(spec, expected);
        assert_eq!(file_of(&test), "e2e/sign-in-and-add-coffee.spec.ts");
    }

    #[test]
    fn a_phone_test_reads_as_a_maestro_flow() {
        let test = TestCase {
            name: "Sign in on the phone".into(),
            description: String::new(),
            target: Target::Android {
                package: "com.example.app".into(),
            },
            clean: Some(true),
            steps: vec![
                Step::Tap {
                    x: 540,
                    y: 1200,
                    mark: Mark {
                        id: "com.example.app:id/name".into(),
                        ..Mark::default()
                    },
                    screen: Some((1080, 2400)),
                },
                Step::TypeText { text: "Ada".into() },
                Step::TypeText {
                    text: "{{secret:password-2}}".into(),
                },
                Step::Tap {
                    x: 1,
                    y: 2,
                    mark: Mark {
                        text: "Say \"hi\"".into(),
                        id: "com.example.app:id/go".into(),
                        desc: String::new(),
                    },
                    screen: None,
                },
                Step::Tap {
                    x: 540,
                    y: 1800,
                    mark: Mark::default(),
                    screen: Some((1080, 2400)),
                },
                Step::Swipe {
                    x1: 540,
                    y1: 1800,
                    x2: 540,
                    y2: 600,
                    screen: Some((1080, 2400)),
                },
                Step::Key { key: "back".into() },
                Step::ExpectText {
                    text: "Welcome (Ada)".into(),
                },
                Step::ExpectNoText {
                    text: "Error".into(),
                },
            ],
        };
        let flow = maestro(&test, ".pilothouse/tests/sign-in-on-the-phone.json");
        let expected = format!(
            "\
# Exported from a UI test in {brand} (.pilothouse/tests/sign-in-on-the-phone.json).
# Exporting it again replaces this file: change the test there.
# It types what is kept out of the test: run it with -e TEST_PASSWORD_2=….
appId: com.example.app
---
- launchApp:
    clearState: true
- tapOn:
    id: \"name\"
- inputText: \"Ada\"
- inputText: ${{TEST_PASSWORD_2}}
- tapOn: \"Say \\\"hi\\\"\"
- tapOn:
    point: \"50%,75%\"
- swipe:
    start: \"50%,75%\"
    end: \"50%,25%\"
- pressKey: Back
- assertVisible: \".*Welcome \\\\(Ada\\\\).*\"
- assertNotVisible: \".*Error.*\"
",
            brand = ph_brand::NAME
        );
        assert_eq!(flow, expected);
        assert_eq!(file_of(&test), ".maestro/sign-in-on-the-phone.yaml");
    }

    #[test]
    fn an_export_never_replaces_someones_own_file() {
        let root = std::env::temp_dir().join(format!("ph-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let test = TestCase {
            name: "Opens".into(),
            description: String::new(),
            target: Target::Web {
                url: "http://localhost:3000".into(),
            },
            clean: None,
            steps: Vec::new(),
        };
        let source = root.join(".pilothouse/tests/opens.json");
        let path = write(&root, &test, &source).unwrap();
        assert!(path.ends_with("e2e/opens.spec.ts"));
        // Its own export is written again.
        write(&root, &test, &source).unwrap();
        std::fs::write(&path, "// mine\n").unwrap();
        let err = write(&root, &test, &source).unwrap_err().to_string();
        assert!(err.contains("was not exported from here"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "// mine\n");
        let _ = std::fs::remove_dir_all(&root);
    }
}
