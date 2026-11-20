//! Settings: a centered dialog with a searchable list of pages, like the
//! preferences windows of desktop IDEs.
//!
//! Every value is read from and written to [`Preferences`] (or the app's own
//! settings) live, so a change shows at once and there is no Save button.

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings},
};
use gpui_kit::{
    App, AppContext as _, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use ph_theme::preferences::{self, installed_fonts};
use ph_theme::{Accent, Appearance, ChatWidth, Corners, Density, Preferences};
use ph_ui::Icons;

use crate::app_state::AppState;
use crate::context_panel::ContextTab;

/// A page Settings can open at.
#[derive(Clone, Copy)]
pub(crate) enum Page {
    First,
    Chrome,
    GitHub,
    Storage,
}

impl Page {
    /// Its place in the list of pages (see [`open`]): pages of features the
    /// brand leaves out are not in it.
    fn index(self) -> usize {
        use ph_brand::{Feature, enabled};
        let chrome = usize::from(enabled(Feature::ChromeExtension));
        let drives = usize::from(enabled(Feature::NetworkDrives));
        match self {
            Page::First => 0,
            Page::Chrome => 6,
            Page::GitHub => 6 + chrome,
            Page::Storage => 8 + chrome + drives,
        }
    }
}

/// Opens Settings over the window at `page`; `project` is the one in front
/// (for its skills and MCP servers).
pub(crate) fn open(
    project: Option<std::path::PathBuf>,
    page: Page,
    workspace: gpui_kit::WeakEntity<crate::Workspace>,
    window: &mut Window,
    cx: &mut App,
) {
    // Listing fonts touches the OS font database: do it once per opening.
    let fonts = font_options(cx);
    let themes = theme_options(cx);
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
    // An agent is installed where it can be watched: in a terminal of the
    // workspace in front, with Settings out of the way.
    let install: Option<ph_agent::agents_view::Install> =
        ph_brand::enabled(ph_brand::Feature::Terminal).then(|| {
            let workspace = workspace.clone();
            std::rc::Rc::new(
                move |spec: &'static ph_agent::agents::AgentSpec,
                      window: &mut Window,
                      cx: &mut App| {
                    use gpui_kit::component::WindowExt as _;
                    let space = workspace
                        .upgrade()
                        .and_then(|workspace| workspace.read(cx).active_space().cloned());
                    let Some(space) = space else {
                        window.push_notification(
                            "Open a workspace first: the command runs in its terminal.",
                            cx,
                        );
                        return;
                    };
                    window.close_dialog(cx);
                    space.update(cx, |space, cx| {
                        space.run_in_terminal(
                            &format!("Install {}", spec.name),
                            spec.install_command(),
                            window,
                            cx,
                        )
                    });
                },
            ) as ph_agent::agents_view::Install
        });
    let agents = {
        let store = store.clone();
        cx.new(|cx| ph_agent::agents_view::AgentsView::new(store, install, window, cx))
    };
    let tools =
        cx.new(|cx| ph_agent::tools_view::ToolsView::new(store, project, open_file, window, cx));
    let storage = cx.new(crate::storage::StorageView::new);
    let chrome = cx.new(ph_browser::ChromeExtensionView::new);
    window.open_dialog(cx, move |dialog, window, cx| {
        // As large as the interface size asks for, within the window.
        // Worked out as it is drawn: the size is changed in this dialog.
        let viewport = window.viewport_size();
        let fit = |room: gpui_kit::Pixels, least: f32, most: f32| {
            let (least, most) = (
                ph_ui::scaled_px(least, window),
                ph_ui::scaled_px(most, window),
            );
            room.clamp(least.min(room), most)
        };
        let width = fit(viewport.width - px(80.), 560., 960.);
        let height = fit(viewport.height - px(120.), 420., 680.);
        let (fonts, themes, workspace) = (fonts.clone(), themes.clone(), workspace.clone());
        // Opaque, also when the window lets something show through: a
        // dialog over the window must not.
        let frame_color = cx.theme().sidebar;
        let theme = cx.theme();
        let sidebar = gpui_kit::StyleRefinement {
            background: Some(frame_color.into()),
            ..Default::default()
        };
        // The search box level with the page's top edge (the list's header
        // has room above it of its own).
        let search = gpui_kit::StyleRefinement::default().mt(px(-12.));
        // Like the workspace: the title, the list and the frame are one
        // surface, and the page floats on it, rounded and bordered, its top
        // level with the search box. The page's bottom corners reach past
        // the list, so the line between the two meets their straight edge;
        // at the top, a strip of the frame's color hides the line beside
        // the curve.
        let frame = div()
            .absolute()
            .top(px(TOP))
            .left(px(SIDEBAR_WIDTH))
            .right(px(FRAME))
            .bottom(px(FRAME))
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .shadow_xs();
        dialog
            .w(width)
            .p_0()
            .bg(frame_color)
            .close_button(true)
            .title(div().px_4().pt_3().child("Settings"))
            .child(
                div()
                    .relative()
                    .h(height)
                    .child(frame)
                    .child(
                        div()
                            .size_full()
                            .pt(px(TOP))
                            .pr(px(FRAME))
                            .pb(theme.radius_lg + px(FRAME + 2.))
                            .child(
                                Settings::new("settings")
                                    .sidebar_width(px(SIDEBAR_WIDTH))
                                    // Fixed: the page's frame is drawn at its edge.
                                    .sidebar_size_range(px(SIDEBAR_WIDTH)..px(SIDEBAR_WIDTH))
                                    .sidebar_style(&sidebar)
                                    .header_style(&search)
                                    .default_selected_index(
                                        gpui_kit::component::setting::SelectIndex {
                                            page_ix: page.index(),
                                            group_ix: None,
                                        },
                                    )
                                    .pages(
                                        // In this order: see `Page::index`.
                                        [
                                            Some(appearance_page(&fonts, &themes, workspace)),
                                            Some(editor_page(&fonts)),
                                            Some(terminal_page()),
                                            Some(chat_page()),
                                            Some(agents_page(agents.clone())),
                                            Some(tools_page(tools.clone())),
                                            ph_brand::enabled(ph_brand::Feature::ChromeExtension)
                                                .then(|| chrome_page(chrome.clone())),
                                            Some(github_page(accounts.clone())),
                                            Some(templates_page(templates.clone())),
                                            ph_brand::enabled(ph_brand::Feature::NetworkDrives)
                                                .then(|| drives_page(drives.clone())),
                                            Some(storage_page(storage.clone())),
                                            Some(features_page()),
                                            Some(shortcuts_page()),
                                            Some(about_page()),
                                        ]
                                        .into_iter()
                                        .flatten(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(TOP))
                            .left(px(SIDEBAR_WIDTH - 1.))
                            .w(px(2.))
                            .h((theme.radius_lg - px(5.)).max(px(0.)))
                            .bg(frame_color),
                    ),
            )
    });
}

/// The accent choices as swatches: the theme's own, then each color in the
/// shade the theme's mode shows.
fn accent_swatches(cx: &App) -> impl IntoElement + use<> {
    let dark = cx.theme().is_dark();
    let chosen = cx
        .try_global::<Preferences>()
        .map_or(Accent::Theme, |preferences| preferences.accent);
    let ring = cx.theme().foreground;
    let border = cx.theme().border;
    h_flex()
        .gap_1p5()
        .flex_wrap()
        .children(Accent::ALL.into_iter().map(move |accent| {
            let id = SharedString::from(format!("accent-{}", accent.label()));
            let selected = accent == chosen;
            let swatch = div()
                .id(id)
                .size(ph_ui::rems_from_px(20.))
                .rounded_full()
                .cursor_pointer()
                .border_2()
                .border_color(if selected {
                    ring
                } else {
                    gpui_kit::transparent_black()
                })
                .p(px(2.))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(accent.label()).build(window, cx)
                })
                .on_click(move |_, _, cx| AppState::update_preferences(cx, |p| p.accent = accent));
            match accent.color(dark) {
                Some(color) => swatch.child(div().size_full().rounded_full().bg(color)),
                // The theme's own: a ring of the border's color, empty.
                None => swatch.child(
                    div()
                        .size_full()
                        .rounded_full()
                        .border_1()
                        .border_color(border),
                ),
            }
        }))
}

/// The Theme dropdown's value for following the system.
const SYSTEM_THEME: &str = "system";

/// The Theme dropdown's choices: the system, PilotHouse's themes, then the
/// user's own (read again each time Settings opens).
fn theme_options(cx: &mut App) -> Vec<(SharedString, SharedString)> {
    ph_theme::themes::reload_user_themes(cx);
    let mut options = vec![(SYSTEM_THEME.into(), "Match system".into())];
    for (theme, mine) in ph_theme::themes::list(cx) {
        let label = if mine {
            format!("{} (yours)", theme.name)
        } else {
            theme.name.to_string()
        };
        options.push((theme.name.clone(), label.into()));
    }
    options
}

/// The settings list's width.
const SIDEBAR_WIDTH: f32 = 210.;
/// Room around the page, as around the workspace's panels.
const FRAME: f32 = 8.;
/// Room above the page and the list, under the title.
const TOP: f32 = 4.;

/// Every installed family (PilotHouse's own fonts among them).
fn font_options(cx: &App) -> Vec<(SharedString, SharedString)> {
    installed_fonts(cx)
        .into_iter()
        .map(|name| (SharedString::from(name.clone()), SharedString::from(name)))
        .collect()
}

/// The choices of a font setting: PilotHouse's own font first (saved as no
/// choice), then `extra`, then every installed family.
fn font_choices(
    default: &str,
    extra: Option<(&'static str, &'static str)>,
    installed: &[(SharedString, SharedString)],
) -> Vec<(SharedString, SharedString)> {
    std::iter::once((
        SharedString::default(),
        SharedString::from(format!("Default ({default})")),
    ))
    .chain(extra.map(|(family, label)| (family.into(), label.into())))
    .chain(installed.iter().cloned())
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

fn appearance_page(
    fonts: &[(SharedString, SharedString)],
    themes: &[(SharedString, SharedString)],
    workspace: gpui_kit::WeakEntity<crate::Workspace>,
) -> SettingPage {
    // The system's own font is not among the installed families on macOS
    // (its name is hidden there), so it gets a row of its own.
    let ui_fonts = font_choices(
        ph_theme::fonts::INTERFACE,
        Some((ph_theme::fonts::SYSTEM_INTERFACE, "System font")),
        fonts,
    );
    SettingPage::new("Appearance")
        .icon(Icon::new(Icons::Palette))
        .group(
            SettingGroup::new()
                .title("Layout")
                .item(
                    SettingItem::new("Density", choice(&Density::ALL, Density::label, |p| p.density, |p, v| p.density = v))
                        .description("How much room rows take in lists, the file tree and the chat.")
                        .keywords(["density", "compact", "spacing", "padding", "rows"]),
                )
                .item(
                    SettingItem::new(
                        "Names under the icons",
                        switch(|p| p.rail_names, |p, v| p.rail_names = v),
                    )
                    .description("In the column at the window's left edge. Off, a name shows when you point at its icon.")
                    .keywords(["layout", "labels", "tools", "tool icons", "icons", "activity bar"]),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Theme")
                .item(
                    SettingItem::new(
                        "Theme",
                        SettingField::scrollable_dropdown(
                            themes.to_vec(),
                            |cx| match AppState::appearance(cx) {
                                Appearance::Theme(name) => name.into(),
                                // Older choices are moved to names when the app starts.
                                _ => SYSTEM_THEME.into(),
                            },
                            |picked, cx| {
                                let appearance = if picked.as_ref() == SYSTEM_THEME {
                                    Appearance::System
                                } else {
                                    Appearance::Theme(picked.to_string())
                                };
                                AppState::set_appearance(appearance, cx);
                            },
                        ),
                    )
                    .description("Match system follows your computer's light or dark mode.")
                    .keywords(["dark", "light", "mode", "theme", "palette", "colors", "dracula", "one dark", "warm", "studio"]),
                )
                .item(
                    SettingItem::new("Accent", SettingField::render(|_, _, cx| accent_swatches(cx)))
                        .description("The color of focus, links and selection. The first one is the theme's own.")
                        .keywords(["accent", "color", "highlight", "tint"]),
                )
                .item(
                    SettingItem::new(
                        "Your own theme",
                        SettingField::render(move |_, _, _| {
                            let workspace = workspace.clone();
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("design-theme")
                                        .small()
                                        .icon(IconName::Palette)
                                        .label("Design with an agent")
                                        // Straight to the workspace: Settings is a dialog
                                        // over it, outside its actions' reach.
                                        .on_click(move |_, window, cx| {
                                            _ = workspace.update(cx, |workspace, cx| {
                                                workspace.design_theme(&crate::DesignTheme, window, cx)
                                            });
                                        }),
                                )
                                .child(
                                    Button::new("import-theme")
                                        .small()
                                        .ghost()
                                        .icon(IconName::ArrowDown)
                                        .label("Import")
                                        .tooltip("Import a VS Code color theme (.json)")
                                        .on_click(|_, _, cx| crate::theme_mods::import_vscode_file(cx)),
                                )
                                .child(
                                    Button::new("theme-folder")
                                        .small()
                                        .ghost()
                                        .icon(IconName::FolderOpen)
                                        .tooltip("Open the folder of your themes (JSON files)")
                                        .accessibility_label("Open themes folder")
                                        .on_click(|_, _, cx| {
                                            if let Some(dir) = ph_theme::themes::user_dir(cx) {
                                                let _ = std::fs::create_dir_all(&dir);
                                                cx.open_with_system(&dir);
                                            }
                                        }),
                                )
                        }),
                    )
                    .description("Describe the look you want in a chat and the agent builds the theme as you talk, or import a VS Code theme.")
                    // Its three buttons are wider than the room a row
                    // leaves beside its text: they go under it.
                    .layout(gpui_kit::Axis::Vertical)
                    .keywords(["theme", "design", "customize", "colors", "mod", "agent", "vs code", "import"]),
                )
                .item(
                    SettingItem::new("Corners", choice(&Corners::ALL, Corners::label, |p| p.corners, |p, v| p.corners = v))
                        .description("How rounded panels, buttons and cards are."),
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
                    .description("For menus, chat and labels.")
                    .keywords(["font", "typeface", "family"]),
                )
                .item(
                    SettingItem::new("Interface size", number(11.0, ph_theme::preferences::MAX_UI_FONT_SIZE as f64, 1.0, |p| p.ui_font_size as f64, |p, v| p.ui_font_size = v as f32))
                        .description("Text, icons and spacing grow and shrink together.")
                        .keywords(["font size", "zoom", "bigger", "smaller"]),
                ),
        )
}

fn editor_page(fonts: &[(SharedString, SharedString)]) -> SettingPage {
    let code_fonts = font_choices(ph_theme::fonts::CODE, None, fonts);
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
                    .description("For code, diffs and terminals.")
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
                )
                .item(
                    SettingItem::new(
                        "Auto save",
                        choice(
                            &ph_theme::AutoSave::ALL,
                            ph_theme::AutoSave::label,
                            |p| p.editor.auto_save,
                            |p, v| p.editor.auto_save = v,
                        ),
                    )
                    .description("Saves a file a moment after you stop typing, or when you leave the editor. A file that changed on disk meanwhile is never overwritten.")
                    .keywords(["autosave", "save", "write"]),
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
                    SettingItem::new(
                        "Shell",
                        SettingField::render(|_, _, cx| {
                            let (shell, _) = ph_env::default_shell();
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(shell.display().to_string())
                        }),
                    )
                    .description("New terminals start your system's default shell.")
                    .keywords(["shell", "powershell", "zsh", "bash"]),
                ),
        )
}

