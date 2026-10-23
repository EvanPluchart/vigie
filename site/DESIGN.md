# Vigie landing page — DESIGN.md

The design system of `site/`, in the DESIGN.md format, like Recto's. Read it before changing the
page; keep the page and this file in step. The brand itself is in `docs/BRAND.md`.

## 1. Visual theme and atmosphere

The lookout post. Vigie watches everything and weighs nothing, so the page is a quiet sheet with one
live thing on it: **the eye**, the lagoon dot that in the app marks where Vigie reports on itself.
* **The headline:** it ends on that dot instead of a full stop.
* **The status strip:** under the hero window, it ticks like the app's own status bar.
* **Section ends:** the dot closes each section heading.

**Light only, on purpose.** French at the root, English in `en/`, same layout, same assets: the
English screenshots are in `assets/en/`.

**One idea carries the page:** the app's cost is the product. The figures get their own Ink field,
in lagoon, at the size of a headline.

**Screenshots** are the real app, in its light theme. They are cropped to the window content and
captured with Vigie's test flags (`--light --lang=fr|en --tab=…`). The ports and file views show a
test process, on 127.0.0.1:3000 and on `V:\devis-atelier-brume.pdf`.

**Fields alternate**, flat:

| Section | Field |
|---|---|
| Hero | Paper, then a Lagoon band behind the app window |
| Cost | Ink |
| Features | Paper and Surface, alternating |
| Comparison | Paper, with the Vigie column in a Lagoon block |
| Questions | Paper |
| Download | Lagoon |
| Footer | Ink |

## 2. Colour palette and roles

| Token | Value | Role |
|---|---|---|
| `--paper` | `#F6F7F4` | Page ground (the app's light background) |
| `--surface` | `#ECEEE9` | One step of tone: alternate sections, open question, table rows |
| `--line` | `#E3E6E1` | The 1px frame of screenshots (the app's border colour) |
| `--ink` | `#0E1116` | All text, the cost field, the footer |
| `--muted` | `#4A5262` | Secondary notes only (7.4:1 on paper) |
| `--lagoon` | `#2EC4B6` | Brand: the eye, the hero band, the download field, the primary button (Ink text, 8.7:1) |
| `--deep` | `#0B7A71` | Lagoon as text on light: links (5.2:1 on white) |

**Lagoon is never text on paper**: 2.2:1, as `docs/BRAND.md` forbids. On Ink it is the figures'
colour (8.7:1).

## 3. Typography

**One family**, Outfit (SIL OFL), the brand's own face. It is self-hosted as a variable latin subset
covering weights 400 to 700.

| Token | Size | Use |
|---|---|---|
| `--t-display` | `clamp(52px, 9vw, 152px)` | The hero sentence; weight 600, tracking -0.045em, leading 0.92 |
| `--t-head` | `clamp(36px, 5vw, 76px)` | Section sentences; weight 600, tracking -0.035em, leading 1.0 |
| `--t-figure` | `clamp(64px, 10vw, 168px)` | The three cost figures; weight 600, tabular |
| `--t-lead` | `clamp(20px, 1.6vw, 25px)` | Leads, table names, questions |
| `--t-body` | `18px` | Running text, leading 1.55, 58ch measure |
| `--t-small` | `15px` | Notes, captions |

**Figures** use tabular numerals, as in the app.

## 4. Components

* **Button:** radius 8 (the app's control radius), 54px tall, Lagoon fill, Ink text, weight 600.
  * Hover: Ink fill, Lagoon text.
  * Press: scale 0.97.
  * Focus: a 3px Ink outline, offset 3px.
  * On the Lagoon field, the button is Ink with Paper text.
* **Navigation:** the lockup, four text links, the language link and one button. No bar
  background, no blur.
* **Screenshots:** radius 12 (the app's panel radius), a 1px `--line` frame, no shadow. Every image
  has `width`, `height` and `alt`.
* **Status strip:** the app's status bar, redrawn in HTML: the eye, the figures, a note. It ticks
  every second within the measured range, and stops under reduced motion.
* **Range chips:** the app's own range bar, redrawn as text: radius 4, the active chip tinted.
* **Comparison table:** no rules, rows separated by a tone step. The Vigie column is one Lagoon
  block with Ink text.
* **Questions:** `<details>`, no rules, the open row on a Surface tile.

## 5. Layout

* **Grid:** 12 columns, outer margins `clamp(16px, 3vw, 40px)`, max width 1440px.
* **Spacing:** sections 150–190px apart on desktop, 96px on a phone.
* **Rhythm:** each section has a different composition from its neighbours:
  * text on the left and the image on the right, then the reverse;
  * two images side by side;
  * a figure row;
  * a table.

## 6. Depth and elevation

None. Surfaces differ by a step in tone: no gradients, glass, glows or shadows.

## 7. Do and don't

**Do:**
* Ink text at full contrast.
* Real screenshots.
* Sentence case.
* French typography: non-breaking spaces before `: ; ? !` and inside `« »`.
* Honest figures, measured and dated.

**Don't:**
* A dark theme.
* Eyebrow labels, numbered steps or thin rules.
* Pill badges, icon cards or emoji.
* Invented proof: users, quotes, press.
* Gradients or stock illustration.

## 8. Responsive behaviour

* **Breakpoints:** 900px and 600px.
* **On a phone:**
  * the hero window takes the full width, with the Lagoon band behind it;
  * feature rows stack, text first;
  * the cost figures stack;
  * the table becomes one block per row.

## 9. Motion

* **On first view:** section parts rise 20px and fade in, staggered by 70ms, once. The hero window
  rises from the Lagoon band.
* **The status strip** ticks.
* **The eye** in the headline breathes once, the dot of a lookout blinking.
* **Reduced motion:** all of it is off under `prefers-reduced-motion`.
* **Without JavaScript:** nothing is hidden.
