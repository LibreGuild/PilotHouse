//! A file opened from outside every project gets a small window of its own:
//! just the file, in the editor (or the large-file viewer), like a notepad.
//! Opening the same file again brings its window forward. "Open folder as
//! project" hands the file to the main window inside its folder's project.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Size,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowBounds,
    WindowOptions, div, px,
};
use ph_files::{FileView, FilesChanged, ProjectWatcher};
use ph_theme::layout;

/// Open lone-file windows, by file.
#[derive(Default)]
struct LoneFiles(HashMap<PathBuf, AnyWindowHandle>);

impl Global for LoneFiles {}

/// Opens `path` (canonical, a file) in its own window, or focuses the
/// window it is already in.
pub fn open(path: PathBuf, cx: &mut App) -> Result<()> {
    let existing = cx
        .try_global::<LoneFiles>()
        .and_then(|open| open.0.get(&path).copied());
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    let mut options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            Size::new(px(900.), px(680.)),
            cx,
        ))),
        window_min_size: Some(Size::new(px(420.), px(300.))),
        ..TitleBar::window_options()
    };
    if let Some(titlebar) = options.titlebar.as_mut() {
        let (x, y) = layout::TRAFFIC_LIGHTS;
        titlebar.traffic_light_position = Some(gpui_kit::point(x, y));
    }
    let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
        ph_theme::apply_appearance(crate::AppState::appearance(cx), Some(window), cx);
        let path = path.clone();
        cx.new(|cx| LoneFile::new(path, window, cx))
    })
    .context("opening a window for the file")?;
    cx.default_global::<LoneFiles>().0.insert(path, handle);
    Ok(())
}

pub struct LoneFile {
    path: PathBuf,
    view: Entity<FileView>,
    _watcher: Entity<ProjectWatcher>,
    _subscriptions: Vec<Subscription>,
}

impl LoneFile {
    fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let folder = path.parent().map(PathBuf::from).unwrap_or_default();
        let name = file_name(&path);
        window.set_window_title(&name);
        let view = cx.new(|cx| {
            let mut view = FileView::new(&folder, &name, None, None, window, cx);
            view.standalone();
            view
        });
        let watcher = cx.new(|cx| ProjectWatcher::for_file(path.clone(), cx));
        let subscriptions = vec![
            cx.subscribe_in(&watcher, window, |this, _, _: &FilesChanged, window, cx| {
                this.view
                    .update(cx, |view, cx| view.reload_if_clean(window, cx));
            }),
            // Forget the window when it closes, so the file opens anew.
            cx.on_release({
                let path = path.clone();
                move |_, cx| {
                    cx.default_global::<LoneFiles>().0.remove(&path);
                }
            }),
        ];
        gpui_kit::Focusable::focus_handle(&view, cx).focus(window, cx);
        Self {
            path,
            view,
            _watcher: watcher,
            _subscriptions: subscriptions,
        }
    }

    /// Adds the file's folder as a project and shows the file there.
    fn open_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self
            .path
            .parent()
            .map(|folder| vec![folder.to_path_buf(), self.path.clone()]);
        let Some(paths) = paths else { return };
        // The main window takes over, so this one closes.
        window.remove_window();
        if let Err(err) = crate::open::open_paths(&paths, cx) {
            tracing::warn!("opening the folder failed: {err:#}");
        }
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

impl Render for LoneFile {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = file_name(&self.path);
        // The title bar does not shrink its content, so only the folder's
        // name is shown; the whole path is in its tooltip.
        let parent = self.path.parent();
        let folder = parent
            .and_then(|folder| folder.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let full = parent
            .map(ph_ui::platform::display_path)
            .unwrap_or_default();
        let (icon, tint) = ph_ui::file_icon(&name);
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                TitleBar::new().h(layout::TITLE_BAR_HEIGHT).child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .pr_3()
                        .gap_2()
                        .child(Icon::new(icon).small().text_color(tint))
                        .child(div().flex_none().text_sm().child(name))
                        .child(
                            div()
                                .id("lone-file-folder")
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("in {folder}"))
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(window, cx)
                                }),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("open-folder-as-project")
                                // Windows treats the bar as a drag area otherwise.
                                .occlude()
                                .flex_none()
                                .ghost()
                                .xsmall()
                                .icon(ph_ui::Icons::FolderOpen)
                                .label("Open folder")
                                .tooltip("Adds this file's folder to PilotHouse, with Git, chat and the rest")
                                .on_click(cx.listener(|this, _, window, cx| this.open_folder(window, cx))),
                        ),
                ),
            )
            .child(div().flex_1().min_h_0().child(self.view.clone()))
    }
}
