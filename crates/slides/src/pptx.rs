//! A deck as a PowerPoint file.
//!
//! PowerPoint has no web page in it: it lays text out itself, in the fonts
//! of the computer it is opened on. So the file is made from where the
//! deck's page says everything is (`page/measure.js`): each piece of text
//! a text box at its place, with its font, size and colour; each fill and
//! outline a shape; each picture a picture; a table a table and a chart a
//! chart with its numbers, so both stay editable there. What PowerPoint
//! cannot say (a gradient, a blur, a drawing in SVG) comes as the nearest
//! thing it can, or as a picture.
//!
//! One pixel of a slide is 6,350 EMU (a 1920-pixel slide is PowerPoint's
//! own 13.33 inches) and half a point of type.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use serde_json::Value;

const XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const GROUP_HEADER: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;
const CLR_MAP: &str = r#"<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>"#;

/// EMU in a pixel of a slide.
const EMU: f64 = 6_350.;
/// A picture inside the file, at most (the same as one a deck takes).
const MAX_PICTURE: u64 = 200 * 1024 * 1024;

fn emu(px: f64) -> i64 {
    (px * EMU).round() as i64
}

fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .unwrap_or(0.)
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // What XML has no place for.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// `#rrggbb` as PowerPoint writes a colour, with how solid it is.
fn colour(hex: &str, alpha: f64) -> String {
    let hex = hex.trim_start_matches('#');
    let hex = if hex.len() >= 6
        && hex.is_char_boundary(6)
        && hex[..6].chars().all(|c| c.is_ascii_hexdigit())
    {
        hex[..6].to_ascii_uppercase()
    } else {
        "000000".into()
    };
    if alpha < 0.995 {
        format!(
            r#"<a:srgbClr val="{hex}"><a:alpha val="{}"/></a:srgbClr>"#,
            (alpha.clamp(0., 1.) * 100_000.).round() as i64
        )
    } else {
        format!(r#"<a:srgbClr val="{hex}"/>"#)
    }
}

fn solid(hex: &str, alpha: f64) -> String {
    format!("<a:solidFill>{}</a:solidFill>", colour(hex, alpha))
}

/// The font a run is written in: its own, or with `basic` one every
/// computer has in its stead.
fn font(family: &str, mono: bool, basic: bool) -> String {
    let lower = family.to_lowercase();
    let system = lower.is_empty()
        || lower.starts_with('-')
        || lower.starts_with('.')
        || lower.contains("system");
    if !basic && !system {
        return family.to_string();
    }
    if mono
        || lower.contains("mono")
        || lower.contains("courier")
        || lower.contains("menlo")
        || lower.contains("consol")
    {
        return "Courier New".into();
    }
    let serif = [
        "georgia",
        "palatino",
        "iowan",
        "times",
        "serif",
        "garamond",
        "baskerville",
        "cambria",
        "charter",
    ]
    .iter()
    .any(|name| lower.contains(name) && !lower.contains("sans"));
    if serif { "Georgia" } else { "Arial" }.into()
}

/// The relationships of one part.
#[derive(Default)]
struct Rels(Vec<(String, String, bool)>);

impl Rels {
    fn add(&mut self, kind: &str, target: &str) -> String {
        self.0.push((kind.into(), target.into(), false));
        format!("rId{}", self.0.len())
    }

    fn external(&mut self, kind: &str, target: &str) -> String {
        self.0.push((kind.into(), target.into(), true));
        format!("rId{}", self.0.len())
    }

    fn xml(&self) -> String {
        let mut xml = format!(
            r#"{XML}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#
        );
        for (index, (kind, target, external)) in self.0.iter().enumerate() {
            let _ = write!(
                xml,
                r#"<Relationship Id="rId{}" Type="{REL}/{kind}" Target="{}"{}/>"#,
                index + 1,
                escape(target),
                if *external {
                    r#" TargetMode="External""#
                } else {
                    ""
                }
            );
        }
        xml.push_str("</Relationships>");
        xml
    }
}

struct Package<'a> {
    site: &'a Path,
    basic: bool,
    parts: BTreeMap<String, Vec<u8>>,
    /// Part name and its content type.
    kinds: Vec<(String, &'static str)>,
    /// The extensions of the media inside.
    extensions: BTreeSet<String>,
    /// A picture's source and its part: one put on ten slides is in once.
    media: BTreeMap<String, String>,
    charts: usize,
}

impl Package<'_> {
    fn put(&mut self, name: &str, content: String) {
        self.parts.insert(name.to_string(), content.into_bytes());
    }

    fn kind(&mut self, part: &str, kind: &'static str) {
        self.kinds.push((part.to_string(), kind));
    }

    /// A picture into the file: one of the deck's own (`src`), or one the
    /// page drew (`data`, a PNG as a data address). Its part, seen from a
    /// slide.
    fn picture(&mut self, item: &Value) -> Result<String> {
        let source = match (text(&item["data"]), text(&item["src"])) {
            ("", "") => bail!("a picture without a source"),
            ("", src) => src.to_string(),
            (data, _) => data.to_string(),
        };
        if let Some(part) = self.media.get(&source) {
            return Ok(part.clone());
        }
        let (bytes, extension) =
            if let Some(encoded) = source.strip_prefix("data:image/png;base64,") {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .context("a picture the page drew could not be read")?;
                (bytes, "png".to_string())
            } else {
                let path = crate::deck::inside(source.split(['?', '#']).next().unwrap_or_default())
                    .with_context(|| format!("{source} is not a picture of the deck's"))?;
                let file = self.site.join(path);
                let size = file
                    .metadata()
                    .with_context(|| format!("{source} is not there"))?
                    .len();
                if size > MAX_PICTURE {
                    bail!("{source} is too large for a PowerPoint file");
                }
                let extension = path
                    .extension()
                    .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif") {
                    bail!("{source} is not a kind of picture PowerPoint shows");
                }
                (std::fs::read(&file)?, extension)
            };
        let part = format!("media/image{}.{extension}", self.media.len() + 1);
        self.parts.insert(format!("ppt/{part}"), bytes);
        self.extensions.insert(extension);
        self.media.insert(source, part.clone());
        Ok(part)
    }
}

