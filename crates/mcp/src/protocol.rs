//! The MCP/JSON-RPC envelope pieces the server needs.

use serde_json::{Value, json};

pub const PARSE_ERROR: i64 = -32700;
pub const METHOD_NOT_FOUND: i64 = -32601;

/// Protocol revisions this server understands, newest first.
const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

pub fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Which browser the user prefers agents to use, as the app says when a
/// chat starts (`browser.preference`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browsers {
    /// The user's own Chrome first. `connected`: its extension is there now.
    Chrome { connected: bool },
    /// The built-in browser first.
    BuiltIn,
}

impl Browsers {
    /// What the app answered. Chrome first when it could not be asked (the
    /// app is still starting): that is the setting's default, and a page
    /// opens in the built-in browser by itself when Chrome is not there.
    pub fn of(answer: Option<&Value>) -> Self {
        match answer.and_then(|answer| answer["preferred"].as_str()) {
            Some("built_in") => Browsers::BuiltIn,
            _ => Browsers::Chrome {
                connected: answer
                    .and_then(|answer| answer["chrome_connected"].as_bool())
                    .unwrap_or(true),
            },
        }
    }
}

/// Answers `initialize`, agreeing to the client's version when we know it.
pub fn initialize_result(params: &Value, browsers: Browsers) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|version| SUPPORTED_VERSIONS.contains(version))
        .unwrap_or(SUPPORTED_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "pilothouse", "version": env!("CARGO_PKG_VERSION") },
        "instructions": instructions(browsers),
    })
}

/// What the server tells agents about its main tools: only those of the
/// features this build has (the brand's `[features]`).
fn instructions(browsers: Browsers) -> String {
    use ph_brand::{Feature, enabled};
    let mut text = concat!(ph_brand::name!(), " tools for this project.").to_string();
    text.push_str(&web_pages(browsers));
    if enabled(Feature::Browser) {
        text.push_str(concat!(
            "\nBuilt-in browser: ",
            ph_brand::name!(),
            "'s own, shown to the user in the app (logins persist). It is what the user means \
            by the built-in, in-app or inbuilt browser, and only this server's browser_ tools \
            reach it: another server's browser, REPL or Playwright tool drives some other \
            browser, or fails. Use browser_open, then browser_snapshot to find elements and their selectors, \
            browser_click / browser_type / browser_press to act, and browser_screenshot to check \
            the result visually. browser_logs shows console errors and failed requests."
        ));
    }
    if enabled(Feature::Artifacts) {
        text.push_str(concat!(
            "\n",
            ph_brand::name!(),
            " artifacts: web pages ",
            ph_brand::name!(),
            " keeps for the user and serves from this computer (a planner, a dashboard, a small \
            game), listed under Artifacts in the app. When the user asks for a ",
            ph_brand::name!(),
            " artifact, or to publish something to ",
            ph_brand::name!(),
            ", read artifact_guide once, build the page in the workspace, then artifact_publish \
            it, and again after every change. They are this app's own: not Claude's or ChatGPT's \
            artifact or canvas, which only those tools make."
        ));
    }
    if enabled(Feature::Artifacts) && enabled(Feature::Presentations) {
        text.push_str(concat!(
            "\n",
            ph_brand::name!(),
            " Slides: a deck is a folder of web pages (deck.json, deck.css, one HTML file a slide) \
            that the user changes by hand, comments on, presents and exports to PowerPoint or \
            PDF. When the user asks for a presentation, a deck, slides or a PowerPoint in a \
            workspace's chat, make one with deck_create (not artifact_publish, and not a script \
            that writes a .pptx): it shows beside the chat as you write it. A deck made under \
            Artifacts → Slides has a chat of its own (each message there says so). Either \
            way you change the deck by editing its files: read deck_guide once first, call \
            deck_look after changing slides (it says what does not fit, what the user changed \
            by hand, and shows slides), answer the user's comments with deck_comments, turn a \
            .pptx into slides with deck_import, and export with deck_export only when asked. \
            deck_list shows a workspace's decks; `deck` names one when there are several."
        ));
    }
    if enabled(Feature::Devices) {
        text.push_str(
            "\nDevices: Android emulators and phones connected over adb. device_list first, then \
            device_screenshot and device_ui to see the screen, device_tap / device_swipe / \
            device_type / device_key to act, device_logcat for logs.",
        );
    }
    text
}

/// Which browser to open web pages in: the user's preference (Settings →
/// Agents), among the browsers this build has. Said once, here: the tools'
/// own descriptions say what each browser is, not which comes first.
fn web_pages(browsers: Browsers) -> String {
    use ph_brand::{Feature, enabled};
    let chrome = "the user's own Chrome (chrome_open, chrome_snapshot, chrome_click, \
        chrome_type, chrome_screenshot, chrome_console…)";
    match (enabled(Feature::ChromeExtension), enabled(Feature::Browser)) {
        (false, _) => String::new(),
        // No built-in browser to fall back on: the user connects the
        // extension instead.
        (true, false) => format!(
            "\nWeb pages: use {chrome}, for websites and the project's own dev server \
            (localhost) alike. When a chrome_ tool answers that the extension is not \
            connected, ask the user to set it up (Settings → Chrome extension)."
        ),
        (true, true) => match browsers {
            Browsers::Chrome { connected: true } => format!(
                "\nWeb pages: the user prefers their own Chrome. Use {chrome} first, for \
                websites and the project's own dev server (localhost) alike. If Chrome is not \
                connected, chrome_open opens the page in the built-in browser by itself and \
                says so: go on there with the browser_ tools."
            ),
            // Known not to be there as this chat starts: no call need find
            // it out. It may come later; chrome_open says where a page went.
            Browsers::Chrome { connected: false } => format!(
                "\nWeb pages: the user prefers their own Chrome, but its extension is not \
                connected now, so use the built-in browser (browser_ tools) for web pages. \
                Should a site need the user's sign-ins, ask them to set the extension up \
                (Settings → Chrome extension); once it is connected, {chrome} works."
            ),
            Browsers::BuiltIn => format!(
                "\nWeb pages: the user prefers the built-in browser. Use the browser_ tools \
                first, for websites and the project's own dev server (localhost) alike. Use \
                {chrome} only when the user asks for their Chrome, or a site needs their \
                sign-ins."
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_s_answer_says_which_browser_comes_first() {
        assert_eq!(Browsers::of(None), Browsers::Chrome { connected: true });
        assert_eq!(
            Browsers::of(Some(
                &json!({ "preferred": "chrome", "chrome_connected": false })
            )),
            Browsers::Chrome { connected: false }
        );
        assert_eq!(
            Browsers::of(Some(
                &json!({ "preferred": "built_in", "chrome_connected": true })
            )),
            Browsers::BuiltIn
        );
    }

    /// An agent is told which browser to reach for, and never to fail first
    /// to find out that Chrome is not there.
    #[test]
    fn instructions_follow_the_preferred_browser() {
        use ph_brand::{Feature, enabled};
        if !(enabled(Feature::ChromeExtension) && enabled(Feature::Browser)) {
            return;
        }
        let chrome = web_pages(Browsers::Chrome { connected: true });
        assert!(chrome.contains("prefers their own Chrome") && chrome.contains("first"));
        assert!(chrome.contains("opens the page in the built-in browser by itself"));
        let away = web_pages(Browsers::Chrome { connected: false });
        assert!(away.contains("not connected now") && away.contains("use the built-in browser"));
        let built_in = web_pages(Browsers::BuiltIn);
        assert!(built_in.contains("prefers the built-in browser"));
        assert!(built_in.contains("only when the user asks for their Chrome"));
    }
}
