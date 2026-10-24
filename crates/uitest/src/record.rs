//! Recording: while an agent works through a test's scenario with the
//! browser or device tools, each action it takes successfully becomes a
//! step. Looking (snapshots, screenshots, logs) is not recorded; checks are
//! added with the `test_expect` tool, which also checks at once so the
//! agent knows it holds.

use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Result, bail};
use gpui_kit::{App, Global, Task};
use ph_control::ControlServer;
use serde_json::{Value, json};

use crate::model::Step;

/// The recording in progress, if any.
#[derive(Default)]
pub struct Recorder {
    pub session: Option<Recording>,
}

impl Global for Recorder {}

#[derive(Debug, Clone)]
pub struct Recording {
    pub project_id: i64,
    /// The test file the steps go into.
    pub path: PathBuf,
    pub steps: Vec<Step>,
}

/// Starts recording `project_id`'s browser and device actions, after the
/// steps already kept (`steps`, empty for a new recording).
pub fn start(project_id: i64, path: PathBuf, steps: Vec<Step>, cx: &mut App) {
    cx.default_global::<Recorder>().session = Some(Recording {
        project_id,
        path,
        steps,
    });
    ControlServer::set_observer(Some(Rc::new(observe)), cx);
}

/// Stops recording and returns it.
pub fn stop(cx: &mut App) -> Option<Recording> {
    ControlServer::set_observer(None, cx);
    cx.default_global::<Recorder>().session.take()
}

pub fn current(cx: &App) -> Option<&Recording> {
    cx.try_global::<Recorder>()?.session.as_ref()
}

fn observe(method: &str, params: &Value, cx: &mut App) {
    let Some(recording) = cx.default_global::<Recorder>().session.as_mut() else {
        return;
    };
    // Browser calls name their project; device calls act on the device in
    // view, which belongs to whoever is recording.
    if method.starts_with("browser.") && params["project_id"].as_i64() != Some(recording.project_id)
    {
        return;
    }
    if let Some(step) = step_for(method, params) {
        // Typing into the same field again replaces what was typed.
        if let (Some(Step::Type { selector: last, .. }), Step::Type { selector, .. }) =
            (recording.steps.last(), &step)
            && last == selector
            && selector.is_some()
        {
            recording.steps.pop();
        }
        recording.steps.push(step);
    }
}

/// The step a successful control call stands for; `None` for calls that
/// only look.
pub fn step_for(method: &str, params: &Value) -> Option<Step> {
    let text = |key: &str| params[key].as_str().map(str::to_string);
    let int = |key: &str| params[key].as_i64();
    Some(match method {
        "browser.open" => Step::Open { url: text("url")? },
        "browser.click" => match text("selector") {
            Some(selector) => Step::Click { selector },
            None => Step::ClickAt {
                x: params["x"].as_f64()?,
                y: params["y"].as_f64()?,
            },
        },
        "browser.type" => Step::Type {
            text: text("text")?,
            selector: text("selector"),
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
        },
        "device.swipe" => Step::Swipe {
            x1: int("x1")?,
            y1: int("y1")?,
            x2: int("x2")?,
            y2: int("y2")?,
        },
        "device.type" => Step::TypeText {
            text: text("text")?,
        },
        "device.key" => Step::Key { key: text("key")? },
        "test.expect" => expectation(params).ok()?,
        _ => return None,
    })
}

/// `{kind: "text" | "no_text" | "url", value}` as a check.
fn expectation(params: &Value) -> Result<Step> {
    let value = params["value"].as_str().unwrap_or_default().to_string();
    if value.is_empty() {
        bail!("`value` is required");
    }
    Ok(match params["kind"].as_str().unwrap_or("text") {
        "text" => Step::ExpectText { text: value },
        "no_text" => Step::ExpectNoText { text: value },
        "url" => Step::ExpectUrl { contains: value },
        other => bail!("unknown kind `{other}`: use text, no_text or url"),
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
        assert_eq!(
            step_for(
                "browser.click",
                &json!({ "selector": "#go", "project_id": 1 })
            ),
            Some(Step::Click {
                selector: "#go".into()
            })
        );
        assert_eq!(
            step_for("browser.click", &json!({ "x": 3.0, "y": 4.0 })),
            Some(Step::ClickAt { x: 3.0, y: 4.0 })
        );
        assert_eq!(step_for("browser.snapshot", &json!({})), None);
        assert_eq!(step_for("device.screenshot", &json!({})), None);
        assert_eq!(
            step_for(
                "device.swipe",
                &json!({ "x1": 1, "y1": 2, "x2": 3, "y2": 4 })
            ),
            Some(Step::Swipe {
                x1: 1,
                y1: 2,
                x2: 3,
                y2: 4
            })
        );
        assert_eq!(
            step_for("test.expect", &json!({ "kind": "url", "value": "/home" })),
            Some(Step::ExpectUrl {
                contains: "/home".into()
            })
        );
        assert_eq!(
            step_for("test.expect", &json!({ "kind": "nope", "value": "x" })),
            None
        );
    }

    #[gpui_kit::test]
    fn recording_keeps_the_projects_actions(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            start(7, PathBuf::from("t.json"), Vec::new(), cx);
            observe(
                "browser.open",
                &json!({ "project_id": 7, "url": "http://x" }),
                cx,
            );
            observe(
                "browser.open",
                &json!({ "project_id": 8, "url": "http://other" }),
                cx,
            );
            observe(
                "browser.type",
                &json!({ "project_id": 7, "selector": "#q", "text": "a" }),
                cx,
            );
            observe(
                "browser.type",
                &json!({ "project_id": 7, "selector": "#q", "text": "ab" }),
                cx,
            );
            observe("browser.snapshot", &json!({ "project_id": 7 }), cx);
            let recording = stop(cx).unwrap();
            assert_eq!(
                recording.steps,
                vec![
                    Step::Open {
                        url: "http://x".into()
                    },
                    Step::Type {
                        text: "ab".into(),
                        selector: Some("#q".into()),
                        submit: false
                    },
                ]
            );
            assert!(current(cx).is_none());
        });
    }
}
