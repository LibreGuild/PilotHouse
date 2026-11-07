//! An agent's reply as it is shown: Markdown, with each finished
//! ```` ```mermaid ```` block drawn as a diagram (flowcharts, sequence, class,
//! state and ER diagrams, timelines…), rendered to SVG without a browser
//! (`mermaid-rs-renderer`) off the UI thread.
//!
//! While a reply streams, a diagram block that has not closed yet shows as
//! code; it becomes a picture when its closing fence arrives. A block that
//! does not parse stays code, with the reason under it.

use std::sync::Arc;

use gpui_kit::component::{
    ActiveTheme as _, clipboard::Clipboard, text::TextView, text::TextViewState, v_flex,
};
use gpui_kit::{
    AppContext as _, Context, Entity, Image, ImageFormat, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Task, Window, div,
};
use ph_ui::DiagramCard;

/// Diagrams are drawn at most this wide (they scale down to fit).
const MAX_WIDTH: f32 = 680.;

/// A piece of a reply.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Markdown(String),
    Mermaid(String),
}

/// `text` cut into Markdown and finished Mermaid blocks. A block still open
/// (no closing fence yet) stays in the Markdown.
fn pieces(text: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut markdown = String::new();
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        let opening = line.trim_start();
        let fence = if opening.starts_with("```mermaid") {
            "```"
        } else if opening.starts_with("~~~mermaid") {
            "~~~"
        } else {
            markdown.push_str(line);
            continue;
        };
        // Collect up to the closing fence; without one, it is still Markdown.
        let mut body = String::new();
        let mut closed = false;
        let mut taken = vec![line];
        while let Some(next) = lines.peek() {
            let next = *next;
            lines.next();
            taken.push(next);
            if next.trim() == fence {
                closed = true;
                break;
            }
            body.push_str(next);
        }
        if closed {
            if !markdown.trim().is_empty() {
                pieces.push(Piece::Markdown(std::mem::take(&mut markdown)));
            }
            markdown.clear();
            pieces.push(Piece::Mermaid(body));
        } else {
            markdown.extend(taken);
        }
    }
    if !markdown.is_empty() || pieces.is_empty() {
        pieces.push(Piece::Markdown(markdown));
    }
    pieces
}

/// A drawn piece.
enum Part {
    Markdown {
        text: String,
        view: Entity<TextViewState>,
        /// Keeps the lines of its paragraphs apart as more text arrives.
        breaks: ph_ui::markdown::LineBreaks,
    },
    Diagram(Entity<Diagram>),
}

/// What a click on a link in a reply does (it gets the link's address).
pub type OnLink = Arc<dyn Fn(&str, &mut Window, &mut gpui_kit::App) + Send + Sync>;

pub struct Reply {
    text: String,
    parts: Vec<Part>,
    /// Links go here; without it, they open as a chat's do.
    on_link: Option<OnLink>,
}

impl Reply {
    pub fn new(text: &str, cx: &mut Context<Self>) -> Self {
        let mut reply = Self {
            text: String::new(),
            parts: Vec::new(),
            on_link: None,
        };
        reply.set_text(text.to_string(), cx);
        reply
    }

    /// Clicks on links go to `on_link` (a view of its own takes its own
    /// links, and passes on the rest).
    pub fn on_link(mut self, on_link: OnLink) -> Self {
        self.on_link = Some(on_link);
        self
    }

    /// More of the reply arrived.
    pub fn push_str(&mut self, chunk: &str, cx: &mut Context<Self>) {
        // Mostly text with no diagram in it, growing at its end: added to
        // the last part as it comes, without reading the whole reply again
        // for every piece (a long reply arrives in thousands of them).
        let tail_from = self.text.len().saturating_sub(16);
        let tail_from = (tail_from..=self.text.len())
            .find(|ix| self.text.is_char_boundary(*ix))
            .unwrap_or(self.text.len());
        let fence_near = self.text[tail_from..].contains('`') || chunk.contains('`');
        if !fence_near
            && self.parts.len() == 1
            && let Some(Part::Markdown { text, view, breaks }) = self.parts.last_mut()
            // (The one part is all of it: same length is enough to know.)
            && text.len() == self.text.len()
        {
            self.text.push_str(chunk);
            text.push_str(chunk);
            let mut shown = String::with_capacity(chunk.len() + 8);
            breaks.push(chunk, &mut shown);
            view.update(cx, |view, cx| view.push_str(&shown, cx));
            cx.notify();
            return;
        }
        let mut text = std::mem::take(&mut self.text);
        text.push_str(chunk);
        self.set_text(text, cx);
    }

    fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        let wanted = pieces(&text);
        self.text = text;
        // Usually only the last Markdown grew: add the new text to it.
        let same_shape = wanted.len() == self.parts.len()
            && wanted
                .iter()
                .zip(&self.parts)
                .enumerate()
                .all(|(ix, pair)| match pair {
                    (Piece::Mermaid(source), Part::Diagram(diagram)) => {
                        diagram.read(cx).source == *source
                    }
                    (Piece::Markdown(new), Part::Markdown { text, .. }) => {
                        if ix + 1 == wanted.len() {
                            new.starts_with(text.as_str())
                        } else {
                            new == text
                        }
                    }
                    _ => false,
                });
        if same_shape {
            if let (Some(Piece::Markdown(new)), Some(Part::Markdown { text, view, breaks })) =
                (wanted.last(), self.parts.last_mut())
                && new.len() > text.len()
            {
                let mut added = String::new();
                breaks.push(&new[text.len()..], &mut added);
                view.update(cx, |view, cx| view.push_str(&added, cx));
                *text = new.clone();
            }
            cx.notify();
            return;
        }
        // The shape changed (a diagram closed): build again, keeping drawn
        // diagrams.
        let mut old: Vec<Entity<Diagram>> = self
            .parts
            .drain(..)
            .filter_map(|part| match part {
                Part::Diagram(diagram) => Some(diagram),
                Part::Markdown { .. } => None,
            })
            .collect();
        let dark = cx.theme().is_dark();
        self.parts = wanted
            .into_iter()
            .map(|piece| match piece {
                Piece::Markdown(text) => {
                    let mut breaks = ph_ui::markdown::LineBreaks::default();
                    let mut shown = String::with_capacity(text.len() + text.len() / 16);
                    breaks.push(&text, &mut shown);
                    let view = cx.new(|cx| TextViewState::markdown(&shown, cx));
                    Part::Markdown { text, view, breaks }
                }
                Piece::Mermaid(source) => {
                    let kept = old
                        .iter()
                        .position(|diagram| diagram.read(cx).source == source)
                        .map(|ix| old.remove(ix));
                    Part::Diagram(
                        kept.unwrap_or_else(|| cx.new(|cx| Diagram::new(source, dark, cx))),
                    )
                }
            })
            .collect();
        cx.notify();
    }
}

impl Render for Reply {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .children(self.parts.iter().map(|part| {
                match part {
                    Part::Markdown { view, .. } => {
                        let on_link = self.on_link.clone();
                        TextView::new(view)
                            .selectable(true)
                            .w_full()
                            .code_block_actions(ph_ui::markdown::code_actions)
                            // An agent wrote it: no files on this computer, and
                            // no drawing that would read them.
                            .image_source(ph_ui::images_safe::web_only)
                            .on_link_click(move |url, _, window, cx| match &on_link {
                                Some(on_link) => on_link(url, window, cx),
                                None => crate::entry_view::open_link(url, cx),
                            })
                            .into_any_element()
                    }
                    Part::Diagram(diagram) => diagram.clone().into_any_element(),
                }
            }))
    }
}

/// How a diagram's drawing went.
enum Drawing {
    Pending,
    Ready {
        image: Arc<Image>,
        width: f32,
        height: f32,
    },
    Failed(String),
}

/// One Mermaid diagram, drawn in the background.
pub struct Diagram {
    source: String,
    drawing: Drawing,
    _task: Task<()>,
}

impl Diagram {
    fn new(source: String, dark: bool, cx: &mut Context<Self>) -> Self {
        // Gone with its reply (a chat closed or trimmed): the picture too.
        cx.on_release(|this, cx| {
            if let Drawing::Ready { image, .. } =
                std::mem::replace(&mut this.drawing, Drawing::Pending)
            {
                ph_ui::images::release_image(image, cx);
            }
        })
        .detach();
        let text = source.clone();
        let draw = cx.background_spawn(async move { draw(&text, dark) });
        let task = cx.spawn(async move |this, cx| {
            let drawing = draw.await;
            _ = this.update(cx, |this, cx| {
                this.drawing = drawing;
                cx.notify();
            });
        });
        Self {
            source,
            drawing: Drawing::Pending,
            _task: task,
        }
    }
}

