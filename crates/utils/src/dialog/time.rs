//! Times and cron lines.
//!
//! A time is shown in every form it is written in (Unix seconds and
//! milliseconds, ISO, an HTTP date, local time), in the time zones the
//! user added (kept for next time), and how far from now. A cron line is
//! said in words, field by field, with its next runs on the clock it will
//! run on: servers and GitHub Actions run cron in UTC, which is where a
//! line written for local time goes wrong.

use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input as Field, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
    div,
};
use jiff::tz::TimeZone;
use ph_ui::Icons;

use super::parts::{self, Tone, Value};
use super::{Ctx, ToolView};
use crate::time::{Clock, Unit};

/// The time zones the user added, kept between openings.
const ZONES: &str = "utilities.time.zones";

pub(crate) struct UnixTime {
    ctx: Ctx,
    input: Entity<InputState>,
    zone: Entity<InputState>,
    zones: Vec<String>,
    /// Every zone name, for finding one by part of it.
    known: Vec<String>,
    _inputs: Vec<Subscription>,
    /// Moves the clock on once a second while the view is open.
    _tick: Task<()>,
}

impl UnixTime {
    fn add_zone(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.zones.contains(&name) && TimeZone::get(&name).is_ok() {
            self.zones.push(name);
            self.remember();
        }
        self.zone
            .update(cx, |zone, cx| zone.set_value("", window, cx));
        cx.notify();
    }

    fn remember(&self) {
        if let Err(err) = self.ctx.store.set_setting(ZONES, &self.zones) {
            tracing::warn!("remembering the time zones failed: {err:#}");
        }
    }

    /// Zone names with what is typed in them, at most six.
    fn found(&self, typed: &str) -> Vec<String> {
        let typed = typed.trim().to_lowercase().replace(' ', "_");
        if typed.is_empty() {
            return Vec::new();
        }
        self.known
            .iter()
            .filter(|name| name.to_lowercase().contains(&typed) && !self.zones.contains(name))
            .take(6)
            .cloned()
            .collect()
    }
}

impl ToolView for UnixTime {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = parts::line(
            "A Unix time (s, ms, µs or ns), an ISO date, an HTTP date… or nothing for now",
            window,
            cx,
        );
        let zone = parts::line(
            "Add a time zone: type a city (Berlin, New York, Tokyo)",
            window,
            cx,
        );
        let inputs = vec![
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe_in(
                &zone,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        let typed = this.zone.read(cx).value().to_string();
                        if let Some(first) = this.found(&typed).into_iter().next() {
                            this.add_zone(first, window, cx);
                        }
                    }
                    _ => cx.notify(),
                },
            ),
        ];
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let zones: Vec<String> = ctx.store.setting(ZONES);
        Self {
            ctx,
            input,
            zone,
            zones,
            known: crate::time::zones(),
            _inputs: inputs,
            _tick: tick,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        cx.notify();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }
}

impl Render for UnixTime {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.input.read(cx).value().to_string();
        let now = jiff::Timestamp::now();
        let read = crate::time::moment(&text);
        let typed_zone = self.zone.read(cx).value().to_string();
        let found = self.found(&typed_zone);
        let rows: AnyElement = match &read {
            Err(err) => v_flex()
                .gap_2()
                .child(parts::failure(format!("{err:#}"), cx))
                .children(parts::suggestion(&text, &self.ctx, cx))
                .into_any_element(),
            Ok((at, unit)) => {
                let at = *at;
                let local = at.to_zoned(TimeZone::system());
                let mut rows: Vec<AnyElement> = vec![
                    Value::new("time-seconds", "Unix seconds", at.as_second().to_string())
                        .into_any_element(),
                    Value::new(
                        "time-millis",
                        "Unix milliseconds",
                        at.as_millisecond().to_string(),
                    )
                    .into_any_element(),
                    Value::new("time-iso", "ISO 8601 (UTC)", at.to_string()).into_any_element(),
                    Value::new(
                        "time-local",
                        "Here",
                        crate::time::in_zone(at, &TimeZone::system()),
                    )
                    .into_any_element(),
                    Value::new("time-http", "HTTP / email", crate::time::http_date(at))
                        .into_any_element(),
                    Value::new(
                        "time-day",
                        "Day",
                        format!(
                            "{}, week {} of {}, day {}",
                            local.strftime("%A"),
                            local.date().iso_week_date().week(),
                            local.date().iso_week_date().year(),
                            local.day_of_year()
                        ),
                    )
                    .plain()
                    .into_any_element(),
                ];
                for (ix, name) in self.zones.iter().enumerate() {
                    let Ok(zone) = TimeZone::get(name) else {
                        continue;
                    };
                    let remove = name.clone();
                    rows.push(
                        h_flex()
                            .child(div().flex_1().min_w_0().child(Value::new(
                                SharedString::from(format!("time-zone-{name}")),
                                name.clone(),
                                crate::time::in_zone(at, &zone),
                            )))
                            .child(
                                Button::new(("time-zone-remove", ix))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .accessibility_label(format!("Remove {name}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.zones.retain(|zone| *zone != remove);
                                        this.remember();
                                        cx.notify();
                                    })),
                            )
                            .into_any_element(),
                    );
                }
                let said = if text.trim().is_empty() {
                    "Now".to_string()
                } else if *unit == Unit::Date {
                    format!("A date · {}", crate::time::from_now(at))
                } else {
                    format!("Read as {} · {}", unit.name(), crate::time::from_now(at))
                };
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .px_2()
                            .pb_1()
                            .child(parts::badge(said, Tone::Plain, cx)),
                    )
                    .children(rows)
                    .into_any_element()
            }
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                h_flex()
                    .gap_2()
                    .child(parts::label("Now", cx))
                    .child(Value::new("time-now", "", now.as_second().to_string()).label_width(0.)),
            ))
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(div().flex_1().child(Field::new(&self.input).small()))
                    .child(
                        parts::action("utility-paste", Icons::ClipboardPaste, "Paste").on_click(
                            cx.listener(|this, _, window, cx| {
                                if let Some(text) = parts::clipboard_text(cx) {
                                    this.take(text.trim().to_string(), window, cx);
                                }
                            }),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .id("time-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_2()
                    .child(rows)
                    .child(
                        v_flex()
                            .gap_1()
                            .px_2()
                            .child(Field::new(&self.zone).small())
                            .when(!found.is_empty(), |this| {
                                this.child(h_flex().flex_wrap().gap_1().children(
                                    found.into_iter().map(|name| {
                                        let add = name.clone();
                                        Button::new(SharedString::from(format!("time-add-{name}")))
                                            .xsmall()
                                            .label(name)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.add_zone(add.clone(), window, cx)
                                            }))
                                    }),
                                ))
                            }),
                    ),
            )
    }
}

