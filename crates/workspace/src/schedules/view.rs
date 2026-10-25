//! The Schedules dialog: a workspace's schedules, their last results, and a
//! form to make or change one.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    WeakEntity, Window, div, px,
};
use ph_store::projects::ProjectId;
use ph_store::schedules::{Job, Schedule, ScheduleId, When};

use crate::AppState;
use crate::workspace::Workspace;

/// Ready-made agent prompts: a name and what to ask.
const PRESETS: &[(&str, &str)] = &[
    (
        "Morning summary",
        "Summarize, in one screen, what changed in this workspace since yesterday: the commits in each Git folder \
         (git log --since=yesterday), open pull requests, the PilotHouse board's columns (board_list), and how the UI \
         tests last went (test_list). End with what looks most important to do next.",
    ),
    (
        "Nightly check",
        "Check this workspace's health: build it and run its tests the way the project usually does, run the UI tests \
         with test_list and test_run, and list anything that fails with the file and a likely cause. Do not change any \
         files.",
    ),
    (
        "Review a pull request",
        "Review the pull request named below: read its changes, and list likely bugs, risky parts and missing tests, \
         each with the file and line. Do not change any files.",
    ),
    (
        "Fix failing tests",
        "Some UI tests failed, as said below. Run them with test_run, find why each fails, fix the app (not the tests), \
         and run them again until they pass.",
    ),
];

/// The kinds of "when" the form offers: times, then events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Every {
    Quarter,
    Half,
    Hour,
    Day,
    Weekday,
    Week,
    Files,
    Commit,
    PullRequest,
    ChecksFail,
    After,
}

impl Every {
    const TIMES: [Every; 6] = [
        Every::Quarter,
        Every::Half,
        Every::Hour,
        Every::Day,
        Every::Weekday,
        Every::Week,
    ];
    const EVENTS: [Every; 5] = [
        Every::Files,
        Every::Commit,
        Every::PullRequest,
        Every::ChecksFail,
        Every::After,
    ];

    fn label(self) -> &'static str {
        match self {
            Every::Quarter => "15 min",
            Every::Half => "30 min",
            Every::Hour => "Hourly",
            Every::Day => "Daily",
            Every::Weekday => "Weekdays",
            Every::Week => "Weekly",
            Every::Files => "Files change",
            Every::Commit => "New commit",
            Every::PullRequest => "PR opens",
            Every::ChecksFail => "Checks fail",
            Every::After => "After another",
        }
    }

    fn of(when: &When) -> Self {
        match when {
            When::Minutes { minutes: ..=15 } => Every::Quarter,
            When::Minutes { minutes: 16..=30 } => Every::Half,
            When::Minutes { .. } => Every::Hour,
            When::Daily { .. } => Every::Day,
            When::Weekdays { .. } => Every::Weekday,
            When::Weekly { .. } => Every::Week,
            When::FilesChange { .. } => Every::Files,
            When::Commit => Every::Commit,
            When::PullRequest => Every::PullRequest,
            When::ChecksFail => Every::ChecksFail,
            When::After { .. } => Every::After,
        }
    }

    fn at_a_time(self) -> bool {
        matches!(self, Every::Day | Every::Weekday | Every::Week)
    }

    /// Can start many times a day.
    fn often(self) -> bool {
        !matches!(
            self,
            Every::Day | Every::Weekday | Every::Week | Every::After
        )
    }
}

/// The form for a new schedule, or the one being changed.
struct Form {
    editing: Option<(ScheduleId, bool)>,
    name: Entity<InputState>,
    prompt: Entity<TextareaState>,
    time: Entity<InputState>,
    /// For file changes: only paths containing this.
    pattern: Entity<InputState>,
    tests: bool,
    every: Every,
    weekday: u8,
    /// For chains: the schedule to follow, and only when it fails.
    after: Option<ScheduleId>,
    on_failure: bool,
    error: Option<SharedString>,
}

pub struct SchedulesView {
    project_id: ProjectId,
    workspace: WeakEntity<Workspace>,
    schedules: Vec<Schedule>,
    form: Option<Form>,
    /// Reads the schedules again while open, so results show as they come.
    _refresh: gpui_kit::Task<()>,
}

