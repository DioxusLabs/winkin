//! Tables' ids by the hash of what each names.

use hashbrown::HashTable;

use super::{HeapBytes, Id, index_to_u32, u32_to_index};

/// A table's ids by the hash of what each names.
///
/// An interning table or a cache keeps one beside its records, to find a
/// record by its content. Every such index is one of these, whatever its id.
/// It holds ids as plain `u32`s, so hashbrown's table compiles once for all
/// id types. That table is a kilobyte or more of code for each type it
/// holds.
///
/// An id goes in and comes out typed, each call naming its type. So one
/// index may hold several tables' ids, as the layout's interning tables
/// share one. A find's `same` compares in its own table, so it passes over
/// another table's entry that hashes alike.
pub(crate) struct HashIndex {
    entries: HashTable<(u64, u32)>,
}

impl HashIndex {
    /// An empty index, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            entries: HashTable::new(),
        }
    }

    /// Forgets every id, keeping the capacity.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// The id noted under `hash` that `same` accepts, if there is one.
    ///
    /// Skips an entry that no `I` can name, which belongs to another table.
    #[inline]
    pub(crate) fn find<I: Id>(&self, hash: u64, mut same: impl FnMut(I) -> bool) -> Option<I> {
        self.entries
            .find(hash, |&(noted, id)| {
                noted == hash && I::try_new(u32_to_index(id)).is_some_and(&mut same)
            })
            .and_then(|&(_, id)| I::try_new(u32_to_index(id)))
    }

    /// Notes `id` under `hash`.
    pub(crate) fn insert<I: Id>(&mut self, hash: u64, id: I) {
        self.entries
            .insert_unique(hash, (hash, index_to_u32(id.get())), noted_hash);
    }

    /// Forgets `id`, noted under `hash`, where it is noted.
    pub(crate) fn remove<I: Id>(&mut self, hash: u64, id: I) {
        let packed = index_to_u32(id.get());
        if let Ok(entry) = self
            .entries
            .find_entry(hash, |&(noted, held)| noted == hash && held == packed)
        {
            entry.remove();
        }
    }
}

/// The hash an entry of a [`HashIndex`] was noted under: one function for
/// every index, so that growing one is compiled once.
fn noted_hash(entry: &(u64, u32)) -> u64 {
    entry.0
}

impl HeapBytes for HashIndex {
    fn heap_bytes(&self) -> usize {
        self.entries.heap_bytes()
    }
}
