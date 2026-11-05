//! Presenting: the slides full screen, one at a time (hidden ones left
//! out), with the arrow keys, space or a click going on; with a second
//! screen, the presenter's view on this one: the slide, the next, the
//! notes and the time.
//!
//! The slide shown and the next are drawn at the screen's size before they
//! are needed, so going on is instant; those passed are let go.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::{
    ActiveTheme as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, MouseButton, ObjectFit, ParentElement as _, Render, RenderImage,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Subscription, Task, Window,
    WindowBounds, WindowOptions, div, img, px,
};
use ph_ui::Icons;

use crate::slides::document::{DeckDocument, Purpose};
use crate::slides::model::{Direction, Effect, Slide, Start, Transition};

const CONTEXT: &str = "SlidesPresent";

gpui_kit::actions!(slides_present, [Next, Previous, First, Last, Close, Black]);

pub(crate) fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    let mut bindings = Vec::new();
    for key in ["right", "down", "space", "pagedown", "enter", "n"] {
        bindings.push(gpui_kit::KeyBinding::new(key, Next, context));
    }
    for key in ["left", "up", "pageup", "backspace", "p"] {
        bindings.push(gpui_kit::KeyBinding::new(key, Previous, context));
    }
    bindings.extend([
        gpui_kit::KeyBinding::new(ph_ui::keys("home"), First, context),
        gpui_kit::KeyBinding::new(ph_ui::keys("end"), Last, context),
        gpui_kit::KeyBinding::new(ph_ui::keys("escape"), Close, context),
        gpui_kit::KeyBinding::new(ph_ui::keys("b"), Black, context),
        gpui_kit::KeyBinding::new(ph_ui::keys("."), Black, context),
    ]);
    cx.bind_keys(bindings);
}

/// One element coming in: when (seconds after its build starts) and how.
#[derive(Debug, Clone, PartialEq)]
struct Entrance {
    id: String,
    effect: Effect,
    from: Direction,
    at: f32,
    seconds: f32,
}

/// Elements that come in together: on a click, or (the first build) on
/// their own as the slide shows.
#[derive(Debug, Clone, PartialEq)]
struct Build {
    auto: bool,
    entrances: Vec<Entrance>,
}

impl Build {
    fn seconds(&self) -> f32 {
        self.entrances
            .iter()
            .map(|entrance| entrance.at + entrance.seconds)
            .fold(0., f32::max)
    }
}

/// A slide's builds, in order.
fn builds(slide: &Slide) -> Vec<Build> {
    let mut builds: Vec<Build> = Vec::new();
    // When the one before was set going, and when it ended.
    let (mut trigger, mut end) = (0f32, 0f32);
    for animation in &slide.animations {
        if !slide
            .elements
            .iter()
            .any(|element| element.id == animation.id)
        {
            continue;
        }
        if animation.start == Start::OnClick || builds.is_empty() {
            builds.push(Build {
                auto: animation.start != Start::OnClick,
                entrances: Vec::new(),
            });
            (trigger, end) = (0., 0.);
        }
        trigger = match animation.start {
            Start::OnClick => 0.,
            Start::WithPrevious => trigger,
            Start::AfterPrevious => end,
        };
        let at = trigger + animation.delay.max(0.);
        let seconds = if animation.effect == Effect::Appear {
            0.
        } else {
            animation.seconds.clamp(0.05, 30.)
        };
        end = at + seconds;
        if let Some(build) = builds.last_mut() {
            build.entrances.push(Entrance {
                id: animation.id.clone(),
                effect: animation.effect,
                from: animation.from,
                at,
                seconds,
            });
        }
    }
    builds
}

/// The elements not come in yet after `step` builds.
fn hidden(builds: &[Build], step: usize) -> Vec<String> {
    builds
        .iter()
        .skip(step)
        .flat_map(|build| build.entrances.iter().map(|entrance| entrance.id.clone()))
        .collect()
}

/// How far `seconds` from `start` is along (0 to 1), and whether it has
/// begun.
fn progress(start: Instant, at: f32, seconds: f32) -> Option<f32> {
    let elapsed = Instant::now()
        .saturating_duration_since(start)
        .as_secs_f32()
        - at;
    if elapsed < 0. {
        return None;
    }
    Some(if seconds <= 0. {
        1.
    } else {
        (elapsed / seconds).min(1.)
    })
}

