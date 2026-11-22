//! The pictures on the tour's cards. Each is the app itself in small, doing
//! the one thing its card is about: an agent's pointer goes to a button and
//! presses it, a failed test is fixed, a review is written and posted. A
//! picture that moves shows what happens; one that stands still only shows
//! that something is there.
//!
//! They are drawn here from the theme's colours, so they need no image
//! file, are sharp at any interface size and sit in a light theme as in a
//! dark one. Every picture is laid out for a box [`WIDTH`] by [`HEIGHT`]
//! pixels at the default interface size, and fills the box it is put in.
//!
//! **How one plays.** A picture is a function of where its loop stands (0
//! to 1): [`playing`] asks for it again at each frame. What happens in it
//! is written as moments of that loop ([`span`]): "the pointer travels from
//! 0.12 to 0.36". A card that comes to the front plays from the start, so
//! what happens is seen in its order; the loop ends on the finished picture
//! and rests there. Someone who has asked the system for less motion gets
//! that finished picture, still ([`REST`]).

use std::time::Duration;

use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Div, FontWeight, Hsla, IntoElement,
    ParentElement as _, Pixels, Rems, SharedString, Styled as _, div, linear_color_stop,
    linear_gradient,
};
use ph_ui::Icons;

/// The box the pictures are laid out for.
pub(crate) const WIDTH: f32 = 612.;
pub(crate) const HEIGHT: f32 = 300.;

/// Where a picture stands when it does not play: in the rest at its loop's
/// end, with everything in it done. (A loop that is not played is held at
/// its start, which here is an empty picture.)
const REST: f32 = 0.95;

fn r(px: f32) -> Rems {
    ph_ui::rems_from_px(px)
}

/// The colours a picture is drawn in: the theme's own.
#[derive(Clone, Copy)]
struct Ink {
    /// A panel of the app, what is written on it, and what is written
    /// small.
    sheet: Hsla,
    ink: Hsla,
    faint: Hsla,
    /// A quiet fill, and a hairline.
    soft: Hsla,
    line: Hsla,
    primary: Hsla,
    on_primary: Hsla,
    good: Hsla,
    bad: Hsla,
    warn: Hsla,
    magenta: Hsla,
    blue: Hsla,
    cyan: Hsla,
    green: Hsla,
    yellow: Hsla,
    red: Hsla,
    radius: Pixels,
    /// The system asks for less motion: the pictures do not play.
    still: bool,
}

impl Ink {
    fn of(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            sheet: theme.background,
            ink: theme.foreground,
            faint: theme.muted_foreground,
            soft: theme.muted,
            line: theme.border,
            primary: theme.primary,
            on_primary: theme.primary_foreground,
            good: theme.success,
            bad: theme.danger,
            warn: theme.warning,
            magenta: theme.magenta,
            blue: theme.blue,
            cyan: theme.cyan,
            green: theme.green,
            yellow: theme.yellow,
            red: theme.red,
            radius: theme.radius_lg,
            still: cx.reduce_motion(),
        }
    }
}

/// A picture that plays: `scene` draws it for a place in its loop, which
/// takes `seconds` and goes round.
fn playing(
    id: &'static str,
    seconds: f32,
    ink: &Ink,
    scene: impl Fn(f32) -> Div + 'static,
) -> AnyElement {
    let still = ink.still;
    div()
        .size_full()
        .with_animation(
            id,
            // Thirty frames a second are smooth for things this small, and
            // half the work of sixty.
            Animation::new(Duration::from_secs_f32(seconds))
                .repeat()
                .with_max_fps(30.),
            move |frame, at| frame.child(scene(if still { REST } else { at })),
        )
        .into_any_element()
}

/// How far along (0 to 1) something is that happens between `from` and
/// `to` of the loop: slow at both ends, as things move.
fn span(at: f32, from: f32, to: f32) -> f32 {
    let x = ((at - from) / (to - from).max(f32::EPSILON)).clamp(0., 1.);
    x * x * (3. - 2. * x)
}

/// Whether the loop is between `from` and `to`.
fn during(at: f32, from: f32, to: f32) -> bool {
    (from..to).contains(&at)
}

fn mix(from: f32, to: f32, x: f32) -> f32 {
    from + (to - from) * x
}

/// The ground of a picture: one colour running into another, with a third
/// coming in from a corner so that it is not one flat sweep. Rounded
/// itself: a frame's corners do not cut what is in it.
fn ground(from: Hsla, to: Hsla, glow: Hsla, ink: &Ink) -> Div {
    div()
        .relative()
        .size_full()
        .rounded(ink.radius)
        .overflow_hidden()
        .bg(linear_gradient(
            135.,
            linear_color_stop(from, 0.),
            linear_color_stop(to, 1.),
        ))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .rounded(ink.radius)
                .bg(linear_gradient(
                    45.,
                    linear_color_stop(glow.opacity(0.), 0.35),
                    linear_color_stop(glow.opacity(0.55), 1.),
                )),
        )
}

/// A panel of the app, at a place in the picture.
fn sheet(x: f32, y: f32, w: f32, h: f32, ink: &Ink) -> Div {
    div()
        .absolute()
        .left(r(x))
        .top(r(y))
        .w(r(w))
        .h(r(h))
        .rounded(r(10.))
        .bg(ink.sheet)
        .border_1()
        .border_color(ink.line.opacity(0.6))
        .shadow_xl()
}

/// A line of words, as a stroke.
fn line(w: f32, h: f32, color: Hsla) -> Div {
    div().w(r(w)).h(r(h)).rounded_full().flex_none().bg(color)
}

/// A few words, small.
fn words(text: impl Into<SharedString>, size: f32, weight: FontWeight, color: Hsla) -> Div {
    div()
        .flex_none()
        .text_size(r(size))
        .font_weight(weight)
        .text_color(color)
        .child(text.into())
}

/// An icon in a rounded spot of its own colour, faint.
fn spot(icon: impl Into<Icon>, size: f32, color: Hsla) -> Div {
    div()
        .size(r(size))
        .flex_none()
        .rounded(r(size * 0.28))
        .bg(color.opacity(0.16))
        .flex()
        .items_center()
        .justify_center()
        .child(icon.into().size(r(size * 0.52)).text_color(color))
}

