//! Tests of the Android generics and its rule for CJK pages.

use crate::fallback::{BUCKETS, GENERICS, GenericBucket};
use crate::platform::android::generics::{CjkGeneric, TABLE, cjk_generic, standard_cjk};

#[test]
fn every_android_cell() {
    use parlance::GenericFamily::*;
    for bucket in BUCKETS {
        let cjk = match bucket {
            GenericBucket::Hans | GenericBucket::Hant | GenericBucket::Jpan => Some('\u{4E00}'),
            GenericBucket::Kore => Some('\u{AC00}'),
            _ => None,
        };
        for family in GENERICS {
            let (names, rule): (&[&str], _) = match family {
                Serif | UiSerif => (&["serif"], cjk.map(|_| CjkGeneric::Serif)),
                SansSerif | UiSansSerif | UiRounded => {
                    (&["sans-serif"], cjk.map(CjkGeneric::Character))
                }
                Monospace | UiMonospace => (&["monospace"], None),
                Cursive => (&["cursive"], cjk.map(CjkGeneric::Character)),
                Fantasy => (&["fantasy"], cjk.map(CjkGeneric::Character)),
                Math => (&["math"], None),
                SystemUi => (&["sans-serif"], None),
                Emoji | FangSong => (&[], None),
            };
            assert_eq!(
                TABLE.families(family, bucket),
                names,
                "{family:?} {bucket:?}"
            );
            assert_eq!(cjk_generic(family, bucket), rule, "{family:?} {bucket:?}");
        }
        assert_eq!(TABLE.standard(bucket), &[] as &[&str]);
        assert_eq!(standard_cjk(bucket), cjk, "{bucket:?}");
    }
}