/// Fast, then settling.
fn ease(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// Where something coming from `from` starts, as a share of the way
/// across (x, y).
fn unit(from: Direction) -> (f32, f32) {
    match from {
        Direction::Left => (-1., 0.),
        Direction::Right => (1., 0.),
        Direction::Top => (0., -1.),
        Direction::Bottom => (0., 1.),
    }
}

/// The slide going away as the next comes in.
struct Moving {
    image: Arc<RenderImage>,
    kind: Transition,
    from: Direction,
    seconds: f32,
    started: Instant,
}

/// The show both windows follow.
pub(crate) struct Show {
    doc: Entity<DeckDocument>,
    /// Where each slide shown is in the deck, by id (hidden ones out).
    order: Vec<String>,
    at: usize,
    /// The builds of the slide shown played so far.
    step: usize,
    /// The build playing and when it began (or begins).
    playing: Option<(usize, Instant)>,
    moving: Option<Moving>,
    black: bool,
    /// Past the last slide: the end.
    ended: bool,
    started: Instant,
    /// The audience's screen, pixels wide.
    width: u32,
    /// The presenter's view alone, in a window (no transitions or
    /// animations played).
    rehearsing: bool,
    windows: Vec<gpui_kit::AnyWindowHandle>,
    _watch: Subscription,
}

impl Show {
    fn slide_index(&self, cx: &App, offset: usize) -> Option<usize> {
        let id = self.order.get(self.at + offset)?;
        self.doc
            .read(cx)
            .deck()
            .slides
            .iter()
            .position(|slide| &slide.id == id)
    }

    fn builds(&self, cx: &App, offset: usize) -> Vec<Build> {
        self.slide_index(cx, offset)
            .map(|index| builds(&self.doc.read(cx).deck().slides[index]))
            .unwrap_or_default()
    }

    /// The slide `offset` on after `step` builds, at the audience's size.
    fn picture_at(
        &mut self,
        offset: usize,
        step: usize,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        let index = self.slide_index(cx, offset)?;
        let hidden = hidden(&self.builds(cx, offset), step);
        let width = self.width;
        self.doc.update(cx, |doc, cx| {
            if hidden.is_empty() {
                doc.picture(index, Purpose::Present, width, None, cx)
            } else {
                doc.step_picture(
                    index,
                    step.min(u16::MAX as usize) as u16,
                    width,
                    &hidden,
                    cx,
                )
            }
        })
    }

    /// The slide shown, as it is now.
    fn picture(&mut self, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let step = self.step;
        self.picture_at(0, step, cx)
    }

    fn next_picture(&mut self, width: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let index = self.slide_index(cx, 1)?;
        self.doc.update(cx, |doc, cx| {
            doc.picture(index, Purpose::Next, width, None, cx)
        })
    }

    /// Makes what comes next before it is needed: the next build's
    /// elements and the slide after it, and the next slide.
    fn prepare(&mut self, cx: &mut Context<Self>) {
        let _ = self.picture_at(1, 0, cx);
        let builds = self.builds(cx, 0);
        let coming = self.playing.map_or(self.step, |(build, _)| build);
        let Some(build) = builds.get(coming) else {
            return;
        };
        let _ = self.picture_at(0, coming + 1, cx);
        if self.rehearsing {
            return;
        }
        let Some(index) = self.slide_index(cx, 0) else {
            return;
        };
        let scale = self.width as f32 / self.doc.read(cx).deck().size.width.max(1.);
        for entrance in &build.entrances {
            let _ = self
                .doc
                .update(cx, |doc, cx| doc.layer(index, &entrance.id, scale, cx));
        }
    }

    /// Plays build `build` from `delay` seconds on.
    fn play(&mut self, build: usize, delay: f32) {
        if self.rehearsing {
            self.step = build + 1;
        } else {
            self.playing = Some((
                build,
                Instant::now() + Duration::from_secs_f32(delay.max(0.)),
            ));
        }
    }

    /// On: the build playing ends at once, else the next plays, else the
    /// next slide.
    fn forward(&mut self, cx: &mut Context<Self>) {
        if self.ended {
            return;
        }
        if let Some((build, _)) = self.playing.take() {
            self.step = build + 1;
            self.moving = None;
            self.keep(cx);
            cx.notify();
            return;
        }
        if self.moving.take().is_some() {
            cx.notify();
            return;
        }
        if self.step < self.builds(cx, 0).len() {
            let step = self.step;
            self.play(step, 0.);
            self.keep(cx);
            cx.notify();
            return;
        }
        let to = self.at as isize + 1;
        self.go(to, true, cx);
    }

    /// Back: the build before undone, else the slide before as it ended.
    fn back(&mut self, cx: &mut Context<Self>) {
        if self.ended {
            let at = self.at as isize;
            self.go(at, false, cx);
            return;
        }
        self.moving = None;
        if self.playing.take().is_some() {
            cx.notify();
            return;
        }
        let builds = self.builds(cx, 0);
        let first = usize::from(builds.first().is_some_and(|build| build.auto));
        if self.step > first {
            self.step -= 1;
            self.keep(cx);
            cx.notify();
            return;
        }
        let to = self.at as isize - 1;
        self.go(to, false, cx);
    }

    /// To slide `to`: `fresh`, from its start (its transition and first
    /// build playing); else as it ended.
    fn go(&mut self, to: isize, fresh: bool, cx: &mut Context<Self>) {
        let count = self.order.len() as isize;
        if count == 0 {
            return;
        }
        let before = self.picture(cx);
        self.black = false;
        if to >= count {
            self.ended = true;
            self.playing = None;
            self.moving = None;
        } else {
            let to = to.clamp(0, count - 1) as usize;
            let was_ended = std::mem::replace(&mut self.ended, false);
            if to == self.at && (!fresh || was_ended) {
                self.keep(cx);
                cx.notify();
                return;
            }
            let forward = to > self.at;
            self.at = to;
            self.playing = None;
            self.moving = None;
            let builds = self.builds(cx, 0);
            if fresh {
                self.step = 0;
                let mut delay = 0.;
                if forward
                    && !self.rehearsing
                    && let Some(index) = self.slide_index(cx, 0)
                    && let Some(image) = before
                {
                    let slide = &self.doc.read(cx).deck().slides[index];
                    if slide.transition != Transition::None {
                        delay = slide.transition_seconds.clamp(0.1, 10.);
                        self.moving = Some(Moving {
                            image,
                            kind: slide.transition,
                            from: slide.transition_from,
                            seconds: delay,
                            started: Instant::now(),
                        });
                    }
                }
                if builds.first().is_some_and(|build| build.auto) {
                    self.play(0, delay);
                }
            } else {
                self.step = builds.len();
            }
        }
        self.keep(cx);
        cx.notify();
    }

    /// Keeps the pictures of the slide shown (its builds either side) and
    /// the next; the others are let go.
    fn keep(&mut self, cx: &mut Context<Self>) {
        let current = self.order.get(self.at).cloned().unwrap_or_default();
        let next = self.order.get(self.at + 1).cloned().unwrap_or_default();
        let step = self.step as u16;
        self.doc.update(cx, |doc, _| {
            doc.keep_presenting(|id, purpose| match purpose {
                Purpose::Step(at) => {
                    (id == current && at + 1 >= step && at <= step + 2) || (id == next && at == 0)
                }
                _ => id == current || id == next,
            })
        });
    }

    /// Frees the pictures let go on the GPU, in every window (the editor's
    /// too: it draws the same deck).
    fn drop_stale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stale = self.doc.update(cx, |doc, _| doc.take_stale());
        for image in stale {
            cx.drop_image(image, Some(window));
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        let windows = std::mem::take(&mut self.windows);
        // Later: this may run inside one of those windows' own updates.
        cx.defer(move |cx| {
            for window in windows {
                _ = window.update(cx, |_, window, _| window.remove_window());
            }
        });
        self.doc.update(cx, |doc, _| doc.forget_presenting());
    }
}

/// The show of `doc` from slide `start` (the first shown at or after it).
fn new_show(
    doc: Entity<DeckDocument>,
    start: usize,
    width: u32,
    rehearsing: bool,
    cx: &mut App,
) -> Option<Entity<Show>> {
    let deck = doc.read(cx).deck();
    let order: Vec<String> = deck
        .slides
        .iter()
        .filter(|slide| !slide.hidden)
        .map(|slide| slide.id.clone())
        .collect();
    if order.is_empty() {
        return None;
    }
    let at = deck
        .slides
        .iter()
        .skip(start)
        .find(|slide| !slide.hidden)
        .and_then(|slide| order.iter().position(|id| *id == slide.id))
        .unwrap_or(0);
    let watch_doc = doc.clone();
    let show = cx.new(|cx: &mut Context<Show>| {
        let mut show = Show {
            doc: doc.clone(),
            order,
            at,
            step: 0,
            playing: None,
            moving: None,
            black: false,
            ended: false,
            started: Instant::now(),
            width,
            rehearsing,
            windows: Vec::new(),
            _watch: cx.observe(&watch_doc, |show: &mut Show, doc, cx| {
                // Slides changed meanwhile (an agent at work): the order
                // follows.
                let deck = doc.read(cx).deck();
                let current = show.order.get(show.at).cloned();
                show.order = deck
                    .slides
                    .iter()
                    .filter(|slide| !slide.hidden)
                    .map(|slide| slide.id.clone())
                    .collect();
                if let Some(at) =
                    current.and_then(|id| show.order.iter().position(|known| *known == id))
                {
                    show.at = at;
                }
                show.at = show.at.min(show.order.len().saturating_sub(1));
                show.step = show.step.min(show.builds(cx, 0).len());
                cx.notify();
            }),
        };
        if show.builds(cx, 0).first().is_some_and(|build| build.auto) {
            show.play(0, 0.);
        }
        show
    });
    Some(show)
}

/// Presents `doc` from slide `start` (the first shown at or after it).
pub fn present(doc: Entity<DeckDocument>, start: usize, cx: &mut App) {
    let displays = cx.displays();
    let primary = cx.primary_display();
    // The audience gets another screen when there is one.
    let audience = displays
        .iter()
        .find(|display| Some(display.id()) != primary.as_ref().map(|p| p.id()))
        .cloned()
        .or_else(|| primary.clone());
    let Some(audience) = audience else {
        return;
    };
    let presenter = primary.filter(|primary| primary.id() != audience.id());
    let bounds = audience.bounds();
    let width = (f32::from(bounds.size.width) * 2.).round() as u32;
    let Some(show) = new_show(doc, start, width, false, cx) else {
        return;
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Fullscreen(bounds)),
        display_id: Some(audience.id()),
        titlebar: None,
        focus: true,
        show: true,
        ..WindowOptions::default()
    };
    let audience_window: Option<gpui_kit::AnyWindowHandle> =
        gpui_kit::open_window(options, cx, |window, cx| {
            let view = cx.new(|cx| Audience::new(show.clone(), window, cx));
            show.update(cx, |show, _| {
                show.width = (f32::from(bounds.size.width) * window.scale_factor()).round() as u32
            });
            view
        })
        .ok()
        .map(|(handle, _)| handle);
    let mut windows: Vec<gpui_kit::AnyWindowHandle> = audience_window.into_iter().collect();
    if let Some(presenter) = presenter {
        let pbounds = presenter.bounds();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Fullscreen(pbounds)),
            display_id: Some(presenter.id()),
            titlebar: None,
            focus: false,
            show: true,
            ..WindowOptions::default()
        };
        if let Ok((handle, _)) = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Presenter::new(show.clone(), window, cx))
        }) {
            windows.push(handle);
        }
    }
    show.update(cx, |show, cx| {
        show.windows = windows;
        show.keep(cx);
    });
}