/// A few words in a pill.
fn pill(text: impl Into<SharedString>, fill: Hsla, on: Hsla) -> Div {
    div()
        .flex_none()
        .px(r(9.))
        .py(r(3.))
        .rounded_full()
        .bg(fill)
        .text_color(on)
        .text_size(r(11.))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

/// A tick in a green spot.
fn tick(size: f32, ink: &Ink) -> Div {
    div()
        .size(r(size))
        .flex_none()
        .rounded_full()
        .bg(ink.good)
        .flex()
        .items_center()
        .justify_center()
        .child(
            Icon::new(Icons::Check)
                .size(r(size * 0.68))
                .text_color(ink.sheet),
        )
}

/// The three dots of a window's bar, with an address when it is a browser's.
fn bar(address: Option<f32>, ink: &Ink) -> Div {
    h_flex()
        .h(r(28.))
        .flex_none()
        .px(r(10.))
        .gap(r(5.))
        .border_b_1()
        .border_color(ink.line)
        .children((0..3).map(|_| div().size(r(7.)).rounded_full().bg(ink.soft)))
        .children(address.map(|width| {
            div()
                .ml(r(8.))
                .w(r(width))
                .h(r(14.))
                .rounded_full()
                .bg(ink.soft)
        }))
}

/// The agent's pointer, its tip at a place in the picture.
fn pointer(x: f32, y: f32, ink: &Ink) -> Div {
    div().absolute().left(r(x)).top(r(y)).child(
        Icon::new(Icons::MousePointer2)
            .size(r(22.))
            .text_color(ink.ink),
    )
}

/// A press: a ring that widens from a place and goes, as `x` runs 0 to 1.
fn press(x: f32, y: f32, at: f32, color: Hsla) -> Div {
    let size = mix(10., 46., at);
    let seen = if at <= 0. || at >= 1. { 0. } else { 1. - at };
    div()
        .absolute()
        .left(r(x - size / 2.))
        .top(r(y - size / 2.))
        .size(r(size))
        .rounded_full()
        .border_2()
        .border_color(color.opacity(seen))
        .bg(color.opacity(seen * 0.18))
}

// ---- 1. The agents --------------------------------------------------------

/// Every agent the app runs, ticked when it is on this computer; under
/// them one session in its two forms: something is said in the chat, the
/// switch goes to the agent's own CLI where it stands too, more is typed
/// there, and back in the chat that is there as well.
pub(crate) fn agents(cx: &App) -> AnyElement {
    let ink = Ink::of(cx);
    let installed = ph_agent::agents::installed();
    let mono = cx.theme().mono_font_family.clone();
    playing("tour-agents", 11., &ink, move |at| {
        let specs = ph_agent::agents::AGENTS;
        let found = |id: &str| {
            installed
                .as_ref()
                .is_some_and(|installed| installed.iter().any(|spec| spec.id == id))
        };
        let count = specs.len() as f32;
        let (tile, gap) = (96., 13.);
        let left = (WIDTH - (count * tile + (count - 1.) * gap)) / 2.;
        let tiles = specs.iter().enumerate().map(|(place, spec)| {
            let seen = span(at, 0.02 + place as f32 * 0.04, 0.1 + place as f32 * 0.04);
            // The one this session is with.
            let picked = place == 0 && at > 0.26;
            sheet(
                left + place as f32 * (tile + gap),
                18. + (1. - seen) * 12.,
                tile,
                92.,
                &ink,
            )
            .opacity(seen)
            .when(picked, |this| this.border_2().border_color(ink.primary))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(r(8.))
            .child(spec.mark(r(32.)))
            .child(words(spec.name, 11.5, FontWeight::MEDIUM, ink.ink))
            .when(found(spec.id), |this| {
                this.child(
                    div()
                        .absolute()
                        .top(r(-8.))
                        .right(r(-8.))
                        .child(tick(22., &ink)),
                )
            })
        });
        // The session's form: the chat, the CLI, and the chat again.
        let cli = during(at, 0.5, 0.8);
        let turned = match at {
            at if at < 0.5 => 1.,
            at if at < 0.8 => span(at, 0.5, 0.55),
            _ => span(at, 0.8, 0.85),
        };
        let form = |name: &'static str, active: bool| {
            div()
                .px(r(9.))
                .py(r(2.))
                .rounded_full()
                .when(active, |this| this.bg(ink.primary))
                .child(words(
                    name,
                    10.5,
                    FontWeight::MEDIUM,
                    if active { ink.on_primary } else { ink.faint },
                ))
        };
        // What is said: asked in the chat, answered, then more typed in
        // the CLI a letter at a time.
        let (ask, answer, more) = ("Fix the login bug", "Fixed in auth.rs", "Add a test");
        let typed = (span(at, 0.6, 0.72) * more.chars().count() as f32).round() as usize;
        let said = |text: &'static str| {
            div()
                .px(r(9.))
                .py(r(2.))
                .rounded(r(9.))
                .bg(ink.primary.opacity(0.16))
                .child(words(text, 11., FontWeight::MEDIUM, ink.ink))
        };
        let chat = v_flex()
            .gap(r(5.))
            .child(
                h_flex()
                    .justify_end()
                    .opacity(span(at, 0.3, 0.35))
                    .child(said(ask)),
            )
            .child(
                h_flex()
                    .gap(r(6.))
                    .opacity(span(at, 0.38, 0.44))
                    .child(
                        Icon::new(Icons::Sparkles)
                            .size(r(12.))
                            .text_color(ink.primary),
                    )
                    .child(words(answer, 11., FontWeight::NORMAL, ink.ink))
                    .child(tick(12., &ink)),
            )
            // What was typed in the CLI is in the chat.
            .child(
                h_flex()
                    .justify_end()
                    .opacity(if at >= 0.8 { 1. } else { 0. })
                    .child(said(more)),
            );
        let prompt = |text: String, caret: bool| {
            h_flex()
                .gap(r(6.))
                .child(words(">", 11., FontWeight::BOLD, ink.good))
                .child(words(text, 11., FontWeight::NORMAL, ink.ink))
                .when(caret, |this| {
                    this.child(div().w(r(6.)).h(r(12.)).bg(ink.ink.opacity(0.7)))
                })
        };
        let terminal = v_flex()
            .gap(r(5.))
            .px(r(10.))
            .py(r(7.))
            .rounded(r(7.))
            .bg(ink.soft)
            .font_family(mono.clone())
            .child(prompt(ask.to_string(), false))
            .child(
                h_flex()
                    .gap(r(6.))
                    .child(div().size(r(7.)).flex_none().rounded_full().bg(ink.good))
                    .child(words(answer, 11., FontWeight::NORMAL, ink.ink)),
            )
            .child(prompt(more.chars().take(typed).collect(), true));
        let session = specs.first().map(|with| {
            sheet(86., 122., 440., 130., &ink)
                .opacity(span(at, 0.22, 0.28))
                .flex()
                .flex_col()
                .gap(r(8.))
                .px(r(16.))
                .py(r(12.))
                .child(
                    h_flex()
                        .gap(r(8.))
                        .child(with.mark(r(18.)))
                        .child(words(with.name, 12.5, FontWeight::SEMIBOLD, ink.ink))
                        .child(
                            pill("Same conversation", ink.good.opacity(0.16), ink.good)
                                .opacity(span(at, 0.55, 0.6)),
                        )
                        .child(div().flex_1())
                        .child(
                            h_flex()
                                .p(r(2.))
                                .rounded_full()
                                .bg(ink.soft)
                                .child(form("Chat", !cli))
                                .child(form("CLI", cli)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .opacity(turned)
                        .child(if cli { terminal } else { chat }),
                )
        });
        let on_computer = match &installed {
            None => "Looking for your agents…",
            Some(installed) if installed.is_empty() => "None on this computer yet",
            Some(_) => "On this computer",
        };
        ground(ink.magenta, ink.blue, ink.cyan, &ink)
            .children(tiles)
            .children(session)
            // The switch, pressed: to the CLI, and back.
            .child(press(493., 145., span(at, 0.46, 0.54), ink.primary))
            .child(press(457., 145., span(at, 0.76, 0.84), ink.primary))
            .child(
                h_flex()
                    .absolute()
                    .left_0()
                    .top(r(262.))
                    .w_full()
                    .justify_center()
                    .child(
                        h_flex()
                            .gap(r(6.))
                            .px(r(12.))
                            .py(r(4.))
                            .rounded_full()
                            .bg(ink.sheet.opacity(0.92))
                            .when(
                                installed.as_ref().is_some_and(|found| !found.is_empty()),
                                |this| this.child(tick(16., &ink)),
                            )
                            .child(words(on_computer, 11.5, FontWeight::NORMAL, ink.ink)),
                    ),
            )
    })
}

// ---- 2. The places --------------------------------------------------------

/// The three places work happens in, each doing what it is for: an agent
/// writing in a project, a plain chat, and a board a task moves across.
pub(crate) fn places(cx: &App) -> AnyElement {
    let ink = Ink::of(cx);
    playing("tour-places", 9., &ink, move |at| {
        // A workspace: the project's files, and the chat whose agent writes.
        let wrote = |from: f32, width: f32, color: Hsla| {
            line(width * span(at, from, from + 0.1), 4., color)
        };
        let workspace = h_flex()
            .gap(r(6.))
            .child(
                v_flex()
                    .w(r(54.))
                    .h(r(44.))
                    .p(r(6.))
                    .gap(r(4.))
                    .rounded(r(5.))
                    .bg(ink.soft)
                    .child(
                        h_flex()
                            .gap(r(3.))
                            .child(
                                Icon::new(Icons::FolderOpen)
                                    .size(r(9.))
                                    .text_color(ink.faint),
                            )
                            .child(line(26., 4., ink.faint.opacity(0.7))),
                    )
                    .child(line(22., 4., ink.faint.opacity(0.5)))
                    .child(line(30., 4., ink.faint.opacity(0.5))),
            )
            .child(
                v_flex()
                    .w(r(88.))
                    .h(r(44.))
                    .p(r(6.))
                    .gap(r(4.))
                    .rounded(r(5.))
                    .bg(ink.soft)
                    .child(wrote(0.06, 64., ink.blue.opacity(0.85)))
                    .child(wrote(0.13, 48., ink.green.opacity(0.85)))
                    .child(wrote(0.2, 56., ink.magenta.opacity(0.85))),
            );
        // A chat that needs no folder: a question, and its answer.
        let chat = v_flex()
            .w(r(148.))
            .gap(r(5.))
            .child(
                h_flex()
                    .justify_end()
                    .child(line(72., 12., ink.blue.opacity(0.35)).opacity(span(at, 0.3, 0.36))),
            )
            .child(line(108. * span(at, 0.38, 0.48), 12., ink.soft))
            .child(line(68. * span(at, 0.46, 0.54), 12., ink.soft));
        // The Bridge: a task leaves "working" and waits for the user.
        let moved = span(at, 0.62, 0.74);
        let card = |color: Hsla| div().w_full().h(r(13.)).rounded(r(3.)).bg(color);
        let column = |color: Hsla| v_flex().w(r(46.)).gap(r(3.)).child(line(24., 4., color));
        let bridge = h_flex()
            .relative()
            .items_start()
            .gap(r(6.))
            .child(
                column(ink.warn)
                    .child(card(ink.soft))
                    // Its place, taken when it arrives.
                    .child(card(ink.warn.opacity(0.35)).opacity(if moved >= 1. { 1. } else { 0. })),
            )
            .child(
                column(ink.blue)
                    .child(card(ink.soft))
                    .child(card(ink.soft).opacity(if moved > 0. { 0. } else { 1. })),
            )
            .child(column(ink.good).child(card(ink.soft)))
            // On its way.
            .when(moved > 0. && moved < 1., |this| {
                this.child(
                    div()
                        .absolute()
                        .left(r(mix(52., 0., moved)))
                        .top(r(23.))
                        .w(r(46.))
                        .h(r(13.))
                        .rounded(r(3.))
                        .bg(ink.warn.opacity(0.5))
                        .shadow_md(),
                )
            });
        let row = |place: f32, icon: Icons, color: Hsla, name: &'static str, shows: Div| {
            let seen = span(at, place * 0.04, 0.1 + place * 0.04);
            sheet(86., 24. + place * 86. + (1. - seen) * 10., 440., 76., &ink)
                .opacity(seen)
                .flex()
                .items_center()
                .gap(r(14.))
                .px(r(16.))
                .child(spot(Icon::new(icon), 42., color))
                .child(
                    div()
                        .flex_1()
                        .text_size(r(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ink.ink)
                        .child(name),
                )
                .child(shows)
        };
        ground(ink.blue, ink.cyan, ink.magenta, &ink)
            .child(row(0., Icons::Folders, ink.blue, "Workspaces", workspace))
            .child(row(1., Icons::MessagesSquare, ink.magenta, "Chats", chat))
            .child(row(2., Icons::ShipWheel, ink.cyan, "Bridge", bridge))
    })
}

// ---- 3. Preview -----------------------------------------------------------

/// The agent presses a button in the built-in browser, goes on in the
/// user's own Chrome where they are signed in, then taps on their phone.
pub(crate) fn preview(cx: &App) -> AnyElement {
    let ink = Ink::of(cx);
    playing("tour-preview", 10., &ink, move |at| {
        // What the agent is doing, said in the chat as it does it.
        let doing = match at {
            at if at < 0.1 => "Opens the page",
            at if at < 0.48 => "Clicks “Buy”",
            at if at < 0.64 => "Now in your Chrome",
            _ => "Taps on your phone",
        };
        let chat = sheet(22., 40., 146., 220., &ink)
            .flex()
            .flex_col()
            .gap(r(9.))
            .p(r(12.))
            .child(
                h_flex().justify_end().child(
                    div()
                        .px(r(8.))
                        .py(r(5.))
                        .rounded(r(8.))
                        .bg(ink.primary.opacity(0.16))
                        .child(words("Try the checkout", 10.5, FontWeight::MEDIUM, ink.ink)),
                ),
            )
            .child(
                h_flex()
                    .gap(r(6.))
                    .child(
                        Icon::new(Icons::Sparkles)
                            .size(r(13.))
                            .text_color(ink.primary),
                    )
                    .child(line(74., 6., ink.faint.opacity(0.6))),
            )
            .child(line(104., 6., ink.faint.opacity(0.4)))
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap(r(5.))
                    .child(
                        Icon::new(Icons::MousePointer2)
                            .size(r(12.))
                            .text_color(ink.primary),
                    )
                    .child(words(doing, 10.5, FontWeight::MEDIUM, ink.ink)),
            );
        // Which browser: the built-in one, then the user's own.
        let own = at >= 0.5;
        let which = |name: &'static str, active: bool| {
            div()
                .px(r(7.))
                .py(r(2.))
                .rounded_full()
                .when(active, |this| this.bg(ink.primary))
                .child(words(
                    name,
                    9.5,
                    FontWeight::MEDIUM,
                    if active { ink.on_primary } else { ink.faint },
                ))
        };
        let bought = at >= 0.42;
        let button = |width: f32, done: bool| {
            div()
                .w(r(width))
                .h(r(26.))
                .rounded(r(7.))
                .bg(if done { ink.good } else { ink.primary })
                .flex()
                .items_center()
                .justify_center()
                .child(if done {
                    Icon::new(Icons::Check)
                        .size(r(14.))
                        .text_color(ink.sheet)
                        .into_any_element()
                } else {
                    line(width * 0.4, 5., ink.on_primary).into_any_element()
                })
        };
        let browser = sheet(180., 24., 286., 252., &ink)
            .flex()
            .flex_col()
            .child(
                h_flex()
                    .h(r(30.))
                    .flex_none()
                    .px(r(10.))
                    .gap(r(5.))
                    .border_b_1()
                    .border_color(ink.line)
                    .children((0..3).map(|_| div().size(r(7.)).rounded_full().bg(ink.soft)))
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .p(r(2.))
                            .rounded_full()
                            .bg(ink.soft)
                            .child(which("Built-in", !own))
                            .child(which("Your Chrome", own)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p(r(16.))
                    .gap(r(9.))
                    .child(
                        h_flex()
                            .child(line(128., 10., ink.ink.opacity(0.85)))
                            .child(div().flex_1())
                            // Their own Chrome: signed in already.
                            .child(
                                h_flex()
                                    .gap(r(4.))
                                    .opacity(span(at, 0.52, 0.58))
                                    .child(tick(13., &ink))
                                    .child(words("Signed in", 10., FontWeight::MEDIUM, ink.good)),
                            ),
                    )
                    .child(line(216., 6., ink.faint.opacity(0.5)))
                    .child(line(170., 6., ink.faint.opacity(0.5)))
                    // What is sold: three things, one of them chosen.
                    .child(
                        h_flex().gap(r(8.)).pt(r(4.)).children(
                            [ink.magenta, ink.yellow, ink.cyan]
                                .into_iter()
                                .enumerate()
                                .map(|(place, color)| {
                                    v_flex()
                                        .w(r(60.))
                                        .gap(r(5.))
                                        .child(
                                            div()
                                                .w_full()
                                                .h(r(46.))
                                                .rounded(r(7.))
                                                .bg(color.opacity(0.32))
                                                .when(place == 0, |this| {
                                                    this.border_2().border_color(ink.primary)
                                                }),
                                        )
                                        .child(line(38., 4., ink.faint.opacity(0.5)))
                                }),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex().gap(r(10.)).child(button(88., bought)).child(
                            words("Order placed", 10.5, FontWeight::MEDIUM, ink.good)
                                .opacity(span(at, 0.42, 0.48)),
                        ),
                    ),
            );
        let tapped = at >= 0.76;
        let phone = div()
            .absolute()
            .left(r(480.))
            .top(r(34.))
            .w(r(106.))
            .h(r(232.))
            .rounded(r(20.))
            .bg(ink.sheet)
            .border_2()
            .border_color(ink.line)
            .shadow_xl()
            .flex()
            .flex_col()
            .items_center()
            .gap(r(8.))
            .p(r(10.))
            .child(line(30., 5., ink.soft))
            .child(div().h(r(4.)))
            .child(line(66., 8., ink.ink.opacity(0.85)))
            .child(line(80., 5., ink.faint.opacity(0.5)))
            .child(
                div()
                    .w(r(82.))
                    .h(r(64.))
                    .rounded(r(7.))
                    .bg(ink.magenta.opacity(0.32)),
            )
            .child(line(56., 4., ink.faint.opacity(0.5)))
            .child(div().flex_1())
            .child(button(82., tapped));
        // The pointer goes to the button and presses it.
        let travel = span(at, 0.12, 0.36);
        let (x, y) = (mix(410., 232., travel), mix(86., 240., travel));
        // On the phone it is a finger: a touch that lands, and its ring.
        let touch = span(at, 0.66, 0.74);
        ground(ink.cyan, ink.blue, ink.green, &ink)
            .child(chat)
            .child(browser)
            .child(phone)
            .child(press(240., 247., span(at, 0.36, 0.46), ink.primary))
            .child(press(533., 241., span(at, 0.74, 0.86), ink.ink))
            .when(during(at, 0.64, 0.8), |this| {
                this.child(
                    div()
                        .absolute()
                        .left(r(533. - 11.))
                        .top(r(241. - 11. + (1. - touch) * 14.))
                        .size(r(22.))
                        .rounded_full()
                        .bg(ink.ink.opacity(0.55 * touch))
                        .border_2()
                        .border_color(ink.sheet.opacity(touch)),
                )
            })
            .when(during(at, 0.1, 0.5), |this| this.child(pointer(x, y, &ink)))
    })
}

// ---- 4. Tests and Utilities -----------------------------------------------

/// The project's tests running, one failing and being fixed, a UI test
/// being recorded; beside them a utility turning a token into what it says.
pub(crate) fn tests(cx: &App) -> AnyElement {
    let ink = Ink::of(cx);
    playing("tour-tests", 10., &ink, move |at| {
        // A test: waiting, then run.
        let waiting = || {
            div()
                .size(r(16.))
                .flex_none()
                .rounded_full()
                .border_2()
                .border_color(ink.faint.opacity(0.4))
        };
        let passed = |from: f32, width: f32| {
            h_flex()
                .h(r(30.))
                .gap(r(9.))
                .child(if at >= from {
                    tick(16., &ink)
                } else {
                    waiting()
                })
                .child(line(width, 6., ink.faint.opacity(0.6)))
        };
        // The one that fails, and is fixed.
        let (failed, fixed) = (at >= 0.2, at >= 0.44);
        let failing = h_flex()
            .h(r(36.))
            .gap(r(9.))
            .px(r(6.))
            .mx(r(-6.))
            .rounded(r(7.))
            .when(failed && !fixed, |this| this.bg(ink.bad.opacity(0.1)))
            .child(if fixed {
                tick(16., &ink)
            } else if failed {
                div().size(r(16.)).flex_none().child(
                    Icon::new(IconName::CircleX)
                        .size(r(16.))
                        .text_color(ink.bad),
                )
            } else {
                waiting()
            })
            .child(line(
                84.,
                6.,
                if failed && !fixed {
                    ink.bad.opacity(0.7)
                } else {
                    ink.faint.opacity(0.6)
                },
            ))
            .child(div().flex_1())
            .when(failed && !fixed, |this| {
                this.child(
                    pill("Fix with agent", ink.primary, ink.on_primary)
                        .opacity(span(at, 0.24, 0.3)),
                )
            });
        // A UI test, recorded step by step and kept.
        let kept = at >= 0.78;
        let step = |from: f32, number: &'static str| {
            div()
                .size(r(18.))
                .flex_none()
                .rounded_full()
                .bg(ink.primary.opacity(0.16))
                .flex()
                .items_center()
                .justify_center()
                .opacity(span(at, from, from + 0.05))
                .child(words(number, 10., FontWeight::SEMIBOLD, ink.primary))
        };
        let recorder = h_flex()
            .h(r(34.))
            .gap(r(7.))
            .px(r(10.))
            .rounded(r(8.))
            .bg(ink.soft)
            .opacity(span(at, 0.5, 0.56))
            .child(if kept {
                tick(14., &ink)
            } else {
                div().size(r(9.)).flex_none().rounded_full().bg(ink.bad)
            })
            .child(words(
                if kept {
                    "Saved as a UI test"
                } else {
                    "Recording"
                },
                11.,
                FontWeight::MEDIUM,
                ink.ink,
            ))
            .child(div().flex_1())
            .when(!kept, |this| {
                this.child(step(0.58, "1"))
                    .child(step(0.64, "2"))
                    .child(step(0.7, "3"))
            });
        let list = sheet(24., 22., 304., 256., &ink)
            .flex()
            .flex_col()
            .p(r(14.))
            .child(
                h_flex()
                    .pb(r(4.))
                    .gap(r(7.))
                    .child(
                        Icon::new(Icons::FlaskConical)
                            .size(r(14.))
                            .text_color(ink.ink),
                    )
                    .child(words("Tests", 12.5, FontWeight::SEMIBOLD, ink.ink)),
            )
            .child(passed(0.06, 150.))
            .child(passed(0.13, 118.))
            .child(failing)
            .child(passed(0.27, 136.))
            .child(div().flex_1())
            .child(recorder);
        // A utility: a token goes in, what it says comes out.
        let tool = |name: &'static str, active: bool| {
            div()
                .px(r(8.))
                .py(r(3.))
                .rounded(r(6.))
                .bg(if active { ink.primary } else { ink.soft })
                .child(words(
                    name,
                    10.5,
                    FontWeight::MEDIUM,
                    if active { ink.on_primary } else { ink.ink },
                ))
        };
        let read = |from: f32, key: &'static str, value: &'static str| {
            h_flex()
                .gap(r(6.))
                .opacity(span(at, from, from + 0.06))
                .child(words(key, 10.5, FontWeight::NORMAL, ink.faint))
                .child(words(value, 10.5, FontWeight::MEDIUM, ink.ink))
        };
        let utilities = sheet(340., 22., 250., 256., &ink)
            .flex()
            .flex_col()
            .gap(r(9.))
            .p(r(14.))
            .child(
                h_flex()
                    .gap(r(7.))
                    .child(Icon::new(Icons::Wrench).size(r(14.)).text_color(ink.ink))
                    .child(words("Utilities", 12.5, FontWeight::SEMIBOLD, ink.ink)),
            )
            .child(
                h_flex()
                    .gap(r(5.))
                    .child(tool("JSON", false))
                    .child(tool("JWT", at >= 0.3))
                    .child(tool("Base64", false))
                    .child(tool("Regex", false)),
            )
            .child(
                h_flex()
                    .gap(r(5.))
                    .child(tool("Hash", false))
                    .child(tool("UUID", false))
                    .child(tool("Cron", false))
                    .child(tool("Color", false)),
            )
            .child(
                div()
                    .mt(r(2.))
                    .px(r(9.))
                    .py(r(6.))
                    .rounded(r(7.))
                    .border_1()
                    .border_color(ink.line)
                    .child(words(
                        "eyJhbGciOiJIUzI1NiIsInR5cCI6…",
                        10.,
                        FontWeight::NORMAL,
                        ink.faint,
                    )),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap(r(5.))
                    .px(r(9.))
                    .py(r(8.))
                    .rounded(r(7.))
                    .bg(ink.soft)
                    .child(read(0.34, "alg", "HS256"))
                    .child(read(0.39, "user", "ada@example.com"))
                    .child(read(0.44, "expires", "in 2 hours")),
            );
        ground(ink.green, ink.cyan, ink.yellow, &ink)
            .child(list)
            .child(utilities)
    })
}

