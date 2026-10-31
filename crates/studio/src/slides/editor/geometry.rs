//! Where things are on the canvas, in the slide's points: what the pointer
//! is over (turned elements included), the handles of a selection, what a
//! drag of a handle makes of a frame, and snapping to the slide and the
//! other elements.

use crate::slides::model::{Element, Frame, Geometry, Kind};

/// A point on the slide.
pub type P = [f32; 2];

/// A handle of the selection: the eight around its frame, and the one
/// above it that turns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    Rotate,
}

impl Handle {
    pub const SIDES: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// Which sides it moves: left/right (-1, 0, 1) and top/bottom.
    fn sides(self) -> (i8, i8) {
        match self {
            Handle::TopLeft => (-1, -1),
            Handle::Top => (0, -1),
            Handle::TopRight => (1, -1),
            Handle::Right => (1, 0),
            Handle::BottomRight => (1, 1),
            Handle::Bottom => (0, 1),
            Handle::BottomLeft => (-1, 1),
            Handle::Left => (-1, 0),
            Handle::Rotate => (0, 0),
        }
    }

    pub fn is_corner(self) -> bool {
        let (x, y) = self.sides();
        x != 0 && y != 0
    }

    /// Where it is on `frame` (turned with it); the turning handle sits
    /// `above` points over the top.
    pub fn at(self, frame: &Frame, above: f32) -> P {
        let (sx, sy) = self.sides();
        let local = if self == Handle::Rotate {
            [0., -frame.h / 2. - above]
        } else {
            [sx as f32 * frame.w / 2., sy as f32 * frame.h / 2.]
        };
        to_slide(frame, local)
    }

    /// The pointer for resizing along this handle, turned with the frame.
    pub fn cursor(self, rotation: f32) -> gpui_kit::CursorStyle {
        use gpui_kit::CursorStyle;
        let base = match self {
            Handle::Top | Handle::Bottom => 90.,
            Handle::Left | Handle::Right => 0.,
            Handle::TopLeft | Handle::BottomRight => 45.,
            Handle::TopRight | Handle::BottomLeft => 135.,
            Handle::Rotate => return CursorStyle::PointingHand,
        };
        let angle = (base + rotation).rem_euclid(180.);
        if !(22.5..157.5).contains(&angle) {
            CursorStyle::ResizeLeftRight
        } else if (67.5..112.5).contains(&angle) {
            CursorStyle::ResizeUpDown
        } else if angle < 90. {
            CursorStyle::ResizeUpLeftDownRight
        } else {
            CursorStyle::ResizeUpRightDownLeft
        }
    }
}

fn rotate(p: P, degrees: f32) -> P {
    let (sin, cos) = degrees.to_radians().sin_cos();
    [p[0] * cos - p[1] * sin, p[0] * sin + p[1] * cos]
}

/// A point in the frame's own space (from its centre, unturned) on the
/// slide.
pub fn to_slide(frame: &Frame, local: P) -> P {
    let (cx, cy) = frame.center();
    let p = rotate(local, frame.rotation);
    [cx + p[0], cy + p[1]]
}

/// A slide point in the frame's own space (from its centre, unturned).
pub fn to_local(frame: &Frame, p: P) -> P {
    let (cx, cy) = frame.center();
    rotate([p[0] - cx, p[1] - cy], -frame.rotation)
}

/// The frame's four corners on the slide, clockwise from the top left.
pub fn corners(frame: &Frame) -> [P; 4] {
    let (w, h) = (frame.w / 2., frame.h / 2.);
    [[-w, -h], [w, -h], [w, h], [-w, h]].map(|local| to_slide(frame, local))
}

/// The box around a frame, turned or not: left, top, right, bottom.
pub fn bounds(frame: &Frame) -> [f32; 4] {
    if frame.rotation.rem_euclid(360.).abs() < 0.01 {
        return [frame.x, frame.y, frame.x + frame.w, frame.y + frame.h];
    }
    let points = corners(frame);
    let mut out = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for [x, y] in points {
        out[0] = out[0].min(x);
        out[1] = out[1].min(y);
        out[2] = out[2].max(x);
        out[3] = out[3].max(y);
    }
    out
}

/// The box around several frames.
pub fn union(frames: impl IntoIterator<Item = Frame>) -> Option<[f32; 4]> {
    frames
        .into_iter()
        .map(|frame| bounds(&frame))
        .reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
}

