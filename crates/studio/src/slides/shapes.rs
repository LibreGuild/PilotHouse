//! Preset shapes as SVG paths in their frame's box, drawn as PowerPoint
//! draws its presets of the same name with their default proportions (so
//! an exported deck looks the same), and their PowerPoint names.

use std::f32::consts::PI;
use std::fmt::Write as _;

use super::model::Geometry;

/// PowerPoint's name for `geometry` (`a:prstGeom prst`), `None` for a
/// freeform.
pub fn preset_name(geometry: Geometry) -> Option<&'static str> {
    Some(match geometry {
        Geometry::Rect => "rect",
        Geometry::RoundRect => "roundRect",
        Geometry::Ellipse => "ellipse",
        Geometry::Triangle => "triangle",
        Geometry::RightTriangle => "rtTriangle",
        Geometry::Diamond => "diamond",
        Geometry::Parallelogram => "parallelogram",
        Geometry::Trapezoid => "trapezoid",
        Geometry::Pentagon => "pentagon",
        Geometry::Hexagon => "hexagon",
        Geometry::Octagon => "octagon",
        Geometry::Chevron => "chevron",
        Geometry::HomePlate => "homePlate",
        Geometry::RightArrow => "rightArrow",
        Geometry::LeftArrow => "leftArrow",
        Geometry::UpArrow => "upArrow",
        Geometry::DownArrow => "downArrow",
        Geometry::LeftRightArrow => "leftRightArrow",
        Geometry::Star5 => "star5",
        Geometry::Star6 => "star6",
        Geometry::Plus => "plus",
        Geometry::Heart => "heart",
        Geometry::Callout => "wedgeRoundRectCallout",
        Geometry::Line => "line",
        Geometry::Path => return None,
    })
}

/// A rounded rectangle's corner radius by default: PowerPoint's 16.667%
/// of the shorter side.
pub fn default_radius(w: f32, h: f32) -> f32 {
    w.min(h) * 0.16667
}

