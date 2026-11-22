//! A PowerPoint file turned into slides of a deck: each slide a file of its
//! own, with its text as text, its pictures as pictures, its tables as
//! tables and its charts as charts, each at the place it had.
//!
//! It is a start to go on from, not the file itself: what a web page has no
//! word for (animations, SmartArt, a video, the template the file was made
//! from) is left out, and said. The slides are written as an agent would
//! write them, so the agent (or the user) tidies them like any others.
//!
//! PowerPoint says many things by leaving them out: a title's place and
//! size come from its layout, the layout's from the master, a colour from
//! the theme. Each is looked up along that chain.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use roxmltree::{Document, Node};

use crate::deck::{DECK, MEDIA, STYLE};
use crate::live::Live;

/// What a file may hold, so that one made to exhaust the reader does not.
const MAX_ENTRIES: usize = 5_000;
const MAX_PART: u64 = 200 * 1024 * 1024;
const MAX_TOTAL: u64 = 1024 * 1024 * 1024;
const MAX_SLIDES: usize = 500;

const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

type Parts = BTreeMap<String, Vec<u8>>;

fn unzip(path: &Path) -> Result<Parts> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("{} cannot be opened", path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("it is not a PowerPoint file (.pptx)")?;
    if archive.len() > MAX_ENTRIES {
        bail!("the file holds too many parts to read");
    }
    let mut parts = Parts::new();
    let mut total = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        if entry.size() > MAX_PART {
            bail!("a part of the file is too large to read");
        }
        let Ok(name) = entry
            .name()
            .map(|name| name.trim_start_matches('/').to_string())
        else {
            continue;
        };
        let mut bytes = Vec::new();
        // Counted as it is read: what a part says of its size is not trusted.
        (&mut entry).take(MAX_PART + 1).read_to_end(&mut bytes)?;
        total += bytes.len() as u64;
        if bytes.len() as u64 > MAX_PART || total > MAX_TOTAL {
            bail!("the file is too large to read");
        }
        parts.insert(name, bytes);
    }
    Ok(parts)
}

fn xml<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    std::str::from_utf8(parts.get(name)?).ok()
}

/// The folder a part is in, for what it names relative to itself.
fn folder(part: &str) -> &str {
    part.rsplit_once('/')
        .map(|(folder, _)| folder)
        .unwrap_or("")
}

/// `target`, as a part names it, seen from `from`.
fn resolve(from: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut path: Vec<&str> = folder(from)
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            part => path.push(part),
        }
    }
    path.join("/")
}

/// The relationships of `part`: id to (kind, the part named).
fn rels(parts: &Parts, part: &str) -> BTreeMap<String, (String, String)> {
    let (dir, name) = part.rsplit_once('/').unwrap_or(("", part));
    let path = if dir.is_empty() {
        format!("_rels/{name}.rels")
    } else {
        format!("{dir}/_rels/{name}.rels")
    };
    let mut found = BTreeMap::new();
    let Some(text) = xml(parts, &path) else {
        return found;
    };
    let Ok(document) = Document::parse(text) else {
        return found;
    };
    for rel in document.root_element().children().filter(Node::is_element) {
        let (Some(id), Some(target)) = (rel.attribute("Id"), rel.attribute("Target")) else {
            continue;
        };
        let kind = rel
            .attribute("Type")
            .unwrap_or_default()
            .rsplit('/')
            .next()
            .unwrap_or_default();
        let named = if rel.attribute("TargetMode") == Some("External") {
            target.to_string()
        } else {
            resolve(part, target)
        };
        found.insert(id.to_string(), (kind.to_string(), named));
    }
    found
}

fn child<'a>(node: Node<'a, 'a>, ns: &str, name: &str) -> Option<Node<'a, 'a>> {
    node.children().find(|child| child.has_tag_name((ns, name)))
}

