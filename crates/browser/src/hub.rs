//! One browser per project, shared by the pane and the control socket.

use std::collections::HashMap;

use gpui_kit::{App, AppContext as _, Entity, Global};
use ph_store::Store;

use crate::browser::Browser;

pub struct BrowserHub {
    store: Store,
    browsers: HashMap<i64, Entity<Browser>>,
}

impl Global for BrowserHub {}

impl BrowserHub {
    pub fn init(store: Store, cx: &mut App) {
        cx.set_global(Self {
            store,
            browsers: HashMap::new(),
        });
    }

    /// The project's browser, created (but not started) on first use.
    pub fn browser(project_id: i64, cx: &mut App) -> Entity<Browser> {
        if let Some(browser) = cx.global::<Self>().browsers.get(&project_id) {
            return browser.clone();
        }
        let store = cx.global::<Self>().store.clone();
        let browser = cx.new(|cx| Browser::new(project_id, store, cx));
        cx.global_mut::<Self>()
            .browsers
            .insert(project_id, browser.clone());
        browser
    }

    /// The browser only if it already exists (it is never started here).
    pub fn existing(project_id: i64, cx: &App) -> Option<Entity<Browser>> {
        cx.try_global::<Self>()?.browsers.get(&project_id).cloned()
    }
}
