//! Holds the hot paths to allocating nothing, by counting.
//!
//! Looking a family up is on every text run's path, and so is asking a key's
//! fallback families. Both are meant to allocate nothing once warm — names
//! hashed on the stack, indexes searched in place, answers kept by the
//! collection — and this is what makes "meant to" into "does". A global
//! allocator that counts, per thread, so tests running alongside cannot
//! disturb each other's numbers.
//!
//! Twice over: against a layer built from names with a fallback override,
//! which runs anywhere and is the shape of the thing, and against the
//! platform's own collection and backend in `mod system`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use fontwich::{
    Collection, FallbackKey, FallbackOverride, FallbackRequest, Family, Font, FontFamilyName,
    GenericClass, GenericFamily, Layer, LoadFamily, Presentation, Role, Script, parse_language,
};

struct Counting;

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

#[global_allocator]
static GLOBAL: Counting = Counting;

/// How many allocations `f` made on this thread.
fn count_allocations(f: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.with(Cell::get);
    f();
    ALLOCATIONS.with(Cell::get) - before
}

#[derive(Debug)]
struct Nothing;

impl LoadFamily for Nothing {
    fn load(&self, _: &str) -> Vec<Font> {
        Vec::new()
    }
}

/// Answers every text key with every family it names that the collection
/// has, and a generic with the first: the shape of a host's own table.
struct Every(&'static [&'static str]);

impl FallbackOverride for Every {
    fn families(&self, key: &FallbackKey, collection: &Collection, out: &mut Vec<Family>) {
        if key.presentation().is_some() {
            return;
        }
        out.extend(
            self.0
                .iter()
                .filter_map(|name| collection.fallback_family(name)),
        );
    }
}

/// A system layer listing families by name, the way DirectWrite and
/// fontconfig do, with a fallback override.
fn system() -> Collection {
    let families = [
        ("Noto Sans", vec![]),
        ("Noto Serif", vec![]),
        ("Noto Naskh Arabic", vec![]),
        ("MS Gothic", vec![String::from("ＭＳ ゴシック")]),
    ];
    let layer = Layer::from_names(
        Role::System,
        families
            .into_iter()
            .map(|(name, aliases)| (String::from(name), aliases)),
        Arc::new(Nothing),
    )
    .with_fallback_override(Every(&["Noto Sans", "Noto Naskh Arabic", "MS Gothic"]));
    Collection::new().with_layer(Arc::new(layer))
}

#[test]
fn looking_a_family_up_allocates_nothing() {
    let collection = system();
    let allocated = count_allocations(|| {
        // By name, in any case, by alias, for fallback, and misses on either
        // side of a real name so the binary search runs to both ends.
        assert!(collection.family("Noto Sans").is_some());
        assert!(collection.family("NOTO SANS").is_some());
        assert!(collection.family("ＭＳ ゴシック").is_some());
        assert!(collection.family("noto serif").is_some());
        assert!(collection.fallback_family("Noto Naskh Arabic").is_some());
        assert!(collection.family("Helvetica").is_none());
        assert!(collection.family("A").is_none());
        assert!(collection.family("Zapfino").is_none());
        // Reading what a handle is called, too.
        let family = collection.family("ms gothic").expect("present");
        assert_eq!(family.name(), "MS Gothic");
        assert_eq!(family.aliases().count(), 1);
    });
    assert_eq!(allocated, 0, "a lookup allocated");
}

fn text(script: &[u8; 4], language: Option<&str>) -> FallbackRequest {
    FallbackRequest::Text {
        script: Script::from_bytes(*script),
        language: language.and_then(parse_language),
        generic: GenericClass::Plain,
    }
}

#[test]
fn asking_a_key_s_families_allocates_nothing_once_warm() {
    let collection = system();
    let requests = [
        text(b"Arab", None),
        text(b"Latn", Some("en-US")),
        text(b"Hani", Some("ja")),
        FallbackRequest::Generic(GenericFamily::SansSerif, None),
        FallbackRequest::Emoji(Presentation::Emoji),
    ];
    let ask = || {
        let mut families = 0;
        for request in &requests {
            families += collection.fallback(&collection.key(request)).len();
        }
        families
    };
    // The first asks fill the collection's answers; after that they are read.
    ask();
    let allocated = count_allocations(|| {
        for _ in 0..10 {
            assert!(ask() > 0);
        }
    });
    assert_eq!(allocated, 0, "a warm fallback allocated");
}

