//! Tests for agents: they list a workspace's UI tests with how each one's
//! newest run went, and run one themselves to check a change (`test.list`,
//! `test.run`, behind the `test_list` and `test_run` tools). And they make
//! one when asked to in a chat: `test.record` starts a new test and its
//! recording, `test.save` ends it (the `test_record` and `test_save`
//! tools), with the agent's own browser or device actions in between.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{App, AppContext as _, Task};
use ph_control::ControlServer;
use ph_store::Store;
use serde_json::{Value, json};

use crate::model::{self, Step, Target, TestCase};
use crate::record;
use crate::run::{self, Report};
use crate::runner::TestRunHub;

/// A test's newest run in a few words: passed, where it failed, how many
/// steps look different, or that it never ran.
pub fn status(project_id: i64, slug: &str) -> (bool, String) {
    let reports = run::last_reports(project_id, slug);
    if reports.is_empty() {
        return (false, "not run yet".into());
    }
    let reports: Vec<&Report> = reports.iter().map(|(_, report)| report).collect();
    if let Some(report) = reports.iter().find(|report| report.failure().is_some()) {
        let step = report.failure().map_or(0, |(ix, _)| ix + 1);
        return (false, format!("failed at step {step}"));
    }
    let different: usize = reports.iter().map(|report| report.looks_different()).sum();
    if different > 0 {
        return (false, format!("{different} screens look different"));
    }
    (true, "passed".into())
}

/// The workspace's tests folder root (its primary folder).
fn root(store: &Store, project_id: i64) -> Result<PathBuf> {
    Ok(store
        .project(project_id)?
        .with_context(|| format!("no workspace with id {project_id}"))?
        .path)
}

/// The test named `name` (its name or file name, any case).
fn find(root: &std::path::Path, name: &str) -> Result<TestCase> {
    let wanted = model::slug(name);
    let tests = model::list(root);
    tests
        .iter()
        .find(|(_, test)| model::slug(&test.name) == wanted)
        .map(|(_, test)| test.clone())
        .ok_or_else(|| {
            let names: Vec<&str> = tests.iter().map(|(_, test)| test.name.as_str()).collect();
            anyhow!(
                "There is no UI test named “{name}”. Tests: {}",
                if names.is_empty() {
                    "none yet".into()
                } else {
                    names.join(", ")
                }
            )
        })
}

/// The test an agent asks to record: what it checks, and where it starts
/// (`url` for a page, `package` for an Android app). Named by `name`, else
/// by the first words of what it checks.
fn new_test(params: &Value) -> Result<TestCase> {
    let text = |key: &str| params[key].as_str().unwrap_or_default().trim().to_string();
    let description = text("checks");
    if description.is_empty() {
        anyhow::bail!("say what the test checks, in `checks`");
    }
    let (url, package) = (text("url"), text("package"));
    let target = match (url.is_empty(), package.is_empty()) {
        (false, true) => Target::Web {
            url: if url.contains("://") {
                url
            } else {
                format!("http://{url}")
            },
        },
        (true, false) => Target::Android { package },
        _ => anyhow::bail!(
            "say where it starts: `url` for a page, or `package` for an Android app (one of them)"
        ),
    };
    let name = match text("name") {
        name if name.is_empty() => model::name_from(&description),
        name => name,
    };
    Ok(TestCase {
        name,
        description,
        target,
        clean: None,
        steps: Vec::new(),
    })
}

/// The Tests tab of workspace `project_id`, if it shows, reads its tests
/// again.
fn tests_changed(project_id: i64, cx: &mut App) {
    TestRunHub::runs(project_id, cx).update(cx, |runs, cx| runs.changed(cx));
}

