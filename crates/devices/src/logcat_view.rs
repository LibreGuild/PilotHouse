//! The Logcat drawer: lines in columns (time, level, tag, message) with
//! colours per level and tag, a minimum level, a filter box (see
//! [`crate::logcat`] for its words), an app picker, follow mode and a detail
//! strip for the selected line.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    menu::PopupMenuItem,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FontWeight,
    HighlightStyle, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText,
    Subscription, UniformListScrollHandle, Window, div, px, uniform_list,
};
use ph_media::ToChat;
use ph_ui::Icons;
use ph_ui::MenuButton as _;

use crate::logcat::{LogLine, Logcat, Priority, Query};

/// Lines sent to chat at most.
const SEND_LINES: usize = 80;
/// One row of the list.
const ROW_HEIGHT: f32 = 20.0;

pub struct LogcatView {
    logcat: Option<Entity<Logcat>>,
    device: String,
    packages: Vec<String>,
    filter: Entity<InputState>,
    min: Priority,
    /// Keep the newest line in view.
    follow: bool,
    selected: Option<Rc<LogLine>>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
    _logcat_subscription: Option<Subscription>,
}

impl EventEmitter<ToChat> for LogcatView {}

impl LogcatView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter: words, tag:Name, -tag:Name, package:com.app, pid:123")
        });
        Self {
            logcat: None,
            device: String::new(),
            packages: Vec::new(),
            filter: filter.clone(),
            min: Priority::Verbose,
            follow: true,
            selected: None,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: vec![cx.observe(&filter, |_, _, cx| cx.notify())],
            _logcat_subscription: None,
        }
    }

    /// The device's running logcat (`None` while the drawer is closed).
    pub fn set_logcat(
        &mut self,
        logcat: Option<Entity<Logcat>>,
        device: String,
        cx: &mut Context<Self>,
    ) {
        let same = match (&self.logcat, &logcat) {
            (Some(a), Some(b)) => a == b,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self._logcat_subscription = logcat
            .as_ref()
            .map(|logcat| cx.observe(logcat, |_, _, cx| cx.notify()));
        self.logcat = logcat;
        self.device = device;
        self.selected = None;
        cx.notify();
    }

    pub fn set_packages(&mut self, packages: Vec<String>, cx: &mut Context<Self>) {
        self.packages = packages;
        cx.notify();
    }

    /// Recent errors, for `@log`.
    pub fn recent_errors(&self, cx: &App) -> Option<String> {
        let logcat = self.logcat.as_ref()?.read(cx);
        let lines = logcat.recent(Priority::Error, 40);
        if lines.is_empty() {
            return None;
        }
        let text: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        Some(format!("Logcat errors:\n```\n{}\n```", text.join("\n")))
    }

    fn query(&self, cx: &App) -> Query {
        Query::parse(&self.filter.read(cx).value())
    }

    fn shown(&self, cx: &mut Context<Self>) -> Vec<Rc<LogLine>> {
        let Some(logcat) = self.logcat.clone() else {
            return Vec::new();
        };
        let (query, min) = (self.query(cx), self.min);
        logcat.update(cx, |logcat, cx| logcat.matching(&query, min, cx))
    }

    fn send(&mut self, lines: Vec<String>, cx: &mut Context<Self>) {
        if lines.is_empty() {
            return;
        }
        cx.emit(ToChat::Text {
            label: "logcat".into(),
            text: format!(
                "Logcat from {}:\n```\n{}\n```",
                self.device,
                lines.join("\n")
            ),
        });
    }

    fn send_shown(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown(cx);
        let lines = shown[shown.len().saturating_sub(SEND_LINES)..]
            .iter()
            .map(|line| line.text.clone())
            .collect();
        self.send(lines, cx);
    }

    /// Puts `token` in the filter box in place of any of the same kind.
    fn set_filter_token(
        &mut self,
        prefix: &str,
        token: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.filter.read(cx).value().to_string();
        let mut words: Vec<String> = current
            .split_whitespace()
            .filter(|word| !word.starts_with(prefix))
            .map(str::to_string)
            .collect();
        words.extend(token);
        let text = words.join(" ");
        self.filter
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let min = self.min;
        let level = Button::new("logcat-level")
            .ghost()
            .xsmall()
            .label(min.name())
            .dropdown_caret(true)
            .tooltip("Lowest level shown")
            .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                Priority::ALL[..5].iter().fold(menu, |menu, &priority| {
                    let view = view.clone();
                    menu.item(
                        PopupMenuItem::new(format!("{} and above", priority.name()))
                            .checked(priority == min)
                            .on_click(move |_, _, cx| {
                                _ = view.update(cx, |view, cx| {
                                    view.min = priority;
                                    cx.notify();
                                });
                            }),
                    )
                })
            });
        let view = cx.entity().downgrade();
        let packages = self.packages.clone();
        let package = self.query(cx).package;
        let app = Button::new("logcat-app")
            .ghost()
            .xsmall()
            .icon(Icons::Package)
            .label(package.clone().map_or("All apps".to_string(), |package| {
                package.rsplit('.').next().unwrap_or(&package).to_string()
            }))
            .dropdown_caret(true)
            .tooltip("Only one app's lines")
            .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                let pick = |name: Option<String>| {
                    let view = view.clone();
                    move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
                        _ = view.update(cx, |view, cx| {
                            view.set_filter_token(
                                "package:",
                                name.clone().map(|name| format!("package:{name}")),
                                window,
                                cx,
                            )
                        });
                    }
                };
                let menu = menu
                    .item(
                        PopupMenuItem::new("All apps")
                            .checked(package.is_none())
                            .on_click(pick(None)),
                    )
                    .separator();
                packages
                    .iter()
                    .fold(menu.scrollable(true).max_h(px(360.)), |menu, name| {
                        menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(package.as_deref() == Some(name.as_str()))
                                .on_click(pick(Some(name.clone()))),
                        )
                    })
            });
        h_flex()
            .flex_none()
            .px_2()
            .py_1()
            .gap_1()
            .child(level)
            .child(app)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.filter).xsmall().cleanable(true)),
            )
            .child(
                Button::new("logcat-follow")
                    .ghost()
                    .xsmall()
                    .icon(Icons::ArrowDownToLine)
                    .selected(self.follow)
                    .tooltip(if self.follow {
                        "Following new lines (scroll up to stop)"
                    } else {
                        "Follow new lines"
                    })
                    .accessibility_label("Follow new lines")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.follow = !this.follow;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("logcat-send")
                    .ghost()
                    .xsmall()
                    .icon(Icons::SendHorizontal)
                    .tooltip("Send the lines shown to chat (the last 80)")
                    .accessibility_label("Send to chat")
                    .on_click(cx.listener(|this, _, _, cx| this.send_shown(cx))),
            )
            .child(
                Button::new("logcat-clear")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Delete)
                    .tooltip("Clear")
                    .accessibility_label("Clear logcat")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected = None;
                        if let Some(logcat) = &this.logcat {
                            logcat.update(cx, |logcat, cx| logcat.clear(cx));
                        }
                    })),
            )
    }

    fn render_detail(&self, line: &LogLine, cx: &mut Context<Self>) -> impl IntoElement {
        let text = line.text.clone();
        let message = line.message.clone();
        h_flex()
            .flex_none()
            .items_start()
            .mx_2()
            .mb_1()
            .p_2()
            .gap_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(level_badge(line.priority, cx))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(tag_color(&line.tag, cx))
                                    .child(line.tag.clone()),
                            )
                            .child(format!(
                                "{} · pid {} · tid {}",
                                line.time, line.pid, line.tid
                            )),
                    )
                    .child(
                        div()
                            .id("logcat-detail")
                            .max_h(px(160.))
                            .overflow_y_scroll()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .child(message),
                    ),
            )
            .child(
                Button::new("logcat-copy-line")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Copy)
                    .tooltip("Copy the line")
                    .accessibility_label("Copy the line")
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
                    }),
            )
            .child(
                Button::new("logcat-send-line")
                    .ghost()
                    .xsmall()
                    .icon(Icons::SendHorizontal)
                    .tooltip("Send this line to chat")
                    .accessibility_label("Send this line to chat")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(line) = this.selected.clone() {
                            this.send(vec![line.text.clone()], cx);
                        }
                    })),
            )
            .child(
                Button::new("logcat-close-line")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Close")
                    .accessibility_label("Close")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected = None;
                        cx.notify();
                    })),
            )
    }
}

