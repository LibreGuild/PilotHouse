//! Recording: while an agent works through a test's scenario with the
//! browser or device tools, each action it takes successfully becomes a
//! step. Looking (snapshots, screenshots, logs) is not recorded; checks are
//! added with the `test_expect` tool, which also checks at once so the
//! agent knows it holds.
//!
//! A step keeps what the action landed on as a person would name it (the
//! “Sign in” button), which the browser and the device say in their answer
//! to the call, not the selector or the point the agent happened to use.

use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Result, bail};
use gpui_kit::{App, Global, Task};
use ph_control::ControlServer;
use serde_json::{Value, json};

use crate::model::{self, Element, Mark, Step, Target, TestCase};

/// The recording in progress, if any.
#[derive(Default)]
pub struct Recorder {
    pub session: Option<Recording>,
}

impl Global for Recorder {}

#[derive(Clone)]
pub struct Recording {
    pub project_id: i64,
    /// The test file the steps go into.
    pub path: PathBuf,
    pub steps: Vec<Step>,
    /// The chat whose agent records, once it has started: only its turn
    /// ending ends the recording.
    pub chat: Option<i64>,
    /// The passwords typed, which the steps name by their place here
    /// (`{{secret:?0}}`) until [`Recording::keep_passwords`] has put them
    /// away. Never written anywhere else.
    passwords: Vec<String>,
}

impl Recording {
    /// Puts the passwords typed into the credential store and names them in
    /// the steps (`{{secret:password}}`). Gives the steps, and the
    /// passwords, for whoever writes the test down to take them out of
    /// what else it says. Blocks: call it off the UI thread.
    pub fn keep_passwords(mut self) -> Result<(Vec<Step>, Vec<String>)> {
        for (at, password) in self.passwords.iter().enumerate() {
            let name = crate::secrets::keep(self.project_id, password)?;
            let (from, to) = (pending(at), format!("{{{{secret:{name}}}}}"));
            for step in &mut self.steps {
                if let Step::Type { text, .. } | Step::TypeText { text } = step
                    && *text == from
                {
                    *text = to.clone();
                }
            }
        }
        Ok((self.steps, self.passwords))
    }
}

/// What stands in a test's description where a password was.
const NOT_SAID: &str = "(the saved password)";

/// `text` without `passwords`: the user told the agent the password in
/// what the test should check, and the test's file is shared.
pub fn without_passwords(text: &str, passwords: &[String]) -> String {
    passwords
        .iter()
        .filter(|password| !password.is_empty())
        .fold(text.to_string(), |text, password| {
            text.replace(password.as_str(), NOT_SAID)
        })
}

/// What a step types for the `at`-th password until it has a name.
fn pending(at: usize) -> String {
    format!("{{{{secret:?{at}}}}}")
}

/// Starts recording `project_id`'s browser and device actions, after the
/// steps already kept (`steps`, empty for a new recording). Refused while
/// another recording runs: there is one for the whole app.
pub fn start(project_id: i64, path: PathBuf, steps: Vec<Step>, cx: &mut App) -> Result<()> {
    let recorder = cx.default_global::<Recorder>();
    if let Some(running) = &recorder.session {
        if running.project_id == project_id {
            bail!("A test is already being recorded here: stop it first.");
        }
        bail!("A test is being recorded in another workspace: stop it there first.");
    }
    recorder.session = Some(Recording {
        project_id,
        path,
        steps,
        chat: None,
        passwords: Vec::new(),
    });
    ControlServer::set_observer(Some(Rc::new(observe)), cx);
    // Taps say which element they landed on, from now on.
    ControlServer::call("device.record", json!({ "on": true }), cx).detach();
    Ok(())
}

/// Starts recording `test` (kept in `path`) where each of its runs starts:
/// with what its site keeps erased, or its app at its first screen. What
/// is done to get there is not a step of it.
pub fn begin(project_id: i64, path: PathBuf, test: &TestCase, cx: &mut App) -> Task<Result<()>> {
    if current(cx).is_some() {
        return Task::ready(Err(anyhow::anyhow!(
            "A test is already being recorded: stop it first."
        )));
    }
    let prepare = match &test.target {
        Target::Web { url } if test.starts_clean() => ControlServer::call(
            "browser.clear_sites",
            json!({ "project_id": project_id, "urls": [url], "quiet": true }),
            cx,
        ),
        Target::Android { package } => ControlServer::call(
            "device.launch",
            json!({ "package": package, "fresh": true, "clear": test.starts_clean() }),
            cx,
        ),
        Target::Web { .. } => Task::ready(Ok(json!({}))),
    };
    cx.spawn(async move |cx| {
        // No browser or no phone: whoever records meets that itself, at its
        // first step, and says so.
        let _ = prepare.await;
        cx.update(|cx| start(project_id, path, Vec::new(), cx))
    })
}

