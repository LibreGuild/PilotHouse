//! How each transcript entry looks.
//!
//! Pure presentation: every function reads an entry and returns an element.
//! Rows sit in a centered reading column. An agent's turn opens with a small
//! header (avatar and name), then its replies, tool calls and plans.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    h_flex,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div,
};
use ph_ui::Icons;

use std::rc::Rc;

use crate::attachment_view;
use crate::permission::PermissionRequest;
use crate::thread::ChatThread;
use crate::transcript::{
    AttachmentSummary, Entry, FileDiff, PlanItem, PlanStatus, ToolEntry, ToolKindLabel, ToolStatus,
};

/// Renders entry `ix` of `thread` inside the reading column.
pub fn render_entry(thread_entity: &Entity<ChatThread>, ix: usize, cx: &App) -> AnyElement {
    let thread = thread_entity.read(cx);
    let entries = thread.entries();
    let Some(entry) = entries.get(ix) else {
        return div().into_any_element();
    };
    // The agent's turn starts after a user message (or at the very top).
    let starts_turn = !matches!(entry, Entry::User { .. })
        && (ix == 0 || matches!(entries[ix - 1], Entry::User { .. }));

    let body = match entry {
        Entry::User {
            text,
            attachments,
            checkpoint,
        } => {
            let user = render_user(ix, text, attachments, cx);
            match checkpoint {
                Some(checkpoint) => v_flex()
                    .id(("user-message", ix))
                    .group("user-message")
                    .items_end()
                    .gap_1()
                    .child(user)
                    .child(restore_button(thread_entity, ix, checkpoint, text, cx))
                    .into_any_element(),
                None => user,
            }
        }
        Entry::Restored { text, undo } => {
            render_restored(thread_entity, ix, text, undo.as_deref(), cx)
        }
        Entry::Agent { text, view } => v_flex()
            .id(("agent-message", ix))
            .group("agent-message")
            .gap_1()
            .map(|this| match view {
                Some(view) => this.child(view.clone()),
                // Not prepared yet (the list prepares a row before drawing it).
                None => this.child(text.clone()),
            })
            .child(
                h_flex()
                    .invisible()
                    .group_hover("agent-message", |style| style.visible())
                    .child(
                        Clipboard::new(("copy-message", ix))
                            .value(text.clone())
                            .tooltip("Copy"),
                    ),
            )
            .into_any_element(),
        Entry::Thought { text, expanded } => render_thought(thread_entity, ix, text, *expanded, cx),
        Entry::Tool(tool) => render_tool(thread_entity, ix, tool, cx),
        Entry::Plan(items) => render_plan(items, cx),
        Entry::Permission(request) => render_permission(request, cx),
        Entry::Notice { text, is_error } => render_notice(text, *is_error, cx),
        Entry::SignIn { message, methods } => {
            render_sign_in(thread_entity, ix, message, methods, cx)
        }
        Entry::Review { card, .. } => div().pt_2().child(card.clone()).into_any_element(),
    };

    let column = v_flex()
        .w_full()
        .max_w(
            ph_theme::preferences::preferences(cx)
                .chat_width
                .max_width(),
        )
        .px_6()
        .when(starts_turn, |this| {
            this.pt_3().child(turn_header(thread.agent(), cx))
        })
        .child(div().py_1().child(body));
    h_flex()
        .w_full()
        .justify_center()
        .child(column)
        .into_any_element()
}

/// Opens a link from agent output. Only web and mail links: agent text can
/// echo pages it read, and a `file://` or custom-scheme link could launch a
/// local program with one click.
pub(crate) fn open_link(url: &str, cx: &mut App) {
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    if matches!(scheme.as_deref(), Some("http" | "https" | "mailto")) {
        cx.open_url(url);
    } else {
        tracing::info!("not opening a non-web link from agent output: {url}");
    }
}

/// The agent's mark and name at the start of its reply, so it is clear who
/// wrote it.
fn turn_header(agent: &'static crate::agents::AgentSpec, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .pb_1()
        .child(agent.mark(gpui_kit::px(18.)))
        .child(
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(agent.name),
        )
}

