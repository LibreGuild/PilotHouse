//! What the tools send to the chat: code to ask about, captures,
//! screenshots and logs of live panes, architecture and parallel prompts.

use super::*;

impl WorkspaceScreen {
    /// Selected code goes to the chat: attached, with a prompt for the
    /// chosen intent (or left for the user to finish typing).
    pub(super) fn ask_about_code(
        &mut self,
        ask: ph_files::Ask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_chat(cx);
        let attachment = Attachment::Text {
            label: ask.label(),
            text: ask.attachment_text(),
        };
        let prompt = ask.prompt().map(|prompt| (ask.title(), prompt));
        self.chat.update(cx, |chat, cx| {
            chat.ask_about(attachment, prompt, window, cx)
        });
        cx.notify();
    }

    /// Captures, picked elements and log lines from a live pane.
    pub(super) fn receive_to_chat(
        &mut self,
        item: &ToChat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let attachments = match item {
            ToChat::Captured { images, note } => images
                .iter()
                .map(|image| Attachment::Image {
                    label: image.label.clone(),
                    path: image.path.clone(),
                })
                .chain(note.iter().map(|note| Attachment::Text {
                    label: format!(
                            "About the {}",
                            images
                                .first()
                                .map_or("capture", |image| image.label.as_str())
                        ),
                    text: note.clone(),
                }))
                .collect(),
            ToChat::Text { label, text } => vec![Attachment::Text {
                label: label.clone(),
                text: text.clone(),
            }],
        };
        self.attach_to_chat(attachments, window, cx);
    }

    /// Adds attachments to the composer, bringing the chat into view.
    pub(super) fn attach_to_chat(
        &mut self,
        attachments: Vec<Attachment>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_chat(cx);
        self.chat.update(cx, |chat, cx| {
            for attachment in attachments {
                chat.attach(attachment, window, cx);
            }
        });
        cx.notify();
    }

    /// ⌘⇧S: send the visible browser or device (with its marks) to chat.
    pub fn capture_to_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tools = match self.context_tab {
            _ if !self.context_open => None,
            ContextTab::Browser => self
                .browser
                .as_ref()
                .map(|browser| browser.read(cx).tools().clone()),
            ContextTab::Devices => self
                .devices
                .as_ref()
                .map(|devices| devices.read(cx).tools().clone()),
            _ => None,
        };
        match tools {
            Some(tools) => tools.update(cx, |tools, cx| tools.capture(cx)),
            None => {
                if let Some(attachment) = self.live_screenshot(cx) {
                    self.attach_to_chat(vec![attachment], window, cx);
                }
            }
        }
    }

    /// For `@screenshot`: the visible live pane first, else whichever of the
    /// browser and device has a picture.
    pub(super) fn live_screenshot(&self, cx: &gpui_kit::App) -> Option<Attachment> {
        let browser = || Some(("browser", self.browser.as_ref()?.read(cx).composite(cx)?));
        let device = || Some(("device", self.devices.as_ref()?.read(cx).composite(cx)?));
        let (source, frame) = if self.context_tab == ContextTab::Devices {
            device().or_else(browser)
        } else {
            browser().or_else(device)
        }?;
        let path = ph_media::capture::save_png(&frame, source).ok()?;
        Some(Attachment::Image {
            label: format!("{source} screenshot"),
            path,
        })
    }

    /// For `@log`: browser console errors and failed requests, and logcat errors.
    pub(super) fn live_logs(&self, cx: &gpui_kit::App) -> Option<Attachment> {
        let browser = self
            .browser
            .as_ref()
            .and_then(|browser| browser.read(cx).recent_logs(cx));
        let device = self
            .devices
            .as_ref()
            .and_then(|devices| devices.read(cx).recent_logs(cx));
        let text: Vec<String> = browser.into_iter().chain(device).collect();
        (!text.is_empty()).then(|| Attachment::Text {
            label: "logs".into(),
            text: text.join("\n"),
        })
    }

    /// Hands the architecture map to the chat, asking for an explanation.
    pub fn explain_architecture(
        &mut self,
        map: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_chat(cx);
        let attachment = Attachment::Text {
            label: "Architecture map".into(),
            text: map,
        };
        let prompt = "Using the attached map of this project's parts and the code itself, explain its architecture: \
                      what each part is for, how they work together (the main flows of data and control), and where a \
                      new contributor should start. Be concise; use a short list per part."
            .to_string();
        self.chat.update(cx, |chat, cx| {
            chat.ask_about(
                attachment,
                Some(("Explain the architecture".into(), prompt)),
                window,
                cx,
            )
        });
    }

    /// Starts the chats of a parallel run whose worktrees are ready.
    pub fn run_parallel(
        &mut self,
        prompt: &str,
        agents: Vec<&'static ph_agent::agents::AgentSpec>,
        lanes: Vec<(i64, std::path::PathBuf)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_chat(cx);
        let title: String = prompt
            .lines()
            .next()
            .unwrap_or(prompt)
            .chars()
            .take(50)
            .collect();
        let starts = agents
            .iter()
            .copied()
            .zip(lanes.iter().map(|(_, path)| path.clone()))
            .collect();
        let sessions = self.chat.update(cx, |chat, cx| {
            chat.start_parallel(starts, &title, prompt, window, cx)
        });
        let store = AppState::store(cx);
        for ((lane, _), session) in lanes.iter().zip(sessions) {
            if let Some(session) = session {
                let _ = store.set_lane_session(*lane, session);
            }
        }
        self.reload_sessions(cx);
        let names: Vec<_> = agents.iter().map(|agent| agent.name).collect();
        window.push_notification(
            Notification::info(format!(
                "{} are working side by side. Compare and keep one in Parallel runs (⌘K).",
                names.join(" and ")
            )),
            cx,
        );
    }
}