/// Rehearses `doc` from slide `start`: the presenter's view in a window
/// on this screen (the slide, the next, the notes and the time).
pub fn rehearse(doc: Entity<DeckDocument>, start: usize, cx: &mut App) {
    let screen = cx
        .primary_display()
        .map(|display| display.bounds().size)
        .unwrap_or(gpui_kit::Size::new(px(1440.), px(900.)));
    let size = gpui_kit::Size::new(
        px(1200.).min(screen.width * 0.94),
        px(760.).min(screen.height * 0.9),
    );
    let Some(show) = new_show(doc, start, 1280, true, cx) else {
        return;
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(gpui_kit::Bounds::centered(
            None, size, cx,
        ))),
        window_min_size: Some(gpui_kit::Size::new(px(760.), px(480.))),
        titlebar: Some(gpui_kit::TitlebarOptions {
            title: Some("Rehearsal".into()),
            ..Default::default()
        }),
        focus: true,
        show: true,
        ..WindowOptions::default()
    };
    let Ok((handle, _)) = gpui_kit::open_window(options, cx, |window, cx| {
        let scale = window.scale_factor();
        show.update(cx, |show, _| show.width = (640. * scale).round() as u32);
        cx.new(|cx| Presenter::new(show.clone(), window, cx))
    }) else {
        return;
    };
    show.update(cx, |show, cx| {
        show.windows = vec![handle];
        show.keep(cx);
    });
}

