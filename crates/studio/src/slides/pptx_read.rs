//! Reading a PowerPoint file (PPTX or POTX) into a deck: its theme's
//! colours and fonts, its masters' and layouts' places and looks, and each
//! slide's shapes, text, pictures, tables, charts (their data), groups,
//! connectors, SmartArt (as the shapes PowerPoint drew it with), notes,
//! comments, transitions and entrance animations. What PilotHouse does not
//! edit (videos, embedded objects, ink, EMF pictures) is kept as it was
//! and written back. The deck remembers the file, so its masters and
//! layouts go back as they were while its theme is the one read.
//!
//! PowerPoint files vary a great deal; what cannot be read is left out
//! rather than stopping the whole file, and each such thing is noted.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use roxmltree::{Document, Node};

use super::model::*;
use super::pptx::rels_of;
use super::space::Space;

/// English Metric Units a point.
const EMU: f32 = 12_700.;
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Numbers from the file, never NaN or infinite: those would be saved as
/// JSON null, and the presentation would not open again.
trait ParseFinite {
    fn parse_finite<T: std::str::FromStr + Copy + Into<f64>>(&self) -> Result<T, ()>;
}

impl ParseFinite for str {
    fn parse_finite<T: std::str::FromStr + Copy + Into<f64>>(&self) -> Result<T, ()> {
        let value: T = self.parse().map_err(|_| ())?;
        if value.into().is_finite() {
            Ok(value)
        } else {
            Err(())
        }
    }
}

fn pt(emu: &str) -> f32 {
    emu.parse_finite::<f64>()
        .map(|value| (value / EMU as f64) as f32)
        .unwrap_or(0.)
}

/// What reading a file gave: the deck, and what was left out.
pub struct Read {
    pub deck: Deck,
    pub notes: Vec<String>,
}

/// The nearest outline PilotHouse draws for a PowerPoint preset.
pub fn nearest_geometry(preset: &str) -> Geometry {
    match preset {
        "rect"
        | "flowChartProcess"
        | "snip1Rect"
        | "snip2SameRect"
        | "frame"
        | "plaque"
        | "bevel"
        | "foldedCorner"
        | "flowChartAlternateProcess" => Geometry::Rect,
        "roundRect" | "round1Rect" | "round2SameRect" | "round2DiagRect" => Geometry::RoundRect,
        "ellipse" | "flowChartConnector" | "donut" | "pie" | "chord" | "arc" | "blockArc" => {
            Geometry::Ellipse
        }
        "triangle" | "flowChartExtract" => Geometry::Triangle,
        "rtTriangle" => Geometry::RightTriangle,
        "diamond" | "flowChartDecision" => Geometry::Diamond,
        "parallelogram" | "flowChartInputOutput" => Geometry::Parallelogram,
        "trapezoid" | "flowChartManualOperation" => Geometry::Trapezoid,
        "pentagon" => Geometry::Pentagon,
        "hexagon" | "flowChartPreparation" => Geometry::Hexagon,
        "octagon" => Geometry::Octagon,
        "chevron" => Geometry::Chevron,
        "homePlate" => Geometry::HomePlate,
        "rightArrow" | "notchedRightArrow" | "stripedRightArrow" => Geometry::RightArrow,
        "leftArrow" => Geometry::LeftArrow,
        "upArrow" => Geometry::UpArrow,
        "downArrow" => Geometry::DownArrow,
        "leftRightArrow" => Geometry::LeftRightArrow,
        "star5" | "star4" | "star7" | "star8" => Geometry::Star5,
        "star6" | "star10" | "star12" | "star16" | "star24" | "star32" => Geometry::Star6,
        "plus" | "mathPlus" => Geometry::Plus,
        "heart" => Geometry::Heart,
        "wedgeRectCallout" | "wedgeRoundRectCallout" | "wedgeEllipseCallout" | "cloudCallout" => {
            Geometry::Callout
        }
        "line" | "straightConnector1" | "bentConnector2" | "bentConnector3"
        | "curvedConnector3" => Geometry::Line,
        _ => Geometry::Rect,
    }
}

/// Our name for a theme colour PowerPoint names.
fn slot_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "dk1" => "dark1",
        "lt1" => "light1",
        "dk2" => "dark2",
        "lt2" => "light2",
        "accent1" => "accent1",
        "accent2" => "accent2",
        "accent3" => "accent3",
        "accent4" => "accent4",
        "accent5" => "accent5",
        "accent6" => "accent6",
        "hlink" => "link",
        "folHlink" => "followed_link",
        _ => return None,
    })
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|child| child.tag_name().name() == name)
}

fn children<'a, 'input>(
    node: Node<'a, 'input>,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'input>> + 'a {
    node.children()
        .filter(move |child| child.tag_name().name() == name)
}

fn path<'a, 'input>(node: Node<'a, 'input>, names: &[&str]) -> Option<Node<'a, 'input>> {
    let mut at = node;
    for name in names {
        at = child(at, name)?;
    }
    Some(at)
}

/// An attribute in the relationships namespace (`r:embed`, `r:id`).
fn rel_attr<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name() == name && attribute.namespace() == Some(REL_NS))
        .map(|attribute| attribute.value())
}

/// Where a placeholder's text and place come from in its layout and
/// master.
#[derive(Clone)]
struct Place {
    kind: String,
    idx: u32,
    frame: Option<Frame>,
    ours: Option<PlaceholderKind>,
    /// Where its text sits, when it says.
    anchor: Option<Anchor>,
}

/// What reading one part (a slide, a layout, a master) needs.
struct Part<'a> {
    name: String,
    rels: Vec<(String, String, String, bool)>,
    /// Places it inherits (its layout's and master's).
    places: &'a [Place],
}

impl Part<'_> {
    fn rel(&self, id: &str) -> Option<&(String, String, String, bool)> {
        self.rels.iter().find(|(rel, ..)| rel == id)
    }
}

struct Reader<'a> {
    files: &'a BTreeMap<String, Vec<u8>>,
    space: &'a Space,
    deck: Deck,
    /// `bg1` and the like, to our theme slots.
    clr_map: HashMap<String, String>,
    /// Pictures already copied, by part.
    media: HashMap<String, String>,
    notes: Vec<String>,
    /// The element each shape id of the part being read became.
    shape_ids: HashMap<String, String>,
    /// Connectors waiting for their ends: element, start and end (shape
    /// id, PowerPoint site).
    connectors: Vec<(String, Option<End>, Option<End>)>,
}

/// A connector's end as PowerPoint gives it: the shape's id and the site.
type End = (String, u32);

/// Reads the PowerPoint file `files` (all of its parts), putting its
/// pictures in `space`.
pub fn read(files: &BTreeMap<String, Vec<u8>>, space: &Space, title: &str) -> Result<Read> {
    let mut reader = Reader {
        files,
        space,
        deck: Deck {
            title: title.to_string(),
            ..Deck::default()
        },
        clr_map: HashMap::new(),
        media: HashMap::new(),
        notes: Vec::new(),
        shape_ids: HashMap::new(),
        connectors: Vec::new(),
    };
    reader.read_all()?;
    Ok(Read {
        deck: reader.deck,
        notes: reader.notes,
    })
}

