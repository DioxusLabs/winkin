//! The languages fontconfig knows the Han traditions by.

use crate::fallback::Han;

/// The language tag fontconfig recognizes for each Han tradition.
///
/// fontconfig distinguishes these by region, not by script subtag: its
/// orthographies are `zh-cn`, `zh-tw` and `zh-hk`, and `zh-Hans`/`zh-Hant`
/// are not among them.
pub(super) const fn han_lang(han: Han) -> &'static str {
    match han {
        Han::Hans => "zh-cn",
        Han::Hant => "zh-tw",
        Han::HantHK => "zh-hk",
        Han::Jpan => "ja",
        Han::Kore => "ko",
    }
}