/// Mermaid source to an SVG image and its size.
fn draw(source: &str, dark: bool) -> Drawing {
    let options = mermaid_rs_renderer::RenderOptions {
        theme: if dark {
            mermaid_rs_renderer::Theme::dark()
        } else {
            mermaid_rs_renderer::Theme::modern()
        },
        ..Default::default()
    };
    let svg = match std::panic::catch_unwind(|| {
        mermaid_rs_renderer::render_with_options(source, options)
    }) {
        Ok(Ok(svg)) => svg,
        Ok(Err(err)) => return Drawing::Failed(format!("{err}")),
        Err(_) => return Drawing::Failed("the diagram could not be drawn".into()),
    };
    let (width, height) = svg_size(&svg).unwrap_or((MAX_WIDTH, MAX_WIDTH * 0.6));
    Drawing::Ready {
        image: Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())),
        width,
        height,
    }
}

/// The `width` and `height` (or `viewBox`) of an SVG's root.
fn svg_size(svg: &str) -> Option<(f32, f32)> {
    let start = svg.find("<svg")?;
    let tag = &svg[start..start + svg[start..].find('>')?];
    let attribute = |name: &str| {
        let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        let value = &tag[at..at + tag[at..].find('"')?];
        Some(value.to_string())
    };
    let number = |text: String| text.trim_end_matches("px").trim().parse::<f32>().ok();
    if let (Some(width), Some(height)) = (
        attribute("width").and_then(number),
        attribute("height").and_then(number),
    ) {
        return Some((width, height));
    }
    let view_box = attribute("viewBox")?;
    let parts: Vec<f32> = view_box
        .split([' ', ','])
        .filter_map(|part| part.parse().ok())
        .collect();
    (parts.len() == 4).then(|| (parts[2], parts[3]))
}

impl Render for Diagram {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = cx.entity_id().as_u64();
        let frame = v_flex()
            .id(("diagram", id))
            .group("diagram")
            .relative()
            .w_full()
            .p_2()
            .gap_1()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background);
        let copy = div()
            .absolute()
            .top_1()
            .right_1()
            .invisible()
            .group_hover("diagram", |style| style.visible())
            .child(
                Clipboard::new(("copy-diagram", id))
                    .value(self.source.clone())
                    .tooltip("Copy the diagram's source"),
            );
        match &self.drawing {
            Drawing::Pending => frame
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Drawing the diagram…"),
                )
                .into_any_element(),
            Drawing::Ready {
                image,
                width,
                height,
            } => frame
                .child(DiagramCard::new(
                    ("diagram-card", id),
                    image.clone(),
                    (*width, *height),
                    self.source.clone(),
                    MAX_WIDTH,
                ))
                .into_any_element(),
            Drawing::Failed(reason) => frame
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .whitespace_normal()
                        .child(self.source.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Not drawn: {reason}")),
                )
                .child(copy)
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_diagrams_are_cut_out_of_the_markdown() {
        let text = "Intro\n\n```mermaid\nflowchart LR\nA-->B\n```\nAfter\n";
        assert_eq!(
            pieces(text),
            vec![
                Piece::Markdown("Intro\n\n".into()),
                Piece::Mermaid("flowchart LR\nA-->B\n".into()),
                Piece::Markdown("After\n".into()),
            ]
        );
        // Still streaming: the open block stays Markdown.
        let open = "Intro\n```mermaid\nflowchart LR\nA-->";
        assert_eq!(pieces(open), vec![Piece::Markdown(open.into())]);
        // Other code stays Markdown too.
        let rust = "```rust\nfn main() {}\n```\n";
        assert_eq!(pieces(rust), vec![Piece::Markdown(rust.into())]);
        assert_eq!(pieces(""), vec![Piece::Markdown(String::new())]);
    }

    #[test]
    fn diagrams_are_drawn_with_their_size() {
        let Drawing::Ready { width, height, .. } =
            draw("flowchart LR\nA[Start] --> B[End]\n", true)
        else {
            panic!("a simple flowchart draws");
        };
        assert!(width > 0. && height > 0.);
        assert!(matches!(
            draw("not a diagram at all {{{", false),
            Drawing::Failed(_)
        ));
    }

    #[test]
    fn svg_sizes_come_from_the_root() {
        assert_eq!(
            svg_size(r#"<svg xmlns="x" width="120" height="80px">"#),
            Some((120., 80.))
        );
        assert_eq!(
            svg_size(r#"<svg viewBox="0 0 300 150" xmlns="x">"#),
            Some((300., 150.))
        );
        assert_eq!(svg_size("<p>"), None);
    }
}
