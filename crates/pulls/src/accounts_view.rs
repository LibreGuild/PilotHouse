//! Settings → GitHub: the accounts PilotHouse can use, one per host, and a
//! form to add one with a personal access token.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, div,
};
use ph_github::{Account, accounts};
use ph_store::Store;
use ph_ui::Icons;

/// Whether an account's token still works, as last checked.
#[derive(Clone)]
enum Health {
    Checking,
    Working,
    Failing(SharedString),
}

pub struct AccountsView {
    store: Store,
    host: Entity<InputState>,
    token: Entity<InputState>,
    api_url: Entity<InputState>,
    accounts: Vec<Account>,
    /// Each account's check, by id.
    health: std::collections::HashMap<String, Health>,
    /// The account just added, to confirm it.
    added: Option<SharedString>,
    adding: bool,
    error: Option<SharedString>,
    _task: Option<Task<()>>,
    _checks: Option<Task<()>>,
}

impl AccountsView {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let host = cx.new(|cx| {
            InputState::new(window, cx).placeholder("github.com or your GitHub Enterprise host")
        });
        let token = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Personal access token")
                .masked(true)
        });
        let api_url = cx.new(|cx| {
            InputState::new(window, cx).placeholder("API URL (optional, e.g. https://host/api/v3)")
        });
        let mut view = Self {
            accounts: accounts::accounts(&store),
            store,
            host,
            token,
            api_url,
            health: Default::default(),
            added: None,
            adding: false,
            error: None,
            _task: None,
            _checks: None,
        };
        view.check(cx);
        view
    }

    /// Asks each account's host who its token belongs to: a tick when it
    /// answers, why not when it does not.
    fn check(&mut self, cx: &mut Context<Self>) {
        let accounts = self.accounts.clone();
        self.health = accounts
            .iter()
            .map(|account| (account.id.clone(), Health::Checking))
            .collect();
        let work = cx.background_spawn(async move {
            std::thread::scope(|scope| {
                accounts
                    .iter()
                    .map(|account| {
                        let id = account.id.clone();
                        let check = scope.spawn(move || {
                            account
                                .client()
                                .and_then(|client| ph_github::pulls::me(&client))
                        });
                        (id, check)
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|(id, check)| {
                        let health = match check.join() {
                            Ok(Ok(_)) => Health::Working,
                            Ok(Err(err)) => Health::Failing(format!("{err:#}").into()),
                            Err(_) => Health::Failing("The check failed.".into()),
                        };
                        (id, health)
                    })
                    .collect::<Vec<_>>()
            })
        });
        self._checks = Some(cx.spawn(async move |this, cx| {
            let checked = work.await;
            _ = this.update(cx, |this, cx| {
                this.health.extend(checked);
                cx.notify();
            });
        }));
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.host.read(cx).value().trim().to_string();
        let host = if host.is_empty() {
            "github.com".to_string()
        } else {
            host
        };
        let token = self.token.read(cx).value().to_string();
        let api_url =
            Some(self.api_url.read(cx).value().trim().to_string()).filter(|url| !url.is_empty());
        let store = self.store.clone();
        self.adding = true;
        self.error = None;
        cx.notify();
        let work =
            cx.background_spawn(async move { accounts::add(&store, &host, &token, api_url) });
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            _ = this.update_in(cx, |this, window, cx| {
                this.adding = false;
                match result {
                    Ok(account) => {
                        this.added = Some(
                            format!("Connected to {} as {}.", account.host, account.login).into(),
                        );
                        this.health.insert(account.id.clone(), Health::Working);
                        this.accounts = accounts::accounts(&this.store);
                        for input in [&this.host, &this.token, &this.api_url] {
                            input.update(cx, |input, cx| input.set_value("", window, cx));
                        }
                    }
                    Err(err) => this.error = Some(format!("{err:#}").into()),
                }
                cx.notify();
            });
        }));
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        self.added = None;
        if let Err(err) = accounts::remove(&self.store, &id) {
            self.error = Some(format!("{err:#}").into());
        }
        self.accounts = accounts::accounts(&self.store);
        cx.notify();
    }
}

impl Render for AccountsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let host_text = self.host.read(cx).value().trim().to_string();
        let token_host = accounts::normalize_host(if host_text.is_empty() {
            "github.com"
        } else {
            &host_text
        })
        .unwrap_or_else(|_| "github.com".into());
        let token_url = format!(
            "https://{token_host}/settings/tokens/new?scopes=repo,read:org&description=PilotHouse"
        );
        let can_add = !self.token.read(cx).value().trim().is_empty() && !self.adding;
        v_flex()
            .w_full()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .children(self.accounts.iter().map(|account| {
                        let id = account.id.clone();
                        let health = self.health.get(&account.id).cloned();
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .child(Icon::new(Icons::GitPullRequest).small().text_color(cx.theme().info))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .child(div().text_sm().child(account.host.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("Signed in as {} · token in the system keychain", account.login)),
                                    )
                                    .when_some(
                                        match &health {
                                            Some(Health::Failing(why)) => Some(why.clone()),
                                            _ => None,
                                        },
                                        |this, why| {
                                            this.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().danger)
                                                    .child(why),
                                            )
                                        },
                                    ),
                            )
                            .child(health_mark(health.as_ref(), cx))
                            .child(
                                Button::new(SharedString::from(format!("remove-{}", account.id)))
                                    .small()
                                    .ghost()
                                    .label("Remove")
                                    .on_click(cx.listener(move |this, _, _, cx| this.remove(id.clone(), cx))),
                            )
                    }))
                    .when(self.accounts.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No accounts yet. Add one to see and review pull requests here."),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary)
                    .child(div().text_sm().font_weight(gpui_kit::FontWeight::MEDIUM).child("Add an account"))
                    .child(Input::new(&self.host).small())
                    .child(Input::new(&self.token).small())
                    .child(Input::new(&self.api_url).small())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("create-token")
                                    .small()
                                    .ghost()
                                    .icon(IconName::ExternalLink)
                                    .label("Create a token")
                                    .tooltip("Opens your GitHub's token page (scopes: repo, read:org)")
                                    .on_click(move |_, _, cx| cx.open_url(&token_url)),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("add-account")
                                    .small()
                                    .primary()
                                    .label(if self.adding { "Checking…" } else { "Add account" })
                                    .disabled(!can_add)
                                    .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                            ),
                    )
                    .when_some(self.added.clone(), |this, added| {
                        this.child(
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(cx.theme().success)
                                .child(Icon::new(IconName::CircleCheck).xsmall())
                                .child(added),
                        )
                    })
                    .when_some(self.error.clone(), |this, error| {
                        this.child(div().text_xs().text_color(cx.theme().danger).child(error))
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("PilotHouse checks the token with the host, then keeps it in the system keychain. It talks only to the hosts you add."),
                    ),
            )
    }
}

/// A tick when the token works, a warning when it does not, a spinner
/// while it is being checked.
fn health_mark(health: Option<&Health>, cx: &gpui_kit::App) -> gpui_kit::AnyElement {
    use gpui_kit::IntoElement as _;
    match health {
        Some(Health::Working) => Icon::new(IconName::CircleCheck)
            .small()
            .text_color(cx.theme().success)
            .into_any_element(),
        Some(Health::Failing(_)) => Icon::new(IconName::TriangleAlert)
            .small()
            .text_color(cx.theme().danger)
            .into_any_element(),
        Some(Health::Checking) => gpui_kit::component::spinner::Spinner::new()
            .xsmall()
            .into_any_element(),
        None => div().into_any_element(),
    }
}
