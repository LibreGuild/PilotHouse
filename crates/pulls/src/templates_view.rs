//! Settings → Templates: what agents are asked to do with a pull request
//! (see [`ph_review::templates`]). Each template can be edited; a shipped
//! one can be put back as shipped or taken out of the list, and the user
//! can add their own.

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Window, div,
};
use ph_review::templates::{self, Template, TemplateKind};
use ph_store::Store;
use ph_ui::Icons;

/// The template being edited.
struct Editing {
    /// `None` for a new one.
    id: Option<String>,
    kind: TemplateKind,
    name: Entity<InputState>,
    description: Entity<InputState>,
    instructions: Entity<TextareaState>,
}

pub struct TemplatesView {
    store: Store,
    editing: Option<Editing>,
    error: Option<SharedString>,
}

impl TemplatesView {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            editing: None,
            error: None,
        }
    }

    fn edit(&mut self, template: Option<Template>, window: &mut Window, cx: &mut Context<Self>) {
        let template = template.unwrap_or(Template {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            kind: TemplateKind::Prompt,
            instructions: String::new(),
        });
        let mut text = |value: String, hint: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder(hint);
                input.set_value(value, window, cx);
                input
            })
        };
        let name = text(template.name, "Name, e.g. Release checklist", cx);
        let description = text(template.description, "One line about it", cx);
        let instructions = cx.new(|cx| {
            let mut area = TextareaState::new(window, cx)
                .placeholder("What the agent should do, and how its answer should look")
                .rows(16);
            area.set_value(template.instructions, window, cx);
            area
        });
        self.editing = Some(Editing {
            id: (!template.id.is_empty()).then_some(template.id),
            kind: template.kind,
            name,
            description,
            instructions,
        });
        self.error = None;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(editing) = &self.editing else { return };
        let name = editing.name.read(cx).value().trim().to_string();
        let id = editing
            .id
            .clone()
            .unwrap_or_else(|| templates::new_id(&self.store, &name));
        let template = Template {
            id,
            name,
            description: editing.description.read(cx).value().trim().to_string(),
            kind: editing.kind,
            instructions: editing.instructions.read(cx).value().to_string(),
        };
        match templates::save(&self.store, template) {
            Ok(()) => self.editing = None,
            Err(err) => self.error = Some(format!("{err:#}").into()),
        }
        cx.notify();
    }

    fn reset(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(err) = templates::reset(&self.store, id) {
            self.error = Some(format!("{err:#}").into());
        }
        self.editing = None;
        cx.notify();
    }

    fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(err) = templates::remove(&self.store, id) {
            self.error = Some(format!("{err:#}").into());
        }
        self.editing = None;
        cx.notify();
    }

    fn render_form(&self, editing: &Editing, cx: &mut Context<Self>) -> impl IntoElement {
        let builtin = editing.id.as_deref().is_some_and(templates::is_builtin);
        let edited = editing
            .id
            .as_deref()
            .is_some_and(|id| templates::is_edited(&self.store, id));
        let kind = editing.kind;
        let kind_button = |value: TemplateKind, label: &'static str, cx: &mut Context<Self>| {
            Button::new(SharedString::from(format!("template-kind-{label}")))
                .xsmall()
                .label(label)
                .when(kind == value, |this| this.primary())
                .when(kind != value, |this| this.ghost())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(editing) = &mut this.editing {
                        editing.kind = value;
                    }
                    cx.notify();
                }))
        };
        let hint = match kind {
            TemplateKind::Review => {
                "The reviewing guide: what to look for, what each severity means, what counts as evidence. PilotHouse adds the pull request, the repository's own rules (.pilothouse/review.md on the base branch) and the answer format its findings card reads, where each finding has the issue, its evidence and the fix."
            }
            TemplateKind::Prompt => {
                "A request about the pull request. PilotHouse adds where the PR is checked out and how to see its diff; the answer is Markdown, with Mermaid diagrams drawn as pictures."
            }
        };
        v_flex()
            .gap_2()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .child(Input::new(&editing.name).small())
            .child(Input::new(&editing.description).small())
            .when(!builtin, |this| {
                this.child(
                    h_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("It makes"),
                        )
                        .child(kind_button(
                            TemplateKind::Review,
                            "A review with findings",
                            cx,
                        ))
                        .child(kind_button(TemplateKind::Prompt, "An answer", cx)),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint),
            )
            .child(Textarea::new(&editing.instructions).small())
            .child(
                h_flex()
                    .gap_2()
                    .when(builtin && edited, |this| {
                        let id = editing.id.clone().unwrap_or_default();
                        this.child(
                            Button::new("template-reset")
                                .small()
                                .ghost()
                                .label("Put back as shipped")
                                .on_click(cx.listener(move |this, _, _, cx| this.reset(&id, cx))),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("template-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editing = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("template-save")
                            .small()
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

impl Render for TemplatesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = templates::all(&self.store);
        let hidden: Vec<Template> = templates::builtin()
            .into_iter()
            .filter(|shipped| !list.iter().any(|template| template.id == shipped.id))
            .collect();
        let editing_id = self.editing.as_ref().map(|editing| editing.id.clone());
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("What agents are asked to do with a pull request in the GitHub view. Each shows as a starter in a pull request's chat. A repository can add its own review rules in .pilothouse/review.md."),
            )
            .children(list.into_iter().map(|template| {
                let editing_this = editing_id.as_ref() == Some(&Some(template.id.clone()));
                if editing_this && let Some(editing) = &self.editing {
                    return self.render_form(editing, cx).into_any_element();
                }
                let (edit, remove) = (template.clone(), template.id.clone());
                let builtin = templates::is_builtin(&template.id);
                let edited = templates::is_edited(&self.store, &template.id);
                h_flex()
                    .id(SharedString::from(format!("template-{}", template.id)))
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                                            .child(template.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(match (template.kind, builtin, edited) {
                                                (TemplateKind::Review, true, false) => "Review · shipped",
                                                (TemplateKind::Review, true, true) => "Review · edited",
                                                (TemplateKind::Review, false, _) => "Review · yours",
                                                (TemplateKind::Prompt, true, false) => "Answer · shipped",
                                                (TemplateKind::Prompt, true, true) => "Answer · edited",
                                                (TemplateKind::Prompt, false, _) => "Answer · yours",
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(template.description.clone()),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("template-edit-{}", template.id)))
                            .xsmall()
                            .ghost()
                            .icon(Icons::Pencil)
                            .label("Edit")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit(Some(edit.clone()), window, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("template-remove-{}", template.id)))
                            .xsmall()
                            .ghost()
                            .icon(Icons::Trash)
                            .tooltip(if builtin {
                                "Take it out of the list (it can be put back)"
                            } else {
                                "Delete it"
                            })
                            .accessibility_label("Remove the template")
                            .on_click(cx.listener(move |this, _, _, cx| this.remove(&remove, cx))),
                    )
                    .into_any_element()
            }))
            .map(|this| match &self.editing {
                Some(editing) if editing.id.is_none() => this.child(self.render_form(editing, cx)),
                _ => this.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("template-new")
                                .small()
                                .outline()
                                .icon(Icons::Plus)
                                .label("New template")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.edit(None, window, cx)),
                                ),
                        )
                        .children(hidden.into_iter().map(|template| {
                            let id = template.id.clone();
                            Button::new(SharedString::from(format!("template-restore-{id}")))
                                .small()
                                .ghost()
                                .label(format!("Bring back “{}”", template.name))
                                .on_click(cx.listener(move |this, _, _, cx| this.reset(&id, cx)))
                        })),
                ),
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
    }
}
