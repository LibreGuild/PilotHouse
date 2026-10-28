//! `.env` files as a table of names and values, the values hidden until
//! asked for: a screen share or a screenshot does not show secrets.

use std::collections::HashSet;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::{
    ClipboardItem, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

#[derive(Debug, Clone, PartialEq)]
enum Line {
    Pair { name: String, value: String },
    Comment(String),
}

#[derive(Default)]
pub struct EnvPreview {
    lines: Vec<Line>,
    shown: HashSet<String>,
    show_all: bool,
}

impl EnvPreview {
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.lines = parse(text);
        cx.notify();
    }
}

fn parse(text: &str) -> Vec<Line> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            if let Some(comment) = line.strip_prefix('#') {
                return Some(Line::Comment(comment.trim().to_string()));
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (name, value) = line.split_once('=')?;
            Some(Line::Pair {
                name: name.trim().to_string(),
                value: unquote(value.trim()),
            })
        })
        .collect()
}

fn unquote(value: &str) -> String {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return value[1..value.len() - 1].to_string();
        }
    }
    // An unquoted value ends at a comment.
    value
        .split_once(" #")
        .map(|(value, _)| value.trim_end())
        .unwrap_or(value)
        .to_string()
}

impl Render for EnvPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        if self.lines.is_empty() {
            return crate::notice("No variables.", cx);
        }
        let mono = theme.mono_font_family.clone();
        let count = self
            .lines
            .iter()
            .filter(|line| matches!(line, Line::Pair { .. }))
            .count();
        let rows = self.lines.iter().enumerate().map(|(ix, line)| match line {
            Line::Comment(text) => div()
                .px_3()
                .pt_2()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("# {text}"))
                .into_any_element(),
            Line::Pair { name, value } => {
                let visible = self.show_all || self.shown.contains(name);
                let shown: SharedString = if value.is_empty() {
                    "(empty)".into()
                } else if visible {
                    value.clone().into()
                } else {
                    "•".repeat(value.chars().count().clamp(6, 16)).into()
                };
                let (toggle_name, copy) = (name.clone(), value.clone());
                h_flex()
                    .id(("env-row", ix))
                    .px_3()
                    .py_1()
                    .gap_3()
                    .rounded(theme.radius)
                    .hover(|this| this.bg(theme.list_hover))
                    .font_family(mono.clone())
                    .text_sm()
                    .child(
                        div()
                            .w(px(220.))
                            .flex_none()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if visible {
                                theme.foreground
                            } else {
                                theme.muted_foreground
                            })
                            .child(shown),
                    )
                    .child(
                        Button::new(("env-show", ix))
                            .ghost()
                            .xsmall()
                            .icon(if visible {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            })
                            .tooltip(if visible { "Hide" } else { "Show" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.shown.remove(&toggle_name) {
                                    this.shown.insert(toggle_name.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("env-copy", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .tooltip("Copy the value")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                            }),
                    )
                    .into_any_element()
            }
        });
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .flex_none()
                    .h_8()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(div().flex_1().child(format!(
                        "{count} variable{}; values are hidden until shown",
                        if count == 1 { "" } else { "s" }
                    )))
                    .child(
                        Button::new("env-show-all")
                            .ghost()
                            .xsmall()
                            .label(if self.show_all {
                                "Hide all"
                            } else {
                                "Show all"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_all = !this.show_all;
                                this.shown.clear();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("env-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .children(rows),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_dotenv_lines() {
        let lines = parse(
            "# Keys\nexport API_KEY=\"abc def\"\nPORT=8080 # web\nEMPTY=\nNAME='x'\nnot a pair\n",
        );
        assert_eq!(lines[0], Line::Comment("Keys".into()));
        let pairs: Vec<(String, String)> = lines
            .into_iter()
            .filter_map(|line| match line {
                Line::Pair { name, value } => Some((name, value)),
                Line::Comment(_) => None,
            })
            .collect();
        assert_eq!(
            pairs,
            [
                ("API_KEY".into(), "abc def".into()),
                ("PORT".into(), "8080".into()),
                ("EMPTY".into(), String::new()),
                ("NAME".into(), "x".into()),
            ]
        );
    }
}
