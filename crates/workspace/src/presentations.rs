//! Making and finding presentations: the New presentation dialog (a
//! title, a size, a theme picked from pictures of each, and optionally
//! what the agent should draft) and the Open presentation dialog (every
//! presentation in the presentations folder, newest first, with a picture
//! of its first slide). Each presentation opens in a Studio window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Global, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, RenderImage, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, div, img, px,
};
use ph_studio::slides::model::{SIZES, SlideSize};
use ph_studio::slides::space::{self, Listed};
use ph_ui::Icons;

/// How long the list of presentations is trusted before it is read again.
const FRESH: Duration = Duration::from_secs(10);

/// The presentations as last read, and pictures of the themes.
#[derive(Default)]
struct Presentations {
    listed: Option<(Instant, Rc<Vec<Listed>>)>,
    /// The list is being read again (off the UI thread).
    listing: bool,
    /// A title slide in each theme, by theme and size (none while it is
    /// being drawn).
    previews: HashMap<(String, String), Option<Arc<RenderImage>>>,
}

impl Global for Presentations {}

/// Every presentation, newest first: as last read, read again now and then
/// off the UI thread (each deck's file is read for its title), the windows
/// drawn again when it changed.
pub(crate) fn listed(cx: &mut App) -> Rc<Vec<Listed>> {
    let state = cx.default_global::<Presentations>();
    let fresh = state
        .listed
        .as_ref()
        .is_some_and(|(at, _)| at.elapsed() < FRESH);
    let current = state
        .listed
        .as_ref()
        .map(|(_, listed)| listed.clone())
        .unwrap_or_default();
    if !fresh && !state.listing {
        state.listing = true;
        let read = cx.background_spawn(async { space::list(&space::root()) });
        cx.spawn(async move |cx| {
            let read = read.await;
            cx.update(|cx| {
                let state = cx.default_global::<Presentations>();
                state.listing = false;
                let key = |list: &[Listed]| -> Vec<(std::path::PathBuf, String, usize, std::time::SystemTime)> {
                    list.iter()
                        .map(|listed| (listed.space.path.clone(), listed.title.clone(), listed.slides, listed.modified))
                        .collect()
                };
                let changed = state
                    .listed
                    .as_ref()
                    .is_none_or(|(_, old)| key(old) != key(&read));
                state.listed = Some((Instant::now(), Rc::new(read)));
                if changed {
                    cx.refresh_windows();
                }
            });
        })
        .detach();
    }
    current
}

/// Reads the list again soon (it changed: one was made or opened); until
/// then it shows as it was.
fn forget_list(cx: &mut App) {
    let state = cx.default_global::<Presentations>();
    if let Some((at, _)) = state.listed.as_mut() {
        *at = Instant::now().checked_sub(FRESH * 2).unwrap_or(*at);
    }
}

/// Opens a presentation's window, saying so if it cannot.
pub(crate) fn open(folder: PathBuf, prompt: Option<String>, window: &mut Window, cx: &mut App) {
    forget_list(cx);
    if let Err(err) = crate::studio_window::open_presentation(folder, prompt, cx) {
        window.push_notification(Notification::error(format!("Not opened: {err:#}")), cx);
    }
}

/// What the New dialog has chosen.
struct Choice {
    size: String,
    /// A built-in theme's id, or a saved theme's name.
    theme: (bool, String),
}

