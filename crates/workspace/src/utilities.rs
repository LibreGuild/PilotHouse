//! The Utilities dialog: [`ph_utils`]'s offline tools (formats, encodings,
//! hashes, ids, times, regex, diff, colors, SQLite). Each runs as you type;
//! nothing leaves the machine.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px,
};
use ph_utils::{Input, UTILITIES, Utility};

/// Opens the dialog, at utility `id` (or the first).
pub fn open(id: Option<&str>, window: &mut Window, cx: &mut App) {
    let selected = id
        .and_then(|id| UTILITIES.iter().position(|utility| utility.id == id))
        .unwrap_or(0);
    let view = cx.new(|cx| UtilitiesView::new(selected, window, cx));
    let focus = view.read(cx).text.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Utilities").w(px(880.)).child(view.clone())
    });
    focus.update(cx, |text, cx| text.focus(window, cx));
}

struct UtilitiesView {
    selected: usize,
    mode: usize,
    text: Entity<TextareaState>,
    second: Entity<TextareaState>,
    output: Result<String, String>,
    _inputs: Vec<Subscription>,
}

impl UtilitiesView {
    fn new(selected: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let area = |rows: usize, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| TextareaState::new(window, cx).rows(rows))
        };
        let (text, second) = (area(5, window, cx), area(3, window, cx));
        let inputs = [&text, &second]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.run(cx);
                    }
                })
            })
            .collect();
        let mut view = Self {
            selected,
            mode: 0,
            text,
            second,
            output: Ok(String::new()),
            _inputs: inputs,
        };
        view.select(selected, window, cx);
        view
    }

    fn utility(&self) -> &'static Utility {
        &UTILITIES[self.selected]
    }

    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = ix;
        self.mode = 0;
        let utility = self.utility();
        self.text.update(cx, |text, cx| {
            text.set_placeholder(utility.input, window, cx);
        });
        self.second.update(cx, |second, cx| {
            second.set_placeholder(utility.second.unwrap_or(""), window, cx);
        });
        self.run(cx);
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        let input = Input {
            text: self.text.read(cx).value().to_string(),
            second: self.second.read(cx).value().to_string(),
            mode: self.mode,
        };
        // Ids and the current time make sense without input; others wait.
        let wants_input = !matches!(self.utility().id, "ids" | "time");
        self.output = if wants_input && input.text.trim().is_empty() {
            Ok(String::new())
        } else {
            ph_utils::run(self.utility().id, &input).map_err(|err| format!("{err:#}"))
        };
        cx.notify();
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut group = "";
        let mut items = Vec::new();
        for (ix, utility) in UTILITIES.iter().enumerate() {
            if utility.group != group {
                group = utility.group;
                items.push(
                    div()
                        .pt_2()
                        .px_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(group)
                        .into_any_element(),
                );
            }
            let selected = ix == self.selected;
            items.push(
                div()
                    .id(("utility", ix))
                    .px_2()
                    .py_0p5()
                    .rounded(cx.theme().radius)
                    .text_sm()
                    .cursor_pointer()
                    .when(selected, |this| this.bg(cx.theme().list_active))
                    .hover(|style| style.bg(cx.theme().list_hover))
                    .on_click(cx.listener(move |this, _, window, cx| this.select(ix, window, cx)))
                    .child(utility.name)
                    .into_any_element(),
            );
        }
        v_flex()
            .id("utilities-list")
            .w(px(170.))
            .flex_none()
            .h(px(440.))
            .overflow_y_scroll()
            .pr_2()
            .border_r_1()
            .border_color(cx.theme().border)
            .children(items)
    }
}

impl Render for UtilitiesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let utility = self.utility();
        let muted = cx.theme().muted_foreground;
        let output = self.output.clone();
        let copy = output.clone().unwrap_or_default();
        h_flex()
            .items_start()
            .gap_3()
            .child(self.render_list(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(
                                div()
                                    .mr_2()
                                    .text_base()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(utility.name),
                            )
                            .when(utility.modes.len() > 1, |this| {
                                this.children(utility.modes.iter().enumerate().map(|(ix, mode)| {
                                    Button::new(("utility-mode", ix))
                                        .xsmall()
                                        .when(ix == self.mode, |this| this.primary())
                                        .when(ix != self.mode, |this| this.ghost())
                                        .label(*mode)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.mode = ix;
                                            this.run(cx);
                                        }))
                                }))
                            }),
                    )
                    .child(Textarea::new(&self.text))
                    .when(utility.second.is_some(), |this| {
                        this.child(Textarea::new(&self.second))
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().text_xs().text_color(muted).child(
                                match &output {
                                    Ok(text) if text.is_empty() => {
                                        "The result shows here as you type."
                                    }
                                    Ok(_) => "Result",
                                    Err(_) => "That did not work:",
                                },
                            ))
                            .child(
                                Button::new("utility-copy")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Copy)
                                    .label("Copy")
                                    .disabled(copy.is_empty())
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("utility-output")
                            .h(px(180.))
                            .overflow_y_scroll()
                            .p_2()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().muted.opacity(0.5))
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .when(output.is_err(), |this| this.text_color(cx.theme().danger))
                            .child(SharedString::from(match output {
                                Ok(text) | Err(text) => text,
                            })),
                    ),
            )
    }
}
