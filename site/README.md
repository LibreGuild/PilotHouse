# The PilotHouse site

A single static page, published to GitHub Pages by `.github/workflows/pages.yml` whenever `site/` changes on `main`. There is no build step: what is here is what is served.

- `index.html`, `styles.css`, `main.js`: the page.
- `mark.svg`, `favicon.svg`: the app icon's tile (`brand/icons/app-tile.svg`).

The page loads nothing from other sites: no web fonts, analytics or scripts from elsewhere. The type is the reader's own system fonts (a serif for headings, a sans for text).

## What it is for

Someone who has never seen PilotHouse should be able to say, after one scroll, what it is, how a task goes in it, and what is in the window. Nobody reads a long page, so the headings and the drawings carry it and the text under them is a line or two. The order: what it is (the hero, with a window that plays a whole task), what it holds to (*You steer. You see. You own it.*), a task in four moves, Explain (a change in plain words), the Bridge, the rest in a grid of tiles, privacy, and the download.

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

- **The hero is one scene**: a sky of stars, the words, the window, and a sea in front with the agents afloat on it as small boats. On a wide screen the scene stays in place for a stretch of scrolling (`.hero.pin`): the words leave, and the window comes up out of the sea and flat to the reader. One number, `--p`, goes from 0 to 1 over that stretch and the styles follow it.
- **The window plays a task** on a loop: the request is typed, the agent works, asks, gets a yes, the tests pass, the files are ticked as read, the change is committed. The steps and their timing are the `play` function; the parts it shows carry `data-d`, the things it points at `data-demo`, and the row of phases above the window says where the task is. On a phone the window has room for one pane, so the script switches it (`data-view`): the chat while the agent works, the Git pane for the review and the commit.
- **A task in four moves** keeps one window in place on a wide screen while the four steps scroll by, and shows each step's drawing in it. On a narrow one each step has its drawing under it.
- **Explain** flips one window between a change as a diff and the same change told in plain words, which comes a line at a time; the reader can flip the switch too, and it stays as they left it for a while. Without the script the explanation is what shows. Three small drawings under it loop (a commit that kept its story, *Why is this here?* on a line, the explanation joining a commit as a file).
- **The Bridge's cards** go from column to column (the `script` list), and a reader can answer one: Allow, Deny and Review changes move its card.
- **The tiles'** small drawings loop while they are on screen (CSS keyframes), and a light follows the pointer across the grid.
- **Make it yours** repaints the page in the accent picked, the way the app's accent setting does.
- **The sun comes up** behind a second sea at the end of the privacy section, as it is scrolled into view.

Each of these runs only while it is on screen. For a reader who asked their system for reduced motion nothing moves, and without JavaScript the page is the same drawings standing still: check both when you change one. So that the page never ends up half-way, a style that depends on the script is written against a class the script itself puts on the thing it runs (`pin`, `afloat`, `demo-on`, `pinned`, `staged`, `live`), never against "scripts are on".

## Look

A night watch that ends at dawn: the page starts dark, the sky lightens through the privacy section, and the download is in daylight. The night is black glass, not a colour: near-black grounds, panes with a faint sheen, and light only from the accent. The accent is teal (a reader can try the others in *Make it yours*), with the app's amber, blue and green for the states. Keep blue and violet glows out of it: they make a page look like every other one. The colours are the custom properties at the top of `styles.css`; `.day` switches them for the light end of the page.

## To look at it locally

```sh
python3 -m http.server -d site 8000
```

Then open <http://localhost:8000>. A browser may keep the stylesheet and the script from the last time: after changing one, reload with the cache off (⌘⇧R, or Ctrl+Shift+R). On the published site this cannot happen: the page links them as `?v=dev`, and the *Site* workflow replaces `dev` with the commit on each publish.

**First publish:** in the repository's settings, under *Pages*, set the source to *GitHub Actions*. The next push to `main` that touches `site/` (or running the *Site* workflow by hand) publishes it.