fn render_user(
    entry_ix: usize,
    text: &str,
    attachments: &[AttachmentSummary],
    cx: &App,
) -> AnyElement {
    let images: Vec<&AttachmentSummary> =
        attachments.iter().filter(|a| a.image.is_some()).collect();
    let others: Vec<&AttachmentSummary> =
        attachments.iter().filter(|a| a.image.is_none()).collect();
    v_flex()
        .pt_3()
        .items_end()
        .gap_1p5()
        .when(!images.is_empty(), |this| {
            this.child(h_flex().gap_1p5().flex_wrap().justify_end().children(
                images.iter().enumerate().map(|(ix, attachment)| {
                    let path = attachment.image.clone().unwrap_or_default();
                    let id = SharedString::from(format!("sent-image-{entry_ix}-{ix}"));
                    attachment_view::thumbnail(id, attachment.label.clone(), path, cx)
                }),
            ))
        })
        .when(!others.is_empty(), |this| {
            this.child(h_flex().gap_1().flex_wrap().justify_end().children(
                others.iter().enumerate().map(|(ix, attachment)| {
                    let summary = (*attachment).clone();
                    let icon = match (&summary.file, &summary.text) {
                        (Some(file), _) => ph_ui::file_icon(&file.to_string_lossy()).0,
                        (_, Some(_)) => Icons::ScrollText,
                        _ => Icons::File,
                    };
                    let id = SharedString::from(format!("sent-attachment-{entry_ix}-{ix}"));
                    let preview: attachment_view::PreviewSource =
                        Rc::new(move |_| summary.preview());
                    attachment_view::chip(
                        id,
                        attachment.label.clone(),
                        icon,
                        None,
                        preview,
                        None,
                        cx,
                    )
                }),
            ))
        })
        .when(!text.is_empty(), |this| {
            this.child(
                div()
                    .max_w(gpui_kit::relative(0.85))
                    .px_3p5()
                    .py_2()
                    .rounded(cx.theme().radius_lg)
                    .bg(cx.theme().muted)
                    .text_sm()
                    .child(text.to_string()),
            )
        })
        .into_any_element()
}

fn render_sign_in(
    thread: &Entity<ChatThread>,
    ix: usize,
    message: &str,
    methods: &[crate::connection::SignInMethod],
    cx: &App,
) -> AnyElement {
    let agent = thread.read(cx).agent().name;
    let buttons = methods
        .iter()
        .cloned()
        .enumerate()
        .map(|(method_ix, method)| {
            let thread = thread.clone();
            let button = Button::new(("sign-in", ix * 8 + method_ix))
                .small()
                .label(method.name.clone())
                .on_click(move |_, _, cx| {
                    let method = method.clone();
                    thread.update(cx, |thread, cx| thread.sign_in(method, cx));
                });
            if method_ix == 0 {
                button.primary()
            } else {
                button.outline()
            }
        });
    v_flex()
        .gap_2()
        .p_3()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().warning.opacity(0.6))
        .bg(cx.theme().warning.opacity(0.06))
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(
                    Icon::new(Icons::KeyRound)
                        .small()
                        .text_color(cx.theme().warning),
                )
                .child(format!("Sign in to {agent}")),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(message.to_string()),
        )
        .child(h_flex().gap_2().flex_wrap().children(buttons))
        .into_any_element()
}

fn render_thought(
    thread: &Entity<ChatThread>,
    ix: usize,
    text: &str,
    expanded: bool,
    cx: &App,
) -> AnyElement {
    let thread = thread.clone();
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .id(("thought", ix))
                .gap_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    thread.update(cx, |thread, cx| thread.toggle_expanded(ix, cx))
                })
                .child(Icon::new(Icons::Brain).xsmall())
                .child("Thinking")
                .child(
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .xsmall(),
                ),
        )
        .when(expanded, |this| {
            this.child(
                div()
                    .ml_1p5()
                    .pl_3()
                    .border_l_2()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .italic()
                    .text_color(cx.theme().muted_foreground)
                    .child(text.to_string()),
            )
        })
        .into_any_element()
}

fn tool_icon(kind: ToolKindLabel) -> Icon {
    match kind {
        ToolKindLabel::Read => Icon::new(IconName::File),
        ToolKindLabel::Edit => Icon::new(Icons::SquarePen),
        ToolKindLabel::Delete => Icon::new(IconName::Delete),
        ToolKindLabel::Move => Icon::new(IconName::ArrowRight),
        ToolKindLabel::Search => Icon::new(IconName::Search),
        ToolKindLabel::Execute => Icon::new(Icons::Terminal),
        ToolKindLabel::Think => Icon::new(Icons::Brain),
        ToolKindLabel::Fetch => Icon::new(IconName::Globe),
        ToolKindLabel::Other => Icon::new(Icons::Zap),
    }
}

