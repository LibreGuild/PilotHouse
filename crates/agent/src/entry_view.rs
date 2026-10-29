//! How each transcript entry looks.
//!
//! Pure presentation: every function reads an entry and returns an element.
//! Rows sit in a centered reading column. An agent's turn opens with a small
//! header (avatar and name), then its replies, tool calls and plans.
//!
//! Tool calls and thoughts read as one line of muted text that opens on
//! click, so a turn with many of them stays compact; the space above each
//! row depends on what came before it (see [`space_before`]).

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    h_flex,
    shimmer::ShimmerText,
    spinner::Spinner,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use ph_ui::Icons;

use std::rc::Rc;

use crate::attachment_view;
use crate::permission::PermissionRequest;
use crate::thread::ChatThread;
use crate::transcript::{
    self, AttachmentSummary, Entry, FileDiff, PlanItem, PlanStatus, ToolEntry, ToolKindLabel,
    ToolStatus,
};
use crate::view_mode::ViewMode;

/// How a step row shows in its run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InRun {
    /// Not in a run: shown as itself.
    Alone,
    /// The run's first row: the run's summary line, and itself when open.
    Head { open: bool },
    /// Another row of the run: shown only while the run is open.
    Member { open: bool },
}

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
    let mode = ViewMode::current(cx);
    let step = matches!(entry, Entry::Tool(_) | Entry::Thought { .. });
    let run = (mode == ViewMode::Normal)
        .then(|| transcript::run_of(entries, ix))
        .flatten();
    let in_run = match &run {
        None => InRun::Alone,
        Some(run) => {
            let open = transcript::run_tools(entries, run.clone())
                .first()
                .is_some_and(|tool| thread.is_run_open(&tool.id));
            if ix == run.start {
                InRun::Head { open }
            } else {
                InRun::Member { open }
            }
        }
    };
    // Summary shows no steps; a closed run shows only its summary line.
    let hidden = (mode == ViewMode::Summary && step) || in_run == InRun::Member { open: false };

    let body = match entry {
        _ if hidden => div().into_any_element(),
        Entry::User {
            text,
            attachments,
            checkpoint,
        } => {
            let user = render_user(ix, text, attachments, cx);
            match checkpoint {
                // The restore button sits beside the message, shown on hover,
                // so it takes no room of its own.
                Some(checkpoint) => h_flex()
                    .id(("user-message", ix))
                    .group("user-message")
                    .items_end()
                    .gap_1()
                    .child(restore_button(thread_entity, ix, checkpoint, text, cx))
                    .child(div().flex_1().min_w_0().child(user))
                    .into_any_element(),
                None => user,
            }
        }
        Entry::Restored { text, undo } => {
            render_restored(thread_entity, ix, text, undo.as_deref(), cx)
        }
        // Whitespace between tool calls takes no room.
        Entry::Agent { text, .. } if text.trim().is_empty() => div().into_any_element(),
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

    // In a run: the summary line on top of the first row, the open run's
    // rows indented under it.
    let body = match (in_run, &run) {
        (InRun::Head { open }, Some(run)) => v_flex()
            .child(run_line(thread_entity, ix, run.clone(), open, cx))
            .when(open, |this| this.child(run_member(body, cx)))
            .into_any_element(),
        (InRun::Member { open: true }, _) => run_member(body, cx),
        _ => body,
    };
    let last = ix + 1 == entries.len();
    let working = last.then(|| thread.activity()).flatten();
    let space = if hidden {
        px(0.)
    } else {
        space_before(ix.checked_sub(1).map(|prev| &entries[prev]), entry)
    };
    // The turn ends here: below its last row, what it changed.
    let ends_turn = !matches!(entry, Entry::User { .. })
        && match entries.get(ix + 1) {
            Some(next) => matches!(next, Entry::User { .. }),
            None => *thread.status() == crate::thread::ThreadStatus::Idle,
        };
    let footer = ends_turn
        .then(|| turn_footer(thread_entity, ix, cx))
        .flatten();
    let column = v_flex()
        .w_full()
        .max_w(
            ph_theme::preferences::preferences(cx)
                .chat_width
                .max_width(),
        )
        .px_6()
        .when(starts_turn, |this| {
            this.pt_4().child(turn_header(thread.agent(), cx))
        })
        .child(div().pt(space).child(body))
        .children(footer)
        .when_some(working, |this, (elapsed, step)| {
            let waiting = thread.pending_question(cx).is_some();
            this.child(working_row(
                thread.agent(),
                elapsed,
                step,
                (thread.is_background(), waiting),
                cx,
            ))
        })
        .when(last, |this| this.pb_4());
    h_flex()
        .w_full()
        .justify_center()
        .child(column)
        .into_any_element()
}