fn children<'a>(
    node: Node<'a, 'a>,
    ns: &'static str,
    name: &'static str,
) -> impl Iterator<Item = Node<'a, 'a>> {
    node.children()
        .filter(move |child| child.has_tag_name((ns, name)))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn attribute(text: &str) -> String {
    escape(text).replace('"', "&quot;")
}

/// A colour as PowerPoint writes one, made a CSS colour.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

impl Rgba {
    fn hex(hex: &str) -> Option<Self> {
        if hex.len() != 6 {
            return None;
        }
        let part = |at: usize| {
            u8::from_str_radix(hex.get(at..at + 2)?, 16)
                .ok()
                .map(f64::from)
        };
        Some(Self {
            r: part(0)?,
            g: part(2)?,
            b: part(4)?,
            a: 1.,
        })
    }

    fn css(self) -> String {
        let part = |value: f64| value.round().clamp(0., 255.) as u8;
        if self.a < 0.995 {
            format!(
                "rgb({} {} {} / {:.2})",
                part(self.r),
                part(self.g),
                part(self.b),
                self.a.clamp(0., 1.)
            )
        } else {
            format!(
                "#{:02x}{:02x}{:02x}",
                part(self.r),
                part(self.g),
                part(self.b)
            )
        }
    }

    fn mix(self, with: f64, amount: f64) -> Self {
        let blend = |value: f64| value * amount + with * (1. - amount);
        Self {
            r: blend(self.r),
            g: blend(self.g),
            b: blend(self.b),
            a: self.a,
        }
    }

    /// Lighter or darker: its lightness times `by`, plus `plus` (of 1).
    fn luminance(self, by: f64, plus: f64) -> Self {
        let (r, g, b) = (self.r / 255., self.g / 255., self.b / 255.);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let light = (max + min) / 2.;
        let spread = max - min;
        let saturation = if spread == 0. {
            0.
        } else {
            spread / (1. - (2. * light - 1.).abs())
        };
        let hue = if spread == 0. {
            0.
        } else if max == r {
            ((g - b) / spread).rem_euclid(6.)
        } else if max == g {
            (b - r) / spread + 2.
        } else {
            (r - g) / spread + 4.
        };
        let light = (light * by + plus).clamp(0., 1.);
        let chroma = (1. - (2. * light - 1.).abs()) * saturation;
        let x = chroma * (1. - (hue.rem_euclid(2.) - 1.).abs());
        let (r, g, b) = match hue as u8 {
            0 => (chroma, x, 0.),
            1 => (x, chroma, 0.),
            2 => (0., chroma, x),
            3 => (0., x, chroma),
            4 => (x, 0., chroma),
            _ => (chroma, 0., x),
        };
        let lift = light - chroma / 2.;
        Self {
            r: (r + lift) * 255.,
            g: (g + lift) * 255.,
            b: (b + lift) * 255.,
            a: self.a,
        }
    }
}

/// The file's theme: its colours by name, and its two fonts.
#[derive(Default)]
struct Theme {
    colours: BTreeMap<String, Rgba>,
    heading: String,
    body: String,
}

impl Theme {
    fn read(text: &str) -> Self {
        let mut theme = Theme::default();
        let Ok(document) = Document::parse(text) else {
            return theme;
        };
        let Some(elements) = child(document.root_element(), A, "themeElements") else {
            return theme;
        };
        for slot in child(elements, A, "clrScheme")
            .into_iter()
            .flat_map(|scheme| scheme.children())
        {
            let colour = slot.children().find(Node::is_element).and_then(|colour| {
                Rgba::hex(
                    colour
                        .attribute("val")
                        .or_else(|| colour.attribute("lastClr"))?,
                )
            });
            if let Some(colour) = colour {
                theme
                    .colours
                    .insert(slot.tag_name().name().to_string(), colour);
            }
        }
        let face = |which: &str| {
            child(elements, A, "fontScheme")
                .and_then(|fonts| child(fonts, A, which))
                .and_then(|font| child(font, A, "latin"))
                .and_then(|latin| latin.attribute("typeface"))
                .unwrap_or_default()
                .to_string()
        };
        theme.heading = face("majorFont");
        theme.body = face("minorFont");
        theme
    }

    /// The colour a fill, a line or a run names (the first colour element
    /// in `node`), with what is said to change it.
    fn colour(&self, node: Node<'_, '_>) -> Option<Rgba> {
        let named = node.children().find(|child| {
            matches!(
                child.tag_name().name(),
                "srgbClr" | "schemeClr" | "sysClr" | "prstClr" | "scrgbClr"
            )
        })?;
        let mut colour = match named.tag_name().name() {
            "srgbClr" => Rgba::hex(named.attribute("val")?)?,
            "sysClr" => Rgba::hex(named.attribute("lastClr")?)?,
            "schemeClr" => {
                // What a slide calls text and background are the theme's
                // dark and light.
                let slot = match named.attribute("val")? {
                    "tx1" => "dk1",
                    "tx2" => "dk2",
                    "bg1" => "lt1",
                    "bg2" => "lt2",
                    other => other,
                };
                *self.colours.get(slot)?
            }
            "prstClr" => match named.attribute("val")? {
                "white" => Rgba::hex("ffffff")?,
                "black" => Rgba::hex("000000")?,
                "red" => Rgba::hex("ff0000")?,
                "green" => Rgba::hex("008000")?,
                "blue" => Rgba::hex("0000ff")?,
                "yellow" => Rgba::hex("ffff00")?,
                "gray" | "grey" => Rgba::hex("808080")?,
                _ => return None,
            },
            _ => return None,
        };
        for change in named.children().filter(Node::is_element) {
            let amount = change
                .attribute("val")
                .and_then(|val| val.parse::<f64>().ok())
                .unwrap_or(100_000.)
                / 100_000.;
            match change.tag_name().name() {
                "alpha" => colour.a = amount,
                "lumMod" => colour = colour.luminance(amount, 0.),
                "lumOff" => colour = colour.luminance(1., amount),
                "tint" => colour = colour.mix(255., amount),
                "shade" => colour = colour.mix(0., amount),
                _ => {}
            }
        }
        Some(colour)
    }

    /// A font by the name a run gives it: the theme's two have names of
    /// their own there.
    fn font(&self, face: &str) -> String {
        match face {
            name if name.starts_with("+mj") => self.heading.clone(),
            name if name.starts_with("+mn") => self.body.clone(),
            name => name.to_string(),
        }
    }
}

/// How text is set, each thing only where it is said.
#[derive(Debug, Default, Clone)]
struct TextStyle {
    size: Option<f64>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    colour: Option<Rgba>,
    font: Option<String>,
    align: Option<String>,
    /// `Some(None)`: said to have no bullet.
    bullet: Option<Option<bool>>,
    line: Option<f64>,
    after: Option<f64>,
}

impl TextStyle {
    /// What `self` says, and what it leaves unsaid from `under`.
    fn over(mut self, under: &TextStyle) -> Self {
        self.size = self.size.or(under.size);
        self.bold = self.bold.or(under.bold);
        self.italic = self.italic.or(under.italic);
        self.underline = self.underline.or(under.underline);
        self.colour = self.colour.or(under.colour);
        self.font = self.font.or_else(|| under.font.clone());
        self.align = self.align.or_else(|| under.align.clone());
        self.bullet = self.bullet.or(under.bullet);
        self.line = self.line.or(under.line);
        self.after = self.after.or(under.after);
        self
    }

    /// What a run's (or a level's default run's) properties say.
    fn of_run(node: Node<'_, '_>, theme: &Theme) -> Self {
        let flag = |name: &str| {
            node.attribute(name)
                .map(|value| value == "1" || value == "true")
        };
        TextStyle {
            size: node
                .attribute("sz")
                .and_then(|size| size.parse::<f64>().ok())
                .map(|size| size / 100.),
            bold: flag("b"),
            italic: flag("i"),
            underline: node.attribute("u").map(|kind| kind != "none"),
            colour: child(node, A, "solidFill").and_then(|fill| theme.colour(fill)),
            font: child(node, A, "latin")
                .and_then(|latin| latin.attribute("typeface"))
                .map(|face| theme.font(face))
                .filter(|face| !face.is_empty()),
            ..TextStyle::default()
        }
    }

    /// What a paragraph's (or a level's) properties say, with its default
    /// run's.
    fn of_paragraph(node: Node<'_, '_>, theme: &Theme) -> Self {
        let spacing = |name: &str| {
            let space = child(node, A, name)?;
            let points = child(space, A, "spcPts")?
                .attribute("val")?
                .parse::<f64>()
                .ok()?;
            Some(points / 100.)
        };
        let bullet = if child(node, A, "buNone").is_some() {
            Some(None)
        } else if child(node, A, "buAutoNum").is_some() {
            Some(Some(true))
        } else if child(node, A, "buChar").is_some() || child(node, A, "buBlip").is_some() {
            Some(Some(false))
        } else {
            None
        };
        let style = TextStyle {
            align: node.attribute("algn").map(|align| {
                match align {
                    "ctr" => "center",
                    "r" => "right",
                    "just" | "dist" => "justify",
                    _ => "left",
                }
                .to_string()
            }),
            bullet,
            line: child(node, A, "lnSpc")
                .and_then(|line| child(line, A, "spcPct"))
                .and_then(|percent| percent.attribute("val")?.parse::<f64>().ok())
                .map(|percent| percent / 100_000.),
            after: spacing("spcAft"),
            ..TextStyle::default()
        };
        match child(node, A, "defRPr") {
            Some(run) => style.over(&TextStyle::of_run(run, theme)),
            None => style,
        }
    }
}

/// The levels of a list of text styles (`lstStyle`, a master's
/// `titleStyle`): what each of the nine levels says.
fn levels(node: Option<Node<'_, '_>>, theme: &Theme) -> Vec<TextStyle> {
    let mut styles = vec![TextStyle::default(); 9];
    for level in node
        .into_iter()
        .flat_map(|node| node.children())
        .filter(Node::is_element)
    {
        let name = level.tag_name().name();
        let at = match name
            .strip_prefix("lvl")
            .and_then(|rest| rest.strip_suffix("pPr"))
        {
            Some(number) => number.parse::<usize>().unwrap_or(1).clamp(1, 9) - 1,
            None if name == "defPPr" => 0,
            None => continue,
        };
        styles[at] = TextStyle::of_paragraph(level, theme);
    }
    styles
}

/// A layout or a master: its shapes with a placeholder's role, its text
/// styles, its background.
struct Sheet<'a> {
    document: Document<'a>,
}

impl<'a> Sheet<'a> {
    fn read(text: &'a str) -> Option<Self> {
        Document::parse(text).ok().map(|document| Self { document })
    }

    fn tree(&self) -> Option<Node<'_, '_>> {
        child(child(self.document.root_element(), P, "cSld")?, P, "spTree")
    }

    /// The shape that holds the place a slide's shape stands in for.
    fn placeholder(&self, kind: &str, index: Option<&str>) -> Option<Node<'_, '_>> {
        let shapes: Vec<(Node<'_, '_>, Node<'_, '_>)> = self
            .tree()?
            .children()
            .filter(|shape| shape.has_tag_name((P, "sp")) || shape.has_tag_name((P, "pic")))
            .filter_map(|shape| Some((shape, placeholder_of(shape)?)))
            .collect();
        let same_kind = |a: &str, b: &str| {
            let group = |kind: &str| match kind {
                "ctrTitle" | "title" => "title",
                "subTitle" | "body" | "" => "body",
                _ => "",
            };
            a == b || (!group(a).is_empty() && group(a) == group(b))
        };
        shapes
            .iter()
            .find(|(_, ph)| index.is_some() && ph.attribute("idx") == index)
            .or_else(|| {
                shapes
                    .iter()
                    .find(|(_, ph)| same_kind(ph.attribute("type").unwrap_or(""), kind))
            })
            .map(|(shape, _)| *shape)
    }
}

fn placeholder_of<'a>(shape: Node<'a, 'a>) -> Option<Node<'a, 'a>> {
    shape
        .children()
        .find(|part| part.tag_name().name().starts_with("nv"))
        .and_then(|non_visual| child(non_visual, P, "nvPr"))
        .and_then(|properties| child(properties, P, "ph"))
}

/// A place on the slide, in the deck's pixels.
#[derive(Debug, Clone, Copy, Default)]
struct Frame {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    turned: f64,
    flip_h: bool,
    flip_v: bool,
}

/// How a group's own coordinates map onto the slide's (EMU).
#[derive(Debug, Clone, Copy)]
struct Space {
    /// The slide's EMU for one of the group's, and the offset, each way.
    sx: f64,
    sy: f64,
    dx: f64,
    dy: f64,
}

impl Space {
    const SLIDE: Space = Space {
        sx: 1.,
        sy: 1.,
        dx: 0.,
        dy: 0.,
    };
}

fn emu(node: Node<'_, '_>, name: &str) -> Option<f64> {
    node.attribute(name)?.parse::<f64>().ok()
}

