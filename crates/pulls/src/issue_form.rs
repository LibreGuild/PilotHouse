//! Opening an issue: its title, its text and who it is assigned to, in a
//! dialog. Used for an issue about a review finding, and for a new issue
//! from a repository's Issues list.
//!
//! The people who can be assigned are read from GitHub when the dialog
//! opens (the repository's assignees, the first hundred); the account
//! itself is offered first, since "I'll take it" is the common case.

use std::collections::BTreeSet;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, div,
};
use ph_github::models::User;

use crate::hosting::Hosting;

#[derive(Debug, Clone)]
pub enum IssueFormEvent {
    /// Open it: the title, the text, and the logins it is assigned to.
    Submit {
        title: String,
        body: String,
        assignees: Vec<String>,
    },
}

/// Who can be assigned, as far as it is known.
enum People {
    Loading,
    Known(Vec<User>),
    /// They could not be read (the token may not list them): the issue can
    /// still be opened, assigned to nobody.
    Unknown(SharedString),
}

/// People shown before the search box is used.
const SHOWN: usize = 8;

pub struct IssueForm {
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    search: Entity<InputState>,
    people: People,
    picked: BTreeSet<String>,
    me: String,
    /// Where a private security advisory is opened instead, for a security
    /// problem in a public repository (an issue would tell everyone).
    advisory: Option<String>,
    _load: Task<()>,
    _search: Subscription,
}

impl EventEmitter<IssueFormEvent> for IssueForm {}

impl IssueForm {
    /// `advisory`: the address of the repository's new-advisory page, when
    /// the issue is about a security problem (the form then finds out
    /// whether the repository is public, and warns).
    pub fn new(
        hosting: &Hosting,
        title: &str,
        body: &str,
        advisory: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title_input = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        title_input.update(cx, |input, cx| input.set_value(title, window, cx));
        let body_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("What is wrong, and how to see it (Markdown)")
                .auto_grow(5, 12)
        });
        body_input.update(cx, |input, cx| input.set_value(body, window, cx));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find a person"));
        let (client, repo) = (hosting.client.clone(), hosting.repo.clone());
        let security = advisory.is_some();
        let load = cx.background_spawn(async move {
            let people = ph_github::issues::assignable(&client, &repo);
            // Only a public repository's issue discloses a security problem.
            let public = security
                && ph_github::pulls::repository(&client, &repo)
                    .map(|repository| !repository.private)
                    .unwrap_or(true);
            (people, public)
        });
        Self {
            title: title_input,
            body: body_input,
            _search: cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            search,
            people: People::Loading,
            picked: BTreeSet::new(),
            me: hosting.account.login.clone(),
            advisory: None,
            _load: cx.spawn(async move |this, cx| {
                let (people, public) = load.await;
                _ = this.update(cx, |this, cx| {
                    this.people = match people {
                        Ok(people) => People::Known(people),
                        Err(err) => People::Unknown(format!("{err:#}").into()),
                    };
                    this.advisory = advisory.filter(|_| public);
                    cx.notify();
                });
            }),
        }
    }

    /// Shows `form` in a dialog titled `title`.
    pub fn open(
        form: &Entity<Self>,
        title: &'static str,
        window: &mut Window,
        cx: &mut gpui_kit::App,
    ) {
        let view = form.clone();
        window.open_dialog(cx, move |dialog, window, _| {
            dialog
                .title(title)
                .w(ph_ui::scaled_px(560., window))
                // Enter in the title is not "OK": the form has its own button.
                .on_ok(|_, _, _| false)
                .child(view.clone())
        });
        form.update(cx, |form, cx| {
            form.title.update(cx, |input, cx| input.focus(window, cx))
        });
    }

    fn toggle(&mut self, login: &str, cx: &mut Context<Self>) {
        if !self.picked.remove(login) {
            self.picked.insert(login.to_string());
        }
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let title = self.title.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        cx.emit(IssueFormEvent::Submit {
            title,
            body: self.body.read(cx).value().trim().to_string(),
            assignees: self.picked.iter().cloned().collect(),
        });
    }

    /// The people offered: those picked, the account, then the rest, cut to
    /// what the search asks for.
    fn offered(&self, cx: &Context<Self>) -> (Vec<String>, usize) {
        let People::Known(people) = &self.people else {
            return (Vec::new(), 0);
        };
        let wanted = self.search.read(cx).value().trim().to_lowercase();
        let mut logins: Vec<String> = people.iter().map(|user| user.login.clone()).collect();
        // Chosen ones stay in sight, the account comes first of the others.
        logins.sort_by_key(|login| {
            (
                !self.picked.contains(login),
                !login.eq_ignore_ascii_case(&self.me),
                login.to_lowercase(),
            )
        });
        if !wanted.is_empty() {
            logins.retain(|login| {
                self.picked.contains(login) || login.to_lowercase().contains(&wanted)
            });
            return (logins, 0);
        }
        let shown = SHOWN.max(self.picked.len());
        let more = logins.len().saturating_sub(shown);
        logins.truncate(shown);
        (logins, more)
    }
}