// ---- 5. Reviews -----------------------------------------------------------

/// The pull requests that wait on GitHub; an agent reads one, writes what
/// it finds, the user keeps two of the three, and the review is posted.
pub(crate) fn review(cx: &App) -> AnyElement {
    let ink = Ink::of(cx);
    playing("tour-review", 11., &ink, move |at| {
        let request = |selected: bool, width: f32, who: Hsla| {
            h_flex()
                .h(r(46.))
                .gap(r(9.))
                .px(r(8.))
                .mx(r(-8.))
                .rounded(r(8.))
                .when(selected, |this| this.bg(ink.primary.opacity(0.12)))
                .child(
                    Icon::new(Icons::GitPullRequest)
                        .size(r(15.))
                        .text_color(ink.good),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .gap(r(6.))
                        .child(line(width, 6., ink.ink.opacity(0.75)))
                        .child(line(width * 0.55, 5., ink.faint.opacity(0.45))),
                )
                .child(div().size(r(18.)).flex_none().rounded_full().bg(who))
        };
        let waiting = sheet(22., 18., 236., 264., &ink)
            .flex()
            .flex_col()
            .gap(r(3.))
            .p(r(14.))
            .child(
                h_flex()
                    .pb(r(7.))
                    .gap(r(7.))
                    .child(Icon::new(Icons::Github).size(r(17.)).text_color(ink.ink))
                    .child(words(
                        "Waiting for you",
                        12.5,
                        FontWeight::SEMIBOLD,
                        ink.ink,
                    ))
                    .child(div().flex_1())
                    .child(pill("3", ink.warn.opacity(0.2), ink.warn)),
            )
            .child(request(at >= 0.05, 120., ink.magenta))
            .child(request(false, 98., ink.cyan))
            .child(request(false, 134., ink.yellow));
        // What the agent found, each kept or left by the user.
        let read = span(at, 0.08, 0.26);
        let finding = |from: f32, kept_at: Option<f32>, color: Hsla, width: f32| {
            let kept = kept_at.is_some_and(|kept| at >= kept);
            h_flex()
                .items_start()
                .gap(r(8.))
                .opacity(span(at, from, from + 0.06))
                .child(
                    div()
                        .mt(r(1.))
                        .size(r(14.))
                        .flex_none()
                        .rounded(r(4.))
                        .border_1()
                        .border_color(if kept {
                            ink.primary
                        } else {
                            ink.faint.opacity(0.5)
                        })
                        .when(kept, |this| this.bg(ink.primary))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(kept, |this| {
                            this.child(
                                Icon::new(Icons::Check)
                                    .size(r(10.))
                                    .text_color(ink.on_primary),
                            )
                        }),
                )
                .child(
                    div()
                        .mt(r(4.))
                        .size(r(8.))
                        .flex_none()
                        .rounded_full()
                        .bg(color),
                )
                .child(
                    v_flex()
                        .gap(r(5.))
                        .child(line(width, 6., ink.faint.opacity(0.7)))
                        .child(line(width * 0.68, 6., ink.faint.opacity(0.4))),
                )
        };
        let posted = at >= 0.8;
        let agent = sheet(270., 18., 320., 264., &ink)
            .flex()
            .flex_col()
            .gap(r(8.))
            .p(r(14.))
            .child(
                h_flex()
                    .gap(r(7.))
                    .child(
                        Icon::new(Icons::Sparkles)
                            .size(r(14.))
                            .text_color(ink.primary),
                    )
                    .child(words("Agent review", 12.5, FontWeight::SEMIBOLD, ink.ink))
                    .child(div().flex_1())
                    .child(words(
                        if read < 1. {
                            "Reading the changes…"
                        } else {
                            "3 findings"
                        },
                        10.5,
                        FontWeight::NORMAL,
                        ink.faint,
                    )),
            )
            .child(
                div().w_full().h(r(4.)).rounded_full().bg(ink.soft).child(
                    div()
                        .h_full()
                        .rounded_full()
                        .w(gpui_kit::relative(read.max(0.02)))
                        .bg(ink.primary),
                ),
            )
            // The changes it reads: lines taken out, lines put in.
            .child(
                v_flex()
                    .gap(r(4.))
                    .px(r(9.))
                    .py(r(7.))
                    .rounded(r(7.))
                    .bg(ink.soft)
                    .children(
                        [
                            (ink.bad, "−", 150.),
                            (ink.good, "+", 186.),
                            (ink.good, "+", 122.),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(place, (color, sign, width))| {
                            let from = 0.08 + place as f32 * 0.05;
                            h_flex()
                                .gap(r(6.))
                                .opacity(span(at, from, from + 0.05).max(0.25))
                                .child(words(sign, 10., FontWeight::BOLD, color))
                                .child(line(width, 5., color.opacity(0.55)))
                        }),
                    ),
            )
            .child(finding(0.3, Some(0.56), ink.bad, 190.))
            .child(finding(0.38, Some(0.62), ink.warn, 160.))
            .child(finding(0.46, None, ink.faint, 176.))
            .child(div().flex_1())
            .child(h_flex().child(div().flex_1()).child(if posted {
                h_flex()
                    .gap(r(6.))
                    .px(r(10.))
                    .py(r(5.))
                    .rounded(r(7.))
                    .bg(ink.good.opacity(0.16))
                    .child(Icon::new(Icons::Github).size(r(13.)).text_color(ink.good))
                    .child(words(
                        "Posted to GitHub",
                        11.,
                        FontWeight::SEMIBOLD,
                        ink.good,
                    ))
            } else {
                h_flex()
                    .gap(r(6.))
                    .px(r(10.))
                    .py(r(5.))
                    .rounded(r(7.))
                    .bg(ink.primary)
                    .opacity(span(at, 0.62, 0.68).max(0.35))
                    .child(words(
                        "Post review",
                        11.,
                        FontWeight::SEMIBOLD,
                        ink.on_primary,
                    ))
            }));
        ground(ink.yellow, ink.red, ink.magenta, &ink)
            .child(waiting)
            .child(agent)
            .child(press(536., 255., span(at, 0.72, 0.82), ink.primary))
    })
}

