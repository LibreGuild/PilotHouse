//! `deck.*` control-socket methods, used by the MCP bridge for agents (the
//! `deck_` tools). An agent changes a deck by editing its files; these are
//! for what files do not give it: how the deck looks as drawn (what does
//! not fit, pictures of slides), the user's comments, exports, and a
//! PowerPoint file turned into slides.
//!
//! The deck is the one whose chat the agent is in (`project_id`): a deck's
//! own chat works in the deck's folder, and that is how it is found. In a
//! workspace's chat it is a deck made from that workspace (`deck.create`):
//! the one named (`deck`), else the one changed last.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use gpui_kit::{App, AppContext as _, Entity, Task};
use ph_artifacts::{Artifact, Artifacts};
use ph_control::ControlServer;
use serde_json::{Value, json};

use crate::comments;
use crate::deck::KIND;
use crate::export::{self, Format};
use crate::hub::Decks;
use crate::live::Live;
use crate::looks;
use crate::{guide, import};

/// Slides shown as pictures in one answer, at most, and how wide: enough
/// to judge a layout, without filling an agent's context.
const MAX_PICTURES: usize = 8;
const PICTURE_WIDTH: u32 = 1280;
/// How long the deck's page is given to say what does not fit, after a
/// change.
const MEASURE: Duration = Duration::from_millis(3500);

pub fn register_control_methods(cx: &mut App) {
    ControlServer::handle(
        "deck.guide",
        |_, _| Ok(json!({ "text": guide::text() })),
        cx,
    );
    ControlServer::handle_async("deck.look", look, cx);
    ControlServer::handle("deck.comments", comment, cx);
    ControlServer::handle_async("deck.export", export_deck, cx);
    ControlServer::handle_async("deck.import", import_file, cx);
    ControlServer::handle_async("deck.create", create, cx);
    ControlServer::handle("deck.list", list, cx);
    // The app's own tests of the deck's window (development builds only,
    // like the `ui.*` methods): a picture of the page, a script in it, who
    // has the keyboard and a key pressed as the system presses it.
    #[cfg(feature = "automation")]
    {
        ControlServer::handle_async("deck.screen", screen, cx);
        ControlServer::handle("deck.script", script, cx);
        ControlServer::handle("deck.keys", keys, cx);
    }
}

/// Whether the deck's page has the keyboard (`web`, where the system
/// says); with `take`, gives it the keyboard first, as a click in it does
/// (for the app's own tests of who gets what is typed).
#[cfg(feature = "automation")]
fn keys(params: Value, cx: &mut App) -> Result<Value> {
    let (artifact, _, decks) = find(&params, cx)?;
    let (view, _) = decks
        .read(cx)
        .view(artifact.id())
        .context("the deck's window is not open")?;
    if params["take"].as_bool().unwrap_or(false) {
        view.read(cx).focus(cx);
    }
    // A key pressed as the system presses it: `press` is what it types
    // (`plain` without the modifiers held), `code` its number.
    if let Some(characters) = params["press"].as_str() {
        let flag = |name: &str| params[name].as_bool().unwrap_or(false);
        let key = ph_webview::Key {
            characters,
            plain: params["plain"].as_str().unwrap_or(characters),
            code: params["code"].as_u64().unwrap_or_default() as u16,
            command: flag("command"),
            shift: flag("shift"),
            function: flag("function"),
        };
        let front = view.read(cx).press(&key, flag("front"), cx)?;
        return Ok(json!({ "web": view.read(cx).has_keys(cx), "front": front }));
    }
    Ok(json!({ "web": view.read(cx).has_keys(cx) }))
}

