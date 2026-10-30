//! `chrome.*` control-socket methods: agents' Chrome ([`AgentChrome`]),
//! behind the MCP bridge's `chrome_*` tools.
//!
//! Every call names its agent session (`owner`, which the bridge sets) and
//! how to show it to the user (`agent`).

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{App, Context, Task};
use ph_control::ControlServer;
use serde_json::{Value, json};

use crate::agent_chrome::{AgentChrome, Target, Who};

/// The longest an agent may ask `chrome.wait` to wait.
const MAX_WAIT: Duration = Duration::from_secs(60);

pub fn register(cx: &mut App) {
    action("chrome.open", cx, |chrome, who, params, cx| {
        let url = string(&params, "url")?;
        Ok(chrome.open(&who, &url, params["new_tab"].as_bool().unwrap_or(false), cx))
    });
    action("chrome.history", cx, |chrome, who, params, cx| {
        Ok(chrome.history(&who, &string(&params, "action")?, cx))
    });
    action("chrome.tabs", cx, |chrome, who, params, cx| {
        let action = params["action"].as_str().unwrap_or("list").to_string();
        let index = params["index"].as_u64().map(|index| index as usize);
        let url = params["url"].as_str().map(str::to_string);
        let tab = params["tab"].as_str().map(str::to_string);
        Ok(chrome.tabs(&who, &action, index, url, tab, cx))
    });
    action("chrome.snapshot", cx, |chrome, who, _, cx| {
        Ok(chrome.snapshot(&who, cx))
    });
    action("chrome.screenshot", cx, |chrome, who, params, cx| {
        Ok(chrome.screenshot(
            &who,
            params["full_page"].as_bool().unwrap_or(false),
            element(&params),
            // Development builds only: how the glow looks.
            cfg!(debug_assertions) && params["with_overlay"].as_bool().unwrap_or(false),
            cx,
        ))
    });
    action("chrome.click", cx, |chrome, who, params, cx| {
        let button = params["button"].as_str().unwrap_or("left").to_string();
        if !["left", "right", "middle"].contains(&button.as_str()) {
            return Err(anyhow!("`button` is left, right or middle"));
        }
        let count = params["count"].as_u64().unwrap_or(1).clamp(1, 3) as u32;
        Ok(chrome.click(
            &who,
            target(&params)?,
            button,
            count,
            modifiers(&params)?,
            cx,
        ))
    });
    action("chrome.hover", cx, |chrome, who, params, cx| {
        Ok(chrome.hover(&who, target(&params)?, cx))
    });
    action("chrome.drag", cx, |chrome, who, params, cx| {
        let from = target(&params["from"]).context("`from`")?;
        let to = target(&params["to"]).context("`to`")?;
        Ok(chrome.drag(&who, from, to, cx))
    });
    action("chrome.type", cx, |chrome, who, params, cx| {
        Ok(chrome.type_text(
            &who,
            string(&params, "text")?,
            element(&params),
            params["clear"].as_bool().unwrap_or(false),
            params["slowly"].as_bool().unwrap_or(false),
            params["submit"].as_bool().unwrap_or(false),
            cx,
        ))
    });
    action("chrome.select", cx, |chrome, who, params, cx| {
        let element = element(&params).context("name the list: `ref`, `selector` or `text`")?;
        let values = match &params["values"] {
            Value::Array(values) => values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            Value::String(value) => vec![value.clone()],
            _ => {
                return Err(anyhow!(
                    "give `values`: the options to choose, by value or text"
                ));
            }
        };
        Ok(chrome.select(&who, element, values, cx))
    });
    action("chrome.press", cx, |chrome, who, params, cx| {
        Ok(chrome.press(&who, string(&params, "key")?, cx))
    });
    action("chrome.scroll", cx, |chrome, who, params, cx| {
        Ok(chrome.scroll(
            &who,
            params["dx"].as_f64().unwrap_or(0.0),
            params["dy"].as_f64().unwrap_or(600.0),
            element(&params),
            cx,
        ))
    });
    action("chrome.wait", cx, |chrome, who, params, cx| {
        let timeout = params["timeout_ms"]
            .as_u64()
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(10))
            .min(MAX_WAIT);
        Ok(chrome.wait(
            &who,
            element(&params),
            params["gone"].as_bool().unwrap_or(false),
            params["url"].as_str().map(str::to_string),
            params["network_idle"].as_bool().unwrap_or(false),
            timeout,
            cx,
        ))
    });
    action("chrome.evaluate", cx, |chrome, who, params, cx| {
        Ok(chrome.evaluate(&who, string(&params, "expression")?, cx))
    });
    action("chrome.request", cx, |chrome, who, params, cx| {
        Ok(chrome.request(
            &who,
            string(&params, "id")?,
            params["body"].as_bool().unwrap_or(false),
            cx,
        ))
    });
    action("chrome.dialog", cx, |chrome, who, params, cx| {
        let accept = params["accept"]
            .as_bool()
            .context("`accept`: true or false")?;
        Ok(chrome.dialog(
            &who,
            accept,
            params["text"].as_str().map(str::to_string),
            cx,
        ))
    });
    action("chrome.upload", cx, |chrome, who, params, cx| {
        let element =
            element(&params).context("name the file field: `ref`, `selector` or `text`")?;
        let paths = params["paths"]
            .as_array()
            .context("`paths`: the files to upload")?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        Ok(chrome.upload(&who, params["project_id"].as_i64(), element, paths, cx))
    });
    action("chrome.show", cx, |chrome, _, _, cx| {
        let shown = chrome.show(cx);
        Ok(cx.spawn(async move |_, _| {
            shown.await?;
            Ok(json!({ "text": "Chrome is in front." }))
        }))
    });

    // Already in memory: answered at once.
    read("chrome.console", cx, |chrome, who, params| {
        chrome.console(
            &who,
            params["level"].as_str().map(str::to_string),
            params["contains"].as_str().map(str::to_string),
            params["new_only"].as_bool().unwrap_or(false),
            params["limit"].as_u64().unwrap_or(100).clamp(1, 1000) as usize,
            params["clear"].as_bool().unwrap_or(false),
        )
    });
    read("chrome.network", cx, |chrome, who, params| {
        chrome.network(
            &who,
            params["errors_only"].as_bool().unwrap_or(false),
            params["contains"].as_str().map(str::to_string),
            params["kind"].as_str().map(str::to_string),
            params["new_only"].as_bool().unwrap_or(false),
            params["limit"].as_u64().unwrap_or(100).clamp(1, 1000) as usize,
        )
    });
    read("chrome.downloads", cx, |chrome, _, _| {
        Ok(chrome.downloads_text())
    });
}

