//! A slide as SVG: the one drawing behind the editor's picture,
//! thumbnails, presenting, PNG and PDF export, and the pictures agents look
//! at, so they all match. Text is laid out here (see `text`) and written
//! as positioned SVG text, so it stays text in a PDF.

use std::fmt::Write as _;
use std::path::Path;

use base64::Engine as _;

use super::charts::{self, attr, escape};
use super::fonts::fonts;
use super::model::*;
use super::shapes::{self, n};
use super::text::{self, Context};

/// How a slide is drawn.
#[derive(Debug, Clone, Default)]
pub struct Options<'a> {
    /// The space's `media` folder, where pictures are.
    pub media: Option<&'a Path>,
    /// For the editor: empty placeholders show what goes in them.
    pub editing: bool,
    /// An element whose text is left out (it is being typed in place).
    pub hide_text: Option<&'a str>,
    /// Pictures from their small copies (see [`super::space::SMALL`]), for
    /// drawings far smaller than the slide.
    pub small: bool,
    /// Elements left out (presenting: those not come in yet).
    pub hidden: &'a [String],
}

impl Options<'_> {
    /// The file of media `name`, if it is there (its small copy when
    /// asked and made).
    pub fn media_file(&self, name: &str) -> Option<std::path::PathBuf> {
        if !super::space::safe_media_name(name) {
            return None;
        }
        let media = self.media?;
        if self.small {
            let small = media.join(super::space::SMALL).join(name);
            if small.is_file() {
                return Some(small);
            }
        }
        let path = media.join(name);
        path.is_file().then_some(path)
    }
}

/// Slide `index` of `deck` as an SVG document, in points.
pub fn slide(deck: &Deck, index: usize, options: &Options) -> String {
    let Some(slide) = deck.slides.get(index) else {
        return String::new();
    };
    let (w, h) = (deck.size.width, deck.size.height);
    let mut svg = Svg {
        body: String::new(),
        defs: String::new(),
        next: 0,
        theme: &deck.theme,
        options,
    };
    let layout = deck.theme.layout(&slide.layout);
    // The background: the slide's, else its layout's, else the theme's.
    let background = slide
        .background
        .as_ref()
        .or(layout.and_then(|layout| layout.background.as_ref()))
        .unwrap_or(&deck.theme.background);
    let fill = svg.fill(background, w, h);
    let _ = write!(
        svg.body,
        r#"<rect width="{}" height="{}" {fill}/>"#,
        n(w),
        n(h)
    );
    if layout.is_none_or(|layout| layout.theme_decorations) {
        for element in &deck.theme.decorations {
            svg.element(element, None);
        }
    }
    if let Some(layout) = layout {
        for element in &layout.decorations {
            svg.element(element, None);
        }
    }
    for element in &slide.elements {
        if options.hidden.contains(&element.id) {
            continue;
        }
        let place = element
            .placeholder
            .and_then(|kind| layout.and_then(|layout| layout.placeholder(kind)));
        svg.element(element, place);
    }
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><defs>{}</defs>{}</svg>"#,
        svg.defs,
        svg.body,
        w = n(w),
        h = n(h)
    )
}

/// Element `id` of slide `index` alone, on nothing, framed by its outline
/// (a little more, for shadows): the SVG and that frame (slide points:
/// left, top, width, height). For an element coming in when presenting.
pub fn element_layer(
    deck: &Deck,
    index: usize,
    id: &str,
    options: &Options,
) -> Option<(String, [f32; 4])> {
    let slide = deck.slides.get(index)?;
    let element = slide.elements.iter().find(|element| element.id == id)?;
    let layout = deck.theme.layout(&slide.layout);
    let place = element
        .placeholder
        .and_then(|kind| layout.and_then(|layout| layout.placeholder(kind)));
    let [l, t, r, b] = super::editor::geometry::bounds(&element.frame);
    let margin = 24.;
    let (x, y) = ((l - margin).max(0.), (t - margin).max(0.));
    let (w, h) = (
        ((r + margin).min(deck.size.width) - x).max(1.),
        ((b + margin).min(deck.size.height) - y).max(1.),
    );
    let mut svg = Svg {
        body: String::new(),
        defs: String::new(),
        next: 0,
        theme: &deck.theme,
        options,
    };
    svg.element(element, place);
    Some((
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{}" height="{}" viewBox="{} {} {} {}"><defs>{}</defs>{}</svg>"#,
            n(w),
            n(h),
            n(x),
            n(y),
            n(w),
            n(h),
            svg.defs,
            svg.body
        ),
        [x, y, w, h],
    ))
}

