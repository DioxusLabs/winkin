//! Tests of the Mac generics against Chrome's settings.

use alloc::vec::Vec;

use parlance::GenericFamily;

use crate::fallback::{
    BUCKETS, GENERICS, GenericBucket, SETTINGS, Setting, grd_families, message_id,
    registered_on_desktop, registered_on_mac_and_windows, setting,
};
use crate::platform::apple::generics::TABLE;

const GRD: &str = include_str!("../../../../tests/fixtures/chrome/locale_settings_mac.grd");

/// Returns whether `kFontDefaults` registers a default for `setting` in
/// `bucket` on the Mac (`prefs_tab_helper.cc:147–222`).
fn registered(setting: Setting, bucket: GenericBucket) -> bool {
    registered_on_desktop(setting, bucket) || registered_on_mac_and_windows(setting, bucket)
}

/// Returns Chrome's default for a setting in a bucket, read from the `.grd`:
/// the bucket's own where `kFontDefaults` registers it, else Common's.
fn chrome_families(setting: Setting, bucket: GenericBucket) -> Vec<&'static str> {
    let bucket = if registered(setting, bucket) {
        bucket
    } else {
        GenericBucket::Common
    };
    grd_families(GRD, &message_id(setting, bucket))
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
    // The seven Common defaults, and 19 per-script ones.
    assert_eq!(checked, 7 + 19);
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
