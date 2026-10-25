//! Settings: a centered dialog with a searchable list of pages, like the
//! preferences windows of desktop IDEs.
//!
//! Every value is read from and written to [`Preferences`] (or the app's own
//! settings) live, so a change shows at once and there is no Save button.

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use ph_theme::preferences::{self, installed_fonts};
use ph_theme::{Accent, Appearance, ChatWidth, Corners, Preferences, WindowStyle};
use ph_ui::Icons;

use crate::app_state::AppState;
use crate::context_panel::ContextTab;

/// Opens Settings over the window; `project` is the one in front (for its
/// skills and MCP servers).
pub(crate) fn open(project: Option<std::path::PathBuf>, window: &mut Window, cx: &mut App) {
    // Listing fonts touches the OS font database: do it once per opening.
    let fonts = font_options(cx);
    let store = AppState::store(cx).clone();
    let accounts = cx.new(|cx| ph_pulls::AccountsView::new(store.clone(), window, cx));
    let templates = cx.new(|_| ph_pulls::TemplatesView::new(store.clone()));
    let drives =
        cx.new(|cx| crate::network_drives::NetworkDrivesView::new(store.clone(), window, cx));
    let open_file: ph_agent::tools_view::OpenFile = std::rc::Rc::new(|path, window, cx| {
        use gpui_kit::component::WindowExt as _;
        window.close_dialog(cx);
        if let Err(err) = crate::open::open_paths(&[path], cx) {
            tracing::warn!("opening the file failed: {err:#}");
        }
    });
    let tools =
        cx.new(|cx| ph_agent::tools_view::ToolsView::new(store, project, open_file, window, cx));
    let viewport = window.viewport_size();
    let width = (viewport.width - px(80.)).clamp(px(560.), px(960.));
    let height = (viewport.height - px(120.)).clamp(px(420.), px(680.));
    window.open_dialog(cx, move |dialog, _, _| {
        let fonts = fonts.clone();
        dialog
            .w(width)
            .p_0()
            .close_button(true)
            .title(div().px_4().pt_3().child("Settings"))
            .child(div().h(height).border_t_1().child(
                Settings::new("settings").sidebar_width(px(210.)).pages([
                    appearance_page(&fonts),
                    editor_page(&fonts),
                    terminal_page(),
                    chat_page(),
                    agents_page(),
                    tools_page(tools.clone()),
                    github_page(accounts.clone()),
                    templates_page(templates.clone()),
                    drives_page(drives.clone()),
                    features_page(),
                    shortcuts_page(),
                    about_page(),
                ]),
            ))
    });
}

/// "System default" first, then every installed family.
fn font_options(cx: &App) -> Vec<(SharedString, SharedString)> {
    std::iter::once((
        SharedString::default(),
        SharedString::from("System default"),
    ))
    .chain(
        installed_fonts(cx)
            .into_iter()
            .map(|name| (SharedString::from(name.clone()), SharedString::from(name))),
    )
    .collect()
}

fn prefs(cx: &App) -> Preferences {
    preferences::preferences(cx)
}

/// A dropdown over a small enum: options from `all`, labels from `label`.
fn choice<T: Copy + PartialEq + 'static>(
    all: &'static [T],
    label: fn(T) -> &'static str,
    get: fn(&Preferences) -> T,
    set: fn(&mut Preferences, T),
) -> SettingField<SharedString> {
    SettingField::dropdown(
        all.iter()
            .map(|value| (label(*value).into(), label(*value).into()))
            .collect(),
        move |cx| label(get(&prefs(cx))).into(),
        move |picked, cx| {
            if let Some(value) = all
                .iter()
                .copied()
                .find(|value| label(*value) == picked.as_ref())
            {
                AppState::update_preferences(cx, |preferences| set(preferences, value));
            }
        },
    )
}

fn number(
    min: f64,
    max: f64,
    step: f64,
    get: fn(&Preferences) -> f64,
    set: fn(&mut Preferences, f64),
) -> SettingField<f64> {
    SettingField::number_input(
        NumberFieldOptions { min, max, step },
        move |cx| get(&prefs(cx)),
        move |value, cx| {
            AppState::update_preferences(cx, |preferences| set(preferences, value.clamp(min, max)))
        },
    )
}

fn switch(get: fn(&Preferences) -> bool, set: fn(&mut Preferences, bool)) -> SettingField<bool> {
    SettingField::switch(
        move |cx| get(&prefs(cx)),
        move |value, cx| AppState::update_preferences(cx, |preferences| set(preferences, value)),
    )
}

