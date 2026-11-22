//! The web view itself, through `wry` (macOS and Windows).

use anyhow::{Context as _, Result};
use gpui_kit::{Bounds, Pixels, Window};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebViewBuilder};

pub struct Native {
    view: wry::WebView,
}

/// The site of an address: its scheme, host and port.
fn site_of(url: &str) -> &str {
    let after = url.find("://").map(|at| at + 3).unwrap_or(0);
    let end = url[after..]
        .find(['/', '?', '#'])
        .map(|at| after + at)
        .unwrap_or(url.len());
    &url[..end]
}

impl Native {
    /// A web view on `url`. It stays on that address's site: what would
    /// leave it (a link on a slide, a window a page opens) is sent to
    /// `outside` instead, for the user's browser.
    pub fn new(
        url: &str,
        window: &mut Window,
        outside: smol::channel::Sender<String>,
    ) -> Result<Self> {
        let home = site_of(url).to_string();
        let elsewhere = outside.clone();
        let view = WebViewBuilder::new()
            .with_url(url)
            .with_navigation_handler(move |to| {
                if site_of(&to) == home || to.starts_with("about:") {
                    return true;
                }
                let _ = outside.try_send(to);
                false
            })
            .with_new_window_req_handler(move |to, _| {
                let _ = elsewhere.try_send(to);
                wry::NewWindowResponse::Deny
            })
            // Placed when its element is first painted.
            .with_bounds(rect(Bounds::default()))
            // It takes the keyboard when the user clicks in it, not by
            // being made: a view made for a picture of a page would bring
            // the app in front and take the keys from what is typed in.
            .with_focused(false)
            // A click in it while another window is in front acts at once,
            // as a click in the rest of the window does.
            .with_accept_first_mouse(true)
            // The page is the app's own: swiping does not leave it.
            .with_back_forward_navigation_gestures(false)
            .build_as_child(&*window)
            .context("the system's web view could not be made")?;
        Ok(Self { view })
    }

    pub fn load(&self, url: &str) {
        if let Err(err) = self.view.load_url(url) {
            tracing::warn!("web view: {url} did not load: {err}");
        }
    }

    pub fn evaluate(&self, script: &str) {
        if let Err(err) = self.view.evaluate_script(script) {
            tracing::warn!("web view: a script did not run: {err}");
        }
    }

    pub fn set_visible(&self, visible: bool) {
        _ = self.view.set_visible(visible);
    }

    pub fn focus(&self) {
        _ = self.view.focus();
    }

    pub fn release_keys(&self) {
        _ = self.view.focus_parent();
    }

    pub fn has_keys(&self) -> Option<bool> {
        crate::system::has_keys(&self.view)
    }

    pub fn press(&self, key: &crate::Key<'_>, front: bool) -> Result<bool> {
        crate::system::press(&self.view, key, front)
    }

    pub fn place(&self, bounds: Bounds<Pixels>) {
        _ = self.view.set_bounds(rect(bounds));
    }

    pub fn snapshot(
        &self,
        rect: Option<Bounds<Pixels>>,
    ) -> smol::channel::Receiver<Result<Vec<u8>>> {
        crate::system::snapshot(&self.view, rect)
    }

    pub fn pdf(&self, rect: Option<Bounds<Pixels>>) -> smol::channel::Receiver<Result<Vec<u8>>> {
        crate::system::pdf(&self.view, rect)
    }
}

impl Native {
    pub fn print(&self, page: gpui_kit::Size<Pixels>) -> smol::channel::Receiver<Result<Vec<u8>>> {
        crate::system::print(&self.view, page)
    }
}

pub use crate::system::join;

/// The app's points are the system's logical pixels.
fn rect(bounds: Bounds<Pixels>) -> Rect {
    Rect {
        position: LogicalPosition::new(
            f64::from(f32::from(bounds.origin.x)),
            f64::from(f32::from(bounds.origin.y)),
        )
        .into(),
        size: LogicalSize::new(
            f64::from(f32::from(bounds.size.width)),
            f64::from(f32::from(bounds.size.height)),
        )
        .into(),
    }
}
