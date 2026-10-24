//! Choosing the agent, its model and options (the model picker), and
//! handing a chat over to another agent.

use super::*;

impl ChatPanel {
    pub(super) fn choose_agent(&mut self, spec: &'static AgentSpec, cx: &mut Context<Self>) {
        let _ = self.store.set_setting(DEFAULT_AGENT_KEY, &spec.id);
        self.active
            .update(cx, |thread, cx| thread.set_agent(spec, cx));
        cx.notify();
    }

    pub(super) fn choose_option(&mut self, option_id: &str, value: &str, cx: &mut Context<Self>) {
        self.active
            .update(cx, |thread, cx| thread.set_option(option_id, value, cx));
    }

    /// What the model picker lists: this chat's agent live, the others from
    /// their cached options with remembered choices applied.
    pub(super) fn picker_data(&self, cx: &App) -> PickerData {
        let thread = self.active.read(cx);
        let active = thread.agent();
        // This chat's agent first, then the others in their usual order.
        let ordered =
            std::iter::once(active).chain(AGENTS.iter().filter(|spec| spec.id != active.id));
        let agents = ordered
            .map(|spec| {
                let options = if spec.id == active.id {
                    thread.options().to_vec()
                } else {
                    options::with_preferences(
                        options::cached(&self.store, spec.id),
                        &options::preferences(&self.store, spec.id),
                    )
                };
                (spec, options)
            })
            .collect();
        PickerData {
            active,
            started: thread.has_started(),
            agents,
            viewing: self.picker_view.unwrap_or(active),
            query: self.picker_search.read(cx).value().to_string(),
            highlight: self.picker_highlight,
        }
    }

    /// A choice made in the model picker.
    pub(crate) fn pick(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active.read(cx).agent();
        match pick {
            Pick::Model {
                agent,
                option,
                value,
            } if agent.id == active.id => self.choose_option(&option, &value, cx),
            Pick::Model {
                agent,
                option,
                value,
            } => {
                // Applied when that agent's session starts.
                options::remember(&self.store, agent.id, &option, &value);
                self.switch_agent(agent, window, cx);
                self.picker_view = None;
            }
            Pick::Agent(agent) => {
                self.switch_agent(agent, window, cx);
                self.picker_view = None;
            }
            Pick::View(agent) => {
                self.picker_view = Some(agent);
                self.picker_highlight = 0;
                cx.notify();
            }
        }
        // The picker stays open: the keyboard stays in it, so the arrows,
        // typing and Escape still work after a click.
        self.picker_search
            .update(cx, |search, cx| search.focus(window, cx));
    }

    /// The picker opened: this chat's agent, no filter, typing goes there.
    pub(super) fn picker_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_view = None;
        self.picker_highlight = 0;
        let search = self.picker_search.clone();
        window.defer(cx, move |window, cx| {
            search.update(cx, |search, cx| {
                search.set_value("", window, cx);
                search.focus(window, cx);
            })
        });
    }

    /// The picker's keys: ↑/↓ move the highlight, Enter picks it, Tab shows
    /// the next agent.
    pub(crate) fn picker_key(
        &mut self,
        key: PickerKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let data = self.picker_data(cx);
        let models = model_picker::listed_models(&data);
        match key {
            PickerKey::Up => {
                self.picker_highlight = self.picker_highlight.saturating_sub(1);
            }
            PickerKey::Down => {
                self.picker_highlight =
                    (self.picker_highlight + 1).min(models.len().saturating_sub(1));
            }
            PickerKey::Pick => {
                if let Some(pick) = models.into_iter().nth(self.picker_highlight) {
                    self.pick(pick, window, cx);
                }
            }
            PickerKey::NextAgent => {
                let ix = data
                    .agents
                    .iter()
                    .position(|(agent, _)| agent.id == data.viewing.id)
                    .unwrap_or_default();
                let next = data.agents[(ix + 1) % data.agents.len()].0;
                self.pick(Pick::View(next), window, cx);
            }
        }
        cx.notify();
    }

    /// Continues this chat with `agent` in a new chat: the new agent gets a
    /// recap of this one with the first message, and the composer offers a
    /// message to send (or edit).
    pub(super) fn hand_off(
        &mut self,
        agent: &'static AgentSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (recap, from, title) = {
            let thread = self.active.read(cx);
            (
                thread.handoff_recap(cx),
                thread.agent().name,
                thread.title(),
            )
        };
        let Some(recap) = recap else { return };
        let _ = self.store.set_setting(DEFAULT_AGENT_KEY, &agent.id);
        self.set_session(None, cx);
        self.choose_agent(agent, cx);
        let origin = format!(
            "Continuing “{title}” from {from}. {} gets a recap with your first message.",
            agent.name
        );
        self.active
            .update(cx, |thread, cx| thread.continue_from(recap, origin, cx));
        self.next_title = Some(format!("{title} · {}", agent.name));
        self.composer.update(cx, |composer, cx| {
            composer.set_value("Continue where the previous agent left off.", window, cx)
        });
        cx.emit(ChatPanelEvent::SessionsChanged);
        self.warm_up(cx);
        self.focus_composer(window, cx);
    }

    /// Uses `agent` for this chat, or for a new one if this chat already
    /// started (an agent cannot change mid-conversation).
    pub(super) fn switch_agent(
        &mut self,
        agent: &'static AgentSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active.read(cx).has_started() {
            let _ = self.store.set_setting(DEFAULT_AGENT_KEY, &agent.id);
            self.set_session(None, cx);
            cx.emit(ChatPanelEvent::SessionsChanged);
        } else {
            self.choose_agent(agent, cx);
        }
        // Start it now, so its models fill in.
        self.warm_up(cx);
        self.focus_composer(window, cx);
    }
}
