# Landing page

Static page for evanpluchart.fr (decision 0009): French, no tracking, no external scripts or
fonts. To publish it:

1. Run `cargo xtask dist`, then copy `dist/Vigie-<version>-setup.exe` and
   `dist/Vigie-<version>-portable.zip` into a `telechargements/` folder next to `index.html`.
   Update the version in the two links of `index.html` if it changed.
2. Upload the folder (`index.html`, `img/`, `telechargements/`) to `evanpluchart.fr/vigie/`.

The screenshots come from Vigie's own capture flags (`--lang=fr`, `--tab=…`). None shows
personal data: the Ports view was captured on a test process listening on 127.0.0.1:3000.
