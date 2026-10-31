//! Charts drawn as SVG: columns, bars, stacked columns, lines, areas,
//! pies, doughnuts and scatter plots, in the theme's accent colours, with
//! axes, gridlines, a legend and value labels.

use std::fmt::Write as _;

use super::model::{Chart, ChartKind, Color, Rgba, Theme};
use super::shapes::n;

/// The colour of series (or slice) `ix`.
pub fn series_color(chart: &Chart, theme: &Theme, ix: usize) -> Rgba {
    let accents = [
        "accent1", "accent2", "accent3", "accent4", "accent5", "accent6",
    ];
    match chart
        .series
        .get(ix)
        .and_then(|series| series.color.as_ref())
    {
        Some(color) if !matches!(chart.chart, ChartKind::Pie | ChartKind::Doughnut) => {
            theme.resolve(color)
        }
        _ => {
            // Past six, the accents again, lighter.
            let round = ix / accents.len();
            theme
                .resolve(&Color::from(accents[ix % accents.len()]))
                .shade(round as f32 * 0.3)
        }
    }
}

/// Writes `chart` in a `w` × `h` box into `out`; text in `font` and
/// `text` colour.
pub fn draw(
    chart: &Chart,
    w: f32,
    h: f32,
    theme: &Theme,
    font: &str,
    text: Rgba,
    out: &mut String,
) {
    let size = chart.font_size.clamp(6., 60.);
    let mut top = 0.;
    let mut bottom = h;
    let pie = matches!(chart.chart, ChartKind::Pie | ChartKind::Doughnut);
    let label = |out: &mut String,
                 x: f32,
                 y: f32,
                 value: &str,
                 anchor: &str,
                 bold: bool,
                 scale: f32| {
        let _ = write!(
            out,
            r##"<text x="{}" y="{}" font-family="{}" font-size="{}" font-weight="{}" fill="#{}" text-anchor="{anchor}">{}</text>"##,
            n(x),
            n(y),
            attr(font),
            n(size * scale),
            if bold { "bold" } else { "normal" },
            text.hex(),
            escape(value)
        );
    };
    if !chart.title.is_empty() {
        label(out, w / 2., size * 1.2, &chart.title, "middle", true, 1.25);
        top += size * 2.;
    }
    // The legend: each series (each category, for pies).
    let names: Vec<String> = if pie {
        chart.categories.clone()
    } else {
        chart
            .series
            .iter()
            .map(|series| series.name.clone())
            .collect()
    };
    if chart.legend && !names.is_empty() && (pie || names.len() > 1 || !names[0].is_empty()) {
        bottom -= size * 1.8;
        let widths: Vec<f32> = names
            .iter()
            .map(|name| size * 1.2 + name.chars().count() as f32 * size * 0.55 + size)
            .collect();
        let total: f32 = widths.iter().sum();
        let mut x = (w - total).max(0.) / 2.;
        let y = h - size * 0.6;
        for (ix, name) in names.iter().enumerate() {
            let color = series_color(chart, theme, ix);
            let _ = write!(
                out,
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" fill="#{}"/>"##,
                n(x),
                n(y - size * 0.8),
                n(size * 0.8),
                n(size * 0.8),
                n(size * 0.15),
                color.hex()
            );
            label(out, x + size * 1.1, y, name, "start", false, 1.);
            x += widths[ix];
        }
    }
    if pie {
        draw_pie(chart, w, top, bottom, theme, font, text, size, out);
        return;
    }
    if chart.series.is_empty() {
        return;
    }
    let horizontal = chart.chart == ChartKind::Bar;
    let scatter = chart.chart == ChartKind::Scatter;
    let stacked = chart.chart == ChartKind::StackedColumn;
    let categories = if scatter {
        0
    } else {
        chart.categories.len().max(
            chart
                .series
                .iter()
                .map(|series| series.values.len())
                .max()
                .unwrap_or(0),
        )
    };
    // The value range.
    let values: Vec<f64> = if stacked {
        (0..categories)
            .flat_map(|ix| {
                let (up, down) = chart.series.iter().fold((0., 0.), |(up, down), series| {
                    let value = series.values.get(ix).copied().unwrap_or(0.);
                    if value >= 0. {
                        (up + value, down)
                    } else {
                        (up, down + value)
                    }
                });
                [up, down]
            })
            .collect()
    } else {
        chart
            .series
            .iter()
            .flat_map(|series| series.values.iter().copied())
            .collect()
    };
    let (low, high) = values.iter().fold((0f64, 0f64), |(low, high), value| {
        (low.min(*value), high.max(*value))
    });
    let (low, high, step) = nice(low, high);
    // Room for the value axis's labels, and the category axis's.
    let widest = ticks(low, high, step)
        .map(|tick| format_value(tick).chars().count())
        .max()
        .unwrap_or(1) as f32;
    let value_axis = widest * size * 0.6 + size * 0.8;
    let category_axis = size * 1.8;
    let (left, right, plot_top, plot_bottom) = if horizontal {
        let longest = chart
            .categories
            .iter()
            .map(|category| category.chars().count())
            .max()
            .unwrap_or(1) as f32;
        (
            (longest * size * 0.55 + size).min(w * 0.35),
            w - size,
            top + size * 0.5,
            bottom - category_axis,
        )
    } else {
        (
            value_axis,
            w - size * 0.5,
            top + size * 0.5,
            bottom - category_axis,
        )
    };
    let (pw, ph) = (right - left, plot_bottom - plot_top);
    if pw <= 1. || ph <= 1. {
        return;
    }
    let grid = Rgba { a: 50, ..text };
    let along = |value: f64| ((value - low) / (high - low)) as f32;
    // Gridlines and the value axis's labels.
    for tick in ticks(low, high, step) {
        let t = along(tick);
        if horizontal {
            let x = left + t * pw;
            line(out, x, plot_top, x, plot_bottom, grid, 0.75);
            label(
                out,
                x,
                plot_bottom + size * 1.3,
                &format_value(tick),
                "middle",
                false,
                0.9,
            );
        } else {
            let y = plot_bottom - t * ph;
            line(out, left, y, right, y, grid, 0.75);
            label(
                out,
                left - size * 0.4,
                y + size * 0.35,
                &format_value(tick),
                "end",
                false,
                0.9,
            );
        }
    }
    if scatter {
        draw_scatter(
            chart, theme, left, plot_top, pw, ph, low, high, size, text, out,
        );
        return;
    }
    let zero = along(0.);
    let slot = if horizontal { ph } else { pw } / categories.max(1) as f32;
    for category in 0..categories {
        let name = chart.categories.get(category).cloned().unwrap_or_default();
        let middle = category as f32 * slot + slot / 2.;
        if horizontal {
            label(
                out,
                left - size * 0.4,
                plot_top + middle + size * 0.35,
                &name,
                "end",
                false,
                0.9,
            );
        } else {
            label(
                out,
                left + middle,
                plot_bottom + size * 1.3,
                &name,
                "middle",
                false,
                0.9,
            );
        }
    }
    match chart.chart {
        ChartKind::Line | ChartKind::Area => {
            for (ix, series) in chart.series.iter().enumerate() {
                let color = series_color(chart, theme, ix);
                let points: Vec<(f32, f32)> = series
                    .values
                    .iter()
                    .enumerate()
                    .map(|(category, value)| {
                        (
                            left + category as f32 * slot + slot / 2.,
                            plot_bottom - along(*value) * ph,
                        )
                    })
                    .collect();
                if points.is_empty() {
                    continue;
                }
                let mut d = String::new();
                for (pix, (x, y)) in points.iter().enumerate() {
                    let _ = write!(
                        d,
                        "{}{} {} ",
                        if pix == 0 { "M" } else { "L" },
                        n(*x),
                        n(*y)
                    );
                }
                if chart.chart == ChartKind::Area {
                    let base = plot_bottom - zero * ph;
                    let (first, last) = (points[0], points[points.len() - 1]);
                    let _ = write!(
                        out,
                        r##"<path d="{d}L{} {} L{} {} Z" fill="#{}" fill-opacity="0.35"/>"##,
                        n(last.0),
                        n(base),
                        n(first.0),
                        n(base),
                        color.hex()
                    );
                }
                let _ = write!(
                    out,
                    r##"<path d="{d}" fill="none" stroke="#{}" stroke-width="{}" stroke-linejoin="round" stroke-linecap="round"/>"##,
                    color.hex(),
                    n(size * 0.22)
                );
                for (pix, (x, y)) in points.iter().enumerate() {
                    if chart.chart == ChartKind::Line {
                        let _ = write!(
                            out,
                            r##"<circle cx="{}" cy="{}" r="{}" fill="#{}"/>"##,
                            n(*x),
                            n(*y),
                            n(size * 0.3),
                            color.hex()
                        );
                    }
                    if chart.labels {
                        label(
                            out,
                            *x,
                            y - size * 0.6,
                            &format_value(series.values[pix]),
                            "middle",
                            false,
                            0.85,
                        );
                    }
                }
            }
        }
        _ => {
            let count = if stacked { 1 } else { chart.series.len() };
            let bar = slot * 0.72 / count as f32;
            let start = slot * 0.14;
            for category in 0..categories {
                let (mut up, mut down) = (0f64, 0f64);
                for (ix, series) in chart.series.iter().enumerate() {
                    let Some(value) = series.values.get(category).copied() else {
                        continue;
                    };
                    let color = series_color(chart, theme, ix);
                    let (from, to) = if stacked {
                        if value >= 0. {
                            up += value;
                            (up - value, up)
                        } else {
                            down += value;
                            (down - value, down)
                        }
                    } else {
                        (0., value)
                    };
                    let (a, b) = (along(from.min(to)), along(from.max(to)));
                    let offset =
                        category as f32 * slot + start + if stacked { 0. } else { ix as f32 * bar };
                    let (x, y, bw, bh) = if horizontal {
                        (left + a * pw, plot_top + offset, (b - a) * pw, bar)
                    } else {
                        (left + offset, plot_bottom - b * ph, bar, (b - a) * ph)
                    };
                    let _ = write!(
                        out,
                        r##"<rect x="{}" y="{}" width="{}" height="{}" fill="#{}"/>"##,
                        n(x),
                        n(y),
                        n(bw.max(0.)),
                        n(bh.max(0.)),
                        color.hex()
                    );
                    if chart.labels {
                        let text = format_value(value);
                        if horizontal {
                            label(
                                out,
                                x + bw + size * 0.3,
                                y + bh / 2. + size * 0.35,
                                &text,
                                "start",
                                false,
                                0.85,
                            );
                        } else {
                            label(
                                out,
                                x + bw / 2.,
                                y - size * 0.4,
                                &text,
                                "middle",
                                false,
                                0.85,
                            );
                        }
                    }
                }
            }
        }
    }
    // The axis line at zero.
    if horizontal {
        let x = left + zero * pw;
        line(
            out,
            x,
            plot_top,
            x,
            plot_bottom,
            Rgba { a: 140, ..text },
            1.,
        );
    } else {
        let y = plot_bottom - zero * ph;
        line(out, left, y, right, y, Rgba { a: 140, ..text }, 1.);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_pie(
    chart: &Chart,
    w: f32,
    top: f32,
    bottom: f32,
    theme: &Theme,
    font: &str,
    text: Rgba,
    size: f32,
    out: &mut String,
) {
    let Some(series) = chart.series.first() else {
        return;
    };
    let values: Vec<f64> = series.values.iter().map(|value| value.max(0.)).collect();
    let total: f64 = values.iter().sum();
    if total <= 0. {
        return;
    }
    let radius = ((bottom - top) / 2. - size * 0.5)
        .min(w / 2. - size)
        .max(1.);
    let (cx, cy) = (w / 2., top + (bottom - top) / 2.);
    let inner = if chart.chart == ChartKind::Doughnut {
        radius * 0.55
    } else {
        0.
    };
    let mut angle = -std::f64::consts::FRAC_PI_2;
    for (ix, value) in values.iter().enumerate() {
        let sweep = value / total * std::f64::consts::TAU;
        if sweep <= 0. {
            continue;
        }
        let color = series_color(chart, theme, ix);
        let end = angle + sweep;
        let large = if sweep > std::f64::consts::PI { 1 } else { 0 };
        let point = |r: f32, a: f64| (cx + r * a.cos() as f32, cy + r * a.sin() as f32);
        let d = if sweep >= std::f64::consts::TAU - 1e-6 {
            // The whole circle (an arc cannot end where it starts).
            format!(
                "M{} {} A{r} {r} 0 1 1 {} {} A{r} {r} 0 1 1 {} {} Z",
                n(cx - radius),
                n(cy),
                n(cx + radius),
                n(cy),
                n(cx - radius),
                n(cy),
                r = n(radius)
            )
        } else {
            let (x1, y1) = point(radius, angle);
            let (x2, y2) = point(radius, end);
            if inner > 0. {
                let (x3, y3) = point(inner, end);
                let (x4, y4) = point(inner, angle);
                format!(
                    "M{} {} A{r} {r} 0 {large} 1 {} {} L{} {} A{i} {i} 0 {large} 0 {} {} Z",
                    n(x1),
                    n(y1),
                    n(x2),
                    n(y2),
                    n(x3),
                    n(y3),
                    n(x4),
                    n(y4),
                    r = n(radius),
                    i = n(inner)
                )
            } else {
                format!(
                    "M{} {} L{} {} A{r} {r} 0 {large} 1 {} {} Z",
                    n(cx),
                    n(cy),
                    n(x1),
                    n(y1),
                    n(x2),
                    n(y2),
                    r = n(radius)
                )
            }
        };
        let background = theme.resolve(&Color::from("light1"));
        let _ = write!(
            out,
            r##"<path d="{d}" fill="#{}" stroke="#{}" stroke-width="{}"/>"##,
            color.hex(),
            background.hex(),
            n(size * 0.12)
        );
        if chart.labels {
            let middle = angle + sweep / 2.;
            let at = if inner > 0. {
                (radius + inner) / 2.
            } else {
                radius * 0.65
            };
            let (x, y) = point(at, middle);
            let share = format!("{:.0}%", value / total * 100.);
            let ink = if color.luminance() > 0.45 {
                Rgba::BLACK
            } else {
                Rgba::WHITE
            };
            let _ = write!(
                out,
                r##"<text x="{}" y="{}" font-family="{}" font-size="{}" font-weight="bold" fill="#{}" text-anchor="middle">{share}</text>"##,
                n(x),
                n(y + size * 0.35),
                attr(font),
                n(size),
                ink.hex()
            );
        }
        angle = end;
    }
    let _ = text;
}

#[allow(clippy::too_many_arguments)]
fn draw_scatter(
    chart: &Chart,
    theme: &Theme,
    left: f32,
    top: f32,
    pw: f32,
    ph: f32,
    low: f64,
    high: f64,
    size: f32,
    text: Rgba,
    out: &mut String,
) {
    let xs: Vec<f64> = chart.series.iter().flat_map(x_values).collect();
    let (x_low, x_high) = xs.iter().fold((f64::MAX, f64::MIN), |(low, high), x| {
        (low.min(*x), high.max(*x))
    });
    if x_low > x_high {
        return;
    }
    let (x_low, x_high, _) = nice(x_low.min(0.), x_high);
    for (ix, series) in chart.series.iter().enumerate() {
        let color = series_color(chart, theme, ix);
        for (x, y) in x_values(series).into_iter().zip(&series.values) {
            let px = left + ((x - x_low) / (x_high - x_low)) as f32 * pw;
            let py = top + ph - ((y - low) / (high - low)) as f32 * ph;
            let _ = write!(
                out,
                r##"<circle cx="{}" cy="{}" r="{}" fill="#{}" fill-opacity="0.85"/>"##,
                n(px),
                n(py),
                n(size * 0.4),
                color.hex()
            );
        }
    }
    line(
        out,
        left,
        top + ph,
        left + pw,
        top + ph,
        Rgba { a: 140, ..text },
        1.,
    );
}

fn x_values(series: &super::model::Series) -> Vec<f64> {
    if series.x.len() == series.values.len() {
        series.x.clone()
    } else {
        (1..=series.values.len()).map(|ix| ix as f64).collect()
    }
}

fn line(out: &mut String, x1: f32, y1: f32, x2: f32, y2: f32, color: Rgba, width: f32) {
    let _ = write!(
        out,
        r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="#{}" stroke-opacity="{:.2}" stroke-width="{}"/>"##,
        n(x1),
        n(y1),
        n(x2),
        n(y2),
        color.hex(),
        color.a as f32 / 255.,
        n(width)
    );
}

/// A rounded range taking `low`..`high`, and the step between its ticks.
pub fn nice(low: f64, high: f64) -> (f64, f64, f64) {
    let span = (high - low).abs().max(1e-9);
    let rough = span / 5.;
    let magnitude = 10f64.powf(rough.log10().floor());
    let step = [1., 2., 2.5, 5., 10.]
        .iter()
        .map(|factor| factor * magnitude)
        .find(|step| *step >= rough)
        .unwrap_or(10. * magnitude);
    let low = (low / step).floor() * step;
    let mut high = (high / step).ceil() * step;
    if high <= low {
        high = low + step;
    }
    (low, high, step)
}

fn ticks(low: f64, high: f64, step: f64) -> impl Iterator<Item = f64> {
    let count = ((high - low) / step).round() as usize;
    (0..=count.min(50)).map(move |ix| low + ix as f64 * step)
}

/// A value as a short label: 1.2K, 3.4M, 0.25.
pub fn format_value(value: f64) -> String {
    let abs = value.abs();
    let (scaled, suffix) = if abs >= 1e9 {
        (value / 1e9, "B")
    } else if abs >= 1e6 {
        (value / 1e6, "M")
    } else if abs >= 1e4 {
        (value / 1e3, "K")
    } else {
        (value, "")
    };
    let text = format!("{scaled:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{}{suffix}", if text == "-0" { "0" } else { text })
}

/// `text` for inside an XML attribute.
pub fn attr(text: &str) -> String {
    format!("'{}'", escape(text).replace('\'', "&apos;"))
}

/// `text` for XML.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Characters XML forbids (control codes) are left out.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_and_labels() {
        assert_eq!(nice(0., 87.), (0., 100., 20.));
        assert_eq!(nice(-3., 9.), (-5., 10., 2.5));
        assert_eq!(format_value(1500.), "1500");
        assert_eq!(format_value(15000.), "15K");
        assert_eq!(format_value(2_500_000.), "2.5M");
        assert_eq!(format_value(0.25), "0.25");
        assert_eq!(escape("a<b & \"c\""), "a&lt;b &amp; &quot;c&quot;");
    }
}