/// The outline of `geometry` in a `w` × `h` box; `radius` rounds
/// rectangles.
pub fn path(geometry: Geometry, w: f32, h: f32, radius: Option<f32>) -> String {
    let short = w.min(h);
    let poly = |points: &[(f32, f32)]| {
        let mut d = String::new();
        for (ix, (x, y)) in points.iter().enumerate() {
            let _ = write!(d, "{}{} {} ", if ix == 0 { "M" } else { "L" }, n(*x), n(*y));
        }
        d.push('Z');
        d
    };
    match geometry {
        Geometry::Rect => match radius.filter(|radius| *radius > 0.) {
            Some(radius) => rounded(w, h, radius),
            None => poly(&[(0., 0.), (w, 0.), (w, h), (0., h)]),
        },
        Geometry::RoundRect => rounded(w, h, radius.unwrap_or_else(|| default_radius(w, h))),
        Geometry::Ellipse => {
            let (rx, ry) = (w / 2., h / 2.);
            format!(
                "M0 {cy} A{rx} {ry} 0 1 0 {w} {cy} A{rx} {ry} 0 1 0 0 {cy} Z",
                cy = n(ry),
                rx = n(rx),
                ry = n(ry),
                w = n(w)
            )
        }
        Geometry::Triangle => poly(&[(w / 2., 0.), (w, h), (0., h)]),
        Geometry::RightTriangle => poly(&[(0., 0.), (w, h), (0., h)]),
        Geometry::Diamond => poly(&[(w / 2., 0.), (w, h / 2.), (w / 2., h), (0., h / 2.)]),
        Geometry::Parallelogram => {
            let a = short * 0.25;
            poly(&[(a, 0.), (w, 0.), (w - a, h), (0., h)])
        }
        Geometry::Trapezoid => {
            let a = short * 0.25;
            poly(&[(a, 0.), (w - a, 0.), (w, h), (0., h)])
        }
        Geometry::Pentagon => regular(5, w, h, -PI / 2.),
        Geometry::Hexagon => {
            let a = short * 0.25;
            poly(&[
                (a, 0.),
                (w - a, 0.),
                (w, h / 2.),
                (w - a, h),
                (a, h),
                (0., h / 2.),
            ])
        }
        Geometry::Octagon => {
            let a = short * 0.29289;
            poly(&[
                (a, 0.),
                (w - a, 0.),
                (w, a),
                (w, h - a),
                (w - a, h),
                (a, h),
                (0., h - a),
                (0., a),
            ])
        }
        Geometry::Chevron => {
            let a = (short * 0.5).min(w * 0.5);
            poly(&[
                (0., 0.),
                (w - a, 0.),
                (w, h / 2.),
                (w - a, h),
                (0., h),
                (a, h / 2.),
            ])
        }
        Geometry::HomePlate => {
            let a = (short * 0.5).min(w);
            poly(&[(0., 0.), (w - a, 0.), (w, h / 2.), (w - a, h), (0., h)])
        }
        Geometry::RightArrow => arrow(w, h, false),
        Geometry::LeftArrow => mirror_x(&arrow_points(w, h), w),
        Geometry::DownArrow => {
            let points: Vec<(f32, f32)> =
                arrow_points(h, w).iter().map(|(x, y)| (*y, *x)).collect();
            poly(&points)
        }
        Geometry::UpArrow => {
            let points: Vec<(f32, f32)> = arrow_points(h, w)
                .iter()
                .map(|(x, y)| (*y, h - *x))
                .collect();
            poly(&points)
        }
        Geometry::LeftRightArrow => {
            let a = (short * 0.5).min(w / 2.);
            let (top, bottom) = (h * 0.25, h * 0.75);
            poly(&[
                (0., h / 2.),
                (a, 0.),
                (a, top),
                (w - a, top),
                (w - a, 0.),
                (w, h / 2.),
                (w - a, h),
                (w - a, bottom),
                (a, bottom),
                (a, h),
            ])
        }
        Geometry::Star5 => star(5, w, h, 0.38197),
        Geometry::Star6 => star(6, w, h, 0.57735),
        Geometry::Plus => {
            let a = short * 0.25;
            poly(&[
                (a, 0.),
                (w - a, 0.),
                (w - a, a),
                (w, a),
                (w, h - a),
                (w - a, h - a),
                (w - a, h),
                (a, h),
                (a, h - a),
                (0., h - a),
                (0., a),
                (a, a),
            ])
        }
        Geometry::Heart => format!(
            "M{} {} C{} {} {} {} {} {} C{} {} {} {} {} {} C{} {} {} {} {} {} C{} {} {} {} {} {} Z",
            n(w / 2.),
            n(h * 0.25),
            n(w * 0.4),
            n(-h * 0.08),
            n(-w * 0.12),
            n(h * 0.02),
            n(w * 0.02),
            n(h * 0.42),
            n(w * 0.12),
            n(h * 0.72),
            n(w * 0.4),
            n(h * 0.86),
            n(w / 2.),
            n(h),
            n(w * 0.6),
            n(h * 0.86),
            n(w * 0.88),
            n(h * 0.72),
            n(w * 0.98),
            n(h * 0.42),
            n(w * 1.12),
            n(h * 0.02),
            n(w * 0.6),
            n(-h * 0.08),
            n(w / 2.),
            n(h * 0.25),
        ),
        Geometry::Callout => {
            // The body over the top 80%; the tail to the bottom left.
            let body = h * 0.8;
            let r = (w.min(body) * 0.16667).min(body / 2.);
            format!(
                "M{r} 0 H{} A{r} {r} 0 0 1 {w} {r} V{} A{r} {r} 0 0 1 {} {body} H{} L{} {h} L{} {body} H{r} A{r} {r} 0 0 1 0 {} V{r} A{r} {r} 0 0 1 {r} 0 Z",
                n(w - r),
                n(body - r),
                n(w - r),
                n(w * 0.38),
                n(w * 0.12),
                n(w * 0.22),
                n(body - r),
                r = n(r),
                w = n(w),
                h = n(h),
                body = n(body),
            )
        }
        Geometry::Line => format!("M0 0 L{} {}", n(w), n(h)),
        Geometry::Path => String::new(),
    }
}

