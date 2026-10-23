"""Generate the Vigie logo kit (SVG masters) from exact geometry. usage: gen.py OUT_DIR WORDMARK_TSV"""
import math, sys, pathlib

OUT = pathlib.Path(sys.argv[1]); OUT.mkdir(parents=True, exist_ok=True)
WORD = [l.rstrip("\n").split("\t") for l in open(sys.argv[2], encoding="utf-8") if not l.startswith("#")]

LAGOON, INK, WHITE = "#2EC4B6", "#0E1116", "#FFFFFF"
ANGLE = math.radians(22.5)          # arm angle from vertical
TOP = 38                            # flat cut of both arms
DY = -10                            # optical lift of the whole mark

def f(v): return f"{v:.2f}".rstrip("0").rstrip(".")

def arms(half, end=(100, 140), top=TOP):
    """Left and right arm paths: flat top cut, round end towards the eye."""
    ex, ey = end[0], end[1] + DY
    ux, uy = math.sin(ANGLE), math.cos(ANGLE)          # down the arm
    nx, ny = math.cos(ANGLE) * half, -math.sin(ANGLE) * half
    lx, ly = ex - nx, ey - ny                          # outer edge end
    rx, ry = ex + nx, ey + ny                          # inner edge end
    xl = lx + (top - ly) / uy * ux
    xr = rx + (top - ry) / uy * ux
    left = f"M{f(xl)} {top}H{f(xr)}L{f(rx)} {f(ry)}A{half} {half} 0 0 1 {f(lx)} {f(ly)}Z"
    m = lambda x: f(256 - x)
    right = f"M{m(xl)} {top}H{m(xr)}L{m(rx)} {f(ry)}A{half} {half} 0 0 0 {m(lx)} {f(ly)}Z"
    return left, right

def eye(r, cy=202):
    cy += DY
    return f"M{f(128 - r)} {cy}A{r} {r} 0 1 0 {f(128 + r)} {cy}A{r} {r} 0 1 0 {f(128 - r)} {cy}Z"

REG = dict(half=22, r=22)       # regular drawing
SMALL = dict(half=27, r=27)     # small-size cut, 16-32 px

def mark(arm_fill, eye_fill, cut=REG):
    l, r = arms(cut["half"])
    return (f'<path fill="{arm_fill}" d="{l}"/><path fill="{arm_fill}" d="{r}"/>'
            f'<path fill="{eye_fill}" d="{eye(cut["r"])}"/>')

def svg(name, w, h, body, title="Vigie logo"):
    (OUT / name).write_text(
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img">'
        f'<title>{title}</title>{body}</svg>\n', encoding="utf-8")

TILE = '<rect width="256" height="256" rx="56" fill="{}"/>'

# Symbol
svg("vigie-symbol-black.svg", 256, 256, mark("#000", "#000"), "Vigie symbol")
svg("vigie-symbol-color.svg", 256, 256, mark(INK, LAGOON), "Vigie symbol")
svg("vigie-symbol-reversed.svg", 256, 256, mark(WHITE, LAGOON), "Vigie symbol")
svg("vigie-symbol-small.svg", 256, 256, mark(INK, LAGOON, SMALL), "Vigie symbol, small sizes")
# App icon
svg("vigie-icon.svg", 256, 256, TILE.format(LAGOON) + mark(INK, WHITE), "Vigie app icon")
svg("vigie-icon-small.svg", 256, 256, TILE.format(LAGOON) + mark(INK, WHITE, SMALL), "Vigie app icon, small sizes")
svg("vigie-icon-dark.svg", 256, 256, TILE.format(INK) + mark(LAGOON, WHITE), "Vigie app icon, dark")

# Wordmark: glyphs from Outfit SemiBold; the i dots take the eye colour.
def word(fill, dot, x0, base, s=1):
    out = []
    for ch, _ox, d in WORD:
        subs = [s + "Z" for s in d.split("Z") if s.strip()]
        if ch == "i":
            out.append(f'<path fill="{fill}" d="{subs[0]}"/><path fill="{dot}" d="{"".join(subs[1:])}"/>')
        else:
            out.append(f'<path fill="{fill}" d="{d}"/>')
    return f'<g transform="translate({f(x0)} {f(base)}) scale({s})">{"".join(out)}</g>'

WW = float(open(sys.argv[2], encoding="utf-8").readline().split("width=")[1].split()[0])
svg("vigie-wordmark.svg", math.ceil(WW) + 8, 200, word(INK, LAGOON, 4, 140), "Vigie wordmark")
svg("vigie-wordmark-reversed.svg", math.ceil(WW) + 8, 200, word(WHITE, LAGOON, 4, 140), "Vigie wordmark")
svg("vigie-wordmark-black.svg", math.ceil(WW) + 8, 200, word("#000", "#000", 4, 140), "Vigie wordmark")

# Lockups: x-height of the wordmark matches the gap-to-eye span; baseline on the eye's bottom.
GAP = 56
LW = 256 + GAP + math.ceil(WW) + 8
svg("vigie-lockup-horizontal.svg", LW, 256, TILE.format(LAGOON) + mark(INK, WHITE) + word(INK, LAGOON, 256 + GAP, 176), "Vigie")
svg("vigie-lockup-horizontal-reversed.svg", LW, 256, TILE.format(LAGOON) + mark(INK, WHITE) + word(WHITE, LAGOON, 256 + GAP, 176), "Vigie")
svg("vigie-lockup-symbol.svg", LW, 256, mark(INK, LAGOON) + word(INK, LAGOON, 256 + GAP - 24, 176), "Vigie")
SS = 0.56  # stacked: the wordmark is a bit narrower than the tile
SW = 256
SH = 256 + 48 + round(130 * SS) + 20
svg("vigie-lockup-stacked.svg", SW, SH, TILE.format(LAGOON) + mark(INK, WHITE) + word(INK, LAGOON, (SW - WW * SS) / 2, 256 + 48 + 96 * SS + 4, SS), "Vigie")
print("ok", sorted(p.name for p in OUT.iterdir()))
