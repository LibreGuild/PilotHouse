//! WebMCP: tools a website offers agents itself (`navigator.modelContext`),
//! such as "search flights" or "add to cart", called instead of clicking
//! through the page. Chrome offers them (the DevTools `WebMCP` domain) in
//! its early preview only; elsewhere the agent is told so.
//!
//! Every frame of the page can offer tools, so a tool is known by its frame
//! and name: an embedded frame (an ad, a widget) can neither replace nor
//! remove the site's own tool of the same name, and a call by name goes to
//! the page's own (top) frame first.

use super::*;

/// How long a site's tool may take.
const TOOL_TIMEOUT: Duration = Duration::from_secs(60);
/// Answers kept for calls that have not begun waiting yet (an answer can
/// arrive before `invokeTool` returns).
const MAX_UNCLAIMED: usize = 16;

/// How waiting for a tool's answer ended.
enum Waited {
    Answered(Value),
    /// The tab went (closed, or the extension disconnected).
    Closed,
    TimedOut,
}

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
            let (tools, main) = this.update(cx, |this, _| {
                this.tab(&page.target)
                    .map(|tab| (tab.site_tools.clone(), tab.main_frame.clone()))
                    .unwrap_or_default()
            })?;
            let Some((name, input)) = call else {
                let frames = frame_urls(&page.cdp).await;
                return Ok(json!({ "text": tools_text(&tools, main.as_deref(), &frames) }));
            };
            let tool = pick(&tools, &name, main.as_deref())
                .with_context(|| {
                    format!(
                        "the page offers no tool `{name}`: {}",
                        tools_text(&tools, main.as_deref(), &HashMap::new())
                    )
                })?
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
            // Without the tab the sender is dropped here, and the wait below
            // ends at once as closed.
            let early = this.update(cx, |this, _| {
                let tab = this.tab_mut(&page.target)?;
                match tab
                    .site_tool_answers
                    .iter()
                    .position(|answer| answer["invocationId"] == id.as_str())
                {
                    Some(at) => tab.site_tool_answers.remove(at),
                    None => {
                        tab.site_tool_waiters.push((id.clone(), sender));
                        None
                    }
                }
            })?;
            let waited = match early {
                Some(answer) => Waited::Answered(answer),
                None => {
                    let timeout = cx.background_executor().timer(TOOL_TIMEOUT);
                    smol::future::or(
                        async {
                            answered
                                .recv()
                                .await
                                .map_or(Waited::Closed, Waited::Answered)
                        },
                        async {
                            timeout.await;
                            Waited::TimedOut
                        },
                    )
                    .await
                }
            };
            let answer = match waited {
                Waited::Answered(answer) => answer,
                Waited::Closed => {
                    bail!("`{name}` got no answer: the tab closed or the extension disconnected")
                }
                Waited::TimedOut => {
                    this.update(cx, |this, _| {
                        if let Some(tab) = this.tab_mut(&page.target) {
                            tab.site_tool_waiters.retain(|(waiting, _)| *waiting != id);
                        }
                    })
                    .ok();
                    bail!("`{name}` did not answer within a minute");
                }
            };
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
                    if !tool.is_object() {
                        continue;
                    }
                    let mut tool = tool.clone();
                    tool["frameId"] = frame_of(&tool, params);
                    tab.site_tools.retain(|known| !same_tool(known, &tool));
                    tab.site_tools.push(tool);
                }
            }
            "WebMCP.toolsRemoved" => {
                for tool in params["tools"].as_array().into_iter().flatten() {
                    let mut tool = tool.clone();
                    if tool.is_object() {
                        tool["frameId"] = frame_of(&tool, params);
                    }
                    tab.site_tools.retain(|known| !same_tool(known, &tool));
                }
            }
            "WebMCP.toolResponded" => {
                let id = params["invocationId"].as_str().unwrap_or_default();
                let mut claimed = false;
                tab.site_tool_waiters.retain(|(waiting, answer)| {
                    if waiting == id {
                        claimed |= answer.try_send(params.clone()).is_ok();
                        return false;
                    }
                    !answer.is_closed()
                });
                if !claimed {
                    tab.site_tool_answers.push_back(params.clone());
                    if tab.site_tool_answers.len() > MAX_UNCLAIMED {
                        tab.site_tool_answers.pop_front();
                    }
                }
            }
            _ => {}
        }
    }
}

