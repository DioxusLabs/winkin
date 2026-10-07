//! What a table keeps on the heap.
//!
//! Every table answers [`HeapBytes::heap_bytes`]: the bytes its capacity
//! takes, counting both what it holds and what it keeps for the next build.
//! Each stage's data type sums its own tables in a method of its own.
//! [`Layout::heap_bytes`](crate::Layout::heap_bytes) and
//! [`Context::heap_bytes`](crate::Context::heap_bytes) report the sums stage
//! by stage, as [`LayoutHeap`](crate::LayoutHeap) and
//! [`ContextHeap`](crate::ContextHeap).
//!
//! A table does not count what its items share with others. A used font's
//! bytes belong to its file, behind an `Arc` the context's font instances
//! share. Values from other crates that do not report their size count at
//! their own size only. These are harfrust's font data, instances, plans and
//! buffer in the shaping context, and fontwich's family lists and character
//! sets in the font caches. The segmenters keep nothing on the heap, since
//! their data is compiled in.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::size_of;

use hashbrown::HashTable;

/// A value that keeps bytes on the heap.
pub(crate) trait HeapBytes {
    /// The bytes its tables' capacity takes on the heap, used or kept for
    /// the next build: what it owns, not what it shares.
    fn heap_bytes(&self) -> usize;
}

/// Implements [`HeapBytes`] for a struct as the sum of its fields'.
///
/// Each field is named once. The fields after the `;` keep nothing on the
/// heap and are not counted.
///
/// ```ignore
/// heap_bytes! {
///     /// Its tables; the initial letter holds nothing on the heap.
///     MeasuredText { prefix, paragraphs, extents; initial_letter }
/// }
/// ```
///
/// Every field is named, as a written-out destructuring names it, so a
/// field added to the struct and not here fails to compile rather than go
/// uncounted, which `tests/heap.rs` would find only later. A struct whose
/// sum is more than its fields' is written out.
macro_rules! heap_bytes {
    (
        $(#[$meta:meta])*
        $name:ident {
            $first:ident $(, $field:ident)*
            $(; $($(#[$skip_meta:meta])* $skip:ident),+)?
        }
    ) => {
        impl $crate::data::HeapBytes for $name {
            $(#[$meta])*
            fn heap_bytes(&self) -> usize {
                let Self {
                    $first,
                    $($field,)*
                    $($($(#[$skip_meta])* $skip: _,)+)?
                } = self;
                $crate::data::HeapBytes::heap_bytes($first)
                    $(+ $crate::data::HeapBytes::heap_bytes($field))*
            }
        }
    };
}
pub(crate) use heap_bytes;

impl<T> HeapBytes for Vec<T> {
    /// Its capacity, whose items hold nothing on the heap themselves: a
    /// vector whose items do counts theirs on its owner's behalf.
    fn heap_bytes(&self) -> usize {
        self.capacity().saturating_mul(size_of::<T>())
    }
}

impl HeapBytes for String {
    fn heap_bytes(&self) -> usize {
        self.capacity()
    }
}

impl<T> HeapBytes for HashTable<T> {
    /// Its buckets and their control bytes, as hashbrown allocated them.
    fn heap_bytes(&self) -> usize {
        self.allocation_size()
    }
}

impl<T: HeapBytes> HeapBytes for Option<Box<T>> {
    /// Rare tables, boxed and made the first time a build has any.
    ///
    /// Counts the box and its tables where one was made, and nothing where
    /// none was.
    fn heap_bytes(&self) -> usize {
        self.as_deref()
            .map_or(0, |rare| size_of::<T>() + rare.heap_bytes())
    }
}
