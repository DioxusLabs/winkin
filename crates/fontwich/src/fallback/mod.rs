//! Installed families for a fallback key, and the walk a missed character makes.

mod cache;
mod classify;
mod emoji;
mod generic;
mod key;
mod language;
mod samples;
#[cfg(test)]
mod tests;
mod unicode;
mod walk;

pub(crate) use cache::{FallbackCache, stamp};
pub use emoji::Presentation;
#[cfg(fontwich_android)]
pub(crate) use generic::normalize;
#[cfg(test)]
pub(crate) use generic::setting;
// For the platform modules' generic tables, where a build has one.
#[cfg(any(
    windows,
    test,
    fontwich_android,
    fontwich_fontconfig,
    all(target_vendor = "apple", feature = "system")
))]
pub(crate) use generic::{GenericTable, Setting};
pub(crate) use key::BackendFacts;
pub use key::{FallbackKey, FallbackRequest, GenericBucket, GenericClass, Han, parse_language};
#[cfg(fontwich_android)]
pub(crate) use language::resolve_han;
pub(crate) use samples::sample_codepoints;
// For the coverage checks, which only the platforms that measure font
// coverage have.
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use samples::{report_outliers, sample_scripts};
#[cfg(test)]
pub(crate) use tests::{
    BUCKETS, COLUMNS, GENERICS, SETTINGS, bucket, drawn, grd_families, message_id,
    registered_on_desktop, registered_on_mac_and_windows,
};
pub(crate) use walk::FallbackFor;
#[cfg(all(test, windows, feature = "system"))]
pub(crate) use walk::miss_keys;

use crate::{Collection, Family};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Provides fallback families for a key.
///
/// Install on a layer with
/// [`LayerBuilder::set_fallback_override`](crate::LayerBuilder::set_fallback_override).
/// The override is consulted before the layer's backend.
///
/// Equal keys must produce equal results for the same collection state,
/// because results are cached by key.
pub trait FallbackOverride: Send + Sync {
    /// Appends fallback families in preference order.
    ///
    /// Use `collection` to resolve family names. Existing entries in `out`
    /// precede the families this override appends.
    fn families(&self, key: &FallbackKey, collection: &Collection, out: &mut Vec<Family>);
}

/// Returns the installed families `collection`'s fallback layers name for
/// `key`, in order, each once: each layer's override's, then its backend's.
///
/// A generic's answer, and the Standard font's, is its first installed
/// family alone. A per-language backend's answer for a script keeps only
/// the families mapping the script's sample letters.
pub(crate) fn answer(collection: &Collection, key: &FallbackKey) -> Arc<[Family]> {
    let mut families: Vec<Family> = Vec::new();
    for layer in collection.fallback_layers() {
        if let Some(fallback) = layer.fallback_override() {
            let from = families.len();
            fallback.families(key, collection, &mut families);
            dedup_from(&mut families, from);
        }
        if let Some(backend) = layer.backend() {
            let from = families.len();
            backend.families(key, |name| {
                if let Some(family) = collection.fallback_family(name)
                    && !families.contains(&family)
                {
                    families.push(family);
                }
            });
            if backend.filters_scripts_by_loading()
                && let Some(script) = key.script()
                && !key.is_common()
            {
                let samples = sample_codepoints(script);
                if !samples.is_empty() {
                    let mut at = 0;
                    families.retain(|family| {
                        let keep = at < from || samples.iter().all(|&c| family.covers_u32(c));
                        at += 1;
                        keep
                    });
                }
            }
        }
        if key.is_one_family() && !families.is_empty() {
            break;
        }
    }
    if key.is_one_family() {
        families.truncate(1);
    }
    families.into()
}

/// Drops from `families[from..]` every family already before it.
fn dedup_from(families: &mut Vec<Family>, from: usize) {
    let mut at = from;
    while at < families.len() {
        if families[..at].contains(&families[at]) {
            families.remove(at);
        } else {
            at += 1;
        }
    }
}
