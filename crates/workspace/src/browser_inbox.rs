//! PilotHouse's Chrome extension, seen from the workspaces: what the user
//! sends from Chrome goes into a composer (the chat on screen, a new chat,
//! or quick chat; nothing is sent to an agent until the user sends it), and
//! the extension hears which chats wait for the user or are at work (its
//! toolbar badge and side panel) and their news (Chrome notifications, when
//! the user chose those).

use anyhow::{Context as _, Result};
use gpui_kit::App;
use ph_agent::Attachment;
use ph_browser::workflows::Workflow;
use ph_browser::{AgentChrome, BrowserHost, ChatNews, ChatNote, Delivery, SendTo, WorkflowUse};
use ph_media::ToChat;
use std::rc::Rc;

/// Tells the extension's link where things go. Call once at startup.
pub(crate) fn install(cx: &mut App) {
    let Some(chrome) = AgentChrome::global(cx) else {
        return;
    };
    let host = BrowserHost {
        deliver: Rc::new(deliver),
        open_chat: Rc::new(crate::schedules::open_from_notification),
        use_workflow: Rc::new(|workflow, using, cx| {
            // After the Settings dialog this comes from has closed.
            cx.defer(move |cx| use_workflow(workflow, using, cx))
        }),
    };
    chrome.update(cx, |chrome, _| chrome.set_host(host));
}

/// The chats waiting for the user and at work, and news of them, for the
/// extension (it hears only of changes).
pub(crate) fn report(
    waiting: Vec<ChatNote>,
    working: Vec<ChatNote>,
    news: Vec<ChatNews>,
    cx: &mut App,
) {
    let Some(chrome) = AgentChrome::global(cx) else {
        return;
    };
    chrome.update(cx, |chrome, _| {
        chrome.set_chats(waiting, working);
        for news in news {
            chrome.news(news);
        }
    });
}

/// What a live pane or the extension hands over, as composer attachments.
pub(crate) fn attachments(items: Vec<ToChat>) -> Vec<Attachment> {
    let mut out = Vec::new();
    for item in items {
        match item {
            ToChat::Captured { images, note } => {
                let first = images.first().map(|image| image.label.clone());
                out.extend(images.into_iter().map(|image| Attachment::Image {
                    label: image.label,
                    path: image.path,
                }));
                out.extend(note.map(|note| Attachment::Text {
                    label: format!("About the {}", first.as_deref().unwrap_or("capture")),
                    text: note,
                }));
            }
            ToChat::Text { label, text } => out.push(Attachment::Text { label, text }),
        }
    }
    out
}

/// A saved workflow becomes a chat drafting its Playwright test, or a
/// schedule (its form filled in, for the user to choose when and save).
fn use_workflow(workflow: Workflow, using: WorkflowUse, cx: &mut App) {
    let Some((handle, workspace)) = crate::main_workspace(cx) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        window.activate_window();
        workspace.update(cx, |workspace, cx| match using {
            WorkflowUse::Test => {
                let attachment = Attachment::Text {
                    label: format!("Workflow: {}", workflow.name),
                    text: format!(
                        "A workflow recorded in Chrome, starting at {}:\n{}",
                        workflow.start_url,
                        workflow.describe()
                    ),
                };
                let prompt = "Write a Playwright test of this recorded workflow, where this project keeps its tests and following its setup (add Playwright if it has none, and say so). Check what each step should show, not only that it runs. Ask me for anything you cannot tell, such as passwords (use environment variables for them).";
                if let Err(err) = workspace.draft_from_browser(attachment, prompt, window, cx) {
                    tracing::warn!("drafting a test of a workflow: {err:#}");
                }
            }
            WorkflowUse::Schedule => {
                let Some(project) = workspace.active_project_id() else {
                    return;
                };
                let prompt = format!(
                    "Run the Chrome workflow “{}” with chrome_run_workflow, then tell me what the last page showed and anything that went wrong or looked different. If a step needs a password or a person, stop and tell me.",
                    workflow.name
                );
                crate::schedules::view::open_with(
                    cx.entity().downgrade(),
                    project,
                    Some((format!("Run “{}”", workflow.name), prompt)),
                    window,
                    cx,
                );
            }
        })
    });
}

/// Puts what the user sent from Chrome where they chose; says where.
fn deliver(delivery: Delivery, cx: &mut App) -> Result<String> {
    let attachments = attachments(delivery.items);
    if delivery.to == SendTo::Quick {
        crate::quick_chat::deliver(attachments, cx)?;
        return Ok("quick chat".into());
    }
    let (handle, workspace) = match crate::main_workspace(cx) {
        Some(main) => main,
        None => {
            let opened = crate::windows::open_window(None, cx)?;
            crate::set_main_window(opened.0, &opened.1, cx);
            opened
        }
    };
    let new_chat = delivery.to == SendTo::New;
    let bring_forward = delivery.bring_forward;
    handle
        .update(cx, |_, window, cx| {
            if bring_forward {
                cx.activate(true);
                window.activate_window();
            }
            workspace.update(cx, |workspace, cx| {
                workspace.receive_from_browser(new_chat, attachments, window, cx)
            })
        })
        .context("PilotHouse's window closed")?
}
