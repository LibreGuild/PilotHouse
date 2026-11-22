//! `browser.*` control-socket methods, used by the MCP bridge for agents.
//!
//! Every method takes a `project_id` and acts on that project's browser,
//! starting Chrome if it is not running yet.

use anyhow::{Context as _, Result};
use gpui_kit::{App, Context, Entity, Task};
use ph_control::ControlServer;
use serde_json::{Value, json};

use crate::browser::{Browser, ClickTarget, Emulation};
use crate::hub::BrowserHub;

pub fn register(cx: &mut App) {
    method("browser.open", cx, |browser, params, cx| {
        let url = string(&params, "url")?;
        Ok(browser.navigate(&url, cx))
    });
    // Which browser agents use first, and whether Chrome is there: asked by
    // the bridge as a chat starts, for the instructions it gives the agent.
    ControlServer::handle(
        "browser.preference",
        |_, cx| {
            let store = BrowserHub::store(cx);
            Ok(crate::preference::preference_note(&store, cx))
        },
        cx,
    );
    method("browser.screenshot", cx, |browser, params, cx| {
        Ok(browser.screenshot(params["full_page"].as_bool().unwrap_or(false), cx))
    });
    method("browser.click", cx, |browser, params, cx| {
        let target = match element(&params) {
            Some(target) => target,
            None => ClickTarget::Point(
                params["x"]
                    .as_f64()
                    .context("give `selector`, or `x` and `y`")? as f32,
                params["y"].as_f64().context("`y` is required with `x`")? as f32,
            ),
        };
        Ok(browser.click(target, wait(&params), cx))
    });
    method("browser.type", cx, |browser, params, cx| {
        let text = string(&params, "text")?;
        Ok(browser.type_text(
            text,
            element(&params),
            params["submit"].as_bool().unwrap_or(false),
            wait(&params),
            cx,
        ))
    });
    method("browser.press", cx, |browser, params, cx| {
        Ok(browser.press(string(&params, "key")?, cx))
    });
    method("browser.scroll", cx, |browser, params, cx| {
        Ok(browser.scroll(params["dy"].as_f64().unwrap_or(600.0) as f32, cx))
    });
    // `width` and `height` (and `mobile`) fix the page's screen size; without
    // them the page follows the pane again.
    method("browser.emulate", cx, |browser, params, cx| {
        let emulation = match (params["width"].as_u64(), params["height"].as_u64()) {
            (Some(width), Some(height)) if width >= 200 && height >= 200 => Some(Emulation {
                width: width as u32,
                height: height as u32,
                mobile: params["mobile"].as_bool().unwrap_or(false),
            }),
            (None, None) => None,
            _ => anyhow::bail!("give `width` and `height` (at least 200), or neither"),
        };
        browser.emulate(emulation, cx);
        Ok(Task::ready(Ok(json!({}))))
    });
    method("browser.clear_cache", cx, |browser, _, cx| {
        Ok(browser.clear_cache(cx))
    });
    // What the sites of `urls` keep (sign-ins, storage) and cached pages.
    method("browser.clear_sites", cx, |browser, params, cx| {
        let urls = params["urls"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|url| url.as_str().map(str::to_string))
            .collect();
        Ok(browser.clear_sites(urls, cx))
    });
    method("browser.text", cx, |browser, _, cx| Ok(browser.text(cx)));
    method("browser.evaluate", cx, |browser, params, cx| {
        Ok(browser.evaluate(string(&params, "expression")?, cx))
    });
    method("browser.snapshot", cx, |browser, _, cx| {
        Ok(browser.snapshot(cx))
    });

    // Where the pane is, without starting Chrome: a new UI test starts there.
    ControlServer::handle(
        "browser.where",
        |params, cx| {
            let project_id = params["project_id"]
                .as_i64()
                .context("missing `project_id`")?;
            // Made if there was none, which starts nothing: it knows where
            // the pane was when the app last ran.
            let url = BrowserHub::browser(project_id, cx)
                .read(cx)
                .url()
                .to_string();
            Ok(json!({ "url": url }))
        },
        cx,
    );

    // Logs are already in memory: answer at once, without starting Chrome.
    ControlServer::handle(
        "browser.logs",
        |params, cx| {
            let project_id = params["project_id"]
                .as_i64()
                .context("missing `project_id`")?;
            let limit = params["limit"].as_u64().unwrap_or(50) as usize;
            let errors_only = params["errors_only"].as_bool().unwrap_or(false);
            let Some(browser) = BrowserHub::existing(project_id, cx) else {
                return Ok(json!({ "text": "" }));
            };
            let browser = browser.read(cx);
            let text = match params["kind"].as_str() {
                Some("network") => browser.logs.network_text(limit, errors_only),
                _ => browser.logs.console_text(limit, errors_only),
            };
            Ok(json!({ "text": text, "url": browser.url() }))
        },
        cx,
    );
}

type BrowserMethod = fn(&mut Browser, Value, &mut Context<Browser>) -> Result<Task<Result<Value>>>;

/// Registers an async method that runs `body` on the project's browser.
/// Opens `url` in the built-in browser of the project `params` names: what
/// `browser.open` does, for a page asked for in Chrome while Chrome is not
/// connected (`chrome.open`).
pub(crate) fn open_in_built_in(params: &Value, cx: &mut App) -> Task<Result<Value>> {
    let Some(project_id) = params["project_id"].as_i64() else {
        return Task::ready(Err(anyhow::anyhow!("missing `project_id`")));
    };
    let url = match string(params, "url") {
        Ok(url) => url,
        Err(err) => return Task::ready(Err(err)),
    };
    BrowserHub::agent_uses(project_id, cx);
    let browser: Entity<Browser> = BrowserHub::browser(project_id, cx);
    browser.update(cx, |browser, cx| browser.navigate(&url, cx))
}

fn method(name: &'static str, cx: &mut App, body: BrowserMethod) {
    ControlServer::handle_async(
        name,
        move |params, cx| {
            let Some(project_id) = params["project_id"].as_i64() else {
                return Task::ready(Err(anyhow::anyhow!("missing `project_id`")));
            };
            // A UI test's run shows its own progress (`quiet`): the pane is
            // not brought in front of the Tests tab for it.
            if params["quiet"] != true {
                BrowserHub::agent_uses(project_id, cx);
            }
            let browser: Entity<Browser> = BrowserHub::browser(project_id, cx);
            match browser.update(cx, |browser, cx| body(browser, params, cx)) {
                Ok(task) => task,
                Err(err) => Task::ready(Err(err)),
            }
        },
        cx,
    );
}

/// The element a call names: as a UI test does (`target`), else by
/// `selector`.
fn element(params: &Value) -> Option<ClickTarget> {
    if params["target"].is_object() {
        return Some(ClickTarget::Element(params["target"].clone()));
    }
    params["selector"]
        .as_str()
        .map(|selector| ClickTarget::Selector(selector.to_string()))
}

/// How long a call waits for its element (`wait_ms`): not at all unless it
/// says, so an agent learns at once that its selector matches nothing.
fn wait(params: &Value) -> std::time::Duration {
    std::time::Duration::from_millis(params["wait_ms"].as_u64().unwrap_or(0).min(60_000))
}

fn string(params: &Value, key: &str) -> Result<String> {
    params[key]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("`{key}` is required"))
}
