//! The themes that come with PilotHouse, and the layouts every theme has.
//! Layouts are made for the deck's size, so a portrait or square deck gets
//! places that suit it. Fonts are ones macOS and Windows both have (Linux
//! gets look-alikes, see `fonts`).

use super::model::*;

/// A built-in theme: id, name, its colours (`dark1`, `light1`, `dark2`,
/// `light2`, `accent1`..`accent6`), fonts (heading, body), whether its
/// slides are dark, and its background.
struct Spec {
    id: &'static str,
    name: &'static str,
    colors: [&'static str; 10],
    heading: &'static str,
    body: &'static str,
    dark: bool,
    background: Background,
    /// A bar of accent colour on content slides.
    bar: bool,
    title_bold: bool,
}

enum Background {
    Slot(&'static str),
    Gradient(&'static str, &'static str, f32),
}

const SPECS: &[Spec] = &[
    Spec {
        id: "clean",
        name: "Clean",
        colors: [
            "#1F2937", "#FFFFFF", "#374151", "#F3F4F6", "#2563EB", "#0EA5E9", "#10B981", "#F59E0B",
            "#EF4444", "#8B5CF6",
        ],
        heading: "Arial",
        body: "Arial",
        dark: false,
        background: Background::Slot("light1"),
        bar: false,
        title_bold: true,
    },
    Spec {
        id: "harbor",
        name: "Harbor",
        colors: [
            "#1E1B4B", "#FFFFFF", "#312E81", "#EEF2FF", "#6D28D9", "#4F46E5", "#0891B2", "#DB2777",
            "#F59E0B", "#059669",
        ],
        heading: "Trebuchet MS",
        body: "Arial",
        dark: false,
        background: Background::Slot("light1"),
        bar: true,
        title_bold: true,
    },
    Spec {
        id: "midnight",
        name: "Midnight",
        colors: [
            "#0B1220", "#F8FAFC", "#1E293B", "#CBD5E1", "#38BDF8", "#A78BFA", "#34D399", "#FBBF24",
            "#F472B6", "#FB7185",
        ],
        heading: "Arial",
        body: "Arial",
        dark: true,
        background: Background::Gradient("#0B1220", "#172036", 90.),
        bar: false,
        title_bold: true,
    },
    Spec {
        id: "slate",
        name: "Slate",
        colors: [
            "#18181B", "#FAFAFA", "#27272A", "#D4D4D8", "#F59E0B", "#3B82F6", "#22C55E", "#EF4444",
            "#A855F7", "#14B8A6",
        ],
        heading: "Verdana",
        body: "Verdana",
        dark: true,
        background: Background::Slot("dark1"),
        bar: true,
        title_bold: true,
    },
    Spec {
        id: "paper",
        name: "Paper",
        colors: [
            "#2B2118", "#FBF7F0", "#5B4636", "#EFE6D8", "#B45309", "#7C2D12", "#4D7C0F", "#1D4ED8",
            "#9F1239", "#0F766E",
        ],
        heading: "Georgia",
        body: "Georgia",
        dark: false,
        background: Background::Slot("light1"),
        bar: false,
        title_bold: false,
    },
    Spec {
        id: "bold",
        name: "Bold",
        colors: [
            "#000000", "#FFFFFF", "#111111", "#E5E5E5", "#FACC15", "#F43F5E", "#22D3EE", "#A3E635",
            "#FB923C", "#C084FC",
        ],
        heading: "Arial Black",
        body: "Arial",
        dark: true,
        background: Background::Slot("dark1"),
        bar: false,
        title_bold: false,
    },
    Spec {
        id: "corporate",
        name: "Corporate",
        colors: [
            "#0F172A", "#FFFFFF", "#1E3A8A", "#E2E8F0", "#1D4ED8", "#0369A1", "#15803D", "#B45309",
            "#B91C1C", "#6D28D9",
        ],
        heading: "Arial",
        body: "Arial",
        dark: false,
        background: Background::Slot("light1"),
        bar: true,
        title_bold: true,
    },
    Spec {
        id: "sunset",
        name: "Sunset",
        colors: [
            "#2A1033", "#FFFFFF", "#4C1D3D", "#FFE4E6", "#FDBA74", "#FDE68A", "#F9A8D4", "#A5F3FC",
            "#C4B5FD", "#BBF7D0",
        ],
        heading: "Trebuchet MS",
        body: "Trebuchet MS",
        dark: true,
        background: Background::Gradient("#7C2D92", "#F97316", 30.),
        bar: false,
        title_bold: true,
    },
    Spec {
        id: "forest",
        name: "Forest",
        colors: [
            "#0F2A1D", "#F4F7F2", "#1F4D36", "#DDE8D8", "#2F855A", "#B7791F", "#2B6CB0", "#C05621",
            "#6B46C1", "#9B2C2C",
        ],
        heading: "Georgia",
        body: "Verdana",
        dark: false,
        background: Background::Slot("light1"),
        bar: true,
        title_bold: false,
    },
    Spec {
        id: "mono",
        name: "Mono",
        colors: [
            "#111111", "#FFFFFF", "#333333", "#EEEEEE", "#111111", "#555555", "#888888", "#BBBBBB",
            "#2563EB", "#DC2626",
        ],
        heading: "Arial",
        body: "Arial",
        dark: false,
        background: Background::Slot("light1"),
        bar: false,
        title_bold: true,
    },
    Spec {
        id: "pastel",
        name: "Pastel",
        colors: [
            "#334155", "#FFFDF8", "#475569", "#F1F5F9", "#F472B6", "#60A5FA", "#34D399", "#FBBF24",
            "#A78BFA", "#FB923C",
        ],
        heading: "Trebuchet MS",
        body: "Trebuchet MS",
        dark: false,
        background: Background::Gradient("#FFFDF8", "#F3F0FF", 45.),
        bar: false,
        title_bold: true,
    },
    Spec {
        id: "ocean",
        name: "Ocean",
        colors: [
            "#03192E", "#F0F9FF", "#0C4A6E", "#BAE6FD", "#22D3EE", "#38BDF8", "#A7F3D0", "#FDE68A",
            "#F9A8D4", "#C7D2FE",
        ],
        heading: "Verdana",
        body: "Verdana",
        dark: true,
        background: Background::Gradient("#03192E", "#0E4A7A", 60.),
        bar: false,
        title_bold: true,
    },
];

/// The built-in themes: id and name.
pub fn builtins() -> impl Iterator<Item = (&'static str, &'static str)> {
    SPECS.iter().map(|spec| (spec.id, spec.name))
}

/// Built-in theme `id` with its layouts for a 16:9 deck.
pub fn builtin(id: &str) -> Option<Theme> {
    builtin_for(id, &SlideSize::default())
}

/// Built-in theme `id` with its layouts made for `size`.
pub fn builtin_for(id: &str, size: &SlideSize) -> Option<Theme> {
    let spec = SPECS.iter().find(|spec| spec.id == id)?;
    let names = [
        "dark1", "light1", "dark2", "light2", "accent1", "accent2", "accent3", "accent4",
        "accent5", "accent6",
    ];
    let mut colors: Vec<(String, String)> = names
        .iter()
        .zip(spec.colors)
        .map(|(name, hex)| (name.to_string(), hex.to_string()))
        .collect();
    let (link, followed) = if spec.dark {
        ("#7DD3FC", "#C4B5FD")
    } else {
        ("#2563EB", "#7C3AED")
    };
    colors.push(("link".into(), link.into()));
    colors.push(("followed_link".into(), followed.into()));
    // Text is dark on light slides, light on dark ones.
    let (text, soft) = if spec.dark {
        ("light1", "light2")
    } else {
        ("dark1", "dark2")
    };
    let background = match spec.background {
        Background::Slot(slot) => Fill::solid(slot),
        Background::Gradient(from, to, angle) => Fill::Gradient {
            stops: vec![
                Stop {
                    at: 0.,
                    color: Color::from(from),
                },
                Stop {
                    at: 1.,
                    color: Color::from(to),
                },
            ],
            angle,
            radial: false,
        },
    };
    let scale = text_scale(size);
    let title = TextStyle {
        font: "heading".into(),
        size: (40. * scale).round(),
        color: Color::from(text),
        bold: spec.title_bold,
        italic: false,
        align: Align::Left,
        line_spacing: 1.,
        space_after: 0.,
        bullets: Vec::new(),
    };
    let body = TextStyle {
        font: "body".into(),
        size: (22. * scale).round(),
        color: Color::from(soft),
        bold: false,
        italic: false,
        align: Align::Left,
        line_spacing: 1.15,
        space_after: (8. * scale).round(),
        bullets: vec![
            Bullet::Char("•".into()),
            Bullet::Char("–".into()),
            Bullet::Char("•".into()),
            Bullet::Char("–".into()),
        ],
    };
    let mut theme = Theme {
        name: spec.name.into(),
        colors,
        fonts: Fonts {
            heading: spec.heading.into(),
            body: spec.body.into(),
            code: "Courier New".into(),
        },
        background,
        decorations: Vec::new(),
        title,
        body,
        layouts: Vec::new(),
    };
    theme.layouts = layouts(size, spec.bar, spec.dark);
    Some(theme)
}

/// How much bigger or smaller text is on `size` than on a 16:9 slide.
pub fn text_scale(size: &SlideSize) -> f32 {
    (size.width.min(size.height) / 540.).clamp(0.5, 2.)
}

/// The layouts every theme has, for `size`.
pub fn layouts(size: &SlideSize, bar: bool, dark: bool) -> Vec<Layout> {
    let (w, h) = (size.width, size.height);
    let portrait = h > w * 1.05;
    let scale = text_scale(size);
    let margin = w.min(h) * 0.07;
    let inner = w - margin * 2.;
    let gap = margin * 0.5;
    let frame = |x: f32, y: f32, fw: f32, fh: f32| Frame::new(x, y, fw, fh);
    let place = |kind, frame: Frame, prompt: &str| Placeholder {
        kind,
        frame,
        prompt: prompt.into(),
        ..Placeholder::default()
    };
    let title_h = h * if portrait { 0.12 } else { 0.16 };
    let title_y = margin * 0.8;
    let content_y = title_y + title_h + gap;
    let content_h = h - content_y - margin;
    let title = || Placeholder {
        anchor: Anchor::Bottom,
        ..place(
            PlaceholderKind::Title,
            frame(margin, title_y, inner, title_h),
            "Title",
        )
    };
    let body = |frame: Frame, kind| Placeholder {
        bullets: true,
        ..place(kind, frame, "Text")
    };
    // A thin accent bar under content titles, when the theme has one.
    let accent_bar = || {
        bar.then(|| Element {
            id: "bar".into(),
            name: "Accent bar".into(),
            frame: frame(margin, title_y + title_h + gap * 0.35, w * 0.08, 4. * scale),
            kind: Kind::Shape(Shape {
                fill: Fill::solid("accent1"),
                ..Shape::default()
            }),
            ..Element::default()
        })
        .into_iter()
        .collect::<Vec<_>>()
    };
    let half = (inner - gap) / 2.;
    let (left, right) = if portrait {
        let half_h = (content_h - gap) / 2.;
        (
            frame(margin, content_y, inner, half_h),
            frame(margin, content_y + half_h + gap, inner, half_h),
        )
    } else {
        (
            frame(margin, content_y, half, content_h),
            frame(margin + half + gap, content_y, half, content_h),
        )
    };
    let heading_h = 40. * scale;
    let compare = |column: Frame| {
        (
            frame(column.x, column.y, column.w, heading_h),
            frame(
                column.x,
                column.y + heading_h + gap * 0.5,
                column.w,
                column.h - heading_h - gap * 0.5,
            ),
        )
    };
    let (heading_left, body_left) = compare(left);
    let (heading_right, body_right) = compare(right);
    let centered = |kind, frame: Frame, prompt: &str, size: f32| Placeholder {
        align: Some(Align::Center),
        anchor: Anchor::Middle,
        size: Some((size * scale).round()),
        ..place(kind, frame, prompt)
    };
    let section_fill = if dark {
        Fill::solid("dark2")
    } else {
        Fill::solid("accent1")
    };
    let picture_w = if portrait { inner } else { w * 0.5 };
    vec![
        Layout {
            id: "title".into(),
            name: "Title".into(),
            background: None,
            decorations: Vec::new(),
            placeholders: vec![
                Placeholder {
                    anchor: Anchor::Bottom,
                    size: Some((54. * scale).round()),
                    ..place(
                        PlaceholderKind::Title,
                        frame(margin, h * 0.22, inner, h * 0.34),
                        "Presentation title",
                    )
                },
                Placeholder {
                    size: Some((24. * scale).round()),
                    ..place(
                        PlaceholderKind::Subtitle,
                        frame(margin, h * 0.58, inner, h * 0.18),
                        "Subtitle",
                    )
                },
            ],
            ..Layout::default()
        },
        Layout {
            id: "title_content".into(),
            name: "Title and content".into(),
            background: None,
            decorations: accent_bar(),
            placeholders: vec![
                title(),
                body(
                    frame(margin, content_y, inner, content_h),
                    PlaceholderKind::Body,
                ),
            ],
            ..Layout::default()
        },
        Layout {
            id: "section".into(),
            name: "Section".into(),
            background: Some(section_fill),
            decorations: Vec::new(),
            placeholders: vec![
                Placeholder {
                    anchor: Anchor::Bottom,
                    size: Some((48. * scale).round()),
                    color: Some(Color::from("light1")),
                    ..place(
                        PlaceholderKind::Title,
                        frame(margin, h * 0.3, inner, h * 0.3),
                        "Section title",
                    )
                },
                Placeholder {
                    color: Some(Color::from("light2")),
                    ..place(
                        PlaceholderKind::Subtitle,
                        frame(margin, h * 0.62, inner, h * 0.15),
                        "What this part covers",
                    )
                },
            ],
            ..Layout::default()
        },
        Layout {
            id: "two_content".into(),
            name: "Two columns".into(),
            background: None,
            decorations: accent_bar(),
            placeholders: vec![
                title(),
                body(left, PlaceholderKind::Body),
                body(right, PlaceholderKind::Body2),
            ],
            ..Layout::default()
        },
        Layout {
            id: "comparison".into(),
            name: "Comparison".into(),
            background: None,
            decorations: accent_bar(),
            placeholders: vec![
                title(),
                Placeholder {
                    bold: Some(true),
                    color: Some(Color::from("accent1")),
                    autofit: Autofit::Shrink,
                    ..place(PlaceholderKind::Heading, heading_left, "Heading")
                },
                body(body_left, PlaceholderKind::Body),
                Placeholder {
                    bold: Some(true),
                    color: Some(Color::from("accent2")),
                    autofit: Autofit::Shrink,
                    ..place(PlaceholderKind::Heading2, heading_right, "Heading")
                },
                body(body_right, PlaceholderKind::Body2),
            ],
            ..Layout::default()
        },
        Layout {
            id: "title_only".into(),
            name: "Title only".into(),
            background: None,
            decorations: accent_bar(),
            placeholders: vec![title()],
            ..Layout::default()
        },
        Layout {
            id: "statement".into(),
            name: "Statement".into(),
            background: None,
            decorations: Vec::new(),
            placeholders: vec![
                centered(
                    PlaceholderKind::Statement,
                    frame(margin, h * 0.2, inner, h * 0.42),
                    "A big number or statement",
                    60.,
                ),
                centered(
                    PlaceholderKind::Caption,
                    frame(margin, h * 0.64, inner, h * 0.14),
                    "What it means",
                    22.,
                ),
            ],
            ..Layout::default()
        },
        Layout {
            id: "quote".into(),
            name: "Quote".into(),
            background: None,
            decorations: Vec::new(),
            placeholders: vec![
                Placeholder {
                    size: Some((34. * scale).round()),
                    anchor: Anchor::Middle,
                    ..place(
                        PlaceholderKind::Statement,
                        frame(margin * 1.6, h * 0.18, w - margin * 3.2, h * 0.46),
                        "“A quote worth repeating.”",
                    )
                },
                Placeholder {
                    align: Some(Align::Right),
                    color: Some(Color::from("accent1")),
                    ..place(
                        PlaceholderKind::Caption,
                        frame(margin * 1.6, h * 0.68, w - margin * 3.2, h * 0.1),
                        "— Who said it",
                    )
                },
            ],
            ..Layout::default()
        },
        Layout {
            id: "picture_caption".into(),
            name: "Picture with caption".into(),
            background: None,
            decorations: Vec::new(),
            placeholders: if portrait {
                vec![
                    place(
                        PlaceholderKind::Picture,
                        frame(0., 0., w, h * 0.55),
                        "Picture",
                    ),
                    Placeholder {
                        anchor: Anchor::Bottom,
                        ..place(
                            PlaceholderKind::Title,
                            frame(margin, h * 0.58, inner, h * 0.14),
                            "Title",
                        )
                    },
                    place(
                        PlaceholderKind::Caption,
                        frame(margin, h * 0.74, inner, h * 0.2),
                        "Caption",
                    ),
                ]
            } else {
                vec![
                    place(
                        PlaceholderKind::Picture,
                        frame(0., 0., picture_w, h),
                        "Picture",
                    ),
                    Placeholder {
                        anchor: Anchor::Bottom,
                        ..place(
                            PlaceholderKind::Title,
                            frame(
                                picture_w + margin,
                                h * 0.2,
                                w - picture_w - margin * 2.,
                                h * 0.3,
                            ),
                            "Title",
                        )
                    },
                    place(
                        PlaceholderKind::Caption,
                        frame(
                            picture_w + margin,
                            h * 0.54,
                            w - picture_w - margin * 2.,
                            h * 0.3,
                        ),
                        "Caption",
                    ),
                ]
            },
            ..Layout::default()
        },
        Layout {
            id: "agenda".into(),
            name: "Agenda".into(),
            background: None,
            decorations: accent_bar(),
            placeholders: vec![
                title(),
                Placeholder {
                    bullets: true,
                    size: Some((26. * scale).round()),
                    ..place(
                        PlaceholderKind::Body,
                        frame(margin, content_y, inner, content_h),
                        "Topics, one a line",
                    )
                },
            ],
            ..Layout::default()
        },
        Layout {
            id: "closing".into(),
            name: "Closing".into(),
            background: None,
            decorations: Vec::new(),
            placeholders: vec![
                centered(
                    PlaceholderKind::Title,
                    frame(margin, h * 0.28, inner, h * 0.24),
                    "Thank you",
                    54.,
                ),
                centered(
                    PlaceholderKind::Subtitle,
                    frame(margin, h * 0.54, inner, h * 0.16),
                    "Questions, contacts, next steps",
                    22.,
                ),
            ],
            ..Layout::default()
        },
        Layout {
            id: "blank".into(),
            name: "Blank".into(),
            ..Layout::default()
        },
    ]
}

/// `theme` made for another slide size: its layouts and decorations
/// scaled, its text sizes kept in proportion.
pub fn resized(theme: &Theme, from: &SlideSize, to: &SlideSize) -> Theme {
    let mut theme = theme.clone();
    let (sx, sy) = (to.width / from.width, to.height / from.height);
    let text = text_scale(to) / text_scale(from);
    let scale_frame = |frame: &mut Frame| {
        frame.x *= sx;
        frame.y *= sy;
        frame.w *= sx;
        frame.h *= sy;
    };
    for element in &mut theme.decorations {
        scale_frame(&mut element.frame);
    }
    for layout in &mut theme.layouts {
        for element in &mut layout.decorations {
            scale_frame(&mut element.frame);
        }
        for place in &mut layout.placeholders {
            scale_frame(&mut place.frame);
            if let Some(size) = &mut place.size {
                *size = (*size * text).round();
            }
        }
    }
    theme.title.size = (theme.title.size * text).round();
    theme.body.size = (theme.body.size * text).round();
    theme
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_has_every_layout_inside_the_slide() {
        for (id, _) in builtins() {
            for (_, _, w, h) in SIZES {
                let size = SlideSize {
                    width: *w,
                    height: *h,
                    preset: None,
                };
                let theme = builtin_for(id, &size).unwrap();
                assert_eq!(theme.layouts.len(), 12, "{id}");
                assert_eq!(theme.colors.len(), 12);
                for layout in &theme.layouts {
                    for place in &layout.placeholders {
                        let f = place.frame;
                        assert!(
                            f.x >= -0.5
                                && f.y >= -0.5
                                && f.x + f.w <= w + 0.5
                                && f.y + f.h <= h + 0.5,
                            "{id} {} {:?} in {w}×{h}",
                            layout.id,
                            place.kind
                        );
                    }
                }
            }
        }
    }
}