struct Reader<'a> {
    parts: &'a Parts,
    theme: Theme,
    /// Pixels of the deck in one EMU.
    scale: f64,
    /// Pixels of the deck in one point of type.
    point: f64,
    site: &'a Path,
    /// The file's pictures put with the deck: the part, and where.
    media: BTreeMap<String, Option<String>>,
    left_out: BTreeMap<&'static str, usize>,
    pictures: usize,
    tables: usize,
    charts: usize,
    ids: usize,
}

impl Reader<'_> {
    fn skip(&mut self, what: &'static str) {
        *self.left_out.entry(what).or_default() += 1;
    }

    fn id(&mut self) -> String {
        self.ids += 1;
        format!("e{}", self.ids)
    }

    /// Where a shape is: said by itself, or by the placeholder it fills.
    fn frame(
        &self,
        properties: Option<Node<'_, '_>>,
        space: Space,
        fallback: &[Option<Node<'_, '_>>],
    ) -> Option<Frame> {
        let own = properties.and_then(|properties| {
            child(properties, A, "xfrm").or_else(|| child(properties, P, "xfrm"))
        });
        let transform = own.or_else(|| {
            fallback.iter().flatten().find_map(|shape| {
                let properties = child(*shape, P, "spPr")?;
                child(properties, A, "xfrm")
            })
        })?;
        let (off, ext) = (child(transform, A, "off")?, child(transform, A, "ext")?);
        // Only a shape's own place is in its group's coordinates.
        let space = if own.is_some() { space } else { Space::SLIDE };
        let px = |value: f64| value * self.scale;
        Some(Frame {
            x: px(emu(off, "x")? * space.sx + space.dx),
            y: px(emu(off, "y")? * space.sy + space.dy),
            w: px(emu(ext, "cx")? * space.sx),
            h: px(emu(ext, "cy")? * space.sy),
            turned: transform
                .attribute("rot")
                .and_then(|rot| rot.parse::<f64>().ok())
                .unwrap_or(0.)
                / 60_000.,
            flip_h: transform.attribute("flipH") == Some("1"),
            flip_v: transform.attribute("flipV") == Some("1"),
        })
    }

    fn place(frame: &Frame) -> String {
        let mut style = format!(
            "position:absolute;left:{:.0}px;top:{:.0}px;width:{:.0}px;height:{:.0}px",
            frame.x, frame.y, frame.w, frame.h
        );
        if frame.turned.abs() > 0.01 {
            let _ = write!(style, ";rotate:{:.1}deg", frame.turned);
        }
        style
    }

    /// A picture of the file, put with the deck's media; its path there.
    fn picture(&mut self, part: &str) -> Option<String> {
        if let Some(known) = self.media.get(part) {
            return known.clone();
        }
        let extension = part
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let shown = matches!(
            extension.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp"
        );
        let path = self.parts.get(part).filter(|_| shown).and_then(|bytes| {
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            let path = format!("{MEDIA}/picture-{:08x}.{extension}", hash as u32);
            let file = self.site.join(&path);
            std::fs::create_dir_all(self.site.join(MEDIA)).ok()?;
            if !file.exists() {
                ph_artifacts::library::write_atomic(&file, bytes).ok()?;
            }
            Some(path)
        });
        if path.is_none() {
            self.skip("pictures in a format a web page does not show (EMF, WMF, TIFF)");
        }
        self.media.insert(part.to_string(), path.clone());
        path
    }

    /// A shape's fill and outline as CSS.
    fn paint(&self, properties: Option<Node<'_, '_>>) -> (Option<Rgba>, Option<(f64, Rgba)>) {
        let Some(properties) = properties else {
            return (None, None);
        };
        let fill = child(properties, A, "solidFill")
            .and_then(|fill| self.theme.colour(fill))
            .or_else(|| {
                // A gradient comes as its first colour.
                let stops = child(child(properties, A, "gradFill")?, A, "gsLst")?;
                self.theme.colour(child(stops, A, "gs")?)
            });
        let line = child(properties, A, "ln").and_then(|line| {
            let colour = self.theme.colour(child(line, A, "solidFill")?)?;
            let width = line
                .attribute("w")
                .and_then(|w| w.parse::<f64>().ok())
                .unwrap_or(12_700.);
            Some(((width * self.scale).max(1.), colour))
        });
        (fill, line)
    }

    /// A text body as HTML: its paragraphs, lists where they are bulleted.
    /// `under` are the styles it stands on, nearest first (each nine levels).
    fn text(
        &mut self,
        body: Node<'_, '_>,
        under: &[Vec<TextStyle>],
        rels: &BTreeMap<String, (String, String)>,
    ) -> String {
        let own = levels(child(body, A, "lstStyle"), &self.theme);
        let shrink = child(body, A, "bodyPr")
            .and_then(|properties| child(properties, A, "normAutofit"))
            .and_then(|fit| fit.attribute("fontScale")?.parse::<f64>().ok())
            .map(|scale| scale / 100_000.)
            .unwrap_or(1.);
        let mut html = String::new();
        // The list being written: whether numbered.
        let mut list: Option<bool> = None;
        for paragraph in children(body, A, "p") {
            let properties = child(paragraph, A, "pPr");
            let level = properties
                .and_then(|properties| properties.attribute("lvl")?.parse::<usize>().ok())
                .unwrap_or(0)
                .min(8);
            let mut base = properties
                .map(|properties| TextStyle::of_paragraph(properties, &self.theme))
                .unwrap_or_default()
                .over(&own[level]);
            for styles in under {
                base = base.over(&styles[level]);
            }
            // What every run of the paragraph says of itself is the
            // paragraph's own: said once, on the paragraph.
            let said_by_runs: Vec<TextStyle> = paragraph
                .children()
                .filter(|part| matches!(part.tag_name().name(), "r" | "fld"))
                .filter(|part| {
                    children(*part, A, "t")
                        .any(|text| text.text().is_some_and(|said| !said.is_empty()))
                })
                .map(|part| {
                    child(part, A, "rPr")
                        .map(|run| TextStyle::of_run(run, &self.theme))
                        .unwrap_or_default()
                })
                .collect();
            if let Some(first) = said_by_runs.first() {
                let mut shared = first.clone();
                for other in &said_by_runs[1..] {
                    if other.size != shared.size {
                        shared.size = None;
                    }
                    if other.bold != shared.bold {
                        shared.bold = None;
                    }
                    if other.italic != shared.italic {
                        shared.italic = None;
                    }
                    if other.colour != shared.colour {
                        shared.colour = None;
                    }
                    if other.font != shared.font {
                        shared.font = None;
                    }
                }
                shared.underline = None;
                base = shared.over(&base);
            }
            let mut runs = String::new();
            let mut words = false;
            for part in paragraph.children().filter(Node::is_element) {
                match part.tag_name().name() {
                    "br" => runs.push_str("<br>"),
                    "r" | "fld" => {
                        let said: String = children(part, A, "t")
                            .filter_map(|text| text.text())
                            .collect();
                        if said.is_empty() {
                            continue;
                        }
                        words |= !said.trim().is_empty();
                        let run = child(part, A, "rPr");
                        let style = run
                            .map(|run| TextStyle::of_run(run, &self.theme))
                            .unwrap_or_default();
                        let mut css = String::new();
                        if let Some(size) = style.size.filter(|size| Some(*size) != base.size) {
                            let _ = write!(css, "font-size:{:.0}px;", size * self.point * shrink);
                        }
                        if let Some(bold) = style.bold.filter(|bold| Some(*bold) != base.bold) {
                            let _ = write!(css, "font-weight:{};", if bold { 700 } else { 400 });
                        }
                        if style.italic == Some(true) && base.italic != Some(true) {
                            css.push_str("font-style:italic;");
                        }
                        if style.underline == Some(true) && base.underline != Some(true) {
                            css.push_str("text-decoration:underline;");
                        }
                        if let Some(colour) =
                            style.colour.filter(|colour| Some(*colour) != base.colour)
                        {
                            let _ = write!(css, "color:{};", colour.css());
                        }
                        if let Some(font) = style
                            .font
                            .as_ref()
                            .filter(|font| Some(*font) != base.font.as_ref())
                        {
                            let _ = write!(css, "font-family:{};", font_stack(font));
                        }
                        let link = run
                            .and_then(|run| child(run, A, "hlinkClick"))
                            .and_then(|link| link.attribute((R, "id")))
                            .and_then(|id| rels.get(id))
                            .map(|(_, target)| target.as_str())
                            .filter(|target| {
                                target.starts_with("http://") || target.starts_with("https://")
                            });
                        let said = escape(&said);
                        let said = match link {
                            Some(link) => format!("<a href=\"{}\">{said}</a>", attribute(link)),
                            None => said,
                        };
                        if css.is_empty() {
                            runs.push_str(&said);
                        } else {
                            let _ = write!(
                                runs,
                                "<span style=\"{}\">{said}</span>",
                                css.trim_end_matches(';')
                            );
                        }
                    }
                    _ => {}
                }
            }
            if !words {
                // An empty line keeps its room between two paragraphs.
                if !html.is_empty() {
                    if let Some(numbered) = list.take() {
                        html.push_str(if numbered { "</ol>" } else { "</ul>" });
                    }
                    html.push_str("<p style=\"margin:0\">&nbsp;</p>");
                }
                continue;
            }
            let mut css = String::from("margin:0;");
            if let Some(size) = base.size {
                let _ = write!(css, "font-size:{:.0}px;", size * self.point * shrink);
            }
            if base.bold == Some(true) {
                css.push_str("font-weight:700;");
            } else if base.bold == Some(false) {
                css.push_str("font-weight:400;");
            }
            if base.italic == Some(true) {
                css.push_str("font-style:italic;");
            }
            if let Some(colour) = base.colour {
                let _ = write!(css, "color:{};", colour.css());
            }
            if let Some(font) = &base.font {
                let _ = write!(css, "font-family:{};", font_stack(font));
            }
            if let Some(align) = base.align.as_deref().filter(|align| *align != "left") {
                let _ = write!(css, "text-align:{align};");
            }
            // PowerPoint's line is of the font's own, about a fifth more
            // than the type's size.
            let _ = write!(css, "line-height:{:.2};", base.line.unwrap_or(1.) * 1.2);
            if let Some(after) = base.after.filter(|after| *after > 0.) {
                let _ = write!(css, "margin-bottom:{:.0}px;", after * self.point);
            }
            let css = css.trim_end_matches(';');
            let bulleted = base.bullet.flatten();
            if bulleted != list {
                if let Some(numbered) = list.take() {
                    html.push_str(if numbered { "</ol>" } else { "</ul>" });
                }
                if let Some(numbered) = bulleted {
                    html.push_str(if numbered {
                        "<ol style=\"margin:0;padding-left:1.2em\">"
                    } else {
                        "<ul style=\"margin:0;padding-left:1.2em\">"
                    });
                    list = Some(numbered);
                }
            }
            let indent = if level > 0 && bulleted.is_some() {
                format!(";margin-left:{:.1}em", level as f64 * 1.2)
            } else {
                String::new()
            };
            match bulleted {
                Some(_) => {
                    let _ = write!(html, "<li style=\"{css}{indent}\">{runs}</li>");
                }
                None => {
                    let _ = write!(html, "<p style=\"{css}\">{runs}</p>");
                }
            }
        }
        if let Some(numbered) = list {
            html.push_str(if numbered { "</ol>" } else { "</ul>" });
        }
        html
    }
}

