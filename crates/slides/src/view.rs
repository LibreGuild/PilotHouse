//! A deck in a window: the system's web view showing the deck's page, which
//! is the editor. Where the system has no web view for the app's windows
//! (Linux), the same page opens in the user's browser, and this says so.
//!
//! The web view covers whatever the app draws in the same place, so while
//! a dialog of the app is open over it ([`DeckView::set_covered`]) it is
//! hidden, and the page's ground shows in its stead.

use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    v_flex,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, ImageSource, IntoElement, ParentElement as _, Pixels,
    Render, RenderImage, SharedString, Styled as _, Subscription, Task, Window, div, img, point,
    px, size,
};
use ph_ui::Icons;
use ph_webview::WebView;
use serde_json::Value;

/// How long a slide may take to be drawn for a picture before it is given
/// up on (its pictures are local; a deck of fifty heavy slides takes a
/// second or two).
const DRAW: Duration = Duration::from_secs(25);

pub struct DeckView {
    /// The deck's artifact, and its address on this computer.
    id: String,
    address: SharedString,
    web: Option<Entity<WebView>>,
    /// Why there is no web view, when there should be one.
    failed: Option<SharedString>,
    /// Something of the app's is drawn where the deck is (a dialog, a
    /// menu of the chat). The system draws the web view over everything
    /// the app draws, so it steps aside meanwhile, and a picture of it
    /// (`still`) stands in its place: the deck seems to stay.
    covered: bool,
    still: Option<Arc<RenderImage>>,
    /// Whether the deck had the keyboard when it stepped aside, to give
    /// it back after.
    had_keys: bool,
    _covering: Option<Task<()>>,
    /// Who waits for the deck to have stepped aside.
    aside: Vec<smol::channel::Sender<()>>,
    /// A second web view, out of sight, showing the deck without controls:
    /// pictures and PDFs are taken there, so the page the user works in is
    /// never disturbed. Made when first needed.
    render: Option<Entity<WebView>>,
    render_ready: bool,
    /// Who waits for the render page to be ready, and for a drawing it was
    /// asked for (by the token given with the asking).
    ready: Vec<smol::channel::Sender<()>>,
    drawn: HashMap<String, smol::channel::Sender<Value>>,
    tokens: u64,
    /// Which of a deck's views this is: its render page says it with all
    /// it tells, so that two views of one deck (its window's, and one
    /// drawing out of sight) never take each other's answers.
    key: &'static str,
    _decks: Option<Subscription>,
}

impl DeckView {
    /// The deck of artifact `id` at `address`, shown in `window`.
    pub fn new(id: &str, address: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The page is told it is in the app's own window.
        let shown = if address.contains('?') {
            format!("{address}&app=1")
        } else {
            format!("{address}?app=1")
        };
        let (web, failed) = if ph_webview::supported() {
            match WebView::new(&shown, window, cx) {
                Ok(web) => (Some(web), None),
                Err(err) => {
                    tracing::warn!("a deck's web view could not be made: {err:#}");
                    (None, Some(format!("{err:#}").into()))
                }
            }
        } else {
            (None, None)
        };
        Self::made(id, address, web, failed, "w", cx)
    }

    /// The deck of artifact `id` at `address`, not shown anywhere: a place
    /// to draw it (a picture, a PDF, what does not fit) while its window
    /// is not open. Whoever asks says which window to draw in, out of
    /// sight.
    pub fn hidden(id: &str, address: &str, cx: &mut Context<Self>) -> Self {
        Self::made(id, address, None, None, "h", cx)
    }

    fn made(
        id: &str,
        address: &str,
        web: Option<Entity<WebView>>,
        failed: Option<SharedString>,
        key: &'static str,
        cx: &mut Context<Self>,
    ) -> Self {
        let decks = crate::Decks::global(cx).map(|decks| {
            cx.subscribe(&decks, |this: &mut Self, _, event: &crate::DeckEvent, _| {
                if event.id == this.id {
                    this.told(&event.what);
                }
            })
        });
        Self {
            id: id.to_string(),
            address: address.to_string().into(),
            web,
            failed,
            key,
            covered: false,
            still: None,
            had_keys: false,
            _covering: None,
            aside: Vec::new(),
            render: None,
            render_ready: false,
            ready: Vec::new(),
            drawn: HashMap::new(),
            tokens: 0,
            _decks: decks,
        }
    }