fn appearance_page(fonts: &[(SharedString, SharedString)]) -> SettingPage {
    let ui_fonts = fonts.to_vec();
    let window_styles: &'static [WindowStyle] =
        Box::leak(WindowStyle::available().into_boxed_slice());
    SettingPage::new("Appearance")
        .icon(Icon::new(Icons::Palette))
        .default_open(true)
        .group(
            SettingGroup::new().title("Layout").item(
                SettingItem::new(
                    "Names under tool icons",
                    switch(|p| p.tool_labels, |p, v| p.tool_labels = v),
                )
                .description(
                    "Show each tool's name under its icon on the right, not only when pointing at it.",
                )
                .keywords(["layout", "labels", "tools", "icons", "activity bar"]),
            ),
        )
        .group(
            SettingGroup::new()
                .title("Theme")
                .item(
                    SettingItem::new(
                        "Mode",
                        SettingField::dropdown(
                            Appearance::ALL.map(|mode| (mode.label().into(), mode.label().into())).to_vec(),
                            |cx| AppState::appearance(cx).label().into(),
                            |picked, cx| {
                                let Some(mode) = Appearance::ALL.into_iter().find(|mode| mode.label() == picked.as_ref()) else {
                                    return;
                                };
                                for window in cx.windows() {
                                    let _ = window.update(cx, |_, window, cx| AppState::set_appearance(mode, window, cx));
                                }
                            },
                        ),
                    )
                    .description("Light, dark, or follow the system.")
                    .keywords(["dark", "light", "theme"]),
                )
                .item(
                    SettingItem::render(|_, _, cx| accent_swatches(cx))
                        .keywords(["accent", "color", "colour", "highlight"]),
                )
                .item(
                    SettingItem::new("Corners", choice(&Corners::ALL, Corners::label, |p| p.corners, |p, v| p.corners = v))
                        .description("How rounded panels, buttons and cards are."),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Window")
                .item(
                    SettingItem::new("Background", choice(window_styles, WindowStyle::label, |p| p.window, |p, v| p.window = v))
                        .description(if cfg!(windows) {
                            "Translucent lets the desktop show through, blurred. Mica uses Windows 11's material."
                        } else {
                            "Translucent lets the desktop show through, blurred."
                        })
                        .keywords(["transparency", "opacity", "blur", "vibrancy", "mica"]),
                )
                .item(
                    SettingItem::new(
                        "Panel opacity (%)",
                        number(55.0, 100.0, 5.0, |p| (p.opacity * 100.0).round() as f64, |p, v| p.opacity = (v / 100.0) as f32),
                    )
                    .description("How solid panels are when the background is translucent.")
                    .keywords(["transparency", "alpha"]),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Text")
                .item(
                    SettingItem::new(
                        "Interface font",
                        SettingField::scrollable_dropdown(
                            ui_fonts,
                            |cx| prefs(cx).ui_font.into(),
                            |picked, cx| AppState::update_preferences(cx, |p| p.ui_font = picked.to_string()),
                        ),
                    )
                    .description("Used for menus, chat and labels.")
                    .keywords(["font", "typeface", "family"]),
                )
                .item(
                    SettingItem::new("Interface size", number(11.0, 20.0, 1.0, |p| p.ui_font_size as f64, |p, v| p.ui_font_size = v as f32))
                        .description("Text size across the app, in points.")
                        .keywords(["font size", "zoom", "bigger", "smaller"]),
                ),
        )
}

/// Six accent swatches; the chosen one is ringed.
fn accent_swatches(cx: &mut App) -> impl IntoElement + use<> {
    let current = prefs(cx).accent;
    let dark = cx.theme().is_dark();
    h_flex()
        .w_full()
        .justify_between()
        .gap_4()
        .child(
            v_flex()
                .gap_0p5()
                .child(div().text_sm().child("Accent color"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Focus rings, selections, links and highlights."),
                ),
        )
        .child(h_flex().gap_2().children(Accent::ALL.map(|accent| {
            let selected = accent == current;
            div()
                .id(SharedString::from(format!("accent-{}", accent.label())))
                .size_6()
                .rounded_full()
                .bg(accent.color(dark))
                .cursor_pointer()
                .border_2()
                .border_color(if selected {
                    cx.theme().foreground
                } else {
                    cx.theme().background
                })
                .on_click(move |_, _, cx| AppState::update_preferences(cx, |p| p.accent = accent))
        })))
}

fn editor_page(fonts: &[(SharedString, SharedString)]) -> SettingPage {
    let code_fonts = fonts.to_vec();
    SettingPage::new("Editor")
        .icon(Icon::new(Icons::FileCode))
        .group(
            SettingGroup::new()
                .title("Font")
                .item(
                    SettingItem::new(
                        "Code font",
                        SettingField::scrollable_dropdown(
                            code_fonts,
                            |cx| prefs(cx).code_font.into(),
                            |picked, cx| {
                                AppState::update_preferences(cx, |p| {
                                    p.code_font = picked.to_string()
                                })
                            },
                        ),
                    )
                    .description(
                        "Used for code, diffs and terminals. A monospaced font reads best.",
                    )
                    .keywords(["font", "monospace", "family"]),
                )
                .item(
                    SettingItem::new(
                        "Code size",
                        number(
                            10.0,
                            24.0,
                            1.0,
                            |p| p.code_font_size as f64,
                            |p, v| p.code_font_size = v as f32,
                        ),
                    )
                    .keywords(["font size"]),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Behavior")
                .item(
                    SettingItem::new(
                        "Tab size",
                        SettingField::dropdown(
                            ["2", "4", "8"]
                                .map(|size| (size.into(), size.into()))
                                .to_vec(),
                            |cx| prefs(cx).editor.tab_size.to_string().into(),
                            |picked, cx| {
                                if let Ok(size) = picked.parse() {
                                    AppState::update_preferences(cx, |p| p.editor.tab_size = size);
                                }
                            },
                        ),
                    )
                    .keywords(["indent", "spaces"]),
                )
                .item(
                    SettingItem::new(
                        "Wrap long lines",
                        switch(|p| p.editor.soft_wrap, |p, v| p.editor.soft_wrap = v),
                    )
                    .keywords(["soft wrap", "word wrap"]),
                )
                .item(SettingItem::new(
                    "Line numbers",
                    switch(|p| p.editor.line_numbers, |p, v| p.editor.line_numbers = v),
                ))
                .item(SettingItem::new(
                    "Indent guides",
                    switch(
                        |p| p.editor.indent_guides,
                        |p, v| p.editor.indent_guides = v,
                    ),
                ))
                .item(
                    SettingItem::new(
                        "Code folding",
                        switch(|p| p.editor.folding, |p, v| p.editor.folding = v),
                    )
                    .keywords(["fold", "collapse"]),
                ),
        )
}

fn terminal_page() -> SettingPage {
    SettingPage::new("Terminal")
        .icon(Icon::new(Icons::Terminal))
        .group(
            SettingGroup::new()
                .item(
                    SettingItem::new(
                        "Font size",
                        number(
                            10.0,
                            24.0,
                            1.0,
                            |p| p.terminal_font_size as f64,
                            |p, v| p.terminal_font_size = v as f32,
                        ),
                    )
                    .description("Terminals use the code font.")
                    .keywords(["terminal", "shell", "console"]),
                )
                .item(
                    SettingItem::render(|_, _, cx| {
                        let (shell, _) = ph_env::default_shell();
                        note(format!("Shell: {}", shell.display()), cx)
                    })
                    .keywords(["shell", "powershell", "zsh", "bash"]),
                ),
        )
}

/// GitHub and GitHub Enterprise accounts, for pull requests.
fn tools_page(tools: gpui_kit::Entity<ph_agent::tools_view::ToolsView>) -> SettingPage {
    SettingPage::new("MCP & skills")
        .icon(Icon::new(Icons::Plug))
        .group(
            SettingGroup::new().item(SettingItem::render(move |_, _, _| {
                div().w_full().py_1().child(tools.clone())
            })),
        )
}

fn github_page(accounts: gpui_kit::Entity<ph_pulls::AccountsView>) -> SettingPage {
    SettingPage::new("GitHub")
        .icon(Icon::new(Icons::GitPullRequest))
        .group(
            SettingGroup::new().item(SettingItem::render(move |_, _, _| {
                div().w_full().py_1().child(accounts.clone())
            })),
        )
}

/// Settings → Network drives (see [`crate::network_drives`]).
fn templates_page(view: gpui_kit::Entity<ph_pulls::TemplatesView>) -> SettingPage {
    SettingPage::new("Templates")
        .icon(Icon::new(Icons::ScrollText))
        .group(
            SettingGroup::new().item(SettingItem::render(move |_, _, _| {
                div().w_full().py_1().child(view.clone())
            })),
        )
}

fn drives_page(view: gpui_kit::Entity<crate::network_drives::NetworkDrivesView>) -> SettingPage {
    SettingPage::new("Network drives")
        .icon(Icon::new(Icons::HardDrive))
        .group(
            SettingGroup::new().item(SettingItem::render(move |_, _, _| {
                div().w_full().py_1().child(view.clone())
            })),
        )
}

/// Each agent's setup status, checked when the page renders.
fn agents_page() -> SettingPage {
    SettingPage::new("Agents")
        .icon(Icon::new(Icons::Bot))
        .group(
            SettingGroup::new().items(ph_agent::agents::AGENTS.iter().map(|spec| {
                SettingItem::render(move |_, _, cx| {
                    let checks = ph_agent::setup::checks(spec);
                    div()
                        .w_full()
                        .py_1()
                        .child(ph_agent::setup_view::card(spec, &checks, None, cx))
                })
            })),
        )
}

fn chat_page() -> SettingPage {
    SettingPage::new("Chat")
        .icon(Icon::new(Icons::MessageSquare))
        .group(
            SettingGroup::new().item(
                SettingItem::new(
                    "Reading width",
                    choice(
                        &ChatWidth::ALL,
                        ChatWidth::label,
                        |p| p.chat_width,
                        |p, v| p.chat_width = v,
                    ),
                )
                .description("How wide messages get on a large window.")
                .keywords(["chat", "width", "column"]),
            ),
        )
}

fn features_page() -> SettingPage {
    SettingPage::new("Features")
        .icon(Icon::new(Icons::LayoutGrid))
        .group(
            SettingGroup::new()
                .description(
                    "Turn off tools you do not use. Nothing runs for a tool until you open it.",
                )
                .items(ContextTab::ALL.into_iter().map(|tab| {
                    SettingItem::new(
                        tab.label(),
                        SettingField::switch(
                            move |cx| AppState::enabled_tabs(cx).contains(&tab),
                            move |enabled, cx| AppState::set_tab_enabled(tab, enabled, cx),
                        ),
                    )
                    .keywords(["feature", "tab", "panel"])
                })),
        )
        .group(system_group())
}

/// "Open with PilotHouse" in the file manager.
fn system_group() -> SettingGroup {
    use crate::shell_integration as shell;
    let group = SettingGroup::new().title("System");
    if !shell::SUPPORTED {
        return group.item(
            SettingItem::new(
                "Open with PilotHouse",
                SettingField::render(|_, _, cx| {
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("In Finder: right-click → Open With → PilotHouse")
                }),
            )
            .description("Folders open as workspaces, files in the editor. From a terminal: pilothouse <path>.")
            .keywords(["finder", "open with", "context menu"]),
        );
    }
    group.item(
        SettingItem::new(
            shell::label(),
            SettingField::switch(
                |_| shell::is_installed(),
                |on, _| {
                    let result = if on {
                        shell::install()
                    } else {
                        shell::uninstall()
                    };
                    if let Err(err) = result {
                        tracing::warn!("changing the file manager menu failed: {err:#}");
                    }
                },
            ),
        )
        .description(
            "Folders open as workspaces, files in the editor. From a terminal: pilothouse <path>.",
        )
        .keywords(["explorer", "open with", "context menu", "right-click"]),
    )
}

/// Every shortcut: the same sheet as ⌘/, read from the key bindings.
fn shortcuts_page() -> SettingPage {
    SettingPage::new("Shortcuts")
        .icon(Icon::new(Icons::Keyboard))
        .group(
            SettingGroup::new().item(
                SettingItem::render(|_, window, cx| {
                    div()
                        .w_full()
                        .py_1()
                        .child(crate::shortcuts::render(window, cx))
                })
                .keywords(["shortcut", "keyboard", "key"]),
            ),
        )
}

fn about_page() -> SettingPage {
    SettingPage::new("About")
        .icon(Icon::new(IconName::Info))
        .group(
            SettingGroup::new()
                .item(SettingItem::render(|_, _, cx| {
                    note(format!("PilotHouse {} · Apache-2.0", ph_ui::version()), cx)
                }))
                .item(SettingItem::render(|_, _, cx| {
                    let data = ph_store::paths::data_dir();
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_3()
                        .child(note(format!("Your data: {}", data.display()), cx))
                        .child(
                            Button::new("open-data-folder")
                                .small()
                                .outline()
                                .icon(Icons::FolderOpen)
                                .label("Open")
                                .on_click(move |_, _, cx| cx.reveal_path(&data)),
                        )
                }))
                .item(SettingItem::render(|_, _, cx| {
                    note(
                        "Everything stays on this computer: there is no PilotHouse server."
                            .to_string(),
                        cx,
                    )
                })),
        )
}

fn note(text: String, cx: &App) -> impl IntoElement + use<> {
    div()
        .w_full()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}