/// A font with the ones to fall back on where it is not there.
fn font_stack(face: &str) -> String {
    let lower = face.to_lowercase();
    let generic = if [
        "georgia",
        "times",
        "garamond",
        "cambria",
        "palatino",
        "book",
        "serif",
        "baskerville",
    ]
    .iter()
    .any(|name| lower.contains(name) && !lower.contains("sans"))
    {
        "Georgia, serif"
    } else if ["courier", "consolas", "mono", "menlo"]
        .iter()
        .any(|name| lower.contains(name))
    {
        "'Courier New', monospace"
    } else {
        "Arial, sans-serif"
    };
    format!(
        "'{}', {generic}",
        face.replace(['\'', '"', ';', '<', '>'], "")
    )
}

/// Everything a slide stands on: its layout and its master, and the styles
/// the file gives text that says nothing of itself.
struct Under<'a> {
    layout: Option<Sheet<'a>>,
    master: Option<Sheet<'a>>,
    title: Vec<TextStyle>,
    body: Vec<TextStyle>,
    other: Vec<TextStyle>,
    default: Vec<TextStyle>,
}

impl Reader<'_> {
    /// One shape of a slide (or a group of them) as HTML.
    fn shape(
        &mut self,
        node: Node<'_, '_>,
        space: Space,
        under: &Under<'_>,
        rels: &BTreeMap<String, (String, String)>,
        html: &mut String,
    ) {
        match node.tag_name().name() {
            "grpSp" => {
                // A group has coordinates of its own: its children's are
                // mapped through them onto the slide.
                let inner = child(node, P, "grpSpPr")
                    .and_then(|properties| child(properties, A, "xfrm"))
                    .and_then(|transform| {
                        let (off, ext) = (child(transform, A, "off")?, child(transform, A, "ext")?);
                        let (ch_off, ch_ext) =
                            (child(transform, A, "chOff")?, child(transform, A, "chExt")?);
                        let sx = emu(ext, "cx")? / emu(ch_ext, "cx")?.max(1.);
                        let sy = emu(ext, "cy")? / emu(ch_ext, "cy")?.max(1.);
                        Some(Space {
                            sx: space.sx * sx,
                            sy: space.sy * sy,
                            dx: space.dx + space.sx * (emu(off, "x")? - emu(ch_off, "x")? * sx),
                            dy: space.dy + space.sy * (emu(off, "y")? - emu(ch_off, "y")? * sy),
                        })
                    })
                    .unwrap_or(space);
                for inside in node.children().filter(Node::is_element) {
                    self.shape(inside, inner, under, rels, html);
                }
            }
            "sp" | "cxnSp" => self.plain_shape(node, space, under, rels, html),
            "pic" => {
                let properties = child(node, P, "spPr");
                let Some(frame) = self.frame(properties, space, &[]) else {
                    return;
                };
                let part = child(node, P, "blipFill")
                    .and_then(|fill| child(fill, A, "blip"))
                    .and_then(|blip| blip.attribute((R, "embed")))
                    .and_then(|id| rels.get(id))
                    .map(|(_, part)| part.clone());
                let Some(path) = part.and_then(|part| self.picture(&part)) else {
                    return;
                };
                let alt = child(node, P, "nvPicPr")
                    .and_then(|non_visual| child(non_visual, P, "cNvPr"))
                    .and_then(|properties| properties.attribute("descr"))
                    .unwrap_or_default();
                self.pictures += 1;
                let _ = writeln!(
                    html,
                    "  <img data-id=\"{}\" src=\"{path}\" alt=\"{}\" style=\"{};object-fit:cover;border-radius:0\">",
                    self.id(),
                    attribute(alt),
                    Self::place(&frame),
                );
            }
            "graphicFrame" => self.graphic(node, space, rels, html),
            "contentPart" => self.skip("ink"),
            _ => {}
        }
    }

    fn plain_shape(
        &mut self,
        node: Node<'_, '_>,
        space: Space,
        under: &Under<'_>,
        rels: &BTreeMap<String, (String, String)>,
        html: &mut String,
    ) {
        let properties = child(node, P, "spPr");
        let placeholder = placeholder_of(node);
        let kind = placeholder
            .and_then(|ph| ph.attribute("type"))
            .unwrap_or("");
        let index = placeholder.and_then(|ph| ph.attribute("idx"));
        // The date, the footer and the slide's number are the template's.
        if matches!(kind, "dt" | "ftr" | "sldNum" | "hdr") {
            return;
        }
        let on_layout = placeholder.and_then(|_| under.layout.as_ref()?.placeholder(kind, index));
        let on_master = placeholder.and_then(|_| under.master.as_ref()?.placeholder(kind, index));
        let Some(frame) = self.frame(properties, space, &[on_layout, on_master]) else {
            return;
        };
        let body = child(node, P, "txBody");
        let has_words = body.is_some_and(|body| {
            body.descendants().any(|part| {
                part.has_tag_name((A, "t"))
                    && part.text().is_some_and(|text| !text.trim().is_empty())
            })
        });
        let (fill, line) = self.paint(properties);
        let preset = properties
            .and_then(|properties| child(properties, A, "prstGeom"))
            .and_then(|geometry| geometry.attribute("prst"))
            .unwrap_or("rect");
        let connector =
            node.has_tag_name((P, "cxnSp")) || matches!(preset, "line" | "straightConnector1");
        if connector {
            let Some((width, colour)) = line else {
                return;
            };
            // From one corner of its box to the other: a line as long as
            // the box's diagonal, turned to it.
            let length = frame.w.hypot(frame.h);
            let rise = if frame.flip_v { -frame.h } else { frame.h };
            let run = if frame.flip_h { -frame.w } else { frame.w };
            let angle = rise.atan2(run).to_degrees() + frame.turned;
            let (x, y) = (
                if frame.flip_h {
                    frame.x + frame.w
                } else {
                    frame.x
                },
                if frame.flip_v {
                    frame.y + frame.h
                } else {
                    frame.y
                },
            );
            let arrow = properties
                .and_then(|properties| child(properties, A, "ln"))
                .is_some_and(|line| {
                    child(line, A, "tailEnd").is_some() || child(line, A, "headEnd").is_some()
                });
            let _ = writeln!(
                html,
                "  <div data-id=\"{}\" data-shape=\"{}\" style=\"position:absolute;left:{x:.0}px;top:{:.0}px;width:{length:.0}px;height:{width:.0}px;background:{};rotate:{angle:.1}deg\"></div>",
                self.id(),
                if arrow { "connector" } else { "line" },
                y - width / 2.,
                colour.css(),
            );
            return;
        }
        if !has_words && fill.is_none() && line.is_none() {
            return;
        }
        let shape = match preset {
            "ellipse" => Some("ellipse"),
            "roundRect" => Some("rounded"),
            "diamond" => Some("diamond"),
            "rightArrow" => Some("arrow-right"),
            "leftArrow" => Some("arrow-left"),
            "upArrow" => Some("arrow-up"),
            "downArrow" => Some("arrow-down"),
            _ => None,
        };
        let mut css = Self::place(&frame);
        if shape == Some("rounded") {
            // How round, as the file says: a part of its shorter side.
            let adjust = properties
                .and_then(|properties| child(properties, A, "prstGeom"))
                .and_then(|geometry| child(geometry, A, "avLst"))
                .and_then(|list| child(list, A, "gd"))
                .and_then(|guide| {
                    guide
                        .attribute("fmla")?
                        .strip_prefix("val ")?
                        .parse::<f64>()
                        .ok()
                })
                .unwrap_or(16_667.);
            let _ = write!(
                css,
                ";border-radius:{:.0}px",
                adjust / 100_000. * frame.w.min(frame.h)
            );
        }
        match fill {
            Some(fill) => {
                let _ = write!(css, ";background:{}", fill.css());
            }
            None if shape.is_some() || line.is_some() => css.push_str(";background:transparent"),
            None => {}
        }
        if let Some((width, colour)) = line {
            let _ = write!(css, ";border:{width:.0}px solid {}", colour.css());
        }
        // Where its words sit in it, and how far from its edges.
        let body_properties = body.and_then(|body| child(body, A, "bodyPr"));
        let anchor = body_properties
            .and_then(|properties| properties.attribute("anchor"))
            .or_else(|| {
                [on_layout, on_master]
                    .into_iter()
                    .flatten()
                    .find_map(|shape| {
                        child(child(shape, P, "txBody")?, A, "bodyPr")?.attribute("anchor")
                    })
            })
            .unwrap_or("t");
        let justify = match anchor {
            "ctr" => "center",
            "b" => "flex-end",
            _ => "flex-start",
        };
        let scale = self.scale;
        let inset = |name: &str, usual: f64| {
            body_properties
                .and_then(|properties| properties.attribute(name)?.parse::<f64>().ok())
                .unwrap_or(usual)
                * scale
        };
        let words = match body.filter(|_| has_words) {
            Some(body) => {
                // What it stands on, nearest first: the placeholder it
                // fills on its layout and master, then the master's style
                // for its kind, then the file's own.
                let mut styles: Vec<Vec<TextStyle>> = Vec::new();
                for shape in [on_layout, on_master].into_iter().flatten() {
                    let list =
                        child(shape, P, "txBody").and_then(|body| child(body, A, "lstStyle"));
                    styles.push(levels(list, &self.theme));
                }
                styles.push(match (placeholder.is_some(), kind) {
                    (true, "title" | "ctrTitle") => under.title.clone(),
                    (true, _) => under.body.clone(),
                    (false, _) => under.other.clone(),
                });
                styles.push(under.default.clone());
                self.text(body, &styles, rels)
            }
            None => String::new(),
        };
        let id = self.id();
        if let Some(shape) = shape.filter(|_| fill.is_some() || line.is_some()) {
            // A shape with words in it: the page centres them.
            // Its words sit where the file put them, not in its middle.
            let column = if words.is_empty() {
                String::new()
            } else {
                format!(
                    ";flex-direction:column;align-items:stretch;justify-content:{justify};text-align:left"
                )
            };
            let _ = writeln!(
                html,
                "  <div data-id=\"{id}\" data-shape=\"{shape}\" style=\"{css}{column}\">{words}</div>"
            );
        } else {
            let _ = writeln!(
                html,
                "  <div data-id=\"{id}\" style=\"{css};display:flex;flex-direction:column;justify-content:{justify};padding:{:.0}px {:.0}px {:.0}px {:.0}px\">{words}</div>",
                inset("tIns", 45_720.),
                inset("rIns", 91_440.),
                inset("bIns", 45_720.),
                inset("lIns", 91_440.),
            );
        }
    }

    /// A table or a chart.
    fn graphic(
        &mut self,
        node: Node<'_, '_>,
        space: Space,
        rels: &BTreeMap<String, (String, String)>,
        html: &mut String,
    ) {
        let Some(frame) = self.frame(Some(node), space, &[]) else {
            return;
        };
        let Some(data) =
            child(node, A, "graphic").and_then(|graphic| child(graphic, A, "graphicData"))
        else {
            return;
        };
        if let Some(table) = child(data, A, "tbl") {
            self.tables += 1;
            let _ = write!(
                html,
                "  <table data-id=\"{}\" style=\"{};height:auto\">",
                self.id(),
                Self::place(&frame)
            );
            let widths: Vec<f64> = child(table, A, "tblGrid")
                .into_iter()
                .flat_map(|grid| children(grid, A, "gridCol"))
                .filter_map(|column| emu(column, "w"))
                .collect();
            let whole: f64 = widths.iter().sum::<f64>().max(1.);
            html.push_str("<colgroup>");
            for width in &widths {
                let _ = write!(html, "<col style=\"width:{:.1}%\">", width / whole * 100.);
            }
            html.push_str("</colgroup>");
            let header = child(table, A, "tblPr")
                .is_some_and(|properties| properties.attribute("firstRow") == Some("1"));
            for (at, row) in children(table, A, "tr").enumerate() {
                html.push_str("<tr>");
                for cell in children(row, A, "tc") {
                    // The rest of a cell that spans several is not a cell.
                    if cell.attribute("hMerge") == Some("1")
                        || cell.attribute("vMerge") == Some("1")
                    {
                        continue;
                    }
                    let tag = if header && at == 0 { "th" } else { "td" };
                    let mut attributes = String::new();
                    if let Some(span) = cell.attribute("gridSpan").filter(|span| *span != "1") {
                        let _ = write!(attributes, " colspan=\"{}\"", attribute(span));
                    }
                    if let Some(span) = cell.attribute("rowSpan").filter(|span| *span != "1") {
                        let _ = write!(attributes, " rowspan=\"{}\"", attribute(span));
                    }
                    let fill = child(cell, A, "tcPr")
                        .and_then(|properties| child(properties, A, "solidFill"))
                        .and_then(|fill| self.theme.colour(fill));
                    // How its words are set, as its first run says.
                    let run = cell
                        .descendants()
                        .find(|part| part.has_tag_name((A, "rPr")))
                        .map(|run| TextStyle::of_run(run, &self.theme))
                        .unwrap_or_default();
                    let mut css = String::new();
                    if let Some(fill) = fill {
                        let _ = write!(css, "background:{};", fill.css());
                    }
                    if let Some(colour) = run.colour {
                        let _ = write!(css, "color:{};", colour.css());
                    }
                    if let Some(size) = run.size {
                        let _ = write!(css, "font-size:{:.0}px;", size * self.point);
                    }
                    if let Some(bold) = run.bold {
                        let _ = write!(css, "font-weight:{};", if bold { 700 } else { 400 });
                    }
                    if !css.is_empty() {
                        let _ = write!(attributes, " style=\"{}\"", css.trim_end_matches(';'));
                    }
                    let said: String = child(cell, A, "txBody")
                        .into_iter()
                        .flat_map(|body| children(body, A, "p"))
                        .map(|paragraph| {
                            paragraph
                                .descendants()
                                .filter(|part| part.has_tag_name((A, "t")))
                                .filter_map(|text| text.text())
                                .collect::<String>()
                        })
                        .filter(|line| !line.trim().is_empty())
                        .map(|line| escape(&line))
                        .collect::<Vec<_>>()
                        .join("<br>");
                    let _ = write!(html, "<{tag}{attributes}>{said}</{tag}>");
                }
                html.push_str("</tr>");
            }
            html.push_str("</table>\n");
            return;
        }
        let chart = data
            .children()
            .find(|part| part.has_tag_name((C, "chart")))
            .and_then(|chart| chart.attribute((R, "id")))
            .and_then(|id| rels.get(id))
            .and_then(|(_, part)| xml(self.parts, part))
            .and_then(chart_spec);
        match chart {
            Some(spec) => {
                self.charts += 1;
                let _ = writeln!(
                    html,
                    "  <figure data-id=\"{}\" data-chart='{}' style=\"{};margin:0\"></figure>",
                    self.id(),
                    spec.to_string()
                        .replace('&', "&amp;")
                        .replace('\'', "&#39;")
                        .replace('<', "&lt;"),
                    Self::place(&frame),
                );
            }
            None => self.skip("diagrams and embedded objects (SmartArt, video, equations)"),
        }
    }
}