/// The deck's window as it shows now: a picture of its page with its
/// controls (for the app's own tests; an agent looks at slides with
/// `deck.look`).
#[cfg(feature = "automation")]
fn screen(params: Value, cx: &mut App) -> Task<Result<Value>> {
    let view = find(&params, cx).and_then(|(artifact, _, decks)| {
        decks
            .read(cx)
            .view(artifact.id())
            .context("the deck's window is not open")
    });
    let picture = match view {
        Ok((view, _)) => view.read(cx).screen(cx),
        Err(err) => return Task::ready(Err(err)),
    };
    cx.background_spawn(async move {
        let png = picture.await?;
        Ok(json!({ "images": [{
            "data": base64::engine::general_purpose::STANDARD.encode(png),
            "mimeType": "image/png",
        }] }))
    })
}

/// Runs a script in the page of the deck's window (for the app's own
/// tests of the page inside the app).
#[cfg(feature = "automation")]
fn script(params: Value, cx: &mut App) -> Result<Value> {
    let (artifact, _, decks) = find(&params, cx)?;
    let (view, _) = decks
        .read(cx)
        .view(artifact.id())
        .context("the deck's window is not open")?;
    view.read(cx)
        .evaluate(params["script"].as_str().context("missing `script`")?, cx);
    Ok(json!({}))
}

