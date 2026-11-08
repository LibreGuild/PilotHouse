//! What the extension brings from the user, and what PilotHouse tells it.
//!
//! - **From the user** (always their own action: the right-click menu, a
//!   shortcut, the side panel): things sent to a chat ([`Delivery`]: they go
//!   into a composer and wait for the user to send them), a chat to open,
//!   Stop and Continue for an agent.
//! - **To the extension**: the chats waiting for the user and the agents at
//!   work (the toolbar badge and the panel), news for Chrome notifications
//!   when the user chose those, and a tab handed to the user when an agent
//!   needs them in it (to sign in, or to pass a check that a person is there).
//!
//! The workspace shell says where things go ([`BrowserHost`]): this crate
//! does not know windows or chats.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use base64::Engine as _;
use ph_media::{CapturedImage, ToChat};
use serde::Serialize;

use super::*;

/// The most items one send carries.
const MAX_ITEMS: usize = 20;
/// The longest text item kept, in bytes.
const MAX_TEXT: usize = 1024 * 1024;
/// The largest image or video kept, in bytes (decoded).
const MAX_FILE: usize = 48 * 1024 * 1024;
/// An agent's tab is handed to the user at most this often.
const HANDOFF_EVERY: Duration = Duration::from_secs(60);

/// Where what the user sends from Chrome goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTo {
    /// The chat on screen in PilotHouse's window.
    Current,
    /// A new chat in the workspace on screen.
    New,
    /// Quick chat.
    Quick,
}

/// What the user sent from Chrome, for a composer.
pub struct Delivery {
    pub to: SendTo,
    /// PilotHouse's window comes forward.
    pub bring_forward: bool,
    /// The page it came from (title, address).
    pub title: String,
    pub source: String,
    pub items: Vec<ToChat>,
}

/// Puts a delivery in a composer; says where (`the chat in “app”`).
pub type DeliverFn = dyn Fn(Delivery, &mut App) -> Result<String>;
/// Opens the chat a notification's tag names.
pub type OpenChatFn = dyn Fn(&str, &mut App);
/// Turns a saved workflow into a test or a schedule (Settings).
pub type UseWorkflowFn = dyn Fn(crate::workflows::Workflow, WorkflowUse, &mut App);

/// What the user makes of a saved workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowUse {
    /// A chat drafts a Playwright test of it.
    Test,
    /// A schedule runs it.
    Schedule,
}

/// How the workspace shell takes what the extension brings.
#[derive(Clone)]
pub struct BrowserHost {
    pub deliver: Rc<DeliverFn>,
    pub open_chat: Rc<OpenChatFn>,
    pub use_workflow: Rc<UseWorkflowFn>,
}

/// A chat, for the extension's badge and panel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatNote {
    /// What [`BrowserHost::open_chat`] opens it by.
    pub tag: String,
    pub title: String,
    pub workspace: String,
    /// What it asks, or is doing.
    pub line: String,
}

/// News of a chat, for a Chrome notification (the extension shows it only
/// when the user chose to).
pub struct ChatNews {
    pub tag: String,
    pub title: String,
    pub body: String,
    /// `needs`, `done` or `failed`.
    pub kind: &'static str,
}

impl AgentChrome {
    /// The workspace shell says where things from the extension go.
    pub fn set_host(&mut self, host: BrowserHost) {
        self.host = Some(host);
    }

    pub fn host(&self) -> Option<BrowserHost> {
        self.host.clone()
    }

    /// The chats waiting for the user and at work; the extension hears of
    /// it (with the agents in its tabs) when anything changed.
    pub fn set_chats(&mut self, waiting: Vec<ChatNote>, working: Vec<ChatNote>) {
        self.chats = (waiting, working);
        self.push_status();
    }

    /// Chat news, for Chrome's notifications.
    pub fn news(&self, news: ChatNews) {
        self.tell_extension(
            "PilotHouse.news",
            json!({ "tag": news.tag, "title": news.title, "body": news.body, "kind": news.kind }),
        );
    }