/// A run's summary line: what its steps did ("Edited 2 files, ran 3
/// commands"), how many failed; while one runs, what it is running.
fn run_line(
    thread: &Entity<ChatThread>,
    ix: usize,
    run: std::ops::Range<usize>,
    open: bool,
    cx: &App,
) -> AnyElement {
    let entries = thread.read(cx).entries();
    let tools = transcript::run_tools(entries, run);
    let running = tools.iter().rev().find(|tool| tool.is_running());
    let (summary, failed) = transcript::run_summary(&tools);
    let (icon, title): (AnyElement, SharedString) = match running {
        Some(tool) => (
            Spinner::new()
                .xsmall()
                .color(cx.theme().info)
                .into_any_element(),
            format!("Running {}", tool.display_title()).into(),
        ),
        None => (
            Icon::new(Icons::Layers)
                .xsmall()
                .text_color(cx.theme().info)
                .into_any_element(),
            summary.into(),
        ),
    };
    let note = (failed > 0).then(|| SharedString::from(format!("{failed} failed")));
    let thread = thread.clone();
    step_line(("run", ix), icon, title, note, open, true, cx)
        .when(failed > 0, |this| {
            this.text_color(cx.theme().muted_foreground)
        })
        .on_click(move |_, _, cx| thread.update(cx, |thread, cx| thread.toggle_run(ix, cx)))
        .into_any_element()
}

/// A row of an open run, indented under its summary line.
fn run_member(body: AnyElement, cx: &App) -> AnyElement {
    div()
        .ml(px(5.))
        .pl_3()
        .border_l_1()
        .border_color(cx.theme().border)
        .child(body)
        .into_any_element()
}

/// Under a turn's last row, when the turn changed files: how many and the
/// lines added and removed, with its diff and a way to undo it.
fn turn_footer(thread_entity: &Entity<ChatThread>, ix: usize, cx: &App) -> Option<AnyElement> {
    let thread = thread_entity.read(cx);
    let entries = thread.entries();
    let user = entries[..ix]
        .iter()
        .rposition(|entry| matches!(entry, Entry::User { .. }));
    let start = user.map_or(0, |user| user + 1);
    let (count, added, removed) = transcript::turn_change_counts(&entries[start..=ix]);
    if count == 0 {
        return None;
    }
    let undo = user.and_then(|user| match &entries[user] {
        Entry::User {
            text,
            checkpoint: Some(checkpoint),
            ..
        } => Some((checkpoint.clone(), text.clone())),
        _ => None,
    });
    let can_restore = thread.can_restore();
    let title = format!("Changed {count} file{}", if count == 1 { "" } else { "s" });
    let review = {
        let (thread, title) = (thread_entity.clone(), title.clone());
        move |_: &gpui_kit::ClickEvent, window: &mut gpui_kit::Window, cx: &mut App| {
            // The diffs are put together once, when asked for; the list
            // draws only the files in view, so thousands open at once.
            let files = transcript::turn_changes(&thread.read(cx).entries()[start..=ix]);
            let view = gpui_kit::AppContext::new(cx, |_| TurnChanges::new(files));
            let title = title.clone();
            window.open_dialog(cx, move |dialog, _, _| {
                dialog.title(title.clone()).w(px(860.)).child(view.clone())
            });
        }
    };
    Some(
        h_flex()
            .pt_2()
            .gap_2()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(title)
            .child(
                div()
                    .text_color(cx.theme().success)
                    .child(format!("+{added}")),
            )
            .child(
                div()
                    .text_color(cx.theme().danger)
                    .child(format!("−{removed}")),
            )
            .child(div().flex_1())
            .child(
                Button::new(("turn-review", ix))
                    .ghost()
                    .xsmall()
                    .label("Review changes")
                    .on_click(review),
            )
            .when_some(undo, |this, (checkpoint, message)| {
                let thread = thread_entity.clone();
                this.child(
                    Button::new(("turn-undo", ix))
                        .ghost()
                        .xsmall()
                        .label("Undo turn")
                        .disabled(!can_restore)
                        .tooltip("Put the files back as they were before this turn")
                        .on_click(move |_, window, cx| {
                            confirm_restore(
                                thread.clone(),
                                checkpoint.clone(),
                                message.clone(),
                                window,
                                cx,
                            )
                        }),
                )
            })
            .into_any_element(),
    )
}

