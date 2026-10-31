//! Lines whose ends are fixed to elements (connectors): after any change,
//! each end is put back on its element's side, so moving or resizing a
//! shape takes its connectors with it.

use std::sync::Arc;

use super::editor::geometry;
use super::model::*;

/// Where side `side` (0 top, 1 right, 2 bottom, 3 left) of `frame` is,
/// at its middle, turned with the frame.
pub fn site(frame: &Frame, side: u8) -> [f32; 2] {
    let (w, h) = (frame.w / 2., frame.h / 2.);
    let local = match side % 4 {
        0 => [0., -h],
        1 => [w, 0.],
        2 => [0., h],
        _ => [-w, 0.],
    };
    geometry::to_slide(frame, local)
}

/// A line's frame from its two ends (and the flips that say which corners
/// they are).
pub fn line_frame(a: [f32; 2], b: [f32; 2]) -> Frame {
    Frame {
        x: a[0].min(b[0]),
        y: a[1].min(b[1]),
        w: (b[0] - a[0]).abs(),
        h: (b[1] - a[1]).abs(),
        rotation: 0.,
        flip_h: b[0] < a[0],
        flip_v: b[1] < a[1],
    }
}

/// A line's two ends: from its frame and flips.
pub fn line_ends(frame: &Frame) -> ([f32; 2], [f32; 2]) {
    let (x0, x1) = if frame.flip_h {
        (frame.x + frame.w, frame.x)
    } else {
        (frame.x, frame.x + frame.w)
    };
    let (y0, y1) = if frame.flip_v {
        (frame.y + frame.h, frame.y)
    } else {
        (frame.y, frame.y + frame.h)
    };
    ([x0, y0], [x1, y1])
}

/// Puts connectors' ends back on their elements, on the slides that
/// changed since `before` (all when there is none).
pub fn follow(deck: &mut Deck, before: Option<&Deck>) {
    for index in 0..deck.slides.len() {
        let slide = &deck.slides[index];
        let unchanged = before
            .and_then(|before| before.slides.get(index))
            .is_some_and(|old| Arc::ptr_eq(old, slide));
        if unchanged || !slide.elements.iter().any(is_connector) {
            continue;
        }
        let frames: Vec<(String, Frame)> = slide
            .elements
            .iter()
            .map(|element| (element.id.clone(), element.frame))
            .collect();
        let frame_of = |id: &str| {
            frames
                .iter()
                .find(|(known, _)| known == id)
                .map(|(_, frame)| *frame)
        };
        // Only lines whose place would change are touched.
        let moved: Vec<(usize, Frame)> = slide
            .elements
            .iter()
            .enumerate()
            .filter_map(|(ix, element)| {
                let Kind::Shape(Shape {
                    connect: Some(connect),
                    ..
                }) = &element.kind
                else {
                    return None;
                };
                let (mut a, mut b) = line_ends(&element.frame);
                if let Some((id, side)) = &connect.start
                    && let Some(frame) = frame_of(id)
                {
                    a = site(&frame, *side);
                }
                if let Some((id, side)) = &connect.end
                    && let Some(frame) = frame_of(id)
                {
                    b = site(&frame, *side);
                }
                let frame = line_frame(a, b);
                let same = (frame.x - element.frame.x).abs() < 0.01
                    && (frame.y - element.frame.y).abs() < 0.01
                    && (frame.w - element.frame.w).abs() < 0.01
                    && (frame.h - element.frame.h).abs() < 0.01
                    && frame.flip_h == element.frame.flip_h
                    && frame.flip_v == element.frame.flip_v
                    && element.frame.rotation == 0.;
                (!same).then_some((ix, frame))
            })
            .collect();
        if moved.is_empty() {
            continue;
        }
        let slide = Arc::make_mut(&mut deck.slides[index]);
        for (ix, frame) in moved {
            slide.elements[ix].frame = frame;
        }
    }
}

/// Frames made NaN or infinite (a division by a zero size somewhere) are
/// put back to something drawable: saved, they would be JSON null and the
/// deck would not open again. Only slides changed since `before` are looked
/// at.
pub fn keep_finite(deck: &mut Deck, before: Option<&Deck>) {
    fn bad(frame: &Frame) -> bool {
        ![frame.x, frame.y, frame.w, frame.h, frame.rotation]
            .iter()
            .all(|value| value.is_finite())
    }
    fn any_bad(elements: &[Element]) -> bool {
        elements.iter().any(|element| {
            bad(&element.frame)
                || matches!(&element.kind, Kind::Group(group) if any_bad(&group.children))
        })
    }
    fn mend(elements: &mut [Element]) {
        for element in elements {
            let frame = &mut element.frame;
            for value in [&mut frame.x, &mut frame.y, &mut frame.rotation] {
                if !value.is_finite() {
                    *value = 0.;
                }
            }
            for value in [&mut frame.w, &mut frame.h] {
                if !value.is_finite() {
                    *value = 1.;
                }
            }
            if let Kind::Group(group) = &mut element.kind {
                mend(&mut group.children);
            }
        }
    }
    for index in 0..deck.slides.len() {
        let unchanged = before
            .and_then(|before| before.slides.get(index))
            .is_some_and(|old| Arc::ptr_eq(old, &deck.slides[index]));
        if !unchanged && any_bad(&deck.slides[index].elements) {
            mend(&mut Arc::make_mut(&mut deck.slides[index]).elements);
        }
    }
}

fn is_connector(element: &Element) -> bool {
    matches!(
        &element.kind,
        Kind::Shape(Shape {
            connect: Some(_),
            ..
        })
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connectors_follow_their_shapes() {
        let mut deck = Deck::default();
        let shape = |id: &str, frame: Frame| Element {
            id: id.into(),
            frame,
            kind: Kind::Shape(Shape::default()),
            ..Element::default()
        };
        let line = Element {
            id: "l".into(),
            kind: Kind::Shape(Shape {
                geometry: Geometry::Line,
                connect: Some(Connect {
                    start: Some(("a".into(), 1)),
                    end: Some(("b".into(), 3)),
                }),
                ..Shape::default()
            }),
            ..Element::default()
        };
        deck.slides.push(Arc::new(Slide {
            elements: vec![
                shape("a", Frame::new(0., 0., 100., 100.)),
                shape("b", Frame::new(300., 200., 100., 100.)),
                line,
            ],
            ..Slide::default()
        }));
        follow(&mut deck, None);
        let frame = deck.slides[0].elements[2].frame;
        assert_eq!(
            (frame.x, frame.y, frame.w, frame.h),
            (100., 50., 200., 200.)
        );
        assert_eq!(line_ends(&frame), ([100., 50.], [300., 250.]));
        // The second shape moves left of the first: the line turns round.
        Arc::make_mut(&mut deck.slides[0]).elements[1].frame.x = -300.;
        follow(&mut deck, None);
        let (a, b) = line_ends(&deck.slides[0].elements[2].frame);
        assert_eq!((a, b), ([100., 50.], [-300., 250.]));
    }
}