/// A page that is one view of its own. `keywords` are what Search finds it
/// by: a view has no titles for it to read, and without any the page is in
/// no search result.
fn view_page<V: gpui_kit::Render>(
    title: &'static str,
    icon: Icons,
    keywords: &'static [&'static str],
    view: gpui_kit::Entity<V>,
) -> SettingPage {
    SettingPage::new(title).icon(Icon::new(icon)).group(
        SettingGroup::new().item(
            SettingItem::render(move |_, _, _| div().w_full().py_1().child(view.clone()))
                .keywords(keywords.iter().copied()),
        ),
    )
}

/// Settings → MCP & skills: what every agent is given to work with.
fn tools_page(tools: gpui_kit::Entity<ph_agent::tools_view::ToolsView>) -> SettingPage {
    view_page(
        "MCP & skills",
        Icons::Plug,
        &["mcp", "server", "skill", "tool", "plugin"],
        tools,
    )
}

/// Settings → GitHub: GitHub and GitHub Enterprise accounts, for pull
/// requests.
fn github_page(accounts: gpui_kit::Entity<ph_pulls::AccountsView>) -> SettingPage {
    view_page(
        "GitHub",
        Icons::GitPullRequest,
        &[
            "github",
            "account",
            "token",
            "enterprise",
            "sign in",
            "pull request",
        ],
        accounts,
    )
}

