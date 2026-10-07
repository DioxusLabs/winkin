//! Which variation sequences a font maps: its `cmap`'s format 14 subtable,
//! asked a pair at a time through a small cache made the first time the
//! font is asked.
//!
//! A variation sequence is a character followed by a variation selector,
//! VS1 to VS16 (U+FE00 to U+FE0F) or VS17 to VS256 (U+E0100 to U+E01EF),
//! asking for one glyph of the character in particular: an ideographic
//! variant, a standardized form of a mathematical symbol, the emoji or the
//! text presentation of an emoji. A font lists the sequences it has in its
//! format 14 subtable, selector by selector: the characters whose own glyph
//! the sequence is (a Default UVS range), and the characters it maps to a
//! glyph of their own (a Non-Default UVS mapping).
//!
//! # Why not in the charset
//!
//! Few fonts have the subtable: 63 of 17,778 faces on Windows, in Google
//! Fonts and in Noto, the Japanese, Korean and Chinese ones, two math fonts
//! and the emoji fonts.
//! But for a Japanese font it is larger than the mapping itself: 14,787
//! pairs over 13,319 characters, 27.4 KB of subtable, which kept as a
//! [`Charset`](super::Charset) a selector is 17.5 KB, and as a mask of
//! selectors a character 106.5 KB. A layer's snapshots are immutable and
//! shared, reading the subtable at scan is a read the scan otherwise avoids,
//! and a caller asks only where its text has a selector, which is rare.
//!
//! So a scan notes where the subtable is, from the encoding records it
//! reads anyway, and nothing else: for a font on disk, where the Unicode
//! Variation Sequences record (platform 0, encoding 5) says, which is where
//! the format keeps it; for a font from bytes, at the first record whose
//! subtable is format 14, as harfrust finds it. The two differ only for a
//! font keeping it elsewhere, which the format does not allow, and there
//! the font on disk has none. The first time a font is asked about a
//! pair it makes a cache of 256 answers, one kilobyte, and a font on disk
//! reads its subtable then too, once. A font never asked, and a font with
//! no subtable, allocates nothing.
//!
//! # The cache
//!
//! Direct-mapped, as HarfBuzz's `hb_cache_t` is: 256 `AtomicU32`s, each
//! holding one answer, which a later pair falling in the same slot replaces.
//! A pair's slot is its character's low eight bits XOR its selector's number
//! (0 for VS1 to 255 for VS256) times an odd constant, so that neighbouring
//! characters with one selector take neighbouring slots, and one character's
//! selectors spread over as many. The slot and the selector give the
//! character's low eight bits back, so an entry holds only the rest of the
//! key:
//!
//! | Bits | Field |
//! |---:|---|
//! | 0..8 | the selector's number |
//! | 8..21 | the character's bits above its low eight |
//! | 21..30 | zero |
//! | 30 | the subtable lists the pair |
//! | 31 | the entry holds an answer |
//!
//! Loads and stores are relaxed. An entry is one word, written whole, so a
//! reader finds an answer or none, never half of one; two threads that miss
//! on the same slot at once each read the subtable, and the last store
//! stands. A miss asks read-fonts' [`Cmap14::map_variant`], as harfrust's
//! charmap does: binary searches over the subtable's records, each bounded
//! by the data, and a record that cannot be read is not a pair, never a
//! panic.
//!
//! Not read-fonts' owned `Font::charmap().map_unicode_variant`, which asks
//! the same `map_variant`: it needs the font held whole, where a font on
//! disk here reads its subtable alone, and it answers with a glyph,
//! resolving a Default UVS entry through the font's Unicode map, where the
//! question here is whether the subtable lists the pair, the base's own
//! mapping being the charset's to answer.

use alloc::boxed::Box;
use core::fmt;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::sync::Once;
use read_fonts::tables::cmap::{Cmap14, MapVariant};
use read_fonts::types::Tag;
use read_fonts::{FontData, FontRead, FontRef, TableProvider};

