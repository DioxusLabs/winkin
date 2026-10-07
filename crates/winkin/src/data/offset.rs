//! The one position type: a byte in the content's text.

use core::fmt;

/// A byte position in the content's text.
///
/// It is not an id: it indexes bytes, not a table of records. The builder
/// caps the text at [`MAX`](Self::MAX) bytes as it appends. So every
/// position fits in the `u32` with room to spare, and a sum of two never
/// overflows it.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub(crate) struct TextOffset(u32);

impl TextOffset {
    /// The most bytes the text may hold: 2^30 − 1.
    ///
    /// Well inside `u32`, so that an offset plus a length taken from the text
    /// is still one, and small enough that a cluster table indexed by byte
    /// would still fit in memory.
    pub(crate) const MAX: usize = (1 << 30) - 1;

    /// The position `at` bytes into the text, which the calling code knows
    /// is within the text.
    ///
    /// `at` is the text's length or a position already in it. Debug builds
    /// check it, and a failure is a bug in the crate, never something a
    /// caller did: the builder refuses text past [`MAX`](Self::MAX) before
    /// writing it. In release a value past `MAX` is clamped to it, which keeps
    /// every offset in order.
    #[inline]
    pub(crate) fn new(at: usize) -> Self {
        debug_assert!(at <= Self::MAX, "TextOffset out of range");
        Self(at.min(Self::MAX) as u32)
    }

    /// The position as an index into the text.
    #[inline]
    pub(crate) fn get(self) -> usize {
        self.0 as usize
    }

    /// The position as the shaper labels a character's cluster: a `u32`,
    /// which every offset the text can hold fits.
    #[inline]
    pub(crate) fn to_u32(self) -> u32 {
        self.0
    }

    /// The position the shaper's cluster label `raw` names, clamped to
    /// [`MAX`](Self::MAX).
    ///
    /// The shaper hands back only labels it was given, so the clamp only
    /// guards against a label it made up.
    #[inline]
    pub(crate) fn from_u32(raw: u32) -> Self {
        Self(raw.min(Self::MAX as u32))
    }
}

impl fmt::Debug for TextOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}
