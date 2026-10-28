//! The transcript's virtual list and its jump-to-latest button.
//!
//! Like gpui-kit's `MessageScroller`, with some slack at the end. The list
//! library counts the view as "at the latest" only on the very last pixel,
//! and not at all while rows above are still unmeasured, so its button
//! stayed up at the bottom of a chat until clicked. Here anything within
//! [`NEAR_END`] of the end counts as the end: the button hides, and a row
//! that arrives or grows keeps the view on the newest text.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, IconName};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, Entity, FollowMode, InteractiveElement as _, IntoElement,
    ListAlignment, ListOffset, ListState, ParentElement as _, Pixels, RenderOnce, Role,
    StatefulInteractiveElement as _, Styled as _, Window, div, list, px, rems,
};

/// How far above the end the view still counts as at the end.
const NEAR_END: Pixels = px(48.);
/// Space under the last row, so the newest text does not sit on the edge.
const END_SPACE: Pixels = px(20.);
/// Rows drawn beyond the view, so scrolling shows no blank rows.
const OVERDRAW: Pixels = px(400.);
/// After the user scrolls, new rows leave the view alone this long: a slow
/// scroll up near the end is not pulled back down by the next streamed word.
const SCROLL_GRACE: Duration = Duration::from_millis(400);
const BUTTON_FADE: Duration = Duration::from_millis(200);

pub struct TranscriptScroll {
    list: ListState,
    scrolled_at: Option<Instant>,
}

impl TranscriptScroll {
    /// A list of `count` rows that follows its newest row.
    pub fn new(count: usize, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(count, ListAlignment::Top, OVERDRAW);
        list.set_follow_mode(FollowMode::Tail);
        let this = cx.weak_entity();
        list.set_scroll_handler(move |_, _, cx| {
            let this = this.clone();
            // The list is still borrowed while it reports the scroll.
            cx.defer(move |cx| {
                _ = this.update(cx, |this, cx| {
                    this.scrolled_at = Some(Instant::now());
                    cx.notify();
                });
            });
        });
        Self {
            list,
            scrolled_at: None,
        }
    }

    /// How far the end of the newest row is below the view.
    fn distance_to_end(&self) -> Pixels {
        // Only the rows from the top of the view down count, and those are
        // measured: they are on screen or within the overdraw.
        (self.list.max_offset_for_scrollbar().y + self.list.scroll_px_offset_for_scrollbar().y)
            .max(px(0.))
    }

    fn near_end(&self) -> bool {
        self.list.is_following_tail() || self.distance_to_end() <= NEAR_END
    }

    /// Whether the newest rows are out of view (the jump button shows).
    pub fn is_scrolled_up(&self) -> bool {
        !self.near_end()
    }

    /// Before rows change: keeps a view near the end on the newest text,
    /// unless the user is scrolling.
    fn hold_end(&self) {
        let scrolling = self
            .scrolled_at
            .is_some_and(|at| at.elapsed() < SCROLL_GRACE);
        if !self.list.is_following_tail() && !scrolling && self.near_end() {
            self.list.set_follow_mode(FollowMode::Tail);
        }
    }

    /// Replaces the rows in `old` with `count` new ones.
    pub fn splice(&mut self, old: Range<usize>, count: usize, cx: &mut Context<Self>) {
        if old.start > old.end || old.end > self.list.item_count() {
            return;
        }
        self.hold_end();
        self.list.splice(old, count);
        cx.notify();
    }

    pub fn append(&mut self, count: usize, cx: &mut Context<Self>) {
        let end = self.list.item_count();
        self.splice(end..end, count, cx);
    }

    /// Measures every row again (the view mode changed).
    pub fn remeasure(&mut self, cx: &mut Context<Self>) {
        self.hold_end();
        self.list.remeasure();
        cx.notify();
    }

    /// Measures the rows in `rows` again (they grew or changed).
    pub fn remeasure_items(&mut self, rows: Range<usize>, cx: &mut Context<Self>) {
        let rows = rows.start.min(rows.end)..rows.end.min(self.list.item_count());
        if rows.is_empty() {
            return;
        }
        self.hold_end();
        self.list.remeasure_items(rows);
        cx.notify();
    }

    /// Shows row `ix` at the top (a search result).
    pub fn scroll_to_item(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.list.item_count() {
            return;
        }
        self.list.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: px(0.),
        });
        cx.notify();
    }

    /// Back to the newest row, following it again.
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.scrolled_at = None;
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        cx.notify();
    }
}

/// Draws the row at an index.
type RenderRow = Box<dyn FnMut(usize, &mut Window, &mut App) -> AnyElement>;

/// The transcript: one row per entry, with the jump-to-latest button.
#[derive(IntoElement)]
pub struct TranscriptList {
    id: ElementId,
    state: Entity<TranscriptScroll>,
    render_row: RenderRow,
}

impl TranscriptList {
    pub fn new<E: IntoElement>(
        id: impl Into<ElementId>,
        state: Entity<TranscriptScroll>,
        mut render_row: impl FnMut(usize, &mut Window, &mut App) -> E + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            state,
            render_row: Box::new(move |ix, window, cx| {
                render_row(ix, window, cx).into_any_element()
            }),
        }
    }
}

impl RenderOnce for TranscriptList {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (list_state, scrolled_up) = {
            let state = self.state.read(cx);
            (state.list.clone(), state.is_scrolled_up())
        };
        let shown = transition(
            (self.id.clone(), "jump"),
            if scrolled_up { 1. } else { 0. },
            Transition::new(BUTTON_FADE),
            window,
            cx,
        );
        let mut render_row = self.render_row;
        // Rows space themselves by what they follow (see `entry_view`).
        let rows = list(list_state.clone(), move |ix, window, cx| {
            div()
                .w_full()
                .min_w_0()
                .px_3()
                .child(render_row(ix, window, cx))
                .into_any_element()
        })
        .size_full()
        .min_h_0()
        .pt_2()
        .pb(END_SPACE);
        let viewport = div()
            .id((self.id.clone(), "viewport"))
            .role(Role::Log)
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                // The bounce moves the list past its own scroll handler, so
                // it tells the state itself.
                gpui_kit::base::ScrollBounce::new((self.id.clone(), "bounce"), &list_state, rows)
                    .on_scroll({
                        let state = self.state.downgrade();
                        move |_, cx| {
                            _ = state.update(cx, |state, cx| {
                                state.scrolled_at = Some(Instant::now());
                                cx.notify();
                            });
                        }
                    }),
            )
            .vertical_scrollbar(&list_state);
        let theme = cx.theme();
        let (border, background, foreground) = (theme.border, theme.background, theme.foreground);
        div()
            .id(self.id.clone())
            .relative()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(viewport)
            .when(shown > 0., |this| {
                let state = self.state.clone();
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(rems(0.5 + shown * 0.5))
                        .flex()
                        .justify_center()
                        .opacity(shown)
                        .child(
                            Button::new((self.id, "jump-to-latest"))
                                .secondary()
                                .icon(IconName::ArrowDown)
                                .tooltip("Jump to latest")
                                .rounded_full()
                                .border_1()
                                .border_color(border)
                                .bg(background)
                                .text_color(foreground)
                                .on_click(move |_, _, cx| {
                                    state.update(cx, |state, cx| state.scroll_to_end(cx));
                                }),
                        ),
                )
            })
    }
}
