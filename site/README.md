# The PilotHouse site

A single static page, published to GitHub Pages by `.github/workflows/pages.yml` whenever `site/` changes on `main`. There is no build step: what is here is what is served.

- `index.html`, `styles.css`, `main.js`: the page. Black, with the app icon's violet (`crates/app/assets/icon/PilotHouse.svg`) only for accents: buttons, highlights, a few glows. Sections in order: the hero (a radar of agents, the app rising as you scroll), the agents in big letters running past like a ticker, the Agents panel, the pinned tour, the chat, the nine tools in a strip that moves sideways, live panes, pull request reviews, Studio's stacking windows, file previews, automation along a line that draws down the day, the theme painter, the rest of the toolbox, why native, privacy, and the download with its mirrors. Keep the feature copy true to `docs/` when the app changes.
- **Motion.** Effects that follow the scroll position (the hero window, the floating chips, the sliding agent names, the marks drawn on the page, the progress bar) are CSS keyframes on `animation-timeline: view()` / `scroll()` (`.sd` and its `--a`/`--b` range in `styles.css`). Where the browser has no scroll timelines, `main.js` adds `no-sda` and scrubs the same keyframes by setting `--p`. The pinned pieces (the tour, the sideways workbench, the Studio stack, the timeline, the file kinds) are driven from one `requestAnimationFrame` scroll loop in `main.js`; the radar, the Agents panel, the chat box and the theme painter run only while on screen. With reduced motion asked for, none of it runs and everything shows in place (the workbench is a plain grid).
- `shots/`: real screenshots of the app (WebP, 1600 px wide), taken with the automation build on a scratch profile and a small demo project, with the macOS window buttons drawn in afterwards (the app does not draw them, so a screenshot leaves their place empty). `shots/themes/` holds the same view in each built-in theme. Retake them when the app's look changes.
- `mark.svg`, `favicon.svg`: the app icon's tile (`crates/app/assets/icon/PilotHouse-tile.svg`).

The page loads nothing from other sites: no web fonts, analytics or scripts from elsewhere.

To look at it locally:

```sh
python3 -m http.server -d site 8000
```

Then open <http://localhost:8000>.

**First publish:** in the repository's settings, under *Pages*, set the source to *GitHub Actions*. The next push to `main` that touches `site/` (or running the *Site* workflow by hand) publishes it.
