//! Chrome's generic families on Linux, per setting and locale bucket.
//!
//! The values are `chrome/app/resources/locale_settings_linux.grd` (copied
//! to `tests/fixtures/chrome/`), where `kFontDefaults` registers them.
//!
//! When the setting's family is not installed, Chrome asks the platform for
//! the generic's own keyword as a family name (`font_fallback_list.cc:165–173`,
//! with the keyword from `FontBuilder::GenericFontFamilyName`). Only
//! fontconfig knows the keywords, as aliases, so here the keyword ends each
//! generic's list.
//!
//! `Times New Roman`, `Arial`, `Monospace`, `sans` and the generic keywords
//! are fontconfig names: an alias resolves to the first installed family its
//! configuration binds to it, and to nothing when it binds none, never to
//! fontconfig's best-effort match.

use crate::fallback::{GenericBucket, GenericTable, Setting};

/// Chrome's generics on Linux.
pub(crate) const TABLE: GenericTable = GenericTable {
    defaults,
    // The toolkit's UI font where there is one, else `sans`
    // (`platform_font_skia.cc:43–45`, `:155`).
    system_ui: &["sans"],
};

/// Returns Chrome's default for `setting` in `bucket`, if `kFontDefaults`
/// registers one on Linux; `None` means the setting is Common's.
///
/// Each generic's list ends with its keyword: Chrome asks fontconfig for it
/// when the setting's family is not installed (P4 measured it: a Japanese
/// UI's uninstalled `VL Gothic` gave Noto Sans Mono). The Standard step asks
/// the settings alone, so Standard has no keyword.
fn defaults(setting: Setting, bucket: GenericBucket) -> Option<&'static [&'static str]> {
    use GenericBucket::*;
    use Setting::*;
    Some(match (setting, bucket) {
        (Standard, Common) => &["Times New Roman"],
        (Serif, Common) => &["Times New Roman", "serif"],
        (Fixed, Common) => &["Monospace", "monospace"],
        (SansSerif, Common) => &["Arial", "sans-serif"],
        (Cursive, Common) => &["Comic Sans MS", "cursive"],
        (Fantasy, Common) => &["Impact", "fantasy"],
        (Math, Common) => &["Latin Modern Math", "math"],

        (Standard, Jpan) => &["Noto Sans JP", "Noto Sans CJK JP", "Times New Roman"],
        (Fixed, Jpan) => &["Noto Sans Mono CJK JP", "monospace"],
        (Serif, Jpan) => &[
            "Noto Serif JP",
            "Noto Serif CJK JP",
            "Times New Roman",
            "serif",
        ],
        (SansSerif, Jpan) => &["Noto Sans JP", "Noto Sans CJK JP", "Arial", "sans-serif"],

        (Standard, Kore) => &["Noto Sans KR", "Noto Sans CJK KR", "Times New Roman"],
        (Serif, Kore) => &[
            "Noto Serif KR",
            "Noto Serif CJK KR",
            "Times New Roman",
            "serif",
        ],
        (SansSerif, Kore) => &["Noto Sans KR", "Noto Sans CJK KR", "Arial", "sans-serif"],

        (Standard, Hans) => &["Noto Sans SC", "Noto Sans CJK SC", "Times New Roman"],
        (Serif, Hans) => &[
            "Noto Serif SC",
            "Noto Serif CJK SC",
            "Times New Roman",
            "serif",
        ],
        (SansSerif, Hans) => &["Noto Sans SC", "Noto Sans CJK SC", "Arial", "sans-serif"],

        (Standard, Hant) => &["Noto Sans TC", "Noto Sans CJK TC", "Times New Roman"],
        (Serif, Hant) => &[
            "Noto Serif TC",
            "Noto Serif CJK TC",
            "Times New Roman",
            "serif",
        ],
        (SansSerif, Hant) => &["Noto Sans TC", "Noto Sans CJK TC", "Arial", "sans-serif"],

        (Standard, Deva) => &["Noto Sans Devanagari"],
        (Fixed, Deva) => &["Noto Sans Mono", "monospace"],
        (Serif, Deva) => &["Noto Serif Devanagari", "serif"],
        (SansSerif, Deva) => &["Noto Sans Devanagari", "sans-serif"],

        _ => return None,
    })
}
