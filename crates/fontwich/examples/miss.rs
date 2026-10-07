//! What a cold miss costs: `cargo run --release --example miss -- U+E000 U+1E900`.
//!
//! For each character, in a fresh system collection, walks the miss path to
//! the first family that maps it, and prints the time, the families loaded,
//! the collection's heap and, on Windows, the process's private bytes and
//! working set before and after.
use fontwich::{Collection, FallbackRequest, GenericClass, Presentation, Script};
use std::time::Instant;

#[cfg(windows)]
fn process_memory() -> (usize, usize) {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged_pool: usize,
        quota_paged_pool: usize,
        quota_peak_non_paged_pool: usize,
        quota_non_paged_pool: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut counters = Counters {
        cb: size_of::<Counters>() as u32,
        ..Counters::default()
    };
    // SAFETY: a pseudo-handle and a struct of the size it says.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    (counters.private, counters.working_set)
}

#[cfg(not(windows))]
fn process_memory() -> (usize, usize) {
    (0, 0)
}

fn loaded(collection: &Collection) -> usize {
    collection.layers().map(|layer| layer.loaded()).sum()
}

fn main() {
    let characters: Vec<char> = std::env::args()
        .skip(1)
        .filter_map(|arg| {
            let hex = arg.trim_start_matches("U+").trim_start_matches("u+");
            char::from_u32(u32::from_str_radix(hex, 16).ok()?)
        })
        .collect();
    let characters = if characters.is_empty() {
        vec!['\u{E000}', '\u{1E900}', '\u{11000}']
    } else {
        characters
    };
    let request = FallbackRequest::Text {
        script: Script::from_bytes(*b"Latn"),
        language: None,
        generic: GenericClass::Plain,
    };
    for c in characters {
        let collection = Collection::system();
        let (private, working) = process_memory();
        let heap = collection.heap_usage();
        let start = Instant::now();
        let mut walked = 0;
        let found = collection
            .char_fallback(c, Presentation::Text, &request)
            .inspect(|_| walked += 1)
            .find(|family| family.covers(c));
        let elapsed = start.elapsed();
        let (private_after, working_after) = process_memory();
        println!(
            "U+{:04X}: {:?} after {walked} families, {} loaded, {elapsed:.1?}; heap {} -> {} KB; private {} -> {} KB; working set {} -> {} KB",
            u32::from(c),
            found.as_ref().map(|family| family.name()),
            loaded(&collection),
            heap / 1024,
            collection.heap_usage() / 1024,
            private / 1024,
            private_after / 1024,
            working / 1024,
            working_after / 1024,
        );
        // The next miss in the same collection walks what is loaded.
        let next = char::from_u32(u32::from(c) + 1).unwrap_or(c);
        let start = Instant::now();
        let found = collection
            .char_fallback(next, Presentation::Text, &request)
            .find(|family| family.covers(next));
        println!(
            "  then U+{:04X}: {:?}, {:.1?}",
            u32::from(next),
            found.as_ref().map(|family| family.name()),
            start.elapsed()
        );
    }
}
