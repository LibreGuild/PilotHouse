//! Regex, diff and Markdown.
//!
//! A regex's matches are marked in the text itself as the pattern is
//! typed, with each match's groups listed; Replace shows the text after.
//! Two texts are compared as the Git tab compares files, side by side or
//! in one column. Markdown is shown as it reads, or as its HTML.

use std::sync::Arc;

use gpui_kit::base::text::TextView;
use gpui_kit::component::diff::{Diff as DiffView, DiffFile, DiffMode, DiffState};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{
        Editor, EditorState, Input as Field, InputEvent, InputState, TextDecoration,
        TextDecorationCollection,
    },
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, HighlightStyle, ImageCacheError,
    ImageSource, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
};
use ph_ui::Icons;

use super::parts::{self, Tone};
use super::{Ctx, ToolView};
use crate::text::{Flags, Found};

/// A moment after the last key.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(120);

/// What a pattern did with the text.
struct Outcome {
    found: Vec<Found>,
    more: bool,
    replaced: Option<String>,
}

pub(crate) struct Regex {
    ctx: Ctx,
    replace: bool,
    flags: Flags,
    pattern: Entity<InputState>,
    text: Entity<EditorState>,
    with: Entity<InputState>,
    output: Entity<EditorState>,
    outcome: Option<Result<Outcome, String>>,
    marks: Option<TextDecorationCollection>,
    _inputs: Vec<Subscription>,
    _run: Option<Task<()>>,
}