/// A turn's changes, file by file, in a list that draws only what shows.
struct TurnChanges {
    files: Rc<Vec<FileDiff>>,
    list: gpui_kit::ListState,
}

impl TurnChanges {
    fn new(files: Vec<FileDiff>) -> Self {
        Self {
            list: gpui_kit::ListState::new(files.len(), gpui_kit::ListAlignment::Top, px(600.)),
            files: Rc::new(files),
        }
    }
}

impl gpui_kit::Render for TurnChanges {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::component::scroll::ScrollableElement as _;
        let files = self.files.clone();
        div()
            .id("turn-changes")
            .relative()
            .h(px(600.))
            .child(
                gpui_kit::list(self.list.clone(), move |ix, _, cx| {
                    let Some(file) = files.get(ix) else {
                        return div().into_any_element();
                    };
                    div()
                        .pb_2()
                        .child(
                            div()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().border)
                                .overflow_hidden()
                                .child(render_diff(file, cx)),
                        )
                        .into_any_element()
                })
                .size_full(),
            )
            .vertical_scrollbar(&self.list)
    }
}

/// Room above an entry, from what is above it: a new message stands apart,
/// steps of one run sit close together.
fn space_before(previous: Option<&Entry>, entry: &Entry) -> Pixels {
    let step = |entry: &Entry| matches!(entry, Entry::Tool(_) | Entry::Thought { .. });
    let blank =
        |entry: &Entry| matches!(entry, Entry::Agent { text, .. } if text.trim().is_empty());
    match (previous, entry) {
        (None, _) => px(0.),
        (_, entry) if blank(entry) => px(0.),
        (_, Entry::User { .. }) => px(20.),
        // The turn header already makes room.
        (Some(Entry::User { .. }), _) => px(4.),
        (Some(previous), entry) if step(previous) && step(entry) => px(1.),
        (Some(previous), entry) if blank(previous) && step(entry) => px(1.),
        (Some(previous), _) if step(previous) || step(entry) => px(8.),
        _ => px(10.),
    }
}

/// "42s", "3m 05s".
pub(crate) fn short_duration(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// Under the newest entry while the agent works: what it is doing, and
/// for how long.
fn working_row(
    agent: &'static crate::agents::AgentSpec,
    elapsed: std::time::Duration,
    step: Option<String>,
    (background, waiting): (bool, bool),
    cx: &App,
) -> impl IntoElement {
    let time = short_duration(elapsed);
    let mut text = if waiting {
        format!("{} is waiting for your answer · {time}", agent.name)
    } else if background {
        format!("{} is carrying on by itself · {time}", agent.name)
    } else {
        format!("{} is working · {time}", agent.name)
    };
    if let Some(step) = step.filter(|_| !waiting) {
        text.push_str(" · ");
        text.push_str(&step);
    }
    h_flex()
        .pt_3()
        .gap_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(Spinner::new().xsmall().color(cx.theme().info))
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(ShimmerText::new(text).id("agent-working")),
        )
}