/// Settings → Review templates (see [`ph_pulls::TemplatesView`]). Named for
/// what they are templates of: "Templates" alone said nothing in a list
/// that also has themes and run configurations.
fn templates_page(view: gpui_kit::Entity<ph_pulls::TemplatesView>) -> SettingPage {
    view_page(
        "Review templates",
        Icons::ScrollText,
        &["template", "review", "prompt", "pull request", "starter"],
        view,
    )
}

/// Settings → Storage (see [`crate::storage`]).
fn storage_page(view: gpui_kit::Entity<crate::storage::StorageView>) -> SettingPage {
    view_page(
        "Storage",
        Icons::HardDrive,
        &[
            "disk",
            "space",
            "clean",
            "clear",
            "clones",
            "worktrees",
            "tasks",
            "storage",
            "logs",
            "cache",
        ],
        view,
    )
}

/// Settings → Network drives (see [`crate::network_drives`]).
fn drives_page(view: gpui_kit::Entity<crate::network_drives::NetworkDrivesView>) -> SettingPage {
    view_page(
        "Network drives",
        Icons::Server,
        &[
            "network", "drive", "share", "ssh", "samba", "smb", "server", "mapped",
        ],
        view,
    )
}

/// Settings → Chrome extension (see [`ph_browser::ChromeExtensionView`]).
fn chrome_page(view: gpui_kit::Entity<ph_browser::ChromeExtensionView>) -> SettingPage {
    view_page(
        "Chrome extension",
        Icons::Globe,
        &[
            "chrome",
            "edge",
            "brave",
            "browser",
            "extension",
            "tab group",
        ],
        view,
    )
}