/// A number for SVG: short, no needless decimals.
pub fn n(value: f32) -> String {
    let rounded = (value * 100.).round() / 100.;
    if rounded == rounded.trunc() {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

fn rounded(w: f32, h: f32, radius: f32) -> String {
    let r = radius.min(w / 2.).min(h / 2.).max(0.);
    format!(
        "M{r} 0 H{} A{r} {r} 0 0 1 {w} {r} V{} A{r} {r} 0 0 1 {} {h} H{r} A{r} {r} 0 0 1 0 {} V{r} A{r} {r} 0 0 1 {r} 0 Z",
        n(w - r),
        n(h - r),
        n(w - r),
        n(h - r),
        r = n(r),
        w = n(w),
        h = n(h),
    )
}

fn arrow_points(w: f32, h: f32) -> Vec<(f32, f32)> {
    let a = (w.min(h) * 0.5).min(w);
    let (top, bottom) = (h * 0.25, h * 0.75);
    vec![
        (0., top),
        (w - a, top),
        (w - a, 0.),
        (w, h / 2.),
        (w - a, h),
        (w - a, bottom),
        (0., bottom),
    ]
}

fn arrow(w: f32, h: f32, _: bool) -> String {
    points_path(&arrow_points(w, h))
}

fn mirror_x(points: &[(f32, f32)], w: f32) -> String {
    let mirrored: Vec<(f32, f32)> = points.iter().map(|(x, y)| (w - x, *y)).collect();
    points_path(&mirrored)
}

fn points_path(points: &[(f32, f32)]) -> String {
    let mut d = String::new();
    for (ix, (x, y)) in points.iter().enumerate() {
        let _ = write!(d, "{}{} {} ", if ix == 0 { "M" } else { "L" }, n(*x), n(*y));
    }
    d.push('Z');
    d
}

fn regular(sides: usize, w: f32, h: f32, start: f32) -> String {
    let points: Vec<(f32, f32)> = (0..sides)
        .map(|ix| {
            let angle = start + ix as f32 * 2. * PI / sides as f32;
            (w / 2. + w / 2. * angle.cos(), h / 2. + h / 2. * angle.sin())
        })
        .collect();
    // Fitted to the box (a pentagon's points do not reach its bottom).
    fit(&points, w, h)
}

fn star(points: usize, w: f32, h: f32, inner: f32) -> String {
    let corners: Vec<(f32, f32)> = (0..points * 2)
        .map(|ix| {
            let angle = -PI / 2. + ix as f32 * PI / points as f32;
            let radius = if ix % 2 == 0 { 1. } else { inner };
            (
                w / 2. + w / 2. * radius * angle.cos(),
                h / 2. + h / 2. * radius * angle.sin(),
            )
        })
        .collect();
    fit(&corners, w, h)
}

/// `points` stretched to fill the `w` × `h` box.
fn fit(points: &[(f32, f32)], w: f32, h: f32) -> String {
    let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in points {
        left = left.min(*x);
        right = right.max(*x);
        top = top.min(*y);
        bottom = bottom.max(*y);
    }
    let (sx, sy) = (w / (right - left).max(0.01), h / (bottom - top).max(0.01));
    let fitted: Vec<(f32, f32)> = points
        .iter()
        .map(|(x, y)| ((x - left) * sx, (y - top) * sy))
        .collect();
    points_path(&fitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_has_an_outline() {
        for geometry in Geometry::ALL {
            let d = path(geometry, 200., 100., None);
            assert!(d.starts_with('M'), "{geometry:?}: {d}");
            assert!(preset_name(geometry).is_some());
        }
        assert_eq!(n(1.0), "1");
        assert_eq!(n(1.256), "1.26");
    }
}