// ---- 6. Artifacts ---------------------------------------------------------

/// Two things asked for in a chat and made: a deck, written slide by slide
/// and exported, and a small app whose numbers move, shared with two
/// people.
pub(crate) fn artifacts(cx: &App) -> AnyElement {
    let ink = Ink::of(cx);
    playing("tour-artifacts", 11., &ink, move |at| {
        let colors = [ink.magenta, ink.blue, ink.cyan];
        let bars = |heights: &[f32], width: f32, grown: f32| {
            h_flex().items_end().gap(r(5.)).children(
                heights
                    .iter()
                    .enumerate()
                    .map(|(place, height)| {
                        div()
                            .w(r(width))
                            .h(r((height * grown).max(2.)))
                            .rounded(r(3.))
                            .bg(colors[place % 3])
                    })
                    .collect::<Vec<_>>(),
            )
        };
        // What was asked, as the chat has it.
        let asked = |x: f32, from: f32, text: &'static str| {
            h_flex()
                .absolute()
                .left(r(x))
                .top(r(16.))
                .gap(r(6.))
                .px(r(10.))
                .py(r(5.))
                .rounded(r(10.))
                .bg(ink.sheet)
                .shadow_md()
                .opacity(span(at, from, from + 0.05))
                .child(
                    Icon::new(Icons::MessageSquare)
                        .size(r(12.))
                        .text_color(ink.primary),
                )
                .child(words(text, 11., FontWeight::MEDIUM, ink.ink))
        };
        // The slides, written one after another: a title, a chart, a list.
        let made = [0.08, 0.22, 0.36];
        let slide = made.iter().rposition(|from| at >= *from).unwrap_or(0);
        let turned = span(at, made[slide], made[slide] + 0.06);
        let wash = |color: Hsla| {
            // A slide has a look of its own: a wash of colour and a shape
            // in a corner.
            div()
                .relative()
                .size_full()
                .rounded(r(6.))
                .overflow_hidden()
                .bg(linear_gradient(
                    135.,
                    linear_color_stop(color.opacity(0.34), 0.),
                    linear_color_stop(ink.blue.opacity(0.14), 1.),
                ))
                .child(
                    div()
                        .absolute()
                        .right(r(-26.))
                        .top(r(-26.))
                        .size(r(84.))
                        .rounded_full()
                        .bg(color.opacity(0.3)),
                )
        };
        let shown = match slide {
            0 => wash(ink.magenta).child(
                v_flex()
                    .size_full()
                    .justify_center()
                    .gap(r(8.))
                    .p(r(16.))
                    .child(line(34., 4., ink.magenta))
                    .child(words("Launch plan", 19., FontWeight::BOLD, ink.ink))
                    .child(line(120., 5., ink.faint.opacity(0.7)))
                    .child(line(84., 5., ink.faint.opacity(0.45))),
            ),
            1 => wash(ink.cyan).child(
                v_flex()
                    .size_full()
                    .p(r(16.))
                    .gap(r(6.))
                    .child(words("Sign-ups", 13., FontWeight::BOLD, ink.ink))
                    .child(div().flex_1())
                    .child(bars(&[22., 38., 30., 52., 40., 64., 76.], 15., turned)),
            ),
            _ => wash(ink.yellow).child(
                h_flex()
                    .size_full()
                    .p(r(16.))
                    .gap(r(12.))
                    .child(
                        v_flex()
                            .flex_1()
                            .gap(r(9.))
                            .child(words("Next steps", 13., FontWeight::BOLD, ink.ink))
                            .children([92., 74., 84.].into_iter().map(|width| {
                                h_flex().gap(r(6.)).child(tick(11., &ink)).child(line(
                                    width,
                                    5.,
                                    ink.faint.opacity(0.7),
                                ))
                            })),
                    )
                    .child(
                        div()
                            .w(r(62.))
                            .h(r(62.))
                            .rounded(r(8.))
                            .bg(ink.yellow.opacity(0.35)),
                    ),
            ),
        };
        let thumb = |place: usize, color: Hsla| {
            div()
                .w(r(46.))
                .h(r(28.))
                .rounded(r(4.))
                .bg(color.opacity(0.3))
                .border_2()
                .border_color(if place == slide {
                    ink.primary
                } else {
                    ink.sheet
                })
                .opacity(span(at, made[place], made[place] + 0.05))
                .flex()
                .items_center()
                .justify_center()
                .child(line(22., 3., ink.ink.opacity(0.5)))
        };
        let deck = sheet(24., 52., 318., 176., &ink)
            .opacity(span(at, 0.05, 0.1))
            .flex()
            .gap(r(10.))
            .p(r(10.))
            .child(
                v_flex()
                    .gap(r(7.))
                    .child(thumb(0, ink.magenta))
                    .child(thumb(1, ink.cyan))
                    .child(thumb(2, ink.yellow)),
            )
            .child(div().flex_1().h_full().child(shown.opacity(turned)));
        // The small app: a number that counts, bars that grow.
        let counted = span(at, 0.6, 0.76);
        let spent = mix(1_180., 1_284., counted) as u32;
        let app = sheet(354., 52., 234., 176., &ink)
            .opacity(span(at, 0.56, 0.62))
            .flex()
            .flex_col()
            .child(bar(Some(110.), &ink))
            .child(
                h_flex()
                    .flex_1()
                    .items_end()
                    .p(r(14.))
                    .gap(r(10.))
                    .child(
                        v_flex()
                            .h_full()
                            .gap(r(4.))
                            .child(words(
                                "Spent this month",
                                10.5,
                                FontWeight::NORMAL,
                                ink.faint,
                            ))
                            .child(words(
                                format!("{},{:03}", spent / 1000, spent % 1000),
                                24.,
                                FontWeight::BOLD,
                                ink.ink,
                            ))
                            .child(div().flex_1())
                            .child(pill("Under budget", ink.good.opacity(0.18), ink.good)),
                    )
                    .child(div().flex_1())
                    .child(bars(&[26., 44., 34., 58., 70.], 12., counted.max(0.15))),
            );
        let person = |from: f32, color: Hsla| {
            div()
                .size(r(22.))
                .ml(r(-6.))
                .rounded_full()
                .bg(color)
                .border_2()
                .border_color(ink.sheet)
                .opacity(span(at, from, from + 0.05))
        };
        ground(ink.red, ink.magenta, ink.blue, &ink)
            .child(asked(24., 0.01, "Make slides about our launch"))
            .child(asked(354., 0.5, "Build a budget dashboard"))
            .child(deck)
            .child(app)
            // The deck leaves as a file.
            .child(
                h_flex()
                    .absolute()
                    .left(r(24.))
                    .top(r(244.))
                    .gap(r(7.))
                    .opacity(span(at, 0.44, 0.5))
                    .child(
                        Icon::new(Icons::Download)
                            .size(r(15.))
                            .text_color(ink.sheet),
                    )
                    .child(pill("PowerPoint", ink.sheet, ink.ink))
                    .child(pill("PDF", ink.sheet, ink.ink)),
            )
            // The app is opened by others.
            .child(
                h_flex()
                    .absolute()
                    .left(r(360.))
                    .top(r(242.))
                    .child(person(0.8, ink.cyan))
                    .child(person(0.83, ink.yellow))
                    .child(
                        h_flex()
                            .ml(r(8.))
                            .gap(r(5.))
                            .px(r(9.))
                            .py(r(3.))
                            .rounded_full()
                            .bg(ink.sheet)
                            .opacity(span(at, 0.78, 0.84))
                            .child(Icon::new(Icons::Wifi).size(r(12.)).text_color(ink.good))
                            .child(words(
                                "Shared on your network",
                                11.,
                                FontWeight::MEDIUM,
                                ink.ink,
                            )),
                    ),
            )
    })
}

