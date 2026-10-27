#!/usr/bin/env python3
# /// script
# requires-python = ">=3.10"
# dependencies = ["fonttools>=4.50", "uharfbuzz>=0.39", "pillow>=10.1"]
# ///
"""Draw the wax logo and app icon: the word "wax" in Fira Sans ExtraBold.

    uv run tools/make_logo.py path/to/FiraSans-ExtraBold.otf

Writes, from the repository's root:

    docs/logo/wax.svg, wax-dark.svg       the word, for light and dark pages
    packaging/linux/icons/hicolor/...     the icon, as SVG and at every size
    crates/app/assets/icon.png            the icon the player puts on its window

The letters are turned into outlines, so nothing needs the font installed to
show them. Fira Sans is free under the SIL Open Font License: it comes with
many Linux systems, and from https://github.com/mozilla/Fira or Google Fonts.
"""

import argparse
import io
import os
import sys

import uharfbuzz as hb
from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont

WORD = "wax"
INK = "#22201d"
CREAM = "#f3efe6"
TRACKING = -0.035  # em, as in the design
ICON = 512  # the icon's canvas
SQUARE = 448  # its rounded square, leaving a margin like other app icons have
RADIUS = 0.22  # of the square
LETTERS = 0.34  # the letters' size, of the square
SIZES = [16, 24, 32, 48, 64, 128, 256, 512]

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def number(v):
    return f"{v:.2f}".rstrip("0").rstrip(".")


class Word:
    """The word set in the font: each letter's outline and where it stands, in font units."""

    def __init__(self, path):
        self.font = TTFont(path)
        self.glyphs = self.font.getGlyphSet()
        self.em = self.font["head"].unitsPerEm
        face = hb.Face(hb.Blob.from_file_path(path))
        buf = hb.Buffer()
        buf.add_str(WORD)
        buf.guess_segment_properties()
        hb.shape(hb.Font(face), buf, {"kern": True})
        self.placed = []  # (glyph name, x, y)
        x = 0
        for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
            self.placed.append((self.font.getGlyphName(info.codepoint), x + pos.x_offset, pos.y_offset))
            x += pos.x_advance + TRACKING * self.em
        bounds = BoundsPen(self.glyphs)
        for name, gx, gy in self.placed:
            self.glyphs[name].draw(TransformPen(bounds, (1, 0, 0, 1, gx, gy)))
        self.box = bounds.bounds  # xmin, ymin, xmax, ymax; y up

    def path(self, scale, left, baseline):
        """SVG path data: font units times scale, the origin moved to (left, baseline), y down."""
        pen = SVGPathPen(self.glyphs, ntos=number)
        for name, gx, gy in self.placed:
            self.glyphs[name].draw(TransformPen(pen, (scale, 0, 0, -scale, left + gx * scale, baseline - gy * scale)))
        return pen.getCommands()

    def centred(self, scale, cx, cy):
        """Where the origin goes for the letters' ink to sit centred on (cx, cy)."""
        xmin, ymin, xmax, ymax = self.box
        return cx - scale * (xmin + xmax) / 2, cy + scale * (ymin + ymax) / 2


def wordmark(word, colour):
    # Drawn at 120 px letters, with a little room so nothing is clipped.
    scale, pad = 120 / word.em, 4
    xmin, ymin, xmax, ymax = word.box
    w, h = (xmax - xmin) * scale + 2 * pad, (ymax - ymin) * scale + 2 * pad
    d = word.path(scale, pad - xmin * scale, pad + ymax * scale)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{number(w)}" height="{number(h)}" '
            f'viewBox="0 0 {number(w)} {number(h)}" role="img" aria-label="wax">'
            f'<path fill="{colour}" d="{d}"/></svg>\n')


def icon_svg(word):
    edge = (ICON - SQUARE) / 2
    scale = LETTERS * SQUARE / word.em
    left, baseline = word.centred(scale, ICON / 2, ICON / 2)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{ICON}" height="{ICON}" viewBox="0 0 {ICON} {ICON}">'
            f'<rect x="{number(edge)}" y="{number(edge)}" width="{SQUARE}" height="{SQUARE}" '
            f'rx="{number(RADIUS * SQUARE)}" fill="{INK}"/>'
            f'<path fill="{CREAM}" d="{word.path(scale, left, baseline)}"/></svg>\n')


def icon_png(word, font_path, size):
    """The icon at one size: drawn four times as large, then scaled down, for smooth edges."""
    big = 4 * size
    k = big / ICON
    image = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    edge = (ICON - SQUARE) / 2 * k
    draw.rounded_rectangle((edge, edge, big - edge, big - edge), radius=RADIUS * SQUARE * k, fill=INK)
    px = LETTERS * SQUARE * k
    font = ImageFont.truetype(font_path, size=px)
    scale = px / word.em
    left, baseline = word.centred(scale, big / 2, big / 2)
    for (name, gx, gy), letter in zip(word.placed, WORD):
        draw.text((left + gx * scale, baseline - gy * scale), letter, font=font, fill=CREAM, anchor="ls")
    out = io.BytesIO()
    image.resize((size, size), Image.Resampling.LANCZOS).save(out, "PNG", optimize=True)
    return out.getvalue()


def write(relative, data):
    path = os.path.join(ROOT, relative)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data.encode() if isinstance(data, str) else data)
    print(relative)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("font", help="FiraSans-ExtraBold.otf or .ttf")
    font_path = parser.parse_args().font
    word = Word(font_path)
    if len(word.placed) != len(WORD):
        sys.exit(f"{font_path} sets {WORD!r} as {len(word.placed)} glyphs; expected {len(WORD)}")

    write("docs/logo/wax.svg", wordmark(word, INK))
    write("docs/logo/wax-dark.svg", wordmark(word, CREAM))
    write("packaging/linux/icons/hicolor/scalable/apps/wax.svg", icon_svg(word))
    for size in SIZES:
        png = icon_png(word, font_path, size)
        write(f"packaging/linux/icons/hicolor/{size}x{size}/apps/wax.png", png)
        if size == 256:
            write("crates/app/assets/icon.png", png)


if __name__ == "__main__":
    main()