/// Settings → Agents: how a session opens, the agents found here (each with
/// what can be set for it) and those that are not, with how to install
/// them. What was found is remembered for a few seconds; *Check again*
/// asks anew.
fn agents_page(agents: gpui_kit::Entity<ph_agent::agents_view::AgentsView>) -> SettingPage {
    SettingPage::new("Agents")
        .icon(Icon::new(Icons::Bot))
        .title_suffix(|_, _| {
            div().pl_2().child(
                Button::new("agents-recheck")
                    .xsmall()
                    .ghost()
                    .icon(IconName::Redo)
                    .label("Check again")
                    .tooltip("Look again for agents installed or signed in since")
                    .on_click(|_, window, cx| {
                        ph_agent::installed::recheck(cx);
                        window.refresh();
                    }),
            )
        })
        .group(
            SettingGroup::new().item(
                SettingItem::render(move |_, _, _| div().w_full().py_1().child(agents.clone()))
                    .keywords(
                        [
                            "agent",
                            "install",
                            "sign in",
                            "cli",
                            "chat",
                            "account",
                            "command",
                            "environment",
                            "arguments",
                        ]
                        .into_iter()
                        .chain(
                            ph_agent::agents::AGENTS
                                .iter()
                                .flat_map(|spec| [spec.name, spec.id]),
                        ),
                    ),
            ),
        )
        .groups(web_pages_group())
}

