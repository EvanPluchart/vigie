# Landing page

The static page for GitHub Pages, in French (`index.html`) and English (`en/index.html`), sharing
`assets/`: the stylesheet, the script, the font, the screenshots and the social images. English
screenshots are in `assets/en/`. There is no build step, no framework and no third-party request:
Outfit is self-hosted (SIL OFL, licence in `assets/fonts/`).

* **Design:** the design system and art direction are in `DESIGN.md`; the brand is in
  `../docs/BRAND.md`. Light only, on purpose.
* **Screenshots:** the real app in its light theme, cropped to the window content:
  * taken with the test flags `--light --lang=fr|en`, plus `--tab=…`, `--palette` or `--locks=…`;
  * the Ports and Files views show a test process listening on 127.0.0.1:3000 and holding
    `V:\devis-atelier-brume.pdf`, on a temporary `subst` drive so no user path appears.
* **Social images:** `assets/og.png` and `assets/og-en.png` are rendered from
  `assets/og-source.html#fr|#en` with headless Chrome at 1200 × 630.
* **Publish:** run the Pages workflow by hand (`.github/workflows/pages.yml`) once Pages is enabled
  with "GitHub Actions" as its source. The page expects to live at
  `https://evanpluchart.github.io/vigie/` (canonical and social links), and every asset path is
  relative.
* **Downloads:** they point to `https://github.com/EvanPluchart/vigie/releases/latest`, which only
  works for visitors once the repository is public and a release is published. macOS and Linux are
  described as early versions (processes only) until they have their full feature set.