/// What both windows answer to.
fn on_keys(element: gpui_kit::Div, show: &Entity<Show>) -> gpui_kit::Div {
    let (next, previous, first, last, close, black) = (
        show.clone(),
        show.clone(),
        show.clone(),
        show.clone(),
        show.clone(),
        show.clone(),
    );
    element
        .key_context(CONTEXT)
        .on_action(move |_: &Next, _, cx| next.update(cx, |show, cx| show.forward(cx)))
        .on_action(move |_: &Previous, _, cx| previous.update(cx, |show, cx| show.back(cx)))
        .on_action(move |_: &First, _, cx| first.update(cx, |show, cx| show.go(0, true, cx)))
        .on_action(move |_: &Last, _, cx| {
            last.update(cx, |show, cx| {
                let to = show.order.len() as isize - 1;
                show.go(to, true, cx)
            })
        })
        .on_action(move |_: &Close, _, cx| close.update(cx, |show, cx| show.close(cx)))
        .on_action(move |_: &Black, _, cx| {
            black.update(cx, |show, cx| {
                show.black = !show.black;
                cx.notify();
            })
        })
}

/// The audience's window: the slide, full screen.
struct Audience {
    show: Entity<Show>,
    focus: FocusHandle,
    /// The last picture shown, kept while the next is made.
    shown: Option<Arc<RenderImage>>,
    _watch: Subscription,
}

