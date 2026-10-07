//! Thread-local allocation counting shared by unit and integration tests.
//!
//! A test crate that counts installs [`Counting`] as its global allocator:
//!
//! ```text
//! #[global_allocator]
//! static GLOBAL: Counting = Counting;
//! ```
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::thread_local;

/// A global allocator that counts each thread's allocations, for
/// [`count_allocations`] to read.
pub struct Counting;

thread_local! {
    // `const` and a type with no destructor, so reading it never allocates
    // and never registers anything itself.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

fn count() {
    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// How many allocations `f` made on this thread, where [`Counting`] is the
/// global allocator.
pub fn count_allocations(f: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.with(Cell::get);
    f();
    ALLOCATIONS.with(Cell::get) - before
}
