//! The extent: how far something reaches over and under a baseline.

use crate::stages::content::Atomic;
use crate::stages::fonts::LineBaseline;
use crate::style::WritingMode;
use crate::unit::LayoutUnit;

/// A pair of distances either side of a baseline, on layout's grid: 8 bytes.
///
/// The ascent is how far it reaches over the baseline, and the descent how
/// far under. Each is positive away from the baseline. Either may be
/// negative: that is how a line height under a font's own makes lines
/// overlap rather than clip. A line box's extent is the union of the strut
/// and the extents of what is on it.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Extent {
    ascent: LayoutUnit,
    descent: LayoutUnit,
}

impl Extent {
    /// What takes no room across the line: a closing edge, a float's anchor,
    /// an item out of the line's flow.
    ///
    /// Both sides are the grid's least, as in Chrome's `FontHeight::Empty()`.
    /// So it is the identity of [`unite`](Self::unite). Something that holds
    /// nothing doesn't floor a line of negative leading at zero.
    pub(crate) const NONE: Self = Self {
        ascent: LayoutUnit::MIN,
        descent: LayoutUnit::MIN,
    };

    /// Returns an extent reaching `ascent` over the baseline and `descent`
    /// under.
    pub(crate) fn new(ascent: LayoutUnit, descent: LayoutUnit) -> Self {
        Self { ascent, descent }
    }

    /// Returns the margin box of `atomic` against its baseline, in a block
    /// of `writing_mode` whose lines are set on `baseline`.
    ///
    /// This follows Blink's `BaselineMetrics`:
    /// - An atomic inline with a baseline of its own sits on it.
    /// - One without sits its margin box's under edge on the line's
    ///   baseline, as a replaced element does.
    /// - In a centred vertical line, one without is centred instead, with
    ///   the odd 1/64 over (`SynthesizeMetrics`, `size - size / 2` over).
    ///
    /// Its baseline is measured down from its block-start edge. Where lines
    /// flip, in `vertical-lr` only, that edge is the under side: the left,
    /// with the lines' over side on the right. There the baseline is that
    /// far over the under edge, as Blink's `IsFlippedLines` turns it. In
    /// `sideways-lr` the left is the over side, and the baseline is that far
    /// under it (Chrome 153).
    ///
    /// It lives here, not in the content, because the content stage doesn't
    /// name extents.
    pub(crate) fn from_atomic(
        atomic: &Atomic,
        writing_mode: WritingMode,
        baseline: LineBaseline,
    ) -> Self {
        // Chrome truncates the margins and the size onto the grid. The row
        // holds the margins already. The baseline is a position in the
        // host's layout, not a given length, so it rounds to the nearest.
        let (over, under) = atomic.margins_across;
        let size = atomic.size;
        let block = LayoutUnit::from_px_truncated(size.block);
        match size.baseline {
            Some(baseline) => {
                let baseline = LayoutUnit::from_px(baseline);
                let (ascent, descent) = if writing_mode.flips_lines() {
                    (block - baseline, baseline)
                } else {
                    (baseline, block - baseline)
                };
                Self::new(over + ascent, descent + under)
            }
            None => {
                let size = over + block + under;
                match baseline {
                    LineBaseline::Alphabetic => Self::new(size, LayoutUnit::ZERO),
                    LineBaseline::Central => Self::new(size - size.half(), size.half()),
                }
            }
        }
    }

    /// Returns how far it reaches over the baseline.
    pub(crate) fn ascent(self) -> LayoutUnit {
        self.ascent
    }

    /// Returns how far it reaches under the baseline.
    pub(crate) fn descent(self) -> LayoutUnit {
        self.descent
    }

    /// Returns the farther of the two on each side, which is how a line box
    /// grows.
    pub(crate) fn unite(self, other: Self) -> Self {
        Self {
            ascent: self.ascent.max(other.ascent),
            descent: self.descent.max(other.descent),
        }
    }

    /// Returns whether it is [`NONE`](Self::NONE), taking no room.
    pub(crate) fn is_none(self) -> bool {
        self == Self::NONE
    }

    /// Returns the extent with its baseline raised by `up`.
    ///
    /// A negative `up` lowers it. The ascent shrinks by `up` and the descent
    /// grows by it, as Blink's `FontHeight::Move` moves by the opposite.
    /// [`NONE`](Self::NONE) stays `NONE`.
    pub(crate) fn raised(self, up: LayoutUnit) -> Self {
        if self.is_none() {
            return self;
        }
        Self {
            ascent: self.ascent + up,
            descent: self.descent - up,
        }
    }

    /// Returns how far it reaches from edge to edge: ascent plus descent.
    ///
    /// [`NONE`](Self::NONE) has zero height.
    pub(crate) fn height(self) -> LayoutUnit {
        if self.is_none() {
            LayoutUnit::ZERO
        } else {
            self.ascent + self.descent
        }
    }

    /// Returns the extent, or zero on both sides where it is
    /// [`NONE`](Self::NONE).
    ///
    /// Blink takes an empty `FontHeight` as zero before it aligns one, and
    /// Chrome does the same for an empty annotation line. A line box of
    /// `NONE` then reaches nowhere from its baseline.
    #[inline]
    pub(crate) fn zero_if_none(self) -> Self {
        if self.is_none() {
            Self::new(LayoutUnit::ZERO, LayoutUnit::ZERO)
        } else {
            self
        }
    }

    /// Returns how tall a line box of this extent is: its
    /// [`height`](Self::height), never negative.
    ///
    /// A line box of negative leading, or one that holds nothing, takes no
    /// room across the block.
    #[inline]
    pub(crate) fn box_height(self) -> LayoutUnit {
        self.height().max(LayoutUnit::ZERO)
    }

    /// Returns its top and bottom across the block, around a baseline at
    /// `baseline`.
    ///
    /// Both are at the baseline for [`NONE`](Self::NONE).
    #[inline]
    pub(crate) fn around(self, baseline: LayoutUnit) -> (LayoutUnit, LayoutUnit) {
        let extent = self.zero_if_none();
        (baseline - extent.ascent, baseline + extent.descent)
    }
}
