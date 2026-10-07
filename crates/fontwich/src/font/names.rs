//! Reading a family name out of a font.
//!
//! Two things need this and must agree on the answer: the Android backend,
//! which turns the files `fonts.xml` names into family names, and the
//! collection, which groups fonts by family. Which `name` record wins is a
//! judgement, and a judgement made twice is a judgement that drifts, so it is
//! made here, once.
//!
//! Every reader takes the `name` table, however it was come by: from a font's
//! bytes in hand with [`name_table`], or from a table read on its own out of a
//! file with [`standalone_name_table`].

use alloc::string::String;

use read_fonts::tables::name::{Name, NameId};
use read_fonts::{FontRef, TableProvider};

/// The `name` table of the font at `index` in `data`.
///
/// Takes bytes rather than a path, so it serves fonts that never touched a
/// disk — a download, an embedded font — and can be tested without one.
pub(crate) fn name_table(data: &[u8], index: u32) -> Option<Name<'_>> {
    FontRef::from_index(data, index).ok()?.name().ok()
}

/// A `name` table read on its own, as from a file.
#[cfg(feature = "std")]
pub(crate) fn standalone_name_table(bytes: &[u8]) -> Option<Name<'_>> {
    use read_fonts::{FontData, FontRead};
    Name::read(FontData::new(bytes)).ok()
}

/// Returns the family name in `name`, and the legacy family name if it
/// differs.
///
/// The legacy name is the family record (name ID 1) of a font whose name is
/// its typographic family (ID 16): "Roboto Condensed" for a font of
/// "Roboto", "Arial Black" for one of "Arial". A page may ask for a font by
/// its legacy name, which then names a secondary family.
///
/// # Which record
///
/// Typographic family (ID 16) comes before family (ID 1). ID 1 follows the
/// four-style model, so a font with nine weights splits them across
/// families like "Roboto Light". ID 16 records the real family, "Roboto".
/// Preferring it keeps those nine weights in one family.
///
/// English comes before other languages, and any record before none. Pages
/// write `font-family` in English far more often than not, but a font with
/// no English name should still be nameable.
pub(crate) fn family_names(name: &Name<'_>) -> Option<(String, Option<String>)> {
    let (mut own, mut legacy) = (String::new(), String::new());
    read_both(name, &mut own, &mut legacy).then(|| {
        let legacy = (!legacy.is_empty()).then_some(legacy);
        (own, legacy)
    })
}

/// [`family_names`] into buffers the caller holds across a scan, with the
/// family's name in the font's other languages beside them.
///
/// One set of these serves a whole directory walk, which reads one `name`
/// table per font and would otherwise allocate a handful of strings for
/// each. `legacy` and `others` are left empty where there are none.
#[cfg(all(target_vendor = "apple", feature = "system"))]
pub(crate) fn read_family_names(
    name: &Name<'_>,
    own: &mut String,
    legacy: &mut String,
    others: &mut alloc::vec::Vec<String>,
) -> bool {
    others.clear();
    if !read_both(name, own, legacy) {
        return false;
    }
    read_aliases(name, own, others);
    true
}

/// The names CSS matches `local()` against, into `full` and `postscript`:
/// the full font name and the PostScript name.
///
/// Both, because CSS Fonts 4 says either: "Segoe UI Semibold" is a full name
/// and "SegoeUI-Semibold" a PostScript name, and a page may use either.
/// `false` where the table has neither.
pub(crate) fn local_names(name: &Name<'_>, full: &mut String, postscript: &mut String) -> bool {
    let has_full = read_best(name, &[NameId::FULL_NAME], full);
    let has_postscript = read_best(name, &[NameId::POSTSCRIPT_NAME], postscript);
    has_full || has_postscript
}

/// The PostScript name in `name`, into `out`.
///
/// Which font of a collection a Core Text descriptor names: Core Text says
/// the file and the PostScript name, never the index, so the index is found
/// by reading this of each font in the file. And what Android's rule for an
/// updated font keys it by.
#[cfg(feature = "std")]
pub(crate) fn postscript_name(name: &Name<'_>, out: &mut String) -> bool {
    read_best(name, &[NameId::POSTSCRIPT_NAME], out)
}

/// Every other name the family has: the record [`read_both`] took its
/// answer from, in the other languages the font writes it in.
///
/// This is how Hiragino Sans is also found as ヒラギノ角ゴシック and Heiti SC
/// as Heiti-简体. Both other backends get these from the platform — a name
/// per locale from DirectWrite, a `familylang` per `family` from fontconfig
/// — and hold them as a family's aliases; a file read directly has to gather
/// them itself.
///
/// `own` is left out, as is any name already in `out`: a font writes one
/// name in several encodings, and those are one alias.
#[cfg(all(target_vendor = "apple", feature = "system"))]
fn read_aliases(name: &Name<'_>, own: &str, out: &mut alloc::vec::Vec<String>) {
    // Whichever record the name itself came from: a font with a typographic
    // family writes that one in every language it has, and one without
    // writes its family record.
    let id = match strings(name, NameId::TYPOGRAPHIC_FAMILY_NAME).next() {
        Some(_) => NameId::TYPOGRAPHIC_FAMILY_NAME,
        None => NameId::FAMILY_NAME,
    };
    for text in strings(name, id) {
        if !crate::names_match(&text, own)
            && !out.iter().any(|seen| crate::names_match(seen, &text))
        {
            out.push(text);
        }
    }
}

