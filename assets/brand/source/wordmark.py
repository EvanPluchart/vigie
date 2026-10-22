"""Outline a word from a variable font into SVG path data. usage: wordmark.py font.ttf WORD WGHT HEIGHT_PX TRACKING"""
import sys
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

font_path, word, wght, x_height_px, tracking = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4]), float(sys.argv[5])
font = instantiateVariableFont(TTFont(font_path), {"wght": wght})
gs = font.getGlyphSet()
cmap = font.getBestCmap()
xh = font["OS/2"].sxHeight
scale = x_height_px / xh
x = 0.0
parts = []
for ch in word:
    name = cmap[ord(ch)]
    pen = SVGPathPen(gs)
    # flip y: baseline at 0, y grows down in SVG
    gs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, x, 0)))
    parts.append((ch, round(x, 2), pen.getCommands()))
    x += gs[name].width * scale + tracking
print(f"# width={x - tracking:.2f} xheight={x_height_px} ascender={font['hhea'].ascent*scale:.2f}", file=sys.stderr)
for ch, ox, d in parts:
    print(f"{ch}\t{ox}\t{d}")
