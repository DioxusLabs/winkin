//! Text direction and bidi isolation values.

use crate::style::same::same_by_value;

/// `direction`: the inline base direction.
///
/// Also determines which side is the start side.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Direction {
    /// Left to right. The initial value.
    #[default]
    Ltr,
    /// Right to left.
    Rtl,
}

impl Direction {
    /// Returns an element's `start` and `end` values in line order, line-left first.
    ///
    /// Left to right keeps them as they are, and right to left swaps them.
    /// The same swap turns line-left and line-right values back into start
    /// and end.
    #[inline]
    pub(crate) fn line_order<T>(self, start: T, end: T) -> (T, T) {
        match self {
            Self::Ltr => (start, end),
            Self::Rtl => (end, start),
        }
    }
}

/// `unicode-bidi`. Not inherited.
///
/// Controls are added to bidi input at box boundaries, not to layout text.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum UnicodeBidi {
    /// No effect on bidi. The initial value.
    #[default]
    Normal,
    /// An embedding level in the element's direction.
    Embed,
    /// The content resolved in isolation.
    Isolate,
    /// Every character forced to the element's direction.
    BidiOverride,
    /// Isolated, and forced to the element's direction.
    IsolateOverride,
    /// Isolated, with the direction of its first strong character.
    Plaintext,
}

same_by_value!(Direction, UnicodeBidi);
