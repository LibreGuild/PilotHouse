//! The tour: a few cards that say what the app has, each with a picture,
//! read with *Next* to the last one's *Done*. There is a lot here that
//! someone who came for a chat with an agent would not go looking for (a
//! phone beside the chat, tests recorded from what an agent did, decks of
//! slides), and a list of features in a document is not read. So the app
//! says it once, at the first start.
//!
//! **Which cards show.** Each card has an id, and the ids a user has gone
//! through are kept (the setting `tour.seen`). At a start, the cards not
//! yet seen show. So:
//!
//! - after an install, all of them (*Welcome*);
//! - after an update that added cards, those alone (*What's new*);
//! - after an update that added none (a fix), nothing.
//!
//! A release says it has something new by adding a card to [`CARDS`], in
//! the place where someone new should meet it; nothing else decides. A card
//! of a feature the build has switched off is left out. Where the welcome
//! would grow too long by it, the card is marked as news: it shows after
//! the update alone, and someone new reads of the thing on another card.
//!
//! **No way round it.** At a start the cards have no close button and Esc
//! does nothing: the way out is the last card's *Done*, by its button (the
//! keys go through the cards and stop at the last). They are few and short
//! for that reason (a test keeps them so). Asked for again (*Help → Welcome
//! tour*), they close like any dialog.
//!
//! The pictures are in [`art`].

mod art;

use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, FocusHandle, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, Styled as _, Subscription, Window, div,
};
use ph_brand::{Feature, enabled};

use crate::AppState;

/// The setting that keeps the ids of the cards gone through.
const SEEN: &str = "tour.seen";
/// Set (to anything) to start without the cards: for the app's own tests
/// and for work on a scratch profile, where every start is a first one.
const OFF: &str = "PILOTHOUSE_NO_TOUR";

/// One card.
struct Card {
    /// Its own for good: what is kept once the user has gone through it.
    id: &'static str,
    /// The feature it is about, when the build can switch that off.
    feature: Option<Feature>,
    /// Where in the app it is found, said small over the title.
    place: &'static str,
    title: &'static str,
    /// At most three, a line each.
    points: &'static [&'static str],
    /// The lines (by their place) that are about another feature than the
    /// card's: left out with it, so a build that has no phone pane does not
    /// promise one.
    needs: &'static [(usize, Feature)],
    /// News of a release: shows (*What's new*) only to someone who went
    /// through the cards before it was added. Someone new hears of the
    /// thing on another card, so that the welcome stays as short as it was.
    news: bool,
    art: fn(&App) -> AnyElement,
}

impl Card {
    /// The lines this build shows.
    fn lines(&self) -> impl Iterator<Item = &'static str> {
        let needs = self.needs;
        self.points
            .iter()
            .enumerate()
            .filter(move |(place, _)| {
                needs
                    .iter()
                    .all(|(of, feature)| of != place || enabled(*feature))
            })
            .map(|(_, line)| *line)
    }
}