/// A chart part's kind and numbers, as the deck's page draws a chart.
fn chart_spec(text: &str) -> Option<serde_json::Value> {
    let document = Document::parse(text).ok()?;
    let plot = document
        .descendants()
        .find(|node| node.has_tag_name((C, "plotArea")))?;
    let chart = plot
        .children()
        .find(|node| node.tag_name().name().ends_with("Chart"))?;
    let kind = match chart.tag_name().name() {
        "barChart" | "bar3DChart" => {
            let across =
                child(chart, C, "barDir").and_then(|dir| dir.attribute("val")) == Some("bar");
            let stacked = child(chart, C, "grouping")
                .and_then(|grouping| grouping.attribute("val"))
                .is_some_and(|grouping| grouping.contains("tacked"));
            match (across, stacked) {
                (true, _) => "hbar",
                (false, true) => "stacked",
                _ => "bar",
            }
        }
        "lineChart" | "line3DChart" | "scatterChart" => "line",
        "areaChart" | "area3DChart" => "area",
        "pieChart" | "pie3DChart" => "pie",
        "doughnutChart" => "donut",
        _ => return None,
    };
    let points = |node: Option<Node<'_, '_>>| -> Vec<String> {
        node.into_iter()
            .flat_map(|node| node.descendants())
            .filter(|point| point.has_tag_name((C, "pt")))
            .map(|point| {
                child(point, C, "v")
                    .and_then(|value| value.text())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    };
    let mut labels: Vec<String> = Vec::new();
    let mut series = Vec::new();
    for one in children(chart, C, "ser") {
        let names = points(child(one, C, "cat").or_else(|| child(one, C, "xVal")));
        if names.len() > labels.len() {
            labels = names;
        }
        let values: Vec<f64> = points(child(one, C, "val").or_else(|| child(one, C, "yVal")))
            .iter()
            .map(|value| value.parse::<f64>().unwrap_or(0.))
            .collect();
        let name = points(child(one, C, "tx"))
            .into_iter()
            .next()
            .unwrap_or_default();
        series.push(serde_json::json!({ "name": name, "values": values }));
    }
    if series.is_empty() {
        return None;
    }
    if labels.is_empty() {
        let most = series
            .iter()
            .map(|one| one["values"].as_array().map_or(0, Vec::len))
            .max()
            .unwrap_or(0);
        labels = (1..=most).map(|n| n.to_string()).collect();
    }
    Some(serde_json::json!({ "type": kind, "labels": labels, "series": series }))
}

/// The background a slide, its layout or its master says.
fn background(document: &Document<'_>, theme: &Theme) -> Option<Rgba> {
    let bg = child(child(document.root_element(), P, "cSld")?, P, "bg")?;
    if let Some(properties) = child(bg, P, "bgPr") {
        return child(properties, A, "solidFill")
            .and_then(|fill| theme.colour(fill))
            .or_else(|| {
                theme.colour(child(
                    child(child(properties, A, "gradFill")?, A, "gsLst")?,
                    A,
                    "gs",
                )?)
            });
    }
    theme.colour(child(bg, P, "bgRef")?)
}

/// The endings of files slides are made from: a deck, or a template.
pub const EXTENSIONS: [&str; 2] = ["pptx", "potx"];

/// Whether `path` is a file slides are made from.
pub fn is_powerpoint(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|extension| EXTENSIONS.contains(&extension.as_str()))
}

/// Turns the PowerPoint file at `path` into slides of `live`'s deck, after
/// the ones it has. With `look`, the deck takes the file's size, colours
/// and fonts (for a deck that was empty). Answers with what was made, in
/// words for an agent.
pub fn powerpoint(path: &Path, live: &Live, look: bool) -> Result<String> {
    let parts = unzip(path)?;
    let presentation_text =
        xml(&parts, "ppt/presentation.xml").context("it is not a PowerPoint file (.pptx)")?;
    let presentation = Document::parse(presentation_text)
        .context("the file's presentation part cannot be read")?;
    let presentation_rels = rels(&parts, "ppt/presentation.xml");
    let size = child(presentation.root_element(), P, "sldSz")
        .and_then(|size| Some((emu(size, "cx")?, emu(size, "cy")?)))
        .unwrap_or((12_192_000., 6_858_000.));
    let mut deck = live.deck();
    if look || deck.slides.is_empty() {
        // The file's own proportions, at the usual height in pixels.
        let width = (1080. * size.0 / size.1.max(1.))
            .round()
            .clamp(320., 7_680.);
        deck.size = [width as u32, 1080];
    }
    let scale = f64::from(deck.size[0]) / size.0.max(1.);
    let slide_parts: Vec<String> = child(presentation.root_element(), P, "sldIdLst")
        .into_iter()
        .flat_map(|list| children(list, P, "sldId"))
        .filter_map(|slide| presentation_rels.get(slide.attribute((R, "id"))?))
        .map(|(_, part)| part.clone())
        .take(MAX_SLIDES)
        .collect();
    if slide_parts.is_empty() {
        bail!("the file has no slides");
    }
    let stem: String = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "slide".into());
    let mut reader = Reader {
        parts: &parts,
        theme: Theme::default(),
        scale,
        point: 12_700. * scale,
        site: live.site(),
        media: BTreeMap::new(),
        left_out: BTreeMap::new(),
        pictures: 0,
        tables: 0,
        charts: 0,
        ids: 0,
    };
    let default = levels(
        child(presentation.root_element(), P, "defaultTextStyle"),
        &reader.theme,
    );
    let mut made = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    let mut first_background = None;
    let taken = |name: &str, files: &[(String, String)]| {
        live.text(name).is_some() || files.iter().any(|(known, _)| known == name)
    };
    for (index, part) in slide_parts.iter().enumerate() {
        let Some(text) = xml(&parts, part) else {
            continue;
        };
        let Ok(document) = Document::parse(text) else {
            reader.skip("slides that cannot be read");
            continue;
        };
        let slide_rels = rels(&parts, part);
        let named = |kind: &str, from: &BTreeMap<String, (String, String)>| {
            from.values()
                .find(|(known, _)| known == kind)
                .map(|(_, part)| part.clone())
        };
        let layout_part = named("slideLayout", &slide_rels);
        let master_part = layout_part
            .as_ref()
            .and_then(|layout| named("slideMaster", &rels(&parts, layout)));
        let layout_text = layout_part.as_ref().and_then(|part| xml(&parts, part));
        let master_text = master_part.as_ref().and_then(|part| xml(&parts, part));
        if let Some(theme) = master_part
            .as_ref()
            .and_then(|master| named("theme", &rels(&parts, master)))
            .and_then(|part| xml(&parts, &part))
        {
            reader.theme = Theme::read(theme);
        }
        let master = master_text.and_then(Sheet::read);
        let styles = |name: &str| {
            let list = master
                .as_ref()
                .and_then(|master| child(master.document.root_element(), P, "txStyles"))
                .and_then(|styles| child(styles, P, name));
            levels(list, &reader.theme)
        };
        let under = Under {
            title: styles("titleStyle"),
            body: styles("bodyStyle"),
            other: styles("otherStyle"),
            default: default.clone(),
            layout: layout_text.and_then(Sheet::read),
            master,
        };
        let ground = background(&document, &reader.theme)
            .or_else(|| background(&under.layout.as_ref()?.document, &reader.theme))
            .or_else(|| background(&under.master.as_ref()?.document, &reader.theme))
            .or_else(|| reader.theme.colours.get("lt1").copied())
            .unwrap_or(Rgba {
                r: 255.,
                g: 255.,
                b: 255.,
                a: 1.,
            });
        first_background.get_or_insert(ground);
        reader.ids = 0;
        let mut html = format!(
            "<section class=\"slide imported\" style=\"background:{}\">\n",
            ground.css()
        );
        // What the layout and the master draw behind every slide (a logo, a
        // bar): their shapes that hold no place for a slide's own.
        let sheets = [
            (under.master.as_ref(), master_part.as_deref()),
            (under.layout.as_ref(), layout_part.as_deref()),
        ];
        for (sheet, sheet_part) in sheets {
            let Some(sheet) = sheet else {
                continue;
            };
            let sheet_rels = sheet_part
                .map(|part| rels(&parts, part))
                .unwrap_or_default();
            for shape in sheet
                .tree()
                .into_iter()
                .flat_map(|tree| tree.children())
                .filter(Node::is_element)
            {
                if placeholder_of(shape).is_none() {
                    reader.shape(shape, Space::SLIDE, &under, &sheet_rels, &mut html);
                }
            }
        }
        let tree =
            child(document.root_element(), P, "cSld").and_then(|slide| child(slide, P, "spTree"));
        for shape in tree
            .into_iter()
            .flat_map(|tree| tree.children())
            .filter(Node::is_element)
        {
            reader.shape(shape, Space::SLIDE, &under, &slide_rels, &mut html);
        }
        html.push_str("</section>\n");
        if document
            .descendants()
            .any(|node| node.has_tag_name((P, "timing")) && node.has_children())
        {
            reader.skip("animations");
        }
        // Its notes: the words of the notes page's body.
        let notes = named("notesSlide", &slide_rels)
            .and_then(|part| xml(&parts, &part).map(str::to_string))
            .and_then(|text| {
                let notes = Document::parse(&text).ok()?;
                let body = notes.descendants().find(|shape| {
                    shape.has_tag_name((P, "sp"))
                        && placeholder_of(*shape)
                            .is_some_and(|ph| ph.attribute("type") == Some("body"))
                })?;
                let lines: Vec<String> = children(child(body, P, "txBody")?, A, "p")
                    .map(|paragraph| {
                        paragraph
                            .descendants()
                            .filter(|part| part.has_tag_name((A, "t")))
                            .filter_map(|text| text.text())
                            .collect::<String>()
                    })
                    .collect();
                Some(lines.join("\n").trim().to_string())
            })
            .unwrap_or_default();
        let file = crate::deck::free_slide_file(&format!("{stem}-{}", index + 1), |name| {
            taken(name, &files)
        });
        deck.slides.push(crate::deck::SlideRef {
            file: file.clone(),
            notes,
            ..Default::default()
        });
        made.push(file.clone());
        files.push((file, html));
    }
    if made.is_empty() {
        bail!("none of the file's slides could be read");
    }
    if deck.title.trim().is_empty() || deck.title == "Untitled deck" {
        deck.title = stem.clone();
    }
    // Written as an agent writes them: the app notices, and the user's page
    // follows.
    let site = live.site();
    std::fs::create_dir_all(site.join(crate::deck::SLIDES))?;
    for (file, html) in &files {
        ph_artifacts::library::write_atomic(&site.join(file), html.as_bytes())?;
    }
    let had_look = live.text(STYLE);
    if look || had_look.is_none() {
        let ground = first_background.unwrap_or(Rgba {
            r: 255.,
            g: 255.,
            b: 255.,
            a: 1.,
        });
        ph_artifacts::library::write_atomic(
            &site.join(STYLE),
            imported_look(&reader.theme, ground, &stem).as_bytes(),
        )?;
    } else if let Some(css) = had_look.filter(|css| !css.contains(".slide.imported")) {
        // The deck keeps its look; the slides that came in lie on it as
        // they were laid out.
        let css = format!("{css}\n{IMPORTED_RULE}");
        ph_artifacts::library::write_atomic(&site.join(STYLE), css.as_bytes())?;
    }
    ph_artifacts::library::write_atomic(&site.join(DECK), deck.to_text().as_bytes())?;

    let mut said = format!(
        "Made {} slide{} from {}: {} to {}. Text is text (each box at the place it had), {} picture{} in media/, {} table{}, {} chart{} with {} numbers.",
        made.len(),
        if made.len() == 1 { "" } else { "s" },
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        made.first().cloned().unwrap_or_default(),
        made.last().cloned().unwrap_or_default(),
        reader.pictures,
        if reader.pictures == 1 { "" } else { "s" },
        reader.tables,
        if reader.tables == 1 { "" } else { "s" },
        reader.charts,
        if reader.charts == 1 { "" } else { "s" },
        if reader.charts == 1 { "its" } else { "their" },
    );
    if !reader.left_out.is_empty() {
        said.push_str(" Left out: ");
        let list: Vec<String> = reader
            .left_out
            .iter()
            .map(|(what, count)| format!("{what} ({count})"))
            .collect();
        said.push_str(&list.join(", "));
        said.push('.');
    }
    said.push_str(
        " Every element is placed with `position:absolute`, as in the file. Call deck_look to see what does not fit, and tidy the slides the user cares about (or rebuild them in the deck's own layouts if they ask for a new look).",
    );
    Ok(said)
}

