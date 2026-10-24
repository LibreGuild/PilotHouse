//! The dialog for creating or editing a card.

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    form::{Field, Form},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    menu::PopupMenuItem,
    notification::Notification,
    select::{Select, SelectState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div,
};
use ph_store::Store;
use ph_store::board::{Card, CardEvent, CardPatch, CardStatus, NewCard};
use ph_store::projects::ProjectId;

use crate::BoardHub;
use ph_ui::MenuButton as _;

/// What the dialog edits: a card that does not exist yet, or an existing one.
#[derive(Clone)]
enum Target {
    New { project_id: ProjectId },
    Existing(Card),
}

impl Target {
    fn project_id(&self) -> ProjectId {
        match self {
            Target::New { project_id } => *project_id,
            Target::Existing(card) => card.project_id,
        }
    }
}

pub struct CardEditor {
    store: Store,
    target: Target,
    title: Entity<InputState>,
    description: Entity<TextareaState>,
    acceptance: Entity<TextareaState>,
    status: Entity<SelectState<Vec<SharedString>>>,
    events: Vec<CardEvent>,
    /// The UI tests this card needs to pass to be Done (test file names),
    /// each with its newest result.
    tests: Vec<(String, bool, String)>,
    /// The workspace's tests that can be linked: file name and name.
    available: Vec<(String, String)>,
}

/// A linked test and its newest result: passed, and in words.
fn with_status(project_id: ProjectId, slug: String) -> (String, bool, String) {
    let (passed, status) = ph_uitest::gate::status(project_id, &slug);
    (slug, passed, status)
}

impl CardEditor {
    fn new(store: Store, target: Target, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let card = match &target {
            Target::Existing(card) => Some(card.clone()),
            Target::New { .. } => None,
        };
        let title = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("What needs to happen?");
            if let Some(card) = &card {
                state.set_value(card.title.clone(), window, cx);
            }
            state
        });
        let description = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Details, context, links")
                .rows(4);
            if let Some(card) = &card {
                state.set_value(card.description.clone(), window, cx);
            }
            state
        });
        let acceptance = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("One criterion per line")
                .rows(3);
            if let Some(card) = &card {
                state.set_value(card.acceptance.clone(), window, cx);
            }
            state
        });
        let labels: Vec<SharedString> = CardStatus::ALL
            .iter()
            .map(|status| status.label().into())
            .collect();
        let selected = card
            .as_ref()
            .and_then(|card| {
                CardStatus::ALL
                    .iter()
                    .position(|status| *status == card.status)
            })
            .unwrap_or(0);
        let status = cx.new(|cx| {
            SelectState::new(
                labels,
                Some(gpui_kit::component::IndexPath::new(selected)),
                window,
                cx,
            )
        });
        let events = card
            .as_ref()
            .and_then(|card| store.card_events(card.id).ok())
            .unwrap_or_default();
        let tests = card
            .as_ref()
            .and_then(|card| store.card_tests(card.id).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|slug| with_status(target.project_id(), slug))
            .collect();
        let available = store
            .project(target.project_id())
            .ok()
            .flatten()
            .map(|project| ph_uitest::model::list(&project.path))
            .unwrap_or_default()
            .into_iter()
            .map(|(_, test)| (ph_uitest::model::slug(&test.name), test.name))
            .collect();
        // The dialog takes focus as it mounts, so focus the title right after.
        let focus = title.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |title, cx| title.focus(window, cx))
        });

        Self {
            store,
            target,
            title,
            description,
            acceptance,
            status,
            events,
            tests,
            available,
        }
    }

    fn project_id(&self) -> ProjectId {
        self.target.project_id()
    }

    /// Writes the card. Returns `false` (and keeps the dialog open) on failure.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let title = self.title.read(cx).value().trim().to_string();
        if title.is_empty() {
            window.push_notification(Notification::warning("A card needs a title."), cx);
            return false;
        }
        let description = self.description.read(cx).value().to_string();
        let acceptance = self.acceptance.read(cx).value().to_string();
        let status = self
            .status
            .read(cx)
            .selected_index(cx)
            .and_then(|ix| CardStatus::ALL.get(ix.row).copied());

        let result = match &self.target {
            Target::New { project_id } => self
                .store
                .add_cards(
                    *project_id,
                    &[NewCard {
                        title,
                        description,
                        acceptance,
                    }],
                )
                .and_then(|cards| match (cards.first(), status) {
                    (Some(card), Some(status)) if status != CardStatus::Todo => {
                        self.store.move_card(card.id, status, None).map(|_| ())
                    }
                    _ => Ok(()),
                }),
            Target::Existing(card) => {
                crate::may_finish(&self.store, card.id, status.unwrap_or(card.status))
                    .and_then(|()| {
                        self.store.update_card(
                            card.id,
                            &CardPatch {
                                title: Some(title),
                                description: Some(description),
                                acceptance: Some(acceptance),
                                status,
                                assignee: None,
                            },
                        )
                    })
                    .map(|_| ())
            }
        };
        match result {
            Ok(()) => {
                BoardHub::notify(self.project_id(), cx);
                true
            }
            Err(err) => {
                window.push_notification(
                    Notification::error(format!("Could not save card: {err:#}")),
                    cx,
                );
                false
            }
        }
    }

    /// Links (or unlinks) a test at once, without waiting for Save.
    fn link(&mut self, slug: String, linked: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Target::Existing(card) = &self.target else {
            return;
        };
        if let Err(err) = self.store.link_card_test(card.id, &slug, linked) {
            window.push_notification(Notification::error(format!("{err:#}")), cx);
            return;
        }
        self.tests
            .retain(|(linked_slug, _, _)| *linked_slug != slug);
        if linked {
            self.tests.push(with_status(card.project_id, slug));
            self.tests.sort();
        }
        BoardHub::notify(card.project_id, cx);
        cx.notify();
    }

    fn render_tests(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let editor = cx.weak_entity();
        let linkable: Vec<(String, String)> = self
            .available
            .iter()
            .filter(|(slug, _)| !self.tests.iter().any(|(linked, _, _)| linked == slug))
            .cloned()
            .collect();
        let name_of = |slug: &str| {
            self.available
                .iter()
                .find(|(available, _)| available == slug)
                .map_or_else(|| slug.to_string(), |(_, name)| name.clone())
        };
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child("Tests to pass before Done"),
                    )
                    .when(!linkable.is_empty(), |this| {
                        this.child(
                            Button::new("card-link-test")
                                .xsmall()
                                .ghost()
                                .icon(IconName::Plus)
                                .label("Link a test")
                                .menu(gpui_kit::Anchor::TopLeft, move |menu, _, _| {
                                    linkable.iter().fold(menu, |menu, (slug, name)| {
                                        let (editor, slug) = (editor.clone(), slug.clone());
                                        menu.item(PopupMenuItem::new(name.clone()).on_click(
                                            move |_, window, cx| {
                                                _ = editor.update(cx, |editor, cx| {
                                                    editor.link(slug.clone(), true, window, cx)
                                                });
                                            },
                                        ))
                                    })
                                }),
                        )
                    }),
            )
            .when(self.tests.is_empty(), |this| {
                this.child(div().text_xs().text_color(muted).child(if self.available.is_empty() {
                    "No UI tests in this workspace yet. Make one in the Tests tab, then link it here."
                } else {
                    "None: the card can be moved to Done any time."
                }))
            })
            .children(self.tests.iter().enumerate().map(|(ix, (slug, passed, status))| {
                let slug = slug.clone();
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(
                        Icon::new(if *passed { IconName::CircleCheck } else { IconName::CircleX })
                            .xsmall()
                            .text_color(if *passed { cx.theme().success } else { cx.theme().danger }),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(name_of(&slug)))
                    .child(div().text_color(muted).child(status.clone()))
                    .child(
                        Button::new(("card-unlink-test", ix))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .accessibility_label("Unlink test")
                            .on_click(cx.listener(move |editor, _, window, cx| {
                                editor.link(slug.clone(), false, window, cx)
                            })),
                    )
            }))
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Target::Existing(card) = &self.target else {
            return;
        };
        match self.store.delete_card(card.id) {
            Ok(()) => BoardHub::notify(card.project_id, cx),
            Err(err) => window.push_notification(
                Notification::error(format!("Could not delete card: {err:#}")),
                cx,
            ),
        }
    }
}

