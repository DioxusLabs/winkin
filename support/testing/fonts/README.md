# Test fonts

Tests that name a font the machine happens to have are tests of that machine.
These files are the inventory instead: checked in, loaded by name from a
collection holding nothing else, and the same on every platform.

## Ahem.ttf

`Ahem` is the font the web platform tests measure with, and it is here for the
same reason: every number it produces is exact arithmetic.

- Its em square is exactly square, and its ascent and descent make one em
  between them: 0.8 above the baseline and 0.2 below.
- `X` (U+0058) is a filled square exactly one em wide and tall. `p` (U+0070)
  is 0.2em high and one em wide, hanging below the baseline. `É` (U+00C9) is
  the square with a gap. The space (U+0020) is one em wide and empty.
- Every glyph advances exactly one em, so the width of a run is its character
  count times the font size, and a break lands where arithmetic says.

What makes it worth vendoring rather than any other face: its `OS/2` win
metrics, its `hhea` metrics and its typographic metrics are all 0.8 and 0.2
with no line gap. Every [`LineMetricsSource`] reads the same numbers from it,
so a line set in Ahem is the same height whichever platform's text stack a
host is matching. A test written in Ahem measures this crate and nothing else.

Taken from [web-platform-tests](https://github.com/web-platform-tests/wpt/blob/master/fonts/Ahem.ttf).
Ahem is public domain, with a Creative Commons Zero fallback where public
domain ownership is not recognized. Its embedded notice identifies
[the upstream COPYING file](https://dev.w3.org/CSS/fonts/ahem/COPYING).
That notice is reproduced in [COPYING-Ahem](COPYING-Ahem); the CC0 1.0
legal code is in [LICENSE-CC0](LICENSE-CC0). The WPT project license does
not replace this font-specific notice.

[`LineMetricsSource`]: ../../../crates/winkin/src/config.rs

## VsA.ttf to VsF.ttf

Six fonts for variation sequences, each a handful of rectangles, whose
advances say which font drew a cluster and which glyph: the base, or the
variant a sequence maps to. Made by [`make_fonts.py`](make_fonts.py) with fontTools' `FontBuilder`,
from nothing: no outline, table or name is taken from any other
font. They are licensed under MIT OR Apache-2.0; see [LICENSE-MIT](LICENSE-MIT)
and [LICENSE-APACHE](LICENSE-APACHE). The font tests in
`crates/winkin/src/stages/fonts/tests/sequences.rs` pin Chrome 153's layout of
them.

At 1000 units to the em:

| Font | Base glyph | Variants 1, 2, 3 | `.notdef` | `cmap` format 14 |
|---|---|---|---|---|
| VsA | 700 | 720, 740, 760 | 320 | the pairs below, all Non-Default but two Default |
| VsB | 800 | -- | 340 | none |
| VsC | 900 | 920, 940, 960 | 360 | as VsA |
| VsD | 1000 | -- | 380 | none |
| VsE | maps only `x` (400) and the space (200) | -- | 400 | none |
| VsF | 1200 | -- | 420 | the same pairs, each a Default UVS |

VsA to VsD and VsF map U+845B 葛, U+8FBB 辻, U+2229 ∩, U+8279 艹, `0`,
U+F900 豈, U+2764 ❤ and `#`, besides `x` and the space. The pairs: 葛 with
VS17 (variant 1), VS18 (Default), VS241 (variant 2) and VS1 (variant 3); 辻
and 豈 with VS17; ∩ with VS1 and VS2 (variants 1 and 2); 艹 and `0` with VS1;
❤ with VS16 (variant 1) and VS15 (Default); `#` with VS16.
