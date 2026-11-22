//! The Utilities dialog: each utility in a view made for what it does.
//!
//! It is used in passing: something is pasted (a token, a blob of JSON, a
//! timestamp), read, perhaps copied, and the dialog is closed. So it opens
//! on the utility used last with the keyboard in its input, a utility is
//! found by typing, and each shows its result as what it is rather than as
//! text: a token's claims with their dates, a certificate's days left, a
//! regex's matches marked in the text, a table as a table, a QR code as a
//! picture.
//!
//! The dialog has one size, whichever utility is shown; each view scrolls
//! inside it. A view keeps what was typed in it while the dialog is open,
//! so going to another utility and back loses nothing. When a utility
//! cannot read what was pasted and another can (a JWT pasted into JSON),
//! it says so and offers to open it there; a decoded result that is JSON
//! (or a token, a URL…) can be opened in that utility the same way.

mod codec;
mod color;
mod crypto;
mod data;
mod ids;
mod inspect;
mod parts;
mod qr;
mod sqlite;
mod table;
mod text;
mod time;

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    input::{Input as Field, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, px,
};
use ph_store::Store;
use ph_ui::Icons;

use crate::{UTILITIES, Utility};

/// The utility used last, which the dialog opens on.
const LAST: &str = "utilities.last";

/// Opens the dialog at utility `id`, or the one used last.
pub fn open(id: Option<&str>, store: Store, window: &mut Window, cx: &mut App) {
    if !ph_brand::enabled(ph_brand::Feature::Utilities) {
        return;
    }
    let last: String = store.setting(LAST);
    let selected = id
        .or(Some(last.as_str()))
        .and_then(|id| UTILITIES.iter().position(|utility| utility.id == id))
        .unwrap_or(0);
    let view = cx.new(|cx| UtilitiesView::new(selected, store, window, cx));
    window.open_dialog(cx, {
        let view = view.clone();
        move |dialog, window, _| {
            let (width, _) = size(window);
            dialog
                .title("Utilities")
                .w(width)
                // Enter in a field (the search, a time zone) is not "OK":
                // the dialog has nothing to confirm, and closed on it.
                .on_ok(|_, _, _| false)
                .child(view.clone())
        }
    });
    view.update(cx, |view, cx| view.focus(window, cx));
}

/// The dialog's width and the height of what is under its title: as large
/// as the interface size asks for, within the window. The same for every
/// utility.
fn size(window: &Window) -> (Pixels, Pixels) {
    let viewport = window.viewport_size();
    let fit = |room: Pixels, least: f32, most: f32| {
        let (least, most) = (
            ph_ui::scaled_px(least, window),
            ph_ui::scaled_px(most, window),
        );
        room.clamp(least.min(room), most)
    };
    (
        fit(viewport.width - px(80.), 680., 1040.),
        fit(viewport.height - px(180.), 380., 640.),
    )
}

/// Whether the room beside the list is wide enough for two columns (an
/// input beside its result, Before beside After).
pub(crate) fn wide(window: &Window) -> bool {
    let (width, _) = size(window);
    width - ph_ui::scaled_px(LIST_WIDTH + 60., window) >= ph_ui::scaled_px(640., window)
}

const LIST_WIDTH: f32 = 196.;

/// Shows a utility with a text as its input.
type Opener = Rc<dyn Fn(&'static str, String, &mut Window, &mut App)>;
/// Gives a view a text as its input.
type Taker = Rc<dyn Fn(String, &mut Window, &mut App)>;
/// Puts the keyboard in a view's input.
type Focuser = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a utility's view is given: the store (for what it remembers) and a
/// way to open another utility with a text.
#[derive(Clone)]
pub(crate) struct Ctx {
    pub id: &'static str,
    pub store: Store,
    open: Opener,
}

impl Ctx {
    pub fn utility(&self) -> &'static Utility {
        crate::find(self.id).expect("a listed utility")
    }

    /// Shows utility `id` with `text` as its input.
    pub fn open_in(&self, id: &'static str, text: String, window: &mut Window, cx: &mut App) {
        // After this click: the view that asked is being updated now.
        let open = self.open.clone();
        window.defer(cx, move |window, cx| open(id, text, window, cx));
    }
}

/// A utility's view.
pub(crate) trait ToolView: Render + Sized {
    fn new(ctx: Ctx, window: &mut Window, cx: &mut Context<Self>) -> Self;
    /// `text` becomes its main input.
    fn take(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>);
    /// The keyboard goes to its main input.
    fn focus(&self, window: &mut Window, cx: &mut App);
}

/// A view made, and how to reach it without knowing its type.
struct Tool {
    view: AnyView,
    take: Taker,
    focus: Focuser,
}

