//! The inbox in the title bar: what needs you, across workspaces, and your
//! to-dos.
//!
//! - **Inbox**: agents' questions waiting for an answer, reminders that came
//!   due, and schedules that finished since you last cleared them.
//! - **To-dos**: a short list of your own, for this workspace or for all of
//!   them, each with an optional reminder. A due reminder is announced once
//!   (in the window and from the system) and stays in the inbox until the
//!   to-do is done or snoozed.
//!
//! The bell's badge counts the inbox. Its numbers are read again on the
//! schedules' tick ([`crate::schedules::Automation`], which also announces
//! reminders) and after every change here, not on every frame.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, WindowExt as _,
    badge::Badge,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    notification::Notification,
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, WeakEntity, Window, div, px,
};
use jiff::{ToSpan as _, Zoned};
use ph_store::projects::ProjectId;
use ph_store::schedules::Schedule;
use ph_store::todos::{Todo, TodoId};

use crate::AppState;
use crate::workspace::Workspace;

/// When the user last cleared the inbox's finished runs (milliseconds).
const SEEN_KEY: &str = "inbox.seen_at";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Inbox,
    Todos,
}

/// When a new to-do reminds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Remind {
    Never,
    InAnHour,
    Tonight,
    Tomorrow,
}

impl Remind {
    const ALL: [Remind; 4] = [
        Remind::Never,
        Remind::InAnHour,
        Remind::Tonight,
        Remind::Tomorrow,
    ];

    fn label(self) -> &'static str {
        match self {
            Remind::Never => "No reminder",
            Remind::InAnHour => "In 1 h",
            Remind::Tonight => "18:00",
            Remind::Tomorrow => "Tomorrow 09:00",
        }
    }

    /// The time, in milliseconds; "18:00" already past means tomorrow's.
    fn at(self) -> Option<i64> {
        let now = Zoned::now();
        let at = |days: i64, hour: i8| {
            now.checked_add(days.days())
                .ok()?
                .with()
                .hour(hour)
                .minute(0)
                .second(0)
                .subsec_nanosecond(0)
                .build()
                .ok()
        };
        let time = match self {
            Remind::Never => return None,
            Remind::InAnHour => now.checked_add(1.hour()).ok()?,
            Remind::Tonight => at(0, 18).filter(|time| *time > now).or_else(|| at(1, 18))?,
            Remind::Tomorrow => at(1, 9)?,
        };
        Some(time.timestamp().as_millisecond())
    }
}

pub struct InboxView {
    workspace: WeakEntity<Workspace>,
    /// The workspace in front: its to-dos are listed (with the shared ones).
    project_id: Option<ProjectId>,
    tab: Tab,
    /// Reminders that came due and are not done.
    due: Vec<Todo>,
    /// Schedules that finished since the inbox was last cleared.
    finished: Vec<Schedule>,
    todos: Vec<Todo>,
    new_todo: Entity<InputState>,
    remind: Remind,
    /// The new to-do is for every workspace, not just this one.
    shared: bool,
    _input: Subscription,
}