impl Render for LogcatView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lines: Rc<Vec<Rc<LogLine>>> = Rc::new(self.shown(cx));
        if self.follow && !lines.is_empty() {
            self.scroll
                .scroll_to_item(lines.len() - 1, ScrollStrategy::Bottom);
        }
        let words: Rc<Vec<String>> = Rc::new(self.query(cx).words);
        let selected = self.selected.clone();
        let list: AnyElement = if lines.is_empty() {
            div()
                .flex_1()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if self.logcat.is_none() {
                    "Logcat starts when a device is on screen."
                } else {
                    "No lines match."
                })
                .into_any_element()
        } else {
            let view = cx.entity().downgrade();
            uniform_list("logcat-rows", lines.len(), move |range, _, cx| {
                range
                    .map(|ix| {
                        let line = lines[ix].clone();
                        let chosen = selected
                            .as_ref()
                            .is_some_and(|picked| Rc::ptr_eq(picked, &line));
                        let view = view.clone();
                        log_row(ix, &line, &words, chosen, cx).on_click(move |_, _, cx| {
                            let line = line.clone();
                            _ = view.update(cx, |view, cx| {
                                view.selected = Some(line);
                                cx.notify();
                            });
                        })
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .flex_1()
            .on_scroll_wheel(
                cx.listener(|this, event: &gpui_kit::ScrollWheelEvent, window, cx| {
                    // Scrolling back stops following; reaching the end again resumes it.
                    let delta = event.delta.pixel_delta(window.line_height()).y;
                    if delta > px(0.) && this.follow {
                        this.follow = false;
                        cx.notify();
                    } else if delta < px(0.)
                        && this.scroll.is_scrolled_to_end() == Some(true)
                        && !this.follow
                    {
                        this.follow = true;
                        cx.notify();
                    }
                }),
            )
            .into_any_element()
        };
        v_flex()
            .size_full()
            .child(self.render_toolbar(cx))
            .when_some(self.selected.clone(), |this, line| {
                this.child(self.render_detail(&line, cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .child(list),
            )
    }
}

fn log_row(
    ix: usize,
    line: &LogLine,
    words: &[String],
    selected: bool,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let theme = cx.theme();
    let message_color = match line.priority {
        Priority::Fatal | Priority::Error => Some(theme.danger),
        Priority::Warn => Some(theme.warning),
        Priority::Verbose => Some(theme.muted_foreground),
        Priority::Debug | Priority::Info => None,
    };
    h_flex()
        .id(("logcat-row", ix))
        .h(px(ROW_HEIGHT))
        .px_2()
        .gap_2()
        .cursor_pointer()
        .when(selected, |this| this.bg(theme.list_active))
        .when(!selected, |this| {
            this.hover(|style| style.bg(theme.list_hover))
        })
        .when(
            matches!(line.priority, Priority::Error | Priority::Fatal),
            |this| this.border_l_2().border_color(theme.danger),
        )
        .child(
            div()
                .flex_none()
                .w(px(86.))
                .text_color(theme.muted_foreground)
                .child(line.time.clone()),
        )
        .child(level_badge(line.priority, cx))
        .child(
            div()
                .flex_none()
                .w(px(140.))
                .truncate()
                .text_color(tag_color(&line.tag, cx))
                .child(line.tag.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .when_some(message_color, |this, color| this.text_color(color))
                .child(highlighted(
                    &line.message,
                    words,
                    theme.warning.opacity(0.35),
                )),
        )
}

/// The level as a small tinted letter.
fn level_badge(priority: Priority, cx: &App) -> impl IntoElement {
    let color = level_color(priority, cx);
    div()
        .flex_none()
        .w(px(16.))
        .rounded(px(3.))
        .bg(color.opacity(0.18))
        .text_color(color)
        .font_weight(FontWeight::BOLD)
        .text_center()
        .child(priority.letter())
}

fn level_color(priority: Priority, cx: &App) -> Hsla {
    let theme = cx.theme();
    match priority {
        Priority::Verbose => theme.muted_foreground,
        Priority::Debug => theme.blue,
        Priority::Info => theme.green,
        Priority::Warn => theme.warning,
        Priority::Error | Priority::Fatal => theme.danger,
    }
}

/// A steady colour per tag, so a tag is easy to follow down the list.
fn tag_color(tag: &str, cx: &App) -> Hsla {
    let theme = cx.theme();
    let palette = [
        theme.blue,
        theme.cyan,
        theme.magenta,
        theme.green,
        theme.yellow,
        theme.blue_light,
        theme.cyan_light,
        theme.magenta_light,
    ];
    let hash = tag.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u32::from(byte))
    });
    palette[hash as usize % palette.len()]
}

/// `text` with every filter word marked.
fn highlighted(text: &str, words: &[String], mark: Hsla) -> StyledText {
    let text: SharedString = text.to_string().into();
    let ranges = match_ranges(&text, words);
    let style = HighlightStyle {
        background_color: Some(mark),
        ..Default::default()
    };
    StyledText::new(text).with_highlights(ranges.into_iter().map(|range| (range, style)))
}

/// Byte ranges of `words` in `text`, any case, in order and not overlapping.
fn match_ranges(text: &str, words: &[String]) -> Vec<std::ops::Range<usize>> {
    if words.is_empty() {
        return Vec::new();
    }
    // Lowercasing keeps byte offsets only for ASCII; otherwise mark nothing.
    if !text.is_ascii() {
        return Vec::new();
    }
    let lower = text.to_ascii_lowercase();
    let mut ranges: Vec<std::ops::Range<usize>> = words
        .iter()
        .filter(|word| !word.is_empty())
        .flat_map(|word| {
            lower
                .match_indices(word.as_str())
                .map(|(at, found)| at..at + found.len())
                .collect::<Vec<_>>()
        })
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_filter_words_once() {
        let words = vec!["api".to_string(), "pi c".to_string()];
        assert_eq!(match_ranges("API call to api", &words), vec![0..5, 12..15]);
        assert!(match_ranges("naïve api", &words).is_empty());
        assert!(match_ranges("text", &[]).is_empty());
    }
}
