# The PilotHouse site

A single static page, published to GitHub Pages by `.github/workflows/pages.yml` whenever `site/` changes on `main`. There is no build step: what is here is what is served.

- `index.html`, `styles.css`, `main.js`: the page. Colours are the app icon's violet (`crates/app/assets/icon/PilotHouse.svg`) on a night sea. The motion (the sea under the hero, the tilting window, the live Agents panel and chat box, the scroll tour, counters) is all in `main.js`, pauses off screen, and is left out when the system asks for reduced motion.
- `shots/`: real screenshots of the app (WebP, 1600 px wide), taken with the automation build on a scratch profile and a small demo project, with the macOS window buttons drawn in afterwards (the app does not draw them, so a screenshot leaves their place empty). Retake them when the app's look changes.
- `mark.svg`, `favicon.svg`: the app icon's tile (`crates/app/assets/icon/PilotHouse-tile.svg`).

The page loads nothing from other sites: no web fonts, analytics or scripts from elsewhere.

To look at it locally:

```sh
python3 -m http.server -d site 8000
```

Then open <http://localhost:8000>.

**First publish:** in the repository's settings, under *Pages*, set the source to *GitHub Actions*. The next push to `main` that touches `site/` (or running the *Site* workflow by hand) publishes it.
