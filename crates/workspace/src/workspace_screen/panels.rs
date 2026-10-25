//! The tool panels of a workspace (Git, board, browser, devices, tests,
//! pull requests, runs, terminals): each made the first time it is shown,
//! and how their events reach the rest of the screen.

use super::*;

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
            },
        ));
        self.changes.insert(self.git_folder, changes.clone());
        changes
    }

    /// The board panel, created on first use.
    pub fn board(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<BoardPanel> {
        if let Some(board) = &self.board {
            return board.clone();
        }
        let store = AppState::store(cx).clone();
        let project_id = self.project.id;
        let board = cx.new(|cx| BoardPanel::new(store, project_id, cx));
        self._subscriptions.push(cx.subscribe_in(
            &board,
            window,
            |this, _, event: &BoardPanelEvent, window, cx| match event {
                BoardPanelEvent::SendToAgent { prompt, title } => {
                    let (prompt, title) = (prompt.clone(), title.clone());
                    this.chat.update(cx, |chat, cx| {
                        chat.send_text(prompt, Some(title), window, cx)
                    });
                }
            },
        ));
        self.board = Some(board.clone());
        board
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
        let devices = cx.new(|cx| DevicesPanel::new(window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &devices,
            window,
            |this, _, event: &ToChat, window, cx| this.receive_to_chat(event, window, cx),
        ));
        self.devices = Some(devices.clone());
        devices
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
                }
                ph_uitest::TestsPanelEvent::Card {
                    title,
                    description,
                    test,
                } => {
                    let card = ph_store::board::NewCard {
                        title: title.clone(),
                        description: description.clone(),
                        acceptance: "The UI test passes again.".into(),
                    };
                    let store = AppState::store(cx).clone();
                    // The card can only be Done once the test passes again.
                    let added = store.add_cards(this.project.id, &[card]).and_then(|cards| {
                        cards
                            .first()
                            .map_or(Ok(()), |card| store.link_card_test(card.id, test, true))
                    });
                    let note = match added {
                        Ok(()) => {
                            ph_board::BoardHub::notify(this.project.id, cx);
                            Notification::success(
                                "Added a card to the board; it is Done once the test passes.",
                            )
                        }
                        Err(err) => Notification::error(format!("Could not add the card: {err:#}")),
                    };
                    window.push_notification(note, cx);
                }
                ph_uitest::TestsPanelEvent::OpenFile(path) => {
                    if let Ok(relative) = path.strip_prefix(&this.project.path) {
                        let relative = relative.to_string_lossy().replace('\\', "/");
                        this.open_file(&relative, None, window, cx);
                    }
                }
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
                    window.dispatch_action(Box::new(crate::actions::OpenSettings), cx)
                }
                // Reviews run in the GitHub view, on its own clone.
                PullsPanelEvent::ReviewWithAgent { hosting, pull } => {
                    cx.emit(WorkspaceScreenEvent::Review {
                        hosting: hosting.clone(),
                        pull: pull.clone(),
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
            |this, _, event: &ph_terminal::TerminalPanelEvent, window, cx| {
                let ph_terminal::TerminalPanelEvent::Ask { terminal, text } = event;
                this.reveal_chat(cx);
                let attachment = Attachment::Text {
                    label: terminal.clone(),
                    text: format!("{terminal}:\n```\n{text}\n```"),
                };
                this.chat
                    .update(cx, |chat, cx| chat.ask_about(attachment, None, window, cx));
                cx.notify();
            },
        ));
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
        match tab {
            ContextTab::Files => {
                if !self.project.empty {
                    self.files(window, cx);
                }
            }
            ContextTab::Board => {
                self.board(window, cx);
            }
            ContextTab::Terminal => {
                if self.terminal.is_none() {
                    // The first time a workspace's Terminal tab opens, a
                    // shell is ready in it.
                    let first = ph_terminal::TerminalHub::existing(self.project.id, cx).is_none();
                    let terminal = self.terminal_panel(window, cx);
                    if first {
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
        let terminals =
            ph_terminal::TerminalHub::terminals(self.project.id, self.project.path.clone(), cx);
        let (cwd, folders) = (self.project.path.clone(), self.named_folders());
        terminals.update(cx, |terminals, _| terminals.set_folders(cwd, folders));
        let terminal = cx.new(|cx| TerminalPanel::new(terminals, cx));
        self.watch_terminal(&terminal, window, cx);
        self.terminal = Some(terminal.clone());
        terminal
    }
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
