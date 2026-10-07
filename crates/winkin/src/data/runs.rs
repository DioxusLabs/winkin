//! Tables of runs that tile a range of positions, and cursors that walk them.
//!
//! Each run ends where the next starts. A cursor holds the run in hand and
//! where it ends.

use core::fmt;
use core::ops::Range;

use super::{HeapBytes, Id, Table};
use crate::work;

/// A record of [`Runs`]: where it starts.
pub(crate) trait Run {
    /// The positions the runs tile, and which the table's owner asks of:
    /// clusters, for every table of runs the crate has.
    type Position: Id;

    /// Its first position.
    fn start(&self) -> Self::Position;
}

/// Runs that tile a range of positions in order, each ending where the next
/// starts.
///
/// The last run ends where the range does. A reader asking about one
/// position finds its run by halving ([`containing`](Self::containing)). A stage
/// walking the positions forward uses a [`RunCursor`] instead, which passes
/// each run once.
///
/// Every table of runs of clusters has this shape: the paragraphs, the
/// items' clusters, the font runs and the shaping runs. The owner knows
/// where the last run ends and passes it in ([`span`](Self::span)). It is
/// the text's end, or where the owner's variant stops.
pub(crate) struct Runs<I, T> {
    runs: Table<I, T>,
}

impl<I: Id, T: Run> Runs<I, T> {
    /// None, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self { runs: Table::new() }
    }

    /// Removes every run, keeping the allocation.
    pub(crate) fn clear(&mut self) {
        self.runs.clear();
    }

    /// Makes room for `count` runs in all.
    pub(crate) fn reserve(&mut self, count: usize) {
        self.runs.reserve(count);
    }

    /// Appends `run`, which starts where the last does or after it, and
    /// returns its id, or `None` past what an id names.
    pub(crate) fn push(&mut self, run: T) -> Option<I> {
        debug_assert!(
            self.runs
                .last()
                .is_none_or(|last| last.start() <= run.start()),
            "runs start in order"
        );
        self.runs.push(run)
    }

    /// How many runs there are.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.runs.len()
    }

    /// Whether there are none.
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Run `id`, or `None` past the last.
    #[inline]
    pub(crate) fn get(&self, id: I) -> Option<&T> {
        self.runs.get(id)
    }

    /// The last run, or `None` where there are none.
    pub(crate) fn last(&self) -> Option<&T> {
        self.runs.last()
    }

    /// Every run with its id, in order.
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = (I, &T)> + ExactSizeIterator {
        self.runs.iter()
    }

    /// Every run, in order, for the tests to check.
    #[cfg(test)]
    pub(crate) fn as_slice(&self) -> &[T] {
        self.runs.as_slice()
    }

    /// Every run from `from` on, in order: from a cursor, the runs of the
    /// positions after it.
    pub(crate) fn rest(&self, from: I) -> &[T] {
        self.runs
            .get_slice(from..self.runs.next_id())
            .unwrap_or_default()
    }

    /// Where run `id` starts, or `end`, where the last run ends, past the
    /// last.
    #[inline]
    pub(crate) fn start(&self, id: I, end: T::Position) -> T::Position {
        self.runs.get(id).map_or(end, Run::start)
    }

    /// Where the run after `id` starts, which is where `id` ends; `None`
    /// where `id` is the last, which ends where the range does.
    #[inline]
    pub(crate) fn next_start(&self, id: I) -> Option<T::Position> {
        self.runs.get(I::new(id.get() + 1)).map(Run::start)
    }

    /// Run `id`'s positions, from its start to the next run's start or `end`.
    ///
    /// `end` is where the last run ends. Past the last run the range is
    /// empty, at `end`.
    #[inline]
    pub(crate) fn span(&self, id: I, end: T::Position) -> Range<T::Position> {
        self.start(id, end)..self.next_start(id).unwrap_or(end)
    }

    /// The run containing `pos`, by halving: the last that starts at or before
    /// it. `None` where none does, as where there are no runs.
    #[inline]
    pub(crate) fn containing(&self, pos: T::Position) -> Option<I> {
        self.last_where(|run| run.start() <= pos)
    }

    /// The last run containing `pos` that `before` holds of, by halving, or
    /// `None` where it holds of none.
    ///
    /// `before` must hold of every run up to some point and of none after.
    /// An owner uses it to seek a run by a position on another axis that its
    /// runs' starts map to in order, such as a caller's offset.
    #[inline]
    pub(crate) fn last_where(&self, before: impl FnMut(&T) -> bool) -> Option<I> {
        self.runs.last_where(before)
    }

    /// The first run that starts at or after `at`, by halving: one past the
    /// last where none does.
    pub(crate) fn first_from(&self, at: T::Position) -> I {
        work::seek();
        I::new(self.runs.as_slice().partition_point(|run| run.start() < at))
    }
}