fn tool<T: ToolView>(ctx: Ctx, window: &mut Window, cx: &mut App) -> Tool {
    let entity: Entity<T> = cx.new(|cx| T::new(ctx, window, cx));
    let (taker, focuser) = (entity.clone(), entity.clone());
    Tool {
        view: entity.into(),
        take: Rc::new(move |text, window, cx| {
            taker.update(cx, |view, cx| view.take(text, window, cx))
        }),
        focus: Rc::new(move |window, cx| focuser.update(cx, |view, cx| view.focus(window, cx))),
    }
}

/// The view for utility `ctx.id`.
fn make(ctx: Ctx, window: &mut Window, cx: &mut App) -> Tool {
    match ctx.id {
        "json" | "yaml" | "toml" | "xml" => tool::<data::Formats>(ctx, window, cx),
        "csv" => tool::<table::Csv>(ctx, window, cx),
        "sqlite" => tool::<sqlite::Sqlite>(ctx, window, cx),
        "base64" | "url" | "hex" | "html" => tool::<codec::Codec>(ctx, window, cx),
        "jwt" => tool::<crypto::Jwt>(ctx, window, cx),
        "x509" => tool::<crypto::Certificates>(ctx, window, cx),
        "qr" => tool::<qr::QrView>(ctx, window, cx),
        "hash" => tool::<crypto::Hashes>(ctx, window, cx),
        "hmac" => tool::<crypto::Hmac>(ctx, window, cx),
        "ids" => tool::<ids::Ids>(ctx, window, cx),
        "secrets" => tool::<ids::Secrets>(ctx, window, cx),
        "time" => tool::<time::UnixTime>(ctx, window, cx),
        "cron" => tool::<time::Cron>(ctx, window, cx),
        "regex" => tool::<text::Regex>(ctx, window, cx),
        "diff" => tool::<text::Diff>(ctx, window, cx),
        "markdown" => tool::<text::Markdown>(ctx, window, cx),
        "text" => tool::<inspect::TextTools>(ctx, window, cx),
        "color" => tool::<color::ColorView>(ctx, window, cx),
        "numbers" => tool::<inspect::Numbers>(ctx, window, cx),
        "ip" => tool::<inspect::Ip>(ctx, window, cx),
        _ => tool::<data::Plain>(ctx, window, cx),
    }
}

/// The icon beside a utility's name in the list.
fn icon(id: &str) -> Icons {
    match id {
        "json" => Icons::Braces,
        "yaml" => Icons::FileCode,
        "toml" => Icons::FileCog,
        "xml" => Icons::CodeXml,
        "csv" => Icons::FileSpreadsheet,
        "sqlite" => Icons::Database,
        "base64" => Icons::Binary,
        "url" => Icons::Link,
        "hex" => Icons::Cpu,
        "html" => Icons::Code,
        "jwt" => Icons::KeyRound,
        "x509" => Icons::ShieldCheck,
        "qr" => Icons::QrCode,
        "hash" => Icons::FingerprintPattern,
        "hmac" => Icons::Signature,
        "ids" => Icons::IdCard,
        "secrets" => Icons::Dices,
        "time" => Icons::Clock,
        "cron" => Icons::CalendarClock,
        "regex" => Icons::Regex,
        "diff" => Icons::GitCompare,
        "text" => Icons::CaseSensitive,
        "color" => Icons::Palette,
        "markdown" => Icons::FileText,
        "numbers" => Icons::Calculator,
        "ip" => Icons::Network,
        _ => Icons::Wrench,
    }
}

struct UtilitiesView {
    selected: usize,
    /// Finds a utility by its name, its group or what it is for.
    search: Entity<InputState>,
    /// The views made so far, by utility id.
    tools: HashMap<&'static str, Tool>,
    store: Store,
    this: gpui_kit::WeakEntity<Self>,
    _search: Subscription,
}