/// Opens a link from agent output. Only web and mail links: agent text can
/// echo pages it read (`ph_ui::platform::is_web_link`).
pub(crate) fn open_link(url: &str, cx: &mut App) {
    if ph_ui::platform::is_web_link(url) {
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
        .child(
            step_line(
                ("thought", ix),
                Icon::new(Icons::Brain).xsmall().into_any_element(),
                "Thinking".into(),
                None,
                expanded,
                true,
                cx,
            )
            .on_click(move |_, _, cx| {
                thread.update(cx, |thread, cx| thread.toggle_expanded(ix, cx))
            }),
        )
        .when(expanded, |this| {
            this.child(
                step_details(cx)
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

/// One step of the agent as a line of text: its icon (a spinner while it
/// runs), what it did, a short note, and a chevron when it opens.
fn step_line(
    id: impl Into<gpui_kit::ElementId>,
    icon: AnyElement,
    title: SharedString,
    note: Option<SharedString>,
    expanded: bool,
    opens: bool,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let muted = cx.theme().muted_foreground;
    let foreground = cx.theme().foreground;
    h_flex()
        .id(id)
        .group("step-line")
        .max_w_full()
        .gap_1p5()
        .py_0p5()
        .text_sm()
        .text_color(muted)
        .when(opens, |this| {
            this.cursor_pointer()
                .hover(move |style| style.text_color(foreground))
        })
        .child(icon)
        .child(div().min_w_0().truncate().child(title))
        .when_some(note, |this, note| {
            this.child(div().flex_none().text_xs().child(note))
        })
        .when(opens, |this| {
            this.child(
                div()
                    .flex_none()
                    .when(!expanded, |this| {
                        this.invisible()
                            .group_hover("step-line", |style| style.visible())
                    })
                    .child(
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall(),
                    ),
            )
        })
}

/// What an opened step shows, indented under its line.
fn step_details(cx: &App) -> gpui_kit::Div {
    v_flex()
        .ml(px(5.))
        .mt_1()
        .mb_1()
        .pl_3()
        .gap_1()
        .border_l_1()
        .border_color(cx.theme().border)
}

/// A tool call's icon: a spinner while it runs, red when it failed; a
/// subagent's call shows a bot.
fn tool_status_icon(tool: &ToolEntry, cx: &App) -> AnyElement {
    let icon = || {
        if tool.children.is_empty() && tool.agent_text.is_empty() {
            tool_icon(tool.kind)
        } else {
            Icon::new(Icons::Bot)
        }
    };
    match tool.status {
        ToolStatus::Pending | ToolStatus::Running => Spinner::new()
            .xsmall()
            .color(cx.theme().info)
            .into_any_element(),
        ToolStatus::Failed => icon()
            .xsmall()
            .text_color(cx.theme().danger)
            .into_any_element(),
        // Done is the normal case: quiet.
        ToolStatus::Done => icon()
            .xsmall()
            .text_color(cx.theme().muted_foreground)
            .into_any_element(),
    }
}

fn render_tool(thread: &Entity<ChatThread>, ix: usize, tool: &ToolEntry, cx: &App) -> AnyElement {
    let subagent = tool.is_subagent();
    // A subagent works on after its call returns (Claude runs them in the
    // background): it spins while its steps do.
    let busy = subagent
        && *thread.read(cx).status() == crate::thread::ThreadStatus::Working
        && tool.is_busy();
    let icon = if busy {
        Spinner::new()
            .xsmall()
            .color(cx.theme().info)
            .into_any_element()
    } else {
        tool_status_icon(tool, cx)
    };
    let has_details = !tool.output.trim().is_empty() || !tool.diffs.is_empty() || subagent;
    let thread = thread.clone();
    let note: Option<SharedString> = if !tool.diffs.is_empty() {
        let added: usize = tool.diffs.iter().map(|diff| diff.added).sum();
        let removed: usize = tool.diffs.iter().map(|diff| diff.removed).sum();
        Some(format!("+{added} −{removed}").into())
    } else if subagent {
        let steps = tool.children.len();
        Some(match steps {
            1 => "1 step".into(),
            steps => format!("{steps} steps").into(),
        })
    } else if tool.status == ToolStatus::Failed {
        Some("failed".into())
    } else {
        None
    };
    let line = step_line(
        ("tool", ix),
        icon,
        tool.display_title().into(),
        note,
        tool.expanded,
        has_details,
        cx,
    )
    .when(has_details, |this| {
        this.on_click(move |_, _, cx| {
            thread.update(cx, |thread, cx| thread.toggle_expanded(ix, cx))
        })
    });
    v_flex()
        .child(line)
        .when(tool.expanded && has_details, |this| {
            this.child(
                step_details(cx)
                    .children(tool.children.iter().map(|child| {
                        h_flex()
                            .gap_1p5()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(tool_status_icon(child, cx))
                            .child(div().min_w_0().truncate().child(child.display_title()))
                    }))
                    .when(!tool.agent_text.trim().is_empty(), |this| {
                        let tail: String = {
                            let text = tool.agent_text.trim();
                            let start = text.char_indices().rev().nth(600).map_or(0, |(at, _)| at);
                            text[start..].to_string()
                        };
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(tail),
                        )
                    })
                    .when(!tool.diffs.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().background)
                                .overflow_hidden()
                                .children(tool.diffs.iter().map(|diff| render_diff(diff, cx))),
                        )
                    })
                    .when(!tool.output.trim().is_empty(), |this| {
                        this.child(
                            div()
                                .px_3()
                                .py_2()
                                .max_h_64()
                                .overflow_hidden()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().muted.opacity(0.4))
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
    if let Some(form) = request.form() {
        return render_question(&handle, form, request.outcome(), cx);
    }
    // Answered: one quiet line, like a step.
    if let Some(outcome) = request.outcome() {
        return h_flex()
            .gap_2()
            .min_w_0()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(Icon::new(Icons::ShieldCheck).xsmall())
            .child(div().min_w_0().truncate().child(request.title.clone()))
            .child(div().flex_none().text_xs().child(outcome.clone()))
            .into_any_element();
    }
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
        .when_some(request.plan.clone(), |this, plan| {
            this.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().muted.opacity(0.5))
                    .text_sm()
                    .child(plan),
            )
        })
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

/// A question the agent asks with a form: the choices to pick, a box for an
/// answer in the user's own words, and Answer / Skip. Once answered, a
/// line with the answers.
fn render_question(
    handle: &Entity<PermissionRequest>,
    form: &crate::form::Form,
    outcome: Option<&SharedString>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    if let Some(outcome) = outcome {
        return v_flex()
            .gap_0p5()
            .min_w_0()
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(Icons::MessageCircleQuestionMark).xsmall())
                    .child(div().min_w_0().truncate().child(form.headline())),
            )
            .child(div().pl_5().text_sm().child(outcome.clone()))
            .into_any_element();
    }
    v_flex()
        .w_full()
        .min_w_0()
        .gap_3()
        .px_3()
        .py_3()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.warning.opacity(0.7))
        .child(
            h_flex()
                .gap_2()
                .items_start()
                .text_sm()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .child(
                    div().pt_0p5().child(
                        Icon::new(Icons::MessageCircleQuestionMark)
                            .small()
                            .text_color(theme.warning),
                    ),
                )
                .child(div().flex_1().min_w_0().child(form.message.clone())),
        )
        .child(form_answers(handle, form, cx))
        .into_any_element()
}

/// The fields of a question and its buttons, shared by the chat card and
/// the inbox.
pub fn form_answers(
    handle: &Entity<PermissionRequest>,
    form: &crate::form::Form,
    cx: &App,
) -> AnyElement {
    use crate::form::FieldKind;
    let theme = cx.theme();
    let id = handle.entity_id().as_u64();
    let immediate = form.answers_on_pick();
    let several = form.fields.len() > 1;
    let fields = form.fields.iter().enumerate().map(|(field_ix, field)| {
        let many = matches!(field.kind, FieldKind::Many(_));
        let heading = field
            .prompt
            .clone()
            .filter(|_| several)
            .or_else(|| field.title.clone().filter(|_| several));
        let typed = form.text(field_ix).to_string();
        let choices = field
            .choices()
            .iter()
            .enumerate()
            .map(|(choice_ix, choice)| {
                let picked = form.is_picked(field_ix, choice_ix);
                let handle = handle.clone();
                let glyph = div()
                    .flex_none()
                    .mt(px(3.))
                    .size(px(12.))
                    .border_1()
                    .border_color(if picked {
                        theme.foreground
                    } else {
                        theme.muted_foreground.opacity(0.6)
                    })
                    .map(|this| {
                        if many {
                            this.rounded(px(3.))
                        } else {
                            this.rounded_full()
                        }
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(picked, |this| {
                        this.child(div().size(px(6.)).bg(theme.foreground).map(|dot| {
                            if many {
                                dot.rounded(px(1.))
                            } else {
                                dot.rounded_full()
                            }
                        }))
                    });
                h_flex()
                    .id((
                        "form-choice",
                        id * 4096 + field_ix as u64 * 64 + choice_ix as u64,
                    ))
                    .items_start()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .when(picked, |this| this.bg(theme.list_active))
                    .hover(|style| style.bg(theme.list_hover))
                    .child(glyph)
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(div().text_sm().child(choice.label.clone()))
                            .when_some(choice.detail.clone(), |this, detail| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(detail),
                                )
                            }),
                    )
                    .on_click(move |_, _, cx| {
                        handle.update(cx, |request, cx| {
                            request.edit_form(cx, |form| form.pick(field_ix, choice_ix));
                            if immediate {
                                request.submit(cx);
                            }
                        })
                    })
            });
        let text_label = if field.choices().is_empty() {
            "Type an answer…"
        } else {
            "Other…"
        };
        v_flex()
            .gap_1()
            .when_some(heading, |this, heading| {
                this.child(div().text_sm().child(heading))
            })
            .children(choices)
            .when(field.takes_text(), |this| {
                let handle = handle.clone();
                let current = typed.clone();
                this.child(
                    h_flex()
                        .gap_2()
                        .px_2()
                        .when(!typed.is_empty(), |this| {
                            this.child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .truncate()
                                    .child(format!("“{typed}”")),
                            )
                        })
                        .child(
                            Button::new(("form-text", id * 64 + field_ix as u64))
                                .ghost()
                                .xsmall()
                                .label(if typed.is_empty() { text_label } else { "Edit" })
                                .on_click(move |_, window, cx| {
                                    ask_text(
                                        handle.clone(),
                                        field_ix,
                                        current.clone(),
                                        immediate,
                                        window,
                                        cx,
                                    )
                                }),
                        ),
                )
            })
    });
    let ready = form.is_ready();
    let (submit, decline) = (handle.clone(), handle.clone());
    v_flex()
        .gap_3()
        .children(fields)
        .child(
            h_flex()
                .gap_2()
                .when(!immediate, |this| {
                    this.child(
                        Button::new(("form-submit", id))
                            .small()
                            .primary()
                            .label("Answer")
                            .disabled(!ready)
                            .on_click(move |_, _, cx| {
                                submit.update(cx, |request, cx| request.submit(cx))
                            }),
                    )
                })
                .child(
                    Button::new(("form-skip", id))
                        .small()
                        .ghost()
                        .label("Skip")
                        .tooltip("Let the agent go on without an answer")
                        .on_click(move |_, _, cx| {
                            decline.update(cx, |request, cx| request.decline(cx))
                        }),
                ),
        )
        .into_any_element()
}

