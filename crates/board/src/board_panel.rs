//! The Board tab: four columns of cards with drag and drop.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    tag::Tag,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Point, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px,
};
use ph_store::Store;
use ph_store::board::{Card, CardId, CardStatus};
use ph_store::projects::ProjectId;
use ph_ui::Icons;

use crate::hub::{BoardChanged, BoardHub};
use crate::{card_editor, plan_dialog, prompts};

/// Requests the workspace routes to the chat.
#[derive(Debug, Clone)]
pub enum BoardPanelEvent {
    /// Send `prompt` to the project's agent; a new chat is named `title`.
    SendToAgent { prompt: String, title: String },
}

/// What is being dragged: one card.
#[derive(Clone)]
struct DraggedCard {
    id: CardId,
    title: SharedString,
}

/// The floating preview that follows the pointer during a drag.
struct DragPreview {
    title: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(COLUMN_WIDTH)
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().drag_border)
            .bg(cx.theme().popover)
            .shadow_lg()
            .text_sm()
            .child(self.title.clone())
    }
}

/// Columns keep one width so cards line up and the board scrolls sideways
/// when the panel is narrow.
const COLUMN_WIDTH: Pixels = px(248.);

pub struct BoardPanel {
    store: Store,
    project_id: ProjectId,
    cards: Vec<Card>,
    /// How many UI tests each card needs to pass to be Done.
    test_counts: std::collections::HashMap<CardId, usize>,
    error: Option<SharedString>,
    _subscription: Subscription,
}

impl EventEmitter<BoardPanelEvent> for BoardPanel {}

impl BoardPanel {
    /// Sends the planning prompt for `goal` to the agent.
    pub(crate) fn request_plan(&mut self, goal: &str, cx: &mut Context<Self>) {
        let goal_line = goal.lines().next().unwrap_or(goal);
        cx.emit(BoardPanelEvent::SendToAgent {
            prompt: prompts::plan(goal),
            title: format!("Plan: {goal_line}"),
        });
    }

    pub fn new(store: Store, project_id: ProjectId, cx: &mut Context<Self>) -> Self {
        let events = BoardHub::events(cx);
        let subscription = cx.subscribe(&events, |this, _, event: &BoardChanged, cx| {
            if event.project_id == this.project_id {
                this.reload(cx);
            }
        });
        let mut panel = Self {
            store,
            project_id,
            cards: Vec::new(),
            test_counts: Default::default(),
            error: None,
            _subscription: subscription,
        };
        panel.reload(cx);
        panel
    }

    /// Cards in To do and In progress, for the status bar.
    pub fn open_card_count(&self) -> usize {
        self.cards
            .iter()
            .filter(|card| matches!(card.status, CardStatus::Todo | CardStatus::InProgress))
            .count()
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        match self.store.cards(self.project_id) {
            Ok(cards) => {
                self.cards = cards;
                self.test_counts = self
                    .store
                    .card_test_counts(self.project_id)
                    .unwrap_or_default();
                self.error = None;
            }
            Err(err) => self.error = Some(format!("Could not load the board: {err:#}").into()),
        }
        cx.notify();
    }

