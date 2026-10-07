//! Tests of the Linux generics against Chrome's settings, and against what
//! Chrome drew.

use alloc::vec::Vec;

use parlance::GenericFamily;

use crate::fallback::{
    BUCKETS, COLUMNS, GENERICS, GenericBucket, SETTINGS, Setting, bucket, drawn, grd_families,
    message_id, registered_on_desktop, setting,
};
use crate::platform::fontconfig::generics::TABLE;

const GRD: &str = include_str!("../../../../tests/fixtures/chrome/locale_settings_linux.grd");

/// Returns the keyword Chrome asks fontconfig for when a setting's family is
/// not installed; the Standard step asks for none.
fn keyword(setting: Setting) -> Option<&'static str> {
    match setting {
        Setting::Standard => None,
        Setting::Fixed => Some("monospace"),
        Setting::Serif => Some("serif"),
        Setting::SansSerif => Some("sans-serif"),
        Setting::Cursive => Some("cursive"),
        Setting::Fantasy => Some("fantasy"),
        Setting::Math => Some("math"),
    }
}

/// Returns Chrome's default for a setting in a bucket, read from the `.grd`
/// (the bucket's own where `kFontDefaults` registers it, else Common's),
/// with the keyword after it.
fn chrome_families(setting: Setting, bucket: GenericBucket) -> Vec<&'static str> {
    let bucket = if registered_on_desktop(setting, bucket) {
        bucket
    } else {
        GenericBucket::Common
    };
    let mut families = grd_families(GRD, &message_id(setting, bucket));
    families.extend(keyword(setting));
    families
}

#[test]
fn every_registered_default_is_the_grd_value() {
    let mut checked = 0;
    for setting in SETTINGS {
        for bucket in BUCKETS {
            let ours = (TABLE.defaults)(setting, bucket);
            if !registered_on_desktop(setting, bucket) {
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
    // The seven Common defaults, and 17 per-script ones.
    assert_eq!(checked, 7 + 17);
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

/// Returns the installed fonts the settings name on the Linux machine Chrome
/// was measured on (WSL Fedora 44), and the fontconfig aliases that bound
/// something there. `cursive`, `fantasy` and `math` bound nothing installed,
/// so Chrome drew the Standard font.
fn installed(name: &str) -> Option<&'static str> {
    const INSTALLED: [&str; 19] = [
        "Noto Sans",
        "Noto Sans Mono",
        "Noto Sans JP",
        "Noto Sans CJK JP",
        "Noto Serif CJK JP",
        "Noto Sans Mono CJK JP",
        "Noto Sans SC",
        "Noto Sans CJK SC",
        "Noto Serif CJK SC",
        "Noto Sans TC",
        "Noto Sans CJK TC",
        "Noto Serif CJK TC",
        "Noto Sans KR",
        "Noto Sans CJK KR",
        "Noto Serif CJK KR",
        "Noto Sans Devanagari",
        "Noto Serif Devanagari",
        "Liberation Serif",
        "Liberation Sans",
    ];
    const ALIASES: [(&str, &str); 4] = [
        ("Times New Roman", "Liberation Serif"),
        ("Arial", "Liberation Sans"),
        ("Monospace", "Noto Sans Mono"),
        ("sans", "Noto Sans"),
    ];
    ALIASES
        .into_iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .map(|(_, family)| family)
        .or_else(|| {
            INSTALLED
                .into_iter()
                .find(|family| family.eq_ignore_ascii_case(name))
        })
}

/// What Chrome drew `Ag` with on Linux, under each language and
/// `font-family`.
///
/// Under `hi` the Devanagari settings have no Latin, so Chrome drew `Ag` in
/// system fallback's Noto Sans. This checks the families resolved, which
/// are the primary fonts Chrome reports.
#[test]
fn generics_draw_what_chrome_drew() {
    const SERIF: &str = "Liberation Serif";
    const MONO: &str = "Noto Sans Mono";
    let latin = [SERIF, "Liberation Sans", SERIF, MONO, SERIF, SERIF, SERIF];
    let cjk = |sans, serif, mono| [sans, sans, serif, mono, sans, sans, sans];
    let hans = cjk("Noto Sans SC", "Noto Serif CJK SC", MONO);
    let hant = cjk("Noto Sans TC", "Noto Serif CJK TC", MONO);
    let deva = "Noto Sans Devanagari";
    let rows: [(&str, [&str; 7]); 15] = [
        ("", latin),
        ("en", latin),
        (
            "ja",
            cjk("Noto Sans JP", "Noto Serif CJK JP", "Noto Sans Mono CJK JP"),
        ),
        ("zh-Hans", hans),
        ("zh-Hant", hant),
        ("zh-CN", hans),
        ("zh-TW", hant),
        // Hong Kong resolves as Traditional.
        ("zh-HK", hant),
        // No fixed setting for Korean or Chinese on Linux: Common's.
        ("ko", cjk("Noto Sans KR", "Noto Serif CJK KR", MONO)),
        (
            "hi",
            [deva, deva, "Noto Serif Devanagari", MONO, deva, deva, deva],
        ),
        ("ar", latin),
        ("fa", latin),
        ("ru", latin),
        ("el", latin),
        ("th", latin),
    ];
    for (tag, first) in rows {
        let bucket = bucket(tag);
        for (at, column) in COLUMNS.into_iter().enumerate() {
            let expected = first.get(at).copied().unwrap_or("Noto Sans");
            let ours = drawn(&TABLE, column, bucket, installed);
            assert_eq!(ours, expected, "lang {tag:?}, {column:?}");
        }
    }
}