/// Asks for an answer in the user's own words for field `field` of a
/// question; with a one-question form, OK answers it.
fn ask_text(
    handle: Entity<PermissionRequest>,
    field: usize,
    current: String,
    submit: bool,
    window: &mut gpui_kit::Window,
    cx: &mut App,
) {
    use gpui_kit::AppContext as _;
    use gpui_kit::component::input::{Input, InputState};
    let input = cx.new(|cx| {
        let mut input = InputState::new(window, cx).placeholder("Your answer");
        input.set_value(current, window, cx);
        input
    });
    let focus = input.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        let (input, handle) = (input.clone(), handle.clone());
        dialog
            .title("Your answer")
            .w(px(460.))
            .child(Input::new(&input))
            .footer(ph_ui::dialog_buttons("OK", true))
            .on_ok(move |_, _, cx| {
                let text = input.read(cx).value().to_string();
                handle.update(cx, |request, cx| {
                    request.edit_form(cx, |form| form.set_text(field, text));
                    if submit {
                        request.submit(cx);
                    }
                });
                true
            })
    });
    focus.update(cx, |input, cx| input.focus(window, cx));
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
                    confirm_restore(
                        thread.clone(),
                        checkpoint.clone(),
                        message.clone(),
                        window,
                        cx,
                    )
                }),
        )
}

/// Asks, then puts the project's files back to `checkpoint` (taken before
/// the message `message`).
fn confirm_restore(
    thread: Entity<ChatThread>,
    checkpoint: String,
    message: String,
    window: &mut gpui_kit::Window,
    cx: &mut App,
) {
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
