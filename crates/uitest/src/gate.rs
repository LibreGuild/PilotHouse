//! Tests as a gate: a board card that lists UI tests can only be Done once
//! each one's newest run passed, and agents can run tests themselves to get
//! there (`test.list`, `test.run`, behind the `test_list` and `test_run`
//! tools).

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{App, Task};
use ph_control::ControlServer;
use ph_store::Store;
use serde_json::{Value, json};

use crate::model::{self, TestCase};
use crate::run::{self, Report};

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
        return (false, format!("{different} steps look different"));
    }
    (true, "passed".into())
}

/// Why a card that needs `tests` may not be Done yet; `None` when every
/// one's newest run passed.
pub fn blocking(project_id: i64, tests: &[String]) -> Option<String> {
    let open: Vec<String> = tests
        .iter()
        .filter_map(|slug| {
            let (passed, status) = status(project_id, slug);
            // The test's own name, from its last run, reads better.
            let name = run::last_reports(project_id, slug)
                .first()
                .map_or_else(|| slug.clone(), |(_, report)| report.test.clone());
            (!passed).then(|| format!("“{name}” ({status})"))
        })
        .collect();
    (!open.is_empty()).then(|| {
        format!(
            "This card needs its tests to pass before it is Done: {}. Run them in the Tests tab (or with test_run), fix what fails, then try again.",
            open.join(", ")
        )
    })
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

pub fn register(store: Store, cx: &mut App) {
    let list_store = store.clone();
    ControlServer::handle(
        "test.list",
        move |params, _| {
            let project_id = params["project_id"]
                .as_i64()
                .context("missing `project_id`")?;
            let tests: Vec<Value> = model::list(&root(&list_store, project_id)?)
                .into_iter()
                .map(|(_, test)| {
                    let (passed, status) = status(project_id, &model::slug(&test.name));
                    json!({
                        "name": test.name, "description": test.description,
                        "steps": test.steps.len(), "passed": passed, "status": status,
                    })
                })
                .collect();
            Ok(json!({ "tests": tests }))
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
                let variant = (!test.is_android()).then(|| run::screens().remove(0));
                Ok(run::run(test, project_id, variant, dir, cx))
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
            Some(json!({ "step": ix + 1, "changed_percent": (visual.changed * 1000.0).round() / 10.0 }))
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
    fn a_test_that_never_ran_blocks() {
        let why = blocking(-1, &["sign-in-works".into()]).unwrap();
        assert!(why.contains("sign-in-works") && why.contains("not run yet"));
        assert!(blocking(-1, &[]).is_none(), "no tests, no gate");
    }
}