struct Svg<'a> {
    body: String,
    defs: String,
    next: u32,
    theme: &'a Theme,
    options: &'a Options<'a>,
}

impl Svg<'_> {
    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    /// `fill="…"` (and its opacity) for `fill` over a `w` × `h` box.
    fn fill(&mut self, fill: &Fill, w: f32, h: f32) -> String {
        match fill {
            Fill::None => r#"fill="none""#.into(),
            Fill::Solid { color, opacity } => {
                let rgba = self.theme.resolve(color);
                paint("fill", rgba, *opacity)
            }
            Fill::Gradient {
                stops,
                angle,
                radial,
            } => {
                let id = self.id("g");
                let mut stops_svg = String::new();
                for stop in stops {
                    let rgba = self.theme.resolve(&stop.color);
                    let _ = write!(
                        stops_svg,
                        r##"<stop offset="{}" stop-color="#{}" stop-opacity="{:.3}"/>"##,
                        n(stop.at.clamp(0., 1.)),
                        rgba.hex(),
                        rgba.a as f32 / 255.
                    );
                }
                if *radial {
                    let _ = write!(
                        self.defs,
                        r#"<radialGradient id="{id}" cx="0.5" cy="0.5" r="0.71">{stops_svg}</radialGradient>"#
                    );
                } else {
                    let radians = angle.to_radians();
                    let (dx, dy) = (radians.cos() / 2., radians.sin() / 2.);
                    let _ = write!(
                        self.defs,
                        r#"<linearGradient id="{id}" x1="{}" y1="{}" x2="{}" y2="{}">{stops_svg}</linearGradient>"#,
                        n(0.5 - dx),
                        n(0.5 - dy),
                        n(0.5 + dx),
                        n(0.5 + dy)
                    );
                }
                format!(r#"fill="url(#{id})""#)
            }
            Fill::Picture { media, fit } => {
                let Some(href) = self.media(media) else {
                    return r#"fill="none""#.into();
                };
                let id = self.id("p");
                let _ = write!(
                    self.defs,
                    r#"<pattern id="{id}" patternUnits="userSpaceOnUse" width="{w}" height="{h}"><image href={href} width="{w}" height="{h}" preserveAspectRatio="{}"/></pattern>"#,
                    aspect(*fit),
                    w = n(w),
                    h = n(h)
                );
                format!(r#"fill="url(#{id})""#)
            }
        }
    }

    /// A media file's `href` (quoted), if it is there.
    fn media(&self, name: &str) -> Option<String> {
        self.options
            .media_file(name)
            .map(|path| attr(&path.to_string_lossy()))
    }

    fn stroke(&self, line: &Line) -> String {
        if line.width <= 0. {
            return r#"stroke="none""#.into();
        }
        let color = self
            .theme
            .resolve(line.color.as_ref().unwrap_or(&Color::from("dark1")));
        let w = line.width;
        let dash = match line.dash {
            Dash::Solid => String::new(),
            Dash::Dash => format!(r#" stroke-dasharray="{} {}""#, n(w * 4.), n(w * 3.)),
            Dash::Dot => format!(r#" stroke-dasharray="{} {}""#, n(w), n(w * 2.)),
            Dash::DashDot => format!(
                r#" stroke-dasharray="{} {} {} {}""#,
                n(w * 4.),
                n(w * 2.),
                n(w),
                n(w * 2.)
            ),
        };
        format!(
            r#"{} stroke-width="{}" stroke-linejoin="round"{dash}"#,
            paint("stroke", color, 1.),
            n(w)
        )
    }

    /// `filter="…"` for a shadow, made in `defs`.
    fn shadow(&mut self, shadow: Option<&Shadow>) -> String {
        let Some(shadow) = shadow else {
            return String::new();
        };
        let id = self.id("s");
        let color = self.theme.resolve(&shadow.color);
        let radians = shadow.angle.to_radians();
        let _ = write!(
            self.defs,
            r##"<filter id="{id}" x="-50%" y="-50%" width="200%" height="200%"><feDropShadow dx="{}" dy="{}" stdDeviation="{}" flood-color="#{}" flood-opacity="{:.3}"/></filter>"##,
            n(radians.cos() * shadow.distance),
            n(radians.sin() * shadow.distance),
            n(shadow.blur / 2.),
            color.hex(),
            shadow.opacity.clamp(0., 1.)
        );
        format!(r#" filter="url(#{id})""#)
    }

    fn element(&mut self, element: &Element, place: Option<&Placeholder>) {
        let frame = element.frame;
        if frame.w <= 0. && frame.h <= 0. {
            return;
        }
        let empty = super::ops::is_empty(element);
        if element.placeholder.is_some() && empty && !self.options.editing {
            return;
        }
        let (w, h) = (frame.w.max(0.), frame.h.max(0.));
        // Groups' children are placed on the slide, not in the group.
        if let Kind::Group(group) = &element.kind {
            let _ = write!(
                self.body,
                r#"<g opacity="{:.3}" transform="rotate({} {} {})">"#,
                element.opacity.clamp(0., 1.),
                n(frame.rotation),
                n(frame.x + w / 2.),
                n(frame.y + h / 2.)
            );
            for child in &group.children {
                self.element(child, None);
            }
            self.body.push_str("</g>");
            return;
        }
        let _ = write!(
            self.body,
            r#"<g opacity="{:.3}" transform="translate({} {}) rotate({} {} {})">"#,
            element.opacity.clamp(0., 1.),
            n(frame.x),
            n(frame.y),
            n(frame.rotation),
            n(w / 2.),
            n(h / 2.)
        );
        // Flips mirror the drawing, not the text.
        let flip = match (frame.flip_h, frame.flip_v) {
            (false, false) => None,
            (true, false) => Some(format!("translate({} 0) scale(-1 1)", n(w))),
            (false, true) => Some(format!("translate(0 {}) scale(1 -1)", n(h))),
            (true, true) => Some(format!("translate({} {}) scale(-1 -1)", n(w), n(h))),
        };
        let open_flip = |svg: &mut Svg| {
            if let Some(flip) = &flip {
                let _ = write!(svg.body, r#"<g transform="{flip}">"#);
            }
        };
        let close_flip = |svg: &mut Svg| {
            if flip.is_some() {
                svg.body.push_str("</g>");
            }
        };
        let hide_text = self.options.hide_text == Some(element.id.as_str());
        match &element.kind {
            Kind::Shape(shape) => {
                open_flip(self);
                self.shape(shape, w, h);
                close_flip(self);
                if let Some(body) = &shape.text
                    && !hide_text
                {
                    // Text reads on the fill: light on dark, dark on light.
                    let on = self.fill_color(&shape.fill);
                    self.text(body, place, on, w, h, None);
                }
            }
            Kind::Text(text) => {
                let fill = self.fill(&text.fill, w, h);
                let stroke = self.stroke(&text.line);
                if !matches!(text.fill, Fill::None) || text.line.width > 0. {
                    let _ = write!(
                        self.body,
                        r#"<rect width="{}" height="{}" {fill} {stroke}/>"#,
                        n(w),
                        n(h)
                    );
                }
                if empty && element.placeholder.is_some() {
                    self.prompt(place, w, h);
                } else if !hide_text {
                    let on = self.fill_color(&text.fill);
                    self.text(&text.text, place, on, w, h, None);
                }
            }
            Kind::Picture(picture) => {
                open_flip(self);
                self.picture(picture, w, h, place);
                close_flip(self);
            }
            Kind::Table(table) => self.table(table, w, h),
            Kind::Chart(chart) => {
                let font = fonts().family(&self.theme.fonts.body);
                let text = self.theme.resolve(&self.theme.body.color);
                charts::draw(chart, w, h, self.theme, &font, text, &mut self.body);
            }
            Kind::Code(code) => self.code(code, w, h),
            Kind::Icon(icon) => {
                open_flip(self);
                self.icon(icon, w, h);
                close_flip(self);
            }
            Kind::Svg(art) => {
                open_flip(self);
                self.art(&art.source, w, h);
                close_flip(self);
            }
            Kind::Raw(raw) => {
                // Its picture when the file had one, else a box saying
                // what it is: it is written back as it was.
                let shown = raw
                    .preview
                    .as_ref()
                    .is_some_and(|preview| self.options.media_file(preview).is_some());
                if shown {
                    let picture = Picture {
                        media: raw.preview.clone().unwrap_or_default(),
                        fit: Fit::Stretch,
                        ..Picture::default()
                    };
                    self.picture(&picture, w, h, None);
                } else {
                    let _ = write!(
                        self.body,
                        r##"<rect width="{}" height="{}" fill="#8080801F" stroke="#80808099" stroke-width="1" stroke-dasharray="4 3"/>"##,
                        n(w),
                        n(h)
                    );
                    let label = TextBody {
                        paragraphs: vec![Paragraph {
                            runs: vec![Run {
                                text: if raw.what.is_empty() {
                                    "PowerPoint object".into()
                                } else {
                                    raw.what.clone()
                                },
                                size: Some((h * 0.18).clamp(8., 16.)),
                                color: Some(Color::from("#808080")),
                                ..Run::default()
                            }],
                            align: Some(Align::Center),
                            bullet: Some(Bullet::None),
                            ..Paragraph::default()
                        }],
                        anchor: Anchor::Middle,
                        ..TextBody::default()
                    };
                    self.text(&label, None, None, w, h, None);
                }
            }
            Kind::Group(_) => unreachable!("drawn above"),
        }
        self.body.push_str("</g>");
    }

    /// The colour text needs on `fill`, when it is solid.
    fn fill_color(&self, fill: &Fill) -> Option<Rgba> {
        match fill {
            Fill::Solid { color, opacity } => {
                let rgba = self.theme.resolve(color);
                // A see-through fill shows what is behind more than itself.
                (rgba.a as f32 / 255. * opacity > 0.5).then(|| contrasting(rgba, self.theme))
            }
            Fill::Gradient { stops, .. } if !stops.is_empty() => {
                Some(contrasting(self.theme.resolve(&stops[0].color), self.theme))
            }
            _ => None,
        }
    }

    fn shape(&mut self, shape: &Shape, w: f32, h: f32) {
        let d = if shape.geometry == Geometry::Path {
            shape.path.clone().unwrap_or_default()
        } else {
            shapes::path(shape.geometry, w, h, shape.radius)
        };
        let line = shape.geometry == Geometry::Line;
        let fill = if line {
            r#"fill="none""#.into()
        } else {
            self.fill(&shape.fill, w, h)
        };
        let mut stroke = self.stroke(&shape.line);
        if line && shape.line.width <= 0. {
            // A line always shows.
            let color = match &shape.fill {
                Fill::Solid { color, .. } => color.clone(),
                _ => Color::from("dark1"),
            };
            stroke = self.stroke(&Line {
                color: Some(color),
                width: 2.,
                ..shape.line.clone()
            });
        }
        let filter = self.shadow(shape.shadow.as_ref());
        let _ = write!(
            self.body,
            r#"<path d="{}" {fill} {stroke} stroke-linecap="round"{filter}/>"#,
            escape(&d)
        );
        if line && (shape.line.head || shape.line.tail) {
            let width = shape.line.width.max(2.);
            let color = self
                .theme
                .resolve(shape.line.color.as_ref().unwrap_or(&Color::from("dark1")));
            let mut head = |from: (f32, f32), to: (f32, f32)| {
                let (dx, dy) = (to.0 - from.0, to.1 - from.1);
                let length = (dx * dx + dy * dy).sqrt().max(0.001);
                let (ux, uy) = (dx / length, dy / length);
                let size = width * 4.;
                let base = (to.0 - ux * size, to.1 - uy * size);
                let (px, py) = (-uy * size * 0.55, ux * size * 0.55);
                let _ = write!(
                    self.body,
                    r#"<path d="M{} {} L{} {} L{} {} Z" {}/>"#,
                    n(to.0),
                    n(to.1),
                    n(base.0 + px),
                    n(base.1 + py),
                    n(base.0 - px),
                    n(base.1 - py),
                    paint("fill", color, 1.)
                );
            };
            if shape.line.tail {
                head((0., 0.), (w, h));
            }
            if shape.line.head {
                head((w, h), (0., 0.));
            }
        }
    }

    /// Text in a `w` × `h` box; `on` is the colour that reads on what is
    /// behind it.
    fn text(
        &mut self,
        body: &TextBody,
        place: Option<&Placeholder>,
        on: Option<Rgba>,
        w: f32,
        h: f32,
        size: Option<(f32, bool)>,
    ) -> f32 {
        let [left, top, right, bottom] = body.insets;
        let (area_w, area_h) = ((w - left - right).max(1.), (h - top - bottom).max(1.));
        let paras = text::paragraphs(
            body,
            &Context {
                theme: self.theme,
                place,
                color: on,
                size: size.map(|(size, _)| size),
                bold: size.map(|(_, bold)| bold),
            },
        );
        let fit = (body.autofit == Autofit::Shrink).then_some(area_h);
        let laid = text::lay_out(&paras, area_w, body.wrap, fit);
        let offset = match body.anchor {
            Anchor::Top => 0.,
            Anchor::Middle => (area_h - laid.height) / 2.,
            Anchor::Bottom => area_h - laid.height,
        };
        let (x0, y0) = (left, top + offset);
        for line in &laid.lines {
            for (bullet, piece) in line
                .bullet
                .iter()
                .map(|piece| (true, piece))
                .chain(line.pieces.iter().map(|piece| (false, piece)))
            {
                if piece.text.is_empty() {
                    continue;
                }
                let style = &piece.style;
                let baseline = y0 + line.baseline
                    - match style.baseline {
                        0 => 0.,
                        up if up > 0 => style.size * 0.33,
                        _ => -style.size * 0.15,
                    };
                if let Some(highlight) = style.highlight {
                    let _ = write!(
                        self.body,
                        r#"<rect x="{}" y="{}" width="{}" height="{}" {}/>"#,
                        n(x0 + piece.x),
                        n(y0 + line.top),
                        n(piece.width),
                        n(line.height),
                        paint("fill", highlight, 1.)
                    );
                }
                let size = if style.baseline != 0 {
                    style.size * 0.65
                } else {
                    style.size
                };
                let decoration = match (style.underline, style.strike) {
                    (true, true) => r#" text-decoration="underline line-through""#,
                    (true, false) => r#" text-decoration="underline""#,
                    (false, true) => r#" text-decoration="line-through""#,
                    (false, false) => "",
                };
                // The width cosmic-text gave it, kept: the drawing matches
                // the layout even where shaping differs a little.
                let length = if !bullet && piece.text.trim().chars().count() > 1 && piece.width > 1.
                {
                    format!(r#" textLength="{}" lengthAdjust="spacing""#, n(piece.width))
                } else {
                    Default::default()
                };
                let family = fonts().family(&style.font);
                let _ = write!(
                    self.body,
                    r#"<text x="{}" y="{}" font-family={} font-size="{}" font-weight="{}" font-style="{}" {}{decoration}{length} xml:space="preserve">{}</text>"#,
                    n(x0 + piece.x),
                    n(baseline),
                    attr(&family),
                    n(size),
                    fonts().weight(&family, style.bold),
                    if style.italic { "italic" } else { "normal" },
                    paint("fill", style.color, 1.),
                    escape(&piece.text)
                );
            }
        }
        laid.height + top + bottom
    }

    /// An empty placeholder in the editor: a dashed outline and what to
    /// put in it.
    fn prompt(&mut self, place: Option<&Placeholder>, w: f32, h: f32) {
        let muted = Rgba {
            a: 110,
            ..self.theme.resolve(&self.theme.body.color)
        };
        let _ = write!(
            self.body,
            r#"<rect x="0.5" y="0.5" width="{}" height="{}" fill="none" {} stroke-width="1" stroke-dasharray="5 4"/>"#,
            n((w - 1.).max(0.)),
            n((h - 1.).max(0.)),
            paint("stroke", muted, 1.)
        );
        let Some(place) = place else {
            return;
        };
        let body = TextBody {
            paragraphs: vec![Paragraph {
                runs: vec![Run {
                    text: place.prompt.clone(),
                    color: Some(Color(format!("#{}{:02X}", muted.hex(), 150))),
                    ..Run::default()
                }],
                bullet: Some(Bullet::None),
                ..Paragraph::default()
            }],
            anchor: place.anchor,
            autofit: Autofit::Shrink,
            ..TextBody::default()
        };
        self.text(&body, Some(place), None, w, h, None);
    }

    fn picture(&mut self, picture: &Picture, w: f32, h: f32, place: Option<&Placeholder>) {
        let Some(path) = self.options.media_file(&picture.media) else {
            if self.options.editing {
                self.prompt(
                    place.or(Some(&Placeholder {
                        kind: PlaceholderKind::Picture,
                        prompt: "Picture".into(),
                        anchor: Anchor::Middle,
                        align: Some(Align::Center),
                        ..Placeholder::default()
                    })),
                    w,
                    h,
                );
            }
            return;
        };
        let (iw, ih) = image::image_dimensions(&path).unwrap_or((1, 1));
        let [cl, ct, cr, cb] = picture.crop.map(|side| side.clamp(0., 0.95));
        let (vx, vy) = (cl * iw as f32, ct * ih as f32);
        let vw = (iw as f32 * (1. - cl - cr)).max(1.);
        let vh = (ih as f32 * (1. - ct - cb)).max(1.);
        let filter = self.shadow(picture.shadow.as_ref());
        let clip = (picture.radius > 0.).then(|| {
            let id = self.id("c");
            let _ = write!(
                self.defs,
                r#"<clipPath id="{id}"><rect width="{}" height="{}" rx="{}"/></clipPath>"#,
                n(w),
                n(h),
                n(picture.radius)
            );
            format!(r#" clip-path="url(#{id})""#)
        });
        let _ = write!(
            self.body,
            r#"<g{filter}><g{}><svg width="{}" height="{}" viewBox="{} {} {} {}" preserveAspectRatio="{}"><image href={} width="{iw}" height="{ih}" preserveAspectRatio="none"/></svg></g></g>"#,
            clip.unwrap_or_default(),
            n(w),
            n(h),
            n(vx),
            n(vy),
            n(vw),
            n(vh),
            aspect(picture.fit),
            attr(&path.to_string_lossy()),
        );
        if picture.line.width > 0. {
            let stroke = self.stroke(&picture.line);
            let _ = write!(
                self.body,
                r#"<rect width="{}" height="{}" rx="{}" fill="none" {stroke}/>"#,
                n(w),
                n(h),
                n(picture.radius)
            );
        }
    }

    fn table(&mut self, table: &Table, w: f32, h: f32) {
        let rows = table.rows.len();
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if rows == 0 || columns == 0 {
            return;
        }
        let weights: Vec<f32> = if table.columns.len() == columns {
            table.columns.iter().map(|w| w.max(0.01)).collect()
        } else {
            vec![1.; columns]
        };
        let total: f32 = weights.iter().sum();
        let widths: Vec<f32> = weights.iter().map(|weight| weight / total * w).collect();
        let lefts: Vec<f32> = widths
            .iter()
            .scan(0., |at, width| {
                let left = *at;
                *at += width;
                Some(left)
            })
            .collect();
        let accent = table.accent.clone().unwrap_or(Color::from("accent1"));
        let accent_rgba = self.theme.resolve(&accent);
        let dark = self
            .theme
            .resolve(&self.theme.background_color())
            .luminance()
            < 0.4;
        let band = if dark {
            Rgba {
                a: 30,
                ..Rgba::WHITE
            }
        } else {
            accent_rgba.shade(0.9)
        };
        let border = self.theme.resolve(&table.border);
        // Each row as tall as its tallest cell, at least an even share.
        let floor = h / rows as f32;
        let pad = table.font_size * 0.45;
        let heights: Vec<f32> = table
            .rows
            .iter()
            .enumerate()
            .map(|(row, cells)| {
                cells
                    .iter()
                    .enumerate()
                    .map(|(column, cell)| {
                        let header = table.header && row == 0;
                        let paras = text::paragraphs(
                            &cell.text,
                            &Context {
                                theme: self.theme,
                                place: None,
                                color: None,
                                size: Some(table.font_size),
                                bold: Some(header),
                            },
                        );
                        let width = widths.get(column).copied().unwrap_or(0.) - pad * 2.;
                        text::lay_out(&paras, width.max(1.), true, None).height + pad * 2.
                    })
                    .fold(floor, f32::max)
            })
            .collect();
        let mut covered = vec![vec![false; columns]; rows];
        let mut y = 0.;
        for (row, cells) in table.rows.iter().enumerate() {
            for (column, cell) in cells.iter().enumerate() {
                if covered[row][column] {
                    continue;
                }
                let (span_c, span_r) = cell
                    .span
                    .map(|(c, r)| (c.max(1) as usize, r.max(1) as usize))
                    .unwrap_or((1, 1));
                let span_c = span_c.min(columns - column);
                let span_r = span_r.min(rows - row);
                for line in covered.iter_mut().skip(row).take(span_r) {
                    for spot in line.iter_mut().skip(column).take(span_c) {
                        *spot = true;
                    }
                }
                let x = lefts[column];
                let cell_w: f32 = widths[column..column + span_c].iter().sum();
                let cell_h: f32 = heights[row..row + span_r].iter().sum();
                let header = table.header && row == 0;
                let fill = match &cell.fill {
                    Some(fill) => Some(fill.clone()),
                    None if header => Some(Fill::Solid {
                        color: accent.clone(),
                        opacity: 1.,
                    }),
                    None if table.banded && row % 2 == (table.header as usize + 1) % 2 => {
                        Some(Fill::Solid {
                            color: Color(format!("#{}{:02X}", band.hex(), band.a)),
                            opacity: 1.,
                        })
                    }
                    None => None,
                };
                let _ = write!(self.body, r#"<g transform="translate({} {})">"#, n(x), n(y));
                if let Some(fill) = &fill {
                    let paint = self.fill(fill, cell_w, cell_h);
                    let _ = write!(
                        self.body,
                        r#"<rect width="{}" height="{}" {paint}/>"#,
                        n(cell_w),
                        n(cell_h)
                    );
                }
                let on = fill.as_ref().and_then(|fill| self.fill_color(fill));
                let mut body = cell.text.clone();
                body.insets = [pad, pad, pad, pad];
                body.anchor = Anchor::Middle;
                self.text(
                    &body,
                    None,
                    if header { on.or(Some(Rgba::WHITE)) } else { on },
                    cell_w,
                    cell_h,
                    Some((table.font_size, header)),
                );
                self.body.push_str("</g>");
            }
            y += heights[row];
        }
        // Lines between rows, and around.
        let mut at = 0.;
        let stroke = paint("stroke", border, 1.);
        for height in &heights {
            at += height;
            let _ = write!(
                self.body,
                r#"<line x1="0" y1="{}" x2="{}" y2="{}" {stroke} stroke-width="0.75"/>"#,
                n(at),
                n(w),
                n(at)
            );
        }
        let _ = write!(
            self.body,
            r#"<rect width="{}" height="{}" fill="none" {stroke} stroke-width="1"/>"#,
            n(w),
            n(at)
        );
    }

    fn code(&mut self, code: &Code, w: f32, h: f32) {
        let background = super::code::background(code.dark);
        let _ = write!(
            self.body,
            r#"<rect width="{}" height="{}" rx="{}" {}/>"#,
            n(w),
            n(h),
            n(code.font_size * 0.5),
            paint("fill", background, 1.)
        );
        let lines = super::code::highlight(&code.code, &code.language, code.dark);
        let numbers = if code.line_numbers {
            lines.len().to_string().len() + 1
        } else {
            0
        };
        let muted = if code.dark {
            Rgba::opaque(0x7A, 0x84, 0x96)
        } else {
            Rgba::opaque(0x8C, 0x95, 0x9F)
        };
        let paragraphs = lines
            .iter()
            .enumerate()
            .map(|(ix, tokens)| {
                let mut runs: Vec<Run> = Vec::new();
                if numbers > 0 {
                    runs.push(Run {
                        text: format!("{:>width$} ", ix + 1, width = numbers - 1),
                        color: Some(Color(format!("#{}", muted.hex()))),
                        ..Run::default()
                    });
                }
                runs.extend(tokens.iter().map(|(text, color, bold, italic)| Run {
                    text: text.clone(),
                    color: Some(Color(format!("#{}", color.hex()))),
                    bold: bold.then_some(true),
                    italic: italic.then_some(true),
                    ..Run::default()
                }));
                Paragraph {
                    runs,
                    bullet: Some(Bullet::None),
                    space_after: Some(0.),
                    line_spacing: Some(1.15),
                    ..Paragraph::default()
                }
            })
            .collect();
        let pad = code.font_size * 0.9;
        let body = TextBody {
            paragraphs,
            insets: [pad, pad, pad, pad],
            wrap: false,
            autofit: Autofit::Shrink,
            ..TextBody::default()
        };
        // The theme's code font for every run.
        let mut body = body;
        for paragraph in &mut body.paragraphs {
            for run in &mut paragraph.runs {
                run.font = Some("code".into());
            }
        }
        self.text(&body, None, None, w, h, Some((code.font_size, false)));
    }

    fn icon(&mut self, icon: &Icon, w: f32, h: f32) {
        let Some(source) = super::icons::svg(&icon.icon) else {
            return;
        };
        let color = self.theme.resolve(&icon.color);
        let svg = String::from_utf8_lossy(&source)
            .replace("currentColor", &format!("#{}", color.hex()))
            .replace(
                r#"stroke-width="2""#,
                &format!(r#"stroke-width="{}""#, n(icon.stroke.clamp(0.25, 6.))),
            );
        self.art(&svg, w, h);
    }

    /// An SVG document fitted into the box (as a picture, so its ids and
    /// styles stay its own).
    fn art(&mut self, source: &str, w: f32, h: f32) {
        if source.trim().is_empty() {
            return;
        }
        let data = base64::engine::general_purpose::STANDARD.encode(source.as_bytes());
        let _ = write!(
            self.body,
            r#"<image href="data:image/svg+xml;base64,{data}" width="{}" height="{}" preserveAspectRatio="xMidYMid meet"/>"#,
            n(w),
            n(h)
        );
    }
}

impl Theme {
    /// The background's main colour (a gradient's first).
    pub fn background_color(&self) -> Color {
        match &self.background {
            Fill::Solid { color, .. } => color.clone(),
            Fill::Gradient { stops, .. } if !stops.is_empty() => stops[0].color.clone(),
            _ => Color::from("light1"),
        }
    }
}

/// `fill="#…"` (or stroke), with its opacity when not opaque.
fn paint(what: &str, color: Rgba, opacity: f32) -> String {
    let alpha = color.a as f32 / 255. * opacity.clamp(0., 1.);
    if alpha >= 0.999 {
        format!(r##"{what}="#{}""##, color.hex())
    } else {
        format!(r##"{what}="#{}" {what}-opacity="{alpha:.3}""##, color.hex())
    }
}

fn aspect(fit: Fit) -> &'static str {
    match fit {
        Fit::Cover => "xMidYMid slice",
        Fit::Contain => "xMidYMid meet",
        Fit::Stretch => "none",
    }
}

/// A colour that reads on `background`: the theme's light or dark text.
pub fn contrasting(background: Rgba, theme: &Theme) -> Rgba {
    let light = theme.slot("light1").unwrap_or(Rgba::WHITE);
    let dark = theme.slot("dark1").unwrap_or(Rgba::BLACK);
    let contrast = |a: Rgba, b: Rgba| {
        let (l1, l2) = (a.luminance(), b.luminance());
        (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
    };
    if contrast(background, light) >= contrast(background, dark) {
        light
    } else {
        dark
    }
}

/// A notes page for slide `index`: the slide above, its notes below, on
/// a US Letter page.
pub fn notes_page(deck: &Deck, index: usize, options: &Options) -> String {
    let (page_w, page_h) = (612., 792.);
    let margin = 54.;
    let slide_w = page_w - margin * 2.;
    let slide_h = slide_w * deck.size.height / deck.size.width.max(1.);
    let slide_h = slide_h.min(page_h * 0.45);
    let picture = slide(deck, index, options);
    let data = base64::engine::general_purpose::STANDARD.encode(picture.as_bytes());
    let notes = deck
        .slides
        .get(index)
        .map(|slide| slide.notes.clone())
        .unwrap_or_default();
    let mut theme = deck.theme.clone();
    theme.colors.retain(|(slot, _)| slot != "dark1");
    theme.colors.push(("dark1".into(), "#222222".into()));
    let mut svg = Svg {
        body: String::new(),
        defs: String::new(),
        next: 0,
        theme: &theme,
        options,
    };
    let body = TextBody {
        paragraphs: notes
            .split('\n')
            .map(|line| Paragraph {
                runs: vec![Run {
                    text: line.to_string(),
                    size: Some(12.),
                    color: Some(Color::from("#222222")),
                    font: Some("body".into()),
                    ..Run::default()
                }],
                bullet: Some(Bullet::None),
                space_after: Some(4.),
                ..Paragraph::default()
            })
            .collect(),
        insets: [0., 0., 0., 0.],
        ..TextBody::default()
    };
    let text_y = margin + slide_h + 24.;
    let _ = write!(
        svg.body,
        r#"<g transform="translate({} {})">"#,
        n(margin),
        n(text_y)
    );
    svg.text(&body, None, None, slide_w, page_h - text_y - margin, None);
    svg.body.push_str("</g>");
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{pw}" height="{ph}" viewBox="0 0 {pw} {ph}"><defs>{}</defs><rect width="{pw}" height="{ph}" fill="#FFFFFF"/><image href="data:image/svg+xml;base64,{data}" x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="xMidYMid meet"/><rect x="{}" y="{}" width="{}" height="{}" fill="none" stroke="#BBBBBB" stroke-width="0.75"/>{}</svg>"##,
        svg.defs,
        n(margin),
        n(margin),
        n(slide_w),
        n(slide_h),
        n(margin),
        n(margin),
        n(slide_w),
        n(slide_h),
        svg.body,
        pw = n(page_w),
        ph = n(page_h),
    )
}
