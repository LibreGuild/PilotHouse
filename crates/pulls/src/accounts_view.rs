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

pub struct AccountsView {
    store: Store,
    host: Entity<InputState>,
    token: Entity<InputState>,
    api_url: Entity<InputState>,
    accounts: Vec<Account>,
    adding: bool,
    error: Option<SharedString>,
    _task: Option<Task<()>>,
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
        Self {
            accounts: accounts::accounts(&store),
            store,
            host,
            token,
            api_url,
            adding: false,
            error: None,
            _task: None,
        }
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
                    Ok(_) => {
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
                                    ),
                            )
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
