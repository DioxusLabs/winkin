//! Allocation tests: every stage allocates nothing once warm.
//!
//! A global allocator counts allocations per thread, so tests running
//! alongside cannot disturb each other's numbers. A buffer that grows on only
//! one path shows up here.
//!
//! This file holds the shared hosts. Each child covers one stage or
//! feature: the builder, analysis, font selection, shaping, measurement,
//! line breaking, line layout and reading back, then mixed directions, fonts
//! in full, shifts, CJK typography, ruby, the first line, cut lines,
//! `text-wrap-style`, vertical text and text on paths.
//!
//! The one exception is ICU, which allocates for Thai and the other scripts
//! whose line breaks and words need its dictionaries or its LSTM, and, with
//! the `dictionaries` feature, for Chinese and Japanese words. The tests pin
//! it, so a change in it is seen.

extern crate alloc;

// Fonts built in memory, and Ahem.
use testing::fonts as test_fonts;

use test_fonts::TestFont;
use testing::allocator::{Counting, count_allocations};

#[global_allocator]
static GLOBAL: Counting = Counting;

// What the stages' modules below share: their text, a relayout, a font and
// a host of floats.

/// `line` as `lines` paragraphs.
fn text(line: &str, lines: usize) -> String {
    vec![line; lines].join("\n")
}

/// Breaks `layout` into lines at each of `widths` in turn, and reads every
/// line back as a host would. Returns how many bytes the lines held.
fn relayout(layout: &mut winkin::Layout, cx: &mut winkin::Context, widths: &[f32]) -> usize {
    let mut read = 0;
    for &width in widths {
        layout.break_lines(cx, winkin::Area::new(width), &mut winkin::NoExclusions);
        for line in layout.lines() {
            read += line.text_range().len();
            let _ = line.metrics();
        }
    }
    read
}

/// Arabic letters and a space, with joining forms of seven of them.
fn arabic() -> TestFont {
    let mut font = TestFont::new("Test Arabic", &[(0x20, 0x20), (0x621, 0x64A)]);
    font.joining = "\u{628}\u{62A}\u{633}\u{644}\u{645}\u{646}\u{64A}"
        .chars()
        .collect();
    font
}

/// A host that places floats in a list it keeps its room for. Each float as high as asked, at its side's edge of
/// the band, and where the host `lowers` floats, never above one before it
/// and lower where it does not fit; each band what the floats leave of the
/// area.
struct Floats {
    width: f32,
    placed: Vec<(winkin::FloatSide, f32, f32, f32, f32)>,
    lowers: bool,
}

impl Floats {
    fn room(&self, start: f32, end: f32) -> (f32, f32) {
        let (mut left, mut right) = (0.0_f32, self.width);
        for &(side, from, to, top, bottom) in &self.placed {
            if top < end && start < bottom {
                match side {
                    winkin::FloatSide::Left => left = left.max(to),
                    winkin::FloatSide::Right => right = right.min(from),
                }
            }
        }
        (left, right.max(left))
    }
}

impl winkin::Exclusions for Floats {
    fn band(&self, _line: usize, block: winkin::BlockExtents) -> winkin::InlineExtents {
        let (left, right) = self.room(block.start, block.end);
        winkin::InlineExtents { left, right }
    }

    fn below(&self, top: f32) -> Option<f32> {
        self.placed
            .iter()
            .map(|placed| placed.4)
            .filter(|&bottom| bottom > top)
            .reduce(f32::min)
    }

    fn place(&mut self, float: winkin::FloatRequest) -> winkin::PlacedFloat {
        let mut top = float.block_start;
        if self.lowers {
            top = self
                .placed
                .iter()
                .map(|placed| placed.3)
                .fold(top, f32::max);
            loop {
                let (left, right) = self.room(top, top + float.block_size);
                if right - left >= float.inline_size {
                    break;
                }
                match self.below(top) {
                    Some(next) => top = next,
                    None => break,
                }
            }
        }
        let (left, right) = self.room(top, top + float.block_size);
        let from = match float.side {
            winkin::FloatSide::Left => left,
            winkin::FloatSide::Right => right - float.inline_size,
        };
        let to = from + float.inline_size;
        let bottom = top + float.block_size;
        self.placed.push((float.side, from, to, top, bottom));
        winkin::PlacedFloat {
            inline: winkin::InlineExtents {
                left: from,
                right: to,
            },
            block: winkin::BlockExtents {
                start: top,
                end: bottom,
            },
        }
    }

    fn checkpoint(&self) -> winkin::ExclusionsCheckpoint {
        winkin::ExclusionsCheckpoint(self.placed.len() as u64)
    }

    fn rewind(&mut self, to: winkin::ExclusionsCheckpoint) {
        self.placed
            .truncate(usize::try_from(to.0).unwrap_or(usize::MAX));
    }
}

mod analyzing;
mod bidi_and_boxes;
mod breaking;
mod building;
mod cjk_typography;
mod cut;
mod first_line;
mod fonts_in_full;
mod laying_out;
mod measuring;
mod ruby_and_emphasis;
mod selecting;
mod shaping;
mod shifts_atomics_and_trim;
mod text_on_paths;
mod vertical_text;
mod wrapping;