/// The frame a tool of an event belongs to: its own, or the event's.
fn frame_of(tool: &Value, params: &Value) -> Value {
    match &tool["frameId"] {
        Value::Null => params["frameId"].clone(),
        frame => frame.clone(),
    }
}

fn same_tool(known: &Value, tool: &Value) -> bool {
    known["name"] == tool["name"] && known["frameId"] == tool["frameId"]
}

/// The tool called `name`: the page's own (top frame) one when it has one,
/// else the first frame's that offers it.
fn pick<'a>(tools: &'a [Value], name: &str, main: Option<&str>) -> Option<&'a Value> {
    let named = || tools.iter().filter(|tool| tool["name"] == name);
    named()
        .find(|tool| main.is_some() && tool["frameId"].as_str() == main)
        .or_else(|| named().next())
}

/// Each frame's address, by frame id, to say where an embedded frame's
/// tools come from.
async fn frame_urls(cdp: &Cdp) -> HashMap<String, String> {
    fn walk(node: &Value, out: &mut HashMap<String, String>) {
        let frame = &node["frame"];
        if let (Some(id), Some(url)) = (frame["id"].as_str(), frame["url"].as_str()) {
            out.insert(id.to_string(), url.chars().take(200).collect());
        }
        for child in node["childFrames"].as_array().into_iter().flatten() {
            walk(child, out);
        }
    }
    let mut out = HashMap::new();
    if let Ok(tree) = cdp.call("Page.getFrameTree", json!({})).await {
        walk(&tree["frameTree"], &mut out);
    }
    out
}

fn tools_text(tools: &[Value], main: Option<&str>, frames: &HashMap<String, String>) -> String {
    if tools.is_empty() {
        return "This page offers no tools of its own (WebMCP). Use it as a person would.".into();
    }
    let lines: Vec<String> = tools
        .iter()
        .map(|tool| {
            let read_only = tool["annotations"]["readOnlyHint"] == true;
            let frame = tool["frameId"].as_str();
            // Tools of embedded frames are not the site's own: said so, with
            // where the frame comes from.
            let source = match frame {
                Some(frame) if Some(frame) != main => format!(
                    " [from an embedded frame: {}]",
                    frames.get(frame).map_or(frame, String::as_str)
                ),
                _ => String::new(),
            };
            format!(
                "- {}{}{source}: {}\n  input: {}",
                tool["name"].as_str().unwrap_or("?"),
                if read_only { " (reads only)" } else { "" },
                tool["description"].as_str().unwrap_or_default(),
                tool["inputSchema"]
            )
        })
        .collect();
    let mut names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    names.sort_unstable();
    let repeated = names.windows(2).any(|pair| pair[0] == pair[1]);
    format!(
        "The page offers these tools (call one with chrome_site_tools, name and input). Ask the user before calling one that changes something (orders, sends, pays, deletes):\n{}{}",
        lines.join("\n"),
        if repeated {
            "\nA name offered by several frames calls the page's own (top frame) tool when it has one."
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_known_by_frame_and_name() {
        let site = json!({ "name": "search", "frameId": "top", "description": "Site search" });
        let ad = json!({ "name": "search", "frameId": "ad", "description": "Ad search" });
        let tools = vec![ad.clone(), site.clone()];
        assert_eq!(pick(&tools, "search", Some("top")), Some(&site));
        assert_eq!(pick(&tools, "search", Some("elsewhere")), Some(&ad));
        assert_eq!(pick(&tools, "other", Some("top")), None);
        assert!(!same_tool(&site, &ad));
        assert_eq!(
            frame_of(&json!({ "name": "x" }), &json!({ "frameId": "f" })),
            json!("f")
        );
        let frames = HashMap::from([("ad".to_string(), "https://ads.example/w".to_string())]);
        let text = tools_text(&tools, Some("top"), &frames);
        assert!(text.contains("[from an embedded frame: https://ads.example/w]"));
        assert!(text.contains("calls the page's own"));
    }
}