    fn move_card(
        &mut self,
        id: CardId,
        status: CardStatus,
        before: Option<CardId>,
        cx: &mut Context<Self>,
    ) {
        if before == Some(id) {
            return;
        }
        match crate::may_finish(&self.store, id, status)
            .and_then(|()| self.store.move_card(id, status, before).map(|_| ()))
        {
            Ok(()) => BoardHub::notify(self.project_id, cx),
            // Nothing moved; the reason stays until the next change.
            Err(err) => {
                self.error = Some(format!("{err:#}").into());
                cx.notify();
            }
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.clone();
        let project_id = self.project_id;
        let has_todo = self
            .cards
            .iter()
            .any(|card| card.status == CardStatus::Todo);

        h_flex()
            .flex_none()
            .px_3()
            .py_2()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} cards · {} open",
                        self.cards.len(),
                        self.open_card_count()
                    )),
            )
            .child(
                Button::new("board-plan")
                    .ghost()
                    .small()
                    .icon(Icons::ListTodo)
                    .label("Plan with agent")
                    .tooltip("Describe a goal; the agent breaks it into cards")
                    .on_click(cx.listener(|_, _, window, cx| {
                        let board = cx.weak_entity();
                        plan_dialog::open(board, window, cx);
                    })),
            )
            .child(
                Button::new("board-work")
                    .ghost()
                    .small()
                    .icon(Icons::Bot)
                    .label("Work through board")
                    .disabled(!has_todo)
                    .tooltip("Ask the agent to take cards from To do one by one")
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(BoardPanelEvent::SendToAgent {
                            prompt: prompts::work_through_board(),
                            title: "Work through the board".into(),
                        });
                    })),
            )
            .child(
                Button::new("board-add-card")
                    .outline()
                    .small()
                    .icon(Icons::Plus)
                    .label("Add card")
                    .on_click(move |_, window, cx| {
                        card_editor::open_new(store.clone(), project_id, window, cx)
                    }),
            )
    }

    fn render_column(&self, status: CardStatus, cx: &mut Context<Self>) -> AnyElement {
        let cards: Vec<&Card> = self
            .cards
            .iter()
            .filter(|card| card.status == status)
            .collect();
        let count = cards.len();
        let panel = cx.weak_entity();

        v_flex()
            .id(SharedString::from(format!("board-column-{}", status.key())))
            .flex_none()
            .w(COLUMN_WIDTH)
            .h_full()
            .rounded(cx.theme().radius_lg)
            .bg(cx.theme().muted.opacity(0.5))
            .drag_over::<DraggedCard>(|style, _, _, cx| style.bg(cx.theme().drop_target))
            .on_drop(move |dragged: &DraggedCard, _, cx| {
                _ = panel.update(cx, |panel, cx| {
                    panel.move_card(dragged.id, status, None, cx)
                });
            })
            .child(
                h_flex()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(status.label()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(count.to_string()),
                    ),
            )
            .child(
                v_flex()
                    .id(SharedString::from(format!("board-cards-{}", status.key())))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .gap_2()
                    .children(cards.into_iter().map(|card| self.render_card(card, cx)))
                    .when(count == 0, |this| {
                        this.child(
                            div()
                                .py_6()
                                .text_center()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if status == CardStatus::Todo {
                                    "Add cards, or ask an agent to plan the work."
                                } else {
                                    "Drop cards here."
                                }),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_card(&self, card: &Card, cx: &mut Context<Self>) -> AnyElement {
        let dragged = DraggedCard {
            id: card.id,
            title: card.title.clone().into(),
        };
        let open_card = card.clone();
        let store = self.store.clone();
        let panel = cx.weak_entity();
        let status = card.status;
        let anchor = card.id;
        let description = card
            .description
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();

        v_flex()
            .id(("board-card", card.id as u64))
            .gap_1()
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .cursor_pointer()
            .hover(|style| style.border_color(cx.theme().ring.opacity(0.6)))
            .on_drag(
                dragged,
                |dragged: &DraggedCard, _: Point<Pixels>, _, cx: &mut App| {
                    let title = dragged.title.clone();
                    cx.new(|_| DragPreview { title })
                },
            )
            // Dropping onto a card inserts the dragged card above it.
            .drag_over::<DraggedCard>(|style, _, _, cx| style.border_color(cx.theme().drag_border))
            .on_drop(move |dragged: &DraggedCard, _, cx| {
                _ = panel.update(cx, |panel, cx| {
                    panel.move_card(dragged.id, status, Some(anchor), cx)
                });
            })
            .on_click(move |_, window, cx| {
                card_editor::open_existing(store.clone(), open_card.clone(), window, cx)
            })
            .child(div().text_sm().child(card.title.clone()))
            .when(!description.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .line_clamp(2)
                        .child(description),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("#{}", card.id))
                    .when_some(self.test_counts.get(&card.id), |this, count| {
                        this.child(
                            h_flex()
                                .gap_0p5()
                                .child(Icon::new(Icons::FlaskConical).xsmall())
                                .child(format!("{count} to pass")),
                        )
                    })
                    .when_some(card.assignee.clone(), |this, assignee| {
                        this.child(
                            Tag::secondary().small().child(
                                h_flex()
                                    .gap_1()
                                    .child(Icon::new(Icons::Bot).xsmall())
                                    .child(assignee),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Render for BoardPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .px_3()
                        .pb_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .id("board-columns")
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .px_3()
                    .pb_3()
                    .gap_3()
                    .overflow_x_scroll()
                    .children(CardStatus::ALL.map(|status| self.render_column(status, cx))),
            )
    }
}
