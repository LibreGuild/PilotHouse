//! What agents are told about decks (`deck_guide`): what a deck is made of,
//! how a slide is written so that it fits, looks right, can be changed by
//! hand and comes out well in PowerPoint, and how to work on one with the
//! user. Read once before the first change to a deck.

/// The guide, in the app's name.
pub fn text() -> String {
    let name = ph_brand::NAME;
    format!(
        r#"# {name} Slides

A deck is a folder of web pages that {name} shows as slides, beside the chat you are in. The user watches it as you work, changes it by hand, presents it, shares it on their network and exports it (PDF, PowerPoint, a web page). You change it by editing its files with your own file tools; nothing else is needed, and every change shows at once.

## The files (in the deck's `site/` folder)

- `deck.json`: the deck. `{{ "title": "…", "size": [1920, 1080], "transition": "fade", "slides": [{{ "file": "slides/cover.html", "notes": "What the presenter says.", "section": "Why it matters" }}] }}`. The order of `slides` is the order of the deck. A slide not listed is not in the deck. `notes` are the speaker notes; `section` names the part of the deck that starts there; `skip: true` leaves a slide out when presenting; `transition` is `none`, `fade` or `slide`.
- `deck.css`: the deck's look. Colours and fonts are variables on top (`--bg`, `--fg`, `--muted`, `--accent`, `--accent-fg`, `--surface`, `--line`, `--font-head`, `--font-body`, `--font-code`); below them, how headings, lists, tables and each kind of slide are set. Read it before you write slides: use what it has. Change it to change every slide at once.
- `slides/<name>.html`: one slide each: one `<section class="slide">` and what is on it, nothing else (no `<html>`, no `<head>`). Names are lowercase letters, digits and dashes.
- `media/`: pictures and videos the slides show (`<img src="media/team.jpg">`). Copy the user's files there; never link to the internet.

To add a slide: write its file, then list it in `deck.json`. To remove one: take it out of `deck.json` and delete its file. To reorder: reorder `deck.json`. Give the deck its `title` once you know what it is about.

## Writing a slide

A slide is {width} × {height} pixels (`size`), whatever the window: write sizes in px of the slide. It does not scroll: what does not fit is cut off.

```html
<section class="slide">
  <p class="eyebrow">Where the money goes</p>
  <h2>Couriers and van runs drive the spend</h2>
  <div class="cols">
    <div class="card">
      <h3>Fewer, fuller trips</h3>
      <p>Group wholesale cafés into fixed delivery days.</p>
    </div>
    <div class="card">
      <p class="stat">38%</p>
      <p>of spend is courier fees.</p>
    </div>
  </div>
</section>
```

What `deck.css` gives you (in every built-in look; read the file for the deck's own):

- Kinds of slide, as a class beside `slide`: `title` (the cover), `section` (a divider on the accent colour), `quote`, `picture` (a picture beside text), `closing`; `dark` turns a slide's colours around. Without one, a slide is a heading with content under it.
- `h1` (cover only), `h2` (a slide's heading), `h3`, `p`, `ul`/`ol`, `table`, `blockquote` + `cite`, `figure` + `figcaption`, `pre`/`code`.
- `.eyebrow` (a small label over a heading), `.lead` (a larger opening line), `.small`, `.muted`, `.stat` (a large number), `.footer` (a line at the foot).
- `.cols` puts its children side by side in equal columns; `.card` is a column with a ground of its own.

Rules that keep a deck good, and editable by the user:

- **It must fit.** About 40 words on a slide, six bullets at most, three or four columns at most. If it does not fit, cut words or make two slides; do not shrink the text below 28px.
- **Text in real text elements** (`h2`, `p`, `li`, `td`), not in pictures or SVG text, and one piece of text per element: the user clicks an element to change it, and PowerPoint gets it as text.
- **The deck's colours and fonts**: `var(--accent)`, `var(--font-head)`. A colour written out (`#a8431f`) does not follow the look when the user changes it.
- **A style of its own** goes in the slide's file as a `style` attribute, or as a class added to `deck.css` when several slides share it. A `<style>` block inside a slide's `<section>` works too, and stays with that slide.
- **No scripts, no frames, no forms, nothing from the internet** (fonts and pictures included): they are removed when the slide is drawn.
- **Fonts**: Inter and JetBrains Mono come with {name}; besides them use only fonts every computer has (Georgia, Palatino, Helvetica, Arial, Times New Roman, Verdana, Courier New), as the built-in looks do.
- **Leave what the user did.** They move and resize things by hand: that shows as `data-id`, `translate`, `position: absolute`, `width`, `height` in a `style` attribute. Keep those when you edit a slide, unless they ask otherwise. `data-id` is how a comment finds its element: never change or reuse one.

## Charts, shapes, pictures

A chart is an element holding its numbers; {name} draws it in the deck's colours, the user changes the numbers by hand, and PowerPoint gets a real chart:

```html
<figure data-chart='{{"type":"bar","labels":["Q1","Q2","Q3"],"series":[{{"name":"Revenue","values":[12,18,24]}}],"unit":"k"}}'></figure>
```

`type` is `bar` (columns), `hbar`, `stacked`, `line`, `area`, `pie` or `donut`. Give it room: it fills the space left in a slide, or set `style="height: 520px"`. Use real numbers, and say in the notes where they come from; never invent data without saying so.

A shape is `<div data-shape="rect">`: `rect`, `rounded`, `ellipse`, `diamond`, `arrow-right`, `arrow-left`, `arrow-up`, `arrow-down`, `line`, `connector`. Place it with `style="position:absolute; left:…px; top:…px; width:…px; height:…px"`; text inside it is centred. For a diagram, shapes and connectors are better than a picture: each part stays editable.

A drawing you write yourself is inline `<svg>` (it exports as a picture). Pictures are `<img src="media/…" alt="…">`; give them a `width` or `height` in a style.

## Working on a deck

1. **A new deck**: unless the user said exactly what they want, ask in one message what it is for, who will see it and what they should take away. Then show an outline (one line a slide) and wait for a yes. Then write the slides one after another: the user sees each as it lands.
2. **Read before you write**: `deck.json`, `deck.css`, and the slide you are about to change. The user may have changed it by hand since you last read it; you are told when they did.
3. **After changing slides, call `deck_look`.** It says, for each slide, what does not fit or cannot be read (text running off the slide, text cut off in its box, text too small or too faint, a picture that did not load). Fix what it finds before you answer. With `render` it shows slides as pictures: look at a slide whose layout you are unsure of.
4. **"This"** in the user's message is what they selected; each message tells you the slide on the stage and the element selected.
5. **Comments**: the user pins comments on slides and sends them to you. Make the change, then answer with `deck_comments` (reply in a sentence, and resolve it).
6. **Speaker notes** for every slide you write: two or three sentences the presenter can say, in `deck.json`.
7. **A PowerPoint file to start from**: `deck_import` turns a `.pptx` into slides of this deck (text, tables, pictures, colours and places kept; animations and its template are not). Then tidy what it made. Other files (a document, a PDF, a sheet) you read yourself.
8. **Export only when asked**, with `deck_export`.

## What good looks like

- One idea a slide, said in its heading as a sentence the audience should remember ("Couriers drive the spend", not "Costs").
- Plenty of empty space; things line up on a few edges; one accent colour used sparingly.
- A picture, a chart or a diagram where it says it better than words; never decoration for its own sake.
- The same kind of content looks the same on every slide.
"#,
        width = crate::deck::SIZE[0],
        height = crate::deck::SIZE[1],
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_guide_names_the_app_and_keeps_its_samples_whole() {
        let guide = super::text();
        assert!(guide.starts_with(&format!("# {} Slides", ph_brand::NAME)));
        assert!(guide.contains("1920 × 1080 pixels"));
        // The samples came through the formatting whole.
        assert!(guide.contains(r#"{ "title": "…", "size": [1920, 1080]"#));
        assert!(guide.contains(r#"data-chart='{"type":"bar","labels":["Q1","Q2","Q3"]"#));
        assert!(guide.contains("<section class=\"slide\">\n  <p class=\"eyebrow\">"));
        for tool in ["deck_look", "deck_comments", "deck_import", "deck_export"] {
            assert!(guide.contains(tool), "{tool}");
        }
    }
}
