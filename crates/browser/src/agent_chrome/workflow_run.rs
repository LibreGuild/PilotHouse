//! Running a recorded workflow ([`crate::workflows`]) in an agent's tab,
//! step by step with the agent's own actions (so the user sees them, with
//! Stop). It stops where a person is needed (a password, files to choose)
//! or a step fails, and says from which step to go on.

use std::collections::HashMap;

use super::*;
use crate::workflows::{self, Workflow, describe_step};

/// How long a step that moves to another page waits for it.
const PAGE_WAIT: Duration = Duration::from_secs(15);

impl AgentChrome {
    /// The saved workflows, newest first (for Settings). Reads the
    /// database: read again only when [`Self::workflows_version`] changed.
    pub fn workflows(&self) -> Vec<Workflow> {
        workflows::list(&self.store)
    }

    /// Changes whenever a workflow is saved, renamed or deleted.
    pub fn workflows_version(&self) -> u64 {
        self.workflows_version
    }

    pub(super) fn workflows_changed(&mut self, cx: &mut Context<Self>) {
        self.workflows_version += 1;
        cx.emit(AgentChromeEvent::Changed);
        cx.notify();
    }

    pub fn rename_workflow(&mut self, id: u64, name: &str, cx: &mut Context<Self>) -> Result<()> {
        workflows::rename(&self.store, id, name)?;
        self.workflows_changed(cx);
        Ok(())
    }

    pub fn delete_workflow(&mut self, id: u64, cx: &mut Context<Self>) -> Result<()> {
        workflows::delete(&self.store, id)?;
        self.workflows_changed(cx);
        Ok(())
    }

    /// The saved workflows, or one (`name`) with its steps.
    pub fn workflows_text(&self, name: Option<&str>) -> Result<Value> {
        if let Some(name) = name {
            let workflow = workflows::find(&self.store, name).with_context(|| {
                format!("no workflow is called “{name}”: chrome_workflows lists them")
            })?;
            return Ok(json!({ "text": format!(
                "“{}” starts at {} ({} steps):\n{}",
                workflow.name,
                workflow.start_url,
                workflow.steps.len(),
                workflow.describe()
            ) }));
        }
        let all = workflows::list(&self.store);
        if all.is_empty() {
            return Ok(
                json!({ "text": "No workflow is saved. The user records one in Chrome: right-click a page → PilotHouse → Record a workflow." }),
            );
        }
        let lines: Vec<String> = all
            .iter()
            .map(|workflow| {
                format!(
                    "- “{}”: {} steps from {}",
                    workflow.name,
                    workflow.steps.len(),
                    workflow.start_url
                )
            })
            .collect();
        Ok(
            json!({ "text": format!("Saved workflows (chrome_run_workflow runs one):\n{}", lines.join("\n")) }),
        )
    }

    /// Runs workflow `name` in the agent's tab from step `from` (1-based),
    /// with `values` in place of what was typed (by the field's label or
    /// selector).
    pub fn run_workflow(
        &mut self,
        who: &Who,
        name: String,
        from: usize,
        values: HashMap<String, String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let Some(workflow) = workflows::find(&self.store, &name) else {
            return Task::ready(Err(anyhow!(
                "no workflow is called “{name}”: chrome_workflows lists them"
            )));
        };
        let who = who.clone();
        cx.spawn(async move |this, cx| run(this, who, workflow, from.max(1), values, cx).await)
    }
}