    /// Tells the extension what it shows, when that changed.
    pub(super) fn push_status(&mut self) {
        let agents: Vec<Value> = self
            .owners
            .iter()
            .filter(|(_, owner)| !owner.tabs.is_empty())
            .map(|(key, owner)| {
                let tabs: Vec<Value> = owner
                    .tabs
                    .iter()
                    .filter_map(|target| self.tab(target))
                    .map(|tab| {
                        json!({
                            "tabId": tab_id(&tab.target).ok(),
                            "title": tab.title,
                            "url": tab.url,
                        })
                    })
                    .collect();
                json!({ "owner": key, "label": owner.label, "paused": owner.paused, "tabs": tabs })
            })
            .collect();
        let status = json!({
            "waiting": self.chats.0,
            "working": self.chats.1,
            "agents": agents,
        });
        if self.sent_status.as_ref() == Some(&status) || self.active.is_none() {
            return;
        }
        self.tell_extension("PilotHouse.status", status.clone());
        self.sent_status = Some(status);
    }

    /// A note to the connected extension (none expected back).
    pub(super) fn tell_extension(&self, method: &str, params: Value) {
        let Some(link) = self.active.and_then(|id| self.links.get(&id)) else {
            return;
        };
        let _ = bridge::send(
            &link.writer,
            json!({ "method": method, "params": params }).to_string(),
        );
    }

    /// Brings agent tab `target` to the user: an agent needs them there.
    pub(super) fn hand_off(&mut self, target: &str, agent: &str, reason: &str) {
        let now = Instant::now();
        if self
            .handed_off
            .get(target)
            .is_some_and(|at| now.duration_since(*at) < HANDOFF_EVERY)
        {
            return;
        }
        self.handed_off.insert(target.to_string(), now);
        let Ok(tab) = tab_id(target) else {
            return;
        };
        self.tell_extension(
            "PilotHouse.handoff",
            json!({ "tabId": tab, "agent": agent, "reason": reason }),
        );
    }

    /// A note from the extension that is not about a tab; true when it was.
    pub(super) fn extension_note(
        &mut self,
        method: &str,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        match method {
            "PilotHouse.ask" => {
                let reference = params["ref"].clone();
                let asked = params["method"].as_str().unwrap_or_default().to_string();
                self.answer(reference, &asked, params["params"].clone(), cx);
            }
            "PilotHouse.openChat" => {
                if let (Some(tag), Some(host)) = (params["tag"].as_str(), self.host.clone()) {
                    let tag = tag.chars().take(200).collect::<String>();
                    cx.defer(move |cx| (host.open_chat)(&tag, cx));
                }
            }
            "PilotHouse.agentControl" => {
                if let Some(owner) = params["owner"].as_str() {
                    let paused = params["paused"].as_bool().unwrap_or(true);
                    self.set_paused(owner, paused, cx);
                    self.push_status();
                }
            }
            _ => return false,
        }
        true
    }

    /// Answers the extension's question `reference` (a `PilotHouse.answer`).
    fn answer(&mut self, reference: Value, method: &str, params: Value, cx: &mut Context<Self>) {
        let reply = |this: &AgentChrome, result: Result<Value>| {
            let answer = match result {
                Ok(result) => json!({ "ref": reference, "result": result }),
                Err(err) => json!({ "ref": reference, "error": { "message": format!("{err:#}") } }),
            };
            this.tell_extension("PilotHouse.answer", answer);
        };
        match method {
            "send" => {
                let Some(host) = self.host.clone() else {
                    return reply(
                        self,
                        Err(anyhow!(
                            "{} is still starting. Try again in a moment.",
                            ph_brand::NAME
                        )),
                    );
                };
                let reference = reference.clone();
                cx.spawn(async move |this, cx| {
                    let delivery = cx
                        .background_spawn(async move {
                            delivery_of(
                                &params,
                                &ph_media::capture::captures_dir(),
                                &ph_media::capture::recordings_dir(),
                            )
                        })
                        .await;
                    let result = match delivery {
                        Ok(delivery) => cx
                            .update(|cx| (host.deliver)(delivery, cx))
                            .map(|place| json!({ "where": place })),
                        Err(err) => Err(err),
                    };
                    let _ = this.update(cx, |this, _| {
                        let answer = match result {
                            Ok(result) => json!({ "ref": reference, "result": result }),
                            Err(err) => json!({ "ref": reference, "error": { "message": format!("{err:#}") } }),
                        };
                        this.tell_extension("PilotHouse.answer", answer);
                    });
                })
                .detach();
            }
            "saveWorkflow" => {
                let steps = params["steps"]
                    .as_array()
                    .map(|steps| steps.iter().take(500).cloned().collect())
                    .unwrap_or_default();
                let saved = crate::workflows::save(
                    &self.store,
                    params["name"].as_str().unwrap_or("Workflow"),
                    params["startUrl"].as_str().unwrap_or_default(),
                    steps,
                )
                .map(|workflow| json!({ "id": workflow.id, "name": workflow.name }));
                reply(self, saved);
                cx.emit(AgentChromeEvent::Changed);
                cx.notify();
            }
            other => reply(
                self,
                Err(anyhow!(
                    "{} does not know `{other}`: update {}",
                    ph_brand::NAME,
                    ph_brand::NAME
                )),
            ),
        }
    }
}

