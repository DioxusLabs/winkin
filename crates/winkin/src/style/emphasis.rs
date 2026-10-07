//! Text emphasis values.

use super::WritingMode;
use crate::style::same::same_by_value;
use crate::unicode::EmphasisClass;

/// The side of horizontal text `text-emphasis` marks are set on.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum EmphasisSide {
    /// Above. The initial value.
    #[default]
    Over,
    /// Below.
    Under,
}

/// The side of vertical text `text-emphasis` marks are set on.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum EmphasisVerticalSide {
    /// To the right. The initial value.
    #[default]
    Right,
    /// To the left.
    Left,
}

impl EmphasisVerticalSide {
    /// Returns the side of a vertical typographic line it names.
    ///
    /// `right` is over in `vertical-rl` and `vertical-lr` alike. Their lines
    /// are over on the right whichever way they stack.
    pub(crate) fn line_side(self) -> EmphasisSide {
        match self {
            Self::Right => EmphasisSide::Over,
            Self::Left => EmphasisSide::Under,
        }
    }
}

/// `text-emphasis-position`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct EmphasisPosition {
    /// The side in horizontal text.
    pub side: EmphasisSide,
    /// The side in vertical text.
    pub vertical_side: EmphasisVerticalSide,
}

impl EmphasisPosition {
    /// Returns the side of the line the marks take in a block of `writing_mode`.
    ///
    /// CSS Text Decoration 4 reads `over` and `under` in horizontal
    /// typographic modes, and `right` and `left` in vertical ones.
    /// - `vertical-rl` and `vertical-lr` use the vertical side, as Chrome 153
    ///   does.
    /// - `horizontal-tb` and the sideways modes use the horizontal side. The
    ///   sideways modes are horizontal typographic modes turned whole. Chrome
    ///   still reads `right` and `left` in them, and this doesn't copy that.
    pub(crate) fn side(self, writing_mode: WritingMode) -> EmphasisSide {
        if writing_mode.is_vertical_typographic() {
            self.vertical_side.line_side()
        } else {
            self.side
        }
    }
}

/// `text-emphasis-skip`: the characters that take no mark.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct EmphasisSkip(u8);

impl EmphasisSkip {
    /// `spaces`: word separators.
    pub const SPACES: Self = Self(1 << 0);
    /// `punctuation`.
    pub const PUNCTUATION: Self = Self(1 << 1);
    /// `symbols`.
    pub const SYMBOLS: Self = Self(1 << 2);
    /// `narrow`: characters narrower than the mark.
    pub const NARROW: Self = Self(1 << 3);
    /// `spaces punctuation`. The initial value.
    pub const INITIAL: Self = Self(Self::SPACES.0 | Self::PUNCTUATION.0);

    /// Returns `true` if every kind in `other` is skipped.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the kinds skipped by either.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether a character of emphasis class `class` takes no mark.
    ///
    /// `wide` says whether its East Asian width is wide or fullwidth. It
    /// takes no mark where its class is skipped, or where `narrow` is
    /// skipped and the character isn't wide.
    #[inline]
    pub(crate) fn skips(self, class: EmphasisClass, wide: bool) -> bool {
        let skipped = match class {
            EmphasisClass::Marked => false,
            EmphasisClass::Space => self.contains(Self::SPACES),
            EmphasisClass::Punctuation => self.contains(Self::PUNCTUATION),
            EmphasisClass::Symbol => self.contains(Self::SYMBOLS),
            EmphasisClass::Never => true,
        };
        let narrow = self.contains(Self::NARROW) && !wide;
        skipped || narrow
    }
}

impl Default for EmphasisSkip {
    fn default() -> Self {
        Self::INITIAL
    }
}

/// Layout properties for `text-emphasis`.
///
/// Specifies which text receives marks and their positions. The host supplies
/// mark shapes and colors when painting.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct TextEmphasis {
    /// Whether `text-emphasis-style` sets marks at all.
    ///
    /// The marks take room on their side, which can make a line taller.
    pub marks: bool,
    /// `text-emphasis-position`.
    pub position: EmphasisPosition,
    /// `text-emphasis-skip`.
    pub skip: EmphasisSkip,
}

impl TextEmphasis {
    /// No marks. The initial value.
    pub const NONE: Self = Self {
        marks: false,
        position: EmphasisPosition {
            side: EmphasisSide::Over,
            vertical_side: EmphasisVerticalSide::Right,
        },
        skip: EmphasisSkip::INITIAL,
    };
}

same_by_value!(TextEmphasis);
