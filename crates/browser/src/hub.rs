//! One browser per project, shared by the pane and the control socket.

use std::collections::{HashMap, HashSet};

use gpui_kit::{App, AppContext as _, Entity, Global};
use ph_store::Store;

use crate::browser::Browser;

/// Shows workspace `project_id`'s browser to the user; says whether it did
/// (not while no window shows the workspace, or the user switched the
/// Browser off).
type ShowBrowser = std::rc::Rc<dyn Fn(i64, &mut App) -> bool>;

pub struct BrowserHub {
    store: Store,
    browsers: HashMap<i64, Entity<Browser>>,
    /// Workspaces whose browser was shown to the user because an agent used
    /// it, in this run (only the first use shows it), and what shows it.
    agent_used: HashSet<i64>,
    /// Workspaces whose showing is on its way (the agent's next calls must
    /// not queue it again).
    showing: HashSet<i64>,
    show_for_agent: Option<ShowBrowser>,
}

impl Global for BrowserHub {}

impl BrowserHub {
    pub fn init(store: Store, cx: &mut App) {
        cx.set_global(Self {
            store,
            browsers: HashMap::new(),
            agent_used: HashSet::new(),
            showing: HashSet::new(),
            show_for_agent: None,
        });
    }

    /// What shows a workspace's browser to the user (the workspace shell);
    /// it says whether it did. Without the hub (a build or test that never
    /// started the browser) there is nothing to show.
    pub fn on_agent_use(show: impl Fn(i64, &mut App) -> bool + 'static, cx: &mut App) {
        if cx.has_global::<Self>() {
            cx.global_mut::<Self>().show_for_agent = Some(std::rc::Rc::new(show));
        }
    }

    /// An agent acts in workspace `project_id`'s browser: the first time in
    /// this run, it is shown to the user, so they see what is going on.
    /// Until it could be shown (no window showed the workspace), each use
    /// tries again.
    pub(crate) fn agent_uses(project_id: i64, cx: &mut App) {
        let hub = cx.global_mut::<Self>();
        if hub.agent_used.contains(&project_id) || hub.showing.contains(&project_id) {
            return;
        }
        let Some(show) = hub.show_for_agent.clone() else {
            return;
        };
        hub.showing.insert(project_id);
        cx.defer(move |cx| {
            let shown = show(project_id, cx);
            let hub = cx.global_mut::<Self>();
            hub.showing.remove(&project_id);
            if shown {
                hub.agent_used.insert(project_id);
            }
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

    /// Workspace `project_id` was removed: its Chrome stops, and its profile
    /// (cookies, logins) is deleted once Chrome has let go of it.
    pub fn remove(project_id: i64, cx: &mut App) {
        if cx.has_global::<Self>()
            && let Some(browser) = cx.global_mut::<Self>().browsers.remove(&project_id)
        {
            browser.update(cx, |browser, cx| browser.stop(cx));
        }
        let profile = crate::chrome::profile_dir(project_id);
        cx.background_spawn(async move {
            for _ in 0..10 {
                smol::Timer::after(std::time::Duration::from_millis(500)).await;
                if !profile.exists() || std::fs::remove_dir_all(&profile).is_ok() {
                    return;
                }
            }
            tracing::warn!("could not delete the browser profile {}", profile.display());
        })
        .detach();
    }

    /// The browser only if it already exists (it is never started here).
    pub fn existing(project_id: i64, cx: &App) -> Option<Entity<Browser>> {
        cx.try_global::<Self>()?.browsers.get(&project_id).cloned()
    }
}
