//! Stashes: put the current changes aside, look at them, bring them back.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Task, Window, div,
};
use ph_git::parse_diff;
use ph_git::stash::{self, Stash};

use crate::branch_picker::ago;
use crate::diff_list::Diffs;
use crate::repo::Repo;

pub struct StashView {
    repo: Entity<Repo>,
    message: Entity<InputState>,
    stashes: Vec<Stash>,
    /// The stash whose changes show, by its index, and its diffs.
    open: Option<(usize, Diffs)>,
    /// The view as a list drawn only where it shows: an open stash can be
    /// a large diff (see [`crate::diff_list`]).
    list: gpui_kit::ListState,
    pieces: Vec<Piece>,
    _load: Option<Task<()>>,
}

/// A row of the stash list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piece {
    /// The message and the Stash button.
    New,
    /// Says there are none.
    Empty,
    /// A stash's title and buttons (by its place in `stashes`).
    Stash(usize),
    /// Item `n` of the open stash's diffs.
    Diff(usize),
    /// The bottom of the open stash's card.
    End,
}

impl StashView {
    pub fn new(repo: Entity<Repo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message =
            cx.new(|cx| InputState::new(window, cx).placeholder("Describe the stash (optional)"));
        cx.observe(&repo, |this, _, cx| this.reload(cx)).detach();
        let mut view = Self {
            repo,
            message,
            stashes: Vec::new(),
            open: None,
            list: gpui_kit::ListState::new(0, gpui_kit::ListAlignment::Top, gpui_kit::px(800.)),
            pieces: Vec::new(),
            _load: None,
        };
        view.sync_list();
        view.reload(cx);
        view
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let root = self.repo.read(cx).root().to_path_buf();
        let read = cx.background_spawn(async move { stash::list(&root) });
        self._load = Some(cx.spawn(async move |this, cx| {
            let stashes = read.await.unwrap_or_default();
            _ = this.update(cx, |this, cx| {
                if this.stashes != stashes {
                    this.stashes = stashes;
                    this.open = None;
                    this.sync_list();
                    cx.notify();
                }
            });
        }));
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.message.read(cx).value().to_string();
        self.message
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.repo.update(cx, |repo, cx| {
            repo.run(
                "Stashing",
                move |root| stash::push(root, &message).map(|_| "Changes stashed".into()),
                cx,
            )
        });
    }

    fn act(&mut self, index: usize, action: Action, cx: &mut Context<Self>) {
        let hash = self
            .stashes
            .iter()
            .find(|stash| stash.index == index)
            .map(|stash| stash.hash.clone())
            .unwrap_or_default();
        self.repo.update(cx, |repo, cx| match action {
            Action::Apply => repo.run(
                "Applying stash",
                move |root| stash::apply(root, index, &hash).map(|_| "Stash applied".into()),
                cx,
            ),
            Action::Pop => repo.run(
                "Popping stash",
                move |root| {
                    stash::pop(root, index, &hash).map(|_| "Stash applied and removed".into())
                },
                cx,
            ),
            Action::Drop => repo.run(
                "Dropping stash",
                move |root| stash::drop(root, index, &hash).map(|_| "Stash dropped".into()),
                cx,
            ),
        });
    }

    fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(|(open, _)| *open == index) {
            self.open = None;
            self.sync_list();
            cx.notify();
            return;
        }
        let root = self.repo.read(cx).root().to_path_buf();
        let read = cx.background_spawn(async move {
            let patch = stash::show(&root, index).unwrap_or_default();
            Diffs::build(&parse_diff(&patch))
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let diffs = read.await;
            _ = this.update(cx, |this, cx| {
                this.open = Some((index, diffs));
                this.sync_list();
                cx.notify();
            });
        }));
    }

    /// The rows as they are now. Only those that changed are measured
    /// again, so opening a stash keeps the list where it was.
    fn sync_list(&mut self) {
        let mut pieces = vec![Piece::New];
        if self.stashes.is_empty() {
            pieces.push(Piece::Empty);
        }
        for (at, stash) in self.stashes.iter().enumerate() {
            pieces.push(Piece::Stash(at));
            if let Some((_, diffs)) = self.open.as_ref().filter(|(open, _)| *open == stash.index) {
                pieces.extend((0..diffs.len()).map(Piece::Diff));
                pieces.push(Piece::End);
            }
        }
        let same_start = self
            .pieces
            .iter()
            .zip(&pieces)
            .take_while(|(was, now)| was == now)
            .count();
        let same_end = self.pieces[same_start..]
            .iter()
            .rev()
            .zip(pieces[same_start..].iter().rev())
            .take_while(|(was, now)| was == now)
            .count();
        // A stash opened in place of another draws other diffs in the same
        // rows: measure those again too.
        let (same_start, same_end) = if self.open.is_some() {
            let first_diff = |pieces: &[Piece]| {
                pieces
                    .iter()
                    .position(|piece| matches!(piece, Piece::Diff(_) | Piece::End))
                    .unwrap_or(pieces.len())
            };
            let start = same_start
                .min(first_diff(&self.pieces))
                .min(first_diff(&pieces));
            (start, 0)
        } else {
            (same_start, same_end)
        };
        self.list.splice(
            same_start..self.pieces.len() - same_end,
            pieces.len() - same_start - same_end,
        );
        self.pieces = pieces;
    }

    /// Row `ix` of the view (see [`Piece`]).
    fn render_piece(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(&piece) = self.pieces.get(ix) else {
            return div().into_any_element();
        };
        // Cards are apart by a gap; the view has a margin all round.
        let last = ix + 1 == self.pieces.len();
        let row = div()
            .px_3()
            .when(ix == 0, |this| this.pt_3())
            .when(last, |this| this.pb_3());
        let gap = |this: gpui_kit::Div| this.when(!last, |this| this.pb_2());
        let border = cx.theme().border;
        let radius = cx.theme().radius;
        match piece {
            Piece::New => gap(row).child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.message).small()))
                    .child(
                        Button::new("stash-save")
                            .small()
                            .outline()
                            .icon(IconName::Inbox)
                            .label("Stash changes")
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            ),
            Piece::Empty => gap(row).child(
                div()
                    .py_4()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "No stashes. Stashing puts your changes (new files too) aside for later.",
                    ),
            ),
            Piece::Stash(at) => {
                let Some(stash) = self.stashes.get(at) else {
                    return div().into_any_element();
                };
                let open = self
                    .pieces
                    .get(ix + 1)
                    .is_some_and(|next| matches!(next, Piece::Diff(_) | Piece::End));
                let head = self.render_stash(stash, cx);
                row.when(!open, gap).child(
                    div()
                        .border_1()
                        .border_color(border)
                        .map(|this| {
                            if open {
                                this.rounded_t(radius)
                            } else {
                                this.rounded(radius)
                            }
                        })
                        .child(head),
                )
            }
            Piece::Diff(item) => {
                let Some((_, diffs)) = &self.open else {
                    return div().into_any_element();
                };
                let first = item == 0;
                row.child(
                    div()
                        .px_2()
                        .when(first, |this| this.pt_2())
                        .border_l_1()
                        .border_r_1()
                        .border_color(border)
                        .child(diffs.render_item("stash", item, cx)),
                )
            }
            Piece::End => {
                let empty = self.open.as_ref().is_none_or(|(_, diffs)| diffs.is_empty());
                gap(row).child(
                    div()
                        .pb_2()
                        .when(empty, |this| this.pt_2())
                        .border_l_1()
                        .border_r_1()
                        .border_b_1()
                        .border_color(border)
                        .rounded_b(radius),
                )
            }
        }
        .into_any_element()
    }

    /// A stash's title, when it was made and its buttons.
    fn render_stash(&self, stash: &Stash, cx: &mut Context<Self>) -> AnyElement {
        let index = stash.index;
        let button =
            |id: &'static str, label: &'static str, action: Action, cx: &mut Context<Self>| {
                Button::new((id, index))
                    .xsmall()
                    .ghost()
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.act(index, action, cx)))
            };
        h_flex()
            .id(("stash", index))
            .gap_2()
            .px_2()
            .py_1p5()
            .cursor_pointer()
            .hover(|style| style.bg(cx.theme().list_hover))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(index, cx)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().truncate().child(stash.message.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("stash@{{{index}}} · {}", ago(stash.created))),
                    ),
            )
            .child(button("stash-apply", "Apply", Action::Apply, cx))
            .child(button("stash-pop", "Pop", Action::Pop, cx))
            .child(button("stash-drop", "Drop", Action::Drop, cx))
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum Action {
    Apply,
    Pop,
    Drop,
}

impl Render for StashView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::scroll::ScrollableElement as _;
        let entity = cx.entity().downgrade();
        div()
            .relative()
            .size_full()
            .child(
                gpui_kit::list(self.list.clone(), move |ix, _, cx| {
                    entity
                        .update(cx, |this, cx| this.render_piece(ix, cx))
                        .unwrap_or_else(|_| div().into_any_element())
                })
                .size_full(),
            )
            .vertical_scrollbar(&self.list)
    }
}