impl Audience {
    fn new(show: Entity<Show>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let watch = cx.observe(&show, |_, _, cx| cx.notify());
        let doc = show.read(cx).doc.clone();
        cx.observe(&doc, |_, _, cx| cx.notify()).detach();
        // Closing this window ends the show.
        let ending = show.clone();
        window.on_window_should_close(cx, move |_, cx| {
            ending.update(cx, |show, cx| show.close(cx));
            true
        });
        Audience {
            show,
            focus,
            shown: None,
            _watch: watch,
        }
    }
}

impl Focusable for Audience {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// An image at `[x, y, w, h]` (pixels) in its parent.
fn placed(image: Arc<RenderImage>, [x, y, w, h]: [f32; 4]) -> gpui_kit::Img {
    img(image)
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(w))
        .h(px(h))
        .object_fit(ObjectFit::Fill)
}

/// `image` (filling `[x, y, w, h]`) shown only on the part reached by a
/// wipe from `from`, `t` of the way.
fn wiped(
    image: Arc<RenderImage>,
    [x, y, w, h]: [f32; 4],
    from: Direction,
    t: f32,
) -> gpui_kit::Div {
    let (cx, cy, cw, ch) = match from {
        Direction::Left => (0., 0., w * t, h),
        Direction::Right => (w * (1. - t), 0., w * t, h),
        Direction::Top => (0., 0., w, h * t),
        Direction::Bottom => (0., h * (1. - t), w, h * t),
    };
    div()
        .absolute()
        .left(px(x + cx))
        .top(px(y + cy))
        .w(px(cw))
        .h(px(ch))
        .overflow_hidden()
        .child(placed(image, [-cx, -cy, w, h]))
}