#[test]
fn resolving_a_name_allocates_nothing_once_warm() {
    let collection = system();
    let request = text(b"Latn", None);
    let names = [
        FontFamilyName::Generic(GenericFamily::Serif),
        FontFamilyName::named("MS Gothic"),
        FontFamilyName::named("Missing"),
    ];
    let resolve = || {
        names
            .iter()
            .filter(|name| collection.resolve(name, &request).is_some())
            .count()
    };
    assert_eq!(resolve(), 2);
    let allocated = count_allocations(|| {
        assert_eq!(resolve(), 2);
    });
    assert_eq!(allocated, 0, "a warm resolve allocated");
}

#[test]
fn walking_a_missed_character_allocates_nothing_once_warm() {
    let collection = system();
    let request = text(b"Latn", Some("ja"));
    let walk = || {
        let mut walked = 0;
        for c in ['。', 'ب', '😀', '\u{E000}'] {
            walked += collection
                .char_fallback(c, Presentation::Text, &request)
                .count();
        }
        walked
    };
    walk();
    let allocated = count_allocations(|| {
        assert!(walk() > 0);
    });
    assert_eq!(allocated, 0, "a warm miss allocated");
}

#[test]
fn keying_a_request_allocates_nothing() {
    let collection = system();
    let allocated = count_allocations(|| {
        for tag in ["zh-Hant-HK", "zh-yue-HK", "ja", "ar", "fa-IR", "!!"] {
            let language = parse_language(tag);
            for script in [*b"Hani", *b"Latn", *b"Arab", *b"Zyyy", *b"Zsye", [0xFF; 4]] {
                let request = FallbackRequest::Text {
                    script: Script::from_bytes(script),
                    language,
                    generic: GenericClass::Serif,
                };
                std::hint::black_box(collection.key(&request));
            }
            let request = FallbackRequest::Generic(GenericFamily::UiMonospace, language);
            std::hint::black_box(collection.key(&request));
        }
        let request = FallbackRequest::Emoji(Presentation::EmojiOnly);
        std::hint::black_box(collection.key(&request));
    });
    assert_eq!(allocated, 0, "keying a request allocated");
}

#[test]
fn the_counter_counts() {
    // Every zero above is only worth something if this is not also zero.
    let allocated = count_allocations(|| {
        let boxed = std::hint::black_box(Box::new(1u64));
        drop(boxed);
    });
    assert_eq!(allocated, 1);
}

/// A font with an English family name and a format 4 cmap mapping `ranges`
/// to consecutive glyphs. The crate's own builders are test-only and private,
/// so this is a copy.
fn font_covering(family: &str, ranges: &[(u16, u16)]) -> Vec<u8> {
    let mut segments: Vec<(u16, u16, u16)> = Vec::new();
    let mut glyph: u16 = 1;
    for &(start, end) in ranges {
        segments.push((start, end, glyph.wrapping_sub(start)));
        glyph += end - start + 1;
    }
    segments.push((0xFFFF, 0xFFFF, 1));
    let count = segments.len() as u16;
    let mut subtable = Vec::new();
    for field in [4, 16 + 8 * count, 0, 2 * count, 0, 0, 0] {
        subtable.extend_from_slice(&field.to_be_bytes());
    }
    subtable.extend(segments.iter().flat_map(|s| s.1.to_be_bytes()));
    subtable.extend_from_slice(&0u16.to_be_bytes());
    subtable.extend(segments.iter().flat_map(|s| s.0.to_be_bytes()));
    subtable.extend(segments.iter().flat_map(|s| s.2.to_be_bytes()));
    subtable.extend(segments.iter().flat_map(|_| 0u16.to_be_bytes()));
    let mut cmap = Vec::new();
    for field in [0u16, 1, 3, 1] {
        cmap.extend_from_slice(&field.to_be_bytes()); // version, count, Windows BMP
    }
    cmap.extend_from_slice(&12u32.to_be_bytes());
    cmap.extend_from_slice(&subtable);

    let named = font_named(family);
    let name = named[28..].to_vec();
    let mut font = Vec::new();
    font.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    for field in [2u16, 32, 1, 0] {
        font.extend_from_slice(&field.to_be_bytes());
    }
    let cmap_at = 12 + 32;
    let name_at = (cmap_at + cmap.len()).next_multiple_of(4);
    for (tag, at, len) in [
        (b"cmap", cmap_at, cmap.len()),
        (b"name", name_at, name.len()),
    ] {
        font.extend_from_slice(tag);
        font.extend_from_slice(&0u32.to_be_bytes());
        font.extend_from_slice(&(at as u32).to_be_bytes());
        font.extend_from_slice(&(len as u32).to_be_bytes());
    }
    font.extend_from_slice(&cmap);
    font.resize(name_at, 0);
    font.extend_from_slice(&name);
    font
}

