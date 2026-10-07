//! Typed ids, and the only casts between an id and an index.

use core::fmt;
use core::ops::Range;

/// A typed index into a [`Table`](super::Table).
///
/// Implemented only through [`define_id!`], which is what keeps every
/// conversion between an id and a `usize` in this module.
pub(crate) trait Id: Copy + Eq + Ord + fmt::Debug {
    /// The largest index this id can name.
    const MAX: usize;

    /// The id naming `index`, or `None` if it is past [`MAX`](Self::MAX).
    ///
    /// Anything that turns a caller's input into an id asks this, and refuses
    /// rather than truncates.
    fn try_new(index: usize) -> Option<Self>;

    /// The id naming `index`, which the calling code knows is in range.
    ///
    /// `index` is an id or length of a [`Table`](super::Table), or one plus a
    /// count bounded by the same table. A table never holds more than `MAX`
    /// items, so its length is always an id this can make.
    ///
    /// Never panics. Debug builds check the range, and a failure there is a
    /// bug in the crate, never something a caller did. In release a value
    /// past `MAX` wraps. Anything a caller controls goes through
    /// [`try_new`](Self::try_new) instead.
    fn new(index: usize) -> Self;

    /// The index this id names.
    fn get(self) -> usize;
}

/// Defines an id newtype over an unsigned integer and implements [`Id`] for it.
///
/// ```ignore
/// define_id! {
///     /// Names a cluster.
///     pub(crate) struct ClusterId(u32);
/// }
/// ```
macro_rules! define_id {
    ($(#[$meta:meta])* $vis:vis struct $name:ident($repr:ty);) => {
        $(#[$meta])*
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        #[repr(transparent)]
        $vis struct $name($repr);

        impl $crate::data::Id for $name {
            const MAX: usize = <$repr>::MAX as usize;

            #[inline]
            fn try_new(index: usize) -> Option<Self> {
                (index <= Self::MAX).then(|| Self(index as $repr))
            }

            #[inline]
            fn new(index: usize) -> Self {
                debug_assert!(
                    index <= Self::MAX,
                    concat!(stringify!($name), " out of range")
                );
                Self(index as $repr)
            }

            #[inline]
            fn get(self) -> usize {
                self.0 as usize
            }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}
pub(crate) use define_id;

/// A range of ids that walks its ids.
///
/// `Range<I>` is not an iterator for an id (that needs the unstable `Step`),
/// so this walks it.
pub(crate) trait IdRange<I> {
    /// Returns the ids from the range's start up to its end, in order.
    fn ids(self) -> impl DoubleEndedIterator<Item = I> + ExactSizeIterator;
}

impl<I: Id> IdRange<I> for Range<I> {
    #[inline]
    fn ids(self) -> impl DoubleEndedIterator<Item = I> + ExactSizeIterator {
        (self.start.get()..self.end.get()).map(I::new)
    }
}

/// Packs `index`, an index or a count, into a `u32` for a record, saturating.
///
/// Every table is indexed by an id of at most a `u32`, so no index is past
/// it. A count past it saturates to `u32::MAX`.
#[inline]
pub(crate) fn index_to_u32(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// Unpacks an index or a count packed by [`index_to_u32`].
///
/// It saturates on a target whose `usize` is narrower than a `u32`, where a
/// value that large is past the end of anything.
#[inline]
pub(crate) fn u32_to_index(packed: u32) -> usize {
    usize::try_from(packed).unwrap_or(usize::MAX)
}