impl Reader<'_> {
    fn text(&self, part: &str) -> Option<String> {
        self.files
            .get(part)
            .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
    }

    fn note(&mut self, what: impl Into<String>) {
        let what = what.into();
        if !self.notes.contains(&what) {
            self.notes.push(what);
        }
    }

    fn read_all(&mut self) -> Result<()> {
        let presentation = self
            .text("ppt/presentation.xml")
            .context("this is not a PowerPoint presentation (no ppt/presentation.xml)")?;
        let document = Document::parse(&presentation).context("its presentation cannot be read")?;
        let root = document.root_element();
        if let Some(size) = child(root, "sldSz") {
            let width = pt(size.attribute("cx").unwrap_or("12192000"));
            let height = pt(size.attribute("cy").unwrap_or("6858000"));
            let preset = SIZES
                .iter()
                .find(|(_, _, w, h)| (w - width).abs() < 1.5 && (h - height).abs() < 1.5)
                .map(|(id, ..)| id.to_string());
            // PowerPoint's own limits: one inch to 56 inches a side.
            self.deck.size = SlideSize {
                width: width.clamp(72., 4032.),
                height: height.clamp(72., 4032.),
                preset,
            };
        }
        let rels = rels_of(self.files, "ppt/presentation.xml");
        let target_of = |id: &str| {
            rels.iter()
                .find(|(rel, ..)| rel == id)
                .map(|(_, _, target, _)| target.clone())
        };
        let masters: Vec<String> = path(root, &["sldMasterIdLst"])
            .into_iter()
            .flat_map(|list| children(list, "sldMasterId"))
            .filter_map(|node| rel_attr(node, "id").and_then(target_of))
            .collect();
        if masters.is_empty() {
            bail!("it has no slide master");
        }
        let mut theme = Theme {
            layouts: Vec::new(),
            decorations: Vec::new(),
            ..Theme::default()
        };
        let mut all_places: HashMap<String, Vec<Place>> = HashMap::new();
        for (ix, master) in masters.iter().enumerate() {
            if let Err(err) = self.read_master(master, ix == 0, &mut theme, &mut all_places) {
                self.note(format!("a slide master could not be read: {err:#}"));
            }
        }
        if theme.layouts.is_empty() {
            bail!("its slide layouts cannot be read");
        }
        self.deck.theme = theme;
        // Slides, in order.
        let slide_ids: Vec<(String, String)> = path(root, &["sldIdLst"])
            .into_iter()
            .flat_map(|list| children(list, "sldId"))
            .filter_map(|node| {
                let target = rel_attr(node, "id").and_then(target_of)?;
                Some((node.attribute("id").unwrap_or_default().to_string(), target))
            })
            .collect();
        for (number, (_, part)) in slide_ids.iter().enumerate() {
            match self.read_slide(part, &all_places) {
                Ok(slide) => self.deck.slides.push(std::sync::Arc::new(slide)),
                Err(err) => self.note(format!("slide {} could not be read: {err:#}", number + 1)),
            }
        }
        // Sections (PowerPoint 2010 on).
        for section in document
            .descendants()
            .filter(|node| node.tag_name().name() == "section")
        {
            let name = section.attribute("name").unwrap_or_default().to_string();
            let first = section
                .descendants()
                .find(|node| node.tag_name().name() == "sldId")
                .and_then(|node| node.attribute("id"));
            if let Some(first) = first
                && let Some(index) = slide_ids.iter().position(|(id, _)| id == first)
                && let Some(slide) = self.deck.slides.get(index)
            {
                self.deck.sections.push(Section {
                    name,
                    slide: slide.id.clone(),
                });
            }
        }
        Ok(())
    }

    /// A master: its theme (the first master's is the deck's), its look,
    /// and its layouts.
    fn read_master(
        &mut self,
        master: &str,
        first: bool,
        theme: &mut Theme,
        all_places: &mut HashMap<String, Vec<Place>>,
    ) -> Result<()> {
        let text = self.text(master).context("missing")?;
        let document = Document::parse(&text)?;
        let root = document.root_element();
        let rels = rels_of(self.files, master);
        // The colour names slides use (`bg1`, `tx1`) and the theme's.
        if let Some(map) = child(root, "clrMap") {
            let mut clr_map = HashMap::new();
            for attribute in map.attributes() {
                if let Some(slot) = slot_name(attribute.value()) {
                    clr_map.insert(attribute.name().to_string(), slot.to_string());
                }
            }
            if first {
                self.clr_map = clr_map;
            }
        }
        if first
            && let Some((_, _, theme_part, _)) =
                rels.iter().find(|(_, kind, ..)| kind.ends_with("/theme"))
        {
            self.read_theme(theme_part, theme);
        }
        // Text styles: titles and body text.
        if first && let Some(styles) = child(root, "txStyles") {
            if let Some(title) = child(styles, "titleStyle") {
                theme.title = self.text_style(title, &theme.title, false);
            }
            if let Some(body) = child(styles, "bodyStyle") {
                theme.body = self.text_style(body, &theme.body, true);
            }
        }
        let part = Part {
            name: master.to_string(),
            rels: rels.clone(),
            places: &[],
        };
        let tree = path(root, &["cSld", "spTree"]).context("no shapes")?;
        let master_places = self.places(tree);
        let background = path(root, &["cSld", "bg"]).and_then(|bg| self.background(bg));
        let decorations = self.shapes(tree, &part, true, (0., 0.));
        if first {
            if let Some(background) = background.clone() {
                theme.background = background;
            }
            theme.decorations = decorations.clone();
        }
        // Its layouts.
        let layouts: Vec<String> = path(root, &["sldLayoutIdLst"])
            .into_iter()
            .flat_map(|list| children(list, "sldLayoutId"))
            .filter_map(|node| {
                let id = rel_attr(node, "id")?;
                rels.iter()
                    .find(|(rel, ..)| rel == id)
                    .map(|(_, _, target, _)| target.clone())
            })
            .collect();
        for layout_part in layouts {
            match self.read_layout(&layout_part, &master_places, theme.layouts.len() + 1) {
                Ok((mut layout, places)) => {
                    if !first {
                        // Another master's: its own look, not the theme's.
                        if layout.background.is_none() {
                            layout.background = background.clone();
                        }
                        if layout.theme_decorations {
                            let mut own = decorations.clone();
                            own.extend(layout.decorations);
                            layout.decorations = own;
                        }
                        layout.theme_decorations = false;
                    }
                    all_places.insert(layout_part.clone(), places);
                    theme.layouts.push(layout);
                }
                Err(err) => self.note(format!("a layout could not be read: {err:#}")),
            }
        }
        Ok(())
    }

    fn read_theme(&mut self, part: &str, theme: &mut Theme) {
        let Some(text) = self.text(part) else {
            return;
        };
        let Ok(document) = Document::parse(&text) else {
            return;
        };
        let root = document.root_element();
        if let Some(name) = root.attribute("name").filter(|name| !name.is_empty()) {
            theme.name = name.to_string();
        }
        if let Some(scheme) = root
            .descendants()
            .find(|node| node.tag_name().name() == "clrScheme")
        {
            let mut colors = Vec::new();
            for slot in scheme.children().filter(Node::is_element) {
                let Some(ours) = slot_name(slot.tag_name().name()) else {
                    continue;
                };
                let hex = slot.children().find(Node::is_element).and_then(|color| {
                    match color.tag_name().name() {
                        "srgbClr" => color.attribute("val").map(str::to_string),
                        "sysClr" => color.attribute("lastClr").map(str::to_string),
                        _ => None,
                    }
                });
                if let Some(hex) = hex {
                    colors.push((ours.to_string(), format!("#{}", hex.to_uppercase())));
                }
            }
            if !colors.is_empty() {
                theme.colors = colors;
            }
        }
        let font = |kind: &str| {
            root.descendants()
                .find(|node| node.tag_name().name() == kind)
                .and_then(|node| child(node, "latin"))
                .and_then(|latin| latin.attribute("typeface"))
                .filter(|face| !face.is_empty())
                .map(str::to_string)
        };
        if let Some(major) = font("majorFont") {
            theme.fonts.heading = major;
        }
        if let Some(minor) = font("minorFont") {
            theme.fonts.body = minor;
        }
    }

    /// A text style from a master's `titleStyle` or `bodyStyle`.
    fn text_style(&self, node: Node, base: &TextStyle, bullets: bool) -> TextStyle {
        let mut style = base.clone();
        if let Some(level) = child(node, "lvl1pPr") {
            if let Some(align) = level.attribute("algn") {
                style.align = align_of(align);
            }
            if let Some(defaults) = child(level, "defRPr") {
                if let Some(size) = defaults
                    .attribute("sz")
                    .and_then(|sz| sz.parse_finite::<f32>().ok())
                {
                    style.size = size / 100.;
                }
                style.bold = defaults.attribute("b") == Some("1");
                style.italic = defaults.attribute("i") == Some("1");
                if let Some(color) = self.fill_color(defaults) {
                    style.color = color;
                }
                if let Some(face) =
                    child(defaults, "latin").and_then(|latin| latin.attribute("typeface"))
                {
                    style.font = font_name(face);
                }
            }
            if let Some(spacing) = path(level, &["lnSpc", "spcPct"])
                .and_then(|pct| pct.attribute("val"))
                .and_then(|val| val.parse_finite::<f32>().ok())
            {
                style.line_spacing = spacing / 100_000.;
            }
            if let Some(after) = path(level, &["spcAft", "spcPts"])
                .and_then(|pts| pts.attribute("val"))
                .and_then(|val| val.parse_finite::<f32>().ok())
            {
                style.space_after = after / 100.;
            }
        }
        if bullets {
            style.bullets = (1..=5)
                .map(|level| {
                    node.children()
                        .find(|child| child.tag_name().name() == format!("lvl{level}pPr"))
                        .and_then(|level| child(level, "buChar"))
                        .and_then(|bullet| bullet.attribute("char"))
                        .map(|char| Bullet::Char(char.to_string()))
                        .unwrap_or(Bullet::Char(if level == 1 { "•" } else { "–" }.into()))
                })
                .collect();
        }
        style
    }

    /// The placeholders of a shape tree: their kinds, indexes and places.
    fn places(&self, tree: Node) -> Vec<Place> {
        tree.children()
            .filter(|node| node.tag_name().name() == "sp")
            .filter_map(|sp| {
                let ph = sp
                    .descendants()
                    .find(|node| node.tag_name().name() == "ph")?;
                Some(Place {
                    kind: ph.attribute("type").unwrap_or("obj").to_string(),
                    idx: ph
                        .attribute("idx")
                        .and_then(|idx| idx.parse().ok())
                        .unwrap_or(0),
                    frame: child(sp, "spPr")
                        .and_then(|sp_pr| child(sp_pr, "xfrm"))
                        .map(|xfrm| frame_of(xfrm, (0., 0.))),
                    ours: None,
                    anchor: path(sp, &["txBody", "bodyPr"])
                        .and_then(|pr| pr.attribute("anchor"))
                        .map(anchor_of),
                })
            })
            .collect()
    }

    /// A layout: its own look and places (with our kinds), and the
    /// places slides in it inherit (this layout's, then the master's).
    fn read_layout(
        &mut self,
        part_name: &str,
        master: &[Place],
        number: usize,
    ) -> Result<(Layout, Vec<Place>)> {
        let text = self.text(part_name).context("missing")?;
        let document = Document::parse(&text)?;
        let root = document.root_element();
        let rels = rels_of(self.files, part_name);
        let tree = path(root, &["cSld", "spTree"]).context("no shapes")?;
        let name = path(root, &["cSld"])
            .and_then(|c_sld| c_sld.attribute("name"))
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("Layout {number}"));
        let part = Part {
            name: part_name.to_string(),
            rels,
            places: master,
        };
        let decorations = self.shapes(tree, &part, true, (0., 0.));
        let background = path(root, &["cSld", "bg"]).and_then(|bg| self.background(bg));
        // Each placeholder gets one of our kinds, in order.
        let mut taken: Vec<PlaceholderKind> = Vec::new();
        let mut placeholders = Vec::new();
        let mut places = Vec::new();
        for sp in tree
            .children()
            .filter(|node| node.tag_name().name() == "sp")
        {
            let Some(ph) = sp.descendants().find(|node| node.tag_name().name() == "ph") else {
                continue;
            };
            let kind = ph.attribute("type").unwrap_or("obj").to_string();
            let idx: u32 = ph
                .attribute("idx")
                .and_then(|idx| idx.parse().ok())
                .unwrap_or(0);
            let frame = child(sp, "spPr")
                .and_then(|sp_pr| child(sp_pr, "xfrm"))
                .map(|xfrm| frame_of(xfrm, (0., 0.)))
                .or_else(|| inherited(master, &kind, idx));
            let ours = our_kind(&kind, &taken);
            let anchor = path(sp, &["txBody", "bodyPr"])
                .and_then(|pr| pr.attribute("anchor"))
                .map(anchor_of)
                .or_else(|| find_place(master, &kind, idx).and_then(|place| place.anchor));
            let mut place = Place {
                kind: kind.clone(),
                idx,
                frame,
                ours,
                anchor,
            };
            if let (Some(ours), Some(frame)) = (ours, frame) {
                taken.push(ours);
                let body = child(sp, "txBody");
                let level = body.and_then(|body| path(body, &["lstStyle", "lvl1pPr"]));
                let defaults = level.and_then(|level| child(level, "defRPr"));
                // The layout's own words only when it asks for them
                // (PowerPoint's custom prompt); its sample text is not one.
                let custom = path(sp, &["nvSpPr", "nvPr", "custPrompt"]).is_some();
                let prompt = body
                    .filter(|_| custom)
                    .map(|body| paragraphs_text(body))
                    .and_then(|text| text.lines().next().map(str::to_string))
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| ours.label().to_string());
                placeholders.push(Placeholder {
                    kind: ours,
                    frame,
                    prompt,
                    size: defaults
                        .and_then(|d| d.attribute("sz"))
                        .and_then(|sz| sz.parse_finite::<f32>().ok())
                        .map(|sz| sz / 100.),
                    color: defaults.and_then(|d| self.fill_color(d)),
                    bold: defaults.and_then(|d| d.attribute("b")).map(|b| b == "1"),
                    align: level.and_then(|l| l.attribute("algn")).map(align_of),
                    anchor: anchor.unwrap_or(Anchor::Top),
                    autofit: Autofit::Shrink,
                    bullets: matches!(kind.as_str(), "body" | "obj")
                        && level.and_then(|l| child(l, "buNone")).is_none()
                        && !matches!(
                            ours,
                            PlaceholderKind::Subtitle
                                | PlaceholderKind::Heading
                                | PlaceholderKind::Heading2
                                | PlaceholderKind::Caption
                        ),
                    ph: Some((
                        if kind == "obj" {
                            String::new()
                        } else {
                            kind.clone()
                        },
                        idx,
                    )),
                });
            } else {
                place.ours = None;
            }
            places.push(place);
        }
        // Slides find their places in the layout, then the master.
        for inherited in master {
            if !places
                .iter()
                .any(|place| place.kind == inherited.kind && place.idx == inherited.idx)
            {
                places.push(inherited.clone());
            }
        }
        let layout = Layout {
            id: format!("layout{number}"),
            name,
            background,
            decorations,
            placeholders,
            theme_decorations: root.attribute("showMasterSp") != Some("0"),
            part: Some(part_name.to_string()),
        };
        Ok((layout, places))
    }

    fn read_slide(
        &mut self,
        part_name: &str,
        all_places: &HashMap<String, Vec<Place>>,
    ) -> Result<Slide> {
        let text = self.text(part_name).context("missing")?;
        let document = Document::parse(&text)?;
        let root = document.root_element();
        let rels = rels_of(self.files, part_name);
        let layout_part = rels
            .iter()
            .find(|(_, kind, ..)| kind.ends_with("/slideLayout"))
            .map(|(_, _, target, _)| target.clone())
            .unwrap_or_default();
        let layout = self
            .deck
            .theme
            .layouts
            .iter()
            .find(|layout| layout.part.as_deref() == Some(layout_part.as_str()))
            .map(|layout| layout.id.clone())
            .or_else(|| {
                self.deck
                    .theme
                    .layouts
                    .first()
                    .map(|layout| layout.id.clone())
            })
            .unwrap_or_else(|| "blank".into());
        let places = all_places.get(&layout_part).cloned().unwrap_or_default();
        let part = Part {
            name: part_name.to_string(),
            rels: rels.clone(),
            places: &places,
        };
        self.shape_ids.clear();
        self.connectors.clear();
        let tree = path(root, &["cSld", "spTree"]).context("no shapes")?;
        let mut elements = self.shapes(tree, &part, false, (0., 0.));
        self.join_connectors(&mut elements);
        let mut slide = Slide {
            id: self.deck.new_id("s"),
            layout,
            background: path(root, &["cSld", "bg"]).and_then(|bg| self.background(bg)),
            elements,
            hidden: root.attribute("show") == Some("0"),
            ..Slide::default()
        };
        self.read_transition(root, &mut slide);
        self.read_animations(root, &mut slide);
        // Notes.
        if let Some((_, _, notes, _)) = rels
            .iter()
            .find(|(_, kind, ..)| kind.ends_with("/notesSlide"))
            && let Some(text) = self.text(notes)
            && let Ok(notes) = Document::parse(&text)
        {
            slide.notes = notes
                .descendants()
                .filter(|node| node.tag_name().name() == "sp")
                .find(|sp| {
                    sp.descendants()
                        .find(|node| node.tag_name().name() == "ph")
                        .is_some_and(|ph| ph.attribute("type") == Some("body"))
                })
                .and_then(|sp| child(sp, "txBody"))
                .map(paragraphs_text)
                .unwrap_or_default();
        }
        self.read_comments(&rels, &mut slide);
        Ok(slide)
    }

    fn read_transition(&mut self, root: Node, slide: &mut Slide) {
        // Newer transitions sit in a choice, with an older one to fall
        // back on.
        let transition = root
            .descendants()
            .filter(|node| node.tag_name().name() == "transition")
            .find(|node| node.ancestors().all(|up| up.tag_name().name() != "Choice"));
        let Some(transition) = transition else {
            return;
        };
        let seconds = transition
            .attributes()
            .find(|attribute| attribute.name() == "dur")
            .and_then(|attribute| attribute.value().parse_finite::<f32>().ok())
            .map(|ms| ms / 1000.)
            .unwrap_or(match transition.attribute("spd") {
                Some("slow") => 1.,
                Some("fast") => 0.4,
                _ => 0.7,
            });
        let Some(effect) = transition.children().find(Node::is_element) else {
            return;
        };
        let dir = effect.attribute("dir").unwrap_or("l");
        // Push, cover and wipe say where the slide moves to (a wipe "l"
        // goes leftwards: PowerPoint's "From Right").
        let towards = |dir: &str| match dir {
            "r" => Direction::Left,
            "l" => Direction::Right,
            "d" => Direction::Top,
            "u" => Direction::Bottom,
            _ => Direction::Right,
        };
        let (kind, from) = match effect.tag_name().name() {
            "fade" | "cut" => (Transition::Fade, Direction::Right),
            "push" => (Transition::Push, towards(dir)),
            "cover" | "pull" => (Transition::Cover, towards(dir)),
            "wipe" | "split" | "strips" | "wheel" | "wedge" | "circle" | "diamond" | "plus" => {
                (Transition::Wipe, towards(dir))
            }
            "zoom" | "newsflash" => (Transition::Zoom, Direction::Right),
            "dissolve" | "randomBar" | "checker" | "blinds" => {
                (Transition::Dissolve, Direction::Right)
            }
            _ => (Transition::Fade, Direction::Right),
        };
        slide.transition = kind;
        slide.transition_from = from;
        slide.transition_seconds = seconds.clamp(0.1, 5.);
    }

    /// Entrance animations, in the order they play.
    fn read_animations(&mut self, root: Node, slide: &mut Slide) {
        let Some(timing) = child(root, "timing") else {
            return;
        };
        for node in timing
            .descendants()
            .filter(|node| node.tag_name().name() == "cTn")
        {
            if node.attribute("presetClass") != Some("entr") {
                continue;
            }
            let Some(shape) = node
                .descendants()
                .find(|node| node.tag_name().name() == "spTgt")
                .and_then(|target| target.attribute("spid"))
            else {
                continue;
            };
            let Some(id) = self.shape_ids.get(shape).cloned() else {
                continue;
            };
            if slide.animations.iter().any(|animation| animation.id == id)
                || !slide.elements.iter().any(|element| element.id == id)
            {
                continue;
            }
            let effect = match node
                .attribute("presetID")
                .and_then(|id| id.parse::<u32>().ok())
            {
                Some(1) => Effect::Appear,
                Some(2) | Some(42) | Some(47) => Effect::FlyIn,
                Some(22) | Some(18) | Some(16) => Effect::Wipe,
                Some(53) | Some(21) | Some(23) | Some(31) => Effect::Zoom,
                _ => Effect::Fade,
            };
            let from = match node
                .attribute("presetSubtype")
                .and_then(|sub| sub.parse::<u32>().ok())
            {
                Some(1) => Direction::Top,
                Some(2) => Direction::Right,
                Some(8) => Direction::Left,
                _ => Direction::Bottom,
            };
            let start = match node.attribute("nodeType") {
                Some("withEffect") => Start::WithPrevious,
                Some("afterEffect") => Start::AfterPrevious,
                _ => Start::OnClick,
            };
            let seconds = node
                .descendants()
                .filter(|inner| inner.tag_name().name() == "cTn" && *inner != node)
                .filter_map(|inner| {
                    inner
                        .attribute("dur")
                        .and_then(|dur| dur.parse_finite::<f32>().ok())
                })
                .fold(0f32, f32::max)
                / 1000.;
            let delay = path(node, &["stCondLst", "cond"])
                .and_then(|cond| cond.attribute("delay"))
                .and_then(|delay| delay.parse_finite::<f32>().ok())
                .unwrap_or(0.)
                / 1000.;
            slide.animations.push(Animation {
                id,
                effect,
                from,
                start,
                seconds: if seconds > 0.01 { seconds } else { 0.5 },
                delay,
            });
        }
    }

    fn read_comments(&mut self, rels: &[(String, String, String, bool)], slide: &mut Slide) {
        // Who wrote them.
        let mut authors: HashMap<String, String> = HashMap::new();
        for part in ["ppt/commentAuthors.xml", "ppt/authors.xml"] {
            if let Some(text) = self.text(part)
                && let Ok(document) = Document::parse(&text)
            {
                for author in document
                    .descendants()
                    .filter(|node| matches!(node.tag_name().name(), "cmAuthor" | "author"))
                {
                    if let (Some(id), Some(name)) =
                        (author.attribute("id"), author.attribute("name"))
                    {
                        authors.insert(id.to_string(), name.to_string());
                    }
                }
            }
        }
        for (_, kind, target, _) in rels {
            if !kind.ends_with("/comments")
                && !kind.ends_with("/comments2018")
                && !target.contains("comments/")
            {
                continue;
            }
            let Some(text) = self.text(target) else {
                continue;
            };
            let Ok(document) = Document::parse(&text) else {
                continue;
            };
            // Where each (author, number) went: a comment, or a reply to one.
            let mut threads: HashMap<(String, String), usize> = HashMap::new();
            for cm in document
                .descendants()
                .filter(|node| node.tag_name().name() == "cm")
            {
                let author = cm
                    .attribute("authorId")
                    .and_then(|id| authors.get(id).cloned())
                    .unwrap_or_else(|| "Someone".into());
                let text = match child(cm, "text") {
                    Some(text) => text.text().unwrap_or_default().to_string(),
                    None => child(cm, "txBody").map(paragraphs_text).unwrap_or_default(),
                };
                let (x, y) = child(cm, "pos")
                    .map(|pos| {
                        (
                            pos.attribute("x")
                                .and_then(|x| x.parse_finite::<f32>().ok())
                                .unwrap_or(0.)
                                / 8.,
                            pos.attribute("y")
                                .and_then(|y| y.parse_finite::<f32>().ok())
                                .unwrap_or(0.)
                                / 8.,
                        )
                    })
                    .unwrap_or((24., 24.));
                let mut resolved = cm.attribute("status") == Some("resolved");
                let text = match text.strip_prefix(super::pptx::RESOLVED) {
                    Some(rest) => {
                        resolved = true;
                        rest.to_string()
                    }
                    None => text,
                };
                let at = cm
                    .attribute("dt")
                    .or_else(|| cm.attribute("created"))
                    .and_then(parse_time)
                    .unwrap_or_else(super::ops::now);
                let key = (
                    cm.attribute("authorId").unwrap_or_default().to_string(),
                    cm.attribute("idx").unwrap_or_default().to_string(),
                );
                // A legacy reply: under the comment it answers.
                let parent = cm
                    .descendants()
                    .find(|node| node.tag_name().name() == "parentCm")
                    .map(|parent| {
                        (
                            parent.attribute("authorId").unwrap_or_default().to_string(),
                            parent.attribute("idx").unwrap_or_default().to_string(),
                        )
                    })
                    .and_then(|parent| threads.get(&parent).copied());
                if let Some(at_index) = parent
                    && let Some(found) = slide.comments.get_mut(at_index)
                {
                    found.replies.push(Reply { author, text, at });
                    threads.insert(key, at_index);
                    continue;
                }
                let mut comment = Comment {
                    id: self.deck.new_id("c"),
                    author,
                    text,
                    x: x.clamp(0., self.deck.size.width),
                    y: y.clamp(0., self.deck.size.height),
                    element: None,
                    resolved,
                    at,
                    replies: Vec::new(),
                };
                // Modern comments carry their replies.
                for reply in cm
                    .descendants()
                    .filter(|node| node.tag_name().name() == "reply")
                {
                    comment.replies.push(Reply {
                        author: reply
                            .attribute("authorId")
                            .and_then(|id| authors.get(id).cloned())
                            .unwrap_or_else(|| "Someone".into()),
                        text: child(reply, "txBody")
                            .map(paragraphs_text)
                            .unwrap_or_default(),
                        at: super::ops::now(),
                    });
                }
                if !comment.text.trim().is_empty() {
                    threads.insert(key, slide.comments.len());
                    slide.comments.push(comment);
                }
            }
        }
    }

    /// The shapes of a tree. `layout` reads a master's or layout's:
    /// placeholders there are places, not elements. `offset` moves them
    /// (SmartArt drawings are relative to their frame).
    fn shapes(
        &mut self,
        tree: Node,
        part: &Part,
        layout: bool,
        offset: (f32, f32),
    ) -> Vec<Element> {
        let mut out = Vec::new();
        for node in tree.children().filter(Node::is_element) {
            let name = node.tag_name().name();
            let result = match name {
                "sp" => self.sp(node, part, layout, offset),
                "pic" => self.pic(node, part, offset),
                "graphicFrame" => self.graphic_frame(node, part, offset),
                "grpSp" => self.group(node, part, layout, offset),
                "cxnSp" => self.connector(node, part, offset),
                "AlternateContent" => {
                    // What PowerPoint offers older readers, else its choice.
                    let pick = child(node, "Fallback").or_else(|| child(node, "Choice"));
                    if let Some(pick) = pick {
                        out.extend(self.shapes(pick, part, layout, offset));
                    }
                    continue;
                }
                "contentPart" => self.raw(node, part, offset, "Ink"),
                _ => continue,
            };
            match result {
                Ok(Some(element)) => out.push(element),
                Ok(None) => {}
                Err(err) => self.note(format!(
                    "a shape in {} could not be read: {err:#}",
                    part.name
                )),
            }
        }
        out
    }

    fn remember(&mut self, node: Node, element: &Element) {
        if let Some(id) = node
            .descendants()
            .find(|node| node.tag_name().name() == "cNvPr")
            .and_then(|c_nv_pr| c_nv_pr.attribute("id"))
        {
            self.shape_ids.insert(id.to_string(), element.id.clone());
        }
    }

    fn base(&mut self, node: Node, frame: Frame) -> Element {
        let c_nv_pr = node
            .descendants()
            .find(|node| node.tag_name().name() == "cNvPr");
        Element {
            id: self.deck.new_id("e"),
            name: c_nv_pr
                .and_then(|c| c.attribute("name"))
                .unwrap_or_default()
                .to_string(),
            frame,
            ..Element::default()
        }
    }

    fn sp(
        &mut self,
        node: Node,
        part: &Part,
        layout: bool,
        offset: (f32, f32),
    ) -> Result<Option<Element>> {
        if is_hidden(node) {
            return Ok(None);
        }
        let ph = node
            .descendants()
            .find(|node| node.tag_name().name() == "ph");
        // A layout's placeholders are places (read apart).
        if layout && ph.is_some() {
            return Ok(None);
        }
        let sp_pr = child(node, "spPr");
        let place = ph.and_then(|ph| {
            let kind = ph.attribute("type").unwrap_or("obj");
            let idx: u32 = ph
                .attribute("idx")
                .and_then(|idx| idx.parse().ok())
                .unwrap_or(0);
            find_place(part.places, kind, idx)
        });
        let frame = sp_pr
            .and_then(|sp_pr| child(sp_pr, "xfrm"))
            .map(|xfrm| frame_of(xfrm, offset))
            .or_else(|| place.as_ref().and_then(|place| place.frame));
        let Some(frame) = frame else {
            // A placeholder of a kind the layout does not have.
            if ph.is_some() {
                self.note("a placeholder without a place was left out");
            }
            return Ok(None);
        };
        let style = child(node, "style");
        let geometry = sp_pr.and_then(|sp_pr| child(sp_pr, "prstGeom"));
        let preset = geometry.and_then(|g| g.attribute("prst")).unwrap_or("rect");
        let custom = sp_pr.and_then(|sp_pr| child(sp_pr, "custGeom"));
        let fill = sp_pr
            .and_then(|sp_pr| self.fill(sp_pr, part))
            .or_else(|| style.and_then(|style| self.style_fill(style)))
            .unwrap_or(Fill::None);
        let line = sp_pr
            .and_then(|sp_pr| child(sp_pr, "ln"))
            .map(|ln| self.line(ln))
            .or_else(|| style.and_then(|style| self.style_line(style)))
            .unwrap_or_default();
        let shadow = sp_pr.and_then(|sp_pr| self.shadow(sp_pr));
        let text_color = style
            .and_then(|style| child(style, "fontRef"))
            .and_then(|font| self.color(font));
        let mut body =
            child(node, "txBody").map(|body| self.text_body(body, part, text_color.clone()));
        // A placeholder's text sits where its layout's (or master's) does,
        // unless it says otherwise.
        if let (Some(body), Some(inherited)) =
            (body.as_mut(), place.as_ref().and_then(|place| place.anchor))
            && path(node, &["txBody", "bodyPr"])
                .and_then(|pr| pr.attribute("anchor"))
                .is_none()
        {
            body.anchor = inherited;
        }
        let mut element = self.base(node, frame);
        let is_text_box = node
            .descendants()
            .find(|node| node.tag_name().name() == "cNvSpPr")
            .is_some_and(|c| c.attribute("txBox") == Some("1"));
        let plain_box =
            matches!(fill, Fill::None) && line.width <= 0. && preset == "rect" && custom.is_none();
        if let Some(place) = place.as_ref().and_then(|place| place.ours) {
            element.placeholder = Some(place);
        }
        element.kind =
            if element.placeholder.is_some() || ((is_text_box || plain_box) && body.is_some()) {
                Kind::Text(TextBox {
                    text: body.unwrap_or_default(),
                    fill,
                    line,
                })
            } else {
                let (geometry, path) = match custom {
                    Some(custom) => (Geometry::Path, custom_path(custom, frame.w, frame.h)),
                    None => (nearest_geometry(preset), None),
                };
                let adjust: Vec<(String, String)> = sp_pr
                    .and_then(|sp_pr| path_of(sp_pr, &["prstGeom", "avLst"]))
                    .into_iter()
                    .flat_map(|list| children(list, "gd"))
                    .map(|gd| {
                        (
                            gd.attribute("name").unwrap_or_default().to_string(),
                            gd.attribute("fmla").unwrap_or_default().to_string(),
                        )
                    })
                    .collect();
                let radius = (geometry == Geometry::RoundRect).then(|| {
                    let adj = adjust
                        .iter()
                        .find(|(name, _)| name == "adj")
                        .and_then(|(_, fmla)| {
                            fmla.strip_prefix("val ")
                                .and_then(|value| value.trim().parse_finite::<f32>().ok())
                        })
                        .unwrap_or(16_667.);
                    adj / 100_000. * frame.w.min(frame.h)
                });
                Kind::Shape(Shape {
                    geometry,
                    radius,
                    path,
                    fill,
                    line,
                    shadow,
                    text: body,
                    connect: None,
                    preset: (custom.is_none()).then(|| (preset.to_string(), adjust)),
                })
            };
        self.remember(node, &element);
        Ok(Some(element))
    }

    fn pic(&mut self, node: Node, part: &Part, offset: (f32, f32)) -> Result<Option<Element>> {
        if is_hidden(node) {
            return Ok(None);
        }
        // A video or sound: kept, its poster shown.
        let media = node.descendants().any(|node| {
            matches!(
                node.tag_name().name(),
                "videoFile" | "audioFile" | "quickTimeFile" | "media"
            )
        });
        if media {
            return self.raw(node, part, offset, "Video");
        }
        let sp_pr = child(node, "spPr").context("no shape properties")?;
        let ph = node
            .descendants()
            .find(|node| node.tag_name().name() == "ph");
        let place = ph.and_then(|ph| {
            find_place(
                part.places,
                ph.attribute("type").unwrap_or("pic"),
                ph.attribute("idx")
                    .and_then(|idx| idx.parse().ok())
                    .unwrap_or(0),
            )
        });
        let frame = child(sp_pr, "xfrm")
            .map(|xfrm| frame_of(xfrm, offset))
            .or_else(|| place.as_ref().and_then(|place| place.frame))
            .context("no place")?;
        let blip_fill = child(node, "blipFill").context("no picture")?;
        let blip = child(blip_fill, "blip").context("no picture")?;
        let Some(target) = rel_attr(blip, "embed")
            .and_then(|id| part.rel(id))
            .map(|(_, _, target, _)| target.clone())
        else {
            return Ok(None);
        };
        // Pictures PilotHouse cannot draw (EMF, WMF) are kept as they are.
        let extension = target
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_lowercase())
            .unwrap_or_default();
        if matches!(extension.as_str(), "emf" | "wmf" | "tif" | "tiff") {
            return self.raw(node, part, offset, "Picture (EMF)");
        }
        let Some(media) = self.copy_media(&target) else {
            return Ok(None);
        };
        let crop = child(blip_fill, "srcRect")
            .map(|rect| {
                ["l", "t", "r", "b"].map(|side| {
                    rect.attribute(side)
                        .and_then(|v| v.parse_finite::<f32>().ok())
                        .unwrap_or(0.)
                        / 100_000.
                })
            })
            .unwrap_or([0.; 4]);
        let mut element = self.base(node, frame);
        let alt = node
            .descendants()
            .find(|node| node.tag_name().name() == "cNvPr")
            .and_then(|c| c.attribute("descr"))
            .unwrap_or_default()
            .to_string();
        let radius = child(sp_pr, "prstGeom")
            .filter(|geometry| geometry.attribute("prst") == Some("roundRect"))
            .map(|_| 0.1667 * frame.w.min(frame.h))
            .unwrap_or(0.);
        if let Some(place) = place.and_then(|place| place.ours) {
            element.placeholder = Some(place);
        }
        element.kind = Kind::Picture(Picture {
            media,
            // PowerPoint stretches the cropped picture to its frame.
            fit: Fit::Stretch,
            crop: crop.map(|side| side.clamp(-0.5, 0.95)),
            radius,
            line: child(sp_pr, "ln")
                .map(|ln| self.line(ln))
                .unwrap_or_default(),
            shadow: self.shadow(sp_pr),
            alt,
        });
        self.remember(node, &element);
        Ok(Some(element))
    }

    fn graphic_frame(
        &mut self,
        node: Node,
        part: &Part,
        offset: (f32, f32),
    ) -> Result<Option<Element>> {
        if is_hidden(node) {
            return Ok(None);
        }
        let frame = child(node, "xfrm")
            .map(|xfrm| frame_of(xfrm, offset))
            .context("no place")?;
        let data = path(node, &["graphic", "graphicData"]).context("no graphic")?;
        let uri = data.attribute("uri").unwrap_or_default();
        if let Some(table) = child(data, "tbl") {
            let mut element = self.base(node, frame);
            element.kind = Kind::Table(self.table(table, part));
            self.remember(node, &element);
            return Ok(Some(element));
        }
        if uri.ends_with("/chart") {
            let target = child(data, "chart")
                .and_then(|chart| rel_attr(chart, "id"))
                .and_then(|id| part.rel(id))
                .map(|(_, _, target, _)| target.clone());
            if let Some(chart) = target.and_then(|target| self.chart(&target)) {
                let mut element = self.base(node, frame);
                element.kind = Kind::Chart(chart);
                self.remember(node, &element);
                return Ok(Some(element));
            }
            return self.raw(node, part, offset, "Chart");
        }
        if uri.ends_with("/diagram") {
            if let Some(group) = self.smart_art(node, data, part, frame) {
                self.remember(node, &group);
                return Ok(Some(group));
            }
            return self.raw(node, part, offset, "SmartArt");
        }
        self.raw(
            node,
            part,
            offset,
            if uri.ends_with("/ole") {
                "Embedded object"
            } else {
                "PowerPoint object"
            },
        )
    }

    fn group(
        &mut self,
        node: Node,
        part: &Part,
        layout: bool,
        offset: (f32, f32),
    ) -> Result<Option<Element>> {
        if is_hidden(node) {
            return Ok(None);
        }
        let xfrm = path(node, &["grpSpPr", "xfrm"]).context("no place")?;
        let frame = frame_of(xfrm, offset);
        // Children are in the group's own space: placed on the slide.
        let ch_off = child(xfrm, "chOff")
            .map(|off| {
                (
                    pt(off.attribute("x").unwrap_or("0")),
                    pt(off.attribute("y").unwrap_or("0")),
                )
            })
            .unwrap_or((0., 0.));
        let ch_ext = child(xfrm, "chExt")
            .map(|ext| {
                (
                    pt(ext.attribute("cx").unwrap_or("0")),
                    pt(ext.attribute("cy").unwrap_or("0")),
                )
            })
            .unwrap_or((frame.w, frame.h));
        let scale = (
            if ch_ext.0 > 0. {
                frame.w / ch_ext.0
            } else {
                1.
            },
            if ch_ext.1 > 0. {
                frame.h / ch_ext.1
            } else {
                1.
            },
        );
        let mut children = self.shapes(node, part, layout, (0., 0.));
        for element in &mut children {
            place_child(element, ch_off, scale, (frame.x, frame.y));
        }
        if children.is_empty() {
            return Ok(None);
        }
        let mut element = self.base(node, frame);
        element.kind = Kind::Group(Group { children });
        self.remember(node, &element);
        Ok(Some(element))
    }

    fn connector(
        &mut self,
        node: Node,
        part: &Part,
        offset: (f32, f32),
    ) -> Result<Option<Element>> {
        let sp_pr = child(node, "spPr").context("no shape properties")?;
        let frame = child(sp_pr, "xfrm")
            .map(|xfrm| frame_of(xfrm, offset))
            .context("no place")?;
        let style = child(node, "style");
        let line = child(sp_pr, "ln")
            .map(|ln| self.line(ln))
            .or_else(|| style.and_then(|style| self.style_line(style)))
            .unwrap_or(Line {
                width: 1.,
                ..Line::default()
            });
        let mut element = self.base(node, frame);
        element.kind = Kind::Shape(Shape {
            geometry: Geometry::Line,
            fill: Fill::None,
            line,
            preset: child(sp_pr, "prstGeom")
                .and_then(|g| g.attribute("prst"))
                .map(|prst| (prst.to_string(), Vec::new())),
            ..Shape::default()
        });
        let end = |tag: &str| {
            node.descendants()
                .find(|node| node.tag_name().name() == tag)
                .and_then(|end| {
                    Some((
                        end.attribute("id")?.to_string(),
                        end.attribute("idx")?.parse::<u32>().ok()?,
                    ))
                })
        };
        let (start, finish) = (end("stCxn"), end("endCxn"));
        if start.is_some() || finish.is_some() {
            self.connectors.push((element.id.clone(), start, finish));
        }
        self.remember(node, &element);
        let _ = part;
        Ok(Some(element))
    }

    /// Connectors' ends on the elements they were fixed to.
    fn join_connectors(&mut self, elements: &mut [Element]) {
        let connectors = std::mem::take(&mut self.connectors);
        for (id, start, end) in connectors {
            let map = |end: Option<(String, u32)>| {
                end.and_then(|(shape, site)| {
                    let element = self.shape_ids.get(&shape)?.clone();
                    // PowerPoint counts a box's sites anticlockwise from the top.
                    let side = match site % 4 {
                        0 => 0,
                        1 => 3,
                        2 => 2,
                        _ => 1,
                    };
                    Some((element, side))
                })
            };
            let connect = Connect {
                start: map(start),
                end: map(end),
            };
            if let Some(element) = elements.iter_mut().find(|element| element.id == id)
                && let Kind::Shape(shape) = &mut element.kind
                && (connect.start.is_some() || connect.end.is_some())
            {
                shape.connect = Some(connect);
            }
        }
    }

    /// Something kept as it was: its XML, its files, a picture of it.
    fn raw(
        &mut self,
        node: Node,
        part: &Part,
        offset: (f32, f32),
        what: &str,
    ) -> Result<Option<Element>> {
        let frame = node
            .descendants()
            .find(|node| node.tag_name().name() == "xfrm")
            .map(|xfrm| frame_of(xfrm, offset))
            .context("no place")?;
        let document = node.document();
        let mut xml = document.input_text()[node.range()].to_string();
        // Its namespaces declared on it, as it is written into another slide.
        let mut declarations = String::new();
        for namespace in node.namespaces() {
            if let Some(prefix) = namespace.name()
                && !matches!(prefix, "a" | "r" | "p" | "xml")
                && !xml.contains(&format!("xmlns:{prefix}="))
                && xml.contains(&format!("{prefix}:"))
            {
                declarations.push_str(&format!(r#" xmlns:{prefix}="{}""#, namespace.uri()));
            }
        }
        if !declarations.is_empty()
            && let Some(at) = xml.find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        {
            xml.insert_str(at, &declarations);
        }
        // The files it names, copied in.
        let mut rels = Vec::new();
        let mut preview = None;
        for inner in node.descendants() {
            for attribute in inner
                .attributes()
                .filter(|attribute| attribute.namespace() == Some(REL_NS))
            {
                let id = attribute.value();
                if rels.iter().any(|rel: &RawRel| rel.id == id) {
                    continue;
                }
                let Some((_, kind, target, external)) = part.rel(id).cloned() else {
                    continue;
                };
                if external {
                    rels.push(RawRel {
                        id: id.into(),
                        kind,
                        target,
                        external: true,
                    });
                    continue;
                }
                let Some(media) = self.copy_media(&target) else {
                    continue;
                };
                if preview.is_none()
                    && kind.ends_with("/image")
                    && !matches!(media.rsplit_once('.').map(|(_, e)| e), Some("emf" | "wmf"))
                {
                    preview = Some(media.clone());
                }
                rels.push(RawRel {
                    id: id.into(),
                    kind,
                    target: media,
                    external: false,
                });
            }
        }
        let mut element = self.base(node, frame);
        element.kind = Kind::Raw(Raw {
            xml,
            rels,
            preview,
            what: what.to_string(),
        });
        self.note(format!(
            "{} kept as it was ({} shows it and writes it back but does not edit it)",
            what.to_lowercase(),
            ph_brand::NAME
        ));
        self.remember(node, &element);
        Ok(Some(element))
    }

    /// A part's file copied into the space's media: its name there.
    fn copy_media(&mut self, target: &str) -> Option<String> {
        if let Some(name) = self.media.get(target) {
            return Some(name.clone());
        }
        let bytes = self.files.get(target)?;
        let extension = target
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_lowercase())
            .unwrap_or_else(|| "bin".into());
        let name = match extension.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => {
                self.space.add_media_bytes(bytes, &extension).ok()?
            }
            // Other files go in as they are.
            _ => {
                let name = format!("{:016x}.{extension}", fingerprint(bytes));
                let path = self.space.media().join(&name);
                if !path.exists() {
                    std::fs::write(&path, bytes).ok()?;
                }
                name
            }
        };
        self.media.insert(target.to_string(), name.clone());
        Some(name)
    }

    fn table(&mut self, table: Node, part: &Part) -> Table {
        let widths: Vec<f32> = path(table, &["tblGrid"])
            .into_iter()
            .flat_map(|grid| children(grid, "gridCol"))
            .map(|col| pt(col.attribute("w").unwrap_or("0")).max(1.))
            .collect();
        let properties = child(table, "tblPr");
        let mut font_size = None;
        let mut rows = Vec::new();
        for tr in children(table, "tr") {
            let mut row = Vec::new();
            for tc in children(tr, "tc") {
                let text = child(tc, "txBody")
                    .map(|body| self.text_body(body, part, None))
                    .unwrap_or_default();
                if font_size.is_none() {
                    font_size = text
                        .paragraphs
                        .iter()
                        .flat_map(|p| &p.runs)
                        .find_map(|run| run.size);
                }
                let fill = child(tc, "tcPr").and_then(|pr| self.fill(pr, part));
                let span = match (
                    tc.attribute("gridSpan").and_then(|v| v.parse::<u16>().ok()),
                    tc.attribute("rowSpan").and_then(|v| v.parse::<u16>().ok()),
                ) {
                    (None, None) => None,
                    (columns, rows) => Some((columns.unwrap_or(1), rows.unwrap_or(1))),
                };
                row.push(Cell { text, fill, span });
            }
            rows.push(row);
        }
        Table {
            columns: widths,
            rows,
            header: properties.and_then(|p| p.attribute("firstRow")) == Some("1"),
            banded: properties.and_then(|p| p.attribute("bandRow")) == Some("1"),
            font_size: font_size.unwrap_or(14.),
            ..Table::default()
        }
    }

    fn chart(&mut self, target: &str) -> Option<Chart> {
        let text = self.text(target)?;
        let document = Document::parse(&text).ok()?;
        let plot = document
            .descendants()
            .find(|node| node.tag_name().name() == "plotArea")?;
        let kind_node = plot
            .children()
            .find(|node| node.tag_name().name().ends_with("Chart"))?;
        let kind = match kind_node.tag_name().name() {
            "barChart" | "bar3DChart" => {
                let horizontal =
                    child(kind_node, "barDir").and_then(|d| d.attribute("val")) == Some("bar");
                let stacked = child(kind_node, "grouping")
                    .and_then(|g| g.attribute("val"))
                    .is_some_and(|g| g.contains("tacked"));
                if horizontal {
                    ChartKind::Bar
                } else if stacked {
                    ChartKind::StackedColumn
                } else {
                    ChartKind::Column
                }
            }
            "lineChart" | "line3DChart" => ChartKind::Line,
            "areaChart" | "area3DChart" => ChartKind::Area,
            "pieChart" | "pie3DChart" | "ofPieChart" => ChartKind::Pie,
            "doughnutChart" => ChartKind::Doughnut,
            "scatterChart" | "bubbleChart" => ChartKind::Scatter,
            _ => return None,
        };
        let strings = |node: Option<Node>| -> Vec<String> {
            let Some(node) = node else {
                return Vec::new();
            };
            let mut points: Vec<(usize, String)> = node
                .descendants()
                .filter(|pt| pt.tag_name().name() == "pt")
                .filter_map(|pt| {
                    // Points past what a chart holds are left out (a crafted
                    // index would otherwise ask for a huge list).
                    let idx = pt
                        .attribute("idx")?
                        .parse::<usize>()
                        .ok()
                        .filter(|idx| *idx < 10_000)?;
                    Some((idx, child(pt, "v")?.text().unwrap_or_default().to_string()))
                })
                .collect();
            points.sort_by_key(|(idx, _)| *idx);
            let count = node
                .descendants()
                .find(|n| n.tag_name().name() == "ptCount")
                .and_then(|n| n.attribute("val"))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(points.len())
                .min(10_000);
            let mut out =
                vec![String::new(); count.max(points.last().map_or(0, |(idx, _)| idx + 1))];
            for (idx, value) in points {
                if let Some(slot) = out.get_mut(idx) {
                    *slot = value;
                }
            }
            out
        };
        let numbers = |node: Option<Node>| -> Vec<f64> {
            strings(node)
                .iter()
                .map(|text| text.trim().parse_finite::<f64>().unwrap_or(0.))
                .collect()
        };
        let mut categories = Vec::new();
        let mut series = Vec::new();
        for ser in children(kind_node, "ser") {
            let name = child(ser, "tx")
                .map(|tx| {
                    strings(Some(tx)).into_iter().next().unwrap_or_else(|| {
                        tx.descendants()
                            .find(|n| n.tag_name().name() == "v")
                            .and_then(|v| v.text())
                            .unwrap_or_default()
                            .to_string()
                    })
                })
                .unwrap_or_default();
            let color = path_of(ser, &["spPr", "solidFill"])
                .or_else(|| path_of(ser, &["spPr", "ln", "solidFill"]))
                .and_then(|fill| self.color(fill));
            if kind == ChartKind::Scatter {
                series.push(Series {
                    name,
                    values: numbers(child(ser, "yVal")),
                    x: numbers(child(ser, "xVal")),
                    color,
                });
            } else {
                if categories.is_empty() {
                    categories = strings(child(ser, "cat"));
                }
                series.push(Series {
                    name,
                    values: numbers(child(ser, "val")),
                    x: Vec::new(),
                    color,
                });
            }
        }
        let title = document
            .descendants()
            .find(|node| {
                node.tag_name().name() == "title"
                    && node
                        .parent()
                        .is_some_and(|p| p.tag_name().name() == "chart")
            })
            .map(|title| {
                title
                    .descendants()
                    .filter(|node| node.tag_name().name() == "t")
                    .filter_map(|node| node.text())
                    .collect::<String>()
            })
            .unwrap_or_default();
        let labels = kind_node
            .descendants()
            .find(|node| node.tag_name().name() == "showVal")
            .is_some_and(|node| node.attribute("val") == Some("1"));
        Some(Chart {
            chart: kind,
            title,
            categories,
            series,
            legend: document
                .descendants()
                .any(|node| node.tag_name().name() == "legend"),
            labels,
            ..Chart::default()
        })
    }

    /// SmartArt as the shapes PowerPoint drew it with (its drawing part).
    fn smart_art(&mut self, node: Node, data: Node, part: &Part, frame: Frame) -> Option<Element> {
        // The drawing's relationship is named in the data part.
        let data_part = child(data, "relIds")
            .and_then(|ids| rel_attr(ids, "dm"))
            .and_then(|id| part.rel(id))
            .map(|(_, _, target, _)| target.clone());
        let drawing_id = data_part.as_deref().and_then(|data_part| {
            let text = self.text(data_part)?;
            let document = Document::parse(&text).ok()?;
            document
                .descendants()
                .find(|node| node.tag_name().name() == "dataModelExt")
                .and_then(|ext| ext.attribute("relId"))
                .map(str::to_string)
        });
        let drawing = drawing_id
            .and_then(|id| part.rel(&id).map(|(_, _, target, _)| target.clone()))
            .or_else(|| {
                part.rels
                    .iter()
                    .find(|(_, kind, ..)| kind.ends_with("/diagramDrawing"))
                    .map(|(_, _, target, _)| target.clone())
            })?;
        let text = self.text(&drawing)?;
        let document = Document::parse(&text).ok()?;
        let tree = document
            .descendants()
            .find(|node| node.tag_name().name() == "spTree")?;
        let drawing_rels = rels_of(self.files, &drawing);
        let drawing_part = Part {
            name: drawing.clone(),
            rels: drawing_rels,
            places: &[],
        };
        let children = self.shapes(tree, &drawing_part, false, (frame.x, frame.y));
        if children.is_empty() {
            return None;
        }
        let mut group = self.base(node, frame);
        group.kind = Kind::Group(Group { children });
        Some(group)
    }

    fn background(&mut self, bg: Node) -> Option<Fill> {
        if let Some(properties) = child(bg, "bgPr") {
            let part = Part {
                name: String::new(),
                rels: Vec::new(),
                places: &[],
            };
            return self.fill(properties, &part);
        }
        // A theme background style: its colour.
        child(bg, "bgRef")
            .and_then(|reference| self.color(reference))
            .map(|color| Fill::Solid { color, opacity: 1. })
    }

    /// A fill among `node`'s children, if it has one.
    fn fill(&mut self, node: Node, part: &Part) -> Option<Fill> {
        for fill in node.children().filter(Node::is_element) {
            match fill.tag_name().name() {
                "noFill" => return Some(Fill::None),
                "solidFill" => {
                    let color = self.color(fill)?;
                    return Some(Fill::Solid { color, opacity: 1. });
                }
                "gradFill" => {
                    let stops: Vec<Stop> = fill
                        .descendants()
                        .filter(|node| node.tag_name().name() == "gs")
                        .filter_map(|gs| {
                            Some(Stop {
                                at: gs.attribute("pos")?.parse_finite::<f32>().ok()? / 100_000.,
                                color: self.color(gs)?,
                            })
                        })
                        .collect();
                    if stops.is_empty() {
                        continue;
                    }
                    let angle = child(fill, "lin")
                        .and_then(|lin| lin.attribute("ang"))
                        .and_then(|ang| ang.parse_finite::<f32>().ok())
                        .map(|ang| ang / 60_000.)
                        .unwrap_or(90.);
                    return Some(Fill::Gradient {
                        stops,
                        angle,
                        radial: child(fill, "path").is_some(),
                    });
                }
                "blipFill" => {
                    let target = child(fill, "blip")
                        .and_then(|blip| rel_attr(blip, "embed"))
                        .and_then(|id| part.rel(id))
                        .map(|(_, _, target, _)| target.clone());
                    if let Some(media) = target.and_then(|target| self.copy_media(&target)) {
                        return Some(Fill::Picture {
                            media,
                            fit: Fit::Cover,
                        });
                    }
                }
                "pattFill" => {
                    if let Some(color) = child(fill, "fgClr").and_then(|fg| self.color(fg)) {
                        return Some(Fill::Solid { color, opacity: 1. });
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The colour among `node`'s children (`srgbClr`, `schemeClr` …), with
    /// its lighter, darker and see-through changes.
    fn color(&self, node: Node) -> Option<Color> {
        let color = node.children().find(|child| {
            matches!(
                child.tag_name().name(),
                "srgbClr" | "schemeClr" | "sysClr" | "prstClr" | "scrgbClr" | "hslClr"
            )
        })?;
        let value = |name: &str| {
            color
                .children()
                .find(|child| child.tag_name().name() == name)
                .and_then(|child| child.attribute("val"))
                .and_then(|val| val.parse_finite::<f32>().ok())
                .map(|val| val / 100_000.)
        };
        let lum_mod = value("lumMod");
        let lum_off = value("lumOff");
        let tint = value("tint");
        let shade = value("shade");
        let alpha = value("alpha");
        let base = match color.tag_name().name() {
            "schemeClr" => {
                let name = color.attribute("val")?;
                let slot = self
                    .clr_map
                    .get(name)
                    .cloned()
                    .or_else(|| slot_name(name).map(str::to_string))
                    .or_else(|| match name {
                        "tx1" => Some("dark1".into()),
                        "bg1" => Some("light1".into()),
                        "tx2" => Some("dark2".into()),
                        "bg2" => Some("light2".into()),
                        _ => None,
                    })
                    // A style's placeholder colour: its accent.
                    .unwrap_or_else(|| "accent1".into());
                let change = match (lum_mod, lum_off, tint, shade) {
                    (Some(lum_mod), Some(lum_off), _, _) if lum_off > 0. => {
                        (lum_off.max(1. - lum_mod) * 100.).round() as i32
                    }
                    (Some(lum_mod), _, _, _) if lum_mod < 1. => {
                        -((1. - lum_mod) * 100.).round() as i32
                    }
                    (_, _, Some(tint), _) if tint < 1. => ((1. - tint) * 100.).round() as i32,
                    (_, _, _, Some(shade)) if shade < 1. => -((1. - shade) * 100.).round() as i32,
                    _ => 0,
                };
                let named = match change {
                    0 => slot,
                    change if change > 0 => format!("{slot}+{}", change.min(95)),
                    change => format!("{slot}-{}", (-change).min(95)),
                };
                if let Some(alpha) = alpha.filter(|alpha| *alpha < 0.999) {
                    let rgba = self.deck.theme.resolve(&Color(named));
                    return Some(Color(format!(
                        "#{}{:02X}",
                        rgba.hex(),
                        (alpha * 255.).round() as u8
                    )));
                }
                return Some(Color(named));
            }
            "srgbClr" => color.attribute("val")?.to_uppercase(),
            "sysClr" => color
                .attribute("lastClr")
                .unwrap_or("000000")
                .to_uppercase(),
            "prstClr" => preset_color(color.attribute("val").unwrap_or("black")).to_string(),
            "scrgbClr" => {
                let channel = |name: &str| {
                    let linear = color
                        .attribute(name)
                        .and_then(|v| v.parse_finite::<f32>().ok())
                        .unwrap_or(0.)
                        / 100_000.;
                    // Linear light to sRGB.
                    let srgb = if linear <= 0.003_130_8 {
                        linear * 12.92
                    } else {
                        1.055 * linear.powf(1. / 2.4) - 0.055
                    };
                    (srgb.clamp(0., 1.) * 255.).round() as u8
                };
                format!(
                    "{:02X}{:02X}{:02X}",
                    channel("r"),
                    channel("g"),
                    channel("b")
                )
            }
            _ => return None,
        };
        let mut rgba = Rgba::parse_hex(&format!("#{base}"))?;
        if let Some(lum_mod) = lum_mod {
            let off = lum_off.unwrap_or(0.);
            let change =
                |c: u8| ((c as f32 / 255. * lum_mod + off).clamp(0., 1.) * 255.).round() as u8;
            rgba = Rgba {
                r: change(rgba.r),
                g: change(rgba.g),
                b: change(rgba.b),
                a: rgba.a,
            };
        }
        let mut hex = format!("#{}", rgba.hex());
        if let Some(alpha) = alpha.filter(|alpha| *alpha < 0.999) {
            hex.push_str(&format!("{:02X}", (alpha * 255.).round() as u8));
        }
        Some(Color(hex))
    }

    /// The colour of a run's or style's fill (`solidFill` child).
    fn fill_color(&self, node: Node) -> Option<Color> {
        child(node, "solidFill").and_then(|fill| self.color(fill))
    }

    fn style_fill(&self, style: Node) -> Option<Fill> {
        let reference = child(style, "fillRef")?;
        if reference.attribute("idx") == Some("0") {
            return Some(Fill::None);
        }
        self.color(reference)
            .map(|color| Fill::Solid { color, opacity: 1. })
    }

    fn style_line(&self, style: Node) -> Option<Line> {
        let reference = child(style, "lnRef")?;
        let idx: u32 = reference.attribute("idx")?.parse().ok()?;
        if idx == 0 {
            return None;
        }
        Some(Line {
            color: self.color(reference),
            width: 0.75 * idx as f32,
            ..Line::default()
        })
    }

    fn line(&self, ln: Node) -> Line {
        let width = ln.attribute("w").map(pt).unwrap_or(0.75);
        if child(ln, "noFill").is_some() {
            return Line::default();
        }
        let dash = match child(ln, "prstDash").and_then(|dash| dash.attribute("val")) {
            Some("dash" | "lgDash" | "sysDash") => Dash::Dash,
            Some("dot" | "sysDot") => Dash::Dot,
            Some("dashDot" | "lgDashDot" | "sysDashDot" | "lgDashDotDot" | "sysDashDotDot") => {
                Dash::DashDot
            }
            _ => Dash::Solid,
        };
        let end = |tag: &str| {
            child(ln, tag)
                .and_then(|end| end.attribute("type"))
                .is_some_and(|kind| kind != "none")
        };
        Line {
            color: self.fill_color(ln),
            width: width.max(0.25),
            dash,
            head: end("headEnd"),
            tail: end("tailEnd"),
        }
    }

    fn shadow(&self, sp_pr: Node) -> Option<Shadow> {
        let shadow = path_of(sp_pr, &["effectLst", "outerShdw"])?;
        let color = self.color(shadow).unwrap_or(Color::from("#000000"));
        let rgba = self.deck.theme.resolve(&color);
        Some(Shadow {
            color: Color(format!("#{}", rgba.hex())),
            blur: shadow.attribute("blurRad").map(pt).unwrap_or(4.),
            distance: shadow.attribute("dist").map(pt).unwrap_or(3.),
            angle: shadow
                .attribute("dir")
                .and_then(|dir| dir.parse_finite::<f32>().ok())
                .map(|dir| dir / 60_000.)
                .unwrap_or(90.),
            opacity: rgba.a as f32 / 255.,
        })
    }

    fn text_body(&mut self, body: Node, part: &Part, color: Option<Color>) -> TextBody {
        let properties = child(body, "bodyPr");
        let inset = |name: &str, default: f32| {
            properties
                .and_then(|pr| pr.attribute(name))
                .map(pt)
                .unwrap_or(default)
        };
        let mut text = TextBody {
            paragraphs: Vec::new(),
            anchor: properties
                .and_then(|pr| pr.attribute("anchor"))
                .map(anchor_of)
                .unwrap_or(Anchor::Top),
            insets: [
                inset("lIns", 7.2),
                inset("tIns", 3.6),
                inset("rIns", 7.2),
                inset("bIns", 3.6),
            ],
            autofit: match properties {
                Some(pr) if child(pr, "normAutofit").is_some() => Autofit::Shrink,
                Some(pr) if child(pr, "spAutoFit").is_some() => Autofit::Grow,
                _ => Autofit::None,
            },
            wrap: properties.and_then(|pr| pr.attribute("wrap")) != Some("none"),
        };
        for p in children(body, "p") {
            let pr = child(p, "pPr");
            let mut paragraph = Paragraph {
                runs: Vec::new(),
                align: pr.and_then(|pr| pr.attribute("algn")).map(align_of),
                level: pr
                    .and_then(|pr| pr.attribute("lvl"))
                    .and_then(|l| l.parse::<u8>().ok())
                    .unwrap_or(0)
                    .min(8),
                bullet: pr.and_then(|pr| {
                    if child(pr, "buNone").is_some() {
                        Some(Bullet::None)
                    } else if let Some(char) =
                        child(pr, "buChar").and_then(|bu| bu.attribute("char"))
                    {
                        Some(Bullet::Char(char.to_string()))
                    } else if child(pr, "buAutoNum").is_some() {
                        Some(Bullet::Number)
                    } else {
                        None
                    }
                }),
                space_before: pr
                    .and_then(|pr| path_of(pr, &["spcBef", "spcPts"]))
                    .and_then(|pts| pts.attribute("val"))
                    .and_then(|val| val.parse_finite::<f32>().ok())
                    .map(|val| val / 100.),
                space_after: pr
                    .and_then(|pr| path_of(pr, &["spcAft", "spcPts"]))
                    .and_then(|pts| pts.attribute("val"))
                    .and_then(|val| val.parse_finite::<f32>().ok())
                    .map(|val| val / 100.),
                line_spacing: pr
                    .and_then(|pr| path_of(pr, &["lnSpc", "spcPct"]))
                    .and_then(|pct| pct.attribute("val"))
                    .and_then(|val| val.parse_finite::<f32>().ok())
                    .map(|val| (val / 100_000.).clamp(0.5, 4.)),
            };
            for node in p.children().filter(Node::is_element) {
                match node.tag_name().name() {
                    "r" | "fld" => {
                        let content = child(node, "t")
                            .and_then(|t| t.text())
                            .unwrap_or_default()
                            .to_string();
                        let run = self.run(child(node, "rPr"), content, part, color.clone());
                        paragraph.runs.push(run);
                    }
                    // A line break within the paragraph.
                    "br" => match paragraph.runs.last_mut() {
                        Some(last) => last.text.push('\n'),
                        None => {
                            let run =
                                self.run(child(node, "rPr"), "\n".into(), part, color.clone());
                            paragraph.runs.push(run);
                        }
                    },
                    _ => {}
                }
            }
            text.paragraphs.push(paragraph);
        }
        text
    }

    fn run(&mut self, pr: Option<Node>, text: String, part: &Part, color: Option<Color>) -> Run {
        let mut run = Run {
            text,
            color,
            ..Run::default()
        };
        let Some(pr) = pr else {
            return run;
        };
        run.size = pr
            .attribute("sz")
            .and_then(|sz| sz.parse_finite::<f32>().ok())
            .map(|sz| sz / 100.);
        run.bold = pr.attribute("b").map(|b| b == "1" || b == "true");
        run.italic = pr.attribute("i").map(|i| i == "1" || i == "true");
        run.underline = pr.attribute("u").map(|u| u != "none");
        run.strike = pr.attribute("strike").map(|s| s != "noStrike");
        run.baseline = pr
            .attribute("baseline")
            .and_then(|b| b.parse::<i32>().ok())
            .map(|b| (b / 1000).clamp(-100, 100) as i16)
            .filter(|b| *b != 0);
        if let Some(color) = self.fill_color(pr) {
            run.color = Some(color);
        }
        run.highlight = child(pr, "highlight").and_then(|h| self.color(h));
        if let Some(face) = child(pr, "latin").and_then(|latin| latin.attribute("typeface")) {
            run.font = Some(font_name(face));
        }
        run.link = child(pr, "hlinkClick")
            .and_then(|link| rel_attr(link, "id"))
            .and_then(|id| part.rel(id))
            .filter(|(.., external)| *external)
            .map(|(_, _, target, _)| target.clone());
        run
    }
}

/// `xfrm`'s frame, in points, moved by `offset`.
fn frame_of(xfrm: Node, offset: (f32, f32)) -> Frame {
    let off = child(xfrm, "off");
    let ext = child(xfrm, "ext");
    Frame {
        x: off.and_then(|o| o.attribute("x")).map(pt).unwrap_or(0.) + offset.0,
        y: off.and_then(|o| o.attribute("y")).map(pt).unwrap_or(0.) + offset.1,
        w: ext.and_then(|e| e.attribute("cx")).map(pt).unwrap_or(0.),
        h: ext.and_then(|e| e.attribute("cy")).map(pt).unwrap_or(0.),
        rotation: xfrm
            .attribute("rot")
            .and_then(|rot| rot.parse_finite::<f32>().ok())
            .map(|rot| (rot / 60_000.).rem_euclid(360.))
            .unwrap_or(0.),
        flip_h: xfrm.attribute("flipH") == Some("1"),
        flip_v: xfrm.attribute("flipV") == Some("1"),
    }
}

/// A group's child placed on the slide: from the group's own space.
fn place_child(element: &mut Element, ch_off: (f32, f32), scale: (f32, f32), origin: (f32, f32)) {
    let frame = &mut element.frame;
    frame.x = origin.0 + (frame.x - ch_off.0) * scale.0;
    frame.y = origin.1 + (frame.y - ch_off.1) * scale.1;
    frame.w *= scale.0;
    frame.h *= scale.1;
    if let Kind::Group(group) = &mut element.kind {
        for child in &mut group.children {
            // Already on its group's space: placed the same way.
            place_child(child, ch_off, scale, origin);
        }
    }
}

fn path_of<'a, 'input>(node: Node<'a, 'input>, names: &[&str]) -> Option<Node<'a, 'input>> {
    path(node, names)
}

fn is_hidden(node: Node) -> bool {
    node.descendants()
        .find(|node| node.tag_name().name() == "cNvPr")
        .is_some_and(|c| c.attribute("hidden") == Some("1"))
}

/// The place a slide's placeholder inherits: by index, else by type.
fn find_place(places: &[Place], kind: &str, idx: u32) -> Option<Place> {
    let same_type = |place: &Place| {
        place.kind == kind
            || (matches!(kind, "title" | "ctrTitle")
                && matches!(place.kind.as_str(), "title" | "ctrTitle"))
            || (matches!(kind, "body" | "obj" | "subTitle")
                && matches!(place.kind.as_str(), "body" | "obj" | "subTitle")
                && idx != 0)
    };
    places
        .iter()
        .find(|place| idx != 0 && place.idx == idx)
        .or_else(|| {
            places
                .iter()
                .find(|place| same_type(place) && (idx == 0 || place.idx == idx))
        })
        .or_else(|| places.iter().find(|place| same_type(place)))
        .cloned()
}

/// A layout placeholder's place, from the master when it gives none.
fn inherited(master: &[Place], kind: &str, idx: u32) -> Option<Frame> {
    find_place(master, kind, idx).and_then(|place| place.frame)
}

/// Our kind for a PowerPoint placeholder type, given those taken.
fn our_kind(kind: &str, taken: &[PlaceholderKind]) -> Option<PlaceholderKind> {
    let free =
        |options: &[PlaceholderKind]| options.iter().copied().find(|kind| !taken.contains(kind));
    match kind {
        "title" | "ctrTitle" => free(&[PlaceholderKind::Title, PlaceholderKind::Statement]),
        "subTitle" => free(&[PlaceholderKind::Subtitle, PlaceholderKind::Caption]),
        "pic" => free(&[PlaceholderKind::Picture]),
        "ftr" => free(&[PlaceholderKind::Footer]),
        "sldNum" => free(&[PlaceholderKind::SlideNumber]),
        "dt" | "hdr" | "sldImg" => None,
        _ => free(&[
            PlaceholderKind::Body,
            PlaceholderKind::Body2,
            PlaceholderKind::Heading,
            PlaceholderKind::Heading2,
            PlaceholderKind::Caption,
            PlaceholderKind::Statement,
        ]),
    }
}

fn align_of(algn: &str) -> Align {
    match algn {
        "ctr" => Align::Center,
        "r" => Align::Right,
        "just" | "dist" => Align::Justify,
        _ => Align::Left,
    }
}

fn anchor_of(anchor: &str) -> Anchor {
    match anchor {
        "ctr" => Anchor::Middle,
        "b" => Anchor::Bottom,
        _ => Anchor::Top,
    }
}

/// A typeface as the deck names it (the theme's heading and body fonts).
fn font_name(face: &str) -> String {
    match face {
        "+mj-lt" | "+mj-ea" | "+mj-cs" => "heading".into(),
        "+mn-lt" | "+mn-ea" | "+mn-cs" => "body".into(),
        other => other.to_string(),
    }
}

/// A text body's words, a line a paragraph.
fn paragraphs_text(body: Node) -> String {
    children(body, "p")
        .map(|p| {
            p.descendants()
                .filter(|node| node.tag_name().name() == "t" || node.tag_name().name() == "br")
                .map(|node| {
                    if node.tag_name().name() == "br" {
                        "\n".to_string()
                    } else {
                        node.text().unwrap_or_default().to_string()
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// DrawingML's preset colours most used, as RGB.
fn preset_color(name: &str) -> &'static str {
    match name {
        "white" => "FFFFFF",
        "red" => "FF0000",
        "green" => "008000",
        "blue" => "0000FF",
        "yellow" => "FFFF00",
        "orange" => "FFA500",
        "gray" | "grey" => "808080",
        "darkGray" | "dkGray" => "A9A9A9",
        "lightGray" | "ltGray" => "D3D3D3",
        _ => "000000",
    }
}

/// A custom outline as SVG path data in the frame's box (`w` × `h`).
fn custom_path(custom: Node, w: f32, h: f32) -> Option<String> {
    use std::fmt::Write as _;
    let mut d = String::new();
    for path in custom.descendants().filter(|node| {
        node.tag_name().name() == "path"
            && node
                .parent()
                .is_some_and(|p| p.tag_name().name() == "pathLst")
    }) {
        let pw = path
            .attribute("w")
            .and_then(|v| v.parse_finite::<f32>().ok())
            .filter(|v| *v > 0.)
            .unwrap_or(w * EMU);
        let ph = path
            .attribute("h")
            .and_then(|v| v.parse_finite::<f32>().ok())
            .filter(|v| *v > 0.)
            .unwrap_or(h * EMU);
        let (sx, sy) = (w / pw, h / ph);
        let point = |node: Node| -> Option<(f32, f32)> {
            let x = node.attribute("x")?.parse_finite::<f32>().ok()?;
            let y = node.attribute("y")?.parse_finite::<f32>().ok()?;
            Some((x * sx, y * sy))
        };
        let mut at = (0f32, 0f32);
        for command in path.children().filter(Node::is_element) {
            let points: Vec<(f32, f32)> = children(command, "pt").filter_map(point).collect();
            match command.tag_name().name() {
                "moveTo" => {
                    if let Some(p) = points.first() {
                        let _ = write!(d, "M{} {} ", p.0, p.1);
                        at = *p;
                    }
                }
                "lnTo" => {
                    if let Some(p) = points.first() {
                        let _ = write!(d, "L{} {} ", p.0, p.1);
                        at = *p;
                    }
                }
                "cubicBezTo" if points.len() == 3 => {
                    let _ = write!(
                        d,
                        "C{} {} {} {} {} {} ",
                        points[0].0,
                        points[0].1,
                        points[1].0,
                        points[1].1,
                        points[2].0,
                        points[2].1
                    );
                    at = points[2];
                }
                "quadBezTo" if points.len() == 2 => {
                    let _ = write!(
                        d,
                        "Q{} {} {} {} ",
                        points[0].0, points[0].1, points[1].0, points[1].1
                    );
                    at = points[1];
                }
                "arcTo" => {
                    let number = |name: &str| {
                        command
                            .attribute(name)
                            .and_then(|v| v.parse_finite::<f32>().ok())
                            .unwrap_or(0.)
                    };
                    let (rx, ry) = (number("wR") * sx, number("hR") * sy);
                    let start = (number("stAng") / 60_000.).to_radians();
                    let sweep = (number("swAng") / 60_000.).to_radians();
                    if rx <= 0. || ry <= 0. {
                        continue;
                    }
                    // The ellipse's centre from where the arc starts.
                    let center = (at.0 - rx * start.cos(), at.1 - ry * start.sin());
                    let end = (
                        center.0 + rx * (start + sweep).cos(),
                        center.1 + ry * (start + sweep).sin(),
                    );
                    let large = (sweep.abs() > std::f32::consts::PI) as u8;
                    let clockwise = (sweep > 0.) as u8;
                    let _ = write!(d, "A{rx} {ry} 0 {large} {clockwise} {} {} ", end.0, end.1);
                    at = end;
                }
                "close" => d.push_str("Z "),
                _ => {}
            }
        }
    }
    (!d.is_empty()).then_some(d)
}

/// A fingerprint of `bytes` (FNV-1a), to name files by what they hold.
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The parts and title a file gives, for [`read`].
pub fn open(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    super::pptx::unzip(path)
}

/// Reads `PPTX_IN` into a scratch space in `PPTX_OUT`, draws each slide
/// and writes it back as PPTX:
/// `PPTX_IN=a.pptx PPTX_OUT=/dir cargo test -p ph-studio pptx_import -- --ignored --nocapture`.
#[cfg(test)]
mod look {
    #[test]
    #[ignore]
    fn pptx_import() {
        let (Ok(input), Ok(out)) = (std::env::var("PPTX_IN"), std::env::var("PPTX_OUT")) else {
            return;
        };
        let out = std::path::PathBuf::from(out);
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let started = std::time::Instant::now();
        let (space, notes) =
            super::super::space::import_pptx(&out, std::path::Path::new(&input)).unwrap();
        println!("read in {:?}; notes: {notes:?}", started.elapsed());
        let (_, deck) = super::super::space::Space::open(&space.path).unwrap();
        println!(
            "{} slides, {} layouts, size {:?}",
            deck.slides.len(),
            deck.theme.layouts.len(),
            deck.size
        );
        for (ix, slide) in deck.slides.iter().enumerate() {
            println!(
                "slide {} layout {} elements {} animations {} comments {}",
                ix + 1,
                slide.layout,
                slide.elements.len(),
                slide.animations.len(),
                slide.comments.len()
            );
            let image =
                super::super::raster::slide(&deck, ix, 1280, Some(&space.media()), false).unwrap();
            image
                .save(out.join(format!("slide{}.png", ix + 1)))
                .unwrap();
        }
        super::super::pptx::write(&deck, Some(&space.media()), &out.join("back.pptx")).unwrap();
        // A copy that is not written back as it was (theme changed).
        let mut changed = deck.clone();
        changed.source = None;
        super::super::pptx::write(&changed, Some(&space.media()), &out.join("ours.pptx")).unwrap();
    }
}

/// Seconds since 1970 from a W3C date and time (2026-10-31T09:30:00.000Z
/// or with an offset); the day and time as written.
fn parse_time(text: &str) -> Option<u64> {
    let (date, time) = text.split_once('T')?;
    let mut date = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let time: String = time
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ':')
        .collect();
    let mut time = time.split(':').map(|part| part.parse::<i64>().ok());
    let (hour, minute, second) = (
        time.next()??,
        time.next().flatten().unwrap_or(0),
        time.next().flatten().unwrap_or(0),
    );
    // Days from 1970 (Howard Hinnant's days_from_civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hour * 3600 + minute * 60 + second).ok()
}

#[cfg(test)]
mod time_tests {
    #[test]
    fn w3c_times() {
        assert_eq!(super::parse_time("1970-01-02T00:00:01.000Z"), Some(86_401));
        assert_eq!(
            super::parse_time("2026-10-31T09:30:00.000"),
            Some(1_793_439_000)
        );
    }
}