/// The smallest font with an English family name: a `name` table and nothing
/// else. The crate's own builder is test-only and private, so this is a copy.
fn font_named(family: &str) -> Vec<u8> {
    let text: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut name = Vec::new();
    for field in [0u16, 1, 18] {
        name.extend_from_slice(&field.to_be_bytes()); // format, count, storage offset
    }
    for field in [3u16, 1, 0x0409, 1, text.len() as u16, 0] {
        name.extend_from_slice(&field.to_be_bytes()); // one Windows English family record
    }
    name.extend_from_slice(&text);
    let mut font = Vec::new();
    font.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    for field in [1u16, 16, 0, 0] {
        font.extend_from_slice(&field.to_be_bytes());
    }
    font.extend_from_slice(b"name");
    font.extend_from_slice(&0u32.to_be_bytes());
    font.extend_from_slice(&28u32.to_be_bytes());
    font.extend_from_slice(&(name.len() as u32).to_be_bytes());
    font.extend_from_slice(&name);
    font
}

#[test]
fn an_edit_under_a_held_snapshot_does_not_copy_the_whole_layer() {
    // Copy-on-write means an edit copies what a held snapshot still shares.
    // Records sit behind `Arc`s and the name index is one flat vector, so an
    // edit copies a vector of pointers, the index, and the one family it
    // touches -- a fixed number of allocations, the same for a small layer as
    // a large one. Before, every family's name and font list was copied, and
    // a tree-shaped index was copied node by node.
    let cost = |families: usize, edit: &str| {
        let mut fonts = fontwich::LayerBuilder::new(Role::Application);
        for n in 0..families {
            fonts
                .add_data(font_named(&format!("Family {n}")))
                .expect("adds");
        }
        let font = font_named(edit);
        let _held = fonts.snapshot();
        count_allocations(|| {
            fonts.add_data(font).expect("adds");
        })
    };
    for (edit, what) in [
        ("Family 0", "a font for a family it has"),
        ("New Family", "a new family"),
    ] {
        let (small, large) = (cost(10, edit), cost(200, edit));
        assert_eq!(
            small, large,
            "adding {what} cost {small} allocations with 10 families and {large} with 200"
        );
    }
}

/// Matching a family's fonts to a request, and asking what to synthesize,
/// allocates nothing: every step is a rank computed in place.
#[cfg(all(
    feature = "system",
    any(windows, all(unix, not(target_vendor = "apple")))
))]
#[test]
fn matching_a_font_allocates_nothing() {
    use fontwich::{Attributes, FontStyle, FontWeight, FontWidth};

    let collection = Collection::system();
    // Families with several fonts, static and variable, on Windows and on
    // common Linux installs; whichever are here.
    let families: Vec<_> = [
        "Arial",
        "Segoe UI",
        "Bahnschrift",
        "Segoe UI Variable",
        "DejaVu Sans",
        "Noto Sans",
        "Liberation Sans",
        "Cantarell",
    ]
    .iter()
    .filter_map(|name| collection.family(name))
    .filter(|family| family.fonts().len() > 1)
    .collect();
    if families.is_empty() {
        eprintln!("no family with several fonts here; nothing to match");
        return;
    }
    let requests = [
        Attributes::default(),
        Attributes {
            width: FontWidth::CONDENSED,
            style: FontStyle::Italic,
            weight: FontWeight::BOLD,
        },
        Attributes {
            width: FontWidth::EXPANDED,
            style: FontStyle::Oblique(Some(-8.0)),
            weight: FontWeight::new(350.0),
        },
        Attributes {
            width: FontWidth::NORMAL,
            style: FontStyle::Oblique(None),
            weight: FontWeight::new(950.0),
        },
    ];
    let allocated = count_allocations(|| {
        for family in &families {
            for request in requests {
                for synthesize in [false, true] {
                    let font = family.match_font(request, synthesize).expect("fonts");
                    std::hint::black_box(font.synthesis(request));
                }
            }
        }
    });
    assert_eq!(allocated, 0, "matching allocated");
}

