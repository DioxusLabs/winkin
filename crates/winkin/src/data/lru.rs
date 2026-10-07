//! A bounded context cache that keeps its most recently used values.
//!
//! Every cache the context keeps has this shape.

use super::{HashIndex, HeapBytes, Id, IdRange, Table};

/// How many of the values last found or added a [`LruCache`] tries before
/// hashing: the scripts a paragraph of Japanese takes turns in, and one
/// over.
const RECENT: usize = 4;

/// A context cache whose values are found by the hash of their content,
/// bounded by a capacity it trims to.
///
/// The owner hashes a value's content and says which value matches it. The
/// cache keeps the hash index, the values last found or added, and the
/// order the values were last used in, as a list linked through their
/// entries.
///
/// Adding never evicts, so an id handed out stays good until the owner
/// calls [`trim`](Self::trim). The owner trims between calls, when no id is
/// held, and the cache then drops its least recently used values down to
/// its capacity. A dropped value's entry is reused by the next value added,
/// so a cache churning at its capacity allocates nothing for its entries.
pub(crate) struct LruCache<I, T> {
    entries: Table<I, Entry<I, T>>,
    /// Each live value's id under the hash of its content.
    index: HashIndex,
    /// The value used last.
    newest: Option<I>,
    /// The value used longest ago, which a trim drops first.
    oldest: Option<I>,
    /// The first vacant entry, which links the next through `older`.
    vacant: Option<I>,
    /// How many entries hold a value.
    len: usize,
    /// How many values a trim keeps.
    capacity: usize,
    /// The values last found or added, which
    /// [`find_recent`](Self::find_recent) tries before hashing anything: a
    /// call mostly asks for what one of the few before it asked for, the
    /// runs of a paragraph being set in a font or two and their scripts
    /// taking turns.
    recent: [Option<I>; RECENT],
    /// Where in `recent` the next value found or added goes.
    next_recent: usize,
}

/// One entry of a [`LruCache`]: a value and its place in the order of use,
/// or a vacant entry and the next vacant one.
struct Entry<I, T> {
    /// The hash the value is indexed under.
    hash: u64,
    /// The value, or `None` for a vacant entry.
    value: Option<T>,
    /// The value used just before this one, or the next vacant entry.
    older: Option<I>,
    /// The value used just after this one.
    newer: Option<I>,
}

impl<I: Id, T> LruCache<I, T> {
    /// Returns an empty cache that keeps `capacity` values, allocating
    /// nothing.
    pub(crate) const fn new(capacity: usize) -> Self {
        Self {
            entries: Table::new(),
            index: HashIndex::new(),
            newest: None,
            oldest: None,
            vacant: None,
            len: 0,
            capacity,
            recent: [None; RECENT],
            next_recent: 0,
        }
    }