impl Regex {
    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pattern = self.pattern.read(cx).value().to_string();
        let text = self.text.read(cx).value().to_string();
        let with = self.replace.then(|| self.with.read(cx).value().to_string());
        if pattern.is_empty() {
            self._run = None;
            self.show(None, window, cx);
            return;
        }
        let flags = self.flags;
        self._run = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let outcome = cx
                .background_spawn(async move {
                    let pattern =
                        crate::text::pattern(&pattern, flags).map_err(|err| format!("{err:#}"))?;
                    let (found, more) =
                        crate::text::find_all(&pattern, &text).map_err(|err| format!("{err:#}"))?;
                    let replaced = match with {
                        Some(with) => Some(
                            crate::text::replace_all(&pattern, &text, &with)
                                .map_err(|err| format!("{err:#}"))?,
                        ),
                        None => None,
                    };
                    Ok(Outcome {
                        found,
                        more,
                        replaced,
                    })
                })
                .await;
            _ = this.update_in(cx, |this, window, cx| this.show(Some(outcome), window, cx));
        }));
    }

    fn show(
        &mut self,
        outcome: Option<Result<Outcome, String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let color = cx.theme().warning.opacity(0.35);
        let marks: Vec<TextDecoration> = match &outcome {
            Some(Ok(outcome)) => outcome
                .found
                .iter()
                .filter(|found| !found.range.is_empty())
                .map(|found| {
                    TextDecoration::new(
                        found.range.clone(),
                        HighlightStyle {
                            background_color: Some(color),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        match &self.marks {
            Some(collection) => collection.set(marks, cx),
            None => {
                self.marks = Some(
                    self.text
                        .update(cx, |text, cx| text.create_decorations_collection(marks, cx)),
                );
            }
        }
        let replaced = match &outcome {
            Some(Ok(Outcome {
                replaced: Some(replaced),
                ..
            })) => replaced.clone(),
            _ => String::new(),
        };
        self.output
            .update(cx, |output, cx| output.set_value(replaced, window, cx));
        self.outcome = outcome;
        cx.notify();
    }

    fn render_flags(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let flag = |id: &'static str,
                    letter: &'static str,
                    tip: &'static str,
                    on: bool,
                    set: fn(&mut Flags)| {
            Button::new(id)
                .xsmall()
                .label(letter)
                .map(|this| if on { this.primary() } else { this.ghost() })
                .tooltip(tip)
                .accessibility_label(tip)
                .on_click(cx.listener(move |this, _, window, cx| {
                    set(&mut this.flags);
                    this.run(window, cx);
                }))
        };
        let flags = self.flags;
        h_flex()
            .gap_0p5()
            .child(flag(
                "regex-flag-i",
                "i",
                "Ignore case",
                flags.ignore_case,
                |f| f.ignore_case = !f.ignore_case,
            ))
            .child(flag(
                "regex-flag-m",
                "m",
                "^ and $ at every line",
                flags.multi_line,
                |f| f.multi_line = !f.multi_line,
            ))
            .child(flag(
                "regex-flag-s",
                "s",
                ". takes line breaks too",
                flags.dot_all,
                |f| f.dot_all = !f.dot_all,
            ))
            .child(flag(
                "regex-flag-x",
                "x",
                "Spaces and # comments in the pattern are ignored",
                flags.verbose,
                |f| f.verbose = !f.verbose,
            ))
    }

    fn render_matches(&self, text: &str, cx: &App) -> AnyElement {
        let theme = cx.theme();
        match &self.outcome {
            None => parts::hint(
                "Type a pattern. Look-around and back-references work, as in JavaScript and PCRE.",
                cx,
            )
            .into_any_element(),
            Some(Err(err)) => parts::failure(err.clone(), cx).into_any_element(),
            Some(Ok(outcome)) => {
                let count = outcome.found.len();
                let said = match (count, outcome.more) {
                    (0, _) => "No matches".to_string(),
                    (count, true) => format!("The first {count} matches"),
                    (1, false) => "1 match".to_string(),
                    (count, false) => format!("{count} matches"),
                };
                let line_of = |at: usize| text[..at.min(text.len())].matches('\n').count() + 1;
                let rows = outcome
                    .found
                    .iter()
                    .take(200)
                    .enumerate()
                    .map(|(ix, found)| {
                        let groups = found.groups.iter().map(|(name, range)| {
                            h_flex()
                                .gap_2()
                                .pl_6()
                                .text_xs()
                                .child(
                                    div()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("{name}:")),
                                )
                                .child(div().font_family(theme.mono_font_family.clone()).child(
                                    match range {
                                        Some(range) => format!("“{}”", &text[range.clone()]),
                                        None => "(took no part)".to_string(),
                                    },
                                ))
                        });
                        v_flex()
                            .px_2()
                            .py_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .child(
                                        div()
                                            .w(ph_ui::rems_from_px(28.))
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(format!("{}", ix + 1)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_family(theme.mono_font_family.clone())
                                            .child(format!("“{}”", &text[found.range.clone()])),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            format!(
                                                "line {}, at {}",
                                                line_of(found.range.start),
                                                found.range.start
                                            ),
                                        ),
                                    ),
                            )
                            .children(groups)
                    });
                let all: String = outcome
                    .found
                    .iter()
                    .map(|found| text[found.range.clone()].to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .px_2()
                            .gap_2()
                            .child(parts::badge(
                                said,
                                if count == 0 { Tone::Plain } else { Tone::Good },
                                cx,
                            ))
                            .child(div().flex_1())
                            .when(count > 0, |this| {
                                this.child(
                                    ph_ui::CopyButton::new("regex-copy-matches", all)
                                        .label("Copy matches"),
                                )
                            }),
                    )
                    .children(rows)
                    .into_any_element()
            }
        }
    }
}

impl ToolView for Regex {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pattern = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Pattern, e.g. (?<year>\\d{4})-(\\d{2})")
        });
        let text = parts::editor("text", "The text to search", window, cx);
        let with = parts::line(
            "Replace with: $1, ${year}, $0 for the whole match",
            window,
            cx,
        );
        let output = parts::editor("text", "", window, cx);
        let inputs = vec![
            cx.subscribe_in(
                &pattern,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.run(window, cx);
                    }
                },
            ),
            cx.subscribe_in(&text, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.run(window, cx);
                }
            }),
            cx.subscribe_in(&with, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.run(window, cx);
                }
            }),
        ];
        Self {
            ctx,
            replace: false,
            flags: Flags::default(),
            pattern,
            text,
            with,
            output,
            outcome: None,
            marks: None,
            _inputs: inputs,
            _run: None,
        }
    }

    /// Text handed over is what is searched.
    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.text
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.run(window, cx);
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.pattern
            .update(cx, |pattern, cx| pattern.focus(window, cx));
    }
}

