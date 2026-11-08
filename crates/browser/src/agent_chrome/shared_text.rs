//! The text of the tabs the user shared with PilotHouse in Chrome (the
//! PilotHouse tab group's tabs that are not an agent's), as Markdown, for a
//! chat's `@tabs`. Each tab is read in an isolated world of its own (the
//! page's scripts neither see nor change the reading), with Chrome's
//! debugger attached only for the read.

use super::*;

/// The page reader the extension uses for *Send page*.
const CLIP: &str = include_str!("../../extension/page/clip.js");
/// The most tabs read at once.
const MAX_TABS: usize = 8;
/// The most text kept of one tab, in bytes.
const MAX_TAB_TEXT: usize = 100 * 1024;

/// One shared tab's text.
#[derive(Debug, Clone)]
pub struct TabText {
    pub title: String,
    pub url: String,
    pub markdown: String,
}

impl AgentChrome {
    /// Reads the tabs the user shared (the one they are in first).
    pub fn shared_tab_texts(&self, cx: &mut Context<Self>) -> Task<Result<Vec<TabText>>> {
        let browser = match self.browser() {
            Ok(browser) => browser,
            Err(err) => return Task::ready(Err(err)),
        };
        let agents: Vec<String> = self.tabs.iter().map(|tab| tab.target.clone()).collect();
        cx.spawn(async move |_, _| {
            let tabs = shared_tabs(&browser, &agents).await?;
            let mut texts = Vec::new();
            for tab in tabs.into_iter().take(MAX_TABS) {
                let reading = read(&browser, &tab.target).await;
                let markdown = match reading {
                    Ok(markdown) => markdown,
                    Err(err) => format!("(This tab could not be read: {err:#})"),
                };
                texts.push(TabText {
                    title: tab.title,
                    url: tab.url,
                    markdown,
                });
            }
            Ok(texts)
        })
    }
}

/// One tab's text: attached, read in an isolated world, let go.
async fn read(browser: &Cdp, target: &str) -> Result<String> {
    browser
        .call("PilotHouse.attach", json!({ "tabId": tab_id(target)? }))
        .await
        .context("could not reach the tab")?;
    let page = browser.for_session(target);
    let result = async {
        let tree = page.call("Page.getFrameTree", json!({})).await?;
        let frame = tree["frameTree"]["frame"]["id"]
            .as_str()
            .context("the tab has no page")?
            .to_string();
        let world = page
            .call(
                "Page.createIsolatedWorld",
                json!({ "frameId": frame, "worldName": "pilothouse-read" }),
            )
            .await?;
        let context = world["executionContextId"]
            .as_i64()
            .context("no world to read in")?;
        let value = page
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": format!("{CLIP};globalThis.__pilothouse.clip({{}})"),
                    "contextId": context,
                    "returnByValue": true,
                }),
            )
            .await?;
        let mut markdown = value["result"]["value"]["markdown"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if markdown.len() > MAX_TAB_TEXT {
            let mut cut = MAX_TAB_TEXT;
            while !markdown.is_char_boundary(cut) {
                cut -= 1;
            }
            markdown.truncate(cut);
            markdown.push_str("\n\n(The page goes on; this is its start.)");
        }
        anyhow::Ok(markdown)
    }
    .await;
    browser
        .call("PilotHouse.detach", json!({ "tabId": tab_id(target)? }))
        .await
        .ok();
    result
}