    /// Drops every value, keeping the entries' capacity.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
        self.newest = None;
        self.oldest = None;
        self.vacant = None;
        self.len = 0;
        self.recent = [None; RECENT];
        self.next_recent = 0;
    }

    /// Returns how many values there are, for tests to count.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Whether every id is taken, so that [`insert`](Self::insert) refuses.
    pub(crate) fn is_full(&self) -> bool {
        self.vacant.is_none() && self.entries.remaining() == 0
    }

    /// Sets how many values a trim keeps.
    pub(crate) fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
    }

    /// Returns the value `id` names, or `None` for one the cache does not
    /// have.
    #[inline]
    pub(crate) fn get(&self, id: I) -> Option<&T> {
        self.entries.get(id)?.value.as_ref()
    }

    /// Returns the value `id` names, for its owner to complete, or `None`
    /// for one the cache does not have.
    pub(crate) fn get_mut(&mut self, id: I) -> Option<&mut T> {
        self.entries.get_mut(id)?.value.as_mut()
    }

    /// Returns every value with its id, in id order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (I, &T)> {
        self.entries
            .iter()
            .filter_map(|(id, entry)| Some((id, entry.value.as_ref()?)))
    }

    /// Returns the value hashing to `hash` that `matches` accepts, without
    /// counting it as used.
    #[inline]
    pub(crate) fn peek(&self, hash: u64, matches: impl Fn(&T) -> bool) -> Option<I> {
        self.index
            .find(hash, |id| self.get(id).is_some_and(&matches))
    }

    /// Returns the value hashing to `hash` that `matches` accepts, and
    /// counts it as used.
    #[inline]
    pub(crate) fn find(&mut self, hash: u64, matches: impl Fn(&T) -> bool) -> Option<I> {
        let found = self.peek(hash, matches)?;
        self.touch(found);
        self.remember(found);
        Some(found)
    }

    /// Finds the value `matches` accepts, trying the recent values first,
    /// and counts it as used.
    ///
    /// A recent value is found without hashing. Otherwise `hash` is called
    /// and the value under that hash is returned. Where there is none, the
    /// hash comes back as the error, for the caller to insert the value
    /// under.
    pub(crate) fn find_recent(
        &mut self,
        hash: impl FnOnce() -> u64,
        matches: impl Fn(&T) -> bool,
    ) -> Result<I, u64> {
        // The newest first.
        let newest = (1..=RECENT).map(|back| (self.next_recent + RECENT - back) % RECENT);
        let held = |&id: &I| self.get(id).is_some_and(&matches);
        if let Some(id) = newest
            .filter_map(|at| self.recent.get(at).copied().flatten())
            .find(held)
        {
            self.touch(id);
            return Ok(id);
        }
        let hash = hash();
        self.find(hash, matches).ok_or(hash)
    }

    /// Adds `value`, which hashes to `hash`, as the newest, and returns its
    /// id.
    ///
    /// It never evicts: a vacant entry is reused, or an entry added. Returns
    /// `None`, keeping nothing, where every id is taken.
    pub(crate) fn insert(&mut self, hash: u64, value: T) -> Option<I> {
        let entry = Entry {
            hash,
            value: Some(value),
            older: None,
            newer: None,
        };
        let id = match self.vacant {
            Some(id) => {
                let reused = self.entries.get_mut(id)?;
                self.vacant = reused.older;
                *reused = entry;
                id
            }
            None => self.entries.push(entry)?,
        };
        self.link_newest(id);
        self.index.insert(hash, id);
        self.len += 1;
        self.remember(id);
        Some(id)
    }

    /// Drops the value `id` names and returns it, or `None` for one the
    /// cache does not have.
    pub(crate) fn remove(&mut self, id: I) -> Option<T> {
        let entry = self.entries.get_mut(id)?;
        let value = entry.value.take()?;
        let hash = entry.hash;
        self.unlink(id);
        if let Some(entry) = self.entries.get_mut(id) {
            entry.older = self.vacant;
            entry.newer = None;
        }
        self.vacant = Some(id);
        self.index.remove(hash, id);
        self.len -= 1;
        for recent in &mut self.recent {
            if *recent == Some(id) {
                *recent = None;
            }
        }
        Some(value)
    }

    /// Drops the least recently used values down to the capacity, handing
    /// each to `evicted` with its id.
    ///
    /// The owner calls it between calls, when nothing holds an id.
    #[inline]
    pub(crate) fn trim(&mut self, mut evicted: impl FnMut(I, T)) {
        while self.len > self.capacity {
            let Some(id) = self.oldest else { break };
            let Some(value) = self.remove(id) else { break };
            evicted(id, value);
        }
    }

    /// Drops every value `keep` refuses, handing each to `dropped` with its
    /// id.
    pub(crate) fn retain(
        &mut self,
        mut keep: impl FnMut(I, &T) -> bool,
        mut dropped: impl FnMut(I, T),
    ) {
        for id in (I::new(0)..self.entries.next_id()).ids() {
            let refused = self.get(id).is_some_and(|value| !keep(id, value));
            if refused && let Some(value) = self.remove(id) {
                dropped(id, value);
            }
        }
    }

    /// Moves `id` to the newest end of the order of use.
    #[inline]
    fn touch(&mut self, id: I) {
        if self.newest != Some(id) {
            self.unlink(id);
            self.link_newest(id);
        }
    }

    /// Takes `id` out of the order of use, joining its neighbours.
    fn unlink(&mut self, id: I) {
        let Some(entry) = self.entries.get(id) else {
            return;
        };
        let (older, newer) = (entry.older, entry.newer);
        match older.and_then(|older| self.entries.get_mut(older)) {
            Some(entry) => entry.newer = newer,
            None => self.oldest = newer,
        }
        match newer.and_then(|newer| self.entries.get_mut(newer)) {
            Some(entry) => entry.older = older,
            None => self.newest = older,
        }
    }

    /// Puts `id`, which is out of the order of use, at its newest end.
    fn link_newest(&mut self, id: I) {
        let older = self.newest;
        if let Some(entry) = self.entries.get_mut(id) {
            entry.older = older;
            entry.newer = None;
        }
        match older.and_then(|older| self.entries.get_mut(older)) {
            Some(entry) => entry.newer = Some(id),
            None => self.oldest = Some(id),
        }
        self.newest = Some(id);
    }

    /// Notes `id` among the values last found or added, in place of the
    /// oldest.
    fn remember(&mut self, id: I) {
        if let Some(recent) = self.recent.get_mut(self.next_recent) {
            *recent = Some(id);
        }
        self.next_recent = (self.next_recent + 1) % RECENT;
    }
}

impl<I, T> HeapBytes for LruCache<I, T> {
    /// Its entries and its index; what a value keeps on the heap is its
    /// owner's to count.
    fn heap_bytes(&self) -> usize {
        let Self {
            entries,
            index,
            newest: _,
            oldest: _,
            vacant: _,
            len: _,
            capacity: _,
            recent: _,
            next_recent: _,
        } = self;
        entries.heap_bytes() + index.heap_bytes()
    }
}