/// Whether `p` is on `element`, with `reach` points to spare (thin lines
/// are hard to hit otherwise).
pub fn hits(element: &Element, p: P, reach: f32) -> bool {
    let frame = &element.frame;
    let local = to_local(frame, p);
    if let Kind::Shape(shape) = &element.kind
        && shape.geometry == Geometry::Line
    {
        let flip = |flip: bool| if flip { -1. } else { 1. };
        let (w, h) = (
            frame.w / 2. * flip(frame.flip_h),
            frame.h / 2. * flip(frame.flip_v),
        );
        let reach = reach.max(shape.line.width / 2.);
        return near_segment(local, [-w, -h], [w, h]) <= reach;
    }
    local[0].abs() <= frame.w / 2. + reach && local[1].abs() <= frame.h / 2. + reach
}

fn near_segment(p: P, a: P, b: P) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = dx * dx + dy * dy;
    let t = if length > 0. {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length).clamp(0., 1.)
    } else {
        0.
    };
    let (x, y) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    (x * x + y * y).sqrt()
}

/// `original` with handle `handle` dragged to `p`: the opposite side
/// stays put; `keep_aspect` keeps its proportions (from a corner), and
/// `from_center` grows it about its centre.
pub fn resize(
    original: &Frame,
    handle: Handle,
    p: P,
    keep_aspect: bool,
    from_center: bool,
) -> Frame {
    let (sx, sy) = handle.sides();
    let local = to_local(original, p);
    let (w, h) = (original.w / 2., original.h / 2.);
    let (mut left, mut right, mut top, mut bottom) = (-w, w, -h, h);
    let least = 1.;
    match sx {
        1 => right = local[0].max(left + least),
        -1 => left = local[0].min(right - least),
        _ => {}
    }
    match sy {
        1 => bottom = local[1].max(top + least),
        -1 => top = local[1].min(bottom - least),
        _ => {}
    }
    if from_center {
        if sx != 0 {
            let half = (if sx > 0 { right } else { -left }).max(least / 2.);
            (left, right) = (-half, half);
        }
        if sy != 0 {
            let half = (if sy > 0 { bottom } else { -top }).max(least / 2.);
            (top, bottom) = (-half, half);
        }
    }
    if keep_aspect && handle.is_corner() && original.h > 0. && original.w > 0. {
        let aspect = original.w / original.h;
        let (width, height) = (right - left, bottom - top);
        // The longer pull decides; the other side follows.
        let (width, height) = if width / aspect > height {
            (width, width / aspect)
        } else {
            (height * aspect, height)
        };
        if from_center {
            (left, right, top, bottom) = (-width / 2., width / 2., -height / 2., height / 2.);
        } else {
            if sx > 0 {
                right = left + width;
            } else {
                left = right - width;
            }
            if sy > 0 {
                bottom = top + height;
            } else {
                top = bottom - height;
            }
        }
    }
    let (width, height) = (right - left, bottom - top);
    let offset = rotate(
        [(left + right) / 2., (top + bottom) / 2.],
        original.rotation,
    );
    let (cx, cy) = original.center();
    Frame {
        x: cx + offset[0] - width / 2.,
        y: cy + offset[1] - height / 2.,
        w: width,
        h: height,
        ..*original
    }
}

/// The turn that puts the turning handle at `p` (in 15° steps unless
/// `free`).
pub fn rotation(frame: &Frame, p: P, free: bool) -> f32 {
    let (cx, cy) = frame.center();
    let degrees = (p[1] - cy).atan2(p[0] - cx).to_degrees() + 90.;
    let degrees = if free {
        degrees
    } else {
        (degrees / 15.).round() * 15.
    };
    degrees.rem_euclid(360.)
}

/// Lines to snap to on one axis.
#[derive(Debug, Default, Clone)]
pub struct Snaps {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
}

impl Snaps {
    /// The slide's edges and middle, and the edges and middles of
    /// `others`.
    pub fn new(width: f32, height: f32, others: impl IntoIterator<Item = Frame>) -> Snaps {
        let mut snaps = Snaps {
            x: vec![0., width / 2., width],
            y: vec![0., height / 2., height],
        };
        for frame in others {
            let [l, t, r, b] = bounds(&frame);
            snaps.x.extend([l, (l + r) / 2., r]);
            snaps.y.extend([t, (t + b) / 2., b]);
        }
        snaps
    }