impl InboxView {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let new_todo = cx
            .new(|cx| InputState::new(window, cx).placeholder("Something to do… (Enter adds it)"));
        let input = cx.subscribe_in(
            &new_todo,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add(window, cx);
                }
            },
        );
        let mut view = Self {
            workspace,
            project_id: None,
            tab: Tab::Inbox,
            due: Vec::new(),
            finished: Vec::new(),
            todos: Vec::new(),
            new_todo,
            remind: Remind::Never,
            shared: false,
            _input: input,
        };
        view.refresh(cx);
        view
    }

    /// How many things need the user: questions, reminders, finished runs.
    pub fn count(&self, cx: &mut App) -> usize {
        ph_agent::approvals::pending_count(cx) + self.due.len() + self.finished.len()
    }

    pub fn set_project(&mut self, project_id: Option<ProjectId>, cx: &mut Context<Self>) {
        if self.project_id != project_id {
            self.project_id = project_id;
            self.refresh(cx);
        }
    }

    /// Reads the inbox again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let store = AppState::store(cx);
        let now = now_millis();
        self.due = store.due_todos(now).unwrap_or_default();
        let seen: i64 = store.setting(SEEN_KEY);
        self.finished = store.finished_since(seen).unwrap_or_default();
        self.todos = store.todos(self.project_id).unwrap_or_default();
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.new_todo.read(cx).value().to_string();
        let project = if self.shared { None } else { self.project_id };
        match AppState::store(cx).add_todo(project, &text, self.remind.at()) {
            Ok(_) => {
                self.new_todo
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.remind = Remind::Never;
            }
            Err(err) => window.push_notification(Notification::warning(format!("{err:#}")), cx),
        }
        self.refresh(cx);
    }

    fn set_done(&mut self, id: TodoId, done: bool, cx: &mut Context<Self>) {
        let _ = AppState::store(cx).set_todo_done(id, done);
        self.refresh(cx);
    }

    fn snooze(&mut self, id: TodoId, cx: &mut Context<Self>) {
        let _ = AppState::store(cx).set_todo_reminder(id, Remind::InAnHour.at());
        self.refresh(cx);
    }

    fn delete(&mut self, id: TodoId, cx: &mut Context<Self>) {
        let _ = AppState::store(cx).delete_todo(id);
        self.refresh(cx);
    }

    fn clear_finished(&mut self, cx: &mut Context<Self>) {
        let _ = AppState::store(cx).set_setting(SEEN_KEY, &now_millis());
        self.refresh(cx);
    }

    fn open_chat(&mut self, schedule: &Schedule, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = schedule.last_session else {
            return;
        };
        let project_id = schedule.project_id;
        _ = self.workspace.update(cx, |workspace, cx| {
            workspace.open_project(project_id, window, cx);
            if let Some(space) = workspace.active_space() {
                space.update(cx, |space, cx| space.select_session(session, cx));
            }
        });
    }

    fn workspace_name(&self, project_id: Option<ProjectId>, cx: &App) -> Option<String> {
        let id = project_id?;
        let workspace = self.workspace.upgrade()?;
        workspace
            .read(cx)
            .projects()
            .iter()
            .find(|project| project.id == id)
            .map(|project| project.name.clone())
    }

    fn section(title: &'static str, cx: &App) -> impl IntoElement {
        div()
            .pt_1()
            .text_xs()
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .text_color(cx.theme().muted_foreground)
            .child(title)
    }

    fn render_inbox(&self, cx: &mut Context<Self>) -> AnyElement {
        let approvals = ph_agent::approvals::rows(cx);
        let muted = cx.theme().muted_foreground;
        if approvals.is_empty() && self.due.is_empty() && self.finished.is_empty() {
            return div()
                .py_4()
                .text_sm()
                .text_color(muted)
                .child("Nothing needs you. Agents' questions, reminders and finished schedules appear here.")
                .into_any_element();
        }
        let due =
            self.due.iter().enumerate().map(|(ix, todo)| {
                let (done, snooze) = (todo.id, todo.id);
                v_flex()
                    .gap_1()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .when_some(self.workspace_name(todo.project_id, cx), |this, name| {
                        this.child(div().text_xs().text_color(muted).child(name))
                    })
                    .child(div().text_sm().child(todo.text.clone()))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(("due-done", ix))
                                    .xsmall()
                                    .primary()
                                    .label("Done")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_done(done, true, cx)
                                    })),
                            )
                            .child(
                                Button::new(("due-snooze", ix))
                                    .xsmall()
                                    .ghost()
                                    .label("Remind me in 1 h")
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.snooze(snooze, cx)),
                                    ),
                            ),
                    )
            });
        let finished = self.finished.iter().enumerate().map(|(ix, schedule)| {
            let chat = schedule.clone();
            v_flex()
                .gap_1()
                .pt_2()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(muted)
                        .child(
                            div().flex_1().min_w_0().truncate().child(
                                self.workspace_name(Some(schedule.project_id), cx)
                                    .unwrap_or_default(),
                            ),
                        )
                        .when_some(schedule.last_run, |this, at| {
                            this.child(ph_ui::time::ago(at / 1000))
                        }),
                )
                .child(div().text_sm().child(format!(
                    "{}: {}",
                    schedule.name,
                    schedule.last_result.clone().unwrap_or_default()
                )))
                .when(schedule.last_session.is_some(), |this| {
                    this.child(
                        Button::new(("finished-chat", ix))
                            .xsmall()
                            .ghost()
                            .label("Open chat")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_chat(&chat, window, cx)
                            })),
                    )
                })
        });
        v_flex()
            .gap_1()
            .when(!approvals.is_empty(), |this| {
                this.child(Self::section("Waiting for you", cx))
                    .children(approvals)
            })
            .when(!self.due.is_empty(), |this| {
                this.child(Self::section("Reminders", cx)).children(due)
            })
            .when(!self.finished.is_empty(), |this| {
                this.child(
                    h_flex()
                        .child(div().flex_1().child(Self::section("Finished", cx)))
                        .child(
                            Button::new("finished-clear")
                                .xsmall()
                                .ghost()
                                .label("Clear")
                                .on_click(cx.listener(|this, _, _, cx| this.clear_finished(cx))),
                        ),
                )
                .children(finished)
            })
            .into_any_element()
    }

    fn render_todos(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let rows = self.todos.iter().enumerate().map(|(ix, todo)| {
            let (toggle, remove, done) = (todo.id, todo.id, todo.done);
            let when = todo
                .remind_at
                .filter(|_| !todo.done)
                .map(|at| reminder_label(at, now_millis()));
            h_flex()
                .gap_2()
                .py_0p5()
                .child(
                    Checkbox::new(("todo-done", ix))
                        .checked(todo.done)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.set_done(toggle, !done, cx)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .when(todo.done, |this| this.line_through().text_color(muted))
                        .child(todo.text.clone()),
                )
                .when(todo.project_id.is_none(), |this| {
                    this.child(div().text_xs().text_color(muted).child("all"))
                })
                .when_some(when, |this, when| {
                    this.child(div().text_xs().text_color(muted).child(when))
                })
                .child(
                    Button::new(("todo-delete", ix))
                        .xsmall()
                        .ghost()
                        .icon(IconName::Close)
                        .accessibility_label("Delete to-do")
                        .on_click(cx.listener(move |this, _, _, cx| this.delete(remove, cx))),
                )
        });
        let choice = |id: SharedString, text: &'static str, on: bool| {
            Button::new(id)
                .xsmall()
                .when(on, |this| this.primary())
                .when(!on, |this| this.ghost())
                .label(text)
        };
        v_flex()
            .gap_2()
            .child(Input::new(&self.new_todo).small())
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(Remind::ALL.map(|remind| {
                        choice(
                            SharedString::from(format!("remind-{}", remind.label())),
                            remind.label(),
                            self.remind == remind,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remind = remind;
                            cx.notify();
                        }))
                    }))
                    .child(div().flex_1())
                    .child(
                        choice("todo-shared".into(), "All workspaces", self.shared).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.shared = !this.shared;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .when(self.todos.is_empty(), |this| {
                this.child(div().text_xs().text_color(muted).child(
                    "Nothing to do yet. Add what you must not forget; give it a reminder to be told when.",
                ))
            })
            .child(
                v_flex()
                    .id("todos-list")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}

impl Render for InboxView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.count(cx);
        let open = self.todos.iter().filter(|todo| !todo.done).count();
        let tab = |id: &'static str, label: String, which: Tab, cx: &mut Context<Self>| {
            Button::new(id)
                .xsmall()
                .when(self.tab == which, |this| this.primary())
                .when(self.tab != which, |this| this.ghost())
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = which;
                    cx.notify();
                }))
        };
        v_flex()
            .w(px(400.))
            .gap_2()
            .p_1()
            .child(
                h_flex()
                    .gap_1()
                    .child(tab(
                        "inbox-tab",
                        if count > 0 {
                            format!("Inbox {count}")
                        } else {
                            "Inbox".into()
                        },
                        Tab::Inbox,
                        cx,
                    ))
                    .child(tab(
                        "todos-tab",
                        if open > 0 {
                            format!("To-dos {open}")
                        } else {
                            "To-dos".into()
                        },
                        Tab::Todos,
                        cx,
                    )),
            )
            .child(match self.tab {
                Tab::Inbox => self.render_inbox(cx),
                Tab::Todos => self.render_todos(cx),
            })
    }
}

