//! Mermaid diagrams, drawn by a Rust renderer (no browser): in Markdown
//! code blocks and in `.mmd` files.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::{
    AppContext as _, Context, Entity, Image, ImageFormat, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Task, Window, div,
};

use crate::image_view::ImagePreview;

/// A drawn diagram: an SVG and its size in pixels.
#[derive(Clone)]
pub struct Diagram {
    pub image: Arc<Image>,
    pub width: f32,
    pub height: f32,
}

/// Diagrams drawn lately, by source and theme: a document is parsed again
/// on every edit, and drawing is the slow part. The oldest goes first when
/// it is full (emptying it would draw every diagram again at once).
#[derive(Default)]
struct Drawn {
    diagrams: HashMap<(String, bool), Kept>,
    order: std::collections::VecDeque<(String, bool)>,
}

/// A diagram kept, and when it was last asked for.
struct Kept {
    drawn: Result<Diagram, String>,
    used: Instant,
}

static DRAWN: LazyLock<Mutex<Drawn>> = LazyLock::new(Mutex::default);
const KEEP: usize = 64;
/// The most the pictures of the kept diagrams may weigh. GPUI draws a
/// diagram it shows at twice its size, four bytes a pixel, and holds those
/// pixels for as long as the diagram is kept here: every version of a
/// large diagram being typed added fifteen megabytes, sixty-four of them a
/// gigabyte.
const KEEP_BYTES: u64 = 128 * 1024 * 1024;
/// A diagram asked for this lately is on screen. It is never pushed out:
/// it would be drawn again at once, and push out another.
const IN_USE: Duration = Duration::from_secs(2);
/// Diagrams pushed out of [`DRAWN`], whose pictures GPUI still holds until
/// [`release_evicted`] lets them go (on the UI thread).
static EVICTED: LazyLock<Mutex<Vec<Arc<Image>>>> = LazyLock::new(Mutex::default);
/// The diagrams being drawn for [`draw_soon`].
static DRAWING: LazyLock<Mutex<std::collections::HashSet<(String, bool)>>> =
    LazyLock::new(Mutex::default);

impl Drawn {
    /// What the pictures of the kept diagrams weigh once GPUI has drawn
    /// them.
    fn bytes(&self) -> u64 {
        self.diagrams
            .values()
            .filter_map(|kept| kept.drawn.as_ref().ok())
            .map(|diagram| 16 * diagram.width as u64 * diagram.height as u64)
            .sum()
    }

    /// Pushes out the oldest diagrams not in use at `now`, until those
    /// kept are within the limits; gives back their pictures.
    fn trim(&mut self, now: Instant) -> Vec<Arc<Image>> {
        let mut pushed_out = Vec::new();
        let mut ix = 0;
        while ix < self.order.len() && (self.order.len() > KEEP || self.bytes() > KEEP_BYTES) {
            let in_use = self
                .diagrams
                .get(&self.order[ix])
                .is_some_and(|kept| now.duration_since(kept.used) < IN_USE);
            if in_use {
                ix += 1;
            } else if let Some(key) = self.order.remove(ix)
                && let Some(Kept {
                    drawn: Ok(diagram), ..
                }) = self.diagrams.remove(&key)
            {
                pushed_out.push(diagram.image);
            }
        }
        pushed_out
    }
}

/// Lets go of the pictures of diagrams no longer cached.
pub fn release_evicted(cx: &mut gpui_kit::App) {
    let evicted = EVICTED
        .lock()
        .map(|mut evicted| std::mem::take(&mut *evicted))
        .unwrap_or_default();
    for image in evicted {
        ph_ui::images::release_image(image, cx);
    }
}

/// The drawing of `source` made before, if one.
pub fn cached(source: &str, dark: bool) -> Option<Result<Diagram, String>> {
    let mut drawn = DRAWN.lock().ok()?;
    let kept = drawn.diagrams.get_mut(&(source.to_string(), dark))?;
    kept.used = Instant::now();
    Some(kept.drawn.clone())
}

/// Draws `source` for a dark or light background.
pub fn draw(source: &str, dark: bool) -> Result<Diagram, String> {
    if let Some(found) = cached(source, dark) {
        return found;
    }
    let drawn = draw_uncached(source, dark);
    if let Ok(mut cache) = DRAWN.lock() {
        let key = (source.to_string(), dark);
        let kept = Kept {
            drawn: drawn.clone(),
            used: Instant::now(),
        };
        if cache.diagrams.insert(key.clone(), kept).is_none() {
            cache.order.push_back(key);
        }
        let pushed_out = cache.trim(Instant::now());
        if !pushed_out.is_empty()
            && let Ok(mut evicted) = EVICTED.lock()
        {
            evicted.extend(pushed_out);
        }
    }
    drawn
}

/// Draws `source` off the UI thread (once, however often it is asked for
/// meanwhile), then draws the windows again; until then, [`cached`] has
/// nothing for it. `wait` first lets typing go on (a diagram half typed is
/// never drawn). Every window, not the one view that asked: several may
/// wait for the same diagram, and a block of a Markdown view has no view
/// of its own to tell.
pub fn draw_soon(source: String, dark: bool, wait: bool, cx: &mut gpui_kit::App) {
    let key = (source, dark);
    if !DRAWING
        .lock()
        .is_ok_and(|mut drawing| drawing.insert(key.clone()))
    {
        return;
    }
    cx.spawn(async move |cx| {
        if wait {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
        }
        let drawn = key.clone();
        cx.background_spawn(async move {
            let _ = draw(&drawn.0, drawn.1);
        })
        .await;
        if let Ok(mut drawing) = DRAWING.lock() {
            drawing.remove(&key);
        }
        cx.update(|cx| {
            release_evicted(cx);
            cx.refresh_windows();
        });
    })
    .detach();
}