/// Lines people write most, a click away.
const PRESETS: [(&str, &str); 6] = [
    ("Every 5 minutes", "*/5 * * * *"),
    ("Hourly", "0 * * * *"),
    ("Daily at 9", "0 9 * * *"),
    ("Weekdays at 9", "0 9 * * 1-5"),
    ("Sundays at midnight", "0 0 * * 0"),
    ("Monthly", "0 0 1 * *"),
];

pub(crate) struct Cron {
    ctx: Ctx,
    line: Entity<InputState>,
    clock: Clock,
    _input: Subscription,
}

impl ToolView for Cron {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let line = parts::line(
            "minute hour day month weekday: */15 9-17 * * 1-5, or @daily",
            window,
            cx,
        );
        let subscription = cx.subscribe(&line, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            ctx,
            line,
            clock: Clock::Local,
            _input: subscription,
        }
    }

    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.line
            .update(cx, |line, cx| line.set_value(text, window, cx));
        cx.notify();
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.line.update(cx, |line, cx| line.focus(window, cx));
    }
}

impl Render for Cron {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let text = self.line.read(cx).value().to_string();
        let presets = h_flex()
            .flex_wrap()
            .gap_1()
            .children(PRESETS.iter().map(|(name, line)| {
                let line = line.to_string();
                Button::new(SharedString::from(format!("cron-preset-{name}")))
                    .xsmall()
                    .label(*name)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.take(line.clone(), window, cx)),
                    )
            }));
        let content: AnyElement = if text.trim().is_empty() {
            parts::hint("Type a cron line, or start from one above.", cx).into_any_element()
        } else {
            match (
                crate::time::normalized(&text),
                crate::time::cron_describe(&text),
                crate::time::cron_next(&text, 10, self.clock),
            ) {
                (Ok(fields), Ok(said), Ok(runs)) => {
                    let names = ["Minute", "Hour", "Day", "Month", "Weekday"];
                    let boxes = fields.iter().zip(names).map(|(field, name)| {
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_0p5()
                            .py_1()
                            .rounded(theme.radius)
                            .bg(theme.muted.opacity(0.5))
                            .child(
                                div()
                                    .font_family(theme.mono_font_family.clone())
                                    .text_sm()
                                    .child(field.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(name),
                            )
                    });
                    let zone = match self.clock {
                        Clock::Local => "this computer's time",
                        Clock::Utc => "UTC",
                    };
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .px_2()
                                .text_lg()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child(said),
                        )
                        .child(h_flex().gap_1().children(boxes))
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    div()
                                        .px_2()
                                        .child(parts::label(format!("Next runs, in {zone}"), cx)),
                                )
                                .children(runs.iter().enumerate().map(|(ix, at)| {
                                    Value::new(
                                        SharedString::from(format!("cron-run-{ix}")),
                                        at.strftime("%a").to_string(),
                                        at.strftime("%Y-%m-%d %H:%M").to_string(),
                                    )
                                    .label_width(36.)
                                    .note(crate::time::from_now(at.timestamp()))
                                })),
                        )
                        .into_any_element()
                }
                (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => v_flex()
                    .gap_2()
                    .child(parts::failure(format!("{err:#}"), cx))
                    .children(parts::suggestion(&text, &self.ctx, cx))
                    .into_any_element(),
            }
        };
        v_flex()
            .size_full()
            .gap_2()
            .child(parts::header(
                &self.ctx,
                parts::modes(
                    "utility-modes",
                    &["Local time", "UTC"],
                    match self.clock {
                        Clock::Local => 0,
                        Clock::Utc => 1,
                    },
                    cx.listener(|this, ix: &usize, _, cx| {
                        this.clock = if *ix == 0 { Clock::Local } else { Clock::Utc };
                        cx.notify();
                    }),
                ),
            ))
            .child(Field::new(&self.line).small().flex_none())
            .child(presets.flex_none())
            .when(self.clock == Clock::Local, |this| {
                this.child(parts::label(
                    "Servers and GitHub Actions run cron in UTC: switch to UTC to see when they will.",
                    cx,
                ))
            })
            .child(
                v_flex()
                    .id("cron-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
    }
}