/// Every non-empty string `id` has in `name`, in the order the records are
/// written.
#[cfg(all(target_vendor = "apple", feature = "system"))]
fn strings<'a>(name: &'a Name<'a>, id: NameId) -> impl Iterator<Item = String> + 'a {
    let data = name.string_data();
    name.name_record()
        .iter()
        .filter(move |record| record.name_id() == id)
        .filter_map(move |record| {
            let text: String = record.string(data).ok()?.chars().collect();
            (!text.is_empty()).then_some(text)
        })
}

/// [`family_names`] into buffers the caller holds. `legacy` is left empty
/// where the family has no legacy name, which is how the caller tells.
fn read_both(name: &Name<'_>, own: &mut String, legacy: &mut String) -> bool {
    const FAMILY: [NameId; 2] = [NameId::TYPOGRAPHIC_FAMILY_NAME, NameId::FAMILY_NAME];
    if !read_best(name, &FAMILY, own) {
        return false;
    }
    if !read_best(name, &FAMILY[1..], legacy) || crate::names_match(legacy, own) {
        legacy.clear();
    }
    true
}

/// The best string `ids` has in `name`, into `out`, by the judgement
/// [`family_names`] documents: the first of `ids` that has one at all,
/// English before anything else, and any record before none.
///
/// Fills a buffer rather than returning a `String` because a scan reads one
/// of these per font and the same buffer serves them all. At most two
/// records are written even when many match: a non-English one is held only
/// until an English one turns up.
fn read_best(name: &Name<'_>, ids: &[NameId], out: &mut String) -> bool {
    let records = name.name_record();
    let data = name.string_data();

    for &id in ids {
        let mut held = false;
        for record in records {
            if record.name_id() != id {
                continue;
            }
            let Ok(string) = record.string(data) else {
                continue;
            };
            // Asked of the decoder rather than of a `String` it filled, so
            // an empty record costs nothing.
            if string.chars().next().is_none() {
                continue;
            }
            // Windows/Unicode BCP-47 aside, an English record is either
            // Macintosh language 0 or a Windows language whose low ten bits
            // are the primary language, 0x09 for English.
            let language = record.language_id();
            let english = language == 0 || language & 0x3FF == 0x09;
            if !english && held {
                continue;
            }
            out.clear();
            out.extend(string.chars());
            if english {
                return true;
            }
            held = true;
        }
        if held {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fonts::{EN_US, JA, MAC, WINDOWS, font_with_names};

    /// The family name of the font at `index` in `font`.
    fn family_name(font: &[u8], index: u32) -> Option<String> {
        name_table(font, index)
            .as_ref()
            .and_then(family_names)
            .map(|(own, _)| own)
    }

    #[test]
    fn a_family_name_is_read() {
        let font = font_with_names(&[(WINDOWS, EN_US, NameId::FAMILY_NAME, "Noto Naskh Arabic")]);
        assert_eq!(family_name(&font, 0).as_deref(), Some("Noto Naskh Arabic"));
    }

    #[test]
    fn the_typographic_family_wins() {
        // The case that matters: a font shipping many weights splits ID 1
        // into "Roboto Light" and records the real family in ID 16. Taking
        // ID 1 would turn one family into nine.
        let font = font_with_names(&[
            (WINDOWS, EN_US, NameId::FAMILY_NAME, "Roboto Light"),
            (WINDOWS, EN_US, NameId::TYPOGRAPHIC_FAMILY_NAME, "Roboto"),
        ]);
        assert_eq!(family_name(&font, 0).as_deref(), Some("Roboto"));
    }

    #[test]
    fn english_wins_over_another_language() {
        let font = font_with_names(&[
            (WINDOWS, JA, NameId::FAMILY_NAME, "ヒラギノ角ゴシック"),
            (WINDOWS, EN_US, NameId::FAMILY_NAME, "Hiragino Sans"),
        ]);
        assert_eq!(family_name(&font, 0).as_deref(), Some("Hiragino Sans"));
    }

    #[test]
    fn a_non_english_name_beats_no_name() {
        let font = font_with_names(&[(WINDOWS, JA, NameId::FAMILY_NAME, "ヒラギノ角ゴシック")]);
        assert_eq!(family_name(&font, 0).as_deref(), Some("ヒラギノ角ゴシック"));
    }

    #[test]
    fn a_macintosh_english_record_counts_as_english() {
        // Macintosh spells English as language 0, not 0x0409.
        let font = font_with_names(&[
            (WINDOWS, JA, NameId::FAMILY_NAME, "ヒラギノ角ゴシック"),
            (MAC, 0, NameId::FAMILY_NAME, "Hiragino Sans"),
        ]);
        assert_eq!(family_name(&font, 0).as_deref(), Some("Hiragino Sans"));
    }

    #[test]
    fn nothing_usable_is_none_rather_than_a_guess() {
        assert_eq!(family_name(b"not a font at all", 0), None);
        assert_eq!(family_name(&[], 0), None);
        // Well-formed, but names nothing we can use.
        let font = font_with_names(&[(WINDOWS, EN_US, NameId::FULL_NAME, "Roboto Regular")]);
        assert_eq!(family_name(&font, 0), None);
    }

    #[test]
    fn an_index_past_the_end_is_none() {
        let font = font_with_names(&[(WINDOWS, EN_US, NameId::FAMILY_NAME, "Roboto")]);
        assert_eq!(family_name(&font, 7), None);
    }
}
