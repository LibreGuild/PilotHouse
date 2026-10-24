//! Searching every chat's messages (⌘⇧K, the palette, the sidebar, or the
//! chat history menu). Results come from the store's full-text index as you
//! type; picking one opens that chat at the message.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, HighlightStyle,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, Task, Window, div, px,
};
use ph_store::projects::ProjectId;
use ph_store::search::{MATCH_END, MATCH_START, SearchHit};
use ph_ui::Icons;

use crate::AppState;

/// Most results shown.
const LIMIT: usize = 50;
/// Pause after a keystroke before searching.
const DEBOUNCE: Duration = Duration::from_millis(120);

/// Opens the search dialog over `window`, scoped to `project` at first.
pub fn open(project: Option<ProjectId>, window: &mut Window, cx: &mut App) {
    let search = cx.new(|cx| ChatSearch::new(project, window, cx));
    let focus = search.read(cx).input.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        let first = search.clone();
        dialog
            .title("Search chats")
            .w(px(640.))
            .child(search.clone())
            // Enter in the dialog opens the best match.
            .on_ok(move |_, window, cx| {
                if let Some(pick) = first.read(cx).hits.first().map(pick_of) {
                    // After the dialog closes (returning true closes it).
                    window.defer(cx, move |window, cx| show(pick, window, cx));
                }
                true
            })
    });
    focus.update(cx, |input, cx| input.focus(window, cx));
}

/// Where a picked result leads.
#[derive(Debug, Clone, Copy)]
pub struct Pick {
    pub project_id: ProjectId,
    pub session_id: ph_store::sessions::SessionId,
    pub message_id: i64,
}

pub struct ChatSearch {
    input: Entity<InputState>,
    /// The project searched, unless `everywhere`.
    project: Option<ProjectId>,
    everywhere: bool,
    hits: Vec<SearchHit>,
    searched: bool,
    focus_handle: FocusHandle,
    _search: Option<Task<()>>,
    _subscription: Subscription,
}

impl ChatSearch {
    fn new(project: Option<ProjectId>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Words from any message…"));
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.search(cx);
            }
        });
        Self {
            input,
            project,
            everywhere: project.is_none(),
            hits: Vec::new(),
            searched: false,
            focus_handle: cx.focus_handle(),
            _search: None,
            _subscription: subscription,
        }
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        let store = AppState::store(cx).clone();
        let project = if self.everywhere { None } else { self.project };
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let hits = cx
                .background_spawn(async move { store.search_messages(project, &text, LIMIT) })
                .await;
            _ = this.update(cx, |this, cx| {
                this.searched = true;
                match hits {
                    Ok(hits) => this.hits = hits,
                    Err(err) => {
                        tracing::warn!("chat search failed: {err:#}");
                        this.hits.clear();
                    }
                }
                cx.notify();
            });
        }));
    }
}

fn pick_of(hit: &SearchHit) -> Pick {
    Pick {
        project_id: hit.project_id,
        session_id: hit.session_id,
        message_id: hit.message_id,
    }
}

/// Closes the dialog and shows the chat at the message.
fn open_pick(pick: Pick, window: &mut Window, cx: &mut App) {
    window.close_dialog(cx);
    show(pick, window, cx);
}

fn show(pick: Pick, window: &mut Window, cx: &mut App) {
    if let Some((_, workspace)) = crate::open::main_workspace(cx) {
        workspace.update(cx, |workspace, cx| workspace.show_message(pick, window, cx));
    }
}

/// The snippet's text on one line, and where its matched words are.
fn marked(text: &str) -> (String, Vec<std::ops::Range<usize>>) {
    let mut plain = String::with_capacity(text.len());
    let mut ranges = Vec::new();
    let mut start = None;
    for c in text.chars() {
        match c {
            MATCH_START => start = Some(plain.len()),
            MATCH_END => {
                if let Some(start) = start.take() {
                    ranges.push(start..plain.len());
                }
            }
            '\n' | '\r' => plain.push(' '),
            c => plain.push(c),
        }
    }
    (plain, ranges)
}

/// The snippet with its matched words highlighted.
fn snippet(text: &str, cx: &App) -> StyledText {
    let (plain, ranges) = marked(text);
    let style = HighlightStyle {
        color: Some(cx.theme().foreground),
        background_color: Some(cx.theme().warning.opacity(0.25)),
        ..Default::default()
    };
    StyledText::new(plain).with_highlights(ranges.into_iter().map(|range| (range, style)))
}

impl Focusable for ChatSearch {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChatSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.input.read(cx).value().trim().is_empty();
        let rows = self.hits.iter().enumerate().map(|(ix, hit)| {
            let pick = pick_of(hit);
            v_flex()
                .id(("chat-search-hit", ix))
                .gap_0p5()
                .px_2()
                .py_1p5()
                .rounded(cx.theme().radius)
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().muted))
                .on_click(move |_, window, cx| open_pick(pick, window, cx))
                .child(
                    h_flex()
                        .gap_1p5()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            Icon::new(if hit.role == "user" {
                                Icons::MessageSquare
                            } else {
                                Icons::Bot
                            })
                            .xsmall(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(hit.session_title.clone()),
                        )
                        // Stored in milliseconds; `ago` takes seconds.
                        .child(ph_ui::time::ago(hit.created_at / 1000)),
                )
                .child(
                    div()
                        .text_sm()
                        .line_clamp(2)
                        .child(snippet(&hit.snippet, cx)),
                )
        });
        v_flex()
            .track_focus(&self.focus_handle)
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.input)))
                    .when(self.project.is_some(), |this| {
                        this.child(
                            Button::new("chat-search-everywhere")
                                .small()
                                .ghost()
                                .label("All workspaces")
                                .selected(self.everywhere)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.everywhere = !this.everywhere;
                                    this.search(cx);
                                })),
                        )
                    }),
            )
            .child(
                v_flex()
                    .id("chat-search-results")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .children(rows)
                    .when(self.searched && self.hits.is_empty() && !empty, |this| {
                        this.child(
                            div()
                                .py_6()
                                .text_center()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from("No messages match.")),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_mark_the_matched_words() {
        let text = format!("why {MATCH_START}login{MATCH_END}\nand {MATCH_START}retry{MATCH_END}");
        let (plain, ranges) = marked(&text);
        assert_eq!(plain, "why login and retry");
        assert_eq!(ranges, vec![4..9, 14..19]);
    }
}
