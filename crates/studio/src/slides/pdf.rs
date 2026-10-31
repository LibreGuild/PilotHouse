//! Writing a deck as PDF: each slide's SVG made a page by svg2pdf, so
//! shapes stay sharp and text stays text (with its fonts in the file);
//! with notes, a notes page after each slide.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context as _, Result, anyhow};
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, TextStr};
use svg2pdf::usvg;

use super::fonts::fonts;
use super::model::Deck;
use super::svg::{self, Options};

/// Writes slides `slides` (indexes; every slide when empty) of `deck` to
/// `out`, each followed by its notes page when `notes`.
pub fn write(
    deck: &Deck,
    media: Option<&Path>,
    slides: &[usize],
    notes: bool,
    out: &Path,
) -> Result<()> {
    let options = Options {
        media,
        editing: false,
        hide_text: None,
        small: false,
        hidden: &[],
    };
    let chosen: Vec<usize> = if slides.is_empty() {
        (0..deck.slides.len()).collect()
    } else {
        slides
            .iter()
            .copied()
            .filter(|index| *index < deck.slides.len())
            .collect()
    };
    if chosen.is_empty() {
        return Err(anyhow!("there are no slides to write"));
    }
    let usvg_options = usvg::Options {
        fontdb: fonts().db.clone(),
        ..usvg::Options::default()
    };
    // Each page: its SVG and its size.
    let mut pages: Vec<(String, f32, f32)> = Vec::new();
    for index in chosen {
        pages.push((
            svg::slide(deck, index, &options),
            deck.size.width,
            deck.size.height,
        ));
        if notes {
            pages.push((svg::notes_page(deck, index, &options), 612., 792.));
        }
    }
    let mut alloc = Ref::new(1);
    let catalog = alloc.bump();
    let tree_id = alloc.bump();
    let info = alloc.bump();
    let mut pdf = Pdf::new();
    let mut page_ids = Vec::new();
    let mut chunks = Vec::new();
    for (svg, width, height) in &pages {
        let tree = usvg::Tree::from_str(svg, &usvg_options).context("a slide's drawing")?;
        let (chunk, id) = svg2pdf::to_chunk(&tree, svg2pdf::ConversionOptions::default())
            .map_err(|err| anyhow!("making a PDF page: {err}"))?;
        let mut map = HashMap::new();
        let chunk = chunk.renumber(|old| *map.entry(old).or_insert_with(|| alloc.bump()));
        let svg_id = *map.get(&id).context("the page's drawing")?;
        let page = alloc.bump();
        let content = alloc.bump();
        let mut writer = pdf.page(page);
        writer.media_box(Rect::new(0., 0., *width, *height));
        writer.parent(tree_id);
        writer.contents(content);
        let mut resources = writer.resources();
        resources.x_objects().pair(Name(b"S1"), svg_id);
        resources.finish();
        writer.finish();
        let mut stream = Content::new();
        stream
            .transform([*width, 0., 0., *height, 0., 0.])
            .x_object(Name(b"S1"));
        pdf.stream(content, &stream.finish());
        page_ids.push(page);
        chunks.push(chunk);
    }
    for chunk in &chunks {
        pdf.extend(chunk);
    }
    pdf.catalog(catalog).pages(tree_id);
    pdf.pages(tree_id)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);
    pdf.document_info(info)
        .title(TextStr(&deck.title))
        .creator(TextStr("PilotHouse"));
    std::fs::write(out, pdf.finish())
        .with_context(|| format!("could not write {}", out.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn writes_pages() {
        let mut deck = super::super::raster::tests::sample("paper");
        std::sync::Arc::make_mut(&mut deck.slides[1]).notes = "Say the numbers slowly.".into();
        let dir = std::env::temp_dir().join(format!("pilothouse-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("deck.pdf");
        super::write(&deck, None, &[], true, &out).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let text = String::from_utf8_lossy(&bytes);
        // A page per slide and per notes page.
        assert_eq!(
            text.matches("/Type /Page").count() - text.matches("/Type /Pages").count(),
            deck.slides.len() * 2
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// `SLIDES_PDF=/some/dir cargo test -p ph-studio slides_pdf -- --ignored`.
#[cfg(test)]
mod look {
    #[test]
    #[ignore]
    fn slides_pdf() {
        let Ok(dir) = std::env::var("SLIDES_PDF") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut deck = super::super::raster::tests::sample("clean");
        std::sync::Arc::make_mut(&mut deck.slides[1]).notes =
            "Say the numbers slowly.\nThen ask for questions.".into();
        super::write(&deck, None, &[1, 2], true, &dir.join("deck.pdf")).unwrap();
    }
}