fn draw_uncached(source: &str, dark: bool) -> Result<Diagram, String> {
    use mermaid_rs_renderer::{RenderOptions, Theme};
    let mut theme = if dark { Theme::dark() } else { Theme::modern() };
    // The pane's own background shows through.
    theme.background = "transparent".into();
    let options = RenderOptions {
        theme,
        ..RenderOptions::default()
    };
    // The renderer has panicked on odd input before: a diagram that cannot
    // be drawn says so, never takes the app down.
    let svg = std::panic::catch_unwind(|| {
        mermaid_rs_renderer::render_with_options(source.trim(), options)
    })
    .map_err(|_| "The diagram could not be drawn.".to_string())?
    .map_err(|err| first_line(&format!("{err:#}")))?;
    let (width, height) = svg_size(&svg).ok_or("The diagram came out empty.")?;
    Ok(Diagram {
        image: Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())),
        width,
        height,
    })
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).trim().to_string()
}

/// The `width` and `height` of an SVG's root element (else its viewBox).
pub fn svg_size(svg: &str) -> Option<(f32, f32)> {
    let start = svg.find("<svg")?;
    let tag = &svg[start..start + svg[start..].find('>')?];
    let attribute = |name: &str| -> Option<&str> {
        let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        let end = tag[at..].find('"')?;
        Some(&tag[at..at + end])
    };
    let number =
        |text: &str| -> Option<f32> { text.trim_end_matches("px").trim().parse::<f32>().ok() };
    if let (Some(width), Some(height)) = (
        attribute("width").and_then(number),
        attribute("height").and_then(number),
    ) && width > 0.
        && height > 0.
    {
        return Some((width, height));
    }
    let view_box: Vec<f32> = attribute("viewBox")?
        .split([' ', ','])
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    match view_box.as_slice() {
        [_, _, width, height] if *width > 0. && *height > 0. => Some((*width, *height)),
        _ => None,
    }
}

/// A `.mmd` file: its diagram, zoomable, or why it cannot be drawn.
pub struct MermaidPreview {
    image: Entity<ImagePreview>,
    error: Option<SharedString>,
    _draw: Option<Task<()>>,
}

impl MermaidPreview {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            image: cx.new(|cx| ImagePreview::new(None, cx)),
            error: None,
            _draw: None,
        }
    }

    pub fn set_text(&mut self, text: Arc<str>, cx: &mut Context<Self>) {
        use gpui_kit::component::ActiveTheme as _;
        let dark = cx.theme().is_dark();
        let draw = cx.background_spawn(async move { draw(&text, dark) });
        self._draw = Some(cx.spawn(async move |this, cx| {
            let drawn = draw.await;
            _ = this.update(cx, |this, cx| {
                match drawn {
                    Ok(diagram) => {
                        this.error = None;
                        this.image
                            .update(cx, |image, cx| image.show_diagram(diagram, cx));
                    }
                    Err(err) => this.error = Some(err.into()),
                }
                // Every pause in typing draws a diagram: the ones this
                // pushed out of the cache give their pictures back.
                release_evicted(cx);
                cx.notify();
            });
        }));
    }
}

impl Render for MermaidPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::ActiveTheme as _;
        use gpui_kit::prelude::FluentBuilder as _;
        div()
            .relative()
            .size_full()
            .child(self.image.clone())
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .absolute()
                        .top_2()
                        .left_2()
                        .right_2()
                        .px_3()
                        .py_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().danger.opacity(0.12))
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_a_flowchart() {
        let diagram = draw(
            "flowchart LR\n  A[Start] --> B{Ok?}\n  B -->|yes| C[Done]",
            false,
        )
        .expect("drawn");
        assert!(diagram.width > 10. && diagram.height > 10.);
        assert!(draw("not a diagram at all", true).is_err());
    }

    #[test]
    fn kept_diagrams_stay_within_a_weight() {
        let then = Instant::now();
        // 2000 × 2000: 64 MB once drawn.
        let diagram = |name: &str| Diagram {
            image: Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                name.as_bytes().to_vec(),
            )),
            width: 2000.,
            height: 2000.,
        };
        let mut drawn = Drawn::default();
        for name in ["first", "second", "third"] {
            let key = (name.to_string(), false);
            let kept = Kept {
                drawn: Ok(diagram(name)),
                used: then,
            };
            drawn.diagrams.insert(key.clone(), kept);
            drawn.order.push_back(key);
        }
        // All on screen: none goes, though they weigh too much together.
        assert!(drawn.trim(then + Duration::from_secs(1)).is_empty());
        // The first still on screen later, the others not: the oldest of
        // those goes, and with it the weight is within the limit.
        let later = then + Duration::from_secs(10);
        if let Some(kept) = drawn.diagrams.get_mut(&("first".to_string(), false)) {
            kept.used = later;
        }
        let pushed_out = drawn.trim(later);
        assert_eq!(pushed_out.len(), 1);
        assert_eq!(pushed_out[0].bytes(), b"second");
        let kept: Vec<&str> = drawn.order.iter().map(|key| key.0.as_str()).collect();
        assert_eq!(kept, ["first", "third"]);
    }

    #[test]
    fn reads_svg_sizes() {
        assert_eq!(
            svg_size(r#"<svg xmlns="x" width="120" height="40px">"#),
            Some((120., 40.))
        );
        assert_eq!(
            svg_size(r#"<svg viewBox="0 0 300 150" xmlns="x">"#),
            Some((300., 150.))
        );
        assert_eq!(svg_size("<p>"), None);
    }
}
