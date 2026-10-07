//! Each script's own macOS font, asked of Core Text with
//! `CTFontCreateForString`.
//!
//! The Latin default (Helvetica, Times) draws Latin, Cyrillic, Greek and
//! Georgian, but not Arabic or Thai. Arabic text should land on Geeza Pro,
//! not on Times.
//!
//! A table of family names per script would go stale: Apple ships and
//! renames these fonts each release. So [`primary`] asks Core Text, as text
//! layout does: it passes one letter of the script, and Core Text returns
//! the installed font it would draw that letter with. Each
//! [`DedicatedScript`] is checked against a live cascade.
//!
//! The base font is Helvetica Neue, a real font with a public name, so the
//! answer is always a name `CTFontCreateWithName` resolves back to that font.

use alloc::string::{String, ToString};

use parlance::Script;

use objc2_core_foundation::{CFRange, CFString};
use objc2_core_text::CTFont;

use crate::script as sc;

/// Scripts with a real, dedicated macOS font — beyond what the Latin-default
/// font already draws directly.
///
/// Latin, Cyrillic, Greek and Georgian are absent on purpose: asked for a
/// substitute for those, Core Text returns the default font itself.
#[derive(Copy, Clone, Debug)]
enum DedicatedScript {
    Arab,
    Armn,
    Beng,
    Cans,
    Cher,
    Deva,
    Ethi,
    Gujr,
    Guru,
    Hebr,
    Khmr,
    Knda,
    Laoo,
    Mlym,
    Mong,
    Mymr,
    Orya,
    Sinh,
    Taml,
    Telu,
    Thai,
    Tibt,
    Yiii,
}

impl DedicatedScript {
    pub(super) fn from_script(script: Script) -> Option<DedicatedScript> {
        Some(match script {
            sc::ARAB => DedicatedScript::Arab,
            sc::ARMN => DedicatedScript::Armn,
            sc::BENG => DedicatedScript::Beng,
            sc::CANS => DedicatedScript::Cans,
            sc::CHER => DedicatedScript::Cher,
            sc::DEVA => DedicatedScript::Deva,
            sc::ETHI => DedicatedScript::Ethi,
            sc::GUJR => DedicatedScript::Gujr,
            sc::GURU => DedicatedScript::Guru,
            sc::HEBR => DedicatedScript::Hebr,
            sc::KHMR => DedicatedScript::Khmr,
            sc::KNDA => DedicatedScript::Knda,
            sc::LAOO => DedicatedScript::Laoo,
            sc::MLYM => DedicatedScript::Mlym,
            sc::MONG => DedicatedScript::Mong,
            sc::MYMR => DedicatedScript::Mymr,
            sc::ORYA => DedicatedScript::Orya,
            sc::SINH => DedicatedScript::Sinh,
            sc::TAML => DedicatedScript::Taml,
            sc::TELU => DedicatedScript::Telu,
            sc::THAI => DedicatedScript::Thai,
            sc::TIBT => DedicatedScript::Tibt,
            sc::YIII => DedicatedScript::Yiii,
            _ => return None,
        })
    }

    /// Returns one letter of the script for Core Text to search cmaps by.
    ///
    /// Any letter works: one gives the same answer as longer sample text.
    fn sample(self) -> char {
        match self {
            DedicatedScript::Arab => 'ا',
            DedicatedScript::Armn => 'հ',
            DedicatedScript::Beng => 'ব',
            DedicatedScript::Cans => 'ᐃ',
            DedicatedScript::Cher => 'Ꮳ',
            DedicatedScript::Deva => 'ह',
            DedicatedScript::Ethi => 'አ',
            DedicatedScript::Gujr => 'ગ',
            DedicatedScript::Guru => 'ਪ',
            DedicatedScript::Hebr => 'א',
            DedicatedScript::Khmr => 'ខ',
            DedicatedScript::Knda => 'ಕ',
            DedicatedScript::Laoo => 'ລ',
            DedicatedScript::Mlym => 'മ',
            DedicatedScript::Mong => 'ᠮ',
            DedicatedScript::Mymr => 'မ',
            DedicatedScript::Orya => 'ଓ',
            DedicatedScript::Sinh => 'ස',
            DedicatedScript::Taml => 'த',
            DedicatedScript::Telu => 'త',
            DedicatedScript::Thai => 'ก',
            DedicatedScript::Tibt => 'བ',
            DedicatedScript::Yiii => 'ꆈ',
        }
    }
}

/// The font Core Text substitutes from Helvetica Neue for a letter of
/// `script`, where the script is one with a dedicated macOS font: `None`
/// for anything outside [`DedicatedScript`].
///
/// Asks Core Text each time; a collection asks once per key.
pub(super) fn primary(script: Script) -> Option<String> {
    let bucket = DedicatedScript::from_script(script)?;
    unsafe {
        let font = CTFont::with_name(
            &CFString::from_str("Helvetica Neue"),
            12.0,
            core::ptr::null(),
        );
        let mut buf = [0u8; 4];
        let sample = CFString::from_str(bucket.sample().encode_utf8(&mut buf));
        let range = CFRange {
            location: 0,
            length: sample.length(),
        };
        let substitute = font.for_string(&sample, range);
        Some(substitute.family_name().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_with_a_font_of_its_own_has_a_primary() {
        for script in [sc::ARAB, sc::DEVA, sc::THAI] {
            assert!(primary(script).is_some(), "{script:?}");
        }
    }

    #[test]
    fn scripts_the_default_font_already_draws_have_no_bucket() {
        for script in [sc::LATN, sc::CYRL, sc::GREK] {
            assert!(DedicatedScript::from_script(script).is_none());
            assert_eq!(primary(script), None);
        }
    }
}
