//! The inbox in the title bar: what needs you, across workspaces, and your
//! to-dos.
//!
//! - **Inbox**: agents' questions waiting for an answer, reminders that came
//!   due, and schedules that finished since you last cleared them.
//! - **To-dos**: a short list of your own for the whole app, each with an
//!   optional reminder (to-dos agents add for a workspace show its name). A due reminder is announced once
//!   (in the window and from the system) and stays in the inbox until the
//!   to-do is done or snoozed.
//!
//! The bell's badge counts the inbox. Its numbers are read again on the
//! schedules' tick ([`crate::schedules::Automation`], which also announces
//! reminders) and after every change here, not on every frame.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
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
use ph_ui::{EmptyState, Icons, MenuButton as _};

/// When the user last cleared the inbox's finished runs (milliseconds).
const SEEN_KEY: &str = "inbox.seen_at";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Inbox,
    Todos,
}

/// When a new to-do reminds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Remind {
    Never,
    InTenMinutes,
    InAnHour,
    Tonight,
    Tomorrow,
}

impl Remind {
    const ALL: [Remind; 5] = [
        Remind::Never,
        Remind::InTenMinutes,
        Remind::InAnHour,
        Remind::Tonight,
        Remind::Tomorrow,
    ];

    fn label(self) -> &'static str {
        match self {
            Remind::Never => "No reminder",
            Remind::InTenMinutes => "In 10 min",
            Remind::InAnHour => "In 1 hour",
            Remind::Tonight => "Today 18:00",
            Remind::Tomorrow => "Tomorrow 09:00",
        }
    }

    /// The time, in milliseconds; "18:00" already past means tomorrow's.
    pub(crate) fn at(self) -> Option<i64> {
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
            Remind::InTenMinutes => now.checked_add(10.minutes()).ok()?,
            Remind::InAnHour => now.checked_add(1.hour()).ok()?,
            Remind::Tonight => at(0, 18).filter(|time| *time > now).or_else(|| at(1, 18))?,
            Remind::Tomorrow => at(1, 9)?,
        };
        Some(time.timestamp().as_millisecond())
    }
}

pub struct InboxView {
    workspace: WeakEntity<Workspace>,
    tab: Tab,
    /// Reminders that came due and are not done.
    due: Vec<Todo>,
    /// Schedules that finished since the inbox was last cleared.
    finished: Vec<Schedule>,
    todos: Vec<Todo>,
    new_todo: Entity<InputState>,
    remind: Remind,
    /// An exact reminder time typed by the user (`14:30`, `tomorrow 9am`,
    /// `fri 10:00`, `in 20 min`, `2026-11-02 10:00`); it wins over the
    /// choices when set.
    remind_text: Entity<InputState>,
    /// The typed reminder time is shown (chosen from the reminder menu).
    custom_time: bool,
    _inputs: Vec<Subscription>,
}

