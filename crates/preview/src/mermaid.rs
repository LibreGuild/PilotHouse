//! Mermaid diagrams, drawn by a Rust renderer (no browser): in Markdown
//! code blocks and in `.mmd` files.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

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
/// on every edit, and drawing is the slow part.
type Drawn = HashMap<(String, bool), Result<Diagram, String>>;
static DRAWN: LazyLock<Mutex<Drawn>> = LazyLock::new(Mutex::default);
const KEEP: usize = 64;

/// Draws `source` for a dark or light background.
pub fn draw(source: &str, dark: bool) -> Result<Diagram, String> {
    let key = (source.to_string(), dark);
    if let Some(found) = DRAWN.lock().ok().and_then(|drawn| drawn.get(&key).cloned()) {
        return found;
    }
    let drawn = draw_uncached(source, dark);
    if let Ok(mut cache) = DRAWN.lock() {
        if cache.len() >= KEEP {
            cache.clear();
        }
        cache.insert(key, drawn.clone());
    }
    drawn
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
    let svg = mermaid_rs_renderer::render_with_options(source.trim(), options)
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
