//! A deck written out for somewhere else: a PDF, a web page in one file, a
//! picture of a slide, a PowerPoint file. Each goes into the deck's
//! `exports/` folder, under the deck's name, never over a file that is
//! there.
//!
//! The PDF and the picture are taken from the deck's own page (the web
//! view draws them, so they are what the user sees). The web page is put
//! together here: the slides, the look, the pictures and the fonts in one
//! file that needs nothing beside it. The PowerPoint file is made from
//! where the page says everything is (`pptx.rs`).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use gpui_kit::{App, AppContext as _, Task};
use ph_artifacts::Artifacts;

use crate::deck::{DECK, Deck, MEDIA, STYLE};
use crate::hub::Decks;
use crate::live::Live;

/// A picture or video larger than this is not put inside an exported web
/// page (the page would not open in a mail): it is left out, and said.
const MAX_INLINE: u64 = 24 * 1024 * 1024;

const CORE: &str = include_str!("page/core.js");
const SLIDE_CSS: &str = include_str!("page/slide.css");
const WEB_PAGE: &str = include_str!("page/exported.html");

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    Pdf,
    Html,
    Png,
    /// With `basic`, in fonts every computer has.
    Pptx {
        basic: bool,
    },
}

impl Format {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "pdf" => Self::Pdf,
            "html" => Self::Html,
            "png" => Self::Png,
            "pptx" => Self::Pptx { basic: false },
            "pptx-basic" => Self::Pptx { basic: true },
            _ => return None,
        })
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Html => "html",
            Self::Png => "png",
            Self::Pptx { .. } => "pptx",
        }
    }
}

/// Exports deck `id` as `format` and answers with the file written. `slide`
/// is the file of the slide a picture is wanted of (the first without).
pub fn run(id: &str, format: Format, slide: Option<String>, cx: &mut App) -> Task<Result<PathBuf>> {
    let prepared = (|| {
        let artifact = Artifacts::global(cx)
            .and_then(|artifacts| artifacts.read(cx).find(id).cloned())
            .context("the deck is not there any more")?;
        let decks = Decks::global(cx).context("this build has no slides")?;
        let live = decks.read(cx).slides().live(id, &artifact.folder);
        // What an agent wrote a moment ago is in it.
        if let Some(applied) = live.scan() {
            decks.read(cx).slides().changed(id, &applied, "outside");
        }
        let deck = live.deck();
        if deck.shown().next().is_none() {
            bail!("the deck has no slides to export");
        }
        Ok((artifact, decks, live, deck))
    })();
    let (artifact, decks, live, deck) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => return Task::ready(Err(err)),
    };
    let title = if deck.title.trim().is_empty() {
        artifact.manifest.title.clone()
    } else {
        deck.title.clone()
    };
    let folder = artifact.folder.join("exports");
    let site = artifact.site();
    // What needs the deck's page: asked where the deck is drawn (its own
    // window, or out of sight when that is not open).
    let drawer = match format {
        Format::Html => None,
        _ => Some(Decks::drawer(&decks, id, cx)),
    };
    let drawn = async move {
        match drawer {
            Some(drawer) => drawer.await,
            None => Err(anyhow!("this export needs no page")),
        }
    };
    let work: Task<Result<(String, Vec<u8>)>> = match format {
        Format::Html => {
            cx.background_spawn(async move { Ok((title, web_page(&live, &site)?.into_bytes())) })
        }
        Format::Pdf => {
            let files: Vec<String> = deck.shown().map(|slide| slide.file.clone()).collect();
            cx.spawn(async move |cx| {
                let (view, window) = drawn.await?;
                let made = window
                    .update(cx, |_, window, cx| {
                        view.update(cx, |view, cx| view.pdf(files, window, cx))
                    })
                    .map_err(|err| anyhow!("{err}"))?;
                Ok((title, made.await?))
            })
        }
        Format::Png => {
            let at = slide
                .and_then(|file| deck.slides.iter().position(|one| one.file == file))
                .unwrap_or(0);
            let file = deck.slides[at].file.clone();
            cx.spawn(async move |cx| {
                let (view, window) = drawn.await?;
                let made = window
                    .update(cx, |_, window, cx| {
                        view.update(cx, |view, cx| view.picture(&file, window, cx))
                    })
                    .map_err(|err| anyhow!("{err}"))?;
                Ok((format!("{title} - slide {}", at + 1), made.await?))
            })
        }
        Format::Pptx { basic } => cx.spawn(async move |cx| {
            let (view, window) = drawn.await?;
            let measured = window
                .update(cx, |_, window, cx| {
                    view.update(cx, |view, cx| view.measure(window, cx))
                })
                .map_err(|err| anyhow!("{err}"))?
                .await?;
            let name = if basic {
                format!("{title} (basic fonts)")
            } else {
                title
            };
            cx.background_spawn(
                async move { Ok((name, crate::pptx::build(&measured, &site, basic)?)) },
            )
            .await
        }),
    };
    cx.background_spawn(async move {
        let (name, bytes) = work.await?;
        std::fs::create_dir_all(&folder)?;
        let path = free(&folder, &name, format.extension());
        ph_artifacts::library::write_atomic(&path, &bytes)?;
        Ok(path)
    })
}