    /// What the render page told the app: that it is ready, or that what
    /// it was asked for is drawn.
    fn told(&mut self, what: &Value) {
        // Another view's render page (see `key`).
        if what.get("key").is_some_and(|key| key != self.key) {
            return;
        }
        if what["render"] == "ready" {
            self.render_ready = true;
            for waiting in self.ready.drain(..) {
                let _ = waiting.try_send(());
            }
        } else if let Some(token) = what["rendered"].as_str()
            && let Some(waiting) = self.drawn.remove(token)
        {
            let _ = waiting.try_send(what.clone());
        }
    }

    /// Has the render page do something (`what` is a function of its
    /// `deckRender`, `about` what it is given before the token every call
    /// ends with) and answers with the web view and what the page told,
    /// once it is done.
    fn draw(
        &mut self,
        what: &'static str,
        about: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<(Entity<WebView>, Value)>> {
        let render = match &self.render {
            Some(render) => render.clone(),
            None => {
                if !ph_webview::supported() {
                    return Task::ready(Err(anyhow!(
                        "on this system the deck is drawn in your browser, where the app cannot take a picture of it"
                    )));
                }
                let made = WebView::new(
                    &format!("{}?render={}&app=1", self.address, self.key),
                    window,
                    cx,
                );
                let render = match made {
                    Ok(render) => render,
                    Err(err) => return Task::ready(Err(err)),
                };
                self.render = Some(render.clone());
                render
            }
        };
        // As large as one slide, so that a picture of it has the slide's
        // own pixels whatever the screen's density; and beyond the
        // window's left edge: drawn, and never seen. (WebKit takes
        // pictures and PDFs only of what is inside its view.)
        let deck = crate::Decks::global(cx)
            .and_then(|decks| decks.read(cx).slides().held(&self.id))
            .map(|live| live.deck().size)
            .unwrap_or(crate::deck::SIZE);
        let scale = window.scale_factor().max(1.);
        let slide = size(px(deck[0] as f32 / scale), px(deck[1] as f32 / scale));
        render.update(cx, |render, _| {
            render.place(Bounds::new(point(-slide.width - px(400.), px(0.)), slide))
        });
        let (is_ready, ready) = smol::channel::bounded(1);
        if self.render_ready {
            let _ = is_ready.try_send(());
        } else {
            self.ready.push(is_ready);
        }
        self.tokens += 1;
        let token = format!("{}{}", self.key, self.tokens);
        let (is_drawn, drawn) = smol::channel::bounded(1);
        self.drawn.insert(token.clone(), is_drawn);
        // Written as JSON, which is how a script writes a string too.
        let call = match about {
            Some(about) => format!(
                "window.deckRender.{what}({}, {token:?})",
                Value::from(about)
            ),
            None => format!("window.deckRender.{what}({token:?})"),
        };
        let timer = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            smol::future::or(
                async { ready.recv().await.context("the deck's page went away") },
                late(&timer, "the deck's page did not open in time"),
            )
            .await?;
            render.read_with(cx, |render, _| render.evaluate(&call));
            let told = smol::future::or(
                async { drawn.recv().await.context("the deck's page went away") },
                late(&timer, "the slide was not drawn in time"),
            )
            .await;
            if told.is_err() {
                _ = this.update(cx, |this, _| this.drawn.remove(&token));
            }
            let told = told?;
            if let Some(error) = told["error"].as_str() {
                bail!("the slide could not be drawn: {error}");
            }
            Ok((render, told))
        })
    }

    /// Where everything on every slide is, as the deck's page draws it: what
    /// an export to PowerPoint is made from.
    pub fn measure(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let drawn = self.draw("measure", None, window, cx);
        cx.background_spawn(async move {
            let (_, told) = drawn.await?;
            Ok(told["measured"].clone())
        })
    }

    /// What does not fit or cannot be read on each slide, as the deck's
    /// page finds it (the report an open editor makes by itself).
    pub fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<Result<Value>> {
        let drawn = self.draw("check", None, window, cx);
        cx.background_spawn(async move {
            let (_, told) = drawn.await?;
            Ok(told["report"].clone())
        })
    }

    /// A picture (PNG) of the slide whose file is `file`, as the deck's
    /// page draws it.
    pub fn picture(
        &mut self,
        file: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<u8>>> {
        let drawn = self.draw("show", Some(file), window, cx);
        cx.spawn(async move |_, cx| {
            let (render, told) = drawn.await?;
            let place = places(&told)
                .first()
                .copied()
                .context("the slide was not drawn")?;
            render
                .read_with(cx, |render, cx| render.snapshot(Some(place), cx))
                .await
        })
    }

    /// The deck as a PDF, a page a slide, drawn by the deck's page: text
    /// stays text. `files` are the slides' files, in order.
    pub fn pdf(
        &mut self,
        files: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<u8>>> {
        // The window is named, not looked up by this view: one that draws
        // out of sight is shown in no window.
        let host = window.window_handle();
        cx.spawn(async move |this, cx| {
            if files.is_empty() {
                bail!("the deck has no slides to export");
            }
            let this = this.upgrade().context("the deck was closed")?;
            if ph_webview::prints_whole_pages() {
                // The system prints the page whole: every slide is laid
                // out as a page of its own, and printed once.
                let drawn = host
                    .update(cx, |_, window, cx| {
                        this.update(cx, |this, cx| this.draw("pages", None, window, cx))
                    })
                    .map_err(|err| anyhow!("{err}"))?;
                let (render, told) = drawn.await?;
                let page = places(&told)
                    .first()
                    .map(|place| place.size)
                    .context("the slides were not drawn")?;
                return render
                    .read_with(cx, |render, cx| render.print(page, cx))
                    .await;
            }
            // A slide at a time: each is drawn to fill the view, and the
            // view made a page.
            let mut pages = Vec::new();
            for file in &files {
                let drawn = host
                    .update(cx, |_, window, cx| {
                        this.update(cx, |this, cx| this.draw("show", Some(file), window, cx))
                    })
                    .map_err(|err| anyhow!("{err}"))?;
                let (render, told) = drawn.await?;
                let place = places(&told)
                    .first()
                    .copied()
                    .context("a slide was not drawn")?;
                let page = render.read_with(cx, |render, cx| render.pdf(Some(place), cx));
                pages.push(page.await?);
            }
            cx.background_spawn(async move { ph_webview::join_pdfs(&pages) })
                .await
        })
    }

    /// Whether the deck shows here, or in the user's browser.
    pub fn embedded(&self) -> bool {
        self.web.is_some()
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    /// Something of the app's is to be drawn over the deck (a dialog, a
    /// menu): the web view, which the system draws over all of that, steps
    /// aside for a picture of itself. Done when the returned task is: what
    /// opens after it shows over the deck from its first frame.
    pub fn cover(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let (done, waited) = smol::channel::bounded::<()>(1);
        let wait = cx.background_spawn(async move {
            // Ends when the sender is let go of.
            let _ = waited.recv().await;
        });
        let Some(web) = self.web.clone() else {
            return wait;
        };
        if self.covered {
            if self._covering.is_some() {
                self.aside.push(done);
            }
            return wait;
        }
        self.covered = true;
        self.aside.push(done);
        // What opens is typed in: the keys go back to the window at once.
        self.had_keys = web.read(cx).has_keys().unwrap_or(false);
        web.read(cx).release_keys();
        let picture = web.read(cx).snapshot(None, cx);
        self._covering = Some(cx.spawn(async move |this, cx| {
            let still = match picture.await {
                Ok(png) => cx.background_spawn(async move { still_of(&png) }).await,
                Err(_) => None,
            };
            _ = this.update(cx, |this, cx| {
                this._covering = None;
                this.aside.clear();
                if this.covered {
                    this.still = still;
                    web.update(cx, |web, cx| web.set_visible(false, cx));
                    cx.notify();
                }
            });
        }));
        wait
    }

    /// Nothing of the app's is over the deck any more: the web view is
    /// back, with the keyboard if it had it.
    pub fn uncover(&mut self, cx: &mut Context<Self>) {
        if !self.covered {
            return;
        }
        self.covered = false;
        self._covering = None;
        self.aside.clear();
        self.still = None;
        if let Some(web) = &self.web {
            web.update(cx, |web, cx| web.set_visible(true, cx));
            if std::mem::take(&mut self.had_keys) {
                web.read(cx).focus();
            }
        }
        cx.notify();
    }

    /// Gives the deck the keyboard.
    pub fn focus(&self, cx: &gpui_kit::App) {
        if let Some(web) = &self.web {
            web.read(cx).focus();
        }
    }

    /// Whether the deck's page has the keyboard, where the system says.
    pub fn has_keys(&self, cx: &gpui_kit::App) -> Option<bool> {
        self.web.as_ref().and_then(|web| web.read(cx).has_keys())
    }

    /// Presses a key in the deck's window as the system does (the app's
    /// tests; see `ph_webview::WebView::press`).
    pub fn press(
        &self,
        key: &ph_webview::Key<'_>,
        front: bool,
        cx: &gpui_kit::App,
    ) -> Result<bool> {
        self.web
            .as_ref()
            .context("the deck is shown in your browser on this system")?
            .read(cx)
            .press(key, front)
    }

    /// A picture (PNG) of the deck's page as the window shows it now, its
    /// own controls included.
    pub fn screen(&self, cx: &gpui_kit::App) -> Task<Result<Vec<u8>>> {
        match &self.web {
            Some(web) => web.read(cx).snapshot(None, cx),
            None => Task::ready(Err(anyhow!(
                "the deck is shown in your browser on this system"
            ))),
        }
    }

    /// Runs a script in the deck's page.
    pub fn evaluate(&self, script: &str, cx: &gpui_kit::App) {
        if let Some(web) = &self.web {
            web.read(cx).evaluate(script);
        }
    }
}

/// Where the render page says it drew: `rects` of `[x, y, width, height]`.
fn places(told: &Value) -> Vec<Bounds<Pixels>> {
    told["rects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|rect| {
            let part = |at: usize| rect[at].as_f64().map(|value| px(value as f32));
            Some(Bounds::new(
                point(part(0)?, part(1)?),
                size(part(2)?, part(3)?),
            ))
        })
        .collect()
}

/// Fails with `what` once a drawing took too long.
async fn late<T>(timer: &gpui_kit::BackgroundExecutor, what: &'static str) -> Result<T> {
    timer.timer(DRAW).await;
    Err(anyhow!("{what}"))
}

impl Render for DeckView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let ground = div().size_full().bg(theme.muted);
        if let Some(web) = &self.web {
            // Stepped aside, the web view leaves its place to its picture.
            return ground
                .relative()
                .child(web.clone())
                .children(self.still.clone().map(|still| {
                    img(ImageSource::Render(still))
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                }));
        }
        let address = self.address.clone();
        let why = self.failed.clone().unwrap_or_else(|| {
            "On this system the deck opens in your browser. Edit it there; this chat works on the same deck.".into()
        });
        ground.child(
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .p_6()
                .child(
                    Icon::new(Icons::Presentation)
                        .size(ph_ui::rems_from_px(28.))
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .max_w(ph_ui::rems_from_px(420.))
                        .text_sm()
                        .text_center()
                        .text_color(theme.muted_foreground)
                        .child(why),
                )
                .child(
                    Button::new("deck-open-browser")
                        .small()
                        .primary()
                        .label("Open the deck in the browser")
                        .on_click(move |_, _, cx| cx.open_url(&address)),
                ),
        )
    }
}

/// A PNG as a picture the app draws.
fn still_of(png: &[u8]) -> Option<Arc<RenderImage>> {
    let mut pixels = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    // GPUI draws BGRA.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}