/// Opens the dialog for the workspace in front.
pub fn open(
    workspace: WeakEntity<Workspace>,
    project_id: ProjectId,
    window: &mut Window,
    cx: &mut App,
) {
    let schedules = AppState::store(cx)
        .schedules(project_id)
        .unwrap_or_default();
    let view = cx.new(|cx| SchedulesView {
        project_id,
        workspace,
        schedules,
        form: None,
        _refresh: cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                let alive = view.update(cx, |view: &mut SchedulesView, cx| {
                    let before = view.schedules.clone();
                    view.reload(cx);
                    if view.schedules != before {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        }),
    });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Schedules").w(px(660.)).child(view.clone())
    });
}

impl SchedulesView {
    fn reload(&mut self, cx: &App) {
        self.schedules = AppState::store(cx)
            .schedules(self.project_id)
            .unwrap_or_default();
    }

    fn name_of(&self, id: ScheduleId) -> Option<String> {
        self.schedules
            .iter()
            .find(|schedule| schedule.id == id)
            .map(|schedule| schedule.name.clone())
    }

    fn open_form(
        &mut self,
        schedule: Option<&Schedule>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name, prompt, tests) = match schedule.map(|schedule| (&schedule.name, &schedule.job)) {
            Some((name, Job::Agent { prompt })) => (name.clone(), prompt.clone(), false),
            Some((name, Job::Tests)) => (name.clone(), String::new(), true),
            None => (String::new(), String::new(), false),
        };
        let when = schedule.map_or(When::Daily { hour: 9, minute: 0 }, |schedule| {
            schedule.timing.clone()
        });
        let (hour, minute, weekday) = match when {
            When::Daily { hour, minute } | When::Weekdays { hour, minute } => (hour, minute, 1),
            When::Weekly {
                weekday,
                hour,
                minute,
            } => (hour, minute, weekday),
            _ => (9, 0, 1),
        };
        let (pattern, after, on_failure) = match &when {
            When::FilesChange { pattern } => (pattern.clone().unwrap_or_default(), None, false),
            When::After {
                schedule,
                on_failure,
            } => (String::new(), Some(*schedule), *on_failure),
            _ => (String::new(), None, false),
        };
        let input = |value: String,
                     placeholder: &'static str,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).placeholder(placeholder);
                state.set_value(value, window, cx);
                state
            })
        };
        let form = Form {
            editing: schedule.map(|schedule| (schedule.id, schedule.enabled)),
            name: input(name, "Name, e.g. Morning summary", window, cx),
            prompt: cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("What the agent should do each time")
                    .rows(4);
                state.set_value(prompt, window, cx);
                state
            }),
            time: input(format!("{hour:02}:{minute:02}"), "09:00", window, cx),
            pattern: input(
                pattern,
                "Only paths containing… (optional), e.g. src/",
                window,
                cx,
            ),
            tests,
            every: Every::of(&when),
            weekday,
            after,
            on_failure,
            error: None,
        };
        form.name.update(cx, |name, cx| name.focus(window, cx));
        self.form = Some(form);
        cx.notify();
    }

    /// The form's "when", or why it is not complete.
    fn when(form: &Form, cx: &App) -> Result<When, &'static str> {
        let time = || -> Result<(u8, u8), &'static str> {
            let text = form.time.read(cx).value().trim().to_string();
            let bad = "Write the time as HH:MM, e.g. 09:00.";
            let (hour, minute) = text.split_once(':').ok_or(bad)?;
            Ok((
                hour.trim().parse().map_err(|_| bad)?,
                minute.trim().parse().map_err(|_| bad)?,
            ))
        };
        Ok(match form.every {
            Every::Quarter => When::Minutes { minutes: 15 },
            Every::Half => When::Minutes { minutes: 30 },
            Every::Hour => When::Minutes { minutes: 60 },
            Every::Day => {
                let (hour, minute) = time()?;
                When::Daily { hour, minute }
            }
            Every::Weekday => {
                let (hour, minute) = time()?;
                When::Weekdays { hour, minute }
            }
            Every::Week => {
                let (hour, minute) = time()?;
                When::Weekly {
                    weekday: form.weekday,
                    hour,
                    minute,
                }
            }
            Every::Files => {
                let pattern = form.pattern.read(cx).value().trim().to_string();
                When::FilesChange {
                    pattern: (!pattern.is_empty()).then_some(pattern),
                }
            }
            Every::Commit => When::Commit,
            Every::PullRequest => When::PullRequest,
            Every::ChecksFail => When::ChecksFail,
            Every::After => When::After {
                schedule: form.after.ok_or("Pick the schedule this one follows.")?,
                on_failure: form.on_failure,
            },
        })
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.form else { return };
        let name = form.name.read(cx).value().to_string();
        let job = if form.tests {
            Job::Tests
        } else {
            Job::Agent {
                prompt: form.prompt.read(cx).value().to_string(),
            }
        };
        let when = match Self::when(form, cx) {
            Ok(when) => when,
            Err(why) => {
                if let Some(form) = &mut self.form {
                    form.error = Some(why.into());
                }
                cx.notify();
                return;
            }
        };
        let store = AppState::store(cx);
        let saved = match form.editing {
            Some((id, enabled)) => store.update_schedule(id, &name, &job, when, enabled),
            None => store.add_schedule(self.project_id, &name, &job, when),
        };
        match saved {
            Ok(_) => {
                self.form = None;
                self.reload(cx);
            }
            Err(err) => {
                if let Some(form) = &mut self.form {
                    form.error = Some(format!("{err:#}").into());
                }
            }
        }
        cx.notify();
    }

    fn set_enabled(&mut self, schedule: &Schedule, enabled: bool, cx: &mut Context<Self>) {
        let _ = AppState::store(cx).update_schedule(
            schedule.id,
            &schedule.name,
            &schedule.job,
            schedule.timing.clone(),
            enabled,
        );
        self.reload(cx);
        cx.notify();
    }

    fn run_now(&mut self, schedule: &Schedule, cx: &mut Context<Self>) {
        let schedule = schedule.clone();
        crate::schedules::Automation::global(cx).update(cx, |automation, cx| {
            automation.run_schedule(&schedule, None, cx)
        });
        self.reload(cx);
        cx.notify();
    }

    fn open_chat(&self, schedule: &Schedule, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), project_id) = (schedule.last_session, schedule.project_id) else {
            return;
        };
        window.close_dialog(cx);
        _ = self.workspace.update(cx, |workspace, cx| {
            workspace.open_chat(project_id, session, window, cx)
        });
    }

    fn delete(&mut self, id: ScheduleId, cx: &mut Context<Self>) {
        let _ = AppState::store(cx).delete_schedule(id);
        self.reload(cx);
        cx.notify();
    }

    fn render_row(
        &self,
        ix: usize,
        schedule: &Schedule,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let what = match &schedule.job {
            Job::Agent { prompt } => {
                let line: String = prompt.chars().take(70).collect();
                format!(
                    "Agent: {line}{}",
                    if prompt.chars().count() > 70 {
                        "…"
                    } else {
                        ""
                    }
                )
            }
            Job::Tests => "Runs the UI tests".to_string(),
        };
        let last = match (schedule.last_run, &schedule.last_result) {
            (Some(at), Some(result)) => {
                format!("Last run {}: {result}", ph_ui::time::ago(at / 1000))
            }
            _ if schedule.timing.is_event() => "Waits for its event".to_string(),
            _ => format!("Next run {}", next_label(schedule.next_run)),
        };
        let when = schedule.timing.describe(|id| self.name_of(id));
        let (toggle, run, chat, edit, remove) = (
            schedule.clone(),
            schedule.clone(),
            schedule.clone(),
            schedule.clone(),
            schedule.id,
        );
        v_flex()
            .gap_1()
            .p_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Switch::new(("schedule-enabled", ix))
                            .checked(schedule.enabled)
                            .small()
                            .on_click(cx.listener(move |this, enabled: &bool, _, cx| {
                                this.set_enabled(&toggle, *enabled, cx)
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(schedule.name.clone()),
                    )
                    .child(div().text_xs().text_color(muted).child(when)),
            )
            .child(div().text_xs().text_color(muted).truncate().child(what))
            .child(
                h_flex()
                    .gap_1()
                    .child(div().flex_1().min_w_0().text_xs().truncate().child(last))
                    .when(schedule.last_session.is_some(), |this| {
                        this.child(
                            Button::new(("schedule-chat", ix))
                                .xsmall()
                                .ghost()
                                .label("Open chat")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_chat(&chat, window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new(("schedule-run", ix))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Play)
                            .label("Run now")
                            .on_click(cx.listener(move |this, _, _, cx| this.run_now(&run, cx))),
                    )
                    .child(
                        Button::new(("schedule-edit", ix))
                            .xsmall()
                            .ghost()
                            .label("Edit")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_form(Some(&edit), window, cx)
                            })),
                    )
                    .child(
                        Button::new(("schedule-delete", ix))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Delete)
                            .accessibility_label("Delete schedule")
                            .on_click(cx.listener(move |this, _, _, cx| this.delete(remove, cx))),
                    ),
            )
    }

    fn render_form(&self, form: &Form, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let label = |text: &'static str| div().text_xs().text_color(muted).child(text);
        let choice = |id: SharedString, text: SharedString, on: bool| {
            Button::new(id)
                .xsmall()
                .when(on, |this| this.primary())
                .when(!on, |this| this.outline())
                .label(text)
        };
        let every_row = |kinds: &[Every], cx: &mut Context<Self>| {
            h_flex()
                .gap_1()
                .flex_wrap()
                .children(kinds.iter().map(|every| {
                    let every = *every;
                    choice(
                        SharedString::from(format!("every-{}", every.label())),
                        every.label().into(),
                        form.every == every,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = &mut this.form {
                            form.every = every;
                        }
                        cx.notify();
                    }))
                }))
        };
        // Schedules this one can follow: not itself.
        let others: Vec<(ScheduleId, String)> = self
            .schedules
            .iter()
            .filter(|schedule| form.editing.is_none_or(|(id, _)| id != schedule.id))
            .map(|schedule| (schedule.id, schedule.name.clone()))
            .collect();
        v_flex()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius)
            .bg(cx.theme().muted.opacity(0.4))
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Name"))
                    .child(Input::new(&form.name).small()),
            )
            .child(
                v_flex().gap_1().child(label("What")).child(
                    h_flex()
                        .gap_1()
                        .child(
                            choice("what-agent".into(), "Ask the agent".into(), !form.tests)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(form) = &mut this.form {
                                        form.tests = false;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            choice("what-tests".into(), "Run the UI tests".into(), form.tests)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(form) = &mut this.form {
                                        form.tests = true;
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
            )
            .when(!form.tests, |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .flex_wrap()
                                .child(div().flex_1().child(label("Prompt")))
                                .children(PRESETS.iter().enumerate().map(|(ix, (name, prompt))| {
                                    Button::new(("preset", ix))
                                        .xsmall()
                                        .ghost()
                                        .label(*name)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if let Some(form) = &this.form {
                                                form.name.update(cx, |input, cx| {
                                                    if input.value().trim().is_empty() {
                                                        input.set_value(name.to_string(), window, cx);
                                                    }
                                                });
                                                form.prompt.update(cx, |input, cx| {
                                                    input.set_value(prompt.to_string(), window, cx)
                                                });
                                            }
                                        }))
                                })),
                        )
                        .child(Textarea::new(&form.prompt)),
                )
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(label("When"))
                    .child(every_row(&Every::TIMES, cx))
                    .child(every_row(&Every::EVENTS, cx))
                    .when(form.every == Every::Week, |this| {
                        this.child(
                            h_flex().gap_1().flex_wrap().children(
                                ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
                                    .into_iter()
                                    .enumerate()
                                    .map(|(ix, day)| {
                                        let weekday = ix as u8 + 1;
                                        choice(
                                            SharedString::from(format!("weekday-{day}")),
                                            day.into(),
                                            form.weekday == weekday,
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(form) = &mut this.form {
                                                form.weekday = weekday;
                                            }
                                            cx.notify();
                                        }))
                                    }),
                            ),
                        )
                    })
                    .when(form.every.at_a_time(), |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .child(div().text_xs().child("at"))
                                .child(div().w(px(80.)).child(Input::new(&form.time).small())),
                        )
                    })
                    .when(form.every == Every::Files, |this| {
                        this.child(Input::new(&form.pattern).small()).child(
                            div().text_xs().text_color(muted).child(
                                "Starts once the files have been quiet for a few seconds. Changes its own run makes do not start it again.",
                            ),
                        )
                    })
                    .when(
                        matches!(form.every, Every::PullRequest | Every::ChecksFail),
                        |this| {
                            this.child(div().text_xs().text_color(muted).child(
                                "PilotHouse asks GitHub every 5 minutes, with the account in Settings → GitHub.",
                            ))
                        },
                    )
                    .when(form.every == Every::After, |this| {
                        this.child(if others.is_empty() {
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("Make the schedule to follow first.")
                                .into_any_element()
                        } else {
                            v_flex()
                                .gap_1()
                                .child(h_flex().gap_1().flex_wrap().children(others.iter().map(
                                    |(id, name)| {
                                        let id = *id;
                                        choice(
                                            SharedString::from(format!("after-{id}")),
                                            name.clone().into(),
                                            form.after == Some(id),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(form) = &mut this.form {
                                                form.after = Some(id);
                                            }
                                            cx.notify();
                                        }))
                                    },
                                )))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            choice("after-any".into(), "Each time it runs".into(), !form.on_failure)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    if let Some(form) = &mut this.form {
                                                        form.on_failure = false;
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            choice("after-failure".into(), "Only when it fails".into(), form.on_failure)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    if let Some(form) = &mut this.form {
                                                        form.on_failure = true;
                                                    }
                                                    cx.notify();
                                                })),
                                        ),
                                )
                                .into_any_element()
                        })
                    }),
            )
            .when(!form.tests && form.every.often(), |this| {
                // Each run is a new chat, with what a chat costs.
                this.child(div().text_xs().text_color(cx.theme().warning).child(
                    "Each run starts a new chat, and each chat costs what any chat with the agent costs. Running this often adds up; the usage view shows how much.",
                ))
            })
            .when_some(form.error.clone(), |this, error| {
                this.child(div().text_xs().text_color(cx.theme().danger).child(error))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("schedule-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.form = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("schedule-save")
                            .small()
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

impl Render for SchedulesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .schedules
            .clone()
            .iter()
            .enumerate()
            .map(|(ix, schedule)| self.render_row(ix, schedule, cx).into_any_element())
            .collect();
        let form = self
            .form
            .as_ref()
            .map(|form| self.render_form(form, cx).into_any_element());
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Schedules run while PilotHouse is open, at set times or when something happens. One that came due while it was closed runs once when it opens."),
                    )
                    .when(self.form.is_none(), |this| {
                        this.child(
                            Button::new("schedule-new")
                                .flex_none()
                                .small()
                                .primary()
                                .icon(IconName::Plus)
                                .label("New schedule")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_form(None, window, cx)
                                })),
                        )
                    }),
            )
            .children(form)
            .when(self.schedules.is_empty() && self.form.is_none(), |this| {
                this.child(
                    ph_ui::EmptyState::new("No schedules yet")
                        .icon(ph_ui::Icons::CalendarClock)
                        .description("Have an agent write a morning summary, review each new pull request, or run the UI tests after every commit."),
                )
            })
            .child(
                v_flex()
                    .id("schedules-list")
                    .gap_2()
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .children(rows),
            )
    }
}

/// "in 3 h", "now".
fn next_label(at: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default();
    let minutes = ((at - now) / 60_000).max(0);
    match minutes {
        0 => "now".into(),
        1..=59 => format!("in {minutes} min"),
        60..=1439 => format!("in {} h", minutes / 60),
        _ => format!("in {} days", minutes / 1440),
    }
}