fn render_tool(thread: &Entity<ChatThread>, ix: usize, tool: &ToolEntry, cx: &App) -> AnyElement {
    let has_details = !tool.output.trim().is_empty() || !tool.diffs.is_empty();
    let thread = thread.clone();
    let status: AnyElement = match tool.status {
        ToolStatus::Pending | ToolStatus::Running => Spinner::new().xsmall().into_any_element(),
        ToolStatus::Done => Icon::new(IconName::Check)
            .xsmall()
            .text_color(cx.theme().success)
            .into_any_element(),
        ToolStatus::Failed => Icon::new(IconName::CircleX)
            .xsmall()
            .text_color(cx.theme().danger)
            .into_any_element(),
    };
    let summary: Option<SharedString> = (!tool.diffs.is_empty()).then(|| {
        let added: usize = tool.diffs.iter().map(|diff| diff.added).sum();
        let removed: usize = tool.diffs.iter().map(|diff| diff.removed).sum();
        format!("+{added} −{removed}").into()
    });

    v_flex()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted.opacity(0.35))
        .overflow_hidden()
        .child(
            h_flex()
                .id(("tool", ix))
                .px_2p5()
                .py_1p5()
                .gap_2()
                .text_xs()
                .when(has_details, |this| {
                    this.cursor_pointer()
                        .hover(|style| style.bg(cx.theme().list_hover))
                        .on_click(move |_, _, cx| {
                            thread.update(cx, |thread, cx| thread.toggle_expanded(ix, cx))
                        })
                })
                .child(
                    tool_icon(tool.kind)
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(tool.display_title()),
                )
                .when_some(summary, |this, summary| {
                    this.child(div().text_color(cx.theme().muted_foreground).child(summary))
                })
                .when(has_details, |this| {
                    this.child(
                        Icon::new(if tool.expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                    )
                })
                .child(status),
        )
        .when(tool.expanded && has_details, |this| {
            this.child(
                v_flex()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .children(tool.diffs.iter().map(|diff| render_diff(diff, cx)))
                    .when(!tool.output.trim().is_empty(), |this| {
                        this.child(
                            div()
                                .px_3()
                                .py_2()
                                .max_h_64()
                                .overflow_hidden()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(tool.output.clone()),
                        )
                    }),
            )
        })
        .into_any_element()
}

fn render_diff(diff: &FileDiff, cx: &App) -> AnyElement {
    let added_bg = cx.theme().success.opacity(0.12);
    let removed_bg = cx.theme().danger.opacity(0.12);
    let mono = cx.theme().mono_font_family.clone();
    v_flex()
        .py_1()
        .text_xs()
        .child(
            div()
                .px_3()
                .py_1()
                .text_color(cx.theme().muted_foreground)
                .child(format!("{}  +{} −{}", diff.path, diff.added, diff.removed)),
        )
        .children(diff.lines.iter().map(|line| {
            let row = div()
                .px_3()
                .font_family(mono.clone())
                .whitespace_nowrap()
                .overflow_hidden()
                .child(line.clone());
            match line.chars().next() {
                Some('+') => row.bg(added_bg),
                Some('-') => row.bg(removed_bg),
                _ => row.text_color(cx.theme().muted_foreground),
            }
        }))
        .when(diff.hidden_lines > 0, |this| {
            this.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("… {} more lines", diff.hidden_lines)),
            )
        })
        .into_any_element()
}

fn render_plan(items: &[PlanItem], cx: &App) -> AnyElement {
    let done = items
        .iter()
        .filter(|item| item.status == PlanStatus::Completed)
        .count();
    v_flex()
        .gap_1p5()
        .px_3()
        .py_2p5()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .child(
            h_flex()
                .gap_2()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(Icon::new(Icons::ListTodo).xsmall())
                .child("Plan")
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{done}/{}", items.len())),
                ),
        )
        .children(items.iter().map(|item| {
            let (icon, color, done) = match item.status {
                PlanStatus::Completed => {
                    (Icon::new(IconName::CircleCheck), cx.theme().success, true)
                }
                PlanStatus::InProgress => {
                    (Icon::new(IconName::LoaderCircle), cx.theme().info, false)
                }
                PlanStatus::Pending => {
                    (Icon::new(Icons::Circle), cx.theme().muted_foreground, false)
                }
            };
            h_flex()
                .gap_2()
                .items_start()
                .text_sm()
                .when(done, |this| {
                    this.text_color(cx.theme().muted_foreground).line_through()
                })
                .child(div().pt_0p5().child(icon.xsmall().text_color(color)))
                .child(div().flex_1().child(item.content.clone()))
        }))
        .into_any_element()
}