impl Render for Regex {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.text.read(cx).value().to_string();
        let replaced = self.output.read(cx).value().to_string();
        let matches = self.render_matches(&text, cx);
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                parts::modes(
                    "utility-modes",
                    &["Matches", "Replace"],
                    usize::from(self.replace),
                    cx.listener(|this, ix: &usize, window, cx| {
                        this.replace = *ix == 1;
                        this.run(window, cx);
                    }),
                ),
            ))
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(
                                Field::new(&self.pattern)
                                    .small()
                                    .prefix(
                                        div().text_color(cx.theme().muted_foreground).child("/"),
                                    )
                                    .suffix(
                                        div().text_color(cx.theme().muted_foreground).child("/"),
                                    ),
                            ),
                    )
                    .child(self.render_flags(cx)),
            )
            .when(self.replace, |this| {
                this.child(Field::new(&self.with).small().flex_none())
            })
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(parts::label("Text (matches are marked)", cx).flex_1())
                    .child(
                        parts::action("utility-paste", Icons::ClipboardPaste, "Paste").on_click(
                            cx.listener(|this, _, window, cx| {
                                if let Some(text) = parts::clipboard_text(cx) {
                                    this.take(text, window, cx);
                                }
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(&self.text).h_full().small()),
            )
            .map(|this| {
                if self.replace {
                    this.child(
                        h_flex()
                            .flex_none()
                            .child(parts::label("After replacing", cx).flex_1())
                            .child(
                                ph_ui::CopyButton::new("regex-copy-replaced", replaced.clone())
                                    .label("Copy")
                                    .disabled(replaced.is_empty()),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(Editor::new(&self.output).h_full().small().readonly(true)),
                    )
                } else {
                    this.child(
                        div()
                            .id("regex-matches")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(matches),
                    )
                }
            })
    }
}

pub(crate) struct Diff {
    ctx: Ctx,
    before: Entity<EditorState>,
    after: Entity<EditorState>,
    state: Entity<DiffState>,
    ignore_space: bool,
    /// Lines added and removed, or none while nothing is compared.
    counts: Option<(usize, usize)>,
    patch: String,
    _inputs: Vec<Subscription>,
    _run: Option<Task<()>>,
}

impl Diff {
    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.before.read(cx).value().to_string();
        let after = self.after.read(cx).value().to_string();
        let ignore_space = self.ignore_space;
        if before.is_empty() && after.is_empty() {
            self.counts = None;
            self.patch.clear();
            self.state
                .update(cx, |state, cx| state.set_files(Vec::new(), cx));
            cx.notify();
            return;
        }
        self._run = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let (patch, files) = cx
                .background_spawn(async move {
                    let patch = crate::text::patch(&before, &after, 3, ignore_space);
                    // The whole text, with the changes in it: the viewer
                    // folds long runs that did not change.
                    let shown = crate::text::patch(&before, &after, usize::MAX / 4, ignore_space);
                    let files = if shown.is_empty() {
                        Vec::new()
                    } else {
                        DiffFile::parse(&format!("diff --git a/text b/text\n{shown}"))
                            .unwrap_or_default()
                    };
                    (patch, files)
                })
                .await;
            _ = this.update(cx, |this, cx| {
                this.counts = Some(
                    files
                        .first()
                        .map_or((0, 0), |file| (file.additions(), file.deletions())),
                );
                this.patch = patch;
                this.state
                    .update(cx, |state, cx| state.set_files(files, cx));
                cx.notify();
            });
        }));
    }

    fn swap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.before.read(cx).value().to_string();
        let after = self.after.read(cx).value().to_string();
        self.before
            .update(cx, |input, cx| input.set_value(after, window, cx));
        self.after
            .update(cx, |input, cx| input.set_value(before, window, cx));
        self.run(window, cx);
    }
}

impl ToolView for Diff {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let before = parts::editor("text", "Before", window, cx);
        let after = parts::editor("text", "After", window, cx);
        let inputs = [&before, &after]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.run(window, cx);
                    }
                })
            })
            .collect();
        let state = cx.new(|cx| DiffState::new(Vec::new(), cx).with_mode(DiffMode::Split));
        Self {
            ctx,
            before,
            after,
            state,
            ignore_space: false,
            counts: None,
            patch: String::new(),
            _inputs: inputs,
            _run: None,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.before
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.run(window, cx);
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.before.update(cx, |input, cx| input.focus(window, cx));
    }
}

impl Render for Diff {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wide = super::wide(window);
        let split = self.state.read(cx).mode() == DiffMode::Split;
        let editors = [("Before", &self.before), ("After", &self.after)].map(|(name, state)| {
            v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .gap_1()
                .child(parts::label(name, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(Editor::new(state).h_full().small()),
                )
        });
        let [before, after] = editors;
        let result: AnyElement = match self.counts {
            None => parts::surface(cx)
                .size_full()
                .child(parts::hint(
                    "Paste two texts to see what changed between them.",
                    cx,
                ))
                .into_any_element(),
            Some((0, 0)) => parts::surface(cx)
                .size_full()
                .child(parts::hint(
                    if self.ignore_space {
                        "They are the same, spaces aside."
                    } else {
                        "They are the same."
                    },
                    cx,
                ))
                .into_any_element(),
            Some(_) => DiffView::new(&self.state)
                .header_visible(false)
                .soft_wrap(split)
                .into_any_element(),
        };
        let patch = self.patch.clone();
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                h_flex()
                    .gap_2()
                    .child(
                        Checkbox::new("diff-ignore-space")
                            .small()
                            .label("Ignore spaces")
                            .checked(self.ignore_space)
                            .on_click(cx.listener(|this, on: &bool, window, cx| {
                                this.ignore_space = *on;
                                this.run(window, cx);
                            })),
                    )
                    .child(
                        Button::new("diff-swap")
                            .ghost()
                            .xsmall()
                            .icon(Icons::ArrowLeftRight)
                            .label("Swap")
                            .on_click(cx.listener(|this, _, window, cx| this.swap(window, cx))),
                    )
                    .child(parts::modes(
                        "utility-modes",
                        &["Side by side", "One column"],
                        if split { 0 } else { 1 },
                        cx.listener(|this, ix: &usize, _, cx| {
                            let mode = if *ix == 0 {
                                DiffMode::Split
                            } else {
                                DiffMode::Unified
                            };
                            this.state.update(cx, |state, cx| state.set_mode(mode, cx));
                            cx.notify();
                        }),
                    )),
            ))
            .child(div().flex_none().h(gpui_kit::relative(0.34)).map(|this| {
                if wide {
                    this.child(
                        h_flex()
                            .size_full()
                            .items_start()
                            .gap_3()
                            .child(before.h_full())
                            .child(after.h_full()),
                    )
                } else {
                    this.child(v_flex().size_full().gap_2().child(before).child(after))
                }
            }))
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(parts::label("Changes", cx))
                    .when_some(
                        self.counts.filter(|counts| *counts != (0, 0)),
                        |this, (added, removed)| {
                            this.child(parts::badge(format!("+{added}"), Tone::Good, cx))
                                .child(parts::badge(format!("−{removed}"), Tone::Bad, cx))
                        },
                    )
                    .child(div().flex_1())
                    .child(
                        ph_ui::CopyButton::new("diff-copy-patch", patch.clone())
                            .label("Copy as patch")
                            .disabled(patch.is_empty()),
                    ),
            )
            .child(div().flex_1().min_h_0().child(result))
    }
}

