//! Chrome's generic families on Windows, per setting and locale bucket.
//!
//! The values are `chrome/app/resources/locale_settings_win.grd` (copied to
//! `tests/fixtures/chrome/`), where `kFontDefaults` registers them.

use crate::fallback::{GenericBucket, GenericTable, Setting};

/// Chrome's generics on Windows.
pub(crate) const TABLE: GenericTable = GenericTable {
    defaults,
    // The non-client menu font (`FontCache::SystemFontFamily`,
    // `font_cache_skia_win.cc:130`), Segoe UI since Vista.
    system_ui: &["Segoe UI"],
};

/// Returns Chrome's default for `setting` in `bucket`, if `kFontDefaults`
/// registers one on Windows; `None` means the setting is Common's.
fn defaults(setting: Setting, bucket: GenericBucket) -> Option<&'static [&'static str]> {
    use GenericBucket::*;
    use Setting::*;
    Some(match (setting, bucket) {
        (Standard | Serif, Common) => &["Times New Roman"],
        // Courier New, swapped for Consolas when ClearType is on, as it is
        // by default (`ShouldUseAlternateDefaultFixedFont`,
        // `prefs_tab_helper.cc:124–135`, `:423–428`). Only Common's: P3
        // measured Courier New for Cyrillic and Greek.
        (Fixed, Common) => &["Consolas"],
        (SansSerif, Common) => &["Arial"],
        (Cursive, Common) => &["Comic Sans MS"],
        (Fantasy, Common) => &["Impact"],
        (Math, Common) => &["Cambria Math"],

        (Standard | SansSerif, Jpan) => {
            &["Noto Sans JP", "Noto Sans CJK JP", "Meiryo", "Yu Gothic"]
        }
        (Fixed, Jpan) => &["BIZ UDGothic", "MS Gothic"],
        (Serif, Jpan) => &[
            "Noto Serif JP",
            "Noto Serif CJK JP",
            "Yu Mincho",
            "MS PMincho",
        ],

        (Standard | SansSerif, Kore) => &["Noto Sans KR", "Noto Sans CJK KR", "Malgun Gothic"],
        (Fixed, Kore) => &["Gulimche"],
        (Serif, Kore) => &["Noto Serif KR", "Noto Serif CJK KR", "Batang"],
        (Cursive, Kore) => &["Gungsuh"],

        (Standard | SansSerif, Hans) => &["Noto Sans SC", "Noto Sans CJK SC", "Microsoft YaHei"],
        (Fixed, Hans) => &["NSimsun"],
        (Serif, Hans) => &["Noto Serif SC", "Noto Serif CJK SC", "Simsun"],
        (Cursive, Hans) => &["KaiTi"],

        (Standard | SansSerif, Hant) => &["Noto Sans TC", "Noto Sans CJK TC", "Microsoft JhengHei"],
        (Fixed, Hant) => &["MingLiU"],
        (Serif, Hant) => &["Noto Serif TC", "Noto Serif CJK TC", "PMingLiU"],
        (Cursive, Hant) => &["DFKai-SB"],

        (Standard | Serif | SansSerif, Deva) => &["Nirmala UI"],
        (Fixed, Deva) => &["Consolas"],

        (Fixed, Arab) => &["Courier New"],
        (SansSerif, Arab) => &["Segoe UI"],

        (Standard | Serif, Cyrl | Grek) => &["Times New Roman"],
        (Fixed, Cyrl | Grek) => &["Courier New"],
        (SansSerif, Cyrl | Grek) => &["Arial"],

        _ => return None,
    })
}