fn render_permission(request: &Entity<PermissionRequest>, cx: &App) -> AnyElement {
    let handle = request.clone();
    let request = request.read(cx);
    let pending = request.is_pending();
    v_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .px_3()
        .py_2p5()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(if pending {
            cx.theme().warning.opacity(0.7)
        } else {
            cx.theme().border
        })
        .when(pending, |this| this.bg(cx.theme().warning.opacity(0.05)))
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    Icon::new(if pending {
                        Icons::TriangleAlert
                    } else {
                        Icons::ShieldCheck
                    })
                    .small()
                    .text_color(if pending {
                        cx.theme().warning
                    } else {
                        cx.theme().muted_foreground
                    }),
                )
                // A long command (a path with no spaces) wraps inside the
                // card instead of running out of it.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(request.title.clone()),
                ),
        )
        .map(|this| match request.outcome() {
            Some(outcome) => this.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(outcome.clone()),
            ),
            None => this.child(permission_buttons(&handle, request)),
        })
        .into_any_element()
}

/// The answer buttons, shared by the inline card and the approval inbox.
pub fn permission_buttons(
    handle: &Entity<PermissionRequest>,
    request: &PermissionRequest,
) -> AnyElement {
    h_flex()
        .gap_2()
        .flex_wrap()
        .children(request.options.iter().enumerate().map(|(ix, choice)| {
            let handle = handle.clone();
            let id = choice.id.clone();
            let button = Button::new((
                "permission-choice",
                handle.entity_id().as_u64() * 16 + ix as u64,
            ))
            .small()
            .label(choice.label.clone())
            .on_click(move |_, _, cx| handle.update(cx, |request, cx| request.choose(&id, cx)));
            if choice.allows && ix == 0 {
                button.primary()
            } else {
                button.outline()
            }
        }))
        .into_any_element()
}

/// Under a user message, on hover: put the files back to before it.
fn restore_button(
    thread: &Entity<ChatThread>,
    ix: usize,
    checkpoint: &str,
    message: &str,
    cx: &App,
) -> impl IntoElement {
    let (thread, checkpoint, message) =
        (thread.clone(), checkpoint.to_string(), message.to_string());
    let enabled = thread.read(cx).can_restore();
    h_flex()
        .invisible()
        .group_hover("user-message", |style| style.visible())
        .child(
            Button::new(("restore-checkpoint", ix))
                .ghost()
                .xsmall()
                .icon(Icons::Undo2)
                .label("Restore files")
                .disabled(!enabled)
                .tooltip("Put the project's files back as they were before this message")
                .on_click(move |_, window, cx| {
                    let (thread, checkpoint, message) = (thread.clone(), checkpoint.clone(), message.clone());
                    window.open_alert_dialog(cx, move |dialog, _, _| {
                        let (thread, checkpoint, message) =
                            (thread.clone(), checkpoint.clone(), message.clone());
                        dialog
                            .confirm()
                            .title("Restore files?")
                            .description(
                                "Files changed since this message go back to how they were, and files created since are removed. Git history, staged changes and branches are not touched. You can undo it.",
                            )
                            .button_props(
                                gpui_kit::component::dialog::DialogButtonProps::default()
                                    .ok_text("Restore files"),
                            )
                            .on_ok(move |_, _, cx| {
                                thread.update(cx, |thread, cx| {
                                    thread.restore_checkpoint(checkpoint.clone(), Some(message.clone()), cx)
                                });
                                true
                            })
                    });
                }),
        )
}

/// A restore, with its undo.
fn render_restored(
    thread: &Entity<ChatThread>,
    ix: usize,
    text: &str,
    undo: Option<&str>,
    cx: &App,
) -> AnyElement {
    let enabled = thread.read(cx).can_restore();
    h_flex()
        .gap_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(Icon::new(Icons::Undo2).xsmall())
        .child(div().flex_1().min_w_0().child(text.to_string()))
        .when_some(undo.map(str::to_string), |this, undo| {
            let thread = thread.clone();
            this.child(
                Button::new(("undo-restore", ix))
                    .ghost()
                    .xsmall()
                    .label("Undo")
                    .disabled(!enabled)
                    .tooltip("Put the files back as they were before the restore")
                    .on_click(move |_, _, cx| {
                        thread.update(cx, |thread, cx| {
                            thread.restore_checkpoint(undo.clone(), None, cx)
                        });
                    }),
            )
        })
        .into_any_element()
}

fn render_notice(text: &str, is_error: bool, cx: &App) -> AnyElement {
    h_flex()
        .gap_2()
        .items_start()
        .text_xs()
        .text_color(if is_error {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        })
        .when(is_error, |this| {
            this.child(Icon::new(IconName::CircleAlert).xsmall())
        })
        .child(div().flex_1().min_w_0().child(text.to_string()))
        .into_any_element()
}
