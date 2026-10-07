//! A count of the steps the stages take and the searches they make, for
//! tests to hold each stage to the size of its input and its output.
//!
//! Each walk whose length a caller controls counts a step per pass of its
//! loop. These walk a line's items and pieces, its boxes and their segments,
//! the boxes open across its start and those waiting on it, a paragraph's
//! controls, and the builder's stack. A test builds the same shape of content
//! at two sizes and compares the steps each stage took. It catches a stage
//! whose steps grow faster than its input and output, however fast the
//! machine is, and no clock makes the test flaky.
//!
//! Each search primitive counts a seek: a halving, a rank on start bits, or
//! a lookup that finds a row by position rather than holding it. A test
//! holds a stage to the seeks its entries need, so a walk that loses its
//! place and searches again shows up as a count, not as a slower bench.
//!
//! Steps and seeks are counted only in the crate's own tests built with
//! debug assertions, as `cargo test` builds them, and there per thread, so
//! tests running side by side do not count each other's. Everywhere else
//! [`step`] and [`seek`] are empty and cost nothing: in the library, and in
//! the release builds the timing tests run in, which they would otherwise
//! slow.

#[cfg(test)]
use core::cell::Cell;

#[cfg(all(test, debug_assertions))]
std::thread_local! {
    static STEPS: Cell<u64> = const { Cell::new(0) };
    static SEEKS: Cell<u64> = const { Cell::new(0) };
}

/// Counts one step of a walk: nothing but in a debug build's tests.
#[inline(always)]
pub(crate) fn step() {
    #[cfg(all(test, debug_assertions))]
    STEPS.with(|steps| steps.set(steps.get().wrapping_add(1)));
}

/// Counts one seek, a search for a row by position: nothing but in a debug
/// build's tests.
#[inline(always)]
pub(crate) fn seek() {
    #[cfg(all(test, debug_assertions))]
    SEEKS.with(|seeks| seeks.set(seeks.get().wrapping_add(1)));
}

#[cfg(test)]
std::thread_local! {
    static GENERAL_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// Whether a stage may take a fast path where the facts its gate reads
/// allow one.
///
/// It is always true, except on a test's thread inside `general_paths`.
/// Those tests check that each fast path's output matches the general
/// path's.
#[inline(always)]
pub(crate) fn fast_paths() -> bool {
    #[cfg(test)]
    if GENERAL_ONLY.with(Cell::get) {
        return false;
    }
    true
}

/// Runs `f` with every fast path closed on this thread, for a test to
/// compare what the general paths make with what the fast paths do.
#[cfg(test)]
pub(crate) fn general_paths<R>(f: impl FnOnce() -> R) -> R {
    let was = GENERAL_ONLY.with(|general| general.replace(true));
    let result = f();
    GENERAL_ONLY.with(|general| general.set(was));
    result
}

/// The steps counted on this thread since the last call, starting the
/// count again.
#[cfg(all(test, debug_assertions))]
pub(crate) fn take() -> u64 {
    STEPS.with(|steps| steps.replace(0))
}

/// The seeks counted on this thread since the last call, starting the
/// count again.
#[cfg(all(test, debug_assertions))]
pub(crate) fn take_seeks() -> u64 {
    SEEKS.with(|seeks| seeks.replace(0))
}
