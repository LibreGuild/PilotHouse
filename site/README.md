# The PilotHouse site

A single static page, published to GitHub Pages by `.github/workflows/pages.yml` whenever `site/` changes on `main`. There is no build step: what is here is what is served.

- `index.html`, `styles.css`, `main.js`: the page.
- `mark.svg`, `favicon.svg`: the app icon's tile (`brand/icons/app-tile.svg`).

The page loads nothing from other sites: no web fonts, analytics or scripts from elsewhere. The type is the reader's own system fonts (a serif for headings, a sans for text).

## What it is for

Someone who has never seen PilotHouse should be able to say, after one read, what it is, how a task goes in it, and what is in the window. So the page goes in that order: what it is (the hero), why it exists and what it holds to (three goals), a task in four steps, the main parts one by one, the smaller things in a grid, privacy, what it is not, and the download.

Write it the way you would explain it to a colleague: short sentences, plain words, "you". Say what a thing does, not how good it is. Keep it true to `docs/` when the app changes.

## The drawings

There are no screenshots on the page. Every picture is a wireframe made of HTML and CSS (the `.wf` figures; their styles are the second half of `styles.css`): boxes for windows and cards, bars for text that does not matter, and real words only for what the reader should notice. One accent rings the thing a drawing is about; amber, blue and green mean what they mean in the app (needs you, working, ready).

A screenshot goes stale each time the app's look changes, in every theme. A drawing changes only when the feature does, follows light and dark by itself, and is a few lines of markup. When a feature changes:

- change the words in its drawing, or its boxes;
- give the figure an `aria-label` that says what it shows (the insides are `aria-hidden`);
- reuse the pieces there are (`.win`, `.pane`, `.bar`, `.dot`, `.tick`, `.wbtn`, `.card`, `.tag`, `.menu`) before adding one.

(The screenshots the repository's README shows are in `docs/images/`.)

## Look

Paper and ink, with the mark's violet as the one accent; light and dark follow the system, and the button in the header switches (the choice is kept in the browser's own storage, nowhere else). Motion is small: a drawing fades in as it is reached, and nothing moves for someone who asked for reduced motion. The page reads and works without JavaScript.

## To look at it locally

```sh
python3 -m http.server -d site 8000
```

Then open <http://localhost:8000>.

**First publish:** in the repository's settings, under *Pages*, set the source to *GitHub Actions*. The next push to `main` that touches `site/` (or running the *Site* workflow by hand) publishes it.