// ---- 7. Studio ------------------------------------------------------------

/// The user asks, and the agent does it on the screenshot: the email is
/// blurred, the button boxed, pointed at and numbered. Under it a
/// recording: a part sped up, a part cut out, and the file brought under
/// a size.
pub(crate) fn studio(cx: &App) -> AnyElement {
    let ink = Ink::of(cx);
    playing("tour-studio", 11., &ink, move |at| {
        let (blurring, drawing, numbering) = (
            during(at, 0.14, 0.26),
            during(at, 0.26, 0.38),
            during(at, 0.38, 0.44),
        );
        let done = at >= 0.44;
        let doing = match at {
            at if at < 0.14 => "On it…",
            _ if blurring => "Blurring the email",
            _ if drawing => "Drawing the arrow",
            _ if numbering => "Numbering the step",
            _ => "Done",
        };
        let chat = sheet(22., 20., 168., 172., &ink)
            .flex()
            .flex_col()
            .gap(r(9.))
            .p(r(12.))
            .child(
                h_flex().justify_end().child(
                    div()
                        .max_w(r(134.))
                        .px(r(9.))
                        .py(r(6.))
                        .rounded(r(9.))
                        .bg(ink.primary.opacity(0.16))
                        .opacity(span(at, 0.02, 0.07))
                        .child(words(
                            "Blur the email, add an arrow",
                            10.5,
                            FontWeight::MEDIUM,
                            ink.ink,
                        )),
                ),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap(r(6.))
                    .opacity(span(at, 0.08, 0.13))
                    .child(if done {
                        tick(14., &ink)
                    } else {
                        div().child(
                            Icon::new(Icons::Sparkles)
                                .size(r(13.))
                                .text_color(ink.primary),
                        )
                    })
                    .child(words(doing, 10.5, FontWeight::MEDIUM, ink.ink)),
            );
        // The email, readable until it is blurred.
        let blurred = span(at, 0.16, 0.24);
        let email = h_flex()
            .relative()
            .h(r(14.))
            .child(
                words("ada@example.com", 10.5, FontWeight::NORMAL, ink.ink).opacity(1. - blurred),
            )
            .child(
                h_flex()
                    .absolute()
                    .left_0()
                    .top(r(2.))
                    .gap(r(2.))
                    .opacity(blurred)
                    .children((0..11).map(|place| {
                        div().size(r(9.)).bg(ink.faint.opacity(if place % 2 == 0 {
                            0.5
                        } else {
                            0.28
                        }))
                    })),
            );
        let boxed = span(at, 0.27, 0.31);
        let arrow = span(at, 0.3, 0.38);
        let numbered = span(at, 0.38, 0.42);
        let picture = sheet(202., 20., 304., 172., &ink)
            .flex()
            .flex_col()
            .child(bar(None, &ink))
            .child(
                v_flex()
                    .relative()
                    .flex_1()
                    .p(r(14.))
                    .gap(r(8.))
                    .child(line(110., 8., ink.ink.opacity(0.8)))
                    .child(
                        h_flex()
                            .gap(r(7.))
                            .child(words("Email", 10.5, FontWeight::NORMAL, ink.faint))
                            .child(email),
                    )
                    .child(line(150., 5., ink.faint.opacity(0.5)))
                    .child(div().flex_1())
                    .child(
                        h_flex().justify_end().child(
                            div()
                                .w(r(72.))
                                .h(r(24.))
                                .rounded(r(6.))
                                .bg(ink.primary)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(words("Pay", 10.5, FontWeight::SEMIBOLD, ink.on_primary)),
                        ),
                    )
                    // The button, boxed, pointed at and numbered.
                    .child(
                        div()
                            .absolute()
                            .right(r(8.))
                            .bottom(r(8.))
                            .w(r(84.))
                            .h(r(36.))
                            .rounded(r(8.))
                            .border_2()
                            .border_color(ink.bad.opacity(boxed)),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(r(mix(128., 96., arrow)))
                            .bottom(r(mix(8., 28., arrow)))
                            .opacity(arrow)
                            .child(
                                Icon::new(Icons::MoveUpRight)
                                    .size(r(34.))
                                    .text_color(ink.bad),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(r(0.))
                            .bottom(r(36.))
                            .size(r(20.))
                            .rounded_full()
                            .bg(ink.bad)
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(numbered)
                            .child(words("1", 11., FontWeight::BOLD, ink.sheet)),
                    ),
            );
        // The tools, the one in use lit.
        let tool = |icon: Icons, lit: bool| {
            div()
                .size(r(34.))
                .rounded(r(9.))
                .bg(if lit { ink.primary } else { ink.soft })
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(icon).size(r(17.)).text_color(if lit {
                    ink.on_primary
                } else {
                    ink.ink
                }))
        };
        let tools = sheet(518., 20., 72., 172., &ink)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(r(7.))
            .child(tool(Icons::EyeOff, blurring))
            .child(tool(Icons::MoveUpRight, drawing))
            .child(tool(Icons::CircleDot, numbering))
            .child(tool(Icons::Crop, false));
        // The recording: where it plays, what is sped up, what is cut, and
        // what most recordings are opened for: the file made to fit a size.
        let played = span(at, 0.46, 0.76);
        let (fast, cut) = (at >= 0.54, span(at, 0.62, 0.68));
        let limit = span(at, 0.76, 0.8);
        let fitted = span(at, 0.8, 0.89);
        let weighs = mix(48., 9., fitted);
        let track = 310.;
        let timeline =
            sheet(22., 204., 568., 76., &ink)
                .flex()
                .items_center()
                .gap(r(12.))
                .px(r(14.))
                .child(Icon::new(Icons::Video).size(r(18.)).text_color(ink.ink))
                .child(
                    h_flex()
                        .relative()
                        .w(r(track))
                        .h(r(26.))
                        .gap(r(4.))
                        .child(
                            h_flex()
                                .w(r(90.))
                                .h_full()
                                .gap(r(2.))
                                .rounded(r(5.))
                                .overflow_hidden()
                                .children((0..4).map(|place| {
                                    div().flex_1().h_full().bg(ink
                                        .blue
                                        .opacity(if place % 2 == 0 { 0.3 } else { 0.2 }))
                                })),
                        )
                        .child(
                            div()
                                .w(r(80.))
                                .h_full()
                                .rounded(r(5.))
                                .bg(if fast {
                                    ink.primary.opacity(0.22)
                                } else {
                                    ink.soft
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(fast, |this| {
                                    this.child(words(
                                        "2× faster",
                                        10.5,
                                        FontWeight::SEMIBOLD,
                                        ink.primary,
                                    ))
                                }),
                        )
                        // A part taken out: it closes up as it goes.
                        .child(
                            div()
                                .w(r(mix(52., 16., cut)))
                                .h_full()
                                .rounded(r(5.))
                                .bg(if cut > 0. {
                                    ink.bad.opacity(0.16)
                                } else {
                                    ink.soft
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(cut > 0., |this| {
                                    this.child(
                                        Icon::new(Icons::Scissors).size(r(12.)).text_color(ink.bad),
                                    )
                                }),
                        )
                        .child(
                            h_flex()
                                .flex_1()
                                .h_full()
                                .gap(r(2.))
                                .rounded(r(5.))
                                .overflow_hidden()
                                .children((0..4).map(|place| {
                                    div().flex_1().h_full().bg(ink
                                        .magenta
                                        .opacity(if place % 2 == 0 { 0.3 } else { 0.2 }))
                                })),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(r(mix(0., track - 2., played)))
                                .top(r(-5.))
                                .w(r(2.))
                                .h(r(36.))
                                .rounded_full()
                                .bg(ink.ink),
                        ),
                )
                .child(div().flex_1())
                .child(
                    v_flex()
                        .items_end()
                        .gap(r(6.))
                        .child(
                            h_flex()
                                .gap(r(5.))
                                .child(
                                    pill("At most 10 MB", ink.primary.opacity(0.18), ink.primary)
                                        .opacity(limit.max(0.3)),
                                )
                                .child(pill("MP4", ink.soft, ink.ink)),
                        )
                        .child(
                            h_flex()
                                .gap(r(6.))
                                .child(
                                    words("48 MB", 11., FontWeight::NORMAL, ink.faint)
                                        .when(fitted > 0., |this| this.line_through()),
                                )
                                .when(fitted > 0., |this| {
                                    this.child(
                                        Icon::new(IconName::ArrowRight)
                                            .size(r(12.))
                                            .text_color(ink.faint),
                                    )
                                    .child(words(
                                        format!("{weighs:.0} MB"),
                                        13.,
                                        FontWeight::BOLD,
                                        if fitted >= 1. { ink.good } else { ink.ink },
                                    ))
                                }),
                        ),
                );
        ground(ink.magenta, ink.cyan, ink.blue, &ink)
            .child(chat)
            .child(picture)
            .child(tools)
            .child(timeline)
    })
}

// ---- 8. Every day ---------------------------------------------------------

/// The search being typed in and finding something, then four small
/// things, each shown at work for a moment.
pub(crate) fn everyday(cx: &App) -> AnyElement {
    let ink = Ink::of(cx);
    playing("tour-everyday", 10., &ink, move |at| {
        let keys = if cfg!(target_os = "macos") {
            "⌘K"
        } else {
            "Ctrl K"
        };
        // Typed a letter at a time.
        let asked = "sched";
        let typed = (span(at, 0.04, 0.18) * asked.len() as f32).round() as usize;
        let typing: String = asked.chars().take(typed).collect();
        let search = sheet(96., 22., 420., 48., &ink)
            .flex()
            .items_center()
            .gap(r(11.))
            .px(r(16.))
            .child(
                Icon::new(IconName::Search)
                    .size(r(16.))
                    .text_color(ink.faint),
            )
            .child(if typing.is_empty() {
                words(
                    "Search or run a command",
                    12.5,
                    FontWeight::NORMAL,
                    ink.faint,
                )
                .flex_1()
            } else {
                words(typing, 12.5, FontWeight::MEDIUM, ink.ink).flex_1()
            })
            .child(
                div()
                    .px(r(8.))
                    .py(r(3.))
                    .rounded(r(6.))
                    .bg(ink.soft)
                    .child(words(keys, 11.5, FontWeight::SEMIBOLD, ink.ink)),
            );
        // What it found.
        let found = sheet(96., 76., 420., 40., &ink)
            .opacity(span(at, 0.18, 0.24))
            .flex()
            .items_center()
            .gap(r(9.))
            .px(r(16.))
            .child(
                Icon::new(Icons::CalendarClock)
                    .size(r(14.))
                    .text_color(ink.primary),
            )
            .child(words("Schedules", 11.5, FontWeight::SEMIBOLD, ink.ink))
            .child(words(
                "run an agent at set times",
                11.,
                FontWeight::NORMAL,
                ink.faint,
            ));
        let (tile, gap) = (132., 12.);
        let left = (WIDTH - (4. * tile + 3. * gap)) / 2.;
        let small = |place: f32, icon: Icons, color: Hsla, name: &'static str, shows: Div| {
            sheet(left + place * (tile + gap), 132., tile, 146., &ink)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(r(9.))
                .child(spot(Icon::new(icon), 42., color))
                .child(words(name, 12., FontWeight::SEMIBOLD, ink.ink))
                .child(h_flex().h(r(22.)).items_center().child(shows))
        };
        // Quick chat: a question asked over whatever is in front.
        let asked_at = span(at, 0.34, 0.4);
        let quick = pill("How do I undo a commit?", ink.blue.opacity(0.18), ink.ink)
            .text_size(r(8.5))
            .opacity(asked_at);
        // A schedule: every morning.
        let schedule = pill("Every day · 9:00", ink.magenta.opacity(0.18), ink.ink)
            .text_size(r(9.5))
            .opacity(span(at, 0.46, 0.52));
        // Themes: the same app in other colours, one after another.
        let theme = ((at - 0.58).max(0.) * 14.) as usize % 3;
        let swatch = |place: usize, color: Hsla| {
            div()
                .size(r(16.))
                .rounded_full()
                .bg(color)
                .border_2()
                .border_color(if at >= 0.58 && place == theme {
                    ink.ink
                } else {
                    ink.sheet
                })
        };
        let themes = h_flex()
            .gap(r(5.))
            .child(swatch(0, ink.blue))
            .child(swatch(1, ink.green))
            .child(swatch(2, ink.magenta));
        // Usage: how much of a limit is gone.
        let used = span(at, 0.7, 0.84) * 0.62;
        let usage = h_flex()
            .gap(r(6.))
            .child(
                div().w(r(62.)).h(r(5.)).rounded_full().bg(ink.soft).child(
                    div()
                        .h_full()
                        .rounded_full()
                        .w(gpui_kit::relative(used.max(0.03)))
                        .bg(ink.cyan),
                ),
            )
            .child(words(
                format!("{:.0}%", used * 100.),
                10.,
                FontWeight::MEDIUM,
                ink.ink,
            ));
        ground(ink.blue, ink.magenta, ink.cyan, &ink)
            .child(small(
                0.,
                Icons::MessageSquare,
                ink.blue,
                "Quick chat",
                quick,
            ))
            .child(small(
                1.,
                Icons::CalendarClock,
                ink.magenta,
                "Schedules",
                schedule,
            ))
            .child(small(2., Icons::Palette, ink.green, "Themes", themes))
            .child(small(3., Icons::Gauge, ink.cyan, "Usage", usage))
            .child(search)
            .child(found)
    })
}