/// The New presentation dialog.
pub(crate) fn new_dialog(window: &mut Window, cx: &mut App) {
    if !ph_brand::enabled(ph_brand::Feature::Presentations) {
        return;
    }
    let title = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("Title")
            .default_value("Untitled presentation")
    });
    title.update(cx, |input, cx| {
        input.focus(window, cx);
        input.select_all(window, cx);
    });
    let brief = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder(
                "Optional: what it is for and what to say. The agent drafts it when it opens.",
            )
            .auto_grow(2, 6)
    });
    let choice = Rc::new(RefCell::new(Choice {
        size: "16:9".into(),
        theme: (true, "clean".into()),
    }));
    let saved: Rc<Vec<String>> = Rc::new(
        space::saved_themes(&space::themes_dir())
            .into_iter()
            .map(|(name, _)| name)
            .collect(),
    );
    window.open_dialog(cx, move |dialog, window, cx| {
        let theme = cx.theme().clone();
        let (size, chosen) = {
            let choice = choice.borrow();
            (choice.size.clone(), choice.theme.clone())
        };
        let sizes = h_flex().flex_wrap().gap_1().children(SIZES.iter().map(|(id, name, _, _)| {
            let choice = choice.clone();
            Button::new(SharedString::from(format!("new-size-{id}")))
                .xsmall()
                .outline()
                .label(*name)
                .selected(size == *id)
                .on_click(move |_, window, _| {
                    choice.borrow_mut().size = id.to_string();
                    window.refresh();
                })
        }));
        let card = |builtin: bool, id: String, name: String, window: &mut Window, cx: &mut App| {
            let picture = preview(builtin, &id, &size, window, cx);
            let selected = chosen == (builtin, id.clone());
            let choice = choice.clone();
            v_flex()
                .id(SharedString::from(format!("new-theme-{id}")))
                .w(px(150.))
                .gap_1()
                .cursor_pointer()
                .on_click(move |_, window, _| {
                    choice.borrow_mut().theme = (builtin, id.clone());
                    window.refresh();
                })
                .child(
                    div()
                        .w(px(150.))
                        .h(px(150. * 9. / 16.))
                        .rounded(px(4.))
                        .overflow_hidden()
                        .border_2()
                        .border_color(if selected { theme.ring } else { theme.border })
                        .bg(theme.muted)
                        .children(picture.map(|picture| img(picture).size_full().object_fit(ObjectFit::Contain))),
                )
                .child(div().text_xs().truncate().child(name))
        };
        let mut cards: Vec<AnyElement> = ph_studio::slides::theme::builtins()
            .map(|(id, name)| card(true, id.to_string(), name.to_string(), window, cx).into_any_element())
            .collect();
        cards.extend(
            saved
                .iter()
                .map(|name| card(false, name.clone(), format!("{name} (yours)"), window, cx).into_any_element()),
        );
        let create = {
            let (title, brief, choice) = (title.clone(), brief.clone(), choice.clone());
            move |window: &mut Window, cx: &mut App| {
                let name = title.read(cx).value().trim().to_string();
                let name = if name.is_empty() { "Untitled presentation".to_string() } else { name };
                let prompt = brief.read(cx).value().trim().to_string();
                let choice = choice.borrow();
                let size = SlideSize::preset(&choice.size).unwrap_or_default();
                let mut deck = ph_studio::slides::ops::new_deck(&name, if choice.theme.0 { &choice.theme.1 } else { "clean" }, size.clone());
                if !choice.theme.0 {
                    let op = ph_studio::slides::Op::SetTheme {
                        theme: None,
                        custom: None,
                        saved: Some(choice.theme.1.clone()),
                        colors: serde_json::Value::Null,
                        fonts: None,
                        background: None,
                    };
                    if let Err(err) = ph_studio::slides::apply(&mut deck, op) {
                        window.push_notification(Notification::error(format!("{err:#}")), cx);
                    }
                }
                match space::Space::create(&space::root(), &deck) {
                    Ok(created) => {
                        window.close_dialog(cx);
                        let prompt = (!prompt.is_empty()).then(|| {
                            format!("Draft this presentation from what I want: {prompt}\n\nUse the theme's layouts, a title slide first, and add speaker notes. Show me the result.")
                        });
                        open(created.path, prompt, window, cx);
                    }
                    Err(err) => window.push_notification(Notification::error(format!("Not made: {err:#}")), cx),
                }
            }
        };
        dialog
            .title("New presentation")
            .w(px(720.))
            .child(
                v_flex()
                    .gap_3()
                    .child(Input::new(&title))
                    .child(div().text_sm().text_color(theme.muted_foreground).child("Size"))
                    .child(sizes)
                    .child(div().text_sm().text_color(theme.muted_foreground).child("Theme"))
                    .child(
                        div()
                            .id("new-themes")
                            .max_h(px(300.))
                            .overflow_y_scroll()
                            .child(div().flex().flex_wrap().gap_3().children(cards)),
                    )
                    .child(Textarea::new(&brief))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("Saved in {}; it saves itself as you work.", space::root().display())),
                    ),
            )
            .footer(
                DialogFooter::new()
                    .w_full()
                    .child(div().flex_1())
                    .child(Button::new("new-cancel").ghost().label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child({
                        let create = create.clone();
                        Button::new("new-create")
                            .primary()
                            .icon(Icons::Plus)
                            .label("Create")
                            .on_click(move |_, window, cx| create(window, cx))
                    }),
            )
    });
}