pub(crate) struct Markdown {
    ctx: Ctx,
    preview: bool,
    input: Entity<EditorState>,
    html: Entity<EditorState>,
    _input: Subscription,
}

impl Markdown {
    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        let html = crate::text::markdown(&crate::Input::new(text))
            .unwrap_or_else(|err| format!("{err:#}"));
        self.html
            .update(cx, |output, cx| output.set_value(html, window, cx));
        cx.notify();
    }
}

impl ToolView for Markdown {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = parts::editor(
            "markdown",
            "Markdown: a README, a pull request's description, notes",
            window,
            cx,
        );
        let html = parts::editor("html", "", window, cx);
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.run(window, cx);
                }
            });
        Self {
            ctx,
            preview: true,
            input,
            html,
            _input: subscription,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        self.run(window, cx);
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }
}

impl Render for Markdown {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wide = super::wide(window);
        let text = self.input.read(cx).value().to_string();
        let html = self.html.read(cx).value().to_string();
        let input = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .child(parts::label("Markdown", cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(&self.input).h_full().small()),
            );
        let shown: AnyElement = if text.trim().is_empty() {
            parts::surface(cx)
                .size_full()
                .child(parts::hint("It shows here as it reads.", cx))
                .into_any_element()
        } else if self.preview {
            let guarded = ph_ui::markdown::guarded(&text).into_owned();
            div()
                .id("markdown-preview")
                .size_full()
                .overflow_y_scroll()
                .p_3()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .child(
                    TextView::markdown("markdown-preview-text", guarded)
                        .selectable(true)
                        // Pictures are not fetched: nothing here leaves the
                        // computer, and a local path could name any file.
                        .image_source(|_| {
                            ImageSource::Custom(Arc::new(|_, _| {
                                Some(Err(ImageCacheError::Other(Arc::new(anyhow::anyhow!(
                                    "pictures are not loaded here"
                                )))))
                            }))
                        })
                        .on_link_click(|url, _, _, cx| {
                            if ph_ui::platform::is_web_link(url) {
                                cx.open_url(url);
                            }
                        }),
                )
                .into_any_element()
        } else {
            Editor::new(&self.html)
                .h_full()
                .small()
                .readonly(true)
                .into_any_element()
        };
        let output = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .child(
                h_flex()
                    .child(
                        parts::label(if self.preview { "As it reads" } else { "HTML" }, cx)
                            .flex_1(),
                    )
                    .child(
                        ph_ui::CopyButton::new("markdown-copy-html", html.clone())
                            .label("Copy HTML")
                            .disabled(html.is_empty()),
                    ),
            )
            .child(div().flex_1().min_h_0().child(shown));
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                parts::modes(
                    "utility-modes",
                    &["Preview", "HTML"],
                    usize::from(!self.preview),
                    cx.listener(|this, ix: &usize, _, cx| {
                        this.preview = *ix == 0;
                        cx.notify();
                    }),
                ),
            ))
            .child(div().flex_1().min_h_0().map(|this| {
                if wide {
                    this.child(
                        h_flex()
                            .size_full()
                            .items_start()
                            .gap_3()
                            .child(input.h_full())
                            .child(output.h_full()),
                    )
                } else {
                    this.child(v_flex().size_full().gap_3().child(input).child(output))
                }
            }))
    }
}