use super::Source;

/// How many answers a font's cache holds.
const ENTRIES: usize = 256;

/// An entry holds an answer.
const VALID: u32 = 1 << 31;

/// The subtable lists the entry's pair.
const LISTED: u32 = 1 << 30;

/// The bits of an entry that hold its key: the selector's number and the
/// character's bits above its low eight.
const KEY: u32 = (1 << 21) - 1;

/// What a selector's number is multiplied by for its slot: odd, so that the
/// 256 selectors of one character take 256 slots.
const SPREAD: u32 = 0x9D;

/// A variation selector, by its number: 0 for VS1 (U+FE00) to 15 for VS16
/// (U+FE0F), and 16 for VS17 (U+E0100) to 255 for VS256 (U+E01EF).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct Selector(u8);

impl Selector {
    /// `ch` as a selector, or `None` where it is not one.
    pub(crate) fn new(ch: char) -> Option<Self> {
        let number = match u32::from(ch) {
            value @ 0xFE00..=0xFE0F => value - 0xFE00,
            value @ 0xE0100..=0xE01EF => value - 0xE0100 + 16,
            _ => return None,
        };
        u8::try_from(number).ok().map(Self)
    }

    /// The selector's code point.
    fn value(self) -> u32 {
        let number = u32::from(self.0);
        if number < 16 {
            0xFE00 + number
        } else {
            0xE0100 + number - 16
        }
    }

    /// Where the pair of `base` and this selector goes in a cache, and the
    /// key its entry holds (see the module documentation).
    fn slot_and_key(self, base: char) -> (usize, u32) {
        let (base, number) = (u32::from(base), u32::from(self.0));
        let slot = (base ^ number.wrapping_mul(SPREAD)) & 0xFF;
        // At most 255, which a `usize` holds on every target.
        let slot = usize::try_from(slot).unwrap_or(0);
        (slot, ((base >> 8) << 8 | number) & KEY)
    }
}

/// Where a font's format 14 subtable is, and the answers read from it so
/// far: shared by every copy of the font, in every snapshot of its layer.
pub(super) struct Sequences {
    /// Where the subtable starts: in the font's bytes, for a font from
    /// bytes; in its file, for a font on disk. Table offsets count from the
    /// start of either, a collection's included.
    at: u32,
    /// The most bytes it can take: to the end of the `cmap` table.
    room: u32,
    /// Made the first time the font is asked about a pair.
    cache: Once<Box<Cache>>,
}

/// A font's answers, and for a font on disk its subtable.
struct Cache {
    entries: [AtomicU32; ENTRIES],
    /// A font on disk's subtable, read from its file when the cache was
    /// made, and empty where it could not be. Empty for a font from bytes,
    /// whose subtable is read where the bytes are.
    subtable: Box<[u8]>,
}

impl Sequences {
    /// A subtable at `at` in a font's bytes or file, with `room` bytes to
    /// the end of its `cmap`.
    pub(crate) fn new(at: u32, room: u32) -> Self {
        Self {
            at,
            room,
            cache: Once::new(),
        }
    }

    /// Where the format 14 subtable of `font`, read from bytes, is in them,
    /// or `None` where it has none: the first subtable the `cmap`'s encoding
    /// records point to that is format 14, as harfrust takes it.
    pub(super) fn from_font(font: &FontRef<'_>) -> Option<Self> {
        let table = font
            .table_directory()
            .table_records()
            .iter()
            .find(|record| record.tag() == Tag::new(b"cmap"))?;
        let cmap = font.cmap().ok()?;
        let (index, _) = cmap.uvs_subtable()?;
        let offset = cmap
            .encoding_records()
            .get(usize::from(index))?
            .subtable_offset()
            .to_u32();
        let at = table.offset().checked_add(offset)?;
        let room = table.length().checked_sub(offset)?;
        Some(Self::new(at, room))
    }