/// Settings → Agents → Web pages: which browser agents open pages in first
/// (see [`ph_browser::preference`]). Only in a build with both browsers:
/// with one, there is nothing to choose.
fn web_pages_group() -> Option<SettingGroup> {
    use ph_brand::{Feature, enabled};
    use ph_browser::preference::{self, Preferred};
    if !(enabled(Feature::ChromeExtension) && enabled(Feature::Browser)) {
        return None;
    }
    Some(
        SettingGroup::new().title("Web pages").item(
            SettingItem::new(
                "Preferred browser",
                SettingField::dropdown(
                    Preferred::ALL
                        .iter()
                        .map(|browser| (browser.label().into(), browser.label().into()))
                        .collect(),
                    |cx| preference::preferred(AppState::store(cx)).label().into(),
                    |picked, cx| {
                        let Some(browser) = Preferred::ALL
                            .into_iter()
                            .find(|browser| browser.label() == picked.as_ref())
                        else {
                            return;
                        };
                        if let Err(err) = preference::set_preferred(AppState::store(cx), browser)
                        {
                            tracing::warn!("saving the preferred browser failed: {err:#}");
                        }
                    },
                ),
            )
            .description(
                "Where agents open web pages first. Chrome is your own, with your sign-ins, through the PilotHouse extension; when it is not connected, agents use the built-in browser.",
            )
            .keywords([
                "browser", "chrome", "built-in", "inbuilt", "preferred", "default", "web", "extension",
            ]),
        ),
    )
}

