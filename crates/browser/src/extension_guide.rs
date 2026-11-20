//! The drawings of Settings → Chrome extension: Chrome's windows, pages and
//! menus as wireframes, each with the one thing to click marked.
//!
//! They are drawn here, from the theme, and are not pictures: they follow
//! light and dark, the accent and the interface size, and there is no image
//! to keep in step with Chrome's look. Only the names the user has to find
//! (*Developer mode*, *Load unpacked*, the extension's own entries) are
//! written out; everything else on Chrome's screen is a bar.

use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Div, FontWeight, Hsla, IntoElement, ParentElement as _, Styled as _, div, px,
};
use ph_ui::rems_from_px as rem;

/// A drawing's width (in the interface's measure, like all sizes here).
const WIDTH: f32 = 300.;
/// The height of the page under a drawn window's toolbar, where a drawing
/// asks for no other.
const PAGE: f32 = 104.;

/// The extension's name in Chrome. It is the same in a build of another
/// name (see "Names that stay as they are" in AGENTS.md).
const EXTENSION: &str = "PilotHouse";

/// The color of the extension's tab group (`purple`, in `lib/tabs.js`).
fn group_color(cx: &App) -> Hsla {
    // Violet, in the shade that reads on this theme.
    ph_ui::workspace_color(1, cx.theme().is_dark())
}

/// The box a drawing is in.
fn canvas(cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_none()
        .w(rem(WIDTH))
        .max_w_full()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .overflow_hidden()
}

/// A line of Chrome's own text that is not the point.
fn bar(width: f32, cx: &App) -> Div {
    div()
        .flex_none()
        .h(rem(5.))
        .w(rem(width))
        .rounded_full()
        .bg(cx.theme().muted_foreground.opacity(0.28))
}

/// A name the user looks for on Chrome's screen.
fn word(text: &'static str, cx: &App) -> Div {
    div()
        .flex_none()
        .text_xs()
        .text_color(cx.theme().foreground)
        .child(text)
}

/// What to click: ringed in the accent.
fn target(cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_none()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.ring)
        .bg(theme.ring.opacity(0.14))
}

/// The pointer, beside what it clicks.
fn pointer(cx: &App) -> Icon {
    Icon::new(ph_ui::Icons::MousePointer2)
        .small()
        .text_color(cx.theme().foreground)
}

/// A button of Chrome's that is not the one to click.
fn plain_button(width: f32, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .h(rem(18.))
        .px_2()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .child(bar(width, cx))
}

/// A tab that is not the point.
fn plain_tab(cx: &App) -> Div {
    h_flex()
        .flex_none()
        .h(rem(16.))
        .px_2()
        .rounded_t(px(5.))
        .bg(cx.theme().background)
        .child(bar(34., cx))
}

/// The extension's mark, as Chrome shows it.
fn extension_mark(cx: &App) -> Div {
    h_flex()
        .flex_none()
        .size(rem(14.))
        .justify_center()
        .rounded(px(3.))
        .bg(cx.theme().secondary)
        .child(ph_ui::logo_mark(rem(10.)))
}

/// A Chrome window: its tabs, the address and the buttons beside it, and
/// the page.
fn window(
    tabs: impl IntoElement,
    address: Option<&'static str>,
    actions: impl IntoElement,
    (height, page): (f32, impl IntoElement),
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let dot = || {
        div()
            .flex_none()
            .size(rem(6.))
            .rounded_full()
            .bg(theme.muted_foreground.opacity(0.35))
    };
    // An address the user types is written out; any other is a bar.
    let address = match address {
        Some(address) => div()
            .truncate()
            .text_xs()
            .font_family(theme.mono_font_family.clone())
            .text_color(theme.foreground)
            .child(address)
            .into_any_element(),
        None => bar(90., cx).into_any_element(),
    };
    canvas(cx)
        .child(
            h_flex()
                .h(rem(24.))
                .px_2()
                .pt(rem(6.))
                .gap_1()
                .items_end()
                // Rounded itself: a child's background is not cut to the
                // box's corners.
                .rounded_t(theme.radius_lg)
                .bg(theme.secondary)
                .child(
                    h_flex()
                        .h(rem(16.))
                        .gap_1()
                        .pr_1()
                        .child(dot())
                        .child(dot())
                        .child(dot()),
                )
                .child(tabs),
        )
        .child(
            h_flex()
                .h(rem(26.))
                .px_2()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .h(rem(16.))
                        .px_2()
                        .rounded_full()
                        .bg(theme.secondary)
                        .child(address),
                )
                .child(actions),
        )
        .child(div().relative().h(rem(height)).p_2().child(page))
}

