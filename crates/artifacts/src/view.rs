//! The *Artifacts* place of the window: everything the user had made, each
//! with a picture of its page, its name and when it last changed.
//!
//! A click opens the artifact in the system's browser, where it is used; one
//! of a kind with a window of its own (a deck) opens there, and the head of
//! the list offers to make a new one of each such kind.
//! The rest is in each one's menu (the ⋯, or a right click): open it beside
//! the chat of the workspace it came from, to change it there; copy its
//! address; show its folder; rename; go back a version; delete.

use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Subscription, Task, WeakEntity, Window, div, img,
};
use ph_ui::{Icons, MenuButton as _};

use crate::hub::{Artifacts, host};
use crate::library::{self, Artifact};

/// A search box only once the list is longer than a glance takes in.
const SEARCH_FROM: usize = 9;
/// A card's width in pixels at the default interface size; its picture is
/// as wide, at the proportions the pictures are kept in (16:10).
const CARD: f32 = 224.;
/// The width of a card that offers something new: smaller than what was
/// made, which is what the page is for.
const MAKE: f32 = 160.;

pub struct ArtifactsView {
    artifacts: Entity<Artifacts>,
    search: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl ArtifactsView {
    pub fn new(artifacts: Entity<Artifacts>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search artifacts"));
        Self {
            _subscriptions: vec![
                cx.observe(&artifacts, |_, _, cx| cx.notify()),
                cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                }),
            ],
            artifacts,
            search,
        }
    }

    /// The place came in front: the folder is the user's, and may have
    /// changed by hand since it was read.
    pub fn shown(&mut self, cx: &mut Context<Self>) {
        self.artifacts
            .update(cx, |artifacts, cx| artifacts.refresh(cx));
    }

    /// Says so when `work` fails; `what` names what was tried.
    fn watch(
        &mut self,
        what: &'static str,
        work: Task<anyhow::Result<()>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |_, cx| {
            if let Err(err) = work.await {
                _ = cx.update(|window, cx| {
                    window.push_notification(Notification::error(format!("{what}: {err:#}")), cx)
                });
            }
        })
        .detach();
    }

    /// The artifact's address, then `then` with it.
    fn with_address(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(String, &mut Window, &mut gpui_kit::App) + 'static,
    ) {
        let address = self
            .artifacts
            .update(cx, |artifacts, cx| artifacts.address(id, cx));
        let work = cx.spawn_in(window, async move |_, cx| {
            let address = address.await?;
            cx.update(|window, cx| then(address, window, cx))?;
            anyhow::Ok(())
        });
        self.watch("The artifact could not be opened", work, window, cx);
    }

    fn open_in_browser(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.with_address(id, window, cx, |address, _, cx| cx.open_url(&address));
    }

    /// Opens artifact `id` where it is used: the window of its kind when it
    /// has one, else the browser.
    fn open(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let artifact = self.artifacts.read(cx).find(id).cloned();
        match artifact.as_ref().and_then(|artifact| kind_of(artifact, cx)) {
            Some(kind) => (kind.open)(artifact.as_ref().expect("found above"), window, cx),
            None => self.open_in_browser(id, window, cx),
        }
    }

    /// Beside the chat of the workspace it was published from, to change it.
    fn open_in_workspace(
        &mut self,
        id: &str,
        project: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = host(cx) else {
            return;
        };
        // One of a kind (a deck) is looked at beside a chat, and worked on
        // in its own window: its page shows that way when asked to.
        let preview = self
            .artifacts
            .read(cx)
            .find(id)
            .is_some_and(|artifact| !artifact.manifest.kind.is_empty());
        self.with_address(id, window, cx, move |address, window, cx| {
            let address = if preview {
                format!("{address}?preview=1")
            } else {
                address
            };
            (host.open_in_workspace)(project, address, window, cx)
        });
    }

    fn copy_address(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.with_address(id, window, cx, |address, _, cx| {
            ph_ui::copy_with_toast(address, "the address", cx)
        });
    }

    /// The dialog that shares it with people on the same network.
    fn share(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        crate::share_view::ShareView::open(self.artifacts.clone(), id, window, cx);
    }

    fn rename(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(artifact) = self.artifacts.read(cx).find(id).cloned() else {
            return;
        };
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(artifact.manifest.title.clone(), window, cx);
            input
        });
        let (view, id, focus) = (cx.weak_entity(), id.to_string(), input.clone());
        window.open_dialog(cx, move |dialog, window, _| {
            let (view, id, input) = (view.clone(), id.clone(), input.clone());
            dialog
                .title("Rename the artifact")
                .w(ph_ui::scaled_px(420., window))
                .child(Input::new(&input))
                .footer(ph_ui::dialog_buttons("Rename", true))
                .on_ok(move |_, window, cx| {
                    let title = input.read(cx).value().trim().to_string();
                    if title.is_empty() {
                        return false;
                    }
                    _ = view.update(cx, |this, cx| {
                        let work = this
                            .artifacts
                            .update(cx, |artifacts, cx| artifacts.rename(&id, title, cx));
                        this.watch("The artifact could not be renamed", work, window, cx);
                    });
                    true
                })
        });
        focus.update(cx, |input, cx| input.focus(window, cx));
    }

    fn restore(&mut self, id: &str, version: u32, window: &mut Window, cx: &mut Context<Self>) {
        let work = self
            .artifacts
            .update(cx, |artifacts, cx| artifacts.restore(id, version, cx));
        self.watch(
            "The earlier version could not be put back",
            work,
            window,
            cx,
        );
    }

    fn delete(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(artifact) = self.artifacts.read(cx).find(id).cloned() else {
            return;
        };
        let (view, id) = (cx.weak_entity(), id.to_string());
        let title = format!("Delete “{}”?", artifact.manifest.title);
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (view, id) = (view.clone(), id.clone());
            dialog
                .confirm()
                .title(title.clone())
                .description(
                    "The page, its earlier versions and what it stored go to the Trash, where you can restore them. Its address stops working.",
                )
                .button_props(
                    gpui_kit::component::dialog::DialogButtonProps::default()
                        .ok_text("Move to Trash"),
                )
                .on_ok(move |_, window, cx| {
                    _ = view.update(cx, |this, cx| {
                        let work = this
                            .artifacts
                            .update(cx, |artifacts, cx| artifacts.delete(&id, cx));
                        this.watch("The artifact could not be deleted", work, window, cx);
                    });
                    true
                })
        });
    }

    /// The artifacts the search asks for (all of them without one).
    pub(crate) fn listed(&self, cx: &Context<Self>) -> Vec<Artifact> {
        let wanted = self.search.read(cx).value().trim().to_lowercase();
        self.artifacts
            .read(cx)
            .all()
            .iter()
            .filter(|artifact| {
                wanted.is_empty()
                    || artifact.manifest.title.to_lowercase().contains(&wanted)
                    || artifact
                        .manifest
                        .description
                        .to_lowercase()
                        .contains(&wanted)
            })
            .cloned()
            .collect()
    }

    /// A click that opens artifact `id`.
    fn opens(
        id: &str,
        cx: &mut Context<Self>,
    ) -> impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static {
        let id = id.to_string();
        cx.listener(move |this, _: &gpui_kit::ClickEvent, window, cx| this.open(&id, window, cx))
    }

    fn render_card(&self, artifact: &Artifact, cx: &mut Context<Self>) -> AnyElement {
        let (border, ring, backdrop, muted, radius, success) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.ring,
                theme.muted,
                theme.muted_foreground,
                theme.radius,
                theme.success,
            )
        };
        // Open to the network: said on the card, where it cannot be missed.
        let sharing = self.artifacts.read(cx).sharing(artifact.id());
        let id = artifact.id().to_string();
        let manifest = &artifact.manifest;
        let group = SharedString::from(format!("artifact-card-{id}"));
        let kind = kind_of(artifact, cx);
        let mut about = ph_ui::time::ago(manifest.updated as i64);
        if let Some(kind) = &kind {
            about = format!("{} · {about}", kind.label);
        }
        if let Some(source) = &manifest.source
            && !source.workspace.is_empty()
        {
            about.push_str(" · ");
            about.push_str(&source.workspace);
        }
        let picture = div()
            .id(SharedString::from(format!("artifact-{id}")))
            .role(gpui_kit::accesskit::Role::Button)
            .aria_label(SharedString::from(match &kind {
                Some(_) => format!("Open {}", manifest.title),
                None => format!("Open {} in the browser", manifest.title),
            }))
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h(ph_ui::rems_from_px(CARD * 10. / 16.))
            .rounded(radius)
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .group_hover(group.clone(), move |style| style.border_color(ring))
            .bg(backdrop)
            .cursor_pointer()
            .on_click(Self::opens(&id, cx))
            .map(|this| {
                if artifact.picture {
                    this.child(
                        // Rounded itself: a frame's corners do not cut
                        // a picture's.
                        img(artifact.thumbnail())
                            .size_full()
                            .rounded(radius)
                            .object_fit(ObjectFit::Cover),
                    )
                } else {
                    // Not shown in the built-in browser yet: no picture.
                    this.child(
                        Icon::new(kind.as_ref().map_or(Icons::Globe, |kind| kind.icon))
                            .size(ph_ui::rems_from_px(28.))
                            .text_color(muted.opacity(0.5)),
                    )
                }
            })
            .test_support();
        let name = v_flex()
            .id(SharedString::from(format!("artifact-name-{id}")))
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .on_click(Self::opens(&id, cx))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .truncate()
                    .child(manifest.title.clone()),
            )
            .child(match &sharing {
                Some(sharing) => h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(success)
                    .child(Icon::new(Icons::Wifi).xsmall())
                    .child(div().truncate().child(match sharing.guests {
                        0 => "Shared on your network".to_string(),
                        guests => format!("Shared · {guests} here"),
                    })),
                None => div().text_xs().text_color(muted).truncate().child(about),
            });
        let (view, for_menu) = (cx.weak_entity(), Rc::new(artifact.clone()));
        let (button_view, button_artifact) = (view.clone(), for_menu.clone());
        v_flex()
            .group(group)
            .w(ph_ui::rems_from_px(CARD))
            .gap_1p5()
            .child(picture)
            .child(
                h_flex().gap_1().items_start().child(name).child(
                    Button::new(SharedString::from(format!("artifact-more-{id}")))
                        .xsmall()
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .accessibility_label(format!("More for {}", manifest.title))
                        .menu(gpui_kit::Anchor::TopRight, move |menu, window, cx| {
                            artifact_menu(menu, button_view.clone(), &button_artifact, window, cx)
                        }),
                ),
            )
            .context_menu(move |menu, window, cx| {
                artifact_menu(menu, view.clone(), &for_menu, window, cx)
            })
            .into_any_element()
    }

    /// The card that offers a new artifact of `kind`, at the head of the
    /// list: as large as an artifact's card, with the kind's own picture
    /// where an artifact has its page's.
    fn render_make(
        &self,
        kind: &crate::hub::NewKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (border, ring, radius) = {
            let theme = cx.theme();
            (theme.border, theme.ring, theme.radius)
        };
        // What it is called: the kind alone. That it makes a new one is
        // said by its picture, and in words to whoever asks (the pointer
        // resting on it, a screen reader).
        let title = format!("New {}", kind.label.to_lowercase());
        let hint = SharedString::from(format!("{title}: {}", kind.hint));
        let group = SharedString::from(format!("artifact-make-{}", kind.kind));
        let create = kind.create.clone();
        let art = (kind.art)(MAKE, window, cx);
        v_flex()
            .id(SharedString::from(format!("artifact-new-{}", kind.kind)))
            .role(gpui_kit::accesskit::Role::Button)
            .aria_label(SharedString::from(title))
            .group(group.clone())
            .w(ph_ui::rems_from_px(MAKE))
            .gap_1p5()
            .cursor_pointer()
            .on_click(move |_, window, cx| create(window, cx))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(ph_ui::tip(hint.clone()))
                    .build(window, cx)
            })
            .test_support()
            .child(
                div()
                    .w_full()
                    .h(ph_ui::rems_from_px(MAKE * 10. / 16.))
                    .rounded(radius)
                    .overflow_hidden()
                    .border_1()
                    .border_color(border)
                    .group_hover(group, move |style| style.border_color(ring))
                    .child(art),
            )
            .child(
                div()
                    .w_full()
                    .text_center()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .truncate()
                    .child(kind.label),
            )
            .into_any_element()
    }

    fn render_empty(&self) -> AnyElement {
        let name = ph_brand::NAME;
        ph_ui::EmptyState::new("No artifacts yet")
            .icon(Icon::new(Icons::LayoutGrid))
            .description(format!(
                "An artifact is a page an agent made that {name} keeps and serves from this computer: a planner, a dashboard, a small game. In any chat, ask for “a {name} artifact”, or say “publish this as a {name} artifact” about a page you built."
            ))
            .into_any_element()
    }
}