/// What lets slides from a PowerPoint file lie as they were laid out,
/// whatever the deck's look says of padding and flow.
const IMPORTED_RULE: &str = "/* Slides from a PowerPoint file: everything on them has a place of its own,\n   and says itself how it is set. */\n.slide.imported { padding: 0; display: block; }\n.slide.imported p, .slide.imported li, .slide.imported ul, .slide.imported ol { margin: 0; }\n.slide.imported table { font-size: 28px; }\n.slide.imported th, .slide.imported td { padding: 10px 14px; border: 0; color: inherit; font-size: inherit; font-weight: inherit; letter-spacing: 0; text-transform: none; text-align: left; }\n";

/// A look made of the file's theme: its colours and fonts as the deck's
/// variables, so slides added later sit beside the ones that came in.
fn imported_look(theme: &Theme, ground: Rgba, name: &str) -> String {
    let slot = |name: &str, fallback: &str| {
        theme
            .colours
            .get(name)
            .copied()
            .or_else(|| Rgba::hex(fallback))
            .unwrap_or(ground)
    };
    let text = slot("dk1", "111111");
    let accent = slot("accent1", "4472c4");
    let dark_ground = (ground.r + ground.g + ground.b) / 3. < 128.;
    let text = if dark_ground {
        slot("lt1", "ffffff")
    } else {
        text
    };
    let body = if theme.body.is_empty() {
        "Arial".to_string()
    } else {
        theme.body.clone()
    };
    let heading = if theme.heading.is_empty() {
        body.clone()
    } else {
        theme.heading.clone()
    };
    let base = crate::looks::look("clean").css();
    // The built-in look's rules, under this file's colours and fonts.
    let rules = base
        .split_once("}\n")
        .map(|(_, rules)| rules)
        .unwrap_or(&base);
    format!(
        "/* The deck's look (from {name}). Colours and fonts first: change them here and every\n   slide follows. Sizes are in pixels of a slide. */\n.slide {{\n  --bg: {};\n  --fg: {};\n  --muted: {};\n  --accent: {};\n  --accent-fg: {};\n  --surface: {};\n  --line: {};\n  --font-head: {};\n  --font-body: {};\n  --font-code: \"JetBrains Mono\", ui-monospace, Menlo, Consolas, monospace;\n  --head-weight: 700;\n}}\n{rules}\n{IMPORTED_RULE}",
        ground.css(),
        text.css(),
        text.mix(if dark_ground { 0. } else { 255. }, 0.6).css(),
        accent.css(),
        if (accent.r + accent.g + accent.b) / 3. < 150. {
            "#ffffff"
        } else {
            "#111111"
        },
        ground.mix(if dark_ground { 255. } else { 0. }, 0.94).css(),
        ground.mix(if dark_ground { 255. } else { 0. }, 0.82).css(),
        font_stack(&heading),
        font_stack(&body),
        name = name.replace("*/", ""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_are_named_from_where_they_are_named() {
        assert_eq!(
            resolve("ppt/slides/slide1.xml", "../media/image1.png"),
            "ppt/media/image1.png"
        );
        assert_eq!(
            resolve("ppt/presentation.xml", "slides/slide1.xml"),
            "ppt/slides/slide1.xml"
        );
        assert_eq!(
            resolve("ppt/slides/slide1.xml", "/ppt/charts/chart1.xml"),
            "ppt/charts/chart1.xml"
        );
        assert_eq!(
            resolve("ppt/slides/slide1.xml", "../../../../etc/passwd"),
            "etc/passwd"
        );
    }

    #[test]
    fn colours_are_the_themes_and_change_as_said() {
        let mut theme = Theme::default();
        theme
            .colours
            .insert("accent1".into(), Rgba::hex("4472c4").unwrap());
        theme
            .colours
            .insert("dk1".into(), Rgba::hex("000000").unwrap());
        let fill = |inner: &str| format!("<a:solidFill xmlns:a=\"{A}\">{inner}</a:solidFill>");
        let colour = |inner: &str| {
            let text = fill(inner);
            let document = Document::parse(&text).unwrap();
            theme.colour(document.root_element()).map(Rgba::css)
        };
        assert_eq!(
            colour("<a:srgbClr val=\"A8431F\"/>").as_deref(),
            Some("#a8431f")
        );
        assert_eq!(
            colour("<a:schemeClr val=\"accent1\"/>").as_deref(),
            Some("#4472c4")
        );
        assert_eq!(
            colour("<a:schemeClr val=\"tx1\"/>").as_deref(),
            Some("#000000")
        );
        assert_eq!(
            colour("<a:srgbClr val=\"000000\"><a:alpha val=\"50000\"/></a:srgbClr>").as_deref(),
            Some("rgb(0 0 0 / 0.50)")
        );
        // Half as light again is lighter; a colour the theme has not is none.
        assert_eq!(
            colour("<a:srgbClr val=\"808080\"><a:lumMod val=\"50000\"/></a:srgbClr>").as_deref(),
            Some("#404040")
        );
        assert_eq!(colour("<a:schemeClr val=\"accent5\"/>"), None);
        assert_eq!(font_stack("Georgia"), "'Georgia', Georgia, serif");
        assert_eq!(
            font_stack("Open Sans\";x"),
            "'Open Sansx', Arial, sans-serif"
        );
    }

    #[test]
    fn a_charts_numbers_are_read() {
        let chart = format!(
            "<c:chartSpace xmlns:c=\"{C}\"><c:chart><c:plotArea><c:barChart><c:barDir val=\"bar\"/><c:grouping val=\"clustered\"/><c:ser><c:tx><c:strRef><c:strCache><c:pt idx=\"0\"><c:v>Share</c:v></c:pt></c:strCache></c:strRef></c:tx><c:cat><c:strRef><c:strCache><c:pt idx=\"0\"><c:v>A</c:v></c:pt><c:pt idx=\"1\"><c:v>B</c:v></c:pt></c:strCache></c:strRef></c:cat><c:val><c:numRef><c:numCache><c:pt idx=\"0\"><c:v>38</c:v></c:pt><c:pt idx=\"1\"><c:v>22.5</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"
        );
        let spec = chart_spec(&chart).unwrap();
        assert_eq!(spec["type"], "hbar");
        assert_eq!(spec["labels"], serde_json::json!(["A", "B"]));
        assert_eq!(spec["series"][0]["name"], "Share");
        assert_eq!(spec["series"][0]["values"], serde_json::json!([38.0, 22.5]));
        assert!(chart_spec("<x/>").is_none());
    }

    /// A deck exported to PowerPoint comes back as slides: the two halves
    /// read each other.
    #[test]
    fn what_was_exported_comes_back() {
        let base = std::env::temp_dir().join(format!("ph-slides-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let site = base.join("site");
        std::fs::create_dir_all(site.join("media")).unwrap();
        std::fs::write(site.join("media/photo.png"), b"\x89PNG....").unwrap();
        let measured: serde_json::Value = serde_json::from_str(
            r##"{ "title": "Plan", "size": [1920, 1080], "slides": [{ "file": "slides/a.html", "notes": "Say hello.", "transition": "", "background": "#f6f1e7", "items": [
                { "kind": "shape", "x": 100, "y": 100, "w": 400, "h": 200, "shape": "rounded", "radius": 24, "fill": "#a8431f", "fillAlpha": 1, "stroke": "", "strokeWidth": 0, "opacity": 1, "rotate": 0 },
                { "kind": "text", "x": 112, "y": 96, "w": 900, "h": 80, "rotate": 0, "valign": "top", "wrap": true, "paragraphs": [
                    { "align": "center", "bullet": "none", "level": 0, "lineHeight": 1.2, "spaceAfter": 0, "runs": [{ "text": "Costs <fell>", "font": "Georgia", "size": 64, "bold": true, "italic": false, "underline": false, "color": "#241d17", "alpha": 1, "link": "" }] },
                    { "align": "left", "bullet": "bullet", "level": 0, "lineHeight": 1.4, "spaceAfter": 0, "runs": [{ "text": "First", "font": "Arial", "size": 34, "bold": false, "italic": false, "underline": false, "color": "#241d17", "alpha": 1, "link": "" }] }
                ] },
                { "kind": "picture", "x": 1000, "y": 300, "w": 400, "h": 300, "rotate": 0, "radius": 0, "src": "media/photo.png" },
                { "kind": "chart", "x": 1100, "y": 620, "w": 700, "h": 380, "rotate": 0, "text": "#6f6357", "colors": ["#a8431f"], "spec": { "type": "hbar", "labels": ["Couriers", "Van"], "series": [{ "name": "Share", "values": [38, 22] }] } }
            ] }] }"##,
        )
        .unwrap();
        let file = base.join("Plan 2026.pptx");
        std::fs::write(&file, crate::pptx::build(&measured, &site, false).unwrap()).unwrap();

        // Into an empty deck: it takes the file's size and a look from it.
        let deck_site = base.join("deck");
        std::fs::create_dir_all(&deck_site).unwrap();
        let live = Live::open("d", &deck_site);
        let said = powerpoint(&file, &live, true).unwrap();
        assert!(
            said.starts_with("Made 1 slide from Plan 2026.pptx: slides/plan-2026-1.html"),
            "{said}"
        );
        assert!(
            said.contains("1 picture") && said.contains("1 chart"),
            "{said}"
        );
        live.scan();
        let deck = live.deck();
        assert_eq!(
            (deck.size, deck.title.as_str()),
            ([1920, 1080], "Plan 2026")
        );
        assert_eq!(deck.slides[0].notes, "Say hello.");
        let slide = live.text("slides/plan-2026-1.html").unwrap();
        assert!(
            slide.starts_with("<section class=\"slide imported\" style=\"background:#f6f1e7\">"),
            "{slide}"
        );
        // The shape, where it was; the words, their size and weight kept.
        assert!(
            slide.contains("data-shape=\"rounded\" style=\"position:absolute;left:100px;top:100px;width:400px;height:200px;border-radius:24px;background:#a8431f\"></div>"),
            "{slide}"
        );
        assert!(
            slide.contains("<p style=\"margin:0;font-size:64px;font-weight:700;color:#241d17;font-family:'Georgia', Georgia, serif;text-align:center;line-height:1.20\">Costs &lt;fell&gt;</p>"),
            "{slide}"
        );
        assert!(
            slide.contains("<ul")
                && slide.contains("font-size:34px")
                && slide.contains(">First</li>"),
            "{slide}"
        );
        // The picture is with the deck; the chart has its numbers.
        let picture = slide
            .split("src=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert!(picture.starts_with("media/picture-") && deck_site.join(picture).is_file());
        assert!(
            slide.contains("data-chart='{\"type\":\"hbar\",\"labels\":[\"Couriers\",\"Van\"]"),
            "{slide}"
        );
        let css = live.text(STYLE).unwrap();
        assert!(css.contains("(from Plan 2026)") && css.contains(".slide.imported { padding: 0"));

        // Into a deck that has slides and a look: both stay, the new ones follow.
        let said = powerpoint(&file, &live, false).unwrap();
        assert!(said.contains("slides/plan-2026-1-2.html"), "{said}");
        live.scan();
        assert_eq!(live.deck().slides.len(), 2);
        assert_eq!(
            live.text(STYLE)
                .unwrap()
                .matches(".slide.imported { padding")
                .count(),
            1
        );

        assert!(powerpoint(&site.join("media/photo.png"), &live, false).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