/// A title slide in a theme at a size, drawn once off the UI thread.
fn preview(
    builtin: bool,
    theme: &str,
    size: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<Arc<RenderImage>> {
    let key = (format!("{builtin}:{theme}"), size.to_string());
    if let Some(image) = cx.default_global::<Presentations>().previews.get(&key) {
        return image.clone();
    }
    let (theme, size) = (theme.to_string(), size.to_string());
    let task = cx.background_spawn(async move {
        let size = SlideSize::preset(&size).unwrap_or_default();
        let mut deck = ph_studio::slides::ops::new_deck(
            "Presentation title",
            if builtin { &theme } else { "clean" },
            size.clone(),
        );
        if !builtin {
            let theme = space::load_theme(&space::themes_dir(), &theme, &size).ok()?;
            ph_studio::slides::apply(
                &mut deck,
                ph_studio::slides::Op::SetTheme {
                    theme: None,
                    custom: Some(Box::new(theme)),
                    saved: None,
                    colors: serde_json::Value::Null,
                    fonts: None,
                    background: None,
                },
            )
            .ok()?;
        }
        if let Some(subtitle) = deck.slides.first().and_then(|slide| {
            slide
                .elements
                .iter()
                .find(|element| {
                    element.placeholder == Some(ph_studio::slides::model::PlaceholderKind::Subtitle)
                })
                .map(|element| element.id.clone())
        }) {
            let _ = ph_studio::slides::apply(
                &mut deck,
                ph_studio::slides::Op::SetText {
                    id: subtitle,
                    text: "A subtitle".into(),
                },
            );
        }
        let image = ph_studio::slides::raster::slide(&deck, 0, 300, None, false).ok()?;
        let (w, h) = image.dimensions();
        Some(ph_media::Frame::from_rgba(w, h, image.into_raw()).render_image())
    });
    // Drawn once; the dialog redraws when it is.
    cx.default_global::<Presentations>()
        .previews
        .insert(key.clone(), None);
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        let image = task.await;
        cx.update(|cx| {
            match image {
                Some(image) => {
                    cx.default_global::<Presentations>()
                        .previews
                        .insert(key, Some(image));
                }
                None => {
                    cx.default_global::<Presentations>().previews.remove(&key);
                }
            }
            _ = handle.update(cx, |_, window, _| window.refresh());
        });
    })
    .detach();
    None
}