/// Reads a `send` from the extension: texts, and images (and videos) saved
/// to PilotHouse's captures folder. Off the UI thread.
fn delivery_of(params: &Value, captures: &Path, recordings: &Path) -> Result<Delivery> {
    let to = match params["to"].as_str() {
        Some("new") => SendTo::New,
        Some("quick") => SendTo::Quick,
        _ => SendTo::Current,
    };
    let items = params["items"].as_array().context("nothing was sent")?;
    if items.is_empty() {
        bail!("nothing was sent");
    }
    let mut out = Vec::new();
    for (index, item) in items.iter().take(MAX_ITEMS).enumerate() {
        let label: String = item["label"]
            .as_str()
            .unwrap_or("From Chrome")
            .chars()
            .take(120)
            .collect();
        match item["kind"].as_str() {
            Some("text") => {
                let mut text = item["text"].as_str().unwrap_or_default().to_string();
                if text.len() > MAX_TEXT {
                    let mut cut = MAX_TEXT;
                    while !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    text.truncate(cut);
                }
                if !text.trim().is_empty() {
                    out.push(ToChat::Text { label, text });
                }
            }
            Some("image") => {
                let path = save(item["png"].as_str(), "png", index, captures)?;
                out.push(ToChat::Captured {
                    images: vec![CapturedImage { label, path }],
                    note: None,
                });
            }
            Some("video") => {
                let path = save(item["webm"].as_str(), "webm", index, recordings)?;
                out.push(ToChat::Text {
                    label,
                    text: format!("A screen recording of the tab, saved at {}", path.display()),
                });
            }
            _ => {}
        }
    }
    if out.is_empty() {
        bail!("nothing was sent");
    }
    Ok(Delivery {
        to,
        bring_forward: params["bringForward"].as_bool().unwrap_or(true),
        title: params["title"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect(),
        source: params["source"]["url"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(2000)
            .collect(),
        items: out,
    })
}

/// Decodes `data` (base64) into a new file in `dir`.
fn save(data: Option<&str>, extension: &str, index: usize, dir: &Path) -> Result<PathBuf> {
    let data = data.context("an item came without its data")?;
    if data.len() > MAX_FILE / 3 * 4 + 4 {
        bail!(
            "that is too large to send ({} MB at most)",
            MAX_FILE / 1024 / 1024
        );
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .context("an item's data is not base64")?;
    std::fs::create_dir_all(dir)?;
    let stamp = millis();
    let path = dir.join(format!("chrome-{stamp}-{index}.{extension}"));
    std::fs::write(&path, bytes).with_context(|| format!("saving {}", path.display()))?;
    Ok(path)
}

/// Milliseconds since the Unix epoch, for file names.
fn millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_send_becomes_chat_items() {
        let png = base64::engine::general_purpose::STANDARD.encode([137u8, 80, 78, 71]);
        let dir = std::env::temp_dir().join(format!("ph-inbox-{}", std::process::id()));
        let delivery = delivery_of(
            &json!({
                "to": "quick",
                "bringForward": false,
                "title": "Docs",
                "source": { "url": "https://example.com" },
                "items": [
                    { "kind": "text", "label": "Page: Docs", "text": "# Docs" },
                    { "kind": "image", "label": "Area of Docs", "png": png },
                    { "kind": "text", "label": "empty", "text": "  " },
                    { "kind": "unknown" }
                ]
            }),
            &dir,
            &dir,
        )
        .unwrap();
        assert_eq!(delivery.to, SendTo::Quick);
        assert!(!delivery.bring_forward);
        assert_eq!(delivery.items.len(), 2);
        let ToChat::Captured { images, .. } = &delivery.items[1] else {
            panic!("an image")
        };
        assert!(images[0].path.starts_with(&dir) && images[0].path.exists());
        assert!(delivery_of(&json!({ "items": [] }), &dir, &dir).is_err());
        assert!(
            delivery_of(
                &json!({ "items": [{ "kind": "image", "png": "%%%" }] }),
                &dir,
                &dir
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