/// The bell in the title bar, with the inbox's count.
pub(crate) fn button(view: &Entity<InboxView>, cx: &mut App) -> impl IntoElement {
    let count = view.update(cx, |view, cx| view.count(cx));
    let trigger = Button::new("inbox")
        .ghost()
        .small()
        .icon(IconName::Bell)
        .tooltip("Inbox and to-dos")
        .accessibility_label("Inbox");
    let view = view.clone();
    let popover = Popover::new("inbox-popover")
        .offset(ph_ui::POPUP_GAP)
        .anchor(gpui_kit::Anchor::TopRight)
        .trigger(trigger)
        .content(move |_, _, _| view.clone());
    // A zero count hides the badge.
    Badge::new().count(count).child(popover)
}

/// "in 40 min", "today 18:00", "Fri 09:00"; "due" once passed.
fn reminder_label(at: i64, now: i64) -> String {
    if at <= now {
        return "due".into();
    }
    let minutes = (at - now) / 60_000;
    if minutes < 60 {
        return format!("in {} min", minutes.max(1));
    }
    let Ok(time) = jiff::Timestamp::from_millisecond(at) else {
        return String::new();
    };
    let time = time.to_zoned(jiff::tz::TimeZone::system());
    if time.date() == Zoned::now().date() {
        time.strftime("today %H:%M").to_string()
    } else {
        time.strftime("%a %H:%M").to_string()
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders_read_as_times() {
        assert_eq!(reminder_label(1_000, 2_000), "due");
        assert_eq!(reminder_label(10 * 60_000, 0), "in 10 min");
        assert!(Remind::InAnHour.at().unwrap() > now_millis());
        assert!(
            Remind::Tonight.at().unwrap() > now_millis(),
            "18:00 already past means tomorrow's"
        );
        assert!(Remind::Never.at().is_none());
    }
}
