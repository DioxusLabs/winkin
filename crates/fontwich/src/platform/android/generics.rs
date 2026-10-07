//! Chrome's generic families on Android, and its rule for CJK pages.
//!
//! Android reads no settings. A generic is its `fonts.xml` name, except
//! that a Chinese, Japanese or Korean page gets its CJK font
//! ([`cjk_generic`]). The Standard font names nothing outside a CJK page
//! ([`standard_cjk`]); a page whose list finds nothing gets Skia's default
//! typeface as Chrome's last resort instead (`font_cache_skia.cc:149–153`).

use parlance::GenericFamily;

use crate::fallback::{GenericBucket, GenericTable, Setting, normalize};

/// Chrome's generics on Android.
pub(crate) const TABLE: GenericTable = GenericTable {
    defaults,
    // Skia's default typeface, `fonts.xml`'s `sans-serif`
    // (`DefaultFontFamily`, `font_cache_android.cc:54–72`).
    system_ui: &["sans-serif"],
};

/// Returns Android's name for a setting in any bucket: the generic's own,
/// which Skia matches against `fonts.xml`'s families and aliases
/// (`font_selector.cc:51–70`).
fn defaults(setting: Setting, _: GenericBucket) -> Option<&'static [&'static str]> {
    Some(match setting {
        Setting::Serif => &["serif"],
        Setting::SansSerif => &["sans-serif"],
        Setting::Fixed => &["monospace"],
        Setting::Cursive => &["cursive"],
        Setting::Fantasy => &["fantasy"],
        // No setting answers `math`, so Chrome asks Skia for the keyword.
        Setting::Math => &["math"],
        // Nothing outside the CJK rule.
        Setting::Standard => &[],
    })
}

/// How Chrome on Android answers a generic on a CJK page.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum CjkGeneric {
    /// The first family that maps this character in the page language's
    /// `fonts.xml` order: U+4E00 for Chinese and Japanese, U+AC00 for
    /// Korean, whatever the generic (`font_cache_android.cc:229–282`). It
    /// is the bucket's `Han` key's first family that maps it.
    Character(char),
    /// The `serif` family's fallback for the page language, if one is
    /// declared (`fallbackFor="serif"` with that `lang`), else `serif`
    /// (`FontCache::CreateLocaleSpecificTypeface`, `font_cache_android.cc:83–122`).
    Serif,
}

/// Returns what replaces [`TABLE`]'s answer for `family` in `bucket`, if
/// anything.
///
/// The CJK rule covers `sans-serif`, `cursive` and `fantasy` (and `ui-sans-serif`
/// and `ui-rounded` with them), and the Standard font ([`standard_cjk`]).
/// `monospace` is exempt ("i18n fonts are likely not monospace"), `serif`
/// takes its locale's serif fallback, and `math` and `system-ui` never reach
/// it.
///
/// Chrome applies the character rule only when the page has a `lang`
/// (`font_description.Locale()`), and the serif rule under the default
/// locale too; a bucket cannot tell them apart.
pub(super) fn cjk_generic(family: GenericFamily, bucket: GenericBucket) -> Option<CjkGeneric> {
    use GenericFamily::*;
    let sample = cjk_sample(bucket)?;
    match normalize(family) {
        SansSerif | Cursive | Fantasy => Some(CjkGeneric::Character(sample)),
        Serif => Some(CjkGeneric::Serif),
        _ => None,
    }
}

/// Returns the character whose first mapping family is the Standard font in
/// `bucket`, on a CJK page.
pub(super) fn standard_cjk(bucket: GenericBucket) -> Option<char> {
    cjk_sample(bucket)
}

/// Returns the character the CJK rule asks for in `bucket`.
fn cjk_sample(bucket: GenericBucket) -> Option<char> {
    match bucket {
        GenericBucket::Hans | GenericBucket::Hant | GenericBucket::Jpan => Some('\u{4E00}'),
        GenericBucket::Kore => Some('\u{AC00}'),
        _ => None,
    }
}