    /// How far to move `edges` (on one axis) so the nearest lands on a
    /// line within `reach`, and the line.
    fn nearest(lines: &[f32], edges: &[f32], reach: f32) -> Option<(f32, f32)> {
        let mut best: Option<(f32, f32)> = None;
        for edge in edges {
            for line in lines {
                let shift = line - edge;
                if shift.abs() <= reach && best.is_none_or(|(b, _)| shift.abs() < b.abs()) {
                    best = Some((shift, *line));
                }
            }
        }
        best
    }

    /// `bounds` moved by up to `reach` so an edge or the middle meets a
    /// line: the shift and the lines met (for guides).
    pub fn snap_box(&self, bounds: [f32; 4], reach: f32) -> ([f32; 2], Vec<Guide>) {
        let [l, t, r, b] = bounds;
        let mut guides = Vec::new();
        let mut shift = [0., 0.];
        if let Some((dx, line)) = Self::nearest(&self.x, &[l, (l + r) / 2., r], reach) {
            shift[0] = dx;
            guides.push(Guide::Vertical(line));
        }
        if let Some((dy, line)) = Self::nearest(&self.y, &[t, (t + b) / 2., b], reach) {
            shift[1] = dy;
            guides.push(Guide::Horizontal(line));
        }
        (shift, guides)
    }

    /// A point (a dragged edge) moved to the nearest line within reach.
    pub fn snap_point(&self, p: P, x: bool, y: bool, reach: f32) -> (P, Vec<Guide>) {
        let mut out = p;
        let mut guides = Vec::new();
        if x && let Some((dx, line)) = Self::nearest(&self.x, &[p[0]], reach) {
            out[0] += dx;
            guides.push(Guide::Vertical(line));
        }
        if y && let Some((dy, line)) = Self::nearest(&self.y, &[p[1]], reach) {
            out[1] += dy;
            guides.push(Guide::Horizontal(line));
        }
        (out, guides)
    }
}

/// A line shown while something snaps to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Guide {
    Vertical(f32),
    Horizontal(f32),
}

/// Which sides of a frame a handle moves, for snapping the dragged edge:
/// along x, along y.
pub fn moves_axes(handle: Handle) -> (bool, bool) {
    let (x, y) = handle.sides();
    (x != 0, y != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn turned_frames_hit_and_resize() {
        let frame = Frame {
            rotation: 90.,
            ..Frame::new(100., 100., 200., 50.)
        };
        // Turned a quarter, it stands 50 wide and 200 tall about (200, 125).
        let element = Element {
            frame,
            ..Element::default()
        };
        assert!(hits(&element, [200., 40.], 0.));
        assert!(!hits(&element, [110., 125.], 0.));
        assert_eq!(bounds(&frame).map(|v| v.round()), [175., 25., 225., 225.]);
        // The right handle of the turned frame is at the bottom.
        let at = Handle::Right.at(&frame, 0.);
        assert!(close(at[0], 200.) && close(at[1], 225.));
        // Pulled 20 further, the frame grows 20 along its length, the far
        // end staying put.
        let grown = resize(&frame, Handle::Right, [200., 245.], false, false);
        assert!(close(grown.w, 220.) && close(grown.h, 50.));
        let far = Handle::Left.at(&grown, 0.);
        let before = Handle::Left.at(&frame, 0.);
        assert!(close(far[0], before[0]) && close(far[1], before[1]));
    }

    #[test]
    fn corners_keep_proportions_and_handles_turn() {
        let frame = Frame::new(0., 0., 200., 100.);
        let grown = resize(&frame, Handle::BottomRight, [400., 120.], true, false);
        assert!(close(grown.w, 400.) && close(grown.h, 200.));
        assert!(close(grown.x, 0.) && close(grown.y, 0.));
        assert_eq!(rotation(&frame, [100., -100.], false), 0.);
        assert_eq!(rotation(&frame, [300., 50.], false), 90.);
    }

    #[test]
    fn boxes_snap_to_the_slide_and_others() {
        let snaps = Snaps::new(960., 540., [Frame::new(500., 100., 100., 100.)]);
        let (shift, guides) = snaps.snap_box([3., 10., 103., 60.], 5.);
        assert_eq!(shift, [-3., 0.]);
        assert_eq!(guides, vec![Guide::Vertical(0.)]);
        let (shift, _) = snaps.snap_box([497., 300., 597., 400.], 5.);
        assert_eq!(shift[0], 3.);
    }
}