/// Makes a deck for the workspace whose chat asks: an artifact of its own,
/// belonging to that workspace, whose chats then work on it. Whoever shows
/// the app's windows is told (`made`), to show the deck beside the chat.
fn create(params: Value, cx: &mut App) -> Task<Result<Value>> {
    let prepared = (|| {
        let project = params["project_id"]
            .as_i64()
            .context("missing `project_id`")?;
        let decks = Decks::global(cx).context("this build has no slides")?;
        if let Some(own) = own_deck(project, cx) {
            bail!(
                "this chat is the deck “{}”'s own: write its slides (deck_guide says how). Another deck is made under {} → Artifacts → Slides, or from a workspace's chat.",
                own.manifest.title,
                ph_brand::NAME
            );
        }
        let (workspace, folders) = ph_artifacts::workspace_of(project, cx)
            .context("this chat has no workspace to make a deck for")?;
        let title: String = params["title"]
            .as_str()
            .map(|title| title.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|title| !title.is_empty())
            .context("`title` is required: what the deck is called (the user can rename it)")?
            .chars()
            .take(120)
            .collect();
        let look = match params["look"]
            .as_str()
            .map(str::trim)
            .filter(|look| !look.is_empty())
        {
            None => looks::LOOKS[0].id,
            Some(named) => looks::LOOKS
                .iter()
                .find(|look| {
                    look.id.eq_ignore_ascii_case(named) || look.name.eq_ignore_ascii_case(named)
                })
                .map(|look| look.id)
                .with_context(|| {
                    format!(
                        "`look` is one of {}",
                        looks::LOOKS
                            .iter()
                            .map(|look| look.id)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?,
        };
        let from = ph_artifacts::library::Source {
            project,
            workspace,
            path: folders.into_iter().next().unwrap_or_default(),
        };
        Ok((decks, project, title, look, from))
    })();
    let (decks, project, title, look, from) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => return Task::ready(Err(err)),
    };
    let made = decks.update(cx, |decks, cx| {
        decks.create_for(title, look, Some(from), cx)
    });
    cx.spawn(async move |cx| {
        let deck = made.await?;
        let others = cx.update(|cx| {
            decks.update(cx, |decks, cx| {
                decks.announce(deck.id(), json!({ "made": { "project": project } }), cx)
            });
            Decks::of_workspace(project, cx).len().saturating_sub(1)
        });
        let mut text = format!(
            "Made the deck {}. Its files are in {}: deck.json lists the slides, deck.css is its look, each slide is a file of its own in slides/. You change these files without asking; every change shows to the user at once, beside this chat, and can be undone there. Read deck_guide once before you write slides, write them, then call deck_look to see what does not fit. The user opens the deck in its own window (Open in Slides) to change it by hand, present or export it.",
            listed(&deck),
            deck.site().display(),
        );
        if others > 0 {
            text.push_str(&format!(
                " This workspace has {others} other deck{}: the deck_ tools work on the one changed last unless `deck` names one (deck_list shows them).",
                if others == 1 { "" } else { "s" }
            ));
        }
        Ok(json!({ "text": text, "id": deck.id() }))
    })
}

/// The decks of the workspace whose chat asks (in a deck's own chat: that
/// deck).
fn list(params: Value, cx: &mut App) -> Result<Value> {
    let project = params["project_id"]
        .as_i64()
        .context("missing `project_id`")?;
    let decks = match own_deck(project, cx) {
        Some(own) => vec![own],
        None => Decks::of_workspace(project, cx),
    };
    if decks.is_empty() {
        return Ok(json!({ "text": "This workspace has no deck yet. deck_create makes one." }));
    }
    let hub = Decks::global(cx).context("this build has no slides")?;
    let mut text = String::new();
    for (index, deck) in decks.iter().enumerate() {
        let live = hub.read(cx).slides().live(deck.id(), &deck.folder);
        let slides = live.deck().slides.len();
        text.push_str(&format!(
            "{}. {}, {slides} slide{}, in {}{}\n",
            index + 1,
            listed(deck),
            if slides == 1 { "" } else { "s" },
            deck.site().display(),
            if index == 0 && decks.len() > 1 {
                " (changed last: the deck_ tools work on this one unless `deck` names another)"
            } else {
                ""
            },
        ));
    }
    Ok(json!({ "text": text.trim_end() }))
}

/// The deck whose own chat `project` is (a deck's chat works in the deck's
/// folder), if it is one.
fn own_deck(project: i64, cx: &App) -> Option<Artifact> {
    let same = |a: &std::path::Path, b: &std::path::Path| {
        a == b
            || std::fs::canonicalize(a)
                .ok()
                .zip(std::fs::canonicalize(b).ok())
                .is_some_and(|(a, b)| a == b)
    };
    let folders = ph_artifacts::folders_of(project, cx);
    Artifacts::global(cx)?
        .read(cx)
        .all()
        .iter()
        .find(|artifact| {
            artifact.manifest.kind == KIND
                && folders.iter().any(|folder| same(folder, &artifact.folder))
        })
        .cloned()
}

/// One line a deck of a workspace is listed by.
fn listed(deck: &Artifact) -> String {
    format!("“{}” (deck `{}`)", deck.manifest.title, deck.id())
}

/// The deck of the chat that asks, held open: the deck the chat is beside,
/// or, in a workspace's chat, a deck made from that workspace (the one
/// `deck` names by its id or its title; without one, the one changed last).
fn find(params: &Value, cx: &mut App) -> Result<(Artifact, Arc<Live>, Entity<Decks>)> {
    let project = params["project_id"]
        .as_i64()
        .context("missing `project_id`")?;
    let decks = Decks::global(cx).context("this build has no slides")?;
    let artifact = match own_deck(project, cx) {
        Some(own) => own,
        None => {
            let made = Decks::of_workspace(project, cx);
            let named = params["deck"]
                .as_str()
                .map(str::trim)
                .filter(|named| !named.is_empty());
            match named {
                None => made.first().cloned().with_context(|| {
                    format!(
                        "this workspace has no deck yet. Make one with deck_create (a title is enough); the deck_ tools then work on it. (A deck made under {} → Artifacts → Slides has a chat of its own.)",
                        ph_brand::NAME
                    )
                })?,
                Some(named) => {
                    let lower = named.to_lowercase();
                    made.iter()
                        .find(|deck| deck.id() == named)
                        .or_else(|| {
                            made.iter()
                                .find(|deck| deck.manifest.title.to_lowercase() == lower)
                        })
                        .or_else(|| {
                            made.iter()
                                .find(|deck| deck.manifest.title.to_lowercase().contains(&lower))
                        })
                        .cloned()
                        .with_context(|| {
                            if made.is_empty() {
                                "this workspace has no deck yet: make one with deck_create".to_string()
                            } else {
                                format!(
                                    "this workspace has no deck “{named}”. Its decks: {}.",
                                    made.iter().map(listed).collect::<Vec<_>>().join(", ")
                                )
                            }
                        })?
                }
            }
        }
    };
    let live = decks
        .read(cx)
        .slides()
        .live(artifact.id(), &artifact.folder);
    // What the agent wrote a moment ago is in it, and on the user's page.
    if let Some(applied) = live.scan() {
        decks
            .read(cx)
            .slides()
            .changed(artifact.id(), &applied, "outside");
    }
    Ok((artifact, live, decks))
}

/// A slide named by its number (from 1) or its file.
fn slide_file(deck: &crate::deck::Deck, named: &Value) -> Option<String> {
    if let Some(number) = named
        .as_u64()
        .or_else(|| named.as_str()?.trim().parse().ok())
    {
        return deck
            .slides
            .get((number as usize).checked_sub(1)?)
            .map(|slide| slide.file.clone());
    }
    let name = named.as_str()?.trim();
    deck.slides
        .iter()
        .find(|slide| {
            slide.file == name
                || slide.file == format!("slides/{name}")
                || slide.file == format!("slides/{name}.html")
        })
        .map(|slide| slide.file.clone())
}

fn look(params: Value, cx: &mut App) -> Task<Result<Value>> {
    let (artifact, live, decks) = match find(&params, cx) {
        Ok(found) => found,
        Err(err) => return Task::ready(Err(err)),
    };
    let id = artifact.id().to_string();
    let slides = decks.read(cx).slides().clone();
    let deck = live.deck();
    let wanted: Vec<String> = match &params["render"] {
        Value::String(all) if all == "all" => {
            deck.slides.iter().map(|slide| slide.file.clone()).collect()
        }
        Value::Array(list) => list
            .iter()
            .filter_map(|named| slide_file(&deck, named))
            .collect(),
        Value::Null => Vec::new(),
        one => slide_file(&deck, one).into_iter().collect(),
    };
    let asked = wanted.len();
    let wanted: Vec<String> = wanted.into_iter().take(MAX_PICTURES).collect();
    let version = live.version();
    // The deck's own window measures by itself when asked; a deck without
    // one open is measured where it is drawn out of sight.
    let open = decks.read(cx).view(&id).is_some();
    if open {
        slides.command(&id, &json!({ "check": true }));
    }
    cx.spawn(async move |cx| {
        // The page measures after it drew the change; that takes a moment.
        let fresh = |live: &Live| {
            live.report()
                .is_some_and(|report| report["version"].as_u64().is_some_and(|told| told >= version))
        };
        let mut waited = Duration::ZERO;
        while open && !fresh(&live) && waited < MEASURE {
            let step = Duration::from_millis(120);
            cx.background_executor().timer(step).await;
            waited += step;
        }
        // Where it is drawn: needed for the pictures, and to measure a
        // deck whose window did not (it is not open, or was busy).
        let drawer = if !fresh(&live) || !wanted.is_empty() {
            Some(cx.update(|cx| Decks::drawer(&decks, &id, cx)).await)
        } else {
            None
        };
        if !fresh(&live)
            && !live.deck().slides.is_empty()
            && let Some(Ok((view, window))) = &drawer
        {
            let checked = window
                .update(cx, |_, window, cx| view.update(cx, |view, cx| view.check(window, cx)))
                .map_err(|err| anyhow::anyhow!("{err}"));
            // The page says the report to the deck itself as it answers.
            if let Ok(checked) = checked {
                let _ = checked.await;
            }
        }
        let measured = fresh(&live);
        let mut text = describe(&artifact, &live, measured);
        // What the user changed by hand since the agent last heard of it
        // (in the deck's own window each message says so too).
        let journal = live.take_journal();
        if !journal.is_empty() {
            text.push_str(&format!(
                "Since you last looked the user changed the deck by hand ({}): read a file again before you change it.\n",
                journal.join("; ")
            ));
        }
        let mut images = Vec::new();
        for file in &wanted {
            let (view, window) = match &drawer {
                Some(Ok(drawer)) => drawer.clone(),
                Some(Err(err)) => {
                    text.push_str(&format!("\nNo pictures: {err:#}."));
                    break;
                }
                None => break,
            };
            let picture = window
                .update(cx, |_, window, cx| view.update(cx, |view, cx| view.picture(file, window, cx)))
                .map_err(|err| anyhow::anyhow!("{err}"));
            let picture = match picture {
                Ok(picture) => picture.await,
                Err(err) => Err(err),
            };
            match picture {
                Ok(png) => {
                    let small = cx.background_spawn(async move { smaller(&png) }).await;
                    images.push(json!({
                        "data": base64::engine::general_purpose::STANDARD.encode(small),
                        "mimeType": "image/png",
                    }));
                }
                Err(err) => {
                    text.push_str(&format!("\nNo picture of {file}: {err:#}."));
                    break;
                }
            }
        }
        if asked > wanted.len() {
            text.push_str(&format!(
                "\nPictures of the first {MAX_PICTURES} slides asked for; ask for the others by number."
            ));
        }
        // A deck with no window of its own gets its picture for the
        // Artifacts list here, where it is drawn anyway.
        cx.update(|cx| decks.update(cx, |decks, cx| decks.picture_soon(&id, cx)));
        Ok(json!({ "text": text, "images": images }))
    })
}

/// A picture made `PICTURE_WIDTH` wide at most. One that cannot be read
/// goes as it is.
fn smaller(png: &[u8]) -> Vec<u8> {
    let Ok(picture) = image::load_from_memory_with_format(png, image::ImageFormat::Png) else {
        return png.to_vec();
    };
    if picture.width() <= PICTURE_WIDTH {
        return png.to_vec();
    }
    let height = (u64::from(picture.height()) * u64::from(PICTURE_WIDTH)
        / u64::from(picture.width())) as u32;
    let small = picture.resize(
        PICTURE_WIDTH,
        height.max(1),
        image::imageops::FilterType::Triangle,
    );
    let mut out = std::io::Cursor::new(Vec::new());
    match small.write_to(&mut out, image::ImageFormat::Png) {
        Ok(()) => out.into_inner(),
        Err(_) => png.to_vec(),
    }
}

/// The deck in words: its slides, what does not fit on each, what the user
/// has selected, and the comments that are open.
fn describe(artifact: &Artifact, live: &Live, measured: bool) -> String {
    let deck = live.deck();
    let mut text = format!(
        "Deck “{}”: {} slide{}, {} × {} px, in {}.\n",
        if deck.title.is_empty() {
            &artifact.manifest.title
        } else {
            &deck.title
        },
        deck.slides.len(),
        if deck.slides.len() == 1 { "" } else { "s" },
        deck.size[0],
        deck.size[1],
        artifact.site().display(),
    );
    if live
        .text(crate::deck::DECK)
        .is_some_and(|json| crate::deck::Deck::parse(&json).is_err())
    {
        text.push_str(
            "! deck.json cannot be read as JSON: the deck shows as empty until it can.\n",
        );
    }
    let report = live.report().filter(|_| measured);
    let reported = |file: &str| -> Option<&Value> {
        report.as_ref()?["slides"]
            .as_array()?
            .iter()
            .find(|slide| slide["file"] == file)
    };
    let mut wrong = 0;
    for (index, slide) in deck.slides.iter().enumerate() {
        let told = reported(&slide.file);
        let title = told
            .and_then(|told| told["title"].as_str())
            .unwrap_or_default();
        text.push_str(&format!("{}. {}", index + 1, slide.file));
        if !title.is_empty() {
            text.push_str(&format!(" — “{title}”"));
        }
        let mut marks = Vec::new();
        if slide.notes.is_empty() {
            marks.push("no notes");
        }
        if slide.skip {
            marks.push("skipped");
        }
        if live.text(&slide.file).is_none() {
            marks.push("its file is missing");
            wrong += 1;
        }
        if !marks.is_empty() {
            text.push_str(&format!(" ({})", marks.join(", ")));
        }
        text.push('\n');
        for issue in told
            .into_iter()
            .flat_map(|told| told["issues"].as_array().into_iter().flatten())
        {
            if let Some(issue) = issue.as_str() {
                text.push_str(&format!("   ! {issue}\n"));
                wrong += 1;
            }
        }
    }
    if deck.slides.is_empty() {
        text.push_str("No slides yet.\n");
    } else if !measured {
        text.push_str(
            "The deck's page is not open (or did not answer in time), so nothing was measured: what fits is not known.\n",
        );
    } else if wrong == 0 {
        text.push_str("Everything fits and can be read.\n");
    } else {
        text.push_str(&format!(
            "{wrong} thing{} to fix (marked !), then look again.\n",
            if wrong == 1 { "" } else { "s" }
        ));
    }
    if let Some(selection) = live.selection() {
        let said = |key: &str| selection[key].as_str().unwrap_or_default();
        text.push_str(&format!(
            "On the stage: slide {} ({})",
            selection["slide"].as_u64().unwrap_or(1),
            said("file")
        ));
        if !said("tag").is_empty() {
            text.push_str(&format!(
                "; selected: <{}> “{}” at `{}`",
                said("tag"),
                said("text"),
                said("path")
            ));
        }
        text.push_str(".\n");
    }
    let open: Vec<comments::Comment> = comments::read(&artifact.folder)
        .into_iter()
        .filter(|comment| !comment.resolved)
        .collect();
    if !open.is_empty() {
        text.push_str(&format!("Open comments ({}):\n", open.len()));
        for comment in &open {
            text.push_str(&comment_line(comment));
        }
    }
    text
}

fn comment_line(comment: &comments::Comment) -> String {
    let about = match (comment.element.as_str(), comment.label.as_str()) {
        ("", _) => "the slide".to_string(),
        (element, "") => format!("data-id=\"{element}\""),
        (element, label) => format!("{label}, data-id=\"{element}\""),
    };
    let mut line = format!(
        "- {} on {} ({about}), by {}: {}\n",
        comment.id, comment.file, comment.author, comment.text
    );
    for reply in &comment.replies {
        line.push_str(&format!("    {}: {}\n", reply.author, reply.text));
    }
    line
}

/// `{op: "list"}` (the default), `{op: "reply", id, text}`, `{op:
/// "resolve", id, text}` (an answer, and it is done).
fn comment(params: Value, cx: &mut App) -> Result<Value> {
    let (artifact, _, decks) = find(&params, cx)?;
    let slides = decks.read(cx).slides().clone();
    let op = params["op"].as_str().unwrap_or("list");
    let id = params["id"].as_str().unwrap_or_default();
    let text = params["text"].as_str().unwrap_or_default().trim();
    let author = params["author"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Agent");
    let apply = |op: Value| slides.comment_as(artifact.id(), &artifact.folder, &op, author);
    match op {
        "list" => {
            let all = comments::read(&artifact.folder);
            if all.is_empty() {
                return Ok(json!({ "text": "The deck has no comments." }));
            }
            let (open, done): (Vec<_>, Vec<_>) = all.iter().partition(|comment| !comment.resolved);
            let mut text = format!("{} open, {} resolved.\n", open.len(), done.len());
            for comment in open {
                text.push_str(&comment_line(comment));
            }
            Ok(json!({ "text": text }))
        }
        "reply" => {
            if text.is_empty() {
                bail!("`text` is required: what you answer");
            }
            apply(json!({ "op": "reply", "id": id, "text": text }))?;
            Ok(json!({ "text": format!("Answered {id}.") }))
        }
        "resolve" => {
            if !text.is_empty() {
                apply(json!({ "op": "reply", "id": id, "text": text }))?;
            }
            apply(json!({ "op": "resolve", "id": id, "resolved": true }))?;
            Ok(json!({ "text": format!("Resolved {id}.") }))
        }
        other => bail!("deck_comments takes list, reply and resolve, not `{other}`"),
    }
}

fn export_deck(params: Value, cx: &mut App) -> Task<Result<Value>> {
    let prepared = (|| {
        let (artifact, live, _) = find(&params, cx)?;
        let format = params["format"]
            .as_str()
            .and_then(Format::parse)
            .context("`format` is one of pdf, pptx, pptx-basic, html, png")?;
        let slide = slide_file(&live.deck(), &params["slide"]);
        Ok::<_, anyhow::Error>((artifact, format, slide))
    })();
    let (artifact, format, slide) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => return Task::ready(Err(err)),
    };
    let written = export::run(artifact.id(), format, slide, cx);
    cx.background_spawn(async move {
        let path = written.await?;
        Ok(json!({ "text": format!("Written: {}", path.display()), "path": path }))
    })
}

fn import_file(params: Value, cx: &mut App) -> Task<Result<Value>> {
    let prepared = (|| {
        let (artifact, live, decks) = find(&params, cx)?;
        let path = params["path"].as_str().map(str::trim).filter(|path| !path.is_empty()).context(
            "`path` is required: the .pptx file to make slides from (one the user gave you, in the deck's references folder)",
        )?;
        let mut path = std::path::PathBuf::from(path);
        if path.is_relative() {
            // In a workspace's chat a file is named from the workspace;
            // in a deck's own chat, from the deck's folder.
            let from = params["project_id"]
                .as_i64()
                .map(|project| ph_artifacts::folders_of(project, cx))
                .unwrap_or_default();
            path = from
                .iter()
                .map(|folder| folder.join(&path))
                .find(|there| there.is_file())
                .unwrap_or_else(|| artifact.folder.join(&path));
        }
        Ok::<_, anyhow::Error>((artifact, live, decks, path))
    })();
    let (artifact, live, decks, path) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => return Task::ready(Err(err)),
    };
    let slides = decks.read(cx).slides().clone();
    let look = params["look"]
        .as_bool()
        .unwrap_or(live.deck().slides.is_empty());
    cx.background_spawn(async move {
        let imported = import::powerpoint(&path, &live, look)?;
        if let Some(applied) = live.scan() {
            slides.changed(artifact.id(), &applied, "outside");
        }
        Ok(json!({ "text": imported }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace's agent makes a deck; the deck tools then find it from
    /// that workspace's chat (the one changed last, or the one named), and
    /// from no other. A deck's own chat has its deck and makes no other.
    #[gpui_kit::test]
    fn a_workspace_makes_decks_and_its_chats_find_them(cx: &mut gpui_kit::TestAppContext) {
        use std::rc::Rc;

        let base = dunce::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("ph-deck-workspace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (work, kept) = (base.join("work"), base.join("kept"));
        std::fs::create_dir_all(&work).unwrap();
        let folders = vec![work.clone()];
        // Space 40 is a deck's own chat: it works in that deck's folder,
        // once there is such a deck.
        let own: Rc<std::cell::RefCell<Option<std::path::PathBuf>>> = Rc::default();
        let own_folder = own.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            ph_theme::init(cx);
            ph_artifacts::start(kept.clone(), cx);
            ph_artifacts::set_host(
                ph_artifacts::Host {
                    workspace: Rc::new(move |project, _| match project {
                        3 => Some(("travel".to_string(), folders.clone())),
                        40 => own_folder
                            .borrow()
                            .clone()
                            .map(|folder| ("deck".to_string(), vec![folder])),
                        _ => None,
                    }),
                    show: Rc::new(|_, _, _| Task::ready(Ok(Vec::new()))),
                    open_in_workspace: Rc::new(|_, _, _, _| {}),
                    trash: |_| Ok(()),
                    renamed: |_, _, _| {},
                    kinds: Vec::new(),
                },
                cx,
            );
            crate::init(cx);
        });
        cx.run_until_parked();
        let run = |cx: &mut gpui_kit::TestAppContext, made: Task<Result<Value>>| {
            cx.run_until_parked();
            cx.foreground_executor().block_test(made)
        };
        let found = |cx: &mut gpui_kit::TestAppContext, params: Value| {
            cx.update(|cx| find(&params, cx).map(|(deck, ..)| deck.manifest.title))
        };

        // Nothing yet, and nothing for a chat that is no workspace's.
        let none = found(cx, json!({ "project_id": 3 }))
            .unwrap_err()
            .to_string();
        assert!(none.contains("deck_create"), "{none}");
        let nowhere = cx.update(|cx| create(json!({ "project_id": 9, "title": "x" }), cx));
        assert!(run(cx, nowhere).is_err());
        let untitled = cx.update(|cx| create(json!({ "project_id": 3, "title": "  " }), cx));
        assert!(run(cx, untitled).is_err());
        let odd =
            cx.update(|cx| create(json!({ "project_id": 3, "title": "x", "look": "neon" }), cx));
        assert!(run(cx, odd).unwrap_err().to_string().contains("paper"));

        let first = cx.update(|cx| {
            create(
                json!({ "project_id": 3, "title": "Trip plan", "look": "Clean" }),
                cx,
            )
        });
        let first = run(cx, first).unwrap();
        let first_id = first["id"].as_str().unwrap().to_string();
        assert!(
            first["text"].as_str().unwrap().contains("Trip plan"),
            "{first}"
        );
        let made = cx.update(|cx| Decks::of_workspace(3, cx));
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].manifest.kind, KIND);
        let source = made[0].manifest.source.clone().unwrap();
        assert_eq!((source.project, source.workspace.as_str()), (3, "travel"));
        assert_eq!(source.path, work);
        assert!(made[0].site().join("deck.json").is_file());
        assert_eq!(found(cx, json!({ "project_id": 3 })).unwrap(), "Trip plan");
        assert!(
            found(cx, json!({ "project_id": 9 })).is_err(),
            "another chat has none"
        );

        // A second one: it is the one changed last; either is named.
        let second = cx.update(|cx| create(json!({ "project_id": 3, "title": "Costs" }), cx));
        let second = run(cx, second).unwrap();
        assert!(
            second["text"].as_str().unwrap().contains("1 other deck"),
            "{second}"
        );
        assert_eq!(cx.update(|cx| Decks::of_workspace(3, cx)).len(), 2);
        let by = |cx: &mut gpui_kit::TestAppContext, deck: &str| {
            found(cx, json!({ "project_id": 3, "deck": deck }))
        };
        assert_eq!(by(cx, &first_id).unwrap(), "Trip plan");
        assert_eq!(by(cx, "costs").unwrap(), "Costs");
        assert_eq!(by(cx, "trip").unwrap(), "Trip plan");
        let unknown = by(cx, "budget").unwrap_err().to_string();
        assert!(
            unknown.contains("Trip plan") && unknown.contains("Costs"),
            "{unknown}"
        );
        let all = cx
            .update(|cx| list(json!({ "project_id": 3 }), cx))
            .unwrap();
        let all = all["text"].as_str().unwrap();
        assert!(
            all.contains("Trip plan") && all.contains("Costs") && all.contains(&first_id),
            "{all}"
        );

        // A deck's own chat: its deck, whatever is named, and no new one.
        *own.borrow_mut() = Some(made[0].folder.clone());
        assert_eq!(found(cx, json!({ "project_id": 40 })).unwrap(), "Trip plan");
        assert_eq!(
            found(cx, json!({ "project_id": 40, "deck": "costs" })).unwrap(),
            "Trip plan"
        );
        let again = cx.update(|cx| create(json!({ "project_id": 40, "title": "More" }), cx));
        assert!(run(cx, again).unwrap_err().to_string().contains("own"));

        let _ = std::fs::remove_dir_all(&base);
    }
}