/// `@test-run`: how each UI test of the workspace last went, and the screen
/// of the first failing step.
pub(super) fn test_results(
    project_id: ph_store::projects::ProjectId,
    root: &std::path::Path,
) -> Vec<ph_agent::Attachment> {
    let tests = ph_uitest::model::list(root);
    if tests.is_empty() {
        return vec![ph_agent::Attachment::Text {
            label: "UI tests".into(),
            text: "This workspace has no UI tests yet.".into(),
        }];
    }
    let mut lines = vec!["UI tests in this workspace, by their newest run:".to_string()];
    let mut failing_screen = None;
    for (_, test) in &tests {
        let slug = ph_uitest::model::slug(&test.name);
        let reports = ph_uitest::run::last_reports(project_id, &slug);
        if reports.is_empty() {
            lines.push(format!("- {}: not run yet", test.name));
            continue;
        }
        for (dir, report) in &reports {
            let on = report
                .variant
                .as_ref()
                .map(|variant| format!(" on {}", variant.label()))
                .unwrap_or_default();
            match report.failure() {
                Some((ix, step)) => {
                    lines.push(format!(
                        "- {}{on}: failed at step {} ({}): {} (report: {})",
                        test.name,
                        ix + 1,
                        step.step.describe(),
                        step.error.clone().unwrap_or_default(),
                        dir.display()
                    ));
                    if failing_screen.is_none()
                        && let Some(shot) = &step.screenshot
                    {
                        failing_screen = Some(ph_agent::Attachment::Image {
                            label: format!("{} step {}", test.name, ix + 1),
                            path: dir.join(shot),
                        });
                    }
                }
                None if report.looks_different() > 0 => lines.push(format!(
                    "- {}{on}: every step worked, but {} look different from the baseline",
                    test.name,
                    report.looks_different()
                )),
                None => lines.push(format!("- {}{on}: passed", test.name)),
            }
        }
    }
    let mut attachments = vec![ph_agent::Attachment::Text {
        label: "UI test results".into(),
        text: lines.join("\n"),
    }];
    attachments.extend(failing_screen);
    attachments
}