fn chat_page() -> SettingPage {
    let page = SettingPage::new("Chat")
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
                .description("How wide messages get in a large window.")
                .keywords(["chat", "width", "column"]),
            ),
        );
    if !ph_brand::enabled(ph_brand::Feature::QuickChat) {
        return page;
    }
    page.group(
                SettingGroup::new().title("Quick chat").item(
                    SettingItem::new(
                        "Shortcut",
                        SettingField::render(|_, _, _| crate::quick_chat::ShortcutRecorder),
                    )
                    .description(format!(
                        "Opens a small chat window over any app while {app} runs, and hides it again. Its chats are kept under Chats.{wayland}",
                        app = ph_brand::NAME,
                        // Only where it matters: a Wayland session gives
                        // an app no key of its own.
                        wayland = if cfg!(target_os = "linux") {
                            " On Wayland, give the command “pilothouse quick-chat” a shortcut in your system's keyboard settings instead."
                        } else {
                            ""
                        }
                    ))
                    .keywords(["quick", "chat", "shortcut", "hotkey", "global", "floating", "popup", "spotlight"]),
                ),
            )
}

fn features_page() -> SettingPage {
    SettingPage::new("Features")
        .icon(Icon::new(Icons::LayoutGrid))
        .group(
            SettingGroup::new()
                // A group's description shows only under a title.
                .title("Workspace tools")
                .description(
                    "Turn off the ones you do not use. Nothing runs for a tool until you open it.",
                )
                .items(ContextTab::available().map(|tab| {
                    SettingItem::new(
                        tab.label(),
                        SettingField::switch(
                            move |cx| AppState::enabled_tabs(cx).contains(&tab),
                            move |enabled, cx| AppState::set_tab_enabled(tab, enabled, cx),
                        ),
                    )
                    .description(tab.about())
                    .keywords(["feature", "tab", "panel", "tool"])
                })),
        )
        .group(system_group())
}

/// "Open with PilotHouse" in the file manager.
fn system_group() -> SettingGroup {
    use crate::shell_integration as shell;
    let group = SettingGroup::new().title("System");
    if !shell::SUPPORTED {
        // Nothing to switch on macOS: Finder offers it by itself. Said as
        // text across the row (as a value beside it, it ran off the page).
        return group.item(
            SettingItem::new(
                format!("Open with {}", ph_brand::NAME),
                SettingField::render(|_, _, _| div()),
            )
            .description(format!(
                "In Finder, right-click a folder or a file, then Open With → {}. Folders open as workspaces, files in the editor. From a terminal: pilothouse <path>.",
                ph_brand::NAME
            ))
            .layout(gpui_kit::Axis::Vertical)
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
                        .child(crate::shortcuts::render(true, window, cx))
                })
                .keywords(["shortcut", "keyboard", "key"]),
            ),
        )
}