/// The app and the browser, linked: what *Set up* makes.
pub(crate) fn link(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let side = |mark: AnyElement, name: &'static str| {
        v_flex()
            .flex_none()
            .w(rem(92.))
            .py_3()
            .gap_1p5()
            .items_center()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(mark)
            .child(div().text_xs().text_color(theme.foreground).child(name))
    };
    let browser = h_flex().flex_none().size(rem(22.)).justify_center().child(
        Icon::new(IconName::Globe)
            .size(rem(20.))
            .text_color(theme.muted_foreground),
    );
    canvas(cx)
        .child(
            h_flex()
                .h(rem(PAGE))
                .px_3()
                .justify_center()
                .child(side(ph_ui::logo_mark(rem(22.)), ph_brand::NAME))
                .child(
                    h_flex()
                        .flex_1()
                        .items_center()
                        .child(div().flex_1().h(px(1.)).bg(theme.ring))
                        .child(
                            div()
                                .flex_none()
                                .size(rem(7.))
                                .rounded_full()
                                .bg(theme.ring),
                        )
                        .child(div().flex_1().h(px(1.)).bg(theme.ring)),
                )
                .child(side(browser.into_any_element(), "Chrome")),
        )
        .into_any_element()
}

/// `chrome://extensions`, with *Developer mode* at its top right.
pub(crate) fn developer_mode(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let switch = h_flex()
        .flex_none()
        .w(rem(22.))
        .h(rem(12.))
        .px(px(2.))
        .justify_end()
        .rounded_full()
        .bg(theme.ring)
        .child(
            div()
                .size(rem(8.))
                .rounded_full()
                .bg(theme.primary_foreground),
        );
    let page = v_flex()
        .gap_3()
        .child(
            h_flex().justify_between().child(bar(64., cx)).child(
                h_flex().gap_1().child(
                    target(cx).child(
                        h_flex()
                            .gap_1p5()
                            .px_1p5()
                            .py_1()
                            .child(word("Developer mode", cx))
                            .child(switch),
                    ),
                ),
            ),
        )
        .child(
            h_flex()
                .justify_end()
                .pr_3()
                .mt(rem(-8.))
                .child(pointer(cx)),
        )
        .child(
            h_flex()
                .gap_2()
                .child(extension_card(cx))
                .child(extension_card(cx)),
        );
    window(
        plain_tab(cx),
        Some("chrome://extensions"),
        div(),
        (PAGE, page),
        cx,
    )
    .into_any_element()
}

/// An extension's card on Chrome's extensions page.
fn extension_card(cx: &App) -> Div {
    v_flex()
        .flex_1()
        .gap_1p5()
        .p_2()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .child(bar(52., cx))
        .child(bar(78., cx))
}

/// The extensions page with *Load unpacked*, and the folder to choose.
pub(crate) fn load_unpacked(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let page = v_flex()
        .gap_2()
        .child(
            h_flex()
                .gap_1p5()
                .child(
                    target(cx)
                        .h(rem(18.))
                        .px_2()
                        .flex()
                        .items_center()
                        .child(word("Load unpacked", cx)),
                )
                .child(plain_button(46., cx))
                .child(plain_button(30., cx)),
        )
        .child(h_flex().pl_8().mt(rem(-6.)).child(pointer(cx)))
        // The folder picker that follows, with the folder to choose.
        .child(
            h_flex()
                .gap_2()
                .p_2()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.secondary)
                .child(
                    Icon::new(IconName::Folder)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.foreground)
                        .child("chrome-extension"),
                )
                .child(target(cx).px_2().py_0p5().child(word("Select", cx))),
        );
    window(
        plain_tab(cx),
        Some("chrome://extensions"),
        div(),
        (PAGE, page),
        cx,
    )
    .into_any_element()
}