    /// Whether the subtable lists `base` with `selector`, as the default
    /// glyph of `base` or as a glyph of its own: the cache's answer, or
    /// read-fonts' where the cache has none, which it then keeps. `source`
    /// is where the font's bytes are.
    pub(super) fn lists(&self, source: &Source, base: char, selector: Selector) -> bool {
        let cache = self.cache.get_or_init(|| {
            Box::new(Cache {
                entries: [const { AtomicU32::new(0) }; ENTRIES],
                subtable: self.read(source),
            })
        });
        let (slot, key) = selector.slot_and_key(base);
        let Some(entry) = cache.entries.get(slot) else {
            return false;
        };
        let held = entry.load(Ordering::Relaxed);
        if held & VALID != 0 && held & KEY == key {
            return held & LISTED != 0;
        }
        let listed = self.look_up(source, &cache.subtable, base, selector);
        let answer = if listed { LISTED } else { 0 };
        entry.store(VALID | answer | key, Ordering::Relaxed);
        listed
    }

    /// Whether the cache has been made, for heap accounting and tests.
    pub(super) fn is_cached(&self) -> bool {
        self.cache.get().is_some()
    }

    /// What it holds on the heap beyond itself: the cache, once made, and
    /// the subtable it read.
    pub(crate) fn heap(&self) -> usize {
        self.cache.get().map_or(0, |cache| {
            core::mem::size_of::<Cache>() + cache.subtable.len()
        })
    }

    /// The subtable of a font on disk, read from its file: no longer than
    /// its own length says, its `cmap` holds, or [`TABLE_LIMIT`] allows. Empty for
    /// a font from bytes, and where it cannot be read or is not format 14.
    ///
    /// [`TABLE_LIMIT`]: super::sfnt::TABLE_LIMIT
    fn read(&self, source: &Source) -> Box<[u8]> {
        #[cfg(feature = "std")]
        if let Source::Path(path) = source {
            return read_file(path, self.at, self.room).unwrap_or_default();
        }
        let _ = source;
        Box::default()
    }

    /// Whether the subtable lists `base` with `selector`, asked of the
    /// subtable itself: in `source`'s bytes for a font from bytes, `read`
    /// for a font on disk. A Non-Default mapping to glyph 0 is `.notdef`,
    /// which is no mapping, as a charset has it.
    fn look_up(&self, source: &Source, read: &[u8], base: char, selector: Selector) -> bool {
        let bytes = match source {
            Source::Data(bytes) => {
                let data = bytes.data();
                let start = usize::try_from(self.at).unwrap_or(usize::MAX);
                let room = usize::try_from(self.room).unwrap_or(usize::MAX);
                let end = start.saturating_add(room).min(data.len());
                data.get(start..end).unwrap_or_default()
            }
            _ => read,
        };
        let Ok(subtable) = Cmap14::read(FontData::new(bytes)) else {
            return false;
        };
        match subtable.map_variant(u32::from(base), selector.value()) {
            Some(MapVariant::UseDefault) => true,
            Some(MapVariant::Variant(glyph)) => glyph.to_u32() != 0,
            None => false,
        }
    }
}

impl fmt::Debug for Sequences {
    // Not the 256 entries.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sequences")
            .field("at", &self.at)
            .field("room", &self.room)
            .field("cached", &self.is_cached())
            .finish()
    }
}