/// The Open presentation dialog.
pub(crate) fn open_dialog(window: &mut Window, cx: &mut App) {
    if !ph_brand::enabled(ph_brand::Feature::Presentations) {
        return;
    }
    forget_list(cx);
    // First-slide pictures are rewritten as decks change: the ones GPUI
    // keeps by path are read again.
    for listed in listed(cx).iter() {
        let thumbnail = listed.space.path.join(space::THUMBNAIL);
        gpui_kit::ImageSource::from(thumbnail).remove_asset(cx);
    }
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search presentations"));
    search.update(cx, |input, cx| input.focus(window, cx));
    let watch = Rc::new(RefCell::new(None));
    *watch.borrow_mut() =
        Some(
            window.subscribe(&search, cx, |_, event: &InputEvent, window, _| {
                if matches!(event, InputEvent::Change) {
                    window.refresh();
                }
            }),
        );
    window.open_dialog(cx, move |dialog, _, cx| {
        let _keep = &watch;
        let theme = cx.theme().clone();
        let query = search.read(cx).value().trim().to_lowercase();
        let all = listed(cx);
        let shown: Vec<Listed> = all
            .iter()
            .filter(|listed| query.is_empty() || listed.title.to_lowercase().contains(&query))
            .cloned()
            .collect();
        let cards: Vec<AnyElement> = shown
            .into_iter()
            .enumerate()
            .map(|(ix, listed)| {
                let thumbnail = listed.space.path.join(space::THUMBNAIL);
                let folder = listed.space.path.clone();
                let when = listed
                    .modified
                    .elapsed()
                    .map(|elapsed| ago(elapsed.as_secs()))
                    .unwrap_or_default();
                v_flex()
                    .id(("open-presentation", ix))
                    .w(px(176.))
                    .gap_1()
                    .p_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.secondary_hover))
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        open(folder.clone(), None, window, cx);
                    })
                    .child(
                        div()
                            .w(px(168.))
                            .h(px(168. * 9. / 16.))
                            .rounded(px(4.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.muted)
                            .when(thumbnail.is_file(), |this| {
                                this.child(
                                    img(thumbnail).size_full().object_fit(ObjectFit::Contain),
                                )
                            }),
                    )
                    .child(div().text_sm().truncate().child(listed.title.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "{} slide{} · {when}",
                                listed.slides,
                                if listed.slides == 1 { "" } else { "s" }
                            )),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = all.is_empty();
        dialog
            .title("Open presentation")
            .w(px(780.))
            .child(
                v_flex().gap_3().child(Input::new(&search)).child(
                    div()
                        .id("open-presentations")
                        .max_h(px(440.))
                        .overflow_y_scroll()
                        .when(empty, |this| {
                            this.child(
                                div()
                                    .py_6()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child("No presentations yet: make one with New presentation."),
                            )
                        })
                        .child(div().flex().flex_wrap().gap_2().children(cards)),
                ),
            )
            .footer(
                DialogFooter::new()
                    .w_full()
                    .child(
                        Button::new("open-pptx")
                            .ghost()
                            .icon(Icons::FolderOpen)
                            .label("Open a PowerPoint file…")
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                pick_pptx(window, cx);
                            }),
                    )
                    .child(
                        Button::new("open-folder")
                            .ghost()
                            .label(if cfg!(target_os = "macos") {
                                "Show in Finder"
                            } else {
                                "Show folder"
                            })
                            .on_click(|_, _, cx| {
                                let root = space::root();
                                let _ = std::fs::create_dir_all(&root);
                                cx.reveal_path(&root);
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("open-new")
                            .primary()
                            .icon(Icons::Plus)
                            .label("New presentation")
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                new_dialog(window, cx);
                            }),
                    ),
            )
    });
}

/// A PowerPoint file, chosen, as a presentation.
fn pick_pptx(window: &mut Window, cx: &mut App) {
    let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Open".into()),
    });
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            _ = cx.update(|window, cx| open_powerpoint(path, window, cx));
        })
        .detach();
}

/// Opens PowerPoint file `path` as a presentation: the one opened from it
/// before when the file has not changed since, else read anew into a
/// space of its own.
pub fn open_powerpoint(path: std::path::PathBuf, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let made = cx
                .background_spawn(async move {
                    let root = space::root();
                    match space::find_imported(&root, &path) {
                        Some(found) => Ok((found, Vec::new())),
                        None => space::import_pptx(&root, &path),
                    }
                })
                .await;
            _ = cx.update(|window, cx| match made {
                Ok((created, notes)) => {
                    if !notes.is_empty() {
                        window.push_notification(
                            Notification::info(format!(
                                "Opened, with a few things to know: {}.",
                                notes.join("; ")
                            )),
                            cx,
                        );
                    }
                    open(created.path, None, window, cx)
                }
                Err(err) => window.push_notification(Notification::error(format!("{err:#}")), cx),
            });
        })
        .detach();
}

fn ago(seconds: u64) -> String {
    match seconds {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}