/// Where a walk over a table of [`Runs`] stands: the run in hand and where
/// it ends.
///
/// The end is read once from the next run's start. So the walk makes one
/// comparison per run and none per position. An id alone would make the
/// walk find each run's end again. A walk over the shaping runs needs the
/// end held: finding each end from the start bits makes relayout 3 to 5%
/// slower.
///
/// It is two positions in size and `Copy`, so a walk that looks ahead copies
/// it. Its fields stay behind methods: `end` is the next run's start, which
/// only the table moving the cursor keeps true.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct RunCursor<I, P> {
    id: I,
    end: P,
}

impl<I: Copy, P: Copy> RunCursor<I, P> {
    /// The run in hand: one past the last where the walk has passed every
    /// run.
    #[inline]
    pub(crate) fn id(self) -> I {
        self.id
    }

    /// Where it ends: where the next starts, or where the last run ends,
    /// which is where a cursor past the last stands.
    #[inline]
    pub(crate) fn end(self) -> P {
        self.end
    }
}

impl<I: Id, T: Run> Runs<I, T> {
    /// A cursor at run `id`, made from a link with no search.
    ///
    /// `end` is where the last run ends. Past the last run the cursor stands
    /// at `end`, holding nothing.
    #[inline]
    pub(crate) fn cursor(&self, id: I, end: T::Position) -> RunCursor<I, T::Position> {
        let end = if id.get() < self.runs.len() {
            self.next_start(id).unwrap_or(end)
        } else {
            end
        };
        RunCursor { id, end }
    }

    /// A cursor at the run containing `at`, found by halving, or `None` where no
    /// run holds it.
    ///
    /// `end` is where the last run ends. A walk with no link to start from
    /// seeks once with this ([`containing`](Self::containing)).
    pub(crate) fn cursor_containing(
        &self,
        at: T::Position,
        end: T::Position,
    ) -> Option<RunCursor<I, T::Position>> {
        self.containing(at).map(|id| self.cursor(id, end))
    }

    /// A cursor at the run containing `at`, walked to from run `near` a run
    /// at a time, either way: what
    /// [`cursor_containing`](Self::cursor_containing) finds, with no search.
    ///
    /// A walk that holds a run near `at` uses it, and pays a step a run
    /// between. `None` where no run holds `at`.
    pub(crate) fn cursor_from(
        &self,
        near: I,
        at: T::Position,
        end: T::Position,
    ) -> Option<RunCursor<I, T::Position>> {
        let last = self.runs.len().checked_sub(1)?;
        let mut id = near.get().min(last);
        while id > 0 && self.start(I::new(id), end) > at {
            work::step();
            id -= 1;
        }
        if self.start(I::new(id), end) > at {
            return None;
        }
        let mut cursor = self.cursor(I::new(id), end);
        self.step_to(&mut cursor, at, end);
        Some(cursor)
    }

    /// Moves `cursor` to the next run, whether or not it holds a position.
    ///
    /// `end` is where the last run ends. Past the last run the cursor stands
    /// at `end` and stays there.
    #[inline]
    pub(crate) fn step(&self, cursor: &mut RunCursor<I, T::Position>, end: T::Position) {
        if cursor.id.get() < self.runs.len() {
            *cursor = self.cursor(I::new(cursor.id.get() + 1), end);
        }
    }

    /// Moves `cursor` forward to the run holding `at`.
    ///
    /// It passes each run ending at or before `at`, but never the last run,
    /// which ends at `end`. It costs one comparison where the cursor holds
    /// `at` already.
    #[inline]
    pub(crate) fn step_to(
        &self,
        cursor: &mut RunCursor<I, T::Position>,
        at: T::Position,
        end: T::Position,
    ) {
        while cursor.end <= at && cursor.id.get() + 1 < self.runs.len() {
            *cursor = self.cursor(I::new(cursor.id.get() + 1), end);
        }
    }
}

impl<I: Id, T: fmt::Debug> fmt::Debug for Runs<I, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.runs.fmt(f)
    }
}

impl<I, T> HeapBytes for Runs<I, T> {
    fn heap_bytes(&self) -> usize {
        self.runs.heap_bytes()
    }
}
