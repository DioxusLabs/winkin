//! A report, not a test: what charsets cost in heap, measured, and what the
//! alternatives would cost on the same fonts.
//!
//! Run alone, since the allocator counts the whole process:
//! `cargo test --release --test heap -- --ignored --nocapture`

#![cfg(all(
    feature = "system",
    any(windows, all(unix, not(target_vendor = "apple")))
))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use fontwich::{Collection, Family, Layer};
use icu_properties::CodePointMapData;
use icu_properties::props::{NamedEnumeratedProperty, Script as IcuScript};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static BLOCKS: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grew(layout.size());
        BLOCKS.fetch_add(1, Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_sub(layout.size(), Relaxed);
        grew(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        BLOCKS.fetch_sub(1, Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// `(live bytes, live blocks)`, and the peak is reset to now.
fn snapshot() -> (usize, usize) {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    (live, BLOCKS.load(Relaxed))
}

/// What the allocator really spends on a block of `size`: a header and
/// rounding to 16, with a 32-byte minimum. glibc's is 8 and 16; Windows'
/// low-fragmentation heap is close. A model, stated so the numbers can be
/// read with it in mind.
fn real(size: usize) -> usize {
    if size == 0 {
        0
    } else {
        (size + 8).next_multiple_of(16).max(32)
    }
}

fn kb(bytes: usize) -> String {
    format!("{:>7.1} KB", bytes as f64 / 1024.0)
}

/// Heap for a set of 256-character pages: one block of page numbers and one
/// of 32-byte leaves.
fn bitmap_heap(pages: usize) -> usize {
    if pages == 0 {
        0
    } else {
        real(4 * pages) + real(32 * pages)
    }
}

fn range_pages(ranges: impl Iterator<Item = std::ops::RangeInclusive<u32>>) -> BTreeSet<u32> {
    ranges
        .flat_map(|range| (range.start() >> 8)..=(range.end() >> 8))
        .collect()
}

#[test]
#[ignore]
fn charset_heap_report() {
    let start = snapshot();
    let layer = Arc::new(Layer::system());
    let collection = Collection::new().with_layer(layer.clone());
    let listed = snapshot();

    let families: Vec<Family> = layer
        .names()
        .filter_map(|name| collection.family(name))
        .collect();
    let fonts: usize = families.iter().map(|family| family.fonts().len()).sum();
    let peak = PEAK.load(Relaxed);
    let loaded = snapshot();
    let held: BTreeSet<*const fontwich::Charset> = families
        .iter()
        .flat_map(|family| family.fonts())
        .map(|font| font.charset() as *const _)
        .collect();

    println!(
        "{} families, {fonts} fonts, {} distinct charsets held\n\nmeasured (bytes requested, blocks)",
        families.len(),
        held.len()
    );
    println!(
        "  listing the layer        {}  {:>6} blocks",
        kb(listed.0 - start.0),
        listed.1 - start.1
    );
    println!(
        "  loading every family, charsets included  {}  {:>6} blocks, peak {} above the listed level",
        kb(loaded.0 - listed.0),
        loaded.1 - listed.1,
        kb(peak - listed.0)
    );

    // The alternatives, from the same charsets. Each is the allocator's real
    // cost under the model in `real`.
    let (mut ranges, mut ranges_blocks) = (0, 0);
    let (mut per_font, mut per_font_blocks) = (0, 0);
    let mut distinct: BTreeSet<Vec<(u32, u32)>> = BTreeSet::new();
    let mut distinct_heap = 0;
    let (mut per_family, mut per_family_blocks) = (0, 0);
    let mut shared_family = 0;

    let scripts = CodePointMapData::<IcuScript>::new();
    let shared: BTreeSet<u32> = (0..=0x10FFFF_u32)
        .filter(|&c| {
            char::from_u32(c)
                .is_some_and(|ch| matches!(scripts.get(ch).short_name(), "Zyyy" | "Zinh"))
        })
        .collect();
    let shared_pages: BTreeSet<u32> = shared.iter().map(|c| c >> 8).collect();

    for family in &families {
        let mut union: BTreeSet<u32> = BTreeSet::new();
        for font in family.fonts() {
            let map = font.charset();
            let n = map.ranges().count();
            if n > 0 {
                ranges += real(8 * n);
                ranges_blocks += 1;
            }
            let pages = range_pages(map.ranges());
            per_font += bitmap_heap(pages.len());
            per_font_blocks += if pages.is_empty() { 0 } else { 2 };
            let key: Vec<(u32, u32)> = map.ranges().map(|r| (*r.start(), *r.end())).collect();
            if distinct.insert(key) {
                distinct_heap += bitmap_heap(pages.len()) + real(16);
            }
            union.extend(pages);
        }
        per_family += bitmap_heap(union.len());
        per_family_blocks += if union.is_empty() { 0 } else { 2 };
        shared_family += bitmap_heap(union.intersection(&shared_pages).count());
    }
    println!("\nmodelled (allocator's real cost)");
    println!(
        "  ranges per font                  {}  {:>6} blocks",
        kb(ranges),
        ranges_blocks
    );
    println!(
        "  bitmap per font                  {}  {:>6} blocks",
        kb(per_font),
        per_font_blocks
    );
    println!(
        "  bitmap per font, identical ones shared, as now  {}  ({} distinct of {fonts})",
        kb(distinct_heap),
        distinct.len()
    );
    println!(
        "  bitmap per family (union)        {}  {:>6} blocks",
        kb(per_family),
        per_family_blocks
    );
    println!(
        "  per family, Common/Inherited pages only  {}  ({} of {} pages hold any)",
        kb(shared_family),
        shared_pages.len(),
        0x110000 / 256
    );
}
