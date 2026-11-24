//! The system's web view placed inside one of the app's windows, for pages
//! that are part of the app (a deck being edited), not for browsing.
//!
//! It is the system's own engine (WebKit on macOS, WebView2 on Windows),
//! drawn by the system straight into the window: typing and dragging in it
//! are as quick as in a browser, which a stream of pictures from a hidden
//! browser is not. Two things follow from it being the system's view and
//! not something the app draws:
//!
//! - It covers whatever the app draws in the same place. Menus and dialogs
//!   of the app cannot show over it, so the view is hidden while one is open
//!   (`set_visible`), and what belongs over the page is drawn by the page.
//! - Linux has no such view for a window that is not GTK's (`supported` is
//!   false there), and the page is shown another way.

use gpui_kit::{
    App, AppContext as _, Bounds, Context, DispatchPhase, Entity, IntoElement, MouseDownEvent,
    Pixels, Render, Styled as _, Task, Window, canvas,
};

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[path = "none.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "mac.rs"]
mod system;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod system;

/// Where the system's web view keeps what it keeps (its profile: a cache,
/// what pages store). Windows' WebView2 otherwise makes a folder beside the
/// program (`pilothouse.exe.WebView2`), which an update replaces and which
/// cannot be written to where the program is installed for every user:
/// there no web view could be made at all. Say it once, before the first
/// web view. (macOS keeps WebKit's own place.)
pub fn keep_data_in(folder: std::path::PathBuf) {
    let _ = DATA.set(folder);
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
static DATA: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// One PDF of the pages of `parts`, in order: each part is a PDF of its
/// own, as [`WebView::pdf`] makes them.
pub fn join_pdfs(parts: &[Vec<u8>]) -> anyhow::Result<Vec<u8>> {
    native::join(parts)
}

/// A key of a keyboard, for [`WebView::press`].
pub struct Key<'a> {
    /// What the key types with the modifiers held, and without them.
    pub characters: &'a str,
    pub plain: &'a str,
    /// The key's number on the keyboard.
    pub code: u16,
    pub command: bool,
    pub shift: bool,
    /// An arrow or another key of the function row or pad.
    pub function: bool,
}

/// Whether this system's web view makes a PDF by printing the whole page
/// ([`WebView::print`]), not part by part.
pub const fn prints_whole_pages() -> bool {
    cfg!(target_os = "windows")
}

/// Whether this system has a web view that can sit inside the app's windows.
pub const fn supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// A web view filling the place its element is given.
pub struct WebView {
    native: native::Native,
    /// Where it was last put, to move it only when its place changes.
    placed: Option<Bounds<Pixels>>,
    visible: bool,
}

impl WebView {
    /// A web view in `window`, showing `url`. It stays on that address's
    /// site; a link that leads elsewhere opens in the user's browser.
    pub fn new(url: &str, window: &mut Window, cx: &mut App) -> anyhow::Result<Entity<Self>> {
        let (outside, left) = smol::channel::unbounded::<String>();
        let native = native::Native::new(url, window, outside)?;
        Ok(cx.new(|cx| {
            cx.spawn(async move |_, cx| {
                while let Ok(url) = left.recv().await {
                    // Web pages only: nothing a page names may start a
                    // program or open a file.
                    if url.starts_with("https://") || url.starts_with("http://") {
                        cx.update(|cx| cx.open_url(&url));
                    }
                }
            })
            .detach();
            Self {
                native,
                placed: None,
                visible: true,
            }
        }))
    }

    /// Goes to another address.
    pub fn load(&self, url: &str) {
        self.native.load(url);
    }

    /// Runs a script in the page.
    pub fn evaluate(&self, script: &str) {
        self.native.evaluate(script);
    }

    /// Shows or hides it. Hidden, what the app draws in its place shows.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible != visible {
            self.visible = visible;
            if !visible {
                // What shows in its place is typed in.
                self.native.release_keys();
            }
            self.native.set_visible(visible);
            cx.notify();
        }
    }

    /// Gives it the keyboard.
    pub fn focus(&self) {
        self.native.focus();
    }

    /// Gives the keyboard back to the window around it. The system keeps
    /// the keys with the view last clicked in, and a click in the app's
    /// part of the window is not one the system sees as leaving the web
    /// view: without this, what is typed in the app would go on acting on
    /// the page. Done by itself at every click in the app's part of the
    /// window while the view is drawn.
    pub fn release_keys(&self) {
        self.native.release_keys();
    }

    /// Whether it has the keyboard, where the system says (macOS).
    pub fn has_keys(&self) -> Option<bool> {
        self.native.has_keys()
    }

    /// Presses `key` in the view's window as the system does (macOS): the
    /// event goes the way a real one goes, through the window's key
    /// equivalents and menus to whatever has the keyboard. For the app's
    /// own tests of who gets a key. With `front`, the window is put in
    /// front first, as it is when a person types. Answers whether the
    /// window was the one typed in.
    pub fn press(&self, key: &Key<'_>, front: bool) -> anyhow::Result<bool> {
        self.native.press(key, front)
    }

    /// A PNG of what the view shows in `rect` (in the view's own points;
    /// all of it without one), at the screen's density.
    pub fn snapshot(
        &self,
        rect: Option<Bounds<Pixels>>,
        cx: &App,
    ) -> Task<anyhow::Result<Vec<u8>>> {
        let answer = self.native.snapshot(rect);
        cx.background_spawn(async move {
            answer
                .recv()
                .await
                .map_err(|_| anyhow::anyhow!("the web view went away"))?
        })
    }

    /// `rect` of the page (in the page's own points; all of it without
    /// one) as a PDF of one page.
    pub fn pdf(&self, rect: Option<Bounds<Pixels>>, cx: &App) -> Task<anyhow::Result<Vec<u8>>> {
        let answer = self.native.pdf(rect);
        cx.background_spawn(async move {
            answer
                .recv()
                .await
                .map_err(|_| anyhow::anyhow!("the web view went away"))?
        })
    }

    /// The whole page printed to a PDF, on paper `page` large (in the
    /// page's points), without margins. Windows' way to a PDF: there the
    /// system prints a page whole ([`prints_whole_pages`]); elsewhere a
    /// PDF is made part by part ([`Self::pdf`], [`join_pdfs`]).
    pub fn print(&self, page: gpui_kit::Size<Pixels>, cx: &App) -> Task<anyhow::Result<Vec<u8>>> {
        let answer = self.native.print(page);
        cx.background_spawn(async move {
            answer
                .recv()
                .await
                .map_err(|_| anyhow::anyhow!("the web view went away"))?
        })
    }

    /// Puts it at `bounds` of the window. A web view that is drawn as an
    /// element is put where its element is; this is for one kept out of
    /// sight (beyond the window's edge) to take pictures of a page with.
    pub fn place(&mut self, bounds: Bounds<Pixels>) {
        if self.placed != Some(bounds) {
            self.placed = Some(bounds);
            self.native.place(bounds);
        }
    }
}

impl Render for WebView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        // Nothing is painted here: the system draws the view. Painting is
        // where the element's place is known, so that is where it is moved.
        canvas(
            |_, _, _| (),
            move |bounds, (), window, cx| {
                _ = view.update(cx, |view, _| view.place(bounds));
                // A click the app gets is one outside the web view (the
                // system gives the view its own): the keys follow it.
                let view = view.clone();
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture
                        && let Some(view) = view.upgrade()
                    {
                        view.read(cx).release_keys();
                    }
                });
            },
        )
        .size_full()
    }
}