/// The kind of `artifact`, when it is of one that has a window of its own.
fn kind_of(artifact: &Artifact, cx: &gpui_kit::App) -> Option<crate::hub::NewKind> {
    if artifact.manifest.kind.is_empty() {
        return None;
    }
    host(cx)?
        .kinds
        .into_iter()
        .find(|kind| kind.kind == artifact.manifest.kind)
}

/// What one artifact's menu offers.
fn artifact_menu(
    menu: PopupMenu,
    view: WeakEntity<ArtifactsView>,
    artifact: &Artifact,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let id = artifact.id().to_string();
    let item =
        |label: SharedString,
         icon: Icons,
         act: fn(&mut ArtifactsView, &str, &mut Window, &mut Context<ArtifactsView>)| {
            let (view, id) = (view.clone(), id.clone());
            PopupMenuItem::new(label)
                .icon(icon)
                .on_click(move |_, window, cx| {
                    _ = view.update(cx, |this, cx| act(this, &id, window, cx));
                })
        };
    // The workspace it was published from, while there still is one.
    let workspace = artifact.manifest.source.as_ref().and_then(|source| {
        let (name, _) = (host(cx)?.workspace)(source.project, cx)?;
        Some((source.project, name))
    });
    let versions = library::versions(artifact);
    let menu = menu
        // One of a kind opens in its own window first; the browser shows it
        // as the people it is shared with see it.
        .when_some(kind_of(artifact, cx), |menu, kind| {
            menu.item(item("Open".into(), kind.icon, ArtifactsView::open))
        })
        .item(item(
            "Open in the browser".into(),
            Icons::ExternalLink,
            ArtifactsView::open_in_browser,
        ))
        .when_some(workspace, |menu, (project, name)| {
            let (view, id) = (view.clone(), id.clone());
            menu.item(
                PopupMenuItem::new(format!("Open beside the chat in {name}"))
                    .icon(Icons::MessagesSquare)
                    .on_click(move |_, window, cx| {
                        _ = view.update(cx, |this, cx| {
                            this.open_in_workspace(&id, project, window, cx)
                        });
                    }),
            )
        })
        .item(item(
            "Copy its address".into(),
            Icons::Copy,
            ArtifactsView::copy_address,
        ))
        .item(item(
            "Share on your network…".into(),
            Icons::Wifi,
            ArtifactsView::share,
        ))
        .item({
            let folder = artifact.folder.clone();
            PopupMenuItem::new(ph_ui::platform::reveal_label())
                .icon(Icons::FolderOpen)
                .on_click(move |_, _, cx| ph_ui::platform::reveal(&folder, cx))
        })
        .separator()
        .item(item("Rename…".into(), Icons::Pencil, ArtifactsView::rename));
    let menu = if versions.is_empty() {
        menu
    } else {
        let (view, id) = (view.clone(), id.clone());
        // An earlier publish, put back in place of the live one.
        let icon = Some(Icon::new(Icons::RotateCcwClock));
        menu.submenu_with_icon(icon, "Go back to", window, cx, move |menu, _, _| {
            versions.iter().fold(menu, |menu, (version, when)| {
                let (view, id, version) = (view.clone(), id.clone(), *version);
                let ago = when
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| ph_ui::time::ago(since.as_secs() as i64))
                    .unwrap_or_default();
                menu.item(
                    PopupMenuItem::new(format!("Version {version}, replaced {ago}")).on_click(
                        move |_, window, cx| {
                            _ = view.update(cx, |this, cx| this.restore(&id, version, window, cx));
                        },
                    ),
                )
            })
        })
    };
    menu.separator()
        .item(item("Delete…".into(), Icons::Trash, ArtifactsView::delete))
}

