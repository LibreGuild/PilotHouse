//! The architecture map: the project's parts drawn in layers, foundations
//! at the bottom and what builds on them above, with a line for each
//! dependency (see [`scan`]). *Explain with agent* hands the map to the chat.

mod scan;

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, PathBuilder, Pixels, Render, StatefulInteractiveElement as _, Styled as _,
    Task, WeakEntity, Window, canvas, div, point, px,
};

use crate::workspace_screen::WorkspaceScreen;
use scan::{Map, Source};

const BOX_W: f32 = 176.;
const BOX_H: f32 = 54.;
const GAP_X: f32 = 16.;
const GAP_Y: f32 = 44.;

pub fn open(space: WeakEntity<WorkspaceScreen>, root: PathBuf, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| ArchMap::new(space, root, cx));
    let viewport = window.viewport_size();
    let width = (viewport.width - px(80.)).clamp(px(560.), px(1100.));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Architecture map")
            .w(width)
            .child(view.clone())
    });
}

struct ArchMap {
    space: WeakEntity<WorkspaceScreen>,
    map: Option<Map>,
    _scan: Task<()>,
}

impl ArchMap {
    fn new(space: WeakEntity<WorkspaceScreen>, root: PathBuf, cx: &mut Context<Self>) -> Self {
        let scan = cx.spawn(async move |this, cx| {
            let map = cx.background_spawn(async move { scan::scan(&root) }).await;
            _ = this.update(cx, |this, cx| {
                this.map = Some(map);
                cx.notify();
            });
        });
        Self {
            space,
            map: None,
            _scan: scan,
        }
    }

    fn explain(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(map) = &self.map else { return };
        let text = scan::describe(map);
        window.close_dialog(cx);
        _ = self
            .space
            .update(cx, |space, cx| space.explain_architecture(text, window, cx));
    }
}

/// Where each part's box goes: rows by layer, the top row the highest.
fn layout(map: &Map) -> (Vec<(f32, f32)>, f32, f32) {
    let layers = map
        .parts
        .iter()
        .map(|part| part.layer)
        .max()
        .map_or(0, |top| top + 1);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); layers];
    for (ix, part) in map.parts.iter().enumerate() {
        rows[part.layer].push(ix);
    }
    let widest = rows.iter().map(Vec::len).max().unwrap_or(0) as f32;
    let width = (widest * (BOX_W + GAP_X) - GAP_X).max(BOX_W);
    let height = layers as f32 * (BOX_H + GAP_Y) - GAP_Y;
    let mut places = vec![(0., 0.); map.parts.len()];
    for (layer, row) in rows.iter().enumerate() {
        let row_width = row.len() as f32 * (BOX_W + GAP_X) - GAP_X;
        let left = (width - row_width) / 2.;
        let top = (layers - 1 - layer) as f32 * (BOX_H + GAP_Y);
        for (slot, &ix) in row.iter().enumerate() {
            places[ix] = (left + slot as f32 * (BOX_W + GAP_X), top);
        }
    }
    (places, width.max(0.), height.max(0.))
}

impl Render for ArchMap {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let Some(map) = &self.map else {
            return div()
                .py_6()
                .text_center()
                .text_sm()
                .text_color(muted)
                .child("Reading the project…")
                .into_any_element();
        };
        let (places, width, height) = layout(map);
        let edges: Vec<((f32, f32), (f32, f32))> = map
            .parts
            .iter()
            .enumerate()
            .flat_map(|(ix, part)| {
                let from = places[ix];
                part.drawn
                    .iter()
                    .map(move |&dep| ((from.0 + BOX_W / 2., from.1 + BOX_H), dep))
            })
            .map(|(from, dep)| (from, (places[dep].0 + BOX_W / 2., places[dep].1)))
            .collect();
        let line_color: Hsla = cx.theme().muted_foreground.opacity(0.45);
        let lines = canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, _, window, _| {
                for ((x1, y1), (x2, y2)) in &edges {
                    let mut path = PathBuilder::stroke(px(1.2));
                    path.move_to(point(bounds.origin.x + px(*x1), bounds.origin.y + px(*y1)));
                    path.line_to(point(bounds.origin.x + px(*x2), bounds.origin.y + px(*y2)));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, line_color);
                    }
                }
            },
        )
        .absolute()
        .size_full();
        let boxes = map.parts.iter().enumerate().map(|(ix, part)| {
            let (x, y) = places[ix];
            v_flex()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(BOX_W))
                .h(px(BOX_H))
                .px_2()
                .justify_center()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().secondary)
                .child(div().text_sm().truncate().child(part.name.clone()))
                .child(div().text_xs().text_color(muted).truncate().child(format!(
                    "{} lines · {}",
                    compact(part.lines),
                    part.languages.join(", ")
                )))
        });
        let source = match map.source {
            Source::Cargo => "Cargo workspace members",
            Source::Npm => "npm workspace packages",
            Source::Folders => "top-level folders (no workspace manifest found)",
        };
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_xs().text_color(muted).child(format!(
                        "{} parts from the {source}. Lines go from a part to what it uses directly (not what it reaches through another part); foundations are at the bottom.",
                        map.parts.len()
                    )))
                    .child(
                        Button::new("explain-architecture")
                            .small()
                            .primary()
                            .label("Explain with agent")
                            .disabled(map.parts.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| this.explain(window, cx))),
                    ),
            )
            .child(
                div()
                    .id("arch-map")
                    .max_h(px(560.))
                    .overflow_scroll()
                    .child(
                        div()
                            .relative()
                            .w(px(width))
                            .h(px(height))
                            .child(lines)
                            .children(boxes),
                    ),
            )
            .when(map.parts.is_empty(), |this| {
                this.child(div().text_sm().text_color(muted).child("No parts found."))
            })
            .into_any_element()
    }
}

/// `950`, `12k`, `1.2M`.
fn compact(n: usize) -> String {
    ph_agent::usage_meter::compact(n as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scan::Part;

    fn part(layer: usize) -> Part {
        Part {
            name: String::new(),
            folder: String::new(),
            files: 0,
            lines: 0,
            languages: Vec::new(),
            depends_on: Vec::new(),
            drawn: Vec::new(),
            layer,
        }
    }

    #[test]
    fn layers_stack_with_foundations_at_the_bottom() {
        let map = Map {
            source: Source::Cargo,
            parts: vec![part(0), part(0), part(1)],
        };
        let (places, width, height) = layout(&map);
        assert_eq!(width, 2. * BOX_W + GAP_X);
        assert_eq!(height, 2. * BOX_H + GAP_Y);
        assert_eq!(places[2].1, 0., "the top layer is drawn first");
        assert_eq!(places[0].1, BOX_H + GAP_Y);
        assert_eq!(places[2].0, (width - BOX_W) / 2., "a lone box is centred");
    }
}
