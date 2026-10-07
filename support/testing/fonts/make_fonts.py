"""Writes the variation-sequence test fonts, VsA.ttf to VsF.ttf, beside this script.

    python support/testing/fonts/make_fonts.py

Six fonts, built from nothing (rectangles for glyphs), whose advances say
which font drew a cluster and which glyph it drew. At the probe's 50px, with
1000 units to the em, a font's glyphs are:

| Font | Base glyph | Variant 1, 2, 3 | .notdef |
|---|---|---|---|
| VsA | 35px | 36, 37, 38 | 16 |
| VsB | 40px | -- | 17 |
| VsC | 45px | 46, 47, 48 | 18 |
| VsD | 50px | -- | 19 |
| VsE | (maps only x and space) | -- | 20 |
| VsF | 60px | every pair a Default UVS: the base glyph | 22 |

VsA to VsD and VsF map the same bases; only VsA, VsC and VsF have `cmap`
format 14. VsA and VsC map these pairs to variant glyphs (Non-Default UVS),
except where marked default:

| Base | Selector | Glyph | Blink's `Character::IsVariationSequence` |
|---|---|---|---|
| U+845B 葛 | VS17 U+E0100 | variant 1 | yes (ideographic) |
| U+845B | VS18 U+E0101 | default | yes |
| U+845B | VS241 U+E01E0 | variant 2 | yes (any VS17-VS256 after an ideograph) |
| U+845B | VS1 U+FE00 | variant 3 | no (not in StandardizedVariants) |
| U+8FBB 辻 | VS17 U+E0100 | variant 1 | yes |
| U+2229 ∩ | VS1 U+FE00 | variant 1 | yes (standardized) |
| U+2229 | VS2 U+FE01 | variant 2 | no |
| U+8279 艹 | VS1 U+FE00 | variant 1 | yes (standardized, CJK compatibility) |
| U+0030 0 | VS1 U+FE00 | variant 1 | yes (standardized) |
| U+F900 豈 | VS17 U+E0100 | variant 1 | no (canonically decomposable) |
| U+2764 ❤ | VS16 U+FE0F | variant 1 | yes (emoji) |
| U+2764 | VS15 U+FE0E | default | yes (emoji) |
| U+0023 # | VS16 U+FE0F | variant 1 | yes (emoji: `#` has Emoji=Yes) |

VsF lists the same pairs, every one as a Default UVS. The fonts are the
probe's own, made here, and licensed under MIT OR Apache-2.0; see
`fonts/README.md`.
"""
import os

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

HERE = os.path.dirname(os.path.abspath(__file__))
UPEM = 1000

BASES = [0x845B, 0x8FBB, 0x2229, 0x8279, 0x0030, 0xF900, 0x2764, 0x0023]

# (base, selector, variant index or None for a Default UVS)
PAIRS = [
    (0x845B, 0xE0100, 1),
    (0x845B, 0xE0101, None),
    (0x845B, 0xE01E0, 2),
    (0x845B, 0xFE00, 3),
    (0x8FBB, 0xE0100, 1),
    (0x2229, 0xFE00, 1),
    (0x2229, 0xFE01, 2),
    (0x8279, 0xFE00, 1),
    (0x0030, 0xFE00, 1),
    (0xF900, 0xE0100, 1),
    (0x2764, 0xFE0F, 1),
    (0x2764, 0xFE0E, None),
    (0x0023, 0xFE0F, 1),
]

# name: (index f, maps the bases, pairs: None, "variants" or "defaults")
FONTS = {
    "VsA": (1, True, "variants"),
    "VsB": (2, True, None),
    "VsC": (3, True, "variants"),
    "VsD": (4, True, None),
    "VsE": (5, False, None),
    "VsF": (6, True, "defaults"),
}


def rect(width, height):
    pen = TTGlyphPen(None)
    left, right = 40, max(80, width - 40)
    pen.moveTo((left, 0))
    pen.lineTo((left, height))
    pen.lineTo((right, height))
    pen.lineTo((right, 0))
    pen.closePath()
    return pen.glyph()


def build(name, f, maps_bases, pairs):
    base_adv = 600 + 100 * f
    glyphs = {".notdef": (300 + 20 * f, rect(300 + 20 * f, 600)), "space": (200, TTGlyphPen(None).glyph())}
    glyphs["x"] = (400, rect(400, 500))
    cmap = {0x20: "space", 0x78: "x"}
    if maps_bases:
        for base in BASES:
            glyph = "base%04X" % base
            glyphs[glyph] = (base_adv, rect(base_adv, 700))
            cmap[base] = glyph
    uvs = []
    if pairs == "variants":
        for k in (1, 2, 3):
            glyphs["var%d" % k] = (base_adv + 20 * k, rect(base_adv + 20 * k, 300 + 100 * k))
        for base, selector, k in PAIRS:
            uvs.append((base, selector, None if k is None else "var%d" % k))
    elif pairs == "defaults":
        for base, selector, _ in PAIRS:
            uvs.append((base, selector, None))

    order = list(glyphs)
    fb = FontBuilder(UPEM, isTTF=True)
    fb.setupGlyphOrder(order)
    fb.setupCharacterMap(cmap, uvs=uvs or None)
    fb.setupGlyf({g: glyphs[g][1] for g in order})
    fb.setupHorizontalMetrics({g: (glyphs[g][0], 0) for g in order})
    fb.setupHorizontalHeader(ascent=800, descent=-200)
    fb.setupNameTable(
        {"familyName": name, "styleName": "Regular", "fullName": name, "psName": name + "-Regular"}
    )
    fb.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
    fb.setupPost()
    fb.save(os.path.join(HERE, name + ".ttf"))
    print("wrote", name, "base", base_adv, "cmap14 pairs", len(uvs))


def main():
    for name, (f, maps_bases, pairs) in FONTS.items():
        build(name, f, maps_bases, pairs)


if __name__ == "__main__":
    main()