impl Render for CardEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_4()
            .child(
                Form::new()
                    .child(Field::new().label("Title").child(Input::new(&self.title)))
                    .child(
                        Field::new()
                            .label("Description")
                            .child(Textarea::new(&self.description)),
                    )
                    .child(
                        Field::new()
                            .label("Acceptance criteria")
                            .child(Textarea::new(&self.acceptance)),
                    )
                    .child(
                        Field::new()
                            .label("Column")
                            .child(Select::new(&self.status)),
                    ),
            )
            .when(matches!(self.target, Target::Existing(_)), |this| {
                this.child(self.render_tests(cx))
            })
            .when(!self.events.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child("Activity"),
                        )
                        .children(self.events.iter().map(|event| {
                            h_flex()
                                .gap_2()
                                .items_start()
                                .text_xs()
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(event.author.clone()),
                                )
                                .child(div().child(event.text.clone()))
                        })),
                )
            })
    }
}

/// Opens the editor for a new card in `project_id`.
pub fn open_new(store: Store, project_id: ProjectId, window: &mut Window, cx: &mut App) {
    open(store, Target::New { project_id }, "New card", window, cx);
}

/// Opens the editor for an existing card.
pub fn open_existing(store: Store, card: Card, window: &mut Window, cx: &mut App) {
    let title = format!("Card #{}", card.id);
    open(store, Target::Existing(card), &title, window, cx);
}

fn open(store: Store, target: Target, title: &str, window: &mut Window, cx: &mut App) {
    let is_existing = matches!(target, Target::Existing(_));
    let editor = cx.new(|cx| CardEditor::new(store, target, window, cx));
    let title: SharedString = title.to_string().into();
    window.open_dialog(cx, move |dialog, _, _| {
        let save_editor = editor.clone();
        let delete_editor = editor.clone();
        dialog
            .title(title.clone())
            .w(gpui_kit::px(520.))
            .child(editor.clone())
            .footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .when(is_existing, |this| {
                        this.child(
                            Button::new("delete-card")
                                .danger()
                                .ghost()
                                .label("Delete")
                                .on_click(move |_, window, cx| {
                                    delete_editor
                                        .update(cx, |editor, cx| editor.delete(window, cx));
                                    window.close_dialog(cx);
                                }),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("cancel-card")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(Button::new("save-card").primary().label("Save").on_click(
                        move |_, window, cx| {
                            if save_editor.update(cx, |editor, cx| editor.save(window, cx)) {
                                window.close_dialog(cx);
                            }
                        },
                    )),
            )
    });
}
