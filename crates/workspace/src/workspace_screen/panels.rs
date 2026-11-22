//! The tool panels of a workspace (Git, browser, devices, tests,
//! pull requests, runs, terminals): each made the first time it is shown,
//! and how their events reach the rest of the screen.

use super::*;

/// How sharp phones stream (`ph_devices::StreamQuality`), app-wide.
const STREAM_QUALITY: &str = "devices.stream_quality";
/// Whether to mirror each device's screen, by serial, as the user chose.
const MIRROR: &str = "devices.mirror";
/// Whether Logcat wraps long messages.
const LOGCAT_WRAP: &str = "logcat.wrap";

impl WorkspaceScreen {
    /// The changes panel, created on first use.
    pub fn changes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<ChangesPanel> {
        if let Some(changes) = self.changes.get(&self.git_folder) {
            return changes.clone();
        }
        let watcher = self.watchers(cx)[self.git_folder.min(self.watchers.len() - 1)].clone();
        let repo = self.repo(window, cx);
        let writer = ph_agent::oneshot::writer(AppState::store(cx).clone());
        let changes = cx.new(|cx| {
            let mut changes = ChangesPanel::new(&watcher, repo, window, cx);
            changes.set_writer(writer, cx);
            changes
        });
        // The tool rail shows the change count.
        self._subscriptions
            .push(cx.observe(&changes, |_, _, cx| cx.notify()));
        self._subscriptions.push(cx.subscribe_in(
            &changes,
            window,
            |this, _, event: &ChangesPanelEvent, window, cx| match event {
                ChangesPanelEvent::OpenFile { path, line } => {
                    // The path is in the Git tab's folder; Files names it
                    // with that folder in a multi-folder workspace.
                    let roots = this.roots();
                    let path = roots
                        .list()
                        .get(this.git_folder)
                        .and_then(|root| roots.path_of(&root.path.join(path)))
                        .unwrap_or_else(|| path.clone());
                    this.open_file(&path, Some(*line), window, cx);
                }
                ChangesPanelEvent::ReviewChanges => this.review_changes(window, cx),
                ChangesPanelEvent::ExplainChanges => this.explain_changes(window, cx),
                ChangesPanelEvent::ChangesRead => this.changes_read(cx),
                ChangesPanelEvent::CommitShown(commit) => {
                    this.commit_shown(commit.clone(), window, cx)
                }
                ChangesPanelEvent::SendNotes(text) => {
                    this.reveal_chat(cx);
                    let attachment = Attachment::Text {
                        label: "Review notes".into(),
                        text: text.clone(),
                    };
                    this.chat.update(cx, |chat, cx| {
                        chat.ask_about(
                            attachment,
                            Some((
                                "Review notes".into(),
                                "Address the review notes attached, then say what you changed."
                                    .into(),
                            )),
                            window,
                            cx,
                        )
                    });
                    cx.notify();
                }
            },
        ));
        self.changes.insert(self.git_folder, changes.clone());
        self.attach_explanation(&changes, window, cx);
        self.update_live_panes(cx);
        changes
    }