/// The format 14 subtable at `at` in the file at `path`, with `room` bytes
/// to the end of its `cmap`: `None` where the file cannot be read or the
/// subtable is not format 14.
#[cfg(feature = "std")]
fn read_file(path: &std::path::Path, at: u32, room: u32) -> Option<Box<[u8]>> {
    use super::sfnt::{FileFont, TABLE_LIMIT, read_u16, read_u32};

    let mut file = FileFont::opened(path)?;
    let header = file.read(u64::from(at), 6.min(room))?;
    if read_u16(&header, 0)? != 14 {
        return None;
    }
    let length = read_u32(&header, 2)?.min(room).min(TABLE_LIMIT);
    Some(file.read(u64::from(at), length)?.into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_selector_has_a_number_and_nothing_else_does() {
        for (value, number) in (0xFE00..=0xFE0F).chain(0xE0100..=0xE01EF).zip(0u32..) {
            let ch = char::from_u32(value).expect("a selector");
            let selector = Selector::new(ch).expect("a selector");
            assert_eq!(u32::from(selector.0), number);
            assert_eq!(selector.value(), value);
        }
        for ch in [
            '\u{FDFF}',
            '\u{FE10}',
            '\u{E00FF}',
            '\u{E01F0}',
            'a',
            '\u{180B}',
        ] {
            assert_eq!(Selector::new(ch), None, "{ch:?}");
        }
    }

    /// The slot and the selector give the character's low eight bits back,
    /// so two pairs with one slot and one key are one pair; and the
    /// neighbours of a character with one selector, and one character's
    /// selectors, take slots of their own.
    #[test]
    fn a_slot_and_a_key_name_one_pair() {
        let pairs = [0x30u32, 0x845B, 0x845C, 0x9089, 0x2A6D6, 0x10FFFF];
        let mut seen: alloc::vec::Vec<(usize, u32, u32, u8)> = alloc::vec::Vec::new();
        for base in pairs.iter().filter_map(|&value| char::from_u32(value)) {
            for number in 0..=255u8 {
                let (slot, key) = Selector(number).slot_and_key(base);
                assert!(slot < ENTRIES);
                assert_eq!(key & !KEY, 0);
                seen.push((slot, key, u32::from(base), number));
            }
        }
        seen.sort_unstable();
        for pair in seen.windows(2) {
            assert!(
                (pair[0].0, pair[0].1) != (pair[1].0, pair[1].1),
                "{pair:?} share a slot and a key"
            );
        }
        let slots = |pairs: &mut dyn Iterator<Item = (char, Selector)>| {
            let mut slots: alloc::vec::Vec<usize> = pairs
                .map(|(base, selector)| selector.slot_and_key(base).0)
                .collect();
            slots.sort_unstable();
            slots.dedup();
            slots.len()
        };
        let vs17 = Selector::new('\u{E0100}').expect("a selector");
        let neighbours = (0x8400..0x8500).filter_map(char::from_u32);
        assert_eq!(slots(&mut neighbours.map(|base| (base, vs17))), 256);
        let selectors = (0..=255).map(|number| ('\u{9089}', Selector(number)));
        assert_eq!(slots(&mut selectors.into_iter()), 256);
    }

    use crate::Font;
    use crate::test_fonts::{cmap, font_with_tables, format12, format14};
    use alloc::vec::Vec;

    /// ASCII, ∩, two pages of ideographs and U+F900, with sequences of each
    /// kind: Default ranges, one of them of a base the font does not map;
    /// Non-Default mappings, one to glyph 0 and one of a base it does not
    /// map; and a pair listed both ways.
    fn sequenced() -> Vec<u8> {
        let mapping = format12(&[
            (0x20, 0x7E),
            (0x2229, 0x2229),
            (0x8400, 0x85FF),
            (0xF900, 0xF900),
        ]);
        let sequences = format14(&[
            (0xFE00, &[(0x2229, 0)], &[(0x30, 900), (0x845B, 901)]),
            (0xFE01, &[], &[(0x2229, 902), (0x8460, 0)]),
            (0xFE0F, &[(0x23, 0), (0x2764, 0)], &[]),
            (0xE0100, &[(0x8400, 0xFF)], &[(0x845B, 903), (0x8FBB, 904)]),
            (0xE0101, &[(0x845B, 0)], &[]),
            (0xE01E0, &[], &[(0x845B, 905), (0xF900, 906)]),
        ]);
        font_with_tables(&[(*b"cmap", cmap(&[(0, 5, sequences), (3, 10, mapping)]))])
    }

    /// Every character the tests ask about with every selector: what the
    /// font maps and the characters beside it, and what it does not.
    fn bases() -> impl Iterator<Item = char> {
        (0x1F..0x81)
            .chain(0x2220..0x2240)
            .chain(0x83F0..0x8610)
            .chain([0x2764, 0x8FBB, 0xF8FF, 0xF900, 0xF901, 0x10FFFF])
            .filter_map(char::from_u32)
    }

    /// VS1 to VS256.
    fn selectors() -> impl Iterator<Item = char> {
        (0xFE00..=0xFE0F)
            .chain(0xE0100..=0xE01EF)
            .filter_map(char::from_u32)
    }

    /// What the font at `index` in `bytes` answers for a pair, asked of
    /// read-fonts directly, as harfrust maps one: listed as a glyph of its
    /// own that is not `.notdef`, or as the base's own glyph; and its
    /// charset maps the base.
    fn direct(bytes: &[u8], index: u32) -> impl Fn(char, char) -> bool + '_ {
        let font = FontRef::from_index(bytes, index).ok();
        let charset = font
            .as_ref()
            .map(super::super::Charset::from_font)
            .unwrap_or_default();
        let subtable = font
            .and_then(|font| font.cmap().ok())
            .and_then(|cmap| cmap.uvs_subtable())
            .map(|(_, subtable)| subtable);
        move |base, selector| {
            charset.contains(base)
                && subtable.as_ref().is_some_and(|subtable| {
                    match subtable.map_variant(u32::from(base), u32::from(selector)) {
                        Some(MapVariant::UseDefault) => true,
                        Some(MapVariant::Variant(glyph)) => glyph.to_u32() != 0,
                        None => false,
                    }
                })
        }
    }

    /// Every character of [`bases`] with every selector, and read-fonts'
    /// answer for each from the font at `index` in `bytes`.
    fn pairs(bytes: &[u8], index: u32) -> Vec<(char, char, bool)> {
        let direct = direct(bytes, index);
        bases()
            .flat_map(|base| selectors().map(move |selector| (base, selector)))
            .map(|(base, selector)| (base, selector, direct(base, selector)))
            .collect()
    }

    /// `font` answers every one of `pairs` as it says, in order and then in
    /// reverse, so that pairs the cache has evicted are asked again.
    fn agrees(font: &Font, pairs: &[(char, char, bool)]) {
        for &(base, selector, listed) in pairs.iter().chain(pairs.iter().rev()) {
            assert_eq!(
                font.maps_variation_sequence(base, selector),
                listed,
                "U+{:04X} U+{:04X}",
                u32::from(base),
                u32::from(selector)
            );
        }
    }

    /// Asked from its bytes, a font answers every pair as read-fonts does, a
    /// Default pair only where it maps the base and a Non-Default pair of
    /// glyph 0 not at all, whatever the cache evicted in between: 154,000
    /// pairs through 256 slots.
    #[test]
    fn a_font_from_bytes_answers_every_pair_as_read_fonts_does() {
        let bytes = sequenced();
        let font = Font::from_data(bytes.clone(), 0);
        assert!(font.has_variation_sequences());
        assert!(!font.has_sequence_cache(), "nothing is made until asked");
        let pairs = pairs(&bytes, 0);
        agrees(&font, &pairs);
        assert!(font.has_sequence_cache());
        let listed = |base: char, selector: char| font.maps_variation_sequence(base, selector);
        assert!(listed('0', '\u{FE00}') && listed('\u{2229}', '\u{FE00}'));
        assert!(listed('\u{845B}', '\u{E0100}') && listed('\u{8401}', '\u{E0100}'));
        assert!(listed('#', '\u{FE0F}') && listed('\u{F900}', '\u{E01E0}'));
        assert!(
            !listed('\u{2764}', '\u{FE0F}'),
            "a Default pair of a base it does not map"
        );
        assert!(
            !listed('\u{8FBB}', '\u{E0100}'),
            "a pair of a base it does not map"
        );
        assert!(!listed('\u{8460}', '\u{FE01}'), "a pair mapped to .notdef");
        assert!(!listed('\u{845B}', '\u{FE02}') && !listed('\u{845B}', 'a'));
        assert!(!listed('\u{8500}', '\u{E0100}'), "past the Default range");
        // A copy shares what was read.
        assert!(font.clone().has_sequence_cache());
    }

    /// Two pairs in one slot take turns in it, each answering right: one
    /// selector on characters 256 apart, one listed and one not.
    #[test]
    fn pairs_sharing_a_slot_evict_each_other_and_answer_right() {
        let font = Font::from_data(sequenced(), 0);
        let vs17 = Selector::new('\u{E0100}').expect("a selector");
        let (a, b) = ('\u{845B}', '\u{855B}');
        assert_eq!(vs17.slot_and_key(a).0, vs17.slot_and_key(b).0);
        assert_ne!(vs17.slot_and_key(a).1, vs17.slot_and_key(b).1);
        for _ in 0..4 {
            assert!(font.maps_variation_sequence(a, '\u{E0100}'));
            assert!(!font.maps_variation_sequence(b, '\u{E0100}'));
        }
    }

    /// A font without variation sequences says so, answers no to every
    /// pair, and never makes a cache.
    #[test]
    fn a_font_without_sequences_answers_no_and_makes_nothing() {
        let table = cmap(&[(3, 10, format12(&[(0x20, 0x7E)]))]);
        let font = Font::from_data(font_with_tables(&[(*b"cmap", table)]), 0);
        assert!(!font.has_variation_sequences());
        for selector in selectors() {
            assert!(!font.maps_variation_sequence('a', selector));
        }
        assert!(!font.has_sequence_cache());
    }

    /// Nothing a subtable holds panics: a subtable of another format where
    /// the sequences' record points, one cut short, one claiming four
    /// billion records, and a record pointing past the table.
    #[test]
    fn a_broken_subtable_lists_nothing_and_never_panics() {
        let mapping = || format12(&[(0x20, 0x7E), (0x8400, 0x85FF)]);
        let whole = format14(&[(0xE0100, &[(0x8400, 0xFF)], &[(0x845B, 903)])]);
        let mut short = whole.clone();
        short.truncate(whole.len() - 3);
        let mut huge = whole.clone();
        huge[6..10].copy_from_slice(&u32::MAX.to_be_bytes());
        let mut past = cmap(&[(0, 5, whole), (3, 10, mapping())]);
        // The sequences' record's offset.
        past[8..12].copy_from_slice(&0x00FF_FFFFu32.to_be_bytes());
        let tables = [
            cmap(&[(0, 5, mapping()), (3, 10, mapping())]),
            cmap(&[(0, 5, short), (3, 10, mapping())]),
            cmap(&[(0, 5, huge), (3, 10, mapping())]),
            past,
        ];
        for table in tables {
            let font = Font::from_data(font_with_tables(&[(*b"cmap", table)]), 0);
            for selector in selectors() {
                for base in ['\u{845B}', '\u{8400}', 'a'] {
                    // The subtable cut short still has the record it keeps;
                    // what matters is that every answer comes back.
                    let _ = font.maps_variation_sequence(base, selector);
                }
            }
        }
    }

    #[cfg(feature = "std")]
    mod on_disk {
        use super::*;
        use crate::test_fonts::{Temporary, font_collection};
        use std::path::Path;
        use std::sync::Arc;

        /// Asked from its file, a font answers as its bytes do, the second
        /// of a collection too, reading its subtable from the file once.
        #[test]
        fn a_font_on_disk_answers_as_its_bytes_do() {
            let table = cmap(&[(3, 10, format12(&[(0x20, 0x7E)]))]);
            let plain = font_with_tables(&[(*b"cmap", table)]);
            let file = Temporary::new("sequences.ttc", &font_collection(&[plain, sequenced()]));
            let bytes = std::fs::read(file.path()).expect("the file");
            let path: Arc<Path> = Arc::from(file.path());
            let font = Font::from_path(path.clone(), 1).expect("a font");
            assert!(font.has_variation_sequences());
            let pairs = pairs(&bytes, 1);
            assert!(pairs.iter().filter(|&&(.., listed)| listed).count() > 256);
            agrees(&font, &pairs);
            let first = Font::from_path(path, 0).expect("a font");
            assert!(!first.has_variation_sequences());
        }

        /// Eight threads asking one font the same pairs in different orders
        /// at once each get read-fonts' answer every time: an entry is
        /// written whole, and a race at most reads the subtable twice.
        #[test]
        fn threads_asking_at_once_all_answer_right() {
            let bytes = sequenced();
            let font = Font::from_data(bytes.clone(), 0);
            let pairs = pairs(&bytes, 0);
            std::thread::scope(|scope| {
                for (thread, step) in [1usize, 7, 31, 127, 257, 1021, 4093, 8191]
                    .into_iter()
                    .enumerate()
                {
                    let (font, pairs) = (&font, &pairs);
                    scope.spawn(move || {
                        for round in 0..pairs.len() {
                            let (base, selector, listed) =
                                pairs[(round * step + thread) % pairs.len()];
                            assert_eq!(font.maps_variation_sequence(base, selector), listed);
                        }
                    });
                }
            });
        }

        /// Over every pair of the installed fonts with variation sequences
        /// this machine has, and the pairs beside each, a font on disk and
        /// the same font from its bytes answer as read-fonts does. Passed
        /// over where none is installed.
        #[test]
        fn installed_fonts_answer_every_pair_as_read_fonts_does() {
            const FONTS: &[(&str, u32)] = &[
                ("C:/Windows/Fonts/YuGothR.ttc", 0),
                ("C:/Windows/Fonts/msgothic.ttc", 0),
                ("C:/Windows/Fonts/cambria.ttc", 1),
                ("C:/Windows/Fonts/seguiemj.ttf", 0),
                (
                    "/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc",
                    0,
                ),
                ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
            ];
            let mut asked = 0usize;
            for &(path, index) in FONTS {
                let Ok(bytes) = std::fs::read(path) else {
                    continue;
                };
                let Some((_, subtable)) = FontRef::from_index(&bytes, index)
                    .ok()
                    .and_then(|font| font.cmap().ok())
                    .and_then(|cmap| cmap.uvs_subtable())
                else {
                    continue;
                };
                // Each pair listed, the character after it with the same
                // selector, and the character with a selector no font uses.
                let mut asking: Vec<(char, char)> = Vec::new();
                for (base, selector, _) in subtable.iter() {
                    let (Some(base), Some(next), Some(selector)) = (
                        char::from_u32(base),
                        char::from_u32(base + 1),
                        char::from_u32(selector),
                    ) else {
                        continue;
                    };
                    asking.extend([(base, selector), (next, selector), (base, '\u{E01EF}')]);
                }
                let direct = direct(&bytes, index);
                let pairs: Vec<(char, char, bool)> = asking
                    .into_iter()
                    .map(|(base, selector)| (base, selector, direct(base, selector)))
                    .collect();
                let on_disk =
                    Font::from_path(Arc::<Path>::from(Path::new(path)), index).expect("a font");
                assert!(on_disk.has_variation_sequences(), "{path}");
                agrees(&on_disk, &pairs);
                agrees(&Font::from_data(bytes.clone(), index), &pairs);
                asked += pairs.len();
            }
            std::println!("{asked} pairs asked of the fonts installed here");
        }
    }
}
