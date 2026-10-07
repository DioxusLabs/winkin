//! Counting what the collection holds on the heap.
//!
//! By walking the structures, not by asking the allocator: what a
//! [`Collection`](crate::Collection) or a [`Layer`](crate::Layer) says it
//! holds is the bytes it asked for, without the allocator's own headers and
//! rounding. Anything shared — a charset several fonts hold, a record two
//! snapshots hold — is counted once per call, by address.

use crate::hash::HashSet;

/// An `Arc`'s two reference counts, ahead of what it holds.
pub(crate) const ARC: usize = 2 * core::mem::size_of::<usize>();

/// The allocations already counted.
#[derive(Default)]
pub(crate) struct Seen(HashSet<usize>);

impl Seen {
    /// `bytes` the first time `at` is seen, and nothing after.
    pub(crate) fn once<T: ?Sized>(&mut self, at: *const T, bytes: impl FnOnce() -> usize) -> usize {
        if self.0.insert(at.cast::<u8>() as usize) {
            bytes()
        } else {
            0
        }
    }
}