/// Two fonts mapping ASCII and Cyrillic for every family: something to key,
/// match and look up pages in.
#[derive(Debug)]
struct Covering;

impl LoadFamily for Covering {
    fn load(&self, name: &str) -> Vec<Font> {
        let bytes = fontwich::FontBytes::new(font_covering(name, &[(0x20, 0x7E), (0x400, 0x4FF)]));
        vec![Font::from_data(bytes.clone(), 0), Font::from_data(bytes, 0)]
    }
}

#[test]
fn what_a_layout_asks_of_a_font_allocates_nothing_once_warm() {
    // A key without loading, the indices matching gives, and a page of a
    // charset and a cluster tested against it: once per font or per style,
    // and none of it may allocate.
    use fontwich::Attributes;

    let layer = Layer::from_names(
        Role::System,
        ["Noto Sans", "Noto Serif"]
            .into_iter()
            .map(|name| (String::from(name), Vec::new())),
        Arc::new(Covering),
    )
    .with_fallback_override(Every(&["Noto Serif", "Noto Sans"]));
    let collection = Collection::new().with_layer(Arc::new(layer));
    let request = text(b"Latn", None);
    let names = [
        FontFamilyName::Generic(GenericFamily::Serif),
        FontFamilyName::named("Noto Sans"),
    ];
    let walk = || {
        let mut count = 0;
        for name in &names {
            let family = collection.resolve(name, &request).expect("a family");
            let at = family
                .matching_indices(Attributes::default(), true)
                .next()
                .expect("a font");
            let font = &family.fonts()[at];
            count += usize::from(font.key().is_some());
            count += family.matching_indices(Attributes::default(), true).count();
            let charset = font.charset();
            count += usize::from(charset.page('м').is_some_and(|page| page.contains('м')));
            count += usize::from(charset.covers_all("Hello, мир".chars()));
            count += usize::from(!charset.covers_all("Hello, 世界".chars()));
        }
        count
    };
    walk();
    let allocated = count_allocations(|| {
        assert_eq!(walk(), 2 * 6);
    });
    assert_eq!(allocated, 0, "a warm walk allocated");
}

/// A font holding nothing but a `cmap`: ASCII in a format 12 subtable and,
/// where `sequences` says, a format 14 subtable listing `0` with VS1 as the
/// digit's own glyph.
fn font_with_cmap(sequences: bool) -> Vec<u8> {
    let mut mapping: Vec<u8> = vec![0, 12, 0, 0];
    for word in [28u32, 0, 1, 0x20, 0x7E, 1] {
        mapping.extend_from_slice(&word.to_be_bytes());
    }
    // One record, VS1, whose Default table follows the header: one range,
    // `0` alone.
    let mut uvs: Vec<u8> = vec![0, 14];
    uvs.extend_from_slice(&29u32.to_be_bytes());
    uvs.extend_from_slice(&1u32.to_be_bytes());
    uvs.extend_from_slice(&[0x00, 0xFE, 0x00]);
    uvs.extend_from_slice(&21u32.to_be_bytes());
    uvs.extend_from_slice(&0u32.to_be_bytes());
    uvs.extend_from_slice(&1u32.to_be_bytes());
    uvs.extend_from_slice(&[0x00, 0x00, 0x30, 0]);
    let mut subtables = vec![(3u16, 10u16, mapping)];
    if sequences {
        subtables.insert(0, (0, 5, uvs));
    }
    let mut cmap: Vec<u8> = vec![0, 0];
    cmap.extend_from_slice(&(subtables.len() as u16).to_be_bytes());
    let mut offset = 4 + 8 * subtables.len() as u32;
    for (platform, encoding, subtable) in &subtables {
        cmap.extend_from_slice(&platform.to_be_bytes());
        cmap.extend_from_slice(&encoding.to_be_bytes());
        cmap.extend_from_slice(&offset.to_be_bytes());
        offset += subtable.len() as u32;
    }
    for (_, _, subtable) in &subtables {
        cmap.extend_from_slice(subtable);
    }
    // One table, at 28 bytes: the header, 12, and its record, 16.
    let mut font: Vec<u8> = vec![0, 1, 0, 0, 0, 1, 0, 16, 0, 0, 0, 0];
    font.extend_from_slice(b"cmap");
    font.extend_from_slice(&0u32.to_be_bytes());
    font.extend_from_slice(&28u32.to_be_bytes());
    font.extend_from_slice(&(cmap.len() as u32).to_be_bytes());
    font.extend_from_slice(&cmap);
    font
}