impl Render for Audience {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show = self.show.clone();
        show.update(cx, |show, cx| show.drop_stale(window, cx));
        let (black, ended, playing, step) = {
            let show = show.read(cx);
            (show.black, show.ended, show.playing, show.step)
        };
        let builds = show.read(cx).builds(cx, 0);
        // A build ended: its slide as it is now, when made (until then
        // its elements stay drawn where they arrived).
        let mut playing = playing.and_then(|(build, start)| {
            let entrance = builds.get(build)?;
            Some((build, start, entrance.clone()))
        });
        if let Some((build, start, entrances)) = &playing
            && progress(*start, entrances.seconds(), 0.).is_some()
            && let Some(after) = show.update(cx, |show, cx| show.picture_at(0, build + 1, cx))
        {
            let build = *build;
            self.shown = Some(after);
            show.update(cx, |show, cx| {
                show.playing = None;
                show.step = build + 1;
                show.keep(cx);
            });
            playing = None;
        }
        let base_step = playing.as_ref().map_or(step, |(build, _, _)| *build);
        let picture = show.update(cx, |show, cx| {
            let picture = show.picture_at(0, base_step, cx);
            show.prepare(cx);
            let _ = show.next_picture(320, cx);
            picture
        });
        if let Some(picture) = &picture {
            self.shown = Some(picture.clone());
        }
        let (size, deck_size) = {
            let deck = show.read(cx).doc.read(cx).deck();
            (
                window.viewport_size(),
                (deck.size.width.max(1.), deck.size.height.max(1.)),
            )
        };
        let aspect = deck_size.0 / deck_size.1;
        let (vw, vh) = (f32::from(size.width), f32::from(size.height));
        let (w, h) = if vw / vh > aspect {
            (vh * aspect, vh)
        } else {
            (vw, vw / aspect)
        };
        // The slide coming in over the one going.
        let moving = show.update(cx, |show, _| {
            let done = show
                .moving
                .as_ref()
                .is_some_and(|moving| progress(moving.started, 0., moving.seconds) == Some(1.));
            if done {
                show.moving = None;
            }
            show.moving.as_ref().map(|moving| {
                let t = progress(moving.started, 0., moving.seconds).unwrap_or(0.);
                (moving.image.clone(), moving.kind, moving.from, t)
            })
        });
        let mut animating = moving.is_some();
        let mut slide = div().relative().w(px(w)).h(px(h)).overflow_hidden();
        let full = [0., 0., w, h];
        match (moving, self.shown.clone()) {
            (Some((old, kind, from, t)), Some(new)) => {
                let e = ease(t);
                let (ux, uy) = unit(from);
                let coming = [ux * w * (1. - e), uy * h * (1. - e), w, h];
                slide = match kind {
                    Transition::Push => slide
                        .child(placed(old, [coming[0] - ux * w, coming[1] - uy * h, w, h]))
                        .child(placed(new, coming)),
                    Transition::Cover => slide.child(placed(old, full)).child(placed(new, coming)),
                    Transition::Wipe => slide
                        .child(placed(old, full))
                        .child(wiped(new, full, from, e)),
                    Transition::Zoom => {
                        let scale = 0.3 + 0.7 * e;
                        let (zw, zh) = (w * scale, h * scale);
                        slide
                            .child(placed(old, full))
                            .child(placed(new, [(w - zw) / 2., (h - zh) / 2., zw, zh]).opacity(t))
                    }
                    // Fade and dissolve.
                    _ => slide
                        .child(placed(old, full))
                        .child(placed(new, full).opacity(t)),
                };
            }
            (_, Some(new)) => slide = slide.child(placed(new, full)),
            _ => {}
        }
        // The build playing: its elements coming in, each its own way.
        if let Some((_, start, build)) = &playing {
            animating = true;
            let index = show.read(cx).slide_index(cx, 0);
            let points = w / deck_size.0;
            let pixels = show.read(cx).width as f32 / deck_size.0;
            for entrance in &build.entrances {
                let Some(t) = progress(*start, entrance.at, entrance.seconds) else {
                    continue;
                };
                let layer = index.and_then(|index| {
                    show.read(cx)
                        .doc
                        .clone()
                        .update(cx, |doc, cx| doc.layer(index, &entrance.id, pixels, cx))
                });
                let Some((image, [x, y, lw, lh])) = layer else {
                    continue;
                };
                let e = ease(t);
                let at = [x * points, y * points, lw * points, lh * points];
                slide = slide.child(match entrance.effect {
                    Effect::Appear => placed(image, at).into_any_element(),
                    Effect::Fade => placed(image, at).opacity(t).into_any_element(),
                    Effect::FlyIn => {
                        let (dx, dy) = match entrance.from {
                            Direction::Left => (-(x + lw), 0.),
                            Direction::Right => (deck_size.0 - x, 0.),
                            Direction::Top => (0., -(y + lh)),
                            Direction::Bottom => (0., deck_size.1 - y),
                        };
                        let off = 1. - e;
                        placed(
                            image,
                            [
                                at[0] + dx * off * points,
                                at[1] + dy * off * points,
                                at[2],
                                at[3],
                            ],
                        )
                        .into_any_element()
                    }
                    Effect::Zoom => {
                        let scale = 0.3 + 0.7 * e;
                        let (zw, zh) = (at[2] * scale, at[3] * scale);
                        placed(
                            image,
                            [at[0] + (at[2] - zw) / 2., at[1] + (at[3] - zh) / 2., zw, zh],
                        )
                        .opacity(t)
                        .into_any_element()
                    }
                    Effect::Wipe => wiped(image, at, entrance.from, e).into_any_element(),
                });
            }
            // Ended (waiting only for the slide's picture): no more frames.
            if progress(*start, build.seconds(), 0.).is_some() {
                animating = false;
            }
        }
        if animating {
            window.request_animation_frame();
        }
        let (next, previous) = (show.clone(), show.clone());
        on_keys(div(), &self.show)
            .track_focus(&self.focus)
            .size_full()
            .bg(gpui_kit::black())
            .flex()
            .items_center()
            .justify_center()
            .cursor(gpui_kit::CursorStyle::Arrow)
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                next.update(cx, |show, cx| show.forward(cx))
            })
            .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                previous.update(cx, |show, cx| show.back(cx))
            })
            .when(!black && !ended, |this| this.child(slide))
            .when(ended, |this| {
                this.child(
                    div()
                        .text_color(gpui_kit::white().opacity(0.6))
                        .text_sm()
                        .child("The end. Press Esc to leave."),
                )
            })
    }
}

