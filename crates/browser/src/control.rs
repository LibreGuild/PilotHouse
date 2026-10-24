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
    method("browser.screenshot", cx, |browser, params, cx| {
        Ok(browser.screenshot(params["full_page"].as_bool().unwrap_or(false), cx))
    });
    method("browser.click", cx, |browser, params, cx| {
        let target = match params["selector"].as_str() {
            Some(selector) => ClickTarget::Selector(selector.to_string()),
            None => ClickTarget::Point(
                params["x"]
                    .as_f64()
                    .context("give `selector`, or `x` and `y`")? as f32,
                params["y"].as_f64().context("`y` is required with `x`")? as f32,
            ),
        };
        Ok(browser.click(target, cx))
    });
    method("browser.type", cx, |browser, params, cx| {
        let text = string(&params, "text")?;
        let selector = params["selector"].as_str().map(str::to_string);
        Ok(browser.type_text(
            text,
            selector,
            params["submit"].as_bool().unwrap_or(false),
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
    method("browser.evaluate", cx, |browser, params, cx| {
        Ok(browser.evaluate(string(&params, "expression")?, cx))
    });
    method("browser.snapshot", cx, |browser, _, cx| {
        Ok(browser.snapshot(cx))
    });

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
fn method(name: &'static str, cx: &mut App, body: BrowserMethod) {
    ControlServer::handle_async(
        name,
        move |params, cx| {
            let Some(project_id) = params["project_id"].as_i64() else {
                return Task::ready(Err(anyhow::anyhow!("missing `project_id`")));
            };
            let browser: Entity<Browser> = BrowserHub::browser(project_id, cx);
            match browser.update(cx, |browser, cx| body(browser, params, cx)) {
                Ok(task) => task,
                Err(err) => Task::ready(Err(err)),
            }
        },
        cx,
    );
}

fn string(params: &Value, key: &str) -> Result<String> {
    params[key]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("`{key}` is required"))
}
