//! Fonts built in memory, for tests.
//!
//! Vendoring real fonts would mean binaries and licences in the tree to
//! exercise a handful of branches. These are deterministic and need neither.
//! They carry only the tables the code under test reads.

use alloc::vec::Vec;

use read_fonts::tables::name::NameId;

pub(crate) const WINDOWS: u16 = 3;
pub(crate) const MAC: u16 = 1;
pub(crate) const EN_US: u16 = 0x0409;
pub(crate) const JA: u16 = 0x0411;

/// A font holding exactly `tables`, in the order given.
pub(crate) fn font_with_tables(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let header = 12 + 16 * tables.len();
    let mut font: Vec<u8> = Vec::new();
    font.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // sfnt version
    font.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    font.extend_from_slice(&16u16.to_be_bytes()); // searchRange, unverified
    font.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
    font.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
    let mut offset = header;
    for (tag, bytes) in tables {
        font.extend_from_slice(tag);
        font.extend_from_slice(&0u32.to_be_bytes()); // checksum, unverified
        font.extend_from_slice(&(offset as u32).to_be_bytes());
        font.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        // Tables start on four-byte boundaries.
        offset += bytes.len().next_multiple_of(4);
    }
    for (_, bytes) in tables {
        font.extend_from_slice(bytes);
        font.resize(font.len().next_multiple_of(4), 0);
    }
    font
}

/// A `name` table with these records, each `(platform, language, name id,
/// text)`.
pub(crate) fn name(records: &[(u16, u16, NameId, &str)]) -> Vec<u8> {
    let mut strings: Vec<u8> = Vec::new();
    let mut table: Vec<u8> = Vec::new();
    table.extend_from_slice(&0u16.to_be_bytes()); // format 0
    table.extend_from_slice(&(records.len() as u16).to_be_bytes());
    let storage = 6 + records.len() * 12;
    table.extend_from_slice(&(storage as u16).to_be_bytes());
    for &(platform, language, id, text) in records {
        // Macintosh records are Mac Roman, a single-byte encoding whose ASCII
        // range is ASCII; Windows records are UTF-16BE. Encoding both the same
        // way is what the first draft of this did, and read-fonts rightly
        // refused the result.
        let (encoding, encoded): (u16, Vec<u8>) = if platform == MAC {
            (0, text.bytes().collect())
        } else {
            (
                1,
                text.encode_utf16()
                    .flat_map(|unit| unit.to_be_bytes())
                    .collect(),
            )
        };
        table.extend_from_slice(&platform.to_be_bytes());
        table.extend_from_slice(&encoding.to_be_bytes());
        table.extend_from_slice(&language.to_be_bytes());
        table.extend_from_slice(&id.to_u16().to_be_bytes());
        table.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
        table.extend_from_slice(&(strings.len() as u16).to_be_bytes());
        strings.extend_from_slice(&encoded);
    }
    table.extend_from_slice(&strings);
    table
}

/// An `OS/2` table, version 4, giving a weight class, a width class (1 to 9)
/// and fsSelection flags. Everything else is zero.
pub(crate) fn os2(weight: u16, width: u16, selection: u16) -> Vec<u8> {
    let mut table = alloc::vec![0u8; 96];
    table[0..2].copy_from_slice(&4u16.to_be_bytes());
    table[4..6].copy_from_slice(&weight.to_be_bytes());
    table[6..8].copy_from_slice(&width.to_be_bytes());
    table[62..64].copy_from_slice(&selection.to_be_bytes());
    table
}

