//! A bit per id of a table, kept beside it, and the same with a count of
//! the bits set before each word.

use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;
use core::ops::Range;

use super::{HeapBytes, Id, make_room};

/// One bit per id of a table, kept beside it: bit `i % 64` of word `i / 64`.
///
/// A reader searches it for the ids whose bit is set, 64 at a word, as the
/// breaker searches the clusters for break opportunities. The owner sets each
/// bit as it pushes the item beside it, so the two stay in step. The bit
/// arithmetic lives here with the other index casts.
pub(crate) struct BitTable<I> {
    words: Vec<u64>,
    id: PhantomData<fn(I) -> I>,
}

impl<I: Id> BitTable<I> {
    /// An empty table, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            words: Vec::new(),
            id: PhantomData,
        }
    }

    /// Removes every bit and keeps the allocation.
    pub(crate) fn clear(&mut self) {
        self.words.clear();
    }

    /// Makes room for the bits of `count` ids in all, as
    /// [`Table::reserve`](super::Table::reserve) makes room for their items.
    pub(crate) fn reserve(&mut self, count: usize) {
        make_room(&mut self.words, count.min(I::MAX).div_ceil(64));
    }

    /// Gives `id` its bit, set where `set` is true.
    ///
    /// `id` is the one the table beside this has just handed out, the next
    /// after every id given a bit so far.
    pub(crate) fn push(&mut self, id: I, set: bool) {
        let at = id.get();
        if at.is_multiple_of(64) {
            self.words.push(0);
        }
        if set && let Some(word) = self.words.last_mut() {
            *word |= 1u64 << (at % 64);
        }
    }

    /// The last id of `range` whose bit is set, scanning back a word of 64
    /// at a time.
    pub(crate) fn last(&self, range: Range<I>) -> Option<I> {
        let (from, below) = (range.start.get(), range.end.get());
        if below <= from {
            return None;
        }
        let mut at = below - 1;
        loop {
            let index = at / 64;
            let word = self.words.get(index).copied().unwrap_or(0);
            // The bits at and below `at` in its word.
            let masked = word & (u64::MAX >> (63 - at % 64));
            if masked != 0 {
                let found = index * 64 + (63 - masked.leading_zeros() as usize);
                return (found >= from).then(|| I::new(found));
            }
            at = (index * 64).checked_sub(1)?;
            if at < from {
                return None;
            }
        }
    }

    /// The first id of `range` whose bit is set, scanning forward a word of
    /// 64 at a time.
    pub(crate) fn first(&self, range: Range<I>) -> Option<I> {
        let (from, below) = (range.start.get(), range.end.get());
        let mut at = from;
        while at < below {
            let index = at / 64;
            let word = self.words.get(index)?;
            // The bits at and above `at` in its word.
            let masked = word & (u64::MAX << (at % 64));
            if masked != 0 {
                let found = index * 64 + masked.trailing_zeros() as usize;
                return (found < below).then(|| I::new(found));
            }
            at = (index + 1) * 64;
        }
        None
    }

    /// The bytes the bits take, capacity aside.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.words.len() * size_of::<u64>()
    }
}

impl<I> HeapBytes for BitTable<I> {
    fn heap_bytes(&self) -> usize {
        self.words.heap_bytes()
    }
}

impl<I: Id> Default for BitTable<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I> fmt::Debug for BitTable<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.words).finish()
    }
}

/// A [`BitTable`] set in increasing order, with a count of the bits set
/// before each word of 64.
///
/// Counting the set bits at or before an id takes one read and one word's
/// popcount, however many ids there are. An owner whose records each start at
/// a set bit uses it to find the record holding an id without searching. The
/// shaping runs, each starting at a cluster, work this way. The record is the
/// count at or before the id, less one.
pub(crate) struct RankedBitTable<I> {
    bits: BitTable<I>,
    /// Before each word, how many bits the words before it have set.
    before: Vec<u32>,
}

impl<I: Id> RankedBitTable<I> {
    /// An empty table, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            bits: BitTable::new(),
            before: Vec::new(),
        }
    }

    /// Removes every bit and keeps the allocation.
    pub(crate) fn clear(&mut self) {
        self.bits.clear();
        self.before.clear();
    }

    /// Makes room for the bits of `count` ids in all, and their counts, as
    /// [`Table::reserve`](super::Table::reserve) makes room for a table's
    /// items.
    pub(crate) fn reserve(&mut self, count: usize) {
        self.bits.reserve(count);
        make_room(&mut self.before, count.min(I::MAX).div_ceil(64));
    }

    /// Sets `id`'s bit, leaving every id between the last set and it unset.
    ///
    /// Ids are set in increasing order, so a word is finished once an id past
    /// it is set, and its count is recorded then.
    pub(crate) fn set(&mut self, id: I) {
        let at = id.get();
        let index = at / 64;
        let words = &mut self.bits.words;
        while words.len() <= index {
            let counted = match (self.before.last(), words.last()) {
                (Some(&before), Some(word)) => before.saturating_add(word.count_ones()),
                _ => 0,
            };
            words.push(0);
            self.before.push(counted);
        }
        if let Some(word) = words.get_mut(index) {
            *word |= 1u64 << (at % 64);
        }
    }

    /// How many ids at or before `id` have their bit set.
    #[inline]
    pub(crate) fn count_through(&self, id: I) -> usize {
        let at = id.get();
        let index = at / 64;
        let words = &self.bits.words;
        match (words.get(index), self.before.get(index)) {
            (Some(&word), Some(&before)) => {
                // The bits at and below `at` in its word.
                let held = word & (u64::MAX >> (63 - at % 64));
                usize::try_from(before)
                    .unwrap_or(usize::MAX)
                    .saturating_add(held.count_ones() as usize)
            }
            // Past the last word: every bit set.
            _ => match (words.last(), self.before.last()) {
                (Some(&word), Some(&before)) => usize::try_from(before)
                    .unwrap_or(usize::MAX)
                    .saturating_add(word.count_ones() as usize),
                _ => 0,
            },
        }
    }
}

impl<I> HeapBytes for RankedBitTable<I> {
    fn heap_bytes(&self) -> usize {
        self.bits.heap_bytes() + self.before.heap_bytes()
    }
}
