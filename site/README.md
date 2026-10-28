# The PilotHouse site

A single static page, published to GitHub Pages by `.github/workflows/pages.yml` whenever `site/` changes on `main`. There is no build step: what is here is what is served.

- `index.html`, `styles.css`, `main.js`: the page. Colours are the app's Warm theme and its signal-magenta accent (`crates/theme`).
- `images/`: the illustrations (WebP with transparency). They were drawn with OpenAI's `gpt-image-1.5` at medium quality, in one style: soft matte clay, espresso browns, cream, one magenta accent, no text. Replace them with real screenshots later by keeping the same file names, or by changing the `<img>` tags.
- `fonts/Courgette-Regular.ttf`: the wordmark's font, the same file the app ships (SIL Open Font License, `fonts/OFL.txt`).
- `favicon.svg`: the app icon.

The page loads nothing from other sites: no web fonts, analytics or scripts from elsewhere.

To look at it locally:

```sh
python3 -m http.server -d site 8000
```

Then open <http://localhost:8000>.

**First publish:** in the repository's settings, under *Pages*, set the source to *GitHub Actions*. The next push to `main` that touches `site/` (or running the *Site* workflow by hand) publishes it.
