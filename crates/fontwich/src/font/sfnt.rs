//! A few tables of a font on disk, without reading the file.
//!
//! What the collection wants from a system font — attributes, a charset — is
//! in small tables, and the file around them can be twenty megabytes of
//! outlines. So a font is opened here by its table directory, and each table
//! is read on its own when asked for.

use alloc::vec::Vec;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The most this reads at once. The largest table anything here wants is a
/// cmap, and the largest real ones — a pan-Unicode font's format 12 — are
/// a few hundred kilobytes. A length past this is a broken or hostile font.
pub(crate) const TABLE_LIMIT: u32 = 1 << 22;

/// A font in a file: its table directory, and the file to read tables from.
///
/// One of these is one *file*, not one font of it: [`seat`](Self::seat)
/// moves it to another font of a collection without opening the file again
/// or allocating anything. Scanning a directory is mostly collections —
/// `PingFangUI.ttc` alone holds thirty-two fonts — so opening per font is
/// most of the syscalls a scan makes.
pub(crate) struct FileFont {
    file: File,
    /// How many fonts the file holds: one, or a collection's count.
    fonts: u32,
    /// Whether it is a collection, whose header says where each font's
    /// table directory is. One font's directory starts at zero.
    collection: bool,
    /// The table directory of the font seated now, if one is.
    records: Vec<u8>,
    /// Every read that does not hand its bytes out: the header, a
    /// collection's offset entry, a table directory's size. Kept so that
    /// seating a font allocates nothing.
    scratch: Vec<u8>,
}

impl FileFont {
    /// The font at `index` in the file at `path`, or `None` when the file
    /// cannot be read, is not a font or collection, or has no font at
    /// `index`.
    pub(crate) fn open(path: &Path, index: u32) -> Option<Self> {
        let mut font = Self::opened(path)?;
        font.seat(index).then_some(font)
    }

    /// The file at `path` with no font of it seated yet, or `None` when it
    /// cannot be read or is not a font or collection.
    ///
    /// For a scan, which wants [`fonts`](Self::fonts) before it decides what
    /// to read: one open serves every font in the file.
    pub(crate) fn opened(path: &Path) -> Option<Self> {
        let mut font = Self {
            file: File::open(path).ok()?,
            fonts: 0,
            collection: false,
            records: Vec::new(),
            scratch: Vec::new(),
        };
        let mut header = Vec::new();
        font.read_into(0, 12, &mut header).then_some(())?;
        (font.fonts, font.collection) = match header.get(..4)? {
            b"ttcf" => (read_u32(&header, 8)?, true),
            [0, 1, 0, 0] | b"OTTO" | b"true" => (1, false),
            _ => return None,
        };
        Some(font)
    }

    /// How many fonts the file holds: one, or a collection's count.
    pub(crate) fn fonts(&self) -> u32 {
        self.fonts
    }

    /// Reads the table directory of the font at `index`, so that
    /// [`locate`](Self::locate) and [`table`](Self::table) answer for it.
    ///
    /// False, leaving the font seated where it was, when the file has no
    /// font at `index` or its directory cannot be read.
    pub(crate) fn seat(&mut self, index: u32) -> bool {
        if index >= self.fonts {
            return false;
        }
        let start = if self.collection {
            let mut entry = core::mem::take(&mut self.scratch);
            let read = self.read_into(12 + 4 * u64::from(index), 4, &mut entry);
            let start = read.then(|| read_u32(&entry, 0)).flatten();
            self.scratch = entry;
            match start {
                Some(start) => u64::from(start),
                None => return false,
            }
        } else {
            0
        };
        let mut directory = core::mem::take(&mut self.scratch);
        let tables = self
            .read_into(start, 12, &mut directory)
            .then(|| read_u16(&directory, 4))
            .flatten();
        self.scratch = directory;
        let Some(tables) = tables else {
            return false;
        };
        let mut records = core::mem::take(&mut self.records);
        let read = self.read_into(start + 12, 16 * u32::from(tables), &mut records);
        self.records = records;
        read
    }