pub fn register(store: Store, cx: &mut App) {
    let record_store = store.clone();
    // A new test, and its recording: what the agent then does in the
    // browser or on the device becomes its steps.
    ControlServer::handle_async(
        "test.record",
        move |params, cx| {
            let started = (|| {
                let project_id = params["project_id"]
                    .as_i64()
                    .context("missing `project_id`")?;
                let test = new_test(&params)?;
                // One still going in this workspace was not ended (an agent
                // in a terminal has no turn that ends it): it is saved, as
                // its agent meant it to be. Another workspace's is its own.
                let earlier = match record::current(cx) {
                    Some(recording) if recording.project_id == project_id => record::stop(cx),
                    Some(_) => anyhow::bail!(
                        "A test is being recorded in another workspace: it has to end first."
                    ),
                    None => None,
                };
                let path = model::create(&root(&record_store, project_id)?, &test)?;
                Ok((project_id, path, test, earlier))
            })();
            let (project_id, path, test, earlier) = match started {
                Ok(started) => started,
                Err(err) => return Task::ready(Err(err)),
            };
            cx.spawn(async move |cx| {
                if let Some(earlier) = earlier {
                    let _ = cx
                        .background_spawn(async move { record::write_down(earlier) })
                        .await;
                }
                let begin = cx.update(|cx| record::begin(project_id, path.clone(), &test, cx));
                if let Err(err) = begin.await {
                    // Nothing was recorded: no empty test is left behind.
                    let _ = std::fs::remove_file(&path);
                    return Err(err);
                }
                cx.update(|cx| tests_changed(project_id, cx));
                Ok(json!({
                    "name": test.name,
                    "file": path.to_string_lossy(),
                    "android": test.is_android(),
                }))
            })
        },
        cx,
    );
    ControlServer::handle_async(
        "test.save",
        |params, cx| {
            let project_id = params["project_id"].as_i64().unwrap_or_default();
            if !record::current(cx).is_some_and(|recording| recording.project_id == project_id) {
                return Task::ready(Err(anyhow!(
                    "No test is being recorded in this workspace: start one with test_record."
                )));
            }
            let Some(recording) = record::stop(cx) else {
                return Task::ready(Err(anyhow!("No test is being recorded.")));
            };
            cx.spawn(async move |cx| {
                // The credential store and the file: off the UI thread.
                let (path, steps) = cx
                    .background_spawn(async move { record::write_down(recording) })
                    .await?;
                cx.update(|cx| tests_changed(project_id, cx));
                let name = model::load(&path).map(|test| test.name).unwrap_or_default();
                Ok(json!({
                    "name": name,
                    "steps": steps.iter().map(Step::describe).collect::<Vec<_>>(),
                }))
            })
        },
        cx,
    );

    let list_store = store.clone();
    ControlServer::handle(
        "test.list",
        move |params, _| {
            let project_id = params["project_id"]
                .as_i64()
                .context("missing `project_id`")?;
            let root = root(&list_store, project_id)?;
            let tests: Vec<Value> = model::list(&root)
                .into_iter()
                .map(|(_, test)| {
                    let (passed, status) = status(project_id, &model::slug(&test.name));
                    json!({
                        "name": test.name, "description": test.description,
                        "steps": test.steps.len(), "passed": passed, "status": status,
                    })
                })
                .collect();
            // The project's own suites, which the agent runs in its shell:
            // the command, and how the last run from the Tests tab went.
            let suites: Vec<Value> = crate::suite::detect(&root)
                .into_iter()
                .map(|suite| {
                    let result = crate::suite::last_result(project_id, &suite.id);
                    json!({
                        "name": suite.name, "command": suite.command,
                        "status": result.as_ref().map_or_else(
                            || "not run from the Tests tab yet".to_string(),
                            crate::suite_run::summary,
                        ),
                    })
                })
                .collect();
            Ok(json!({ "tests": tests, "suites": suites }))
        },
        cx,
    );
    ControlServer::handle_async(
        "test.run",
        move |params, cx| {
            let started = (|| {
                let project_id = params["project_id"]
                    .as_i64()
                    .context("missing `project_id`")?;
                let name = params["name"].as_str().context("missing `name`")?;
                let test = find(&root(&store, project_id)?, name)?;
                if test.steps.is_empty() {
                    anyhow::bail!(
                        "“{}” has no steps yet; record it in the Tests tab first.",
                        test.name
                    );
                }
                let dir = run::report_dir(project_id, &model::slug(&test.name));
                Ok(run::run_once(test, project_id, dir, cx))
            })();
            match started {
                Ok(task) => cx.spawn(async move |_| Ok(report_json(&task.await?))),
                Err(err) => Task::ready(Err(err)),
            }
        },
        cx,
    );
}

/// A run as the agent sees it: passed or not, and what went wrong.
fn report_json(report: &Report) -> Value {
    let failure = report.failure().map(|(ix, step)| {
        json!({
            "step": ix + 1,
            "does": step.step.describe(),
            "error": step.error,
        })
    });
    let different: Vec<Value> = report
        .steps
        .iter()
        .enumerate()
        .filter_map(|(ix, step)| {
            let visual = step.visual.as_ref().filter(|visual| visual.differs())?;
            Some(json!({ "step": ix + 1, "changed_pixels": visual.pixels }))
        })
        .collect();
    json!({
        "test": report.test,
        "passed": report.passed(),
        "failure": failure,
        "looks_different": different,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_says_what_a_new_test_checks_and_where_it_starts() {
        let page = new_test(&json!({
            "checks": "Sign in as Ada, then add a coffee; the cart should say Coffee.",
            "url": "localhost:3000/login"
        }))
        .unwrap();
        assert_eq!(page.name, "Sign in as Ada");
        assert_eq!(
            page.target,
            Target::Web {
                url: "http://localhost:3000/login".into()
            }
        );
        let app = new_test(&json!({
            "name": "Settings open", "checks": "Open the settings", "package": "com.example.app"
        }))
        .unwrap();
        assert_eq!(
            (app.name.as_str(), app.is_android()),
            ("Settings open", true)
        );
        // Neither, both, or nothing to check: said, not guessed.
        assert!(new_test(&json!({ "checks": "x" })).is_err());
        assert!(new_test(&json!({ "checks": "x", "url": "a", "package": "b.c" })).is_err());
        assert!(new_test(&json!({ "url": "http://x" })).is_err());
    }

    #[test]
    fn a_test_that_never_ran_says_so() {
        assert_eq!(status(-1, "sign-in-works"), (false, "not run yet".into()));
    }
}