impl Render for IssueForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (offered, more) = self.offered(cx);
        let total = match &self.people {
            People::Known(people) => people.len(),
            _ => 0,
        };
        let label = |text: &'static str| {
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(text)
        };
        let empty_title = self.title.read(cx).value().trim().is_empty();
        let advisory = self.advisory.clone();
        v_flex()
            .id("issue-form")
            .gap_3()
            .when_some(advisory.clone(), |this, advisory| {
                this.child(
                    v_flex()
                        .gap_1p5()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().warning.opacity(0.1))
                        .text_sm()
                        .child("This is a security problem and the repository is public: an issue would tell everyone. A private security advisory is safer.")
                        .child(
                            h_flex().child(
                                Button::new("issue-advisory")
                                    .small()
                                    .primary()
                                    .label("Open an advisory")
                                    .on_click(move |_, window, cx| {
                                        cx.open_url(&advisory);
                                        window.close_dialog(cx);
                                    }),
                            ),
                        ),
                )
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Title"))
                    .child(Input::new(&self.title)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(label("Description"))
                    .child(Textarea::new(&self.body)),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(label("Assign to"))
                    .map(|this| match &self.people {
                        People::Loading => this.child(
                            h_flex()
                                .gap_2()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(ph_ui::Loader::new().xsmall())
                                .child("Reading who can be assigned…"),
                        ),
                        People::Unknown(why) => this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "Could not read who can be assigned ({why}). The issue is opened assigned to nobody."
                                )),
                        ),
                        People::Known(_) => this
                            // A search only where the list is cut short.
                            .when(total > SHOWN, |this| {
                                this.child(
                                    Input::new(&self.search)
                                        .small()
                                        .prefix(Icon::new(IconName::Search).xsmall()),
                                )
                            })
                            .child(
                                h_flex()
                                    .gap_1()
                                    .flex_wrap()
                                    .children(offered.into_iter().map(|login| {
                                        let picked = self.picked.contains(&login);
                                        let me = login.eq_ignore_ascii_case(&self.me);
                                        h_flex()
                                            .id(SharedString::from(format!("issue-assign-{login}")))
                                            .role(gpui_kit::accesskit::Role::CheckBox)
                                            .aria_label(SharedString::from(format!(
                                                "Assign to {login}"
                                            )))
                                            .gap_1()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_full()
                                            .border_1()
                                            .text_sm()
                                            .cursor_pointer()
                                            .map(|this| {
                                                if picked {
                                                    this.border_color(cx.theme().primary)
                                                        .bg(cx.theme().primary.opacity(0.14))
                                                } else {
                                                    this.border_color(cx.theme().border)
                                                        .hover(|style| style.bg(cx.theme().list_hover))
                                                }
                                            })
                                            .when(picked, |this| {
                                                this.child(Icon::new(IconName::Check).xsmall())
                                            })
                                            .child(if me {
                                                format!("{login} (you)")
                                            } else {
                                                login.clone()
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.toggle(&login, cx)
                                            }))
                                            .test_support()
                                    }))
                                    .when(more > 0, |this| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(format!("and {more} more: type a name")),
                                        )
                                    })
                                    .when(total == 0, |this| {
                                        this.child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Nobody can be assigned in this repository."),
                                        )
                                    }),
                            ),
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("issue-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("issue-submit")
                            .small()
                            .map(|this| {
                                if self.advisory.is_some() {
                                    this.danger().label("Open a public issue anyway")
                                } else {
                                    this.primary().label("Open issue")
                                }
                            })
                            .disabled(empty_title)
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
            .test_support()
    }
}
