//! Workflows: steps the user recorded in Chrome (PilotHouse's extension:
//! *Record a workflow*), kept in PilotHouse's settings. Agents run them
//! again in their tab (`chrome_run_workflow`), a chat can turn one into a
//! Playwright test, and a schedule can run one regularly. Passwords and
//! card numbers were never recorded: a step that typed one says so, and an
//! agent asks the user for it.

use anyhow::{Result, bail};
use ph_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const KEY: &str = "chrome.workflows";
/// The most workflows kept.
const MAX: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workflow {
    pub id: u64,
    pub name: String,
    pub start_url: String,
    /// As the recorder wrote them: `click`, `fill`, `select`, `press`,
    /// `upload`, `navigate` (see the extension's `page/recorder.js`).
    pub steps: Vec<Value>,
    /// When it was recorded (Unix milliseconds).
    pub recorded_at: i64,
}

impl Workflow {
    /// The steps as words, one a line, numbered.
    pub fn describe(&self) -> String {
        self.steps
            .iter()
            .enumerate()
            .map(|(index, step)| format!("{}. {}", index + 1, describe_step(step)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A step as words: `Clicked “Save” (button#save)`.
pub fn describe_step(step: &Value) -> String {
    let text = step["text"].as_str().filter(|text| !text.is_empty());
    let what = text.map_or("an element".to_string(), |text| format!("“{text}”"));
    let selector = step["selector"].as_str().unwrap_or_default();
    match step["type"].as_str().unwrap_or_default() {
        "click" => format!("Click {what} ({selector})"),
        "fill" if step["secret"] == true => {
            format!("Type a password or secret into {what} ({selector}): ask the user")
        }
        "fill" => format!(
            "Type “{}” into {what} ({selector})",
            step["value"].as_str().unwrap_or_default()
        ),
        "select" => format!(
            "Choose “{}” in {what} ({selector})",
            step["value"].as_str().unwrap_or_default()
        ),
        "press" => format!("Press {}", step["key"].as_str().unwrap_or("a key")),
        "upload" => format!("Choose files for {what} ({selector})"),
        "navigate" => format!(
            "The page moves to {}",
            step["url"].as_str().unwrap_or_default()
        ),
        other => other.to_string(),
    }
}

pub fn list(store: &Store) -> Vec<Workflow> {
    store.setting(KEY)
}

pub fn find(store: &Store, name_or_id: &str) -> Option<Workflow> {
    let all = list(store);
    let wanted = name_or_id.trim();
    all.iter()
        .find(|workflow| workflow.id.to_string() == wanted)
        .or_else(|| {
            all.iter()
                .find(|workflow| workflow.name.eq_ignore_ascii_case(wanted))
        })
        .cloned()
}

/// Saves a recording under `name` (made unique: `Name 2`…); returns it.
pub fn save(store: &Store, name: &str, start_url: &str, steps: Vec<Value>) -> Result<Workflow> {
    if steps.is_empty() {
        bail!("nothing was recorded");
    }
    let mut all = list(store);
    let base: String = name.trim().chars().take(80).collect();
    let base = if base.is_empty() {
        "Workflow".to_string()
    } else {
        base
    };
    let name = (1..)
        .map(|n| match n {
            1 => base.clone(),
            n => format!("{base} {n}"),
        })
        .find(|candidate| {
            !all.iter()
                .any(|workflow| workflow.name.eq_ignore_ascii_case(candidate))
        })
        .unwrap_or(base);
    let workflow = Workflow {
        id: all.iter().map(|workflow| workflow.id).max().unwrap_or(0) + 1,
        name,
        start_url: start_url.chars().take(2000).collect(),
        steps,
        recorded_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_millis() as i64)
            .unwrap_or_default(),
    };
    all.insert(0, workflow.clone());
    all.truncate(MAX);
    store.set_setting(KEY, &all)?;
    Ok(workflow)
}

pub fn rename(store: &Store, id: u64, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("a workflow needs a name");
    }
    let mut all = list(store);
    if all
        .iter()
        .any(|workflow| workflow.id != id && workflow.name.eq_ignore_ascii_case(name))
    {
        bail!("another workflow is called “{name}”");
    }
    if let Some(workflow) = all.iter_mut().find(|workflow| workflow.id == id) {
        workflow.name = name.chars().take(80).collect();
    }
    store.set_setting(KEY, &all)
}

pub fn delete(store: &Store, id: u64) -> Result<()> {
    let mut all = list(store);
    all.retain(|workflow| workflow.id != id);
    store.set_setting(KEY, &all)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn workflows_are_saved_named_and_described() {
        let store = Store::open_in_memory().unwrap();
        let steps = vec![
            json!({ "type": "click", "selector": "#new", "text": "New invoice" }),
            json!({ "type": "fill", "selector": "#amount", "text": "Amount", "value": "40" }),
            json!({ "type": "fill", "selector": "#pw", "text": "Password", "secret": true }),
            json!({ "type": "press", "key": "Enter" }),
        ];
        let first = save(&store, "Invoices", "https://example.com", steps.clone()).unwrap();
        let second = save(&store, "invoices", "https://example.com", steps).unwrap();
        assert_eq!(second.name, "invoices 2");
        assert_eq!(find(&store, "INVOICES").unwrap().id, first.id);
        assert_eq!(
            find(&store, &second.id.to_string()).unwrap().name,
            "invoices 2"
        );
        let text = first.describe();
        assert!(text.contains("1. Click “New invoice” (#new)"));
        assert!(text.contains("Type “40” into “Amount”"));
        assert!(text.contains("ask the user"));
        assert!(rename(&store, second.id, "Invoices").is_err());
        rename(&store, second.id, "Monthly invoices").unwrap();
        delete(&store, first.id).unwrap();
        assert_eq!(list(&store).len(), 1);
        assert!(save(&store, "Empty", "", Vec::new()).is_err());
    }
}
