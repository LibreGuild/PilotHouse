//! WebMCP: tools a website offers agents itself (`navigator.modelContext`),
//! such as "search flights" or "add to cart", called instead of clicking
//! through the page. Chrome offers them (the DevTools `WebMCP` domain) in
//! its early preview only; elsewhere the agent is told so.

use super::*;

/// How long a site's tool may take.
const TOOL_TIMEOUT: Duration = Duration::from_secs(60);

impl AgentChrome {
    /// The tools the page in the agent's tab offers (`list`), or one called
    /// with `input` (`call`).
    pub fn site_tools(
        &mut self,
        who: &Who,
        call: Option<(String, Value)>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Value>> {
        let page = self.page(who, false, false, cx);
        cx.spawn(async move |this, cx| {
            let page = page.await?;
            begin(&this, &page, cx).await;
            if page.cdp.call("WebMCP.enable", json!({})).await.is_err() {
                return Ok(json!({ "text": "This Chrome does not offer WebMCP (sites' own tools for agents) yet: it is in Chrome's early preview. Use the page as a person would." }));
            }
            // The page's tools arrive as events once enabled.
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let tools = this.update(cx, |this, _| {
                this.tab(&page.target).map(|tab| tab.site_tools.clone()).unwrap_or_default()
            })?;
            let Some((name, input)) = call else {
                return Ok(json!({ "text": tools_text(&tools) }));
            };
            let tool = tools
                .iter()
                .find(|tool| tool["name"] == name.as_str())
                .with_context(|| format!("the page offers no tool `{name}`: {}", tools_text(&tools)))?
                .clone();
            let (sender, answered) = smol::channel::bounded(1);
            let invoked = page
                .cdp
                .call(
                    "WebMCP.invokeTool",
                    json!({ "frameId": tool["frameId"], "toolName": name, "input": input }),
                )
                .await?;
            let id = invoked["invocationId"]
                .as_str()
                .context("the page did not take the call")?
                .to_string();
            this.update(cx, |this, _| {
                if let Some(tab) = this.tab_mut(&page.target) {
                    tab.site_tool_waiters.push((id.clone(), sender));
                }
            })?;
            let timeout = cx.background_executor().timer(TOOL_TIMEOUT);
            let answer = smol::future::or(async { answered.recv().await.ok() }, async {
                timeout.await;
                None
            })
            .await
            .with_context(|| format!("`{name}` did not answer within a minute"))?;
            let error = answer["exception"]["description"]
                .as_str()
                .or(answer["errorText"].as_str());
            let text = match error {
                Some(error) => format!("`{name}` failed ({}): {error}", answer["status"]),
                None => format!(
                    "`{name}` answered ({}):\n{}",
                    answer["status"],
                    serde_json::to_string_pretty(&answer["output"]).unwrap_or_default()
                ),
            };
            Ok(json!({ "text": text }))
        })
    }

    /// The page's tools changed, or one answered (`WebMCP.*` events).
    pub(super) fn site_tools_event(&mut self, index: usize, method: &str, params: &Value) {
        let tab = &mut self.tabs[index];
        match method {
            "WebMCP.toolsAdded" => {
                for tool in params["tools"].as_array().into_iter().flatten() {
                    tab.site_tools.retain(|known| known["name"] != tool["name"]);
                    tab.site_tools.push(tool.clone());
                }
            }
            "WebMCP.toolsRemoved" => {
                for tool in params["tools"].as_array().into_iter().flatten() {
                    tab.site_tools.retain(|known| known["name"] != tool["name"]);
                }
            }
            "WebMCP.toolResponded" => {
                let id = params["invocationId"].as_str().unwrap_or_default();
                tab.site_tool_waiters.retain(|(waiting, answer)| {
                    let done = waiting == id;
                    if done {
                        let _ = answer.try_send(params.clone());
                    }
                    !done && !answer.is_closed()
                });
            }
            _ => {}
        }
    }
}

fn tools_text(tools: &[Value]) -> String {
    if tools.is_empty() {
        return "This page offers no tools of its own (WebMCP). Use it as a person would.".into();
    }
    let lines: Vec<String> = tools
        .iter()
        .map(|tool| {
            let read_only = tool["annotations"]["readOnlyHint"] == true;
            format!(
                "- {}{}: {}\n  input: {}",
                tool["name"].as_str().unwrap_or("?"),
                if read_only { " (reads only)" } else { "" },
                tool["description"].as_str().unwrap_or_default(),
                tool["inputSchema"]
            )
        })
        .collect();
    format!(
        "The page offers these tools (call one with chrome_site_tools, name and input). Ask the user before calling one that changes something (orders, sends, pays, deletes):\n{}",
        lines.join("\n")
    )
}