/// The puzzle-piece menu, with the pin beside the extension.
pub(crate) fn pin(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let puzzle = target(cx).p(px(2.)).child(
        Icon::new(ph_ui::Icons::Puzzle)
            .small()
            .text_color(theme.foreground),
    );
    let row = |mark: Div, name: AnyElement, pin: AnyElement| {
        h_flex()
            .gap_2()
            .child(mark)
            .child(div().flex_1().min_w_0().child(name))
            .child(pin)
    };
    let menu = v_flex()
        .absolute()
        .top_1()
        .right_2()
        .w(rem(176.))
        .gap_2()
        .p_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .shadow_md()
        .child(bar(58., cx))
        .child(row(
            extension_mark(cx),
            word(EXTENSION, cx).into_any_element(),
            h_flex()
                .gap_1()
                .child(
                    target(cx).p(px(2.)).child(
                        Icon::new(ph_ui::Icons::Pin)
                            .small()
                            .text_color(theme.foreground),
                    ),
                )
                .child(pointer(cx))
                .into_any_element(),
        ))
        .child(row(
            div()
                .flex_none()
                .size(rem(14.))
                .rounded(px(3.))
                .bg(theme.muted_foreground.opacity(0.28)),
            bar(70., cx).into_any_element(),
            Icon::new(ph_ui::Icons::Pin)
                .small()
                .text_color(theme.muted_foreground.opacity(0.5))
                .into_any_element(),
        ));
    let page = v_flex()
        .gap_2()
        .child(bar(120., cx))
        .child(bar(84., cx))
        .child(menu);
    window(plain_tab(cx), None, puzzle, (PAGE, page), cx).into_any_element()
}

/// The extension's tab group, and a tab an agent acts in.
pub(crate) fn tab_group(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let group = group_color(cx);
    let tabs = h_flex()
        .gap_1()
        .items_end()
        .child(
            h_flex()
                .flex_none()
                .h(rem(14.))
                .mb(px(1.))
                .px_1p5()
                .rounded(px(4.))
                .bg(group)
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        // Paper on the group's color: dark on the light
                        // violet of a dark theme, light on the other.
                        .text_color(theme.background)
                        .child(EXTENSION),
                ),
        )
        // The agent's tab: under the group's line, like every tab of it.
        .child(
            h_flex()
                .flex_none()
                .h(rem(16.))
                .px_2()
                .rounded_t(px(5.))
                .bg(theme.background)
                .border_b_2()
                .border_color(group)
                .child(bar(40., cx)),
        )
        .child(h_flex().flex_none().h(rem(16.)).px_2().child(bar(30., cx)));
    // The glow around a page an agent acts in.
    let page = v_flex()
        .size_full()
        .gap_2()
        .p_2()
        .rounded(theme.radius)
        .border_2()
        .border_color(group.opacity(0.7))
        .bg(group.opacity(0.06))
        .child(bar(130., cx))
        .child(bar(180., cx))
        .child(bar(96., cx));
    window(tabs, None, extension_mark(cx), (PAGE, page), cx).into_any_element()
}

