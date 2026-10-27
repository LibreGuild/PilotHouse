//! The composer's `@` and `#` completions: what matches what is typed,
//! moving through the menu and putting the choice in the message.

use super::*;

impl ChatPanel {
    /// Looks for a mention being typed and suggests what it could be.
    pub(super) fn update_mention(&mut self, cx: &mut Context<Self>) {
        let composer = self.composer.read(cx);
        let typing = mention_complete::typing_at(&composer.value(), composer.cursor());
        let Some(typing) = typing else {
            if self.mention.take().is_some() {
                cx.notify();
            }
            return;
        };
        // Files are listed in the background, again after a minute.
        let stale = self
            .files
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > std::time::Duration::from_secs(60));
        if typing.trigger == '@' && stale && self._files.is_none() {
            let roots = self.project.roots();
            self._files = Some(cx.spawn(async move |this, cx| {
                let (files, symbols) = cx
                    .background_spawn(async move {
                        let files = roots.index();
                        let symbols = ph_files::symbols::index(&roots, &files);
                        (files, symbols)
                    })
                    .await;
                _ = this.update(cx, |this, cx| {
                    this.symbols = std::sync::Arc::new(symbols);
                    this.files = Some((std::time::Instant::now(), files));
                    this._files = None;
                    this.update_mention(cx);
                });
            }));
        }
        let empty = ph_files::FileList::default();
        let files = self
            .files
            .as_ref()
            .map(|(_, files)| files)
            .unwrap_or(&empty);
        let cards: Vec<(i64, String)> = if typing.trigger == '#' {
            self.store
                .cards(self.project.id)
                .unwrap_or_default()
                .into_iter()
                .map(|card| (card.id, card.title))
                .collect()
        } else {
            Vec::new()
        };
        if typing.trigger == '@' && typing.query.starts_with("pr") && !self.pulls_asked {
            self.pulls_asked = true;
            cx.emit(ChatPanelEvent::WantPulls);
        }
        let pulls: Vec<(u64, String)> = self
            .pulls
            .iter()
            .map(|pull| (pull.number, pull.title.clone()))
            .collect();
        let commands = self.active.read(cx).commands().to_vec();
        let extra = mention_complete::Extra {
            symbols: &self.symbols,
            pulls: &pulls,
            commands: &commands,
        };
        let items = mention_complete::suggestions(&typing, files, &cards, &extra);
        let selected = self
            .mention
            .as_ref()
            .filter(|menu| menu.typing.range.start == typing.range.start)
            .map(|menu| menu.selected.min(items.len().saturating_sub(1)))
            .unwrap_or_default();
        self.mention = (!items.is_empty()).then_some(MentionMenu {
            typing,
            items,
            selected,
        });
        cx.notify();
    }

    /// Puts suggestion `ix` in place of the mention being typed.
    pub(super) fn accept_mention(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.mention.take() else {
            return;
        };
        let Some(item) = menu.items.get(ix) else {
            return;
        };
        let (range, insert) = (menu.typing.range.clone(), item.insert.clone());
        self.composer.update(cx, |composer, cx| {
            composer.set_selected_range(range, cx);
            composer.replace(insert, window, cx);
        });
        cx.notify();
    }

    pub(super) fn move_mention(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.mention {
            let count = menu.items.len() as isize;
            menu.selected = (menu.selected as isize + by).rem_euclid(count.max(1)) as usize;
            cx.notify();
        }
    }

    pub(super) fn render_mention_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.mention.as_ref()?;
        Some(
            v_flex()
                .id("mention-menu")
                .absolute()
                .bottom_full()
                .left_0()
                .mb_1()
                .w(px(420.))
                .max_w_full()
                .p_1()
                .rounded(cx.theme().radius_lg)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .shadow_lg()
                .children(menu.items.iter().enumerate().map(|(ix, item)| {
                    h_flex()
                        .id(("mention-item", ix))
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(cx.theme().radius)
                        .cursor_pointer()
                        .when(ix == menu.selected, |this| this.bg(cx.theme().list_active))
                        .hover(|style| style.bg(cx.theme().list_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, cx| {
                                this.accept_mention(ix, window, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(div().flex_none().text_sm().child(item.label.clone()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(item.detail.clone()),
                        )
                }))
                .child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("↑↓ to pick · Enter or Tab to insert · Esc to close"),
                ),
        )
    }
}
