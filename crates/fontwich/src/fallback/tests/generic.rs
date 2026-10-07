//! Tests of the generic families, and the reader of Chrome's settings that
//! each platform's generic tests share.

use alloc::string::String;
use alloc::vec::Vec;

use parlance::GenericFamily;

use crate::fallback::generic::{GenericTable, Setting};
use crate::fallback::key::{
    BackendFacts, FallbackKey, FallbackRequest, GenericBucket, parse_language,
};

/// Every generic family.
pub(crate) const GENERICS: [GenericFamily; 13] = [
    GenericFamily::Serif,
    GenericFamily::SansSerif,
    GenericFamily::Monospace,
    GenericFamily::Cursive,
    GenericFamily::Fantasy,
    GenericFamily::SystemUi,
    GenericFamily::UiSerif,
    GenericFamily::UiSansSerif,
    GenericFamily::UiMonospace,
    GenericFamily::UiRounded,
    GenericFamily::Emoji,
    GenericFamily::Math,
    GenericFamily::FangSong,
];

/// Every locale bucket.
pub(crate) const BUCKETS: [GenericBucket; 9] = [
    GenericBucket::Common,
    GenericBucket::Hans,
    GenericBucket::Hant,
    GenericBucket::Jpan,
    GenericBucket::Kore,
    GenericBucket::Deva,
    GenericBucket::Arab,
    GenericBucket::Cyrl,
    GenericBucket::Grek,
];

/// Every font setting.
pub(crate) const SETTINGS: [Setting; 7] = [
    Setting::Standard,
    Setting::Fixed,
    Setting::Serif,
    Setting::SansSerif,
    Setting::Cursive,
    Setting::Fantasy,
    Setting::Math,
];

/// Returns whether `kFontDefaults` registers a default for `setting` in
/// `bucket` on every desktop platform (`prefs_tab_helper.cc:147–222`).
pub(crate) fn registered_on_desktop(setting: Setting, bucket: GenericBucket) -> bool {
    use GenericBucket::*;
    use Setting::*;
    matches!(
        (setting, bucket),
        (_, Common)
            | (Standard | Fixed | Serif | SansSerif, Jpan)
            | (Standard | Serif | SansSerif, Kore | Hans | Hant)
            | (Standard | Fixed | Serif | SansSerif, Deva)
    )
}

/// Returns whether `kFontDefaults` registers a default for `setting` in
/// `bucket` on the Mac and Windows, beyond the desktop ones.
pub(crate) fn registered_on_mac_and_windows(setting: Setting, bucket: GenericBucket) -> bool {
    matches!(
        (setting, bucket),
        (Setting::Cursive, GenericBucket::Hans | GenericBucket::Hant)
    )
}

/// Returns the `.grd` message's text for `id`, trimmed.
fn message<'a>(grd: &'a str, id: &str) -> Option<&'a str> {
    let open = alloc::format!("<message name=\"{id}\"");
    let at = grd.find(&open)?;
    let body = &grd[at..];
    let start = body.find('>')? + 1;
    let end = body.find("</message>")?;
    Some(body[start..end].trim())
}

/// Returns the message id of a setting's default in a bucket.
pub(crate) fn message_id(setting: Setting, bucket: GenericBucket) -> String {
    let setting = match setting {
        Setting::Standard => "STANDARD",
        Setting::Fixed => "FIXED",
        Setting::Serif => "SERIF",
        Setting::SansSerif => "SANS_SERIF",
        Setting::Cursive => "CURSIVE",
        Setting::Fantasy => "FANTASY",
        Setting::Math => "MATH",
    };
    let script = match bucket {
        GenericBucket::Common => "",
        GenericBucket::Hans => "_SIMPLIFIED_HAN",
        GenericBucket::Hant => "_TRADITIONAL_HAN",
        GenericBucket::Jpan => "_JAPANESE",
        GenericBucket::Kore => "_KOREAN",
        GenericBucket::Deva => "_DEVANAGARI",
        GenericBucket::Arab => "_ARABIC",
        GenericBucket::Cyrl => "_CYRILLIC",
        GenericBucket::Grek => "_GREEK",
    };
    alloc::format!("IDS_{setting}_FONT_FAMILY{script}")
}

/// Returns the families the `.grd` message `id` names: a list after a
/// leading comma, else one family.
pub(crate) fn grd_families(grd: &'static str, id: &str) -> Vec<&'static str> {
    let value = message(grd, id);
    assert!(value.is_some(), "no {id} in the .grd");
    let value = value.unwrap_or_default();
    match value.strip_prefix(',') {
        Some(list) => list.split(',').collect(),
        None => alloc::vec![value],
    }
}

/// Returns the bucket a page language resolves a generic in, through the key.
pub(crate) fn bucket(tag: &str) -> GenericBucket {
    let language = if tag.is_empty() {
        None
    } else {
        parse_language(tag)
    };
    let request = FallbackRequest::Generic(GenericFamily::Serif, language);
    let key = FallbackKey::new(&request, BackendFacts::default());
    key.generic()
        .map_or(GenericBucket::Common, |(_, bucket)| bucket)
}

/// The measured columns: the Standard font, then seven generics.
pub(crate) const COLUMNS: [Option<GenericFamily>; 8] = [
    None,
    Some(GenericFamily::SansSerif),
    Some(GenericFamily::Serif),
    Some(GenericFamily::Monospace),
    Some(GenericFamily::Cursive),
    Some(GenericFamily::Fantasy),
    Some(GenericFamily::Math),
    Some(GenericFamily::SystemUi),
];

/// Returns the family Chrome draws `Ag` in: the generic's first installed
/// family, else the Standard font's, as `installed` names them.
pub(crate) fn drawn(
    table: &GenericTable,
    column: Option<GenericFamily>,
    bucket: GenericBucket,
    installed: impl Fn(&str) -> Option<&'static str>,
) -> &'static str {
    let generic: &[&str] = match column {
        Some(family) => table.families(family, bucket),
        None => &[],
    };
    generic
        .iter()
        .chain(table.standard(bucket))
        .find_map(|name| installed(name))
        .unwrap_or("(none)")
}

/// A table registering each setting for Common and Japanese only.
const PROBE: GenericTable = GenericTable {
    defaults: |setting, bucket| {
        let name: &'static [&'static str] = match setting {
            Setting::Standard => &["standard"],
            Setting::Fixed => &["fixed"],
            Setting::Serif => &["serif"],
            Setting::SansSerif => &["sans-serif"],
            Setting::Cursive => &["cursive"],
            Setting::Fantasy => &["fantasy"],
            Setting::Math => &["math"],
        };
        match bucket {
            GenericBucket::Common => Some(name),
            GenericBucket::Jpan => Some(&["jpan"]),
            _ => None,
        }
    },
    system_ui: &["system-ui"],
};

#[test]
fn ui_generics_fold_and_the_rest_name_nothing() {
    use GenericFamily::*;
    for bucket in BUCKETS {
        let of = |family| PROBE.families(family, bucket);
        assert_eq!(of(UiSerif), of(Serif));
        assert_eq!(of(UiSansSerif), of(SansSerif));
        assert_eq!(of(UiRounded), of(SansSerif));
        assert_eq!(of(UiMonospace), of(Monospace));
        assert!(of(Emoji).is_empty());
        assert!(of(FangSong).is_empty());
        assert_eq!(of(SystemUi), &["system-ui"]);
    }
}
