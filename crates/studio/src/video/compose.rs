//! A frame of the result: the source frame cropped, its hidden areas
//! blurred or pixelated, resized, with the marks shown at that time drawn
//! over it, the same way a picture's are. Marks change rarely between
//! frames, so their layer is kept and reused until they do.

use image::{RgbaImage, imageops};
use resvg::tiny_skia::Pixmap;

use super::edit::VideoEdit;
use crate::edit::{Mark, MarkKind, Rect};
use crate::render::{blend, blur_area, marks_layer, pixelate};

pub struct Composer {
    edit: VideoEdit,
    /// The source frame size, upright.
    size: (u32, u32),
    layer: Option<MarksLayer>,
}

/// The marks layer last made, and what it shows: its size, and each
/// mark's id and how visible.
type MarksLayer = ((u32, u32), Vec<(u32, u8)>, Pixmap);

impl Composer {
    pub fn new(edit: VideoEdit, size: (u32, u32)) -> Composer {
        Composer {
            edit,
            size,
            layer: None,
        }
    }

    /// The result's frame at source time `t`, `target` pixels, from
    /// `source` (the source frame at any scale).
    pub fn frame(&mut self, source: &RgbaImage, t: f64, target: (u32, u32)) -> RgbaImage {
        let scale = source.width() as f32 / self.size.0.max(1) as f32;
        let crop = self.edit.crop_rect(self.size);
        let (x, y) = (
            ((crop.x * scale).round() as u32).min(source.width().saturating_sub(1)),
            ((crop.y * scale).round() as u32).min(source.height().saturating_sub(1)),
        );
        let width = ((crop.width * scale).round() as u32).clamp(1, source.width() - x);
        let height = ((crop.height * scale).round() as u32).clamp(1, source.height() - y);
        let mut image = if (x, y, width, height) == (0, 0, source.width(), source.height()) {
            source.clone()
        } else {
            imageops::crop_imm(source, x, y, width, height).to_image()
        };

        // Hidden areas, at the source's own pixels.
        for overlay in &self.edit.overlays {
            if overlay.alpha(t) <= 0. {
                continue;
            }
            let Some(area) = overlay.kind.area() else {
                continue;
            };
            let area = Rect::new(
                (area.x - crop.x) * scale,
                (area.y - crop.y) * scale,
                area.width * scale,
                area.height * scale,
            );
            match overlay.kind {
                MarkKind::Blur { strength, .. } => blur_area(&mut image, area, strength * scale),
                MarkKind::Pixelate { size, .. } => {
                    pixelate(&mut image, area, (size * scale).max(2.))
                }
                _ => {}
            }
        }

        if image.dimensions() != target {
            image = imageops::resize(&image, target.0, target.1, imageops::FilterType::Triangle);
        }

        // Marks, faded as they come and go.
        let shown: Vec<(u32, u8)> = self
            .edit
            .overlays
            .iter()
            .filter(|overlay| {
                !matches!(
                    overlay.kind,
                    MarkKind::Blur { .. } | MarkKind::Pixelate { .. }
                )
            })
            .filter_map(|overlay| {
                let alpha = overlay.alpha(t);
                (alpha > 0.).then(|| (overlay.id, (alpha * 20.).round() as u8))
            })
            .collect();
        if shown.is_empty() {
            return image;
        }
        let fresh = self
            .layer
            .as_ref()
            .is_some_and(|(size, key, _)| *size == target && *key == shown);
        if !fresh {
            let marks: Vec<Mark> = shown
                .iter()
                .filter_map(|(id, step)| {
                    let overlay = self.edit.overlay(*id)?;
                    Some(Mark {
                        id: *id,
                        kind: overlay.kind.faded(*step as f32 / 20.),
                    })
                })
                .collect();
            let marks: Vec<&Mark> = marks.iter().collect();
            let (by_x, by_y) = (target.0 as f32 / crop.width, target.1 as f32 / crop.height);
            let place = |p: [f32; 2]| [(p[0] - crop.x) * by_x, (p[1] - crop.y) * by_y];
            self.layer = marks_layer(target.0, target.1, &marks, place, (by_x + by_y) / 2.)
                .map(|layer| (target, shown, layer));
        }
        if let Some((_, _, layer)) = &self.layer {
            blend(&mut image, layer);
        }
        image
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{Color, MarkType};
    use crate::video::edit::Overlay;

    #[test]
    fn marks_show_only_in_their_time() {
        let mut edit = VideoEdit::default();
        edit.add_overlay(
            Overlay {
                id: 0,
                start: 1.,
                end: 2.,
                fade: 0.,
                kind: MarkType::Rect.mark([10., 10.], [90., 50.], Color::RED),
            },
            (100, 60),
        )
        .unwrap();
        edit.crop = Some(Rect::new(0., 0., 100., 50.));
        let source = RgbaImage::from_pixel(100, 60, image::Rgba([255, 255, 255, 255]));
        let mut composer = Composer::new(edit, (100, 60));
        let before = composer.frame(&source, 0.5, (100, 50));
        assert_eq!(before.dimensions(), (100, 50));
        assert!(before.pixels().all(|p| p.0 == [255, 255, 255, 255]));
        let during = composer.frame(&source, 1.5, (100, 50));
        assert_ne!(during, before);
        // Half the size: the same marks, made again for it.
        let small = composer.frame(&source, 1.5, (50, 26));
        assert_eq!(small.dimensions(), (50, 26));
    }
}