impl UtilitiesView {
    fn new(selected: usize, store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a utility…"));
        let subscription = cx.subscribe_in(
            &search,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                // Enter takes the first utility found, ready to type in.
                InputEvent::PressEnter { .. } => {
                    if let Some(first) = this.found(cx).into_iter().next() {
                        this.select(first, window, cx);
                        this.search
                            .update(cx, |search, cx| search.set_value("", window, cx));
                        this.focus(window, cx);
                    }
                }
                _ => {}
            },
        );
        let mut view = Self {
            selected,
            search,
            tools: HashMap::new(),
            store,
            this: cx.entity().downgrade(),
            _search: subscription,
        };
        view.select(selected, window, cx);
        view
    }

    fn utility(&self) -> &'static Utility {
        &UTILITIES[self.selected]
    }

    fn found(&self, cx: &App) -> Vec<usize> {
        matching(&self.search.read(cx).value())
    }

    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = ix;
        let id = self.utility().id;
        if !self.tools.contains_key(id) {
            let this = self.this.clone();
            let ctx = Ctx {
                id,
                store: self.store.clone(),
                open: Rc::new(move |id, text, window, cx| {
                    _ = this.update(cx, |this, cx| this.show(id, text, window, cx));
                }),
            };
            self.tools.insert(id, make(ctx, window, cx));
        }
        if let Err(err) = self.store.set_setting(LAST, &id) {
            tracing::warn!("remembering the utility failed: {err:#}");
        }
        cx.notify();
    }

    /// Shows utility `id` with `text` as its input.
    fn show(
        &mut self,
        id: &'static str,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = UTILITIES.iter().position(|utility| utility.id == id) else {
            return;
        };
        self.select(ix, window, cx);
        if let Some(tool) = self.tools.get(id) {
            (tool.take)(text, window, cx);
            (tool.focus)(window, cx);
        }
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(tool) = self.tools.get(self.utility().id) {
            (tool.focus)(window, cx);
        }
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let found = self.found(cx);
        let mut group = "";
        let mut items = Vec::new();
        for ix in found {
            let utility = &UTILITIES[ix];
            if utility.group != group {
                group = utility.group;
                items.push(
                    div()
                        .flex_none()
                        .pt_2()
                        .pb_0p5()
                        .px_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(group)
                        .into_any_element(),
                );
            }
            let selected = ix == self.selected;
            items.push(
                h_flex()
                    .id(gpui_kit::SharedString::from(format!(
                        "utility-{}",
                        utility.id
                    )))
                    .role(gpui_kit::accesskit::Role::ListItem)
                    .aria_label(utility.name)
                    .flex_none()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .when(selected, |this| {
                        this.bg(theme.list_active)
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                    })
                    .when(!selected, |this| {
                        this.hover(|style| style.bg(theme.list_hover))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select(ix, window, cx);
                        this.focus(window, cx);
                    }))
                    .child(
                        Icon::new(icon(utility.id))
                            .xsmall()
                            .text_color(if selected {
                                theme.foreground
                            } else {
                                theme.muted_foreground
                            }),
                    )
                    .child(div().min_w_0().truncate().child(utility.name))
                    // Findable in UI tests and by the automation socket.
                    .test_support()
                    .into_any_element(),
            );
        }
        let none = items.is_empty();
        v_flex()
            .w(ph_ui::rems_from_px(LIST_WIDTH))
            .flex_none()
            .h_full()
            .gap_1()
            .pr_3()
            .border_r_1()
            .border_color(theme.border)
            .child(
                div().id("utilities-search").test_support().child(
                    Field::new(&self.search).small().prefix(
                        Icon::new(IconName::Search)
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    ),
                ),
            )
            .child(
                v_flex()
                    .id("utilities-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(items)
                    .when(none, |this| {
                        this.child(
                            div()
                                .p_2()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("No utility of that name."),
                        )
                    }),
            )
    }
}

/// The places in [`UTILITIES`] of those with `typed` in their name, group
/// or keywords, any case; all of them when nothing is typed.
fn matching(typed: &str) -> Vec<usize> {
    let typed = typed.trim().to_lowercase();
    UTILITIES
        .iter()
        .enumerate()
        .filter(|(_, utility)| {
            typed.is_empty()
                || [utility.name, utility.group, utility.id]
                    .into_iter()
                    .chain(utility.keywords.iter().copied())
                    .any(|word| word.to_lowercase().contains(&typed))
        })
        .map(|(ix, _)| ix)
        .collect()
}

impl Render for UtilitiesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (_, height) = size(window);
        let body = self
            .tools
            .get(self.utility().id)
            .map(|tool| tool.view.clone());
        h_flex()
            .id("utilities")
            // Findable in UI tests and by the automation socket.
            .test_support()
            .h(height)
            .items_start()
            .gap_4()
            .child(self.render_list(cx))
            .child(v_flex().flex_1().min_w_0().h_full().children(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_utility_is_found_by_its_name_group_or_use() {
        let names = |typed: &str| -> Vec<&str> {
            matching(typed)
                .into_iter()
                .map(|ix| UTILITIES[ix].id)
                .collect()
        };
        assert_eq!(names("").len(), UTILITIES.len());
        assert_eq!(names(" JWT "), ["jwt"]);
        // By what it is for, and by its group.
        assert!(names("pretty").contains(&"json"));
        assert_eq!(names("cron"), ["cron"]);
        assert!(names("subnet").contains(&"ip"));
        assert!(names("camel").contains(&"text"));
        assert!(names("no such thing").is_empty());
    }

    #[test]
    fn every_utility_has_a_view_and_an_icon() {
        for utility in UTILITIES {
            assert_ne!(
                icon(utility.id),
                Icons::Wrench,
                "{} has no icon",
                utility.id
            );
        }
    }
}