async fn run(
    this: WeakEntity<AgentChrome>,
    who: Who,
    workflow: Workflow,
    from: usize,
    values: HashMap<String, String>,
    cx: &mut AsyncApp,
) -> Result<Value> {
    let mut done = Vec::new();
    let steps = &workflow.steps;
    if from == 1 {
        let opening = this.update(cx, |this, cx| {
            this.open(&who, &workflow.start_url, false, cx)
        })?;
        opening.await?;
        done.push(format!("Opened {}", workflow.start_url));
    }
    for (index, step) in steps.iter().enumerate().skip(from - 1) {
        let number = index + 1;
        let selector = step["selector"].as_str().map(str::to_string);
        let text = step["text"].as_str().unwrap_or_default().to_string();
        let value_for = |recorded: Option<&str>| {
            values
                .iter()
                .find(|(key, _)| {
                    key.eq_ignore_ascii_case(&text) || Some(key.as_str()) == selector.as_deref()
                })
                .map(|(_, value)| value.clone())
                .or_else(|| recorded.map(str::to_string))
        };
        let mut typed = None;
        let result: Result<()> = match step["type"].as_str().unwrap_or_default() {
            "click" => click(&this, &who, selector.clone(), &text, cx).await,
            "fill" => {
                let value = value_for(step["value"].as_str());
                match value {
                    None if step["secret"] == true => {
                        return Ok(stopped(
                            &workflow,
                            &done,
                            number,
                            &format!(
                                "it types a password or secret into “{text}”. Ask the user to type it in the tab themselves (agents never type passwords), then run the workflow again with from_step: {}.",
                                number + 1
                            ),
                        ));
                    }
                    value => {
                        let value = value.unwrap_or_default();
                        let element = json!({ "selector": selector });
                        let typing = this.update(cx, |this, cx| {
                            this.type_text(
                                &who,
                                value.clone(),
                                Some(element),
                                true,
                                false,
                                false,
                                cx,
                            )
                        })?;
                        typing.await.map(|_| typed = Some(value))
                    }
                }
            }
            "select" => {
                let value = value_for(step["value"].as_str()).unwrap_or_default();
                let choosing = this.update(cx, |this, cx| {
                    this.select(&who, json!({ "selector": selector }), vec![value], cx)
                })?;
                choosing.await.map(|_| ())
            }
            "press" => {
                let key = step["key"].as_str().unwrap_or("Enter").to_string();
                let pressing = this.update(cx, |this, cx| this.press(&who, key, cx))?;
                pressing.await.map(|_| ())
            }
            "upload" => {
                return Ok(stopped(
                    &workflow,
                    &done,
                    number,
                    &format!(
                        "it chooses files for “{text}”. Use chrome_upload with the files the user wants, then run the workflow again with from_step: {}.",
                        number + 1
                    ),
                ));
            }
            "navigate" => {
                let url = step["url"].as_str().unwrap_or_default().to_string();
                let previous = index
                    .checked_sub(1)
                    .and_then(|previous| steps[previous]["type"].as_str());
                // A move the previous step caused: wait for it. Otherwise
                // the user went there themselves: go there.
                if matches!(previous, Some("click" | "press")) {
                    let path = url_path(&url);
                    let waiting = this.update(cx, |this, cx| {
                        this.wait(&who, None, false, Some(path), false, PAGE_WAIT, cx)
                    })?;
                    // A page whose address differs (an id in it) still counts.
                    waiting.await.ok();
                    Ok(())
                } else {
                    let opening = this.update(cx, |this, cx| this.open(&who, &url, false, cx))?;
                    opening.await.map(|_| ())
                }
            }
            other => Err(anyhow!("a step of an unknown kind ({other})")),
        };
        match result {
            // What was typed, which `values` may have changed.
            Ok(()) => done.push(match typed.take() {
                Some(value) if step["secret"] != true => format!(
                    "Type “{value}” into “{text}” ({})",
                    selector.as_deref().unwrap_or_default()
                ),
                _ => describe_step(step),
            }),
            Err(err) => {
                return Ok(stopped(
                    &workflow,
                    &done,
                    number,
                    &format!(
                        "it failed: {err:#}. Look at the page (chrome_snapshot), do the step yourself if it moved, then run the workflow again with from_step: {}.",
                        number + 1
                    ),
                ));
            }
        }
    }
    let page = this.update(cx, |this, cx| this.snapshot(&who, cx))?.await?;
    let text: String = page["text"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(1500)
        .collect();
    Ok(json!({ "text": format!(
        "Ran “{}”: all {} steps.\n{}\n\nThe page now: {}\n{}\n\n{text}\n\n(chrome_snapshot lists its elements.)",
        workflow.name,
        steps.len(),
        numbered(&done),
        page["title"].as_str().unwrap_or_default(),
        page["url"].as_str().unwrap_or_default(),
    ) }))
}

/// Clicks by selector, then by the element's words when the selector is
/// gone (pages change their markup).
async fn click(
    this: &WeakEntity<AgentChrome>,
    who: &Who,
    selector: Option<String>,
    text: &str,
    cx: &mut AsyncApp,
) -> Result<()> {
    let mut last = Err(anyhow!("the step names no element"));
    if let Some(selector) = selector {
        let clicking = this.update(cx, |this, cx| {
            this.click(
                who,
                Target::Element(json!({ "selector": selector })),
                "left".into(),
                1,
                0,
                cx,
            )
        })?;
        last = clicking.await.map(|_| ());
        if last.is_ok() {
            return Ok(());
        }
    }
    if !text.is_empty() {
        let clicking = this.update(cx, |this, cx| {
            this.click(
                who,
                Target::Element(json!({ "text": text })),
                "left".into(),
                1,
                0,
                cx,
            )
        })?;
        last = clicking.await.map(|_| ());
    }
    last
}

fn stopped(workflow: &Workflow, done: &[String], number: usize, why: &str) -> Value {
    json!({ "text": format!(
        "Ran “{}” up to step {number} of {}, and stopped there: {why}\nDone so far:\n{}",
        workflow.name,
        workflow.steps.len(),
        numbered(done)
    ) })
}

fn numbered(done: &[String]) -> String {
    if done.is_empty() {
        return "(nothing yet)".into();
    }
    done.iter()
        .enumerate()
        .map(|(index, line)| format!("{}. {line}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// An address's path, which a page moved to keeps even when its query
/// differs.
fn url_path(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    without_query
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, path)| format!("/{path}"))
        .unwrap_or_else(|| without_query.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_of_addresses() {
        assert_eq!(url_path("https://x.dev/orders/42?tab=2"), "/orders/42");
        assert_eq!(url_path("https://x.dev"), "https://x.dev");
        assert_eq!(numbered(&[]), "(nothing yet)");
    }
}