/// The extension's popup, under its button in the toolbar.
pub(crate) fn popup(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let row = |text: &'static str| h_flex().h(rem(17.)).px_2().child(word(text, cx));
    let heading = |width: f32| h_flex().h(rem(13.)).px_2().child(bar(width, cx));
    let switch = h_flex()
        .flex_none()
        .w(rem(20.))
        .h(rem(11.))
        .px(px(2.))
        .rounded_full()
        .bg(theme.muted_foreground.opacity(0.35))
        .child(div().size(rem(7.)).rounded_full().bg(theme.background));
    let menu = v_flex()
        .absolute()
        .top_1()
        .right_2()
        .w(rem(172.))
        .py_1()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .shadow_md()
        .child(heading(64.))
        .child(row("This page"))
        .child(row("A screenshot"))
        .child(row("An element"))
        .child(div().my_1().h(px(1.)).bg(theme.border))
        .child(heading(36.))
        .child(row("A bug report"))
        .child(row("A workflow"))
        .child(div().my_1().h(px(1.)).bg(theme.border))
        .child(
            h_flex()
                .h(rem(17.))
                .px_2()
                .justify_between()
                .child(word("Let agents use this tab", cx))
                .child(switch),
        );
    let button = h_flex()
        .gap_1()
        .child(pointer(cx))
        .child(target(cx).p(px(2.)).child(extension_mark(cx)));
    let page = v_flex()
        .gap_2()
        .child(bar(96., cx))
        .child(bar(70., cx))
        .child(menu);
    // Taller than the others: the whole popup shows.
    window(plain_tab(cx), None, button, (168., page), cx).into_any_element()
}

/// A page's right-click menu, with the extension's entries beside it.
pub(crate) fn context_menu(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let menu = |width: f32| {
        v_flex()
            .absolute()
            .w(rem(width))
            .py_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_md()
    };
    let plain = |width: f32| h_flex().h(rem(15.)).px_2().child(bar(width, cx));
    let entry = |text: &'static str| {
        h_flex()
            .h(rem(15.))
            .px_2()
            .child(div().truncate().child(word(text, cx)))
    };
    let chrome = menu(120.)
        .top_1()
        .left_2()
        .child(plain(48.))
        .child(plain(66.))
        .child(plain(40.))
        .child(
            target(cx).mx_1().child(
                h_flex()
                    .h(rem(16.))
                    .px_1()
                    .gap_1()
                    .child(extension_mark(cx))
                    .child(div().flex_1().child(word(EXTENSION, cx)))
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .xsmall()
                            .text_color(theme.muted_foreground),
                    ),
            ),
        )
        .child(plain(56.));
    let ours = menu(152.)
        .top(rem(38.))
        .left(rem(134.))
        .child(entry("Send this page"))
        .child(entry("Screenshot an area…"))
        .child(entry("Pick an element…"));
    let page = v_flex()
        .gap_2()
        .child(bar(150., cx))
        .child(bar(200., cx))
        .child(chrome)
        .child(ours);
    window(plain_tab(cx), None, extension_mark(cx), (PAGE, page), cx).into_any_element()
}

/// A chat's message box with what was sent from Chrome waiting in it.
pub(crate) fn waiting(cx: &App) -> AnyElement {
    let theme = cx.theme();
    let chip = h_flex()
        .flex_none()
        .gap_1p5()
        .h(rem(20.))
        .px_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.ring)
        .bg(theme.ring.opacity(0.14))
        .child(
            Icon::new(IconName::Globe)
                .xsmall()
                .text_color(theme.foreground),
        )
        .child(word("example.com", cx));
    canvas(cx)
        .bg(theme.secondary)
        .child(
            v_flex()
                .p_3()
                .gap_2()
                .child(bar(170., cx))
                .child(bar(110., cx))
                .child(
                    v_flex()
                        .mt_1()
                        .gap_2()
                        .p_2()
                        .rounded(theme.radius_lg)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.background)
                        .child(h_flex().child(chip))
                        .child(
                            h_flex().justify_between().child(bar(120., cx)).child(
                                h_flex()
                                    .flex_none()
                                    .size(rem(20.))
                                    .justify_center()
                                    .rounded_full()
                                    .bg(theme.button_primary)
                                    .child(
                                        Icon::new(IconName::ArrowUp)
                                            .xsmall()
                                            .text_color(theme.button_primary_foreground),
                                    ),
                            ),
                        ),
                ),
        )
        .into_any_element()
}