/// The PowerPoint file of a deck, from what its page measured.
pub fn build(measured: &Value, site: &Path, basic: bool) -> Result<Vec<u8>> {
    if let Some(error) = measured["error"].as_str() {
        bail!("the deck could not be measured: {error}");
    }
    let slides = measured["slides"]
        .as_array()
        .context("the deck was not measured")?;
    if slides.is_empty() {
        bail!("the deck has no slides to export");
    }
    let width = number(&measured["size"][0]).max(16.);
    let height = number(&measured["size"][1]).max(16.);
    let mut package = Package {
        site,
        basic,
        parts: BTreeMap::new(),
        kinds: Vec::new(),
        extensions: BTreeSet::new(),
        media: BTreeMap::new(),
        charts: 0,
    };
    let has_notes = slides
        .iter()
        .any(|slide| !text(&slide["notes"]).trim().is_empty());

    let mut presentation = Rels::default();
    let master = presentation.add("slideMaster", "slideMasters/slideMaster1.xml");
    let slide_rels: Vec<String> = (0..slides.len())
        .map(|index| presentation.add("slide", &format!("slides/slide{}.xml", index + 1)))
        .collect();
    let notes_master =
        has_notes.then(|| presentation.add("notesMaster", "notesMasters/notesMaster1.xml"));
    presentation.add("theme", "theme/theme1.xml");
    presentation.add("presProps", "presProps.xml");
    presentation.add("viewProps", "viewProps.xml");
    presentation.add("tableStyles", "tableStyles.xml");

    let body_font = font(&most_used_font(slides), false, basic);
    let mut xml = format!(
        r#"{XML}<p:presentation {NS} saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="{master}"/></p:sldMasterIdLst>"#
    );
    if let Some(notes) = &notes_master {
        let _ = write!(
            xml,
            r#"<p:notesMasterIdLst><p:notesMasterId r:id="{notes}"/></p:notesMasterIdLst>"#
        );
    }
    xml.push_str("<p:sldIdLst>");
    for (index, rel) in slide_rels.iter().enumerate() {
        let _ = write!(xml, r#"<p:sldId id="{}" r:id="{rel}"/>"#, 256 + index);
    }
    let _ = write!(
        xml,
        r#"</p:sldIdLst><p:sldSz cx="{}" cy="{}"/><p:notesSz cx="6858000" cy="9144000"/><p:defaultTextStyle><a:lvl1pPr marL="0" algn="l"><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="{}"/></a:defRPr></a:lvl1pPr></p:defaultTextStyle></p:presentation>"#,
        emu(width).clamp(914_400, 51_206_400),
        emu(height).clamp(914_400, 51_206_400),
        escape(&body_font),
    );
    package.put("ppt/presentation.xml", xml);
    package.put("ppt/_rels/presentation.xml.rels", presentation.xml());
    package.kind(
        "/ppt/presentation.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
    );
    package.put(
        "ppt/presProps.xml",
        format!(r#"{XML}<p:presentationPr {NS}/>"#),
    );
    package.put(
        "ppt/viewProps.xml",
        format!(
            r#"{XML}<p:viewPr {NS}><p:normalViewPr><p:restoredLeft sz="15620"/><p:restoredTop sz="94660"/></p:normalViewPr><p:gridSpacing cx="76200" cy="76200"/></p:viewPr>"#
        ),
    );
    package.put(
        "ppt/tableStyles.xml",
        format!(
            r#"{XML}<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{{5C22544A-7EE6-4342-B048-85BDC9FD1C3A}}"/>"#
        ),
    );
    package.kind(
        "/ppt/presProps.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.presProps+xml",
    );
    package.kind(
        "/ppt/viewProps.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.viewProps+xml",
    );
    package.kind(
        "/ppt/tableStyles.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml",
    );

    // One master and one layout, both empty: every slide carries all that
    // is on it.
    let title = text(&measured["title"]);
    package.put(
        "ppt/theme/theme1.xml",
        theme_xml(if title.is_empty() { "Deck" } else { title }, &body_font),
    );
    package.kind(
        "/ppt/theme/theme1.xml",
        "application/vnd.openxmlformats-officedocument.theme+xml",
    );
    let mut master_rels = Rels::default();
    let layout = master_rels.add("slideLayout", "../slideLayouts/slideLayout1.xml");
    master_rels.add("theme", "../theme/theme1.xml");
    package.put(
        "ppt/slideMasters/slideMaster1.xml",
        format!(
            r#"{XML}<p:sldMaster {NS}><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree>{GROUP_HEADER}</p:spTree></p:cSld>{CLR_MAP}<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="{layout}"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr algn="l"><a:defRPr sz="4400"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mj-lt"/></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr marL="0" indent="0" algn="l"><a:buNone/><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr marL="0" algn="l"><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#
        ),
    );
    package.put(
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        master_rels.xml(),
    );
    package.kind(
        "/ppt/slideMasters/slideMaster1.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml",
    );
    let mut layout_rels = Rels::default();
    layout_rels.add("slideMaster", "../slideMasters/slideMaster1.xml");
    package.put(
        "ppt/slideLayouts/slideLayout1.xml",
        format!(
            r#"{XML}<p:sldLayout {NS} type="blank" preserve="1"><p:cSld name="Blank"><p:spTree>{GROUP_HEADER}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
        ),
    );
    package.put(
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        layout_rels.xml(),
    );
    package.kind(
        "/ppt/slideLayouts/slideLayout1.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml",
    );

    for (index, slide) in slides.iter().enumerate() {
        write_slide(&mut package, index, slide)?;
    }
    if has_notes {
        write_notes_master(&mut package, &body_font);
    }

    // What is in the file, by kind.
    let mut types = format!(
        r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>"#
    );
    for extension in &package.extensions {
        let kind = match extension.as_str() {
            "png" => "image/png",
            "gif" => "image/gif",
            "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            _ => "image/jpeg",
        };
        let _ = write!(
            types,
            r#"<Default Extension="{extension}" ContentType="{kind}"/>"#
        );
    }
    for (part, kind) in &package.kinds {
        let _ = write!(
            types,
            r#"<Override PartName="{part}" ContentType="{kind}"/>"#
        );
    }
    types.push_str("</Types>");
    package.put("[Content_Types].xml", types);
    package.put(
        "_rels/.rels",
        format!(
            r#"{XML}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#
        ),
    );

    let mut bytes = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // The content types first, as PowerPoint writes them.
        let first = "[Content_Types].xml";
        zip.start_file(first, options)?;
        zip.write_all(&package.parts[first])?;
        for (name, content) in &package.parts {
            if name != first {
                zip.start_file(name.as_str(), options)?;
                zip.write_all(content)?;
            }
        }
        zip.finish()?;
    }
    Ok(bytes)
}

/// The font most of the deck's text is in: the file's own text font.
fn most_used_font(slides: &[Value]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for item in slides
        .iter()
        .flat_map(|slide| slide["items"].as_array().into_iter().flatten())
    {
        for run in item["paragraphs"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|paragraph| paragraph["runs"].as_array().into_iter().flatten())
        {
            *counts.entry(text(&run["font"])).or_default() += text(&run["text"]).len();
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(family, _)| family.to_string())
        .unwrap_or_default()
}

fn write_slide(package: &mut Package<'_>, index: usize, slide: &Value) -> Result<()> {
    let mut rels = Rels::default();
    rels.add("slideLayout", "../slideLayouts/slideLayout1.xml");
    let mut shapes = String::new();
    let mut id = 1;
    for item in slide["items"].as_array().into_iter().flatten() {
        id += 1;
        let made = match text(&item["kind"]) {
            "shape" => Ok(shape_xml(id, item)),
            "text" => Ok(text_box_xml(id, item, &mut rels, package.basic)),
            "picture" => picture_xml(package, id, item, &mut rels),
            "table" => Ok(table_xml(id, item, &mut rels, package.basic)),
            "chart" => chart_frame(package, id, item, &mut rels),
            _ => Ok(String::new()),
        };
        match made {
            Ok(xml) => shapes.push_str(&xml),
            // One thing that cannot be written does not cost the deck.
            Err(err) => tracing::warn!("left out of the PowerPoint file: {err:#}"),
        }
    }
    let notes = text(&slide["notes"]).trim();
    if !notes.is_empty() {
        rels.add(
            "notesSlide",
            &format!("../notesSlides/notesSlide{}.xml", index + 1),
        );
        write_notes(package, index, notes);
    }
    let transition = match text(&slide["transition"]) {
        "fade" => r#"<p:transition spd="med"><p:fade/></p:transition>"#,
        "slide" => r#"<p:transition spd="med"><p:push dir="l"/></p:transition>"#,
        _ => "",
    };
    let name = format!("ppt/slides/slide{}.xml", index + 1);
    package.put(
        &name,
        format!(
            r#"{XML}<p:sld {NS}><p:cSld><p:bg><p:bgPr>{}<a:effectLst/></p:bgPr></p:bg><p:spTree>{GROUP_HEADER}{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>{transition}</p:sld>"#,
            solid(text(&slide["background"]), 1.),
        ),
    );
    package.put(
        &format!("ppt/slides/_rels/slide{}.xml.rels", index + 1),
        rels.xml(),
    );
    package.kind(
        &format!("/{name}"),
        "application/vnd.openxmlformats-officedocument.presentationml.slide+xml",
    );
    Ok(())
}

/// Where and how large, and turned by how much.
fn xfrm(item: &Value, tag: &str) -> String {
    let turned = number(&item["rotate"]);
    let rot = if turned.abs() > 0.01 {
        format!(
            r#" rot="{}""#,
            (turned.rem_euclid(360.) * 60_000.).round() as i64
        )
    } else {
        String::new()
    };
    format!(
        r#"<{tag}:xfrm{rot}><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></{tag}:xfrm>"#,
        emu(number(&item["x"])),
        emu(number(&item["y"])),
        emu(number(&item["w"])).max(0),
        emu(number(&item["h"])).max(0),
    )
}

fn shape_xml(id: u32, item: &Value) -> String {
    let (w, h) = (number(&item["w"]), number(&item["h"]));
    let opacity = number(&item["opacity"]).clamp(0., 1.);
    let opacity = if item["opacity"].is_null() {
        1.
    } else {
        opacity
    };
    let fill_hex = text(&item["fill"]);
    let fill_alpha = number(&item["fillAlpha"]) * opacity;
    let kind = text(&item["shape"]);
    if kind == "line" || kind == "connector" {
        // A line is as thick as its box was high, through its middle.
        let mut line = item.clone();
        line["y"] = (number(&item["y"]) + h / 2.).into();
        line["h"] = 0.into();
        let head = if kind == "connector" {
            r#"<a:tailEnd type="triangle"/>"#
        } else {
            ""
        };
        return format!(
            r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="{id}" name="Line {id}"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr>{}<a:prstGeom prst="line"><a:avLst/></a:prstGeom><a:ln w="{}">{}{head}</a:ln></p:spPr></p:cxnSp>"#,
            xfrm(&line, "a"),
            emu(h.max(1.)),
            solid(
                if fill_hex.is_empty() {
                    "#000000"
                } else {
                    fill_hex
                },
                fill_alpha.max(0.05)
            ),
        );
    }
    let geometry = match kind {
        "rounded" => {
            let least = w.min(h).max(1.);
            let adjust = ((number(&item["radius"]) / least) * 100_000.)
                .round()
                .clamp(0., 50_000.) as i64;
            format!(
                r#"<a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val {adjust}"/></a:avLst></a:prstGeom>"#
            )
        }
        other => {
            let preset = match other {
                "ellipse" => "ellipse",
                "diamond" => "diamond",
                "arrow-right" => "rightArrow",
                "arrow-left" => "leftArrow",
                "arrow-up" => "upArrow",
                "arrow-down" => "downArrow",
                _ => "rect",
            };
            format!(r#"<a:prstGeom prst="{preset}"><a:avLst/></a:prstGeom>"#)
        }
    };
    let fill = if fill_hex.is_empty() || fill_alpha <= 0.004 {
        "<a:noFill/>".to_string()
    } else {
        solid(fill_hex, fill_alpha)
    };
    let stroke = text(&item["stroke"]);
    let line = if stroke.is_empty() || number(&item["strokeWidth"]) <= 0. {
        "<a:ln><a:noFill/></a:ln>".to_string()
    } else {
        format!(
            r#"<a:ln w="{}">{}</a:ln>"#,
            emu(number(&item["strokeWidth"])),
            solid(stroke, opacity)
        )
    };
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Shape {id}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr>{}{geometry}{fill}{line}</p:spPr></p:sp>"#,
        xfrm(item, "a"),
    )
}

/// Half a point a pixel, in the hundredths of a point PowerPoint counts in.
fn hundredths(px: f64) -> i64 {
    (px * 50.).round() as i64
}

/// One run's properties.
fn run_props(run: &Value, rels: &mut Rels, basic: bool, tag: &str) -> String {
    let face = escape(&font(
        text(&run["font"]),
        run["mono"].as_bool().unwrap_or(false),
        basic,
    ));
    let spacing = number(&run["spacing"]);
    let mut attrs = format!(
        r#" lang="en-US" sz="{}""#,
        hundredths(number(&run["size"])).clamp(100, 400_000)
    );
    if run["bold"].as_bool().unwrap_or(false) {
        attrs.push_str(r#" b="1""#);
    }
    if run["italic"].as_bool().unwrap_or(false) {
        attrs.push_str(r#" i="1""#);
    }
    if run["underline"].as_bool().unwrap_or(false) {
        attrs.push_str(r#" u="sng""#);
    }
    if run["strike"].as_bool().unwrap_or(false) {
        attrs.push_str(r#" strike="sngStrike""#);
    }
    if spacing.abs() > 0.05 {
        let _ = write!(attrs, r#" spc="{}""#, hundredths(spacing));
    }
    attrs.push_str(r#" dirty="0""#);
    let alpha = if run["alpha"].is_null() {
        1.
    } else {
        number(&run["alpha"])
    };
    let link = match text(&run["link"]) {
        "" => String::new(),
        link => format!(
            r#"<a:hlinkClick r:id="{}"/>"#,
            rels.external("hyperlink", link)
        ),
    };
    format!(
        r#"<a:{tag}{attrs}>{}<a:latin typeface="{face}"/><a:ea typeface="{face}"/><a:cs typeface="{face}"/>{link}</a:{tag}>"#,
        solid(text(&run["color"]), alpha),
    )
}

/// One paragraph. `hang` is how far a bullet stands left of its text, in
/// pixels.
fn paragraph_xml(paragraph: &Value, rels: &mut Rels, basic: bool, hang: f64) -> String {
    let align = match text(&paragraph["align"]) {
        "center" => "ctr",
        "right" => "r",
        "justify" => "just",
        _ => "l",
    };
    let level = number(&paragraph["level"]).clamp(0., 8.);
    let bullet = text(&paragraph["bullet"]);
    let (margins, bullet) = match bullet {
        "bullet" | "number" => {
            // The bullet stands a little left of the words, as on the page.
            let size = number(&paragraph["runs"][0]["size"]).max(8.);
            let ink = match text(&paragraph["bulletColor"]) {
                "" => String::new(),
                hex => format!("<a:buClr>{}</a:buClr>", colour(hex, 1.)),
            };
            (
                format!(
                    r#" marL="{}" indent="{}""#,
                    emu(hang * (level + 1.)),
                    -emu(hang.min(size * 0.95))
                ),
                if bullet == "number" {
                    format!(
                        r#"{ink}<a:buFont typeface="+mj-lt"/><a:buAutoNum type="arabicPeriod"/>"#
                    )
                } else {
                    format!(r#"{ink}<a:buFont typeface="Arial"/><a:buChar char="•"/>"#)
                },
            )
        }
        _ => (
            r#" marL="0" indent="0""#.to_string(),
            "<a:buNone/>".to_string(),
        ),
    };
    // The page's line height is of the type's size; PowerPoint's is of the
    // font's own line, about a fifth more than its size.
    let spacing = ((number(&paragraph["lineHeight"]).max(0.5) / 1.2) * 100_000.).round() as i64;
    let mut xml = format!(
        r#"<a:p><a:pPr{margins} algn="{align}"><a:lnSpc><a:spcPct val="{spacing}"/></a:lnSpc><a:spcBef><a:spcPts val="0"/></a:spcBef><a:spcAft><a:spcPts val="{}"/></a:spcAft>{bullet}</a:pPr>"#,
        hundredths(number(&paragraph["spaceAfter"]).max(0.)),
    );
    let mut last = None;
    for run in paragraph["runs"].as_array().into_iter().flatten() {
        let props = run_props(run, rels, basic, "rPr");
        for (at, line) in text(&run["text"]).split('\n').enumerate() {
            if at > 0 {
                let _ = write!(xml, "<a:br>{props}</a:br>");
            }
            if !line.is_empty() {
                let _ = write!(xml, "<a:r>{props}<a:t>{}</a:t></a:r>", escape(line));
            }
        }
        last = Some(run);
    }
    if let Some(run) = last {
        xml.push_str(&run_props(run, rels, basic, "endParaRPr"));
    }
    xml.push_str("</a:p>");
    xml
}

fn text_box_xml(id: u32, item: &Value, rels: &mut Rels, basic: bool) -> String {
    let paragraphs: Vec<&Value> = item["paragraphs"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    // How far a list's words stand right of its edge (its bullets before
    // them): as the page measured it, or about the usual when it did not.
    let size = paragraphs
        .first()
        .map(|paragraph| number(&paragraph["runs"][0]["size"]))
        .unwrap_or(34.);
    let hang = match number(&item["hang"]) {
        measured if measured > 0. => measured,
        _ => size * 1.35,
    };
    let mut placed = item.clone();
    // A hair wider than measured: PowerPoint's own measure of a line may be
    // a little longer, and a word must not drop to the next line for it.
    placed["w"] = (number(&placed["w"]) * 1.02 + 2.).into();
    let anchor = match text(&item["valign"]) {
        "middle" => "ctr",
        "bottom" => "b",
        _ => "t",
    };
    let wrap = if item["wrap"].as_bool().unwrap_or(true) {
        "square"
    } else {
        "none"
    };
    let body: String = paragraphs
        .iter()
        .map(|paragraph| paragraph_xml(paragraph, rels, basic, hang))
        .collect();
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Text {id}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr>{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="{wrap}" lIns="0" tIns="0" rIns="0" bIns="0" anchor="{anchor}" rtlCol="0"><a:noAutofit/></a:bodyPr><a:lstStyle/>{body}</p:txBody></p:sp>"#,
        xfrm(&placed, "a"),
    )
}

fn picture_xml(
    package: &mut Package<'_>,
    id: u32,
    item: &Value,
    rels: &mut Rels,
) -> Result<String> {
    let part = package.picture(item)?;
    let blip = rels.add("image", &format!("../{part}"));
    let crop = &item["crop"];
    let src_rect = if crop.is_object() {
        let part = |side: &str| (number(&crop[side]).clamp(0., 0.95) * 100_000.).round() as i64;
        format!(
            r#"<a:srcRect l="{}" t="{}" r="{}" b="{}"/>"#,
            part("left"),
            part("top"),
            part("right"),
            part("bottom")
        )
    } else {
        String::new()
    };
    let radius = number(&item["radius"]);
    let least = number(&item["w"]).min(number(&item["h"])).max(1.);
    let geometry = if radius >= least / 2. - 0.5 {
        r#"<a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom>"#.to_string()
    } else if radius > 0.5 {
        format!(
            r#"<a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val {}"/></a:avLst></a:prstGeom>"#,
            ((radius / least) * 100_000.).round().clamp(0., 50_000.) as i64
        )
    } else {
        r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>"#.to_string()
    };
    Ok(format!(
        r#"<p:pic><p:nvPicPr><p:cNvPr id="{id}" name="Picture {id}"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="{blip}"/>{src_rect}<a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{}{geometry}</p:spPr></p:pic>"#,
        xfrm(item, "a"),
    ))
}

fn table_xml(id: u32, item: &Value, rels: &mut Rels, basic: bool) -> String {
    let columns: Vec<f64> = item["columns"]
        .as_array()
        .into_iter()
        .flatten()
        .map(number)
        .collect();
    let count = columns.len().max(1);
    let grid: String = if columns.is_empty() {
        format!(r#"<a:gridCol w="{}"/>"#, emu(number(&item["w"])))
    } else {
        columns
            .iter()
            .map(|width| format!(r#"<a:gridCol w="{}"/>"#, emu(*width)))
            .collect()
    };
    let rows: Vec<&Value> = item["rows"].as_array().into_iter().flatten().collect();
    // Which places of the grid a cell spanning down from above holds.
    let mut held = vec![vec![false; count]; rows.len()];
    let mut body = String::new();
    for (at, row) in rows.iter().enumerate() {
        let _ = write!(body, r#"<a:tr h="{}">"#, emu(number(&row["h"])).max(1));
        let mut cells = row["cells"].as_array().into_iter().flatten();
        let mut column = 0;
        while column < count {
            if held[at][column] {
                body.push_str(r#"<a:tc vMerge="1"><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody><a:tcPr/></a:tc>"#);
                column += 1;
                continue;
            }
            let Some(cell) = cells.next() else {
                body.push_str(
                    r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody><a:tcPr/></a:tc>"#,
                );
                column += 1;
                continue;
            };
            let across = (number(&cell["colspan"]).max(1.) as usize).min(count - column);
            let down = (number(&cell["rowspan"]).max(1.) as usize).min(rows.len() - at);
            for below in held.iter_mut().skip(at + 1).take(down - 1) {
                for place in below.iter_mut().skip(column).take(across) {
                    *place = true;
                }
            }
            let mut spans = String::new();
            if across > 1 {
                let _ = write!(spans, r#" gridSpan="{across}""#);
            }
            if down > 1 {
                let _ = write!(spans, r#" rowSpan="{down}""#);
            }
            let paragraph = &cell["paragraph"];
            let words = if paragraph["runs"]
                .as_array()
                .is_some_and(|runs| !runs.is_empty())
            {
                paragraph_xml(paragraph, rels, basic, 0.)
            } else {
                "<a:p/>".to_string()
            };
            let pad = |side: usize| emu(number(&cell["pad"][side]));
            let fill = match text(&cell["fill"]) {
                "" => "<a:noFill/>".to_string(),
                hex => solid(hex, 1.),
            };
            let none = |side: &str| format!(r#"<a:ln{side} w="0"><a:noFill/></a:ln{side}>"#);
            let under = match (text(&cell["line"]), number(&cell["lineWidth"])) {
                (hex, width) if !hex.is_empty() && width > 0. => {
                    format!(r#"<a:lnB w="{}">{}</a:lnB>"#, emu(width), solid(hex, 1.))
                }
                _ => none("B"),
            };
            let _ = write!(
                body,
                r#"<a:tc{spans}><a:txBody><a:bodyPr/><a:lstStyle/>{words}</a:txBody><a:tcPr marL="{}" marR="{}" marT="{}" marB="{}" anchor="t">{}{}{}{under}{fill}</a:tcPr></a:tc>"#,
                pad(3),
                pad(1),
                pad(0),
                pad(2),
                none("L"),
                none("R"),
                none("T"),
            );
            for _ in 1..across {
                body.push_str(r#"<a:tc hMerge="1"><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody><a:tcPr/></a:tc>"#);
            }
            column += across;
        }
        body.push_str("</a:tr>");
    }
    format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="{id}" name="Table {id}"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr>{}<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr/><a:tblGrid>{grid}</a:tblGrid>{body}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
        xfrm(item, "p"),
    )
}

/// A chart on a slide: the chart as a part of its own, with its numbers in
/// a workbook inside the file, so PowerPoint's *Edit Data* opens them.
fn chart_frame(
    package: &mut Package<'_>,
    id: u32,
    item: &Value,
    rels: &mut Rels,
) -> Result<String> {
    package.charts += 1;
    let nth = package.charts;
    let spec = &item["spec"];
    let book = format!("ppt/embeddings/Microsoft_Excel_Worksheet{nth}.xlsx");
    package.parts.insert(book, workbook(spec)?);
    package.extensions.insert("xlsx".into());
    let mut chart_rels = Rels::default();
    let data = chart_rels.add(
        "package",
        &format!("../embeddings/Microsoft_Excel_Worksheet{nth}.xlsx"),
    );
    let name = format!("ppt/charts/chart{nth}.xml");
    package.put(&name, chart_xml(item, &data, package.basic));
    package.put(
        &format!("ppt/charts/_rels/chart{nth}.xml.rels"),
        chart_rels.xml(),
    );
    package.kind(
        &format!("/{name}"),
        "application/vnd.openxmlformats-officedocument.drawingml.chart+xml",
    );
    let rel = rels.add("chart", &format!("../charts/chart{nth}.xml"));
    Ok(format!(
        r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="{id}" name="Chart {id}"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>{}<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="{rel}"/></a:graphicData></a:graphic></p:graphicFrame>"#,
        xfrm(item, "p"),
    ))
}

/// The letters of a sheet's column: A, B … Z, AA.
fn column(index: usize) -> String {
    let mut out = String::new();
    let mut n = index + 1;
    while n > 0 {
        out.insert(0, (b'A' + ((n - 1) % 26) as u8) as char);
        n = (n - 1) / 26;
    }
    out
}

fn labels(spec: &Value) -> Vec<String> {
    spec["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|label| text(label).to_string())
        .collect()
}

fn series(spec: &Value) -> Vec<(String, Vec<f64>)> {
    spec["series"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|one| {
            let values = one["values"]
                .as_array()
                .into_iter()
                .flatten()
                .map(number)
                .collect();
            (text(&one["name"]).to_string(), values)
        })
        .collect()
}

/// A chart's numbers as a workbook: the labels down the first column, a
/// series a column, its name on top.
fn workbook(spec: &Value) -> Result<Vec<u8>> {
    let mut sheet = format!(
        r#"{XML}<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#
    );
    let (labels, series) = (labels(spec), series(spec));
    let words = |at: String, said: &str| {
        format!(
            r#"<c r="{at}" t="inlineStr"><is><t xml:space="preserve">{}</t></is></c>"#,
            escape(said)
        )
    };
    sheet.push_str(r#"<row r="1">"#);
    sheet.push_str(&words("A1".into(), ""));
    for (at, (name, _)) in series.iter().enumerate() {
        sheet.push_str(&words(format!("{}1", column(at + 1)), name));
    }
    sheet.push_str("</row>");
    for (row, label) in labels.iter().enumerate() {
        let _ = write!(sheet, r#"<row r="{}">"#, row + 2);
        sheet.push_str(&words(format!("A{}", row + 2), label));
        for (at, (_, values)) in series.iter().enumerate() {
            if let Some(value) = values.get(row).filter(|value| value.is_finite()) {
                let _ = write!(
                    sheet,
                    r#"<c r="{}{}"><v>{value}</v></c>"#,
                    column(at + 1),
                    row + 2
                );
            }
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    let parts: [(&str, String); 6] = [
        (
            "[Content_Types].xml",
            format!(
                r#"{XML}<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/></Types>"#
            ),
        ),
        (
            "_rels/.rels",
            format!(
                r#"{XML}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
            ),
        ),
        (
            "xl/workbook.xml",
            format!(
                r#"{XML}<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{REL}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            format!(
                r#"{XML}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="{REL}/styles" Target="styles.xml"/></Relationships>"#
            ),
        ),
        ("xl/worksheets/sheet1.xml", sheet),
        (
            "xl/styles.xml",
            format!(
                r#"{XML}<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs></styleSheet>"#
            ),
        ),
    ];
    let mut bytes = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in parts {
            zip.start_file(name, options)?;
            zip.write_all(content.as_bytes())?;
        }
        zip.finish()?;
    }
    Ok(bytes)
}

/// The chart as DrawingML, its numbers both written in and in the workbook
/// `data` (a relationship) names.
fn chart_xml(item: &Value, data: &str, basic: bool) -> String {
    let ns = r#"xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
    let spec = &item["spec"];
    let kind = text(&spec["type"]);
    let (labels, series) = (labels(spec), series(spec));
    let ink = match text(&item["text"]) {
        "" => "404040".to_string(),
        hex => hex.trim_start_matches('#').to_ascii_uppercase(),
    };
    let colours: Vec<String> = item["colors"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|hex| text(hex).trim_start_matches('#').to_ascii_uppercase())
        .filter(|hex| hex.len() == 6)
        .collect();
    let fallback = ["4F7CFF", "2F4B99", "A7BDFF", "777777", "444444", "D3DEFF"];
    let paint = |at: usize| {
        colours
            .get(at)
            .cloned()
            .unwrap_or_else(|| fallback[at % fallback.len()].to_string())
    };
    let face = escape(&font(text(&item["font"]), false, basic));
    let tx_pr = format!(
        r#"<c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="1200"><a:solidFill><a:srgbClr val="{ink}"/></a:solidFill><a:latin typeface="{face}"/></a:defRPr></a:pPr><a:endParaRPr lang="en-US"/></a:p></c:txPr>"#
    );
    let range =
        |at: usize, rows: usize| format!("Sheet1!${0}$2:${0}${1}", column(at), rows.max(1) + 1);
    let count = labels.len();
    let mut categories = format!(
        r#"<c:cat><c:strRef><c:f>{}</c:f><c:strCache><c:ptCount val="{count}"/>"#,
        range(0, count)
    );
    for (at, label) in labels.iter().enumerate() {
        let _ = write!(
            categories,
            r#"<c:pt idx="{at}"><c:v>{}</c:v></c:pt>"#,
            escape(label)
        );
    }
    categories.push_str("</c:strCache></c:strRef></c:cat>");
    let pie = matches!(kind, "pie" | "donut");
    let lined = matches!(kind, "line");
    // Values on the bars where the page shows them: one series of bars.
    let shown = if series.len() == 1 && matches!(kind, "bar" | "hbar") {
        1
    } else {
        0
    };
    let values_shown = format!(
        r#"<c:dLbls><c:showLegendKey val="0"/><c:showVal val="{shown}"/><c:showCatName val="0"/><c:showSerName val="0"/><c:showPercent val="{}"/><c:showBubbleSize val="0"/></c:dLbls>"#,
        u8::from(pie)
    );
    let mut series_xml = String::new();
    for (at, (name, values)) in series.iter().enumerate() {
        if pie && at > 0 {
            break;
        }
        let colour = paint(at);
        let look = if lined {
            format!(
                r#"<c:spPr><a:ln w="28575" cap="rnd"><a:solidFill><a:srgbClr val="{colour}"/></a:solidFill><a:round/></a:ln></c:spPr><c:marker><c:symbol val="circle"/><c:size val="6"/><c:spPr><a:solidFill><a:srgbClr val="{colour}"/></a:solidFill></c:spPr></c:marker>"#
            )
        } else {
            format!(r#"<c:spPr><a:solidFill><a:srgbClr val="{colour}"/></a:solidFill></c:spPr>"#)
        };
        let points: String = if pie {
            (0..values.len())
                .map(|point| {
                    format!(
                        r#"<c:dPt><c:idx val="{point}"/><c:bubble3D val="0"/><c:spPr><a:solidFill><a:srgbClr val="{}"/></a:solidFill></c:spPr></c:dPt>"#,
                        paint(point)
                    )
                })
                .collect()
        } else {
            String::new()
        };
        let mut numbers = format!(
            r#"<c:val><c:numRef><c:f>{}</c:f><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val="{}"/>"#,
            range(at + 1, values.len()),
            values.len()
        );
        for (point, value) in values.iter().enumerate() {
            let _ = write!(numbers, r#"<c:pt idx="{point}"><c:v>{value}</c:v></c:pt>"#);
        }
        numbers.push_str("</c:numCache></c:numRef></c:val>");
        let _ = write!(
            series_xml,
            r#"<c:ser><c:idx val="{at}"/><c:order val="{at}"/><c:tx><c:strRef><c:f>Sheet1!${}$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>{}</c:v></c:pt></c:strCache></c:strRef></c:tx>{look}{points}{values_shown}{categories}{numbers}{}</c:ser>"#,
            column(at + 1),
            escape(name),
            if lined { r#"<c:smooth val="0"/>"# } else { "" },
        );
    }
    let axes_ids = r#"<c:axId val="111111111"/><c:axId val="222222222"/>"#;
    let plot = match kind {
        "line" => format!(
            r#"<c:lineChart><c:grouping val="standard"/><c:varyColors val="0"/>{series_xml}<c:marker val="1"/>{axes_ids}</c:lineChart>"#
        ),
        "area" => format!(
            r#"<c:areaChart><c:grouping val="standard"/><c:varyColors val="0"/>{series_xml}{axes_ids}</c:areaChart>"#
        ),
        "pie" => format!(
            r#"<c:pieChart><c:varyColors val="1"/>{series_xml}<c:firstSliceAng val="0"/></c:pieChart>"#
        ),
        "donut" => format!(
            r#"<c:doughnutChart><c:varyColors val="1"/>{series_xml}<c:firstSliceAng val="0"/><c:holeSize val="58"/></c:doughnutChart>"#
        ),
        other => format!(
            r#"<c:barChart><c:barDir val="{}"/><c:grouping val="{}"/><c:varyColors val="0"/>{series_xml}<c:gapWidth val="80"/>{}{axes_ids}</c:barChart>"#,
            if other == "hbar" { "bar" } else { "col" },
            if other == "stacked" {
                "stacked"
            } else {
                "clustered"
            },
            if other == "stacked" {
                r#"<c:overlap val="100"/>"#
            } else {
                ""
            },
        ),
    };
    let horizontal = kind == "hbar";
    let axes = if pie {
        String::new()
    } else {
        format!(
            r#"<c:catAx><c:axId val="111111111"/><c:scaling><c:orientation val="{}"/></c:scaling><c:delete val="0"/><c:axPos val="{}"/><c:numFmt formatCode="General" sourceLinked="0"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="222222222"/><c:crosses val="autoZero"/><c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/></c:catAx><c:valAx><c:axId val="222222222"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="{}"/><c:majorGridlines><c:spPr><a:ln w="6350"><a:solidFill><a:srgbClr val="{ink}"><a:alpha val="25000"/></a:srgbClr></a:solidFill></a:ln></c:spPr></c:majorGridlines><c:numFmt formatCode="General" sourceLinked="0"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="nextTo"/><c:crossAx val="111111111"/><c:crosses val="autoZero"/><c:crossBetween val="between"/></c:valAx>"#,
            if horizontal { "maxMin" } else { "minMax" },
            if horizontal { "l" } else { "b" },
            if horizontal { "b" } else { "l" },
        )
    };
    let legend = if pie || series.len() > 1 {
        r#"<c:legend><c:legendPos val="b"/><c:overlay val="0"/></c:legend>"#
    } else {
        ""
    };
    format!(
        r#"{XML}<c:chartSpace {ns}><c:roundedCorners val="0"/><c:chart><c:autoTitleDeleted val="1"/><c:plotArea><c:layout/>{plot}{axes}<c:spPr><a:noFill/><a:ln><a:noFill/></a:ln></c:spPr></c:plotArea>{legend}<c:plotVisOnly val="1"/><c:dispBlanksAs val="gap"/></c:chart><c:spPr><a:noFill/><a:ln><a:noFill/></a:ln></c:spPr>{tx_pr}<c:externalData r:id="{data}"><c:autoUpdate val="0"/></c:externalData></c:chartSpace>"#
    )
}

fn write_notes(package: &mut Package<'_>, index: usize, notes: &str) {
    let paragraphs: String = notes
        .split('\n')
        .map(|line| {
            format!(
                r#"<a:p><a:r><a:rPr lang="en-US"/><a:t>{}</a:t></a:r></a:p>"#,
                escape(line)
            )
        })
        .collect();
    let name = format!("ppt/notesSlides/notesSlide{}.xml", index + 1);
    package.put(
        &name,
        format!(
            r#"{XML}<p:notes {NS}><p:cSld><p:spTree>{GROUP_HEADER}<p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide image"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg"/></p:nvPr></p:nvSpPr><p:spPr/></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>{paragraphs}</p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#
        ),
    );
    let mut rels = Rels::default();
    rels.add("notesMaster", "../notesMasters/notesMaster1.xml");
    rels.add("slide", &format!("../slides/slide{}.xml", index + 1));
    package.put(
        &format!("ppt/notesSlides/_rels/notesSlide{}.xml.rels", index + 1),
        rels.xml(),
    );
    package.kind(
        &format!("/{name}"),
        "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml",
    );
}

fn write_notes_master(package: &mut Package<'_>, body_font: &str) {
    package.put(
        "ppt/notesMasters/notesMaster1.xml",
        format!(
            r#"{XML}<p:notesMaster {NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{GROUP_HEADER}<p:sp><p:nvSpPr><p:cNvPr id="2" name="Slide image"/><p:cNvSpPr><a:spLocks noGrp="1" noRot="1" noChangeAspect="1"/></p:cNvSpPr><p:nvPr><p:ph type="sldImg" idx="2"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="381000" y="685800"/><a:ext cx="6096000" cy="3429000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Notes"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" sz="quarter" idx="3"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="685800" y="4400550"/><a:ext cx="5486400" cy="3600450"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp></p:spTree></p:cSld>{CLR_MAP}<p:notesStyle><a:lvl1pPr marL="0" algn="l"><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:notesStyle></p:notesMaster>"#
        ),
    );
    let mut rels = Rels::default();
    rels.add("theme", "../theme/theme2.xml");
    package.put("ppt/notesMasters/_rels/notesMaster1.xml.rels", rels.xml());
    package.put("ppt/theme/theme2.xml", theme_xml("Notes", body_font));
    package.kind(
        "/ppt/notesMasters/notesMaster1.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml",
    );
    package.kind(
        "/ppt/theme/theme2.xml",
        "application/vnd.openxmlformats-officedocument.theme+xml",
    );
}

/// A theme PowerPoint needs to open the file. The slides say their own
/// colours and fonts; this names the font new text starts in.
fn theme_xml(name: &str, body_font: &str) -> String {
    let face = format!(
        r#"<a:latin typeface="{}"/><a:ea typeface=""/><a:cs typeface=""/>"#,
        escape(body_font)
    );
    format!(
        r#"{XML}<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="{name}"><a:themeElements><a:clrScheme name="{name}"><a:dk1><a:srgbClr val="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="{name}"><a:majorFont>{face}</a:majorFont><a:minorFont>{face}</a:minorFont></a:fontScheme><a:fmtScheme name="Office"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="50000"/></a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:shade val="80000"/></a:schemeClr></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="95000"/></a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:shade val="90000"/></a:schemeClr></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements><a:objectDefaults/><a:extraClrSchemeLst/></a:theme>"#,
        name = escape(name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Read as _;

    fn unzip(bytes: &[u8]) -> BTreeMap<String, String> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut parts = BTreeMap::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).unwrap();
            let mut content = Vec::new();
            file.read_to_end(&mut content).unwrap();
            let name = file.name().unwrap().into_owned();
            parts.insert(name, String::from_utf8_lossy(&content).into_owned());
        }
        parts
    }

    /// A run of plain words, as the page measures one.
    fn run(said: &str) -> String {
        format!(
            r##"{{ "text": "{said}", "font": "Inter", "size": 34, "bold": false, "italic": false, "underline": false, "color": "#241d17", "alpha": 1, "link": "" }}"##
        )
    }

    fn measured() -> Value {
        let cell = |fill: &str, align: &str, runs: &str, more: &str| {
            format!(
                r##"{{ "header": false, "colspan": 1, "rowspan": 1, "fill": "{fill}", "line": "", "lineWidth": 0, "pad": [20, 20, 20, 0]{more}, "paragraph": {{ "align": "{align}", "bullet": "none", "level": 0, "lineHeight": 1.3, "spaceAfter": 0, "runs": [{runs}] }} }}"##
            )
        };
        let text = r##"{
            "title": "Plan & more",
            "size": [1920, 1080],
            "slides": [{
                "file": "slides/a.html",
                "notes": "Say hello.\nThen go on.",
                "transition": "fade",
                "background": "#f6f1e7",
                "items": [
                    { "kind": "shape", "x": 100, "y": 100, "w": 400, "h": 200, "shape": "rounded", "radius": 24, "fill": "#a8431f", "fillAlpha": 1, "stroke": "", "strokeWidth": 0, "opacity": 1, "rotate": 0 },
                    { "kind": "shape", "x": 100, "y": 400, "w": 300, "h": 6, "shape": "connector", "fill": "#241d17", "fillAlpha": 1, "opacity": 1, "rotate": 45 },
                    { "kind": "text", "x": 112, "y": 96, "w": 900, "h": 80, "rotate": 0, "valign": "top", "wrap": false, "paragraphs": [
                        { "align": "left", "bullet": "none", "level": 0, "lineHeight": 1.1, "spaceAfter": 44, "runs": [
                            { "text": "Costs <fell>\nby half", "font": "Iowan Old Style", "size": 64, "bold": true, "italic": false, "underline": false, "color": "#241d17", "alpha": 1, "link": "https://example.com/a?b=1&c=2" }
                        ] },
                        { "align": "left", "bullet": "bullet", "level": 0, "lineHeight": 1.4, "spaceAfter": 14, "runs": [RUN_FIRST] }
                    ] },
                    { "kind": "picture", "x": 1000, "y": 300, "w": 400, "h": 300, "rotate": 0, "radius": 20, "src": "media/photo.png", "crop": { "left": 0.1, "right": 0.1, "top": 0, "bottom": 0 } },
                    { "kind": "picture", "x": 0, "y": 0, "w": 10, "h": 10, "rotate": 0, "src": "media/missing.png" },
                    { "kind": "table", "x": 112, "y": 600, "w": 900, "h": 200, "rotate": 0, "columns": [600, 300], "rows": [
                        { "h": 60, "cells": [CELL_HEAD] },
                        { "h": 70, "cells": [CELL_ONE, CELL_TWO] }
                    ] },
                    { "kind": "chart", "x": 1100, "y": 620, "w": 700, "h": 380, "rotate": 0, "text": "#6f6357", "colors": ["#a8431f"], "spec": { "type": "hbar", "labels": ["Couriers", "Van"], "series": [{ "name": "Share", "values": [38, 22] }], "unit": "%" } }
                ]
            }, {
                "file": "slides/b.html", "notes": "", "transition": "", "background": "#101010", "items": []
            }]
        }"##
        .replace("RUN_FIRST", &run("First"))
        .replace(
            "CELL_HEAD",
            &cell("", "left", &run("Action"), r##", "line": "#241d17", "lineWidth": 3"##)
                .replace(r#""colspan": 1"#, r#""colspan": 2"#)
                .replace(r##""line": "", "lineWidth": 0, "##, ""),
        )
        .replace("CELL_ONE", &cell("#ffffff", "left", &run("Fixed days"), ""))
        .replace("CELL_TWO", &cell("", "right", "", ""));
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn a_deck_becomes_a_file_powerpoint_reads() {
        let site = std::env::temp_dir().join(format!("ph-slides-pptx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&site);
        std::fs::create_dir_all(site.join("media")).unwrap();
        std::fs::write(site.join("media/photo.png"), b"\x89PNG....").unwrap();
        let parts = unzip(&build(&measured(), &site, false).unwrap());

        // Every part is XML that reads, and every one named is there.
        for (name, content) in &parts {
            if name.ends_with(".xml") || name.ends_with(".rels") {
                roxmltree::Document::parse(content).unwrap_or_else(|err| panic!("{name}: {err}"));
            }
        }
        let types = &parts["[Content_Types].xml"];
        for part in [
            "/ppt/presentation.xml",
            "/ppt/slides/slide1.xml",
            "/ppt/slides/slide2.xml",
            "/ppt/charts/chart1.xml",
            "/ppt/notesSlides/notesSlide1.xml",
            "/ppt/notesMasters/notesMaster1.xml",
        ] {
            assert!(types.contains(&format!("PartName=\"{part}\"")), "{part}");
            assert!(parts.contains_key(part.trim_start_matches('/')), "{part}");
        }
        assert!(types.contains("Extension=\"png\"") && types.contains("Extension=\"xlsx\""));
        assert!(parts["ppt/presentation.xml"].contains("cx=\"12192000\" cy=\"6858000\""));

        let slide = &parts["ppt/slides/slide1.xml"];
        // Text is text: its words, half a point a pixel, its own font, a link.
        assert!(
            slide.contains("<a:t>Costs &lt;fell&gt;</a:t>") && slide.contains("<a:t>by half</a:t>")
        );
        assert!(
            slide.contains("sz=\"3200\" b=\"1\"") && slide.contains("typeface=\"Iowan Old Style\"")
        );
        assert!(slide.contains("<a:br>") && slide.contains("wrap=\"none\""));
        assert!(slide.contains("<a:buChar char=\"•\"/>"));
        assert!(
            parts["ppt/slides/_rels/slide1.xml.rels"]
                .contains("Target=\"https://example.com/a?b=1&amp;c=2\" TargetMode=\"External\"")
        );
        // Shapes, a line with its head, the picture cut as it was shown.
        assert!(
            slide.contains("prst=\"roundRect\"") && slide.contains("<a:srgbClr val=\"A8431F\"/>")
        );
        assert!(
            slide.contains("<p:cxnSp>")
                && slide.contains("tailEnd")
                && slide.contains("rot=\"2700000\"")
        );
        assert!(slide.contains("<a:srcRect l=\"10000\" t=\"0\" r=\"10000\" b=\"0\"/>"));
        assert_eq!(
            slide.matches("<p:pic>").count(),
            1,
            "the picture that is missing is left out"
        );
        // A table with a cell across two columns; a chart with its numbers.
        assert!(slide.contains("gridSpan=\"2\"") && slide.contains("hMerge=\"1\""));
        assert!(slide.contains("<a:gridCol w=\"3810000\"/><a:gridCol w=\"1905000\"/>"));
        let chart = &parts["ppt/charts/chart1.xml"];
        assert!(chart.contains("<c:barDir val=\"bar\"/>") && chart.contains("<c:v>38</c:v>"));
        assert!(chart.contains("A8431F") && chart.contains("Sheet1!$B$2:$B$3"));
        assert!(
            parts["ppt/charts/_rels/chart1.xml.rels"]
                .contains("embeddings/Microsoft_Excel_Worksheet1.xlsx")
        );
        // Notes, a line a paragraph; the transition; the second slide plain.
        assert!(parts["ppt/notesSlides/notesSlide1.xml"].contains("<a:t>Say hello.</a:t>"));
        assert!(
            slide.contains("<p:fade/>") && !parts["ppt/slides/slide2.xml"].contains("p:transition")
        );
        assert!(parts["ppt/slides/slide2.xml"].contains("101010"));
        assert!(!parts.contains_key("ppt/notesSlides/notesSlide2.xml"));

        // With basic fonts, every face is one every computer has.
        let basic = unzip(&build(&measured(), &site, true).unwrap());
        let slide = &basic["ppt/slides/slide1.xml"];
        assert!(slide.contains("typeface=\"Georgia\"") && slide.contains("typeface=\"Arial\""));
        assert!(!slide.contains("Iowan") && !slide.contains("Inter"));
        let _ = std::fs::remove_dir_all(&site);
    }

    #[test]
    fn fonts_are_kept_or_made_basic() {
        assert_eq!(font("Inter", false, false), "Inter");
        assert_eq!(font("Inter", false, true), "Arial");
        assert_eq!(font("-apple-system", false, false), "Arial");
        assert_eq!(font("Palatino Linotype", false, true), "Georgia");
        assert_eq!(font("JetBrains Mono", true, true), "Courier New");
        assert_eq!(font("Open Sans", false, true), "Arial");
        assert_eq!(column(0), "A");
        assert_eq!(column(27), "AB");
        assert_eq!(
            colour("#a8431f", 0.5),
            "<a:srgbClr val=\"A8431F\"><a:alpha val=\"50000\"/></a:srgbClr>"
        );
        assert_eq!(colour("nonsense", 1.), "<a:srgbClr val=\"000000\"/>");
    }

    #[test]
    fn a_deck_that_was_not_measured_says_so() {
        let site = std::env::temp_dir();
        assert!(build(&json!({ "error": "no page" }), &site, false).is_err());
        assert!(build(&json!({ "size": [1920, 1080], "slides": [] }), &site, false).is_err());
        assert!(build(&Value::Null, &site, false).is_err());
    }
}