/// `folder/name.extension`, or the first `name 2`, `name 3` … that is free.
fn free(folder: &Path, name: &str, extension: &str) -> PathBuf {
    let plain: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let plain: String = if plain.is_empty() {
        "Deck".into()
    } else {
        plain.chars().take(80).collect()
    };
    let mut path = folder.join(format!("{plain}.{extension}"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{plain} {n}.{extension}"));
        n += 1;
    }
    path
}

/// The MIME type a file of the deck's media is served as.
fn media_type(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// The deck as one web page that needs nothing beside it: opened in any
/// browser, it shows the slides, one at a time, with the keys and a click.
pub fn web_page(live: &Live, site: &Path) -> Result<String> {
    let deck = live.deck();
    let mut css = live
        .text(STYLE)
        .map(|text| text.to_string())
        .unwrap_or_default();
    let mut slides: Vec<(String, String)> = deck
        .shown()
        .map(|slide| {
            let html = live
                .text(&slide.file)
                .map(|text| text.to_string())
                .unwrap_or_default();
            (html, slide.transition.clone())
        })
        .collect();
    // Each file of the deck's media that a slide or the look names goes
    // into the page as it is.
    let mut left_out = Vec::new();
    for entry in std::fs::read_dir(site.join(MEDIA))
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{MEDIA}/{name}");
        let named = css.contains(&path) || slides.iter().any(|(html, _)| html.contains(&path));
        if !named {
            continue;
        }
        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(u64::MAX);
        if size > MAX_INLINE {
            left_out.push(name);
            continue;
        }
        let bytes =
            std::fs::read(entry.path()).with_context(|| format!("could not read {path}"))?;
        let inline = format!(
            "data:{};base64,{}",
            media_type(&path),
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        css = css.replace(&path, &inline);
        for (html, _) in &mut slides {
            *html = html.replace(&path, &inline);
        }
    }
    // The fonts the app brings, where the deck is set in them.
    let uses =
        |family: &str| css.contains(family) || slides.iter().any(|(html, _)| html.contains(family));
    let code = slides
        .iter()
        .any(|(html, _)| html.contains("<code") || html.contains("<pre"));
    let mut fonts = String::new();
    for face in &ph_theme::fonts::WEB_FACES {
        let wanted = match face.family {
            "Inter" => uses("Inter"),
            _ => code || slides.iter().any(|(html, _)| html.contains("font-code")),
        };
        if !wanted {
            continue;
        }
        fonts.push_str(&format!(
            "@font-face{{font-family:\"{}\";font-weight:{};font-style:{};src:url(data:font/ttf;base64,{}) format(\"truetype\")}}\n",
            face.family,
            face.weight,
            if face.italic { "italic" } else { "normal" },
            base64::engine::general_purpose::STANDARD.encode(face.bytes),
        ));
    }
    let data = serde_json::json!({
        "title": deck.title,
        "size": deck.size,
        "transition": deck.transition,
        "slides": slides.iter().map(|(html, transition)| serde_json::json!({ "html": html, "transition": transition })).collect::<Vec<_>>(),
        "leftOut": left_out,
    });
    // The page's own script, as one that needs no other file: what it
    // shares with the editor (`core.js`), without its `export`s.
    let core = CORE
        .replace("\nexport function ", "\nfunction ")
        .replace("\nexport class ", "\nclass ")
        .replace("\nexport const ", "\nconst ");
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    // In a `<script>` or a `<style>`, nothing may close it early.
    let inert = |text: String| text.replace("</", "<\\/");
    Ok(WEB_PAGE
        .replace("__TITLE__", &escape(&deck.title))
        .replace("__FONTS__", &fonts)
        .replace("__SLIDE_CSS__", &inert(SLIDE_CSS.to_string()))
        .replace("__DECK_CSS__", &inert(css))
        .replace("__DATA__", &inert(data.to_string()))
        .replace("__CORE__", &core))
}

/// The deck as `deck.json` says, for callers that only have its folder.
pub fn deck_of(site: &Path) -> Deck {
    std::fs::read_to_string(site.join(DECK))
        .ok()
        .and_then(|text| Deck::parse(&text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_web_page_carries_the_slides_their_pictures_and_the_look() {
        let site = std::env::temp_dir().join(format!("ph-slides-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&site);
        std::fs::create_dir_all(site.join("slides")).unwrap();
        std::fs::create_dir_all(site.join(MEDIA)).unwrap();
        std::fs::write(
            site.join(DECK),
            r#"{"title":"Plan <1>","slides":[{"file":"slides/a.html"},{"file":"slides/b.html","skip":true},{"file":"slides/c.html","transition":"slide"}]}"#,
        )
        .unwrap();
        std::fs::write(
            site.join(STYLE),
            ".slide { font-family: Inter; background: url(media/bg.png) }",
        )
        .unwrap();
        std::fs::write(
            site.join("slides/a.html"),
            "<section class=\"slide\"><img src=\"media/logo.png\"></section>",
        )
        .unwrap();
        std::fs::write(
            site.join("slides/b.html"),
            "<section class=\"slide\">skipped</section>",
        )
        .unwrap();
        std::fs::write(
            site.join("slides/c.html"),
            "<section class=\"slide\"></script><h2>End</h2></section>",
        )
        .unwrap();
        std::fs::write(site.join("media/logo.png"), b"PNG!").unwrap();
        std::fs::write(site.join("media/bg.png"), b"BG").unwrap();
        std::fs::write(site.join("media/unused.png"), b"UNUSED").unwrap();
        let live = Live::open("d", &site);
        let page = web_page(&live, &site).unwrap();

        assert!(page.contains("<title>Plan &lt;1&gt;</title>"));
        // The pictures a slide and the look name are inside; the other is not.
        assert!(page.contains("data:image/png;base64,UE5HIQ=="));
        assert!(page.contains("url(data:image/png;base64,Qkc=)"));
        assert!(!page.contains("VU5VU0VE") && !page.contains("media/logo.png"));
        // The skipped slide is left out; nothing closes the script early.
        assert!(!page.contains("skipped") && page.contains("<\\/script><h2>End"));
        // Set in the app's font: the font comes along; the code font does not.
        assert!(
            page.contains("font-family:\"Inter\"")
                && !page.contains("JetBrains Mono\";font-weight")
        );
        // The viewer's own script is whole, and nothing is left to fill in.
        assert!(page.contains("class Show") && !page.contains("\nexport "));
        for placeholder in [
            "__TITLE__",
            "__FONTS__",
            "__SLIDE_CSS__",
            "__DECK_CSS__",
            "__DATA__",
            "__CORE__",
        ] {
            assert!(!page.contains(placeholder), "{placeholder}");
        }
        let _ = std::fs::remove_dir_all(&site);
    }

    #[test]
    fn exports_never_take_a_name_that_is_there() {
        let folder = std::env::temp_dir().join(format!("ph-slides-free-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let first = free(&folder, "Q3: plan/review", "pdf");
        assert_eq!(first.file_name().unwrap(), "Q3 plan review.pdf");
        std::fs::write(&first, b"x").unwrap();
        assert_eq!(
            free(&folder, "Q3: plan/review", "pdf").file_name().unwrap(),
            "Q3 plan review 2.pdf"
        );
        assert_eq!(free(&folder, "  ", "png").file_name().unwrap(), "Deck.png");
        assert_eq!(
            Format::parse("pptx-basic"),
            Some(Format::Pptx { basic: true })
        );
        assert_eq!(Format::parse("doc"), None);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
