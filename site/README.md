# The PilotHouse site

A single static page, published to GitHub Pages by `.github/workflows/pages.yml` whenever `site/` changes on `main`. There is no build step: what is here is what is served.

- `index.html`, `styles.css`, `main.js`: the page.
- `mark.svg`, `favicon.svg`: the app icon's tile (`brand/icons/app-tile.svg`).

The page loads nothing from other sites: no web fonts, analytics or scripts from elsewhere. The type is the reader's own system fonts (a serif for headings, a sans for text).

## What it is for

Someone who has never seen PilotHouse should be able to say, after one scroll, what it is, how a task goes in it, and what is in the window. Nobody reads a long page, so the headings and the drawings carry it and the text under them is a line or two. The order: what it is (the hero, with a window that plays a whole task), what it holds to (*You steer. You see. You own it.*), a task in four moves, the Bridge, the rest in a grid of tiles, privacy, and the download.

Write it the way you would say it to a colleague: short, plain words, "you". Say what a thing does, not how good it is. If a heading needs a paragraph to be understood, change the heading. Keep it true to `docs/` when the app changes.

## The drawings

There are no screenshots on the page. Every picture is a wireframe made of HTML and CSS: boxes for windows and cards, bars for text that does not matter, and real words only for what the reader should notice. One accent rings the thing a drawing is about; amber, blue and green mean what they mean in the app (needs you, working, ready).

A screenshot goes stale each time the app's look changes, in every theme. A drawing changes only when the feature does, and is a few lines of markup. When a feature changes:

- change the words in its drawing, or its boxes;
- give the figure an `aria-label` that says what it shows (the insides are `aria-hidden`);
- reuse the pieces there are (`.win`, `.pane`, `.bar`, `.dot`, `.tick`, `.wbtn`, `.card`, `.menu`, and `.tile` with its `.art` for the grid) before adding one.

(The screenshots the repository's README shows are in `docs/images/`.)

## What moves

`main.js` adds the motion; the comment at the top of each of its parts says what it does.

- **The sea** under the hero is drawn on a canvas, and the agents float on its first wave.
- **The hero's window** plays a task on a loop: the request is typed, the agent works, asks, gets a yes, the tests pass, the files are ticked as read, the change is committed. The steps and their timing are the `play` function; the parts it shows carry `data-d`, the things it points at `data-demo`.
- **A task in four moves** keeps one window in place on a wide screen while the four steps scroll by, and shows each step's drawing in it. On a narrow one each step has its drawing under it.
- **The Bridge's cards** go from column to column (the `script` list).
- **The tiles'** small drawings loop while they are on screen (CSS keyframes).
- **Make it yours** repaints the page in the accent picked, the way the app's accent setting does.

Each of these runs only while it is on screen. For a reader who asked their system for reduced motion nothing moves, and without JavaScript the page is the same drawings standing still: check both when you change one.

## Look

A night watch that ends at dawn: the page starts dark, the sky lightens through the privacy section, and the download is in daylight. The mark's violet is the accent, with the app's amber, blue and green for the states. The colours are the custom properties at the top of `styles.css`; `.day` switches them for the light end of the page.

## To look at it locally

```sh
python3 -m http.server -d site 8000
```

Then open <http://localhost:8000>.

**First publish:** in the repository's settings, under *Pages*, set the source to *GitHub Actions*. The next push to `main` that touches `site/` (or running the *Site* workflow by hand) publishes it.