/// The cards, in the order they are read: what the app is, where things
/// are, then what it has that a terminal with an agent does not, the most
/// surprising first, and the small things last.
const CARDS: &[Card] = &[
    Card {
        id: "agents",
        feature: None,
        place: "In every chat",
        title: "Your coding agents, in one app",
        points: &[
            "Claude Code, Codex, Gemini CLI, Antigravity or Cline: pick one per chat.",
            "Chat or the agent's own CLI: switch any time, carry on where you were.",
            "No sign-up and no tracking. It all runs on this computer.",
        ],
        needs: &[],
        news: false,
        art: art::agents,
    },
    Card {
        id: "places",
        feature: None,
        place: "The icons on the left",
        title: "Three places to work",
        points: &[
            "Workspaces: open a project folder and an agent works in it.",
            "Chats: just a chat, no folder needed. Questions, drafts, ideas.",
            "Bridge: all your tasks on one board, the ones waiting for you first.",
        ],
        needs: &[],
        news: false,
        art: art::places,
    },
    Card {
        id: "preview",
        feature: Some(Feature::Browser),
        place: "Preview, next to the chat",
        title: "The agent uses a browser and your phone",
        points: &[
            "A built-in browser, right next to the chat.",
            "Or your own Chrome, so it can work in sites you are signed in to.",
            "Your Android phone, by USB or Wi-Fi: the agent taps, types and looks.",
        ],
        needs: &[(1, Feature::ChromeExtension), (2, Feature::Devices)],
        news: false,
        art: art::preview,
    },
    Card {
        id: "tests",
        feature: Some(Feature::Tests),
        place: "Tests and Utilities",
        title: "Tests and Utilities, built in",
        points: &[
            "Tests: all your project's tests in one list. One fails? Fix with agent.",
            "Ask for a UI test: the agent clicks through your app and records it.",
            "Utilities: small offline tools for JSON, JWT, Base64, regex, hashes.",
        ],
        needs: &[(2, Feature::Utilities)],
        news: false,
        art: art::tests,
    },
    Card {
        id: "review",
        feature: Some(Feature::GitHub),
        place: "Reviews",
        title: "Review GitHub pull requests with an agent",
        points: &[
            "Connect GitHub: every pull request waiting for you, in one list.",
            "An agent reads the changes and lists the problems it finds.",
            "Tick the findings you agree with, then post your review to GitHub.",
        ],
        needs: &[],
        news: false,
        art: art::review,
    },
    Card {
        id: "artifacts",
        feature: Some(Feature::Artifacts),
        place: "Artifacts: what agents made for you",
        title: "Slides and small apps, made in a chat",
        points: &[
            "Slides about your project or any topic, to PowerPoint or PDF.",
            "A whiteboard the agent draws on: a flow, a map of your code, a plan.",
            "Small apps: a dashboard, a planner, a game. Share one on your network.",
        ],
        needs: &[(0, Feature::Presentations), (1, Feature::Whiteboards)],
        news: false,
        art: art::artifacts,
    },
    Card {
        id: "whiteboard",
        feature: Some(Feature::Whiteboards),
        place: "Artifacts → Whiteboard",
        title: "A whiteboard the agent draws on",
        points: &[
            "Ask for a flow, a plan or a map of your code: the agent draws it.",
            "Move things, add notes and arrows by hand. Each change can be undone.",
            "Pin a note to anything and send it to the agent, which makes the change.",
        ],
        needs: &[],
        news: true,
        art: art::whiteboard,
    },
    Card {
        id: "studio",
        feature: None,
        place: "Studio: Edit on any picture or video",
        title: "Edit pictures and video, or ask the agent to",
        points: &[
            "Say “blur the email, add an arrow”, or draw it yourself.",
            "Compress a video: choose “At most 10 MB” and it is made to fit.",
            "Cut parts out, speed up the slow bits, export MP4 or GIF.",
        ],
        needs: &[],
        news: false,
        art: art::studio,
    },
    Card {
        id: "everyday",
        feature: None,
        place: "Around the app",
        title: "Small things you will use every day",
        points: &[
            "Search: one box finds your chats, workspaces and commands.",
            "Quick chat: a small chat window over any app, one shortcut away.",
            "Schedules run an agent on a timer. Themes: light, dark or your own.",
        ],
        needs: &[(1, Feature::QuickChat), (2, Feature::Schedules)],
        news: false,
        art: art::everyday,
    },
];

/// The cards that show: those of features this build has, and unless
/// `all`, only the ones not in `seen`.
fn cards(seen: &[String], all: bool) -> Vec<&'static Card> {
    CARDS
        .iter()
        .filter(|card| card.feature.is_none_or(crate::features::has))
        .filter(|card| all || !seen.iter().any(|id| id == card.id))
        // News is for someone who has gone through the cards before.
        .filter(|card| !card.news || (!all && !seen.is_empty()))
        .collect()
}