impl InboxView {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let new_todo = cx.new(|cx| InputState::new(window, cx).placeholder("Add a to-do…"));
        let remind_text = cx.new(|cx| {
            InputState::new(window, cx).placeholder("14:30, tomorrow 9am, fri 10:00, in 20 min")
        });
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
            tab: Tab::Inbox,
            due: Vec::new(),
            finished: Vec::new(),
            todos: Vec::new(),
            new_todo,
            remind: Remind::Never,
            remind_text: remind_text.clone(),
            custom_time: false,
            // The typed time is read back as it is typed.
            _inputs: vec![input, cx.observe(&remind_text, |_, _, cx| cx.notify())],
        };
        view.refresh(cx);
        view
    }

    /// How many things need the user: questions, reminders, finished runs.
    pub fn count(&self, cx: &mut App) -> usize {
        ph_agent::approvals::pending_count(cx) + self.due.len() + self.finished.len()
    }

    /// Reads the inbox again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let store = AppState::store(cx);
        let now = now_millis();
        self.due = store.due_todos(now).unwrap_or_default();
        let seen: i64 = store.setting(SEEN_KEY);
        self.finished = store.finished_since(seen).unwrap_or_default();
        self.todos = store.all_todos().unwrap_or_default();
        cx.notify();
    }

    /// The new to-do's reminder: the typed time if any (`Err` when it
    /// cannot be read), else the choice.
    fn new_reminder(&self, cx: &App) -> Result<Option<i64>, ()> {
        let typed = self.remind_text.read(cx).value().trim().to_string();
        if typed.is_empty() {
            return Ok(self.remind.at());
        }
        parse_when(&typed, &Zoned::now())
            .map(|at| Some(at.timestamp().as_millisecond()))
            .ok_or(())
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.new_todo.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Ok(remind_at) = self.new_reminder(cx) else {
            window.push_notification(
                Notification::warning(
                    "That reminder time cannot be read; try 14:30 or tomorrow 9am.",
                ),
                cx,
            );
            return;
        };
        match AppState::store(cx).add_todo(None, &text, remind_at) {
            Ok(_) => {
                self.new_todo
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.remind_text
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.remind = Remind::Never;
                self.custom_time = false;
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
        self.remind_again(id, Remind::InAnHour, cx);
    }

    /// Moves a to-do's reminder (`Never` takes it away).
    fn remind_again(&mut self, id: TodoId, remind: Remind, cx: &mut Context<Self>) {
        let _ = AppState::store(cx).set_todo_reminder(id, remind.at());
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
            workspace.open_chat(project_id, session, window, cx)
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
                .py_6()
                .child(
                    EmptyState::new("All caught up")
                        .icon(Icons::Inbox)
                        .description(
                            "Agents' questions, reminders and finished schedules appear here.",
                        ),
                )
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
        let now = now_millis();
        let rows = self.todos.iter().enumerate().map(|(ix, todo)| {
            let (id, done) = (todo.id, todo.done);
            let due = todo.is_due(now);
            let workspace = self.workspace_name(todo.project_id, cx);
            let when = todo
                .remind_at
                .filter(|_| !todo.done)
                .map(|at| reminder_label(at, now));
            let view = cx.entity().downgrade();
            // The reminder, as a pill that moves it.
            let pill = Button::new(("todo-when", ix))
                .xsmall()
                .ghost()
                .icon(if when.is_some() {
                    Icons::BellRing
                } else {
                    Icons::Bell
                })
                .when_some(when.clone(), |this, when| this.label(when))
                .when(due, |this| this.text_color(cx.theme().warning))
                .tooltip(if when.is_some() {
                    "Move or clear the reminder"
                } else {
                    "Remind me"
                })
                .menu(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                    Remind::ALL.iter().fold(menu, |menu, remind| {
                        let (view, remind) = (view.clone(), *remind);
                        let label = match remind {
                            Remind::Never => "Clear the reminder",
                            other => other.label(),
                        };
                        menu.item(
                            gpui_kit::component::menu::PopupMenuItem::new(label).on_click(
                                move |_, _, cx| {
                                    _ = view
                                        .update(cx, |this, cx| this.remind_again(id, remind, cx));
                                },
                            ),
                        )
                    })
                });
            h_flex()
                .id(("todo", ix))
                .group("todo")
                .gap_2p5()
                .px_2()
                .py_1p5()
                .rounded(cx.theme().radius)
                .hover(|style| style.bg(cx.theme().list_hover))
                .when(due, |this| this.bg(cx.theme().warning.opacity(0.1)))
                .child(
                    Checkbox::new(("todo-done", ix))
                        .checked(todo.done)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_done(id, !done, cx))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .when(todo.done, |this| this.line_through().text_color(muted))
                                .child(todo.text.clone()),
                        )
                        // One an agent added for a workspace says which.
                        .when_some(workspace, |this, name| {
                            this.child(div().text_xs().text_color(muted).child(name))
                        }),
                )
                .when(!todo.done, |this| this.child(pill))
                .child(
                    div()
                        .invisible()
                        .group_hover("todo", |style| style.visible())
                        .child(
                            Button::new(("todo-delete", ix))
                                .xsmall()
                                .ghost()
                                .icon(IconName::Close)
                                .tooltip("Delete")
                                .accessibility_label("Delete to-do")
                                .on_click(cx.listener(move |this, _, _, cx| this.delete(id, cx))),
                        ),
                )
        });
        let typed = self.remind_text.read(cx).value().trim().to_string();
        let typed_at = (!typed.is_empty()).then(|| parse_when(&typed, &Zoned::now()));
        let theme = cx.theme();
        let (border, background) = (theme.border, theme.background);
        // The reminder: a menu of times, or one typed.
        let remind_label: SharedString = match &typed_at {
            Some(Some(at)) => at.strftime("%a %-d %b, %H:%M").to_string().into(),
            Some(None) => "Pick a time".into(),
            None if self.remind != Remind::Never => self.remind.label().into(),
            None => "Remind me".into(),
        };
        let reminding =
            matches!(typed_at, Some(Some(_))) || (typed.is_empty() && self.remind != Remind::Never);
        let view = cx.entity().downgrade();
        let remind_button = Button::new("todo-remind")
            .small()
            .ghost()
            .icon(if reminding {
                Icons::BellRing
            } else {
                Icons::Bell
            })
            .label(remind_label)
            .when(reminding, |this| this.text_color(cx.theme().primary))
            .tooltip("When to remind you")
            .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                let pick = |menu: gpui_kit::component::menu::PopupMenu,
                            label: &'static str,
                            remind: Option<Remind>| {
                    let view = view.clone();
                    menu.item(
                        gpui_kit::component::menu::PopupMenuItem::new(label).on_click(
                            move |_, window, cx| {
                                _ = view.update(cx, |this, cx| {
                                    match remind {
                                        Some(remind) => {
                                            this.remind = remind;
                                            this.custom_time = false;
                                            this.remind_text.update(cx, |input, cx| {
                                                input.set_value("", window, cx)
                                            });
                                        }
                                        // Pick a time: the field for it, focused.
                                        None => {
                                            this.custom_time = true;
                                            this.remind_text
                                                .update(cx, |input, cx| input.focus(window, cx));
                                        }
                                    }
                                    cx.notify();
                                });
                            },
                        ),
                    )
                };
                let menu = [
                    Remind::InTenMinutes,
                    Remind::InAnHour,
                    Remind::Tonight,
                    Remind::Tomorrow,
                ]
                .into_iter()
                .fold(menu, |menu, remind| {
                    pick(menu, remind.label(), Some(remind))
                });
                let menu = pick(menu.separator(), "Pick a time…", None);
                pick(menu, "No reminder", Some(Remind::Never))
            });
        let can_add = !self.new_todo.read(cx).value().trim().is_empty();
        let add_button = Button::new("todo-add")
            .small()
            .primary()
            .icon(Icons::Plus)
            .label("Add")
            .disabled(!can_add)
            .tooltip("Add (Enter)")
            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx)));
        let show_time = self.custom_time || !typed.is_empty();
        let composer = v_flex()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(border)
            .bg(background)
            .child(
                h_flex()
                    .pl_3()
                    .pr_2()
                    .pt_2()
                    .gap_2()
                    .child(Icon::new(Icons::Circle).small().text_color(muted))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.new_todo).appearance(false)),
                    ),
            )
            .when(show_time, |this| {
                this.child(
                    h_flex()
                        .pl_3()
                        .pr_3()
                        .gap_2()
                        .child(Icon::new(Icons::CalendarClock).small().text_color(muted))
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.remind_text).appearance(false).small()),
                        )
                        .when_some(typed_at.clone(), |this, at| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(if at.is_some() {
                                        muted
                                    } else {
                                        cx.theme().danger
                                    })
                                    .child(if at.is_some() {
                                        "✓"
                                    } else {
                                        "Cannot read it"
                                    }),
                            )
                        }),
                )
            })
            .child(
                h_flex()
                    .mt_1()
                    .px_1p5()
                    .py_1p5()
                    .gap_1()
                    .border_t_1()
                    .border_color(border)
                    .child(remind_button)
                    .child(div().flex_1())
                    .child(add_button),
            );
        v_flex()
            .gap_3()
            .child(composer)
            .when(self.todos.is_empty(), |this| {
                this.child(
                    div().py_4().child(
                        EmptyState::new("Nothing to do yet")
                            .icon(Icons::ListTodo)
                            .description(
                                "Add what you must not forget; give it a reminder to be told when.",
                            ),
                    ),
                )
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
        let theme = cx.theme();
        let (fg, muted, background, radius) = (
            theme.foreground,
            theme.muted_foreground,
            theme.background,
            theme.radius,
        );
        let tab = |id: &'static str,
                   label: &'static str,
                   number: usize,
                   which: Tab,
                   cx: &mut Context<Self>| {
            let selected = self.tab == which;
            h_flex()
                .id(id)
                .flex_1()
                .h_7()
                .px_3()
                .gap_1p5()
                .justify_center()
                .rounded(radius)
                .cursor_pointer()
                .text_sm()
                .role(gpui_kit::accesskit::Role::Tab)
                .aria_label(label)
                .text_color(if selected { fg } else { muted })
                .when(selected, |this| {
                    this.bg(background)
                        .shadow_xs()
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                })
                .when(!selected, |this| this.hover(|style| style.text_color(fg)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = which;
                    cx.notify();
                }))
                .child(label)
                .when(number > 0, |this| {
                    this.child(
                        div()
                            .px_1p5()
                            .rounded(radius)
                            .bg(cx
                                .theme()
                                .primary
                                .opacity(if selected { 0.18 } else { 0.12 }))
                            .text_color(cx.theme().primary)
                            .text_xs()
                            .child(number.to_string()),
                    )
                })
                .test_support()
        };
        v_flex()
            .w(px(400.))
            .gap_3()
            .p_2()
            .child(
                h_flex()
                    .id("inbox-tabs")
                    .role(gpui_kit::accesskit::Role::TabList)
                    .p_0p5()
                    .gap_0p5()
                    .rounded(radius)
                    .bg(cx.theme().muted)
                    .child(tab("inbox-tab", "Inbox", count, Tab::Inbox, cx))
                    .child(tab("todos-tab", "To-dos", open, Tab::Todos, cx)),
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

/// A reminder time as people type it, after `now`: `14:30`, `9am`,
/// `9:30 pm`, `today 18:00`, `tomorrow 9am`, `fri 10:00` (the next Friday),
/// `2026-11-02 10:00`, `in 20 min`, `in 2 h`, `in 3 days`. A time of day
/// already past today means tomorrow's.
pub(crate) fn parse_when(text: &str, now: &Zoned) -> Option<Zoned> {
    let text = text.trim().to_lowercase();
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.first() == Some(&"in") {
        let (count, unit) = match words.as_slice() {
            [_, count, unit] => (count.parse::<i64>().ok()?, *unit),
            [_, joined] => {
                let split = joined.find(|c: char| !c.is_ascii_digit())?;
                (joined[..split].parse().ok()?, &joined[split..])
            }
            _ => return None,
        };
        let span = match unit {
            "m" | "min" | "mins" | "minute" | "minutes" => count.minutes(),
            "h" | "hr" | "hrs" | "hour" | "hours" => count.hours(),
            "d" | "day" | "days" => count.days(),
            _ => return None,
        };
        return now.checked_add(span).ok();
    }
    // The day, then the time of day.
    let (day, time) = match words.as_slice() {
        [time] => (None, *time),
        [time, meridiem @ ("am" | "pm")] => {
            return parse_when(&format!("{time}{meridiem}"), now);
        }
        [day, time] => (Some(*day), *time),
        [day, time, meridiem @ ("am" | "pm")] => {
            return parse_when(&format!("{day} {time}{meridiem}"), now);
        }
        _ => return None,
    };
    let (hour, minute) = time_of_day(time)?;
    let at = |date: jiff::civil::Date| {
        date.at(hour, minute, 0, 0)
            .to_zoned(now.time_zone().clone())
            .ok()
    };
    let today = now.date();
    match day {
        None | Some("today") => {
            let time = at(today)?;
            if day.is_none() && time <= *now {
                at(today.tomorrow().ok()?)
            } else {
                Some(time)
            }
        }
        Some("tomorrow" | "tmr" | "tom") => at(today.tomorrow().ok()?),
        Some(day) if day.len() == 10 && day.as_bytes()[4] == b'-' => {
            at(day.parse::<jiff::civil::Date>().ok()?)
        }
        Some(day) => {
            let weekday = weekday(day)?;
            let mut date = today.tomorrow().ok()?;
            while date.weekday() != weekday {
                date = date.tomorrow().ok()?;
            }
            at(date)
        }
    }
}

/// `14:30`, `9`, `9am`, `9:30pm`: hour and minute.
fn time_of_day(text: &str) -> Option<(i8, i8)> {
    let (text, offset) = if let Some(rest) = text.strip_suffix("am") {
        (rest, Some(0))
    } else if let Some(rest) = text.strip_suffix("pm") {
        (rest, Some(12))
    } else {
        (text, None)
    };
    let (hour, minute) = match text.split_once([':', '.']) {
        Some((hour, minute)) => (hour.parse::<i8>().ok()?, minute.parse::<i8>().ok()?),
        None => (text.parse::<i8>().ok()?, 0),
    };
    let hour = match offset {
        Some(offset) if (1..=12).contains(&hour) => hour % 12 + offset,
        Some(_) => return None,
        None => hour,
    };
    ((0..24).contains(&hour) && (0..60).contains(&minute)).then_some((hour, minute))
}

fn weekday(text: &str) -> Option<jiff::civil::Weekday> {
    use jiff::civil::Weekday::*;
    let days = [
        ("monday", Monday),
        ("tuesday", Tuesday),
        ("wednesday", Wednesday),
        ("thursday", Thursday),
        ("friday", Friday),
        ("saturday", Saturday),
        ("sunday", Sunday),
    ];
    (text.len() >= 3)
        .then(|| {
            days.iter()
                .find(|(name, _)| name.starts_with(text))
                .map(|(_, day)| *day)
        })
        .flatten()
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
    fn typed_reminder_times_are_read() {
        // Thursday 29 October 2026, 13:00.
        let now: Zoned = "2026-10-29T13:00:00[UTC]".parse().unwrap();
        let read = |text: &str| {
            parse_when(text, &now).map(|at| at.strftime("%a %Y-%m-%d %H:%M").to_string())
        };
        assert_eq!(read("14:30").as_deref(), Some("Thu 2026-10-29 14:30"));
        assert_eq!(
            read("9am").as_deref(),
            Some("Fri 2026-10-30 09:00"),
            "past: tomorrow"
        );
        assert_eq!(read("9:30 pm").as_deref(), Some("Thu 2026-10-29 21:30"));
        assert_eq!(read("12am").as_deref(), Some("Fri 2026-10-30 00:00"));
        assert_eq!(
            read("tomorrow 9am").as_deref(),
            Some("Fri 2026-10-30 09:00")
        );
        assert_eq!(read("Fri 10:00").as_deref(), Some("Fri 2026-10-30 10:00"));
        assert_eq!(
            read("thursday 8").as_deref(),
            Some("Thu 2026-11-05 08:00"),
            "next week's"
        );
        assert_eq!(
            read("2026-11-02 10:00").as_deref(),
            Some("Mon 2026-11-02 10:00")
        );
        assert_eq!(read("in 20 min").as_deref(), Some("Thu 2026-10-29 13:20"));
        assert_eq!(read("in 2h").as_deref(), Some("Thu 2026-10-29 15:00"));
        assert_eq!(read("in 3 days").as_deref(), Some("Sun 2026-11-01 13:00"));
        for bad in ["", "soon", "25:00", "13pm", "in a bit", "xyz 10:00"] {
            assert_eq!(read(bad), None, "{bad}");
        }
    }

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
