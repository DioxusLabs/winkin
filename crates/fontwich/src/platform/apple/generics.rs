//! Chrome's generic families on the Mac, per setting and locale bucket.
//!
//! The values are `chrome/app/resources/locale_settings_mac.grd` (copied to
//! `tests/fixtures/chrome/`), where `kFontDefaults` registers them.

use crate::fallback::{GenericBucket, GenericTable, Setting};

/// Chrome's generics on the Mac.
pub(crate) const TABLE: GenericTable = GenericTable {
    defaults,
    // `CTFontCreateUIFontForLanguage(kCTFontUIFontSystem)`
    // (`font_matcher_mac.mm:549–556`).
    system_ui: &[".AppleSystemUIFont"],
};

/// Returns Chrome's default for `setting` in `bucket`, if `kFontDefaults`
/// registers one on the Mac; `None` means the setting is Common's.
fn defaults(setting: Setting, bucket: GenericBucket) -> Option<&'static [&'static str]> {
    use GenericBucket::*;
    use Setting::*;
    Some(match (setting, bucket) {
        (Standard | Serif, Common) => &["Times"],
        (Fixed, Common) => &["Menlo"],
        (SansSerif, Common) => &["Helvetica"],
        (Cursive, Common) => &["Apple Chancery"],
        (Fantasy, Common) => &["Papyrus"],
        (Math, Common) => &["STIX Two Math"],

        (Standard | SansSerif, Jpan) => &["Hiragino Kaku Gothic ProN"],
        (Fixed, Jpan) => &["Osaka", "BIZ UDGothic", "Menlo"],
        (Serif, Jpan) => &["Hiragino Mincho ProN"],

        (Standard | SansSerif, Kore) => &["Apple SD Gothic Neo"],
        (Serif, Kore) => &["AppleMyungjo"],

        (Standard | SansSerif, Hans) => &["PingFang SC", "STHeiti"],
        (Serif, Hans) => &["Songti SC"],
        (Cursive, Hans) => &["Kaiti SC"],

        (Standard | SansSerif, Hant) => &["PingFang TC", "Heiti TC"],
        (Serif, Hant) => &["Songti TC"],
        (Cursive, Hant) => &["Kaiti TC"],

        (Standard | Serif, Deva) => &["Devanagari MT"],
        (Fixed, Deva) => &["Menlo"],
        (SansSerif, Deva) => &["Kohinoor Devanagari"],

        _ => return None,
    })
}