/// At a start: shows the cards not gone through yet, if there are any.
/// Whoever opens the app's first window calls this once.
pub fn open_if_due(window: &mut Window, cx: &mut App) {
    if !enabled(Feature::Tour) || std::env::var_os(OFF).is_some_and(|set| !set.is_empty()) {
        return;
    }
    let seen: Vec<String> = AppState::store(cx).setting(SEEN);
    let due = cards(&seen, false);
    if due.is_empty() {
        return;
    }
    open(
        due,
        Kind::Due {
            news: !seen.is_empty(),
        },
        window,
        cx,
    );
}

/// Asked for from Help: every card, again.
pub(crate) fn open_again(window: &mut Window, cx: &mut App) {
    if !enabled(Feature::Tour) {
        return;
    }
    open(cards(&[], true), Kind::Again, window, cx);
}

/// Why the cards show.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// At a start, not gone through yet: the first time (*Welcome*), or new
    /// since an update (`news`).
    Due { news: bool },
    /// Asked for.
    Again,
}

fn open(cards: Vec<&'static Card>, kind: Kind, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| Tour::new(cards, kind, cx));
    let focus = view.read(cx).focus.clone();
    let forced = kind != Kind::Again;
    window.open_dialog(cx, move |dialog, window, _| {
        dialog
            .w(ph_ui::scaled_px(art::WIDTH + 48., window))
            // At a start the only way out is the last card's Done.
            .close_button(!forced)
            .overlay_closable(!forced)
            .keyboard(!forced)
            // With the keyboard its own, a dialog takes Enter as its
            // answer and closes: here Enter is the next card, as it is at a
            // start, and never the way out.
            .on_ok({
                let view = view.clone();
                move |_, window, cx| {
                    view.update(cx, |tour, cx| tour.step(window, cx));
                    false
                }
            })
            .child(view.clone())
    });
    // The keys (the arrows, Enter) are the cards', once the dialog has taken
    // the keyboard for itself.
    window.defer(cx, move |window, cx| focus.focus(window, cx));
}

struct Tour {
    cards: Vec<&'static Card>,
    at: usize,
    kind: Kind,
    focus: FocusHandle,
    /// The first card says which agents are on this computer: it is drawn
    /// again when the look for them ends.
    _installed: Subscription,
}

impl Tour {
    fn new(cards: Vec<&'static Card>, kind: Kind, cx: &mut Context<Self>) -> Self {
        Self {
            cards,
            at: 0,
            kind,
            focus: cx.focus_handle(),
            _installed: cx.observe_global::<ph_agent::installed::Installed>(|_, cx| cx.notify()),
        }
    }

    fn last(&self) -> bool {
        self.at + 1 >= self.cards.len()
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        if self.at > 0 {
            self.at -= 1;
            cx.notify();
        }
    }

    /// The next card, by a key: the keys go through the cards and do not
    /// end them. On the last card a key held, or pressed once too often,
    /// would close it unread: that one is left by its button.
    fn step(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.last() {
            self.at += 1;
            cx.notify();
        }
    }

    /// The button: the next card, and from the last one, done.
    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.last() {
            self.step(window, cx);
            return;
        }
        // Gone through: these do not show at a start again.
        let store = AppState::store(cx).clone();
        let mut seen: Vec<String> = store.setting(SEEN);
        // (Nor does the news someone new was not shown: they read of it
        // on the other cards.)
        let news = CARDS.iter().filter(|card| card.news);
        for card in self.cards.iter().copied().chain(news) {
            if !seen.iter().any(|id| id == card.id) {
                seen.push(card.id.to_string());
            }
        }
        if let Err(err) = store.set_setting(SEEN, &seen) {
            tracing::warn!("the cards gone through could not be noted: {err:#}");
        }
        window.close_dialog(cx);
    }
}