/// The presenter's view: the slide, the next, the notes, the time.
struct Presenter {
    show: Entity<Show>,
    focus: FocusHandle,
    _watch: Subscription,
    _tick: Task<()>,
}

impl Presenter {
    fn new(show: Entity<Show>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let watch = cx.observe(&show, |_, _, cx| cx.notify());
        // The clock: once a second, nothing more.
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let ending = show.clone();
        window.on_window_should_close(cx, move |_, cx| {
            ending.update(cx, |show, cx| show.close(cx));
            true
        });
        Presenter {
            show,
            focus,
            _watch: watch,
            _tick: tick,
        }
    }
}

impl Focusable for Presenter {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Presenter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let show = self.show.clone();
        let next_width = (360. * window.scale_factor()).round() as u32;
        let (current, next) = show.update(cx, |show, cx| {
            show.drop_stale(window, cx);
            // As it will be once the build playing has come in.
            let after = show.playing.map_or(show.step, |(build, _)| build + 1);
            let current = show.picture_at(0, after, cx).or_else(|| show.picture(cx));
            if show.rehearsing {
                show.prepare(cx);
            }
            (current, show.next_picture(next_width, cx))
        });
        let (at, count, elapsed, notes, aspect, clicks, ended) = {
            let show = show.read(cx);
            let deck = show.doc.read(cx).deck();
            let notes = show
                .slide_index(cx, 0)
                .map(|index| deck.slides[index].notes.clone())
                .unwrap_or_default();
            let builds = show.builds(cx, 0);
            let first = usize::from(builds.first().is_some_and(|build| build.auto));
            let after = show.playing.map_or(show.step, |(build, _)| build + 1);
            let clicks =
                (builds.len() > first).then(|| (after.saturating_sub(first), builds.len() - first));
            (
                show.at,
                show.order.len(),
                show.started.elapsed().as_secs(),
                notes,
                deck.size.width / deck.size.height.max(1.),
                clicks,
                show.ended,
            )
        };
        let viewport = f32::from(window.viewport_size().width);
        // Room for the slide beside the next one and the notes.
        let current_width = (viewport - 48. - 24. - 300.).clamp(320., 960.);
        let next_box = (viewport - 48. - 24. - current_width).clamp(220., 360.);
        let picture = |image: Option<Arc<RenderImage>>, width: f32| {
            div()
                .w(px(width))
                .h(px(width / aspect))
                .flex_shrink_0()
                .bg(gpui_kit::black())
                .border_1()
                .border_color(theme.border)
                .children(image.map(|image| img(image).size_full().object_fit(ObjectFit::Fill)))
        };
        let (go_back, go_on, end) = (show.clone(), show.clone(), show.clone());
        let place = match (ended, clicks) {
            (true, _) => "The end".to_string(),
            (false, Some((done, all))) => {
                format!("Slide {} of {count} · click {done} of {all}", at + 1)
            }
            (false, None) => format!("Slide {} of {count}", at + 1),
        };
        on_keys(div(), &self.show)
            .track_focus(&self.focus)
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .p_6()
            .child(
                v_flex()
                    .size_full()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_4()
                            .child(
                                div()
                                    .text_3xl()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child(format!("{:02}:{:02}", elapsed / 60, elapsed % 60)),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .text_color(theme.muted_foreground)
                                    .child(place),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("present-back")
                                    .outline()
                                    .icon(Icons::ChevronLeft)
                                    .label("Back")
                                    .on_click(move |_, _, cx| {
                                        go_back.update(cx, |show, cx| show.back(cx))
                                    }),
                            )
                            .child(
                                Button::new("present-on")
                                    .primary()
                                    .icon(Icons::ChevronRight)
                                    .label("Next")
                                    .on_click(move |_, _, cx| {
                                        go_on.update(cx, |show, cx| show.forward(cx))
                                    }),
                            )
                            .child(Button::new("present-end").ghost().label("End").on_click(
                                move |_, _, cx| end.update(cx, |show, cx| show.close(cx)),
                            )),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .items_start()
                            .gap_6()
                            .child(picture(current, current_width))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .gap_3()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child("Next"),
                                    )
                                    .child(picture(next, next_box))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child("Notes"),
                                    )
                                    .child(
                                        div()
                                            .id("present-notes")
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .text_xl()
                                            .child(if notes.trim().is_empty() {
                                                "No notes for this slide.".to_string()
                                            } else {
                                                notes
                                            }),
                                    ),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slides::model::{Animation, Element};

    #[test]
    fn builds_group_by_click() {
        let element = |id: &str| Element {
            id: id.into(),
            ..Element::default()
        };
        let animation = |id: &str, start: Start, effect: Effect| Animation {
            id: id.into(),
            start,
            effect,
            seconds: 1.,
            ..Animation::default()
        };
        let slide = Slide {
            elements: vec![element("a"), element("b"), element("c"), element("d")],
            animations: vec![
                animation("a", Start::WithPrevious, Effect::Fade),
                animation("b", Start::OnClick, Effect::FlyIn),
                animation("c", Start::AfterPrevious, Effect::Zoom),
                animation("d", Start::OnClick, Effect::Appear),
                animation("gone", Start::OnClick, Effect::Fade),
            ],
            ..Slide::default()
        };
        let builds = builds(&slide);
        assert_eq!(builds.len(), 3);
        assert!(builds[0].auto && !builds[1].auto);
        assert_eq!(builds[1].entrances[1].at, 1.);
        assert_eq!(builds[1].seconds(), 2.);
        assert_eq!(builds[2].seconds(), 0.);
        assert_eq!(hidden(&builds, 1), vec!["b", "c", "d"]);
        assert!(hidden(&builds, 3).is_empty());
    }
}