/// A `head` table, whose `fontRevision` is the version the font gives
/// itself — what decides which of two copies supersedes the other.
///
/// Only scanning directories reads revisions, and that needs `std`.
#[cfg(feature = "std")]
pub(crate) fn head(revision: u32) -> Vec<u8> {
    let mut table = alloc::vec![0u8; 54];
    table[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    table[4..8].copy_from_slice(&revision.to_be_bytes());
    table[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    table[18..20].copy_from_slice(&1000u16.to_be_bytes());
    table
}

/// A `post` table, version 3, giving an italic angle in OpenType's
/// counter-clockwise degrees.
pub(crate) fn post(italic_angle: f32) -> Vec<u8> {
    let mut table = alloc::vec![0u8; 32];
    table[0..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
    let fixed = (italic_angle * 65536.0) as i32;
    table[4..8].copy_from_slice(&fixed.to_be_bytes());
    table
}

/// An `fvar` table with these axes, as `(tag, min, default, max)`, and no
/// named instances.
pub(crate) fn fvar(axes: &[([u8; 4], f32, f32, f32)]) -> Vec<u8> {
    let fixed = |value: f32| ((value * 65536.0) as i32).to_be_bytes();
    let count = axes.len() as u16;
    let mut table = Vec::new();
    for field in [1u16, 0, 16, 2, count, 20, 0, 4 + 4 * count] {
        table.extend_from_slice(&field.to_be_bytes());
    }
    for (tag, min, default, max) in axes {
        table.extend_from_slice(tag);
        table.extend_from_slice(&fixed(*min));
        table.extend_from_slice(&fixed(*default));
        table.extend_from_slice(&fixed(*max));
        table.extend_from_slice(&[0; 4]);
    }
    table
}

/// The smallest font that carries a `name` table with these records.
pub(crate) fn font_with_names(records: &[(u16, u16, NameId, &str)]) -> Vec<u8> {
    font_with_tables(&[(*b"name", name(records))])
}

/// A font with one English family name.
pub(crate) fn font_named(family: &str) -> Vec<u8> {
    font_with_names(&[(WINDOWS, EN_US, NameId::FAMILY_NAME, family)])
}

/// A `.ttc` holding `fonts`, each as built by [`font_with_tables`].
///
/// Offsets in a collection's table records count from the start of the
/// *file*, not of the font, so every table offset in each font is moved by
/// where the font lands.
pub(crate) fn font_collection(fonts: &[Vec<u8>]) -> Vec<u8> {
    let header = 12 + 4 * fonts.len();
    let mut file: Vec<u8> = Vec::new();
    file.extend_from_slice(b"ttcf");
    file.extend_from_slice(&1u16.to_be_bytes()); // major version
    file.extend_from_slice(&0u16.to_be_bytes()); // minor version
    file.extend_from_slice(&(fonts.len() as u32).to_be_bytes());
    let mut base = header;
    for font in fonts {
        file.extend_from_slice(&(base as u32).to_be_bytes());
        base += font.len();
    }
    for font in fonts {
        let start = file.len();
        file.extend_from_slice(font);
        let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
        for table in 0..tables {
            let at = start + 12 + 16 * table + 8;
            let offset = u32::from_be_bytes(file[at..at + 4].try_into().expect("four bytes"));
            file[at..at + 4].copy_from_slice(&(offset + start as u32).to_be_bytes());
        }
    }
    file
}

/// A `cmap` table holding these subtables, each `(platform, encoding,
/// subtable)`.
pub(crate) fn cmap(subtables: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
    let mut table: Vec<u8> = Vec::new();
    table.extend_from_slice(&0u16.to_be_bytes()); // version
    table.extend_from_slice(&(subtables.len() as u16).to_be_bytes());
    let mut offset = 4 + 8 * subtables.len();
    for (platform, encoding, subtable) in subtables {
        table.extend_from_slice(&platform.to_be_bytes());
        table.extend_from_slice(&encoding.to_be_bytes());
        table.extend_from_slice(&(offset as u32).to_be_bytes());
        offset += subtable.len();
    }
    for (_, _, subtable) in subtables {
        table.extend_from_slice(subtable);
    }
    table
}

/// A format 4 subtable mapping each inclusive range to consecutive glyphs
/// from 1.
pub(crate) fn format4(ranges: &[(u16, u16)]) -> Vec<u8> {
    // Every format 4 subtable ends with a segment for U+FFFF.
    let mut segments: Vec<(u16, u16, u16)> = Vec::new();
    let mut glyph: u16 = 1;
    for &(start, end) in ranges {
        segments.push((start, end, glyph.wrapping_sub(start)));
        glyph += end - start + 1;
    }
    segments.push((0xFFFF, 0xFFFF, 1));
    let count = segments.len() as u16;
    let mut table: Vec<u8> = Vec::new();
    table.extend_from_slice(&4u16.to_be_bytes());
    table.extend_from_slice(&(16 + 8 * count).to_be_bytes()); // length
    table.extend_from_slice(&0u16.to_be_bytes()); // language
    table.extend_from_slice(&(2 * count).to_be_bytes());
    for _ in 0..3 {
        table.extend_from_slice(&0u16.to_be_bytes()); // search fields, unread
    }
    for &(_, end, _) in &segments {
        table.extend_from_slice(&end.to_be_bytes());
    }
    table.extend_from_slice(&0u16.to_be_bytes()); // reserved
    for &(start, _, _) in &segments {
        table.extend_from_slice(&start.to_be_bytes());
    }
    for &(_, _, delta) in &segments {
        table.extend_from_slice(&delta.to_be_bytes());
    }
    for _ in &segments {
        table.extend_from_slice(&0u16.to_be_bytes()); // idRangeOffset
    }
    table
}

/// A format 4 subtable from raw segments, `(start, end, delta, glyphs)`:
/// a segment with glyphs maps through the glyph array, one entry per
/// character, and the rest by delta. The U+FFFF segment is added.
pub(crate) fn format4_segments(segments: &[(u16, u16, i16, &[u16])]) -> Vec<u8> {
    let mut all: Vec<(u16, u16, i16, &[u16])> = segments.to_vec();
    all.push((0xFFFF, 0xFFFF, 1, &[]));
    let count = all.len() as u16;
    let mut array: Vec<u16> = Vec::new();
    let mut offsets: Vec<u16> = Vec::new();
    for (at, &(_, _, _, glyphs)) in all.iter().enumerate() {
        if glyphs.is_empty() {
            offsets.push(0);
        } else {
            // From this segment's own offset word to its first glyph.
            let words = (count as usize - at) + array.len();
            offsets.push((2 * words) as u16);
            array.extend_from_slice(glyphs);
        }
    }
    let mut table: Vec<u8> = Vec::new();
    table.extend_from_slice(&4u16.to_be_bytes());
    table.extend_from_slice(&(16 + 8 * count + 2 * array.len() as u16).to_be_bytes());
    table.extend_from_slice(&0u16.to_be_bytes()); // language
    table.extend_from_slice(&(2 * count).to_be_bytes());
    for _ in 0..3 {
        table.extend_from_slice(&0u16.to_be_bytes()); // search fields, unread
    }
    for &(_, end, _, _) in &all {
        table.extend_from_slice(&end.to_be_bytes());
    }
    table.extend_from_slice(&0u16.to_be_bytes()); // reserved
    for &(start, _, _, _) in &all {
        table.extend_from_slice(&start.to_be_bytes());
    }
    for &(_, _, delta, _) in &all {
        table.extend_from_slice(&delta.to_be_bytes());
    }
    for offset in offsets {
        table.extend_from_slice(&offset.to_be_bytes());
    }
    for glyph in array {
        table.extend_from_slice(&glyph.to_be_bytes());
    }
    table
}

/// A format 12 subtable mapping each inclusive range to consecutive glyphs
/// from 1.
pub(crate) fn format12(ranges: &[(u32, u32)]) -> Vec<u8> {
    let mut table: Vec<u8> = Vec::new();
    table.extend_from_slice(&12u16.to_be_bytes());
    table.extend_from_slice(&0u16.to_be_bytes()); // reserved
    table.extend_from_slice(&((16 + 12 * ranges.len()) as u32).to_be_bytes());
    table.extend_from_slice(&0u32.to_be_bytes()); // language
    table.extend_from_slice(&(ranges.len() as u32).to_be_bytes());
    let mut glyph: u32 = 1;
    for &(start, end) in ranges {
        table.extend_from_slice(&start.to_be_bytes());
        table.extend_from_slice(&end.to_be_bytes());
        table.extend_from_slice(&glyph.to_be_bytes());
        glyph = glyph.wrapping_add(end.wrapping_sub(start)).wrapping_add(1);
    }
    table
}

/// The Default UVS ranges of one selector in a format 14 subtable, each
/// `(start, additional count)`.
pub(crate) type DefaultUvs<'a> = &'a [(u32, u8)];

/// The Non-Default UVS mappings of one selector in a format 14 subtable,
/// each `(character, glyph)`.
pub(crate) type NonDefaultUvs<'a> = &'a [(u32, u16)];

/// A format 14 subtable with a record for each selector, given in ascending
/// order with its Default and Non-Default tables: none where a list is
/// empty.
pub(crate) fn format14(selectors: &[(u32, DefaultUvs<'_>, NonDefaultUvs<'_>)]) -> Vec<u8> {
    let u24 = |value: u32| [(value >> 16) as u8, (value >> 8) as u8, value as u8];
    let header = 10 + 11 * selectors.len();
    let mut records: Vec<u8> = Vec::new();
    let mut tables: Vec<u8> = Vec::new();
    for &(selector, defaults, mappings) in selectors {
        let mut default_offset = 0;
        if !defaults.is_empty() {
            default_offset = header + tables.len();
            tables.extend_from_slice(&(defaults.len() as u32).to_be_bytes());
            for &(start, additional) in defaults {
                tables.extend_from_slice(&u24(start));
                tables.push(additional);
            }
        }
        let mut mapping_offset = 0;
        if !mappings.is_empty() {
            mapping_offset = header + tables.len();
            tables.extend_from_slice(&(mappings.len() as u32).to_be_bytes());
            for &(character, glyph) in mappings {
                tables.extend_from_slice(&u24(character));
                tables.extend_from_slice(&glyph.to_be_bytes());
            }
        }
        records.extend_from_slice(&u24(selector));
        records.extend_from_slice(&(default_offset as u32).to_be_bytes());
        records.extend_from_slice(&(mapping_offset as u32).to_be_bytes());
    }
    let mut table: Vec<u8> = Vec::new();
    table.extend_from_slice(&14u16.to_be_bytes());
    table.extend_from_slice(&((header + tables.len()) as u32).to_be_bytes());
    table.extend_from_slice(&(selectors.len() as u32).to_be_bytes());
    table.extend_from_slice(&records);
    table.extend_from_slice(&tables);
    table
}

/// A file in the temporary directory, removed when dropped.
#[cfg(feature = "std")]
pub(crate) struct Temporary(std::path::PathBuf);

#[cfg(feature = "std")]
impl Temporary {
    pub(crate) fn new(name: &str, bytes: &[u8]) -> Self {
        let path =
            std::env::temp_dir().join(alloc::format!("fontwich-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("a temporary file");
        Self(path)
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
}

#[cfg(feature = "std")]
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