impl Render for Tour {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let card = self.cards[self.at.min(self.cards.len().saturating_sub(1))];
        let (count, at, last) = (self.cards.len(), self.at, self.last());
        let heading = match self.kind {
            Kind::Due { news: true } => "What's new".to_string(),
            _ => format!("Welcome to {}", ph_brand::NAME),
        };
        let r = ph_ui::rems_from_px;
        v_flex()
            .id("tour")
            .key_context("Tour")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "right" | "enter" | "space" => this.step(window, cx),
                    "left" => this.back(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .gap_4()
            // Alone on its line: asked for again, the dialog's close button
            // is at this line's end.
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(heading),
            )
            // The picture: what the card is about, as it looks in the app.
            .child(div().w_full().h(r(art::HEIGHT)).child((card.art)(cx)))
            // As tall for every card, so the buttons stay where they are.
            .child(
                v_flex()
                    .min_h(r(148.))
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(theme.primary)
                            .child(card.place),
                    )
                    .child(
                        div()
                            .text_xl()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(card.title),
                    )
                    .child(
                        v_flex()
                            .pt_1()
                            .gap_1p5()
                            .children(card.lines().map(|point| {
                                h_flex()
                                    .items_start()
                                    .gap_2()
                                    .child(
                                        div()
                                            .mt(r(7.))
                                            .size(r(5.))
                                            .flex_none()
                                            .rounded_full()
                                            .bg(theme.primary),
                                    )
                                    .child(div().text_sm().child(point))
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    // Where one is among the cards.
                    .child(h_flex().gap_1p5().children((0..count).map(|dot| {
                        div()
                            .h(r(6.))
                            .w(r(if dot == at { 18. } else { 6. }))
                            .rounded_full()
                            .bg(if dot == at {
                                theme.primary
                            } else {
                                theme.muted_foreground.opacity(0.35)
                            })
                    })))
                    .child(
                        div()
                            .pl_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} of {count}", at + 1)),
                    )
                    .child(div().flex_1())
                    .when(at > 0, |this| {
                        this.child(
                            Button::new("tour-back")
                                .ghost()
                                .label("Back")
                                .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                        )
                    })
                    .child(
                        Button::new(if last { "tour-done" } else { "tour-next" })
                            .primary()
                            .label(if last { "Done" } else { "Next" })
                            .on_click(cx.listener(|this, _, window, cx| this.next(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cards_not_seen_show_and_no_others() {
        let all: Vec<&str> = cards(&[], false).iter().map(|card| card.id).collect();
        let welcome = CARDS.iter().filter(|card| !card.news).count();
        assert_eq!(
            all.len(),
            welcome,
            "a build with every feature shows them all, but for the news"
        );
        // Someone who went through them before a release added news: the
        // news alone.
        let before: Vec<String> = all.iter().map(|id| id.to_string()).collect();
        let news: Vec<&str> = cards(&before, false).iter().map(|card| card.id).collect();
        assert_eq!(news, ["whiteboard"]);
        // Gone through all of them: nothing at the next start.
        let seen: Vec<String> = CARDS.iter().map(|card| card.id.to_string()).collect();
        assert!(cards(&seen, false).is_empty());
        // A release added one since: that one alone, in its place.
        let before: Vec<String> = seen.iter().filter(|id| *id != "studio").cloned().collect();
        let news: Vec<&str> = cards(&before, false).iter().map(|card| card.id).collect();
        assert_eq!(news, ["studio"]);
        // An id that is no card's any more (one taken out) changes nothing.
        let mut odd = seen.clone();
        odd.push("gone".into());
        assert!(cards(&odd, false).is_empty());
        // Asked for again: the welcome whole, whatever was seen.
        assert_eq!(cards(&seen, true).len(), welcome);
    }

    /// Nobody can skip the cards, so they stay few and short.
    #[test]
    fn the_cards_are_few_and_short() {
        let welcome = CARDS.iter().filter(|card| !card.news).count();
        assert!(welcome <= 8, "{welcome} cards");
        let mut ids: Vec<&str> = CARDS.iter().map(|card| card.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CARDS.len(), "every card has an id of its own");
        for card in CARDS {
            assert!(card.title.chars().count() <= 44, "{}", card.title);
            assert!((1..=3).contains(&card.points.len()), "{}", card.id);
            for point in card.points {
                assert!(point.chars().count() <= 72, "{point}");
            }
            for (place, _) in card.needs {
                assert!(*place < card.points.len(), "{}: line {place}", card.id);
            }
        }
    }
}