fn about_page() -> SettingPage {
    let page = SettingPage::new("About")
        .icon(Icon::new(IconName::Info))
        .group(
            SettingGroup::new()
                .item(SettingItem::render(|_, _, cx| {
                    // A build with its own name says what it is built on.
                    let based_on = if ph_brand::NAME == "PilotHouse" {
                        ""
                    } else {
                        " · based on PilotHouse"
                    };
                    note(
                        format!(
                            "{} {}{based_on} · Apache-2.0",
                            ph_brand::NAME,
                            ph_ui::version()
                        ),
                        cx,
                    )
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
                                .on_click(move |_, _, cx| ph_ui::platform::reveal(&data, cx)),
                        )
                }))
                .item(SettingItem::render(|_, _, cx| {
                    note(
                        format!(
                            "Everything stays on this computer: there is no {} server.",
                            ph_brand::NAME
                        ),
                        cx,
                    )
                })),
        );
    // A build whose brand turns updates off has no Updates section; the
    // project's page is still linked.
    if !crate::updates::enabled() {
        return page.group(SettingGroup::new().item(SettingItem::render(|_, _, cx| {
            repository_row(
                format!("The project's page: {}", crate::updates::REPOSITORY_URL),
                "Open",
                cx,
            )
        })));
    }
    page.group(
            SettingGroup::new()
                .title("Updates")
                .item(SettingItem::render(|_, _, cx| update_status(cx)))
                .item(SettingItem::render(|_, _, cx| {
                    repository_row(
                        "Release notes, every download and the source are on GitHub.".to_string(),
                        "GitHub",
                        cx,
                    )
                }))
                .item(
                    SettingItem::new(
                        "Check for updates automatically",
                        SettingField::switch(
                            crate::updates::auto_check,
                            crate::updates::set_auto_check,
                        ),
                    )
                    .description("At start and every six hours, from the project's GitHub releases. Nothing about you is sent.")
                    .keywords(["update", "version", "release", "github", "mirror"]),
                )
                .item(
                    SettingItem::new(
                        "Install updates automatically",
                        SettingField::switch(
                            crate::updates::auto_install,
                            crate::updates::set_auto_install,
                        ),
                    )
                    .description("A new version downloads in the background and installs when you quit. Each download is verified against the release's signed manifest.")
                    .keywords(["update", "install", "download"]),
                ),
        )
}

/// A note and a button that opens the project's page (the brand's
/// `[links] repository`).
fn repository_row(text: String, label: &'static str, cx: &App) -> impl IntoElement + use<> {
    h_flex()
        .w_full()
        .justify_between()
        .gap_3()
        .child(note(text, cx))
        .child(
            Button::new("open-repository")
                .small()
                .outline()
                .icon(Icons::Github)
                .label(label)
                .on_click(|_, _, cx| cx.open_url(crate::updates::REPOSITORY_URL)),
        )
}

/// Where updates stand, with *Check now* (or *Restart to update*).
fn update_status(cx: &App) -> impl IntoElement + use<> {
    use crate::updates::{self, Status};
    use ph_brand::NAME;
    let status = updates::Updater::status(cx);
    let text = match &status {
        Status::Idle => "Not checked yet.".to_string(),
        Status::Checking => "Checking…".to_string(),
        Status::UpToDate => format!("{NAME} {} is the newest version.", ph_ui::version()),
        Status::Available { version } => format!("{NAME} {version} is available."),
        Status::Downloading { version } => match updates::Updater::progress(cx) {
            Some((done, total)) if total > 0 => format!(
                "Downloading {NAME} {version}… {}%",
                (done * 100 / total).min(100)
            ),
            _ => format!("Downloading {NAME} {version}…"),
        },
        Status::Ready { version, .. } => {
            format!("{NAME} {version} is ready: it installs when you quit.")
        }
        Status::Manual { version } => format!(
            "{NAME} {version} is available. This copy cannot update itself: download it from GitHub."
        ),
        Status::Failed(reason) => format!("The update failed: {reason}"),
    };
    let action = match status {
        Status::Ready { .. } => Button::new("restart-to-update")
            .small()
            .primary()
            .label("Restart to update")
            .on_click(|_, _, cx| updates::restart_to_update(cx)),
        Status::Available { .. } => Button::new("download-update")
            .small()
            .outline()
            .label("Download")
            .on_click(|_, _, cx| updates::download_update(cx)),
        Status::Manual { .. } => Button::new("open-release")
            .small()
            .outline()
            .label("Open GitHub")
            .on_click(|_, _, cx| cx.open_url(updates::RELEASES_URL)),
        Status::Checking | Status::Downloading { .. } => Button::new("check-updates")
            .small()
            .outline()
            .label("Check now")
            .loading(true),
        _ => Button::new("check-updates")
            .small()
            .outline()
            .label("Check now")
            .on_click(|_, _, cx| updates::check(true, cx)),
    };
    h_flex()
        .w_full()
        .justify_between()
        .gap_3()
        .child(note(text, cx))
        .child(action)
}

fn note(text: String, cx: &App) -> impl IntoElement + use<> {
    div()
        .w_full()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}