type Action =
    fn(&mut AgentChrome, Who, Value, &mut Context<AgentChrome>) -> Result<Task<Result<Value>>>;
type Read = fn(&mut AgentChrome, Who, Value) -> Result<Value>;

fn action(name: &'static str, cx: &mut App, body: Action) {
    ControlServer::handle_async(
        name,
        move |params, cx| {
            let Some(chrome) = AgentChrome::global(cx) else {
                return Task::ready(Err(anyhow!("agents' Chrome is not set up")));
            };
            let who = who(&params);
            match chrome.update(cx, |chrome, cx| body(chrome, who, params, cx)) {
                Ok(task) => task,
                Err(err) => Task::ready(Err(err)),
            }
        },
        cx,
    );
}

fn read(name: &'static str, cx: &mut App, body: Read) {
    ControlServer::handle(
        name,
        move |params, cx| {
            let chrome = AgentChrome::global(cx).context("agents' Chrome is not set up")?;
            let who = who(&params);
            chrome.update(cx, |chrome, _| body(chrome, who, params))
        },
        cx,
    );
}

/// The agent session: the bridge's `owner` key, named by `agent`.
fn who(params: &Value) -> Who {
    let agent = params["agent"].as_str().unwrap_or("agent");
    Who {
        owner: params["owner"].as_str().unwrap_or(agent).to_string(),
        label: display_name(agent),
    }
}

/// `claude-code` → `Claude Code`.
fn display_name(agent: &str) -> String {
    agent
        .split(['-', '_', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// An element named by `ref`, `selector` or `text`, for the page script.
fn element(params: &Value) -> Option<Value> {
    let mut spec = serde_json::Map::new();
    for key in ["ref", "selector", "text"] {
        if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
            spec.insert(key.to_string(), value.clone());
        }
    }
    (!spec.is_empty()).then(|| Value::Object(spec))
}

/// An element, or a point (`x`, `y`).
fn target(params: &Value) -> Result<Target> {
    if let Some(element) = element(params) {
        return Ok(Target::Element(element));
    }
    match (params["x"].as_f64(), params["y"].as_f64()) {
        (Some(x), Some(y)) => Ok(Target::Point(x, y)),
        _ => Err(anyhow!(
            "name an element (`ref` from chrome_snapshot, `selector` or `text`), or give `x` and `y`"
        )),
    }
}

fn modifiers(params: &Value) -> Result<u32> {
    let mut bits = 0;
    for name in params["modifiers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        bits |= match name.to_ascii_lowercase().as_str() {
            "alt" | "option" => 1,
            "control" | "ctrl" => 2,
            "meta" | "cmd" | "command" => 4,
            "shift" => 8,
            other => {
                return Err(anyhow!(
                    "unknown modifier `{other}`: Alt, Control, Meta or Shift"
                ));
            }
        };
    }
    Ok(bits)
}

fn string(params: &Value, key: &str) -> Result<String> {
    params[key]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("`{key}` is required"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_targets() {
        assert_eq!(display_name("claude-code"), "Claude Code");
        assert_eq!(display_name("codex"), "Codex");
        let who = who(&json!({ "owner": "codex-42", "agent": "codex" }));
        assert_eq!(
            (who.owner.as_str(), who.label.as_str()),
            ("codex-42", "Codex")
        );
        assert!(matches!(
            target(&json!({ "ref": 3 })),
            Ok(Target::Element(_))
        ));
        assert!(matches!(
            target(&json!({ "x": 1, "y": 2 })),
            Ok(Target::Point(_, _))
        ));
        assert!(target(&json!({})).is_err());
        assert_eq!(
            modifiers(&json!({ "modifiers": ["Meta", "shift"] })).unwrap(),
            12
        );
    }
}
