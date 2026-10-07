//! The default [`FamilyNames`], for a caller with no font
//! collection.
//!
//! This is the fallback position, not the intended one. A caller that already
//! enumerated the system's fonts knows every family name on the device and
//! should say so by implementing the trait. Opening the same two hundred
//! files a second time is waste.
//!
//! What it does is what Skia does. `SkFontMgr_android` reads each font in
//! `fonts.xml` through its scanner and takes `proxy->getFamilyName()`, with
//! the XML's own name winning where there is one. The difference is that
//! Skia's is eager — its constructor loops every `<font>` in every family, and
//! carries `// TODO? make this lazy` — whereas nothing here opens a font until
//! a query needs its name.

use alloc::string::String;
use alloc::vec::Vec;
use std::path::Path;

use super::FamilyNames;
use crate::font::{FileFont, TABLE_LIMIT, family_names, standalone_name_table};

/// A family-name resolver that reads font files.
///
/// Paths already include the directory supplied to
/// [`Android::from_fonts_xml`](super::Android::from_fonts_xml).
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemFonts;

impl FamilyNames for SystemFonts {
    fn family_name(&self, path: &str, index: u32) -> Option<String> {
        // The table directory and the `name` table, not the file: a CJK
        // collection is tens of megabytes, and its names a few hundred
        // bytes. Read rather than mapped, as the scan reads them, so a
        // truncated file is an error rather than a SIGBUS.
        let mut file = FileFont::open(Path::new(path), index)?;
        let mut table = Vec::new();
        file.table_into(b"name", TABLE_LIMIT, &mut table)
            .then_some(())?;
        let name = standalone_name_table(&table)?;
        family_names(&name).map(|(own, _)| own)
    }
}