    /// `len` bytes at `at`, or `None` if the file is shorter.
    pub(crate) fn read(&mut self, at: u64, len: u32) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        self.read_into(at, len, &mut bytes).then_some(bytes)
    }

    /// `len` bytes at `at`, into `buffer`, reusing whatever it has already
    /// allocated. False if the file is shorter, leaving `buffer` to be
    /// written over rather than read.
    fn read_into(&mut self, at: u64, len: u32, buffer: &mut Vec<u8>) -> bool {
        if len > TABLE_LIMIT {
            return false;
        }
        buffer.clear();
        buffer.resize(len as usize, 0);
        self.file.seek(SeekFrom::Start(at)).is_ok() && self.file.read_exact(buffer).is_ok()
    }

    /// Where table `tag` is in the file, as `(offset, length)`.
    pub(crate) fn locate(&self, tag: &[u8; 4]) -> Option<(u32, u32)> {
        let record = self
            .records
            .as_chunks::<16>()
            .0
            .iter()
            .find(|record| &record[..4] == tag)?;
        Some((read_u32(record, 8)?, read_u32(record, 12)?))
    }

    /// Table `tag`, or its first `most` bytes if it is longer.
    pub(crate) fn table(&mut self, tag: &[u8; 4], most: u32) -> Option<Vec<u8>> {
        let (offset, length) = self.locate(tag)?;
        self.read(u64::from(offset), length.min(most))
    }

    /// Table `tag` into `buffer`, as [`read_into`](Self::read_into) reads.
    pub(crate) fn table_into(&mut self, tag: &[u8; 4], most: u32, buffer: &mut Vec<u8>) -> bool {
        let Some((offset, length)) = self.locate(tag) else {
            return false;
        };
        self.read_into(u64::from(offset), length.min(most), buffer)
    }
}

/// The big-endian `u16` at `at`.
pub(super) fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// The big-endian `u32` at `at`.
pub(crate) fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fonts::{
        EN_US, Temporary, WINDOWS, font_collection, font_named, font_with_names,
    };
    use read_fonts::tables::name::NameId;

    #[test]
    fn a_collection_is_read_one_font_at_a_time_from_one_open() {
        let bytes =
            font_collection(&[font_named("Alpha"), font_named("Beta"), font_named("Gamma")]);
        let file = Temporary::new("seat.ttc", &bytes);
        let mut font = FileFont::opened(file.path()).expect("a collection");
        assert_eq!(font.fonts(), 3);
        // Seated in any order, forwards and back, off one file handle.
        let mut table = Vec::new();
        for index in [0, 2, 1, 2, 0] {
            assert!(font.seat(index), "no font at {index}");
            assert!(font.table_into(b"name", TABLE_LIMIT, &mut table));
            let names = crate::font::standalone_name_table(&table);
            let (name, _) = names
                .as_ref()
                .and_then(crate::font::names::family_names)
                .expect("a name");
            assert_eq!(name, ["Alpha", "Beta", "Gamma"][index as usize]);
        }
        assert!(!font.seat(3), "seated a font that is not there");
    }

    #[test]
    fn a_lone_font_holds_one_font_at_index_zero() {
        let bytes = font_named("Alpha");
        let file = Temporary::new("seat.ttf", &bytes);
        let mut font = FileFont::opened(file.path()).expect("a font");
        assert_eq!(font.fonts(), 1);
        assert!(font.seat(0));
        assert!(!font.seat(1));
    }

    #[test]
    fn a_file_that_is_not_a_font_is_not_opened() {
        // `Font::from_path` says it answers `None` for a file that is not a
        // font, and reads its header through here to know.
        let file = Temporary::new("prose.ttf", b"this is not a font at all, nor a collection");
        assert!(FileFont::opened(file.path()).is_none());
        assert!(FileFont::open(file.path(), 0).is_none());
        assert!(crate::Font::from_path(file.path(), 0).is_none());
    }

    #[test]
    fn a_buffer_is_written_over_rather_than_appended_to() {
        let bytes = font_with_names(&[(WINDOWS, EN_US, NameId::FAMILY_NAME, "Alpha")]);
        let file = Temporary::new("reuse.ttf", &bytes);
        let mut font = FileFont::open(file.path(), 0).expect("a font");
        let mut buffer = alloc::vec![0xAA; 4096];
        assert!(font.table_into(b"name", TABLE_LIMIT, &mut buffer));
        assert_eq!(Some(buffer.clone()), font.table(b"name", TABLE_LIMIT));
        // And a table that is not there leaves the buffer alone to be
        // written over, not silently kept as the last one read.
        assert!(!font.table_into(b"nope", TABLE_LIMIT, &mut buffer));
    }
}
