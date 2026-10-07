//! Tests of the Windows generics against Chrome's settings, and against
//! what Chrome drew.

use alloc::vec::Vec;

use parlance::GenericFamily;

use crate::fallback::{
    BUCKETS, COLUMNS, GENERICS, GenericBucket, SETTINGS, Setting, bucket, drawn, grd_families,
    message_id, registered_on_desktop, registered_on_mac_and_windows, setting,
};
use crate::platform::windows::generics::TABLE;

const GRD: &str = include_str!("../../../../tests/fixtures/chrome/locale_settings_win.grd");

/// Returns whether `kFontDefaults` registers a default for `setting` in
/// `bucket` on Windows (`prefs_tab_helper.cc:147–222`).
fn registered(setting: Setting, bucket: GenericBucket) -> bool {
    use GenericBucket::*;
    use Setting::*;
    registered_on_desktop(setting, bucket)
        || registered_on_mac_and_windows(setting, bucket)
        || matches!(
            (setting, bucket),
            (Fixed | SansSerif, Arab)
                | (Standard | Fixed | Serif | SansSerif, Cyrl | Grek)
                | (Fixed | Cursive, Kore)
                | (Fixed, Hans | Hant)
        )
}

/// Returns Chrome's default for a setting in a bucket, read from the `.grd`:
/// the bucket's own where `kFontDefaults` registers it, else Common's.
fn chrome_families(setting: Setting, bucket: GenericBucket) -> Vec<&'static str> {
    let bucket = if registered(setting, bucket) {
        bucket
    } else {
        GenericBucket::Common
    };
    // ClearType swaps Common's fixed font (`prefs_tab_helper.cc:423–428`).
    let id = if (setting, bucket) == (Setting::Fixed, GenericBucket::Common) {
        "IDS_FIXED_FONT_FAMILY_ALT_WIN".into()
    } else {
        message_id(setting, bucket)
    };
    grd_families(GRD, &id)
}

#[test]
fn every_registered_default_is_the_grd_value() {
    let mut checked = 0;
    for setting in SETTINGS {
        for bucket in BUCKETS {
            let ours = (TABLE.defaults)(setting, bucket);
            if !registered(setting, bucket) {
                assert_eq!(ours, None, "{setting:?} {bucket:?} has no default");
                continue;
            }
            let expected = chrome_families(setting, bucket);
            assert_eq!(
                ours.map(<[_]>::to_vec),
                Some(expected),
                "{setting:?} {bucket:?}"
            );
            checked += 1;
        }
    }
    // The seven Common defaults, and 33 per-script ones.
    assert_eq!(checked, 7 + 33);
}

#[test]
fn every_cell_is_chromes() {
    for family in GENERICS {
        for bucket in BUCKETS {
            let expected = match setting(family) {
                Some(setting) => chrome_families(setting, bucket),
                None => match family {
                    GenericFamily::SystemUi => TABLE.system_ui.to_vec(),
                    _ => Vec::new(),
                },
            };
            assert_eq!(
                TABLE.families(family, bucket),
                expected.as_slice(),
                "{family:?} {bucket:?}"
            );
        }
    }
    for bucket in BUCKETS {
        let expected = chrome_families(Setting::Standard, bucket);
        assert_eq!(
            TABLE.standard(bucket),
            expected.as_slice(),
            "Standard {bucket:?}"
        );
    }
}

/// Returns the fonts the settings name that the Windows machine Chrome was
/// measured on has (Windows 11 Home 10.0.26200, en-US UI), by Chrome's
/// case-insensitive match.
fn installed(name: &str) -> Option<&'static str> {
    const INSTALLED: [&str; 16] = [
        "Times New Roman",
        "Arial",
        "Consolas",
        "Courier New",
        "Comic Sans MS",
        "Impact",
        "Cambria Math",
        "Segoe UI",
        "Yu Gothic",
        "MS Gothic",
        "Microsoft YaHei",
        "SimSun",
        "NSimSun",
        "Microsoft JhengHei",
        "Malgun Gothic",
        "Nirmala UI",
    ];
    INSTALLED
        .into_iter()
        .find(|family| family.eq_ignore_ascii_case(name))
}

/// What Chrome drew `Ag` with on Windows, under each language
/// and `font-family`, Chrome 153 and 154 alike.
#[test]
fn generics_draw_what_chrome_drew() {
    const TNR: &str = "Times New Roman";
    const YAHEI: &str = "Microsoft YaHei";
    const JHENGHEI: &str = "Microsoft JhengHei";
    const MALGUN: &str = "Malgun Gothic";
    const REST: [&str; 3] = ["Impact", "Cambria Math", "Segoe UI"];
    let latin = |mono| [TNR, "Arial", TNR, mono, "Comic Sans MS"];
    let arabic = [TNR, "Segoe UI", TNR, "Courier New", "Comic Sans MS"];
    let hans = [YAHEI, YAHEI, "SimSun", "NSimSun", YAHEI];
    let rows: [(&str, [&str; 5]); 15] = [
        ("", latin("Consolas")),
        ("en", latin("Consolas")),
        (
            "ja",
            [
                "Yu Gothic",
                "Yu Gothic",
                "Yu Gothic",
                "MS Gothic",
                "Comic Sans MS",
            ],
        ),
        ("zh-Hans", hans),
        ("zh-Hant", [JHENGHEI; 5]),
        ("zh-CN", hans),
        ("zh-TW", [JHENGHEI; 5]),
        ("zh-HK", [JHENGHEI; 5]),
        ("ko", [MALGUN; 5]),
        (
            "hi",
            [
                "Nirmala UI",
                "Nirmala UI",
                "Nirmala UI",
                "Consolas",
                "Comic Sans MS",
            ],
        ),
        ("ar", arabic),
        ("fa", arabic),
        // Cyrillic and Greek monospace is
        // Courier New, since ClearType's Consolas is only Common's.
        ("ru", latin("Courier New")),
        ("el", latin("Courier New")),
        ("th", latin("Consolas")),
    ];
    for (tag, first) in rows {
        let bucket = bucket(tag);
        for (at, column) in COLUMNS.into_iter().enumerate() {
            let expected = first
                .get(at)
                .or_else(|| REST.get(at - first.len()))
                .copied();
            let ours = drawn(&TABLE, column, bucket, installed);
            assert_eq!(Some(ours), expected, "lang {tag:?}, {column:?}");
        }
    }
}