    /// The browser pane, created on first use (Chrome starts when it is shown).
    pub fn browser(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<BrowserPanel> {
        if let Some(browser) = &self.browser {
            return browser.clone();
        }
        let project_id = self.project.id;
        let browser = cx.new(|cx| BrowserPanel::new(project_id, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &browser,
            window,
            |this, _, event: &ToChat, window, cx| this.receive_to_chat(event, window, cx),
        ));
        self.browser = Some(browser.clone());
        browser
    }

    /// The devices pane, created on first use.
    pub fn devices(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<DevicesPanel> {
        if let Some(devices) = &self.devices {
            return devices.clone();
        }
        let quality: String = AppState::store(cx).setting(STREAM_QUALITY);
        let mirror: std::collections::HashMap<String, bool> = AppState::store(cx).setting(MIRROR);
        // The log shows under the screen too, where the build has Logcat.
        let logcat = ph_brand::enabled(ph_brand::Feature::Logcat).then(|| self.logcat(window, cx));
        let devices = cx.new(|cx| {
            let mut devices = DevicesPanel::new(cx);
            devices.set_stream_quality(ph_devices::StreamQuality::from_key(&quality), cx);
            devices.set_mirror_choices(mirror, cx);
            if let Some(logcat) = logcat {
                devices.set_logcat(logcat, cx);
            }
            devices
        });
        self._subscriptions.push(cx.subscribe_in(
            &devices,
            window,
            |this, _, event: &ToChat, window, cx| this.receive_to_chat(event, window, cx),
        ));
        // The choice holds for every workspace's devices, and the next run.
        self._subscriptions.push(cx.subscribe(
            &devices,
            |_, _, ph_devices::StreamQualityChanged(quality), cx| {
                let _ = AppState::store(cx).set_setting(STREAM_QUALITY, &quality.key());
            },
        ));
        self._subscriptions.push(cx.subscribe(
            &devices,
            |_, _, ph_devices::MirrorChanged { serial, mirror }, cx| {
                let store = AppState::store(cx);
                let mut choices: std::collections::HashMap<String, bool> = store.setting(MIRROR);
                choices.insert(serial.clone(), *mirror);
                let _ = store.set_setting(MIRROR, &choices);
            },
        ));
        self._subscriptions.push(cx.subscribe_in(
            &devices,
            window,
            |this, _, _: &ph_devices::OpenLogcat, window, cx| {
                this.show_tab(ContextTab::Logcat, window, cx)
            },
        ));
        self.devices = Some(devices.clone());
        devices
    }

    /// The Logcat tool, created on first use.
    pub fn logcat(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ph_devices::LogcatPanel> {
        if let Some(logcat) = &self.logcat {
            return logcat.clone();
        }
        // Wrapping unless switched off: an unset choice reads as `false`.
        let unwrapped: bool = AppState::store(cx).setting(LOGCAT_WRAP);
        let folders = self.project.folders();
        let logcat = cx.new(|cx| {
            let mut logcat = ph_devices::LogcatPanel::new(window, cx);
            logcat.set_wrap(!unwrapped, cx);
            logcat.set_project_folders(folders, window, cx);
            logcat
        });
        self._subscriptions.push(cx.subscribe_in(
            &logcat,
            window,
            |this, _, event: &ToChat, window, cx| this.receive_to_chat(event, window, cx),
        ));
        self._subscriptions.push(cx.subscribe_in(
            &logcat,
            window,
            |this, _, event: &ph_devices::LogcatPanelEvent, window, cx| match event {
                ph_devices::LogcatPanelEvent::OpenSource(source) => {
                    this.open_source(source.clone(), window, cx)
                }
                ph_devices::LogcatPanelEvent::WrapChanged(wrap) => {
                    let _ = AppState::store(cx).set_setting(LOGCAT_WRAP, &!wrap);
                }
                ph_devices::LogcatPanelEvent::Crashed(crash) => this.app_crashed(crash, window, cx),
            },
        ));
        self.logcat = Some(logcat.clone());
        logcat
    }

    /// Shows the file a stack frame names, at its line: the one in this
    /// workspace's folders whose place fits the frame's class best.
    fn open_source(
        &mut self,
        source: ph_devices::SourceRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folders = self.project.folders();
        cx.spawn_in(window, async move |this, cx| {
            let wanted = source.clone();
            let found = cx
                .background_spawn(
                    async move { super::source_lookup::find_source(&folders, &wanted) },
                )
                .await;
            let _ = this.update_in(cx, |this, window, cx| match found {
                Some(path) => this.open_path(&path, Some(u64::from(source.line)), window, cx),
                // A frame of Android's own or of a library.
                None => window.push_notification(
                    Notification::info(format!("{} is not in this workspace.", source.file)),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// An app of this workspace crashed on the device: says so, with the
    /// exception, and offers its stack to the chat.
    fn app_crashed(
        &mut self,
        crash: &ph_devices::Crash,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(runs) = &self.runs {
            runs.update(cx, |runs, cx| runs.app_crashed(crash, cx));
        }
        let name = crash.process.clone().unwrap_or_else(|| "The app".into());
        let report = ToChat::Text {
            label: "crash".into(),
            text: crash.report(),
        };
        let this = cx.entity().downgrade();
        window.push_notification(
            Notification::error(crash.summary())
                .title(format!("{name} crashed"))
                .action(move |_, _, cx| {
                    let (this, report) = (this.clone(), report.clone());
                    Button::new("crash-to-chat")
                        .small()
                        .label("Send to chat")
                        .on_click(cx.listener(move |notification, _, window, cx| {
                            _ = this
                                .update(cx, |this, cx| this.receive_to_chat(&report, window, cx));
                            notification.dismiss(window, cx);
                        }))
                })
                // It goes by itself like any notice: the crash stays on
                // the Run tab's card and in the log.
                .autohide(true),
            cx,
        );
    }

    /// The Run tab, created on first use.
    /// The pull requests panel, created on first use.
    /// The Tests tab, created on first use.
    pub fn tests(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ph_uitest::TestsPanel> {
        if let Some(tests) = &self.tests {
            return tests.clone();
        }
        let (project_id, root) = (self.project.id, self.project.path.clone());
        let tests = cx.new(|cx| ph_uitest::TestsPanel::new(project_id, root, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &tests,
            window,
            |this, _, event: &ph_uitest::TestsPanelEvent, window, cx| match event {
                ph_uitest::TestsPanelEvent::Agent { title, prompt } => {
                    this.reveal_chat(cx);
                    let (title, prompt) = (title.clone(), prompt.clone());
                    this.chat.update(cx, |chat, cx| {
                        chat.start_tool_task(title, prompt, window, cx)
                    });
                    // Only this chat's turn ends the recording it does.
                    if let Some(chat) = this.chat.read(cx).active_session_id(cx) {
                        ph_uitest::record::set_chat(chat, cx);
                    }
                }
                // A test's file, or where a test of the project failed (the
                // editor counts lines from 0).
                ph_uitest::TestsPanelEvent::Open { path, line } => {
                    if let Ok(relative) = path.strip_prefix(&this.project.path) {
                        let relative = relative.to_string_lossy().replace('\\', "/");
                        let line = line.map(|line| line.saturating_sub(1));
                        this.open_file(&relative, line, window, cx);
                    }
                }
                ph_uitest::TestsPanelEvent::ShowRun => this.show_tab(ContextTab::Runs, window, cx),
                ph_uitest::TestsPanelEvent::ShowPreview { phone } => this.show_tab(
                    if *phone {
                        ContextTab::Devices
                    } else {
                        ContextTab::Browser
                    },
                    window,
                    cx,
                ),
            },
        ));
        self.tests = Some(tests.clone());
        tests
    }

    pub fn pulls(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<PullsPanel> {
        if let Some(pulls) = self.pulls.get(&self.git_folder) {
            return pulls.clone();
        }
        let repo = self.repo(window, cx);
        let store = AppState::store(cx).clone();
        let writer = ph_agent::oneshot::writer(store.clone());
        let pulls = cx.new(|cx| {
            let mut pulls = PullsPanel::new(store, repo, cx);
            pulls.set_writer(writer);
            pulls
        });
        self._subscriptions.push(cx.subscribe_in(
            &pulls,
            window,
            |_, _, event: &PullsPanelEvent, window, cx| match event {
                PullsPanelEvent::OpenSettings => {
                    window.dispatch_action(Box::new(crate::actions::OpenGitHubSettings), cx)
                }
                // Reviews run in the GitHub view, on its own clone.
                PullsPanelEvent::Agent { hosting, request } => {
                    cx.emit(WorkspaceScreenEvent::Agent {
                        hosting: hosting.clone(),
                        request: request.clone(),
                    })
                }
                PullsPanelEvent::OpenSession(id) => cx.emit(WorkspaceScreenEvent::ShowSession(*id)),
            },
        ));
        self.pulls.insert(self.git_folder, pulls.clone());
        pulls
    }

    /// Loads the pull requests of the Git folder in view and keeps the chat
    /// told, for `@pr:` mentions.
    pub(super) fn feed_pulls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.folders().is_empty() {
            return;
        }
        let pulls = self.pulls(window, cx);
        let chat = self.chat.downgrade();
        self._subscriptions
            .push(cx.observe(&pulls, move |_, pulls, cx| {
                let briefs = briefs(pulls.read(cx).list());
                _ = chat.update(cx, |chat, cx| chat.set_pulls(briefs, cx));
            }));
        let briefs = briefs(pulls.read(cx).list());
        if briefs.is_empty() {
            pulls.update(cx, |pulls, cx| pulls.reload(cx));
        }
        self.chat.update(cx, |chat, cx| chat.set_pulls(briefs, cx));
    }

    /// ⌘L in a terminal: its selection (or screen) goes to the chat.
    pub(super) fn watch_terminal(
        &mut self,
        terminal: &Entity<TerminalPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscriptions.push(cx.subscribe_in(
            terminal,
            window,
            |this, _, event: &ph_terminal::TerminalPanelEvent, window, cx| match event {
                ph_terminal::TerminalPanelEvent::Ask { terminal, text } => {
                    this.reveal_chat(cx);
                    let attachment = Attachment::Text {
                        label: terminal.clone(),
                        text: format!("{terminal}:\n```\n{text}\n```"),
                    };
                    this.chat
                        .update(cx, |chat, cx| chat.ask_about(attachment, None, window, cx));
                    cx.notify();
                }
                ph_terminal::TerminalPanelEvent::Open { link, cwd } => {
                    this.open_link(link, Some(cwd), window, cx)
                }
                ph_terminal::TerminalPanelEvent::ToggleTranscript(terminal) => {
                    this.toggle_transcript(terminal.clone(), window, cx)
                }
            },
        ));
    }

    /// Shows the session of the agent in `terminal` beside it, as a chat
    /// (read only), unless it shows already.
    pub(crate) fn show_transcript(
        &mut self,
        terminal: &Entity<ph_terminal::Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = self
            .terminal
            .as_ref()
            .is_some_and(|panel| panel.read(cx).shows_transcript(terminal.entity_id()));
        if !shown {
            self.toggle_transcript(terminal.clone(), window, cx);
        }
    }

    /// (read only), or hides it.
    fn toggle_transcript(
        &mut self,
        terminal: Entity<ph_terminal::Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.terminal.clone() else {
            return;
        };
        let id = terminal.entity_id();
        if panel.read(cx).shows_transcript(id) {
            panel.update(cx, |panel, cx| panel.set_transcript(id, None, cx));
            self.transcript_subscriptions.remove(&id);
            return;
        }
        let Some(agent) = crate::terminal_agents::TerminalAgents::of_terminal(&terminal, cx)
            .map(|agent| agent.agent)
        else {
            return;
        };
        let Some(format) = ph_agent::cli_transcript::Format::of(agent.id) else {
            return;
        };
        let weak = terminal.downgrade();
        let find: ph_agent::cli_transcript::FindSession =
            Box::new(move |cx| crate::terminal_agents::session_file(&weak, cx));
        let store = AppState::store(cx).clone();
        let project = self.project.clone();
        let view = cx.new(|cx| {
            ph_agent::cli_transcript::TranscriptView::new(store, &project, agent, format, find, cx)
        });
        // "Continue in a chat": a new chat here, from a recap of the session.
        let from = format!("{} in a terminal", agent.name);
        let subscription = cx.subscribe_in(
            &view,
            window,
            move |this, _, event: &ph_agent::cli_transcript::TranscriptViewEvent, window, cx| {
                let ph_agent::cli_transcript::TranscriptViewEvent::Continue {
                    agent,
                    recap,
                    title,
                } = event;
                this.show_chat(window, cx);
                this.chat.update(cx, |chat, cx| {
                    chat.continue_with(agent, recap.clone(), title.clone(), &from, window, cx)
                });
            },
        );
        // Terminals closed since have nothing to continue from.
        let shown = panel.read(cx);
        self.transcript_subscriptions
            .retain(|terminal, _| shown.shows_transcript(*terminal));
        self.transcript_subscriptions.insert(id, subscription);
        panel.update(cx, |panel, cx| {
            panel.set_transcript(id, Some(view.into()), cx)
        });
    }

    pub fn runs(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<RunsPanel> {
        if let Some(runs) = &self.runs {
            return runs.clone();
        }
        let (store, id, folders) = (
            AppState::store(cx).clone(),
            self.project.id,
            self.named_folders(),
        );
        let runs = cx.new(|cx| RunsPanel::new(store, id, folders, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &runs,
            window,
            |this, _, event: &RunsPanelEvent, window, cx| match event {
                RunsPanelEvent::Run { name, command, cwd } => {
                    let terminal = this.terminal_for_run(window, cx);
                    terminal.update(cx, |terminal, cx| {
                        terminal.run_in(name, command, cwd.clone(), window, cx)
                    });
                }
                // The log runs from the start of a run, so that what the
                // app writes as it starts (and a crash then) is in it.
                RunsPanelEvent::AppStarting => {
                    if ph_brand::enabled(ph_brand::Feature::Logcat) {
                        let logcat = this.logcat(window, cx);
                        logcat.update(cx, |logcat, cx| logcat.start(cx));
                    }
                }
                RunsPanelEvent::AppLaunched { package } => {
                    if ph_brand::enabled(ph_brand::Feature::Logcat) {
                        let logcat = this.logcat(window, cx);
                        logcat.update(cx, |logcat, cx| logcat.add_project_app(package, window, cx));
                    }
                    this.show_tab(ContextTab::Devices, window, cx)
                }
                RunsPanelEvent::ToChat(message) => this.receive_to_chat(message, window, cx),
                RunsPanelEvent::OpenUrl(url) => {
                    this.show_tab(ContextTab::Browser, window, cx);
                    let browser = ph_browser::BrowserHub::browser(this.project.id, cx);
                    browser.update(cx, |browser, cx| browser.open(url, cx));
                }
            },
        ));
        self.runs = Some(runs.clone());
        runs
    }

    pub(super) fn ensure_panel(
        &mut self,
        tab: ContextTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A tool the brand leaves out never starts.
        if !tab.in_build() {
            return;
        }
        match tab {
            ContextTab::Files => {
                if !self.project.empty {
                    self.files(window, cx);
                }
            }
            ContextTab::Terminal => {
                if self.terminal.is_none() {
                    // The first time a workspace's Terminal tab opens, a
                    // shell is ready in it.
                    let first = ph_terminal::TerminalHub::existing(self.project.id, cx).is_none();
                    let terminal = self.terminal_panel(window, cx);
                    // Its terminals of the last run came back with it; else
                    // one shell.
                    let none = terminal
                        .read(cx)
                        .terminals()
                        .read(cx)
                        .terminals()
                        .is_empty();
                    if first && none {
                        terminal.update(cx, |terminal, cx| terminal.new_terminal(window, cx));
                    }
                }
            }
            ContextTab::Changes => {
                self.changes(window, cx);
            }
            ContextTab::Browser => {
                self.browser(window, cx);
            }
            ContextTab::Devices => {
                self.devices(window, cx);
            }
            ContextTab::Logcat => {
                self.logcat(window, cx);
            }
            ContextTab::Runs => {
                self.runs(window, cx);
            }
            ContextTab::Pulls => {
                self.pulls(window, cx);
            }
            ContextTab::Tests => {
                self.tests(window, cx);
            }
        }
    }

    /// The open tab's panel. Panels are created by `ensure_panel` before
    /// their tab is shown, so `None` only flashes for a frame at most.
    /// The Terminal tab, shown (and turned on, since runs need it), for a
    /// command that opens its own terminal; a first one opens no spare shell.
    pub(super) fn terminal_for_run(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalPanel> {
        if !AppState::enabled_tabs(cx).contains(&ContextTab::Terminal) {
            AppState::set_tab_enabled(ContextTab::Terminal, true, cx);
        }
        let terminal = self.terminal_panel(window, cx);
        self.show_tab(ContextTab::Terminal, window, cx);
        terminal
    }

    /// The Terminal tab's panel, over the workspace's shells (which outlive
    /// it), made when first needed.
    pub(super) fn terminal_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalPanel> {
        if let Some(terminal) = self.terminal.clone() {
            return terminal;
        }
        self.make_terminal_panel(window, cx)
    }

    /// Lists an agent's own past sessions in `cwd` (those started outside
    /// PilotHouse too: in another terminal, another editor); the one
    /// picked goes on here, as a session of the workspace.
    pub(crate) fn pick_agent_session(
        &mut self,
        agent: &'static str,
        cwd: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = cwd.clone();
        let list =
            cx.background_spawn(async move { ph_agent::cli_sessions::sessions(agent, &folder) });
        let name = ph_agent::agents::find(agent).map_or(agent, |spec| spec.name);
        cx.spawn_in(window, async move |this, cx| {
            let sessions = list.await;
            _ = this.update_in(cx, |_, window, cx| {
                let screen = cx.entity().downgrade();
                window.open_dialog(cx, move |dialog, window, cx| {
                    let rows = sessions.iter().map(|session| {
                        let (screen, past) = (screen.clone(), session.clone());
                        let group = SharedString::from(format!("agent-session-row-{}", session.id));
                        gpui_kit::component::h_flex()
                            .id(SharedString::from(format!("agent-session-{}", session.id)))
                            .role(gpui_kit::accesskit::Role::Button)
                            .aria_label(session.title.clone())
                            .group(group.clone())
                            .gap_3()
                            .px_2()
                            .h_8()
                            .rounded(cx.theme().radius)
                            .cursor_pointer()
                            .hover(|style| style.bg(cx.theme().list_hover))
                            .child(
                                gpui_kit::div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .child(session.title.clone()),
                            )
                            .child(
                                gpui_kit::div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(ph_ui::time::age(session.updated_at)),
                            )
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                _ = screen.update(cx, |screen, cx| {
                                    screen.open_outside_session(agent, &past, window, cx)
                                });
                            })
                    });
                    dialog
                        .title(format!("Go on with a {name} session"))
                        .w(ph_ui::scaled_px(560., window))
                        .child(if sessions.is_empty() {
                            gpui_kit::div()
                                .py_4()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("No {name} sessions in this folder yet."))
                                .into_any_element()
                        } else {
                            gpui_kit::component::v_flex()
                                .id("agent-sessions")
                                .max_h(ph_ui::rems_from_px(420.))
                                .overflow_y_scroll()
                                .children(rows)
                                .into_any_element()
                        })
                });
            });
        })
        .detach();
    }

    /// Opens a link ⌘-clicked in a terminal: a dev server's page in the
    /// Browser tool, another web page in the system browser, a file of the
    /// workspace in the editor at its line.
    pub(super) fn open_link(
        &mut self,
        link: &ph_terminal::links::Link,
        cwd: Option<&std::path::Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match link {
            ph_terminal::links::Link::Url(url) => {
                let local = local_url(url);
                if local {
                    self.show_tab(ContextTab::Browser, window, cx);
                    let browser = ph_browser::BrowserHub::browser(self.project.id, cx);
                    browser.update(cx, |browser, cx| browser.open(url, cx));
                } else {
                    cx.open_url(url);
                }
            }
            ph_terminal::links::Link::File { path, line } => {
                let roots = self.roots();
                let given = std::path::Path::new(path);
                // Only files in the workspace's folders: no way out of them.
                if given
                    .components()
                    .any(|part| part == std::path::Component::ParentDir)
                {
                    return;
                }
                let found = if given.is_absolute() {
                    given.is_file().then(|| given.to_path_buf())
                } else {
                    // Where the shell started, then the workspace's folders.
                    cwd.map(std::path::Path::to_path_buf)
                        .into_iter()
                        .chain(self.project.folders())
                        .map(|folder| folder.join(given))
                        .find(|candidate| candidate.is_file())
                };
                if let Some(name) = found.and_then(|file| roots.path_of(&file)) {
                    let line = line.map(|line| line.saturating_sub(1));
                    self.open_file(&name, line, window, cx);
                }
            }
        }
    }

    /// Runs `command` in this workspace's terminal named `name`, shown.
    pub(crate) fn run_in_terminal(
        &mut self,
        name: &str,
        command: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal = self.terminal_for_run(window, cx);
        terminal.update(cx, |terminal, cx| {
            terminal.run_in(name, command, None, window, cx)
        });
    }

    /// Continues agent `agent`'s past session `past` as a chat: a new chat
    /// of that agent bound to the session, which the agent loads (and plays
    /// back) when the chat starts.
    pub(crate) fn resume_in_chat(
        &mut self,
        agent: &str,
        past: &ph_agent::cli_sessions::PastSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = AppState::store(cx).clone();
        let made = store
            .create_session(self.project.id, &past.title, agent)
            .and_then(|session| {
                store.set_session_acp_id(session.id, Some(&past.id))?;
                Ok(session.id)
            });
        match made {
            Ok(id) => {
                self.reload_sessions(cx);
                self.reveal_chat(cx);
                self.select_session(id, cx);
                // The agent loads the session now and plays its history
                // into the chat.
                self.chat.update(cx, |chat, cx| {
                    chat.start_agent(cx);
                    chat.focus_composer(window, cx);
                });
                cx.emit(WorkspaceScreenEvent::SessionsChanged);
            }
            Err(err) => tracing::warn!("resuming the session in a chat failed: {err:#}"),
        }
    }

    /// Goes on here with one of an agent's own sessions that did not start
    /// here: in a chat, which takes the session back and plays its history
    /// in; for an agent that cannot do that (Cline), in its CLI. The
    /// header's switch then offers the other form.
    pub(crate) fn open_outside_session(
        &mut self,
        agent: &'static str,
        past: &ph_agent::cli_sessions::PastSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let loads = ph_agent::agents::find(agent).is_some_and(|spec| !spec.quirks.no_session_load);
        if loads || !ph_brand::enabled(ph_brand::Feature::Terminal) {
            return self.resume_in_chat(agent, past, window, cx);
        }
        let store = AppState::store(cx).clone();
        let cwd = self.project.path.to_string_lossy().into_owned();
        let made = store
            .create_session(self.project.id, &past.title, agent)
            .and_then(|session| {
                store.set_session_acp_id(session.id, Some(&past.id))?;
                store.set_session_place(session.id, "terminal", Some(&cwd))?;
                Ok(session.id)
            });
        match made {
            Ok(id) => {
                self.reload_sessions(cx);
                self.reveal_chat(cx);
                self.select_session(id, cx);
                self.show_as(ph_agent::Form::Cli, window, cx);
            }
            Err(err) => tracing::warn!("going on with the session failed: {err:#}"),
        }
    }

    /// Shows `terminal` in the Terminal tool, with the keyboard in it.
    pub(crate) fn show_terminal(
        &mut self,
        terminal: &Entity<ph_terminal::Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A chat's CLI shows in its chat, not among the Terminal tab's.
        if let Some(session) = super::cli::chat_of(self.project.id, terminal, cx) {
            self.select_session(session, cx);
            return;
        }
        let panel = self.terminal_panel(window, cx);
        let terminals = panel.read(cx).terminals().clone();
        terminals.update(cx, |terminals, cx| {
            if let Some(ix) = terminals
                .terminals()
                .iter()
                .position(|known| known == terminal)
            {
                terminals.select(ix, cx);
            }
        });
        self.show_tab(ContextTab::Terminal, window, cx);
        panel.update(cx, |panel, cx| panel.focus(terminal, window, cx));
    }

    fn make_terminal_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalPanel> {
        let terminals =
            ph_terminal::TerminalHub::terminals(self.project.id, self.project.path.clone(), cx);
        let (cwd, folders) = (self.project.path.clone(), self.named_folders());
        terminals.update(cx, |terminals, _| terminals.set_folders(cwd, folders));
        let terminal = cx.new(|cx| {
            let mut panel = TerminalPanel::new(terminals, cx);
            // Agents that keep a readable session file can show it beside
            // their terminal.
            panel.set_has_transcript(std::rc::Rc::new(|terminal, cx| {
                crate::terminal_agents::TerminalAgents::of_terminal(terminal, cx).is_some_and(
                    |agent| ph_agent::cli_transcript::Format::of(agent.agent.id).is_some(),
                )
            }));
            panel
        });
        self.watch_terminal(&terminal, window, cx);
        self.terminal = Some(terminal.clone());
        terminal
    }
}

/// Whether `url` is a web address on this computer (its host, not just
/// somewhere in it).
fn local_url(url: &str) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host.split(':').next().unwrap_or_default(),
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "0.0.0.0" | "::1"
    )
}

/// Open pull requests as `@pr:` mentions attach them.
pub(crate) fn briefs(pulls: &[ph_github::models::PullRequest]) -> Vec<ph_agent::PullBrief> {
    pulls
        .iter()
        .filter(|pull| pull.state == "open")
        .map(|pull| ph_agent::PullBrief {
            number: pull.number,
            title: pull.title.clone(),
            author: pull.user.login.clone(),
            branches: format!("{} → {}", pull.head.name, pull.base.name),
            url: pull.html_url.clone(),
            body: pull.body.clone().unwrap_or_default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::local_url;

    #[test]
    fn knows_addresses_on_this_computer_by_their_host() {
        assert!(local_url("http://localhost:5173/"));
        assert!(local_url("http://127.0.0.1:8080/a?b=//x"));
        assert!(local_url("http://[::1]:3000/"));
        assert!(!local_url("https://example.com/?next=//localhost"));
        assert!(!local_url("https://localhost.evil.com/"));
        assert!(!local_url("https://user@example.com/"));
    }
}