#[test]
fn asking_for_variation_sequences_allocates_once_a_font_at_most() {
    // A font without them never allocates; one with them makes its cache
    // the first time it is asked, and never again.
    let plain = Font::from_data(font_with_cmap(false), 0);
    let sequenced = Font::from_data(font_with_cmap(true), 0);
    assert!(!plain.has_variation_sequences() && sequenced.has_variation_sequences());
    let ask = |font: &Font| {
        let mut listed = 0;
        for base in '0'..='9' {
            for selector in ['\u{FE00}', '\u{FE01}', '\u{E0100}'] {
                listed += usize::from(font.maps_variation_sequence(base, selector));
            }
        }
        listed
    };
    assert_eq!(count_allocations(|| assert_eq!(ask(&plain), 0)), 0);
    assert_eq!(count_allocations(|| assert_eq!(ask(&sequenced), 1)), 1);
    assert_eq!(count_allocations(|| assert_eq!(ask(&sequenced), 1)), 0);
}

/// The same, against the platform's own fonts and its own backend.
///
/// The tests above build a layer from names with an override, which is the
/// shape of the thing and runs anywhere. These ask the real system
/// collection — DirectWrite's, fontconfig's or Core Text's — whose answers
/// are long, whose backends ask the platform, and whose families load fonts
/// and read charsets. Warm, all of that has to come from memory: a single
/// allocation here is a per-run allocation in a shaper.
#[cfg(all(any(windows, unix), feature = "system"))]
mod system {
    use super::*;
    use fontwich::Attributes;

    /// Runs worth asking for, with a generic each.
    const RUNS: &[(GenericFamily, [u8; 4], Option<&str>)] = &[
        (GenericFamily::SansSerif, *b"Latn", Some("en-US")),
        (GenericFamily::Serif, *b"Latn", Some("en-US")),
        (GenericFamily::Monospace, *b"Latn", None),
        (GenericFamily::SystemUi, *b"Cyrl", None),
        (GenericFamily::SansSerif, *b"Arab", Some("ar")),
        (GenericFamily::SansSerif, *b"Hani", Some("ja")),
        (GenericFamily::Serif, *b"Hani", Some("zh-Hant")),
        (GenericFamily::SansSerif, *b"Hebr", None),
    ];

    /// Characters a run has to place across several scripts, including the
    /// shared ones that reach another script's font.
    const CLUSTERS: &str = "Aa1 。、「」— … ← ① ∑ é ñ ©";

    /// The system collection, or `None` on a machine with no fonts — where
    /// every assertion here would be about an empty answer, and a bare
    /// container should skip rather than pass for the wrong reason.
    fn collection() -> Option<Collection> {
        let collection = Collection::system();
        let any = collection.layers().any(|layer| !layer.is_empty());
        any.then_some(collection)
    }

    /// Each run's generic and key, then each cluster the run's families do
    /// not map walked as a miss, as a layout does: how many clusters found a
    /// font.
    fn walk(collection: &Collection) -> usize {
        let mut drawn = 0;
        for &(generic, script, language) in RUNS {
            let request = text(&script, language);
            let listed = collection.resolve(&FontFamilyName::Generic(generic), &request);
            let run = collection.fallback(&collection.key(&request));
            for c in CLUSTERS.chars() {
                let maps = |family: &Family| {
                    family
                        .match_font(Attributes::default(), true)
                        .is_some_and(|font| font.charset().contains(c))
                };
                let found = listed.iter().any(maps)
                    || run.iter().any(maps)
                    || collection
                        .char_fallback(c, Presentation::Text, &request)
                        .any(|family| maps(&family));
                drawn += usize::from(found);
            }
        }
        drawn
    }

    #[test]
    fn a_warm_walk_over_clusters_allocates_nothing() {
        let Some(collection) = collection() else {
            return;
        };
        // Twice: the first fills the collection's answers and loads the
        // families the walk reaches, the second reads them.
        for _ in 0..2 {
            walk(&collection);
        }
        let mut drawn = 0;
        let allocated = count_allocations(|| {
            drawn = walk(&collection);
        });
        // Every run has to have drawn nearly everything, or the walk stopped
        // early and this measured a few families rather than the answers.
        let wanted = (CLUSTERS.chars().count() - 1) * RUNS.len();
        assert!(drawn >= wanted, "drew only {drawn} of {wanted} clusters");
        assert_eq!(allocated, 0, "a warm cluster walk allocated");
    }
}