impl Render for ArtifactsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (total, loaded, root) = {
            let artifacts = self.artifacts.read(cx);
            (
                artifacts.all().len(),
                artifacts.loaded(),
                artifacts.root().to_path_buf(),
            )
        };
        let listed = self.listed(cx);
        let cards: Vec<AnyElement> = listed
            .iter()
            .map(|artifact| self.render_card(artifact, cx))
            .collect();
        let muted = cx.theme().muted_foreground;
        // What can be made here, without an agent's publish: a card a
        // kind, in a row of their own above what was made.
        let kinds = host(cx).map(|host| host.kinds).unwrap_or_default();
        let makes: Vec<AnyElement> = kinds
            .iter()
            .map(|kind| self.render_make(kind, window, cx))
            .collect();
        v_flex()
            .id("artifacts")
            .size_full()
            .child(
                div()
                    .id("artifacts-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .when(!makes.is_empty(), |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_4()
                                .px_4()
                                .pt_4()
                                .pb_2()
                                .children(makes),
                        )
                    })
                    // What was made: how many, a search once they are many,
                    // and where they are kept.
                    .when(total > 0, |this| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .px_4()
                                .pt_3()
                                .pb_1()
                                .child(div().text_sm().text_color(muted).child(format!(
                                    "{total} artifact{}",
                                    if total == 1 { "" } else { "s" }
                                )))
                                .when(total >= SEARCH_FROM, |this| {
                                    this.child(
                                        div().w(ph_ui::rems_from_px(260.)).child(
                                            Input::new(&self.search)
                                                .small()
                                                .prefix(Icon::new(IconName::Search).xsmall()),
                                        ),
                                    )
                                })
                                .child(div().flex_1())
                                .child(
                                    Button::new("artifacts-folder")
                                        .small()
                                        .ghost()
                                        .icon(Icons::FolderOpen)
                                        .accessibility_label("Show the artifacts' folder")
                                        .tooltip("Show the folder they are kept in")
                                        .on_click(move |_, _, cx| {
                                            ph_ui::platform::reveal(&root, cx)
                                        }),
                                ),
                        )
                    })
                    .map(|this| {
                        if total == 0 {
                            // Nothing is said while the folder is first read.
                            this.when(loaded, |this| this.child(self.render_empty()))
                        } else if cards.is_empty() {
                            this.child(
                                div()
                                    .p_4()
                                    .text_sm()
                                    .text_color(muted)
                                    .child("No artifact has that in its name."),
                            )
                        } else {
                            this.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_4()
                                    .px_4()
                                    .pt_2()
                                    .pb_4()
                                    .children(cards),
                            )
                        }
                    }),
            )
            .test_support()
    }
}
