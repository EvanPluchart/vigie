# Brand sources

The SVG masters in `assets/brand/` are generated from exact geometry, not drawn by hand. To change the mark, edit `gen.py` and regenerate.

| File | Role |
|---|---|
| `gen.py` | Builds every SVG master (symbol, icon, wordmark, lockups) from the construction in `docs/BRAND.md`. |
| `wordmark.py` | Outlines the word "vigie" from Outfit SemiBold (wght 600) into `wm.tsv`. Needs `fonttools` and the Outfit variable font (SIL OFL 1.1, not committed). |
| `wm.tsv` | Outlined wordmark glyphs, so `gen.py` runs without the font. |
| `ico.py` | Packs the PNG icon set into `icons/vigie.ico`. |
| `ui-mockup.html` | Static mockup of the interface direction (dark and light), rendered to `previews/ui-direction.png`. |

```sh
python wordmark.py Outfit.ttf vigie 600 96 0 > wm.tsv 2> wm.meta   # then prepend wm.meta to wm.tsv
python gen.py .. wm.tsv
```

PNG icons are rendered from `vigie-icon-small.svg` (16 to 48 px) and `vigie-icon.svg` (64 px and up).