/// Writes a recording that ended into its test's file: its steps, with the
/// passwords typed put away and taken out of what the test says it checks.
/// Gives the file and the steps. Blocks: call it off the UI thread.
pub fn write_down(recording: Recording) -> Result<(PathBuf, Vec<Step>)> {
    let path = recording.path.clone();
    let (steps, passwords) = recording.keep_passwords()?;
    // As the file is now: it may have been edited since the recording began.
    let mut test = model::load(&path)?;
    test.steps = steps.clone();
    test.description = without_passwords(&test.description, &passwords);
    model::save(&path, &test)?;
    Ok((path, steps))
}

/// Names the chat whose agent does the recording just started.
pub fn set_chat(chat: i64, cx: &mut App) {
    if let Some(recording) = cx.default_global::<Recorder>().session.as_mut() {
        recording.chat.get_or_insert(chat);
    }
}

/// Stops recording and returns it.
pub fn stop(cx: &mut App) -> Option<Recording> {
    ControlServer::set_observer(None, cx);
    ControlServer::call("device.record", json!({ "on": false }), cx).detach();
    cx.default_global::<Recorder>().session.take()
}

pub fn current(cx: &App) -> Option<&Recording> {
    cx.try_global::<Recorder>()?.session.as_ref()
}

fn observe(method: &str, params: &Value, result: &Value, cx: &mut App) {
    let Some(recording) = cx.default_global::<Recorder>().session.as_mut() else {
        return;
    };
    // A replay's own calls (the agent ran a test meanwhile) are not what the
    // agent did.
    if params["quiet"] == true {
        return;
    }
    // Agents' calls name their workspace: another workspace's are not this
    // test's. Browser calls must name it; a device call without one (from
    // `pilothouse ctl`) acts on the device in view.
    let project = params["project_id"].as_i64();
    if project.is_some_and(|project| project != recording.project_id)
        || (method.starts_with("browser.") && project.is_none())
    {
        return;
    }
    let Some(mut step) = step_for(method, params, result) else {
        return;
    };
    // A password typed while recording is never written to the test (which
    // is shared with the project): it goes to this computer's credential
    // store when the recording ends, and the step names it.
    if let Step::Type { text, .. } | Step::TypeText { text } = &mut step
        && result["secret"] == true
    {
        let at = match recording.passwords.iter().position(|kept| kept == text) {
            Some(at) => at,
            None => {
                recording.passwords.push(std::mem::take(text));
                recording.passwords.len() - 1
            }
        };
        *text = pending(at);
    }
    // Typing into the same field again replaces what was typed.
    if let (Some(Step::Type { element: last, .. }), Step::Type { element, .. }) =
        (recording.steps.last(), &step)
        && last == element
        && !element.is_empty()
    {
        recording.steps.pop();
    }
    // A look check is named, so that its accepted picture stays its own
    // when steps are added before it.
    if let Step::ExpectLook { id, .. } = &mut step {
        let taken: Vec<&str> = recording
            .steps
            .iter()
            .filter_map(|step| match step {
                Step::ExpectLook { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        *id = (1..)
            .map(|n| format!("look-{n}"))
            .find(|id| !taken.contains(&id.as_str()))
            .unwrap_or_default();
    }
    recording.steps.push(step);
}

/// The step a successful control call stands for (`result` is its answer,
/// which says what it acted on); `None` for calls that only look.
pub fn step_for(method: &str, params: &Value, result: &Value) -> Option<Step> {
    let text = |key: &str| params[key].as_str().map(str::to_string);
    let int = |key: &str| params[key].as_f64().map(|value| value.round() as i64);
    // The element as the browser named it, else as the agent pointed at it.
    let element = || {
        Element::from_json(&result["element"]).or_else(|| {
            text("selector").map(|selector| Element {
                selector: Some(selector),
                ..Element::default()
            })
        })
    };
    let screen = || {
        let size = &result["screen"];
        Some((size.get(0)?.as_i64()?, size.get(1)?.as_i64()?))
    };
    Some(match method {
        "browser.open" => Step::Open { url: text("url")? },
        "browser.click" => match element() {
            Some(element) => Step::Click { element },
            None => Step::ClickAt {
                x: params["x"].as_f64()?,
                y: params["y"].as_f64()?,
            },
        },
        "browser.type" => Step::Type {
            text: text("text")?,
            element: element().unwrap_or_default(),
            submit: params["submit"].as_bool().unwrap_or(false),
        },
        "browser.press" => Step::Press { key: text("key")? },
        "browser.scroll" => Step::Scroll {
            dy: params["dy"].as_f64().unwrap_or(600.0),
        },
        "device.launch" => Step::Launch {
            package: text("package")?,
        },
        "device.tap" => Step::Tap {
            x: int("x")?,
            y: int("y")?,
            mark: serde_json::from_value::<Mark>(result["element"].clone()).unwrap_or_default(),
            screen: screen(),
        },
        "device.swipe" => Step::Swipe {
            x1: int("x1")?,
            y1: int("y1")?,
            x2: int("x2")?,
            y2: int("y2")?,
            screen: screen(),
        },
        "device.type" => Step::TypeText {
            text: text("text")?,
        },
        "device.key" => Step::Key { key: text("key")? },
        "test.expect" => expectation(params).ok()?,
        _ => return None,
    })
}

/// `{kind: "text" | "no_text" | "url" | "look", value}` as a check.
fn expectation(params: &Value) -> Result<Step> {
    let value = params["value"].as_str().unwrap_or_default().to_string();
    let kind = params["kind"].as_str().unwrap_or("text");
    if kind == "look" {
        // Named when it joins a recording.
        return Ok(Step::ExpectLook {
            id: String::new(),
            ignore: Vec::new(),
        });
    }
    if value.is_empty() {
        bail!("`value` is required");
    }
    Ok(match kind {
        "text" => Step::ExpectText { text: value },
        "no_text" => Step::ExpectNoText { text: value },
        "url" => Step::ExpectUrl { contains: value },
        other => bail!("unknown kind `{other}`: use text, no_text, url or look"),
    })
}

/// `test.expect`: checks the screen now (so the agent learns whether it
/// holds); a recording keeps it as a step when it does.
pub fn register(cx: &mut App) {
    ControlServer::handle_async(
        "test.expect",
        |params, cx| {
            let step = match expectation(&params) {
                Ok(step) => step,
                Err(err) => return Task::ready(Err(err)),
            };
            // There is nothing to compare a look with until a run of the
            // test was accepted.
            if matches!(step, Step::ExpectLook { .. }) {
                return Task::ready(Ok(json!({
                    "ok": true,
                    "note": "Recorded: the screen here is compared with the look the user accepts after a run."
                })));
            }
            let project_id = params["project_id"].as_i64().unwrap_or_default();
            let android = params["device"].as_bool().unwrap_or(false);
            let check = crate::run::check(step, project_id, android, None, cx);
            cx.spawn(async move |_| {
                check.await?;
                Ok(json!({ "ok": true, "note": "It holds; recorded as a check if a test is being recorded." }))
            })
        },
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_become_steps_and_looking_does_not() {
        let none = json!({});
        // The browser says what the click landed on: that is kept.
        assert_eq!(
            step_for(
                "browser.click",
                &json!({ "selector": "#go", "project_id": 1 }),
                &json!({ "element": { "role": "button", "name": "Go", "selector": "#go" } })
            ),
            Some(Step::Click {
                element: Element {
                    role: Some("button".into()),
                    name: Some("Go".into()),
                    selector: Some("#go".into()),
                    ..Element::default()
                }
            })
        );
        // Without it, the selector the agent used.
        assert_eq!(
            step_for("browser.click", &json!({ "selector": "#go" }), &none),
            Some(Step::Click {
                element: Element {
                    selector: Some("#go".into()),
                    ..Element::default()
                }
            })
        );
        assert_eq!(
            step_for("browser.click", &json!({ "x": 3.0, "y": 4.0 }), &none),
            Some(Step::ClickAt { x: 3.0, y: 4.0 })
        );
        assert_eq!(step_for("browser.snapshot", &json!({}), &none), None);
        assert_eq!(step_for("device.screenshot", &json!({}), &none), None);
        assert_eq!(
            step_for(
                "device.tap",
                &json!({ "x": 540, "y": 1200 }),
                &json!({ "element": { "id": "com.app:id/go", "text": "Go", "desc": "" }, "screen": [1080, 2400] })
            ),
            Some(Step::Tap {
                x: 540,
                y: 1200,
                mark: Mark {
                    id: "com.app:id/go".into(),
                    text: "Go".into(),
                    desc: String::new()
                },
                screen: Some((1080, 2400))
            })
        );
        assert_eq!(
            step_for(
                "device.swipe",
                &json!({ "x1": 1, "y1": 2, "x2": 3, "y2": 4 }),
                &none
            ),
            Some(Step::Swipe {
                x1: 1,
                y1: 2,
                x2: 3,
                y2: 4,
                screen: None
            })
        );
        assert_eq!(
            step_for(
                "test.expect",
                &json!({ "kind": "url", "value": "/home" }),
                &none
            ),
            Some(Step::ExpectUrl {
                contains: "/home".into()
            })
        );
        assert_eq!(
            step_for(
                "test.expect",
                &json!({ "kind": "nope", "value": "x" }),
                &none
            ),
            None
        );
    }

    #[gpui_kit::test]
    fn recording_keeps_the_projects_actions(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            start(7, PathBuf::from("t.json"), Vec::new(), cx).unwrap();
            // One recording at a time, whoever asks.
            assert!(start(8, PathBuf::from("u.json"), Vec::new(), cx).is_err());
            let none = json!({});
            observe(
                "browser.open",
                &json!({ "project_id": 7, "url": "http://x" }),
                &none,
                cx,
            );
            observe(
                "browser.open",
                &json!({ "project_id": 8, "url": "http://other" }),
                &none,
                cx,
            );
            let field =
                json!({ "element": { "role": "textbox", "name": "Search", "selector": "#q" } });
            observe(
                "browser.type",
                &json!({ "project_id": 7, "selector": "#q", "text": "a" }),
                &field,
                cx,
            );
            observe(
                "browser.type",
                &json!({ "project_id": 7, "selector": "#q", "text": "ab" }),
                &field,
                cx,
            );
            observe("browser.snapshot", &json!({ "project_id": 7 }), &none, cx);
            // A test the agent ran meanwhile: its clicks are not recorded.
            observe(
                "browser.click",
                &json!({ "project_id": 7, "selector": "#replayed", "quiet": true }),
                &none,
                cx,
            );
            // Another workspace's device tap, and a password (twice the
            // same one: it is one secret).
            observe(
                "device.tap",
                &json!({ "project_id": 8, "x": 1, "y": 2 }),
                &none,
                cx,
            );
            for selector in ["#pw", "#again"] {
                observe(
                    "browser.type",
                    &json!({ "project_id": 7, "selector": selector, "text": "hunter2" }),
                    &json!({ "secret": true }),
                    cx,
                );
            }
            for _ in 0..2 {
                observe(
                    "test.expect",
                    &json!({ "project_id": 7, "kind": "look" }),
                    &none,
                    cx,
                );
            }
            let recording = stop(cx).unwrap();
            let typed: Vec<String> = recording
                .steps
                .iter()
                .filter_map(|step| match step {
                    Step::Type { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                typed,
                ["ab", "{{secret:?0}}", "{{secret:?0}}"],
                "typing again replaces, and no password is kept in a step"
            );
            assert_eq!(recording.passwords, ["hunter2"]);
            assert_eq!(
                without_passwords("Sign in with hunter2, then go on", &recording.passwords),
                "Sign in with (the saved password), then go on"
            );
            assert_eq!(recording.steps.len(), 6);
            let looks: Vec<&str> = recording
                .steps
                .iter()
                .filter_map(|step| match step {
                    Step::ExpectLook { id, .. } => Some(id.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(looks, ["look-1", "look-2"]);
            assert!(current(cx).is_none());
        });
    }
}
