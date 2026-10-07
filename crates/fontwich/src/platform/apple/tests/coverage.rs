//! The sample letters against every font the system layer lists.

use crate::fallback::{report_outliers, sample_codepoints, sample_scripts};
use crate::{Collection, Layer};

/// How many installed fonts carry each sample codepoint.
///
/// Every font on the system rather than a panel, as the fontconfig
/// checker does and the DirectWrite one cannot: the layer already lists
/// every family and reads a charset per font as it loads, so the count
/// is a walk over charsets already in hand, not a query per codepoint
/// into the platform.
///
/// Loading every family is the cost here — a few tens of milliseconds
/// and every system font's `cmap` — which is why the whole table is
/// counted in one pass rather than `report_outliers` calling back per
/// codepoint into a fresh collection.
fn covering() -> crate::hash::HashMap<u32, usize> {
    let layer = alloc::sync::Arc::new(Layer::system());
    let collection = Collection::new().with_layer(layer.clone());
    let mut counts: crate::hash::HashMap<u32, usize> = sample_scripts()
        .flat_map(sample_codepoints)
        .map(|&codepoint| (codepoint, 0))
        .collect();
    for name in layer.names() {
        let Some(family) = collection.family(name) else {
            continue;
        };
        for font in family.fonts() {
            for (codepoint, count) in counts.iter_mut() {
                if char::from_u32(*codepoint).is_some_and(|c| font.charset().contains(c)) {
                    *count += 1;
                }
            }
        }
    }
    counts
}

#[test]
fn no_sample_codepoint_is_an_outlier() {
    let counts = covering();
    assert!(
        counts.values().any(|&n| n > 0),
        "no installed font carries any sample; nothing to judge against"
    );
    let suspect = report_outliers(|codepoint| counts.get(&codepoint).copied().unwrap_or(0));
    assert!(
        suspect.is_empty(),
        "{} script(s) have a sample far less supported than its siblings, \
         which will demote good fonts:\n  {}\n\nReplace it with a more \
         established letter from the same script.",
        suspect.len(),
        suspect.join("\n  ")
    );
}
