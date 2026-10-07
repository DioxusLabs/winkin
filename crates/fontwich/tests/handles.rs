//! Family handles as cache keys.
//!
//! There is no family id: a handle is what a consumer keys its caches by, so
//! it has to hash and compare the way a key must. That is std's `HashSet`
//! here, which the crate itself cannot use.

// A `Family` holds its layer, whose records carry the cell a family's fonts
// load into, so clippy's `mutable_key_type` sees interior mutability behind
// the key. Equality and hashing use only the record's address, which that
// cell never changes, so the lint is a false positive here -- and one any
// caller keying a map by `Family` will meet too.
#![allow(clippy::mutable_key_type)]

use std::collections::HashSet;
use std::sync::Arc;

use fontwich::{Collection, Font, Layer, LoadFamily, Role};

#[derive(Debug)]
struct Nothing;

impl LoadFamily for Nothing {
    fn load(&self, _: &str) -> Vec<Font> {
        Vec::new()
    }
}

fn collection(names: &[&str]) -> Collection {
    let layer = Layer::from_names(
        Role::System,
        names.iter().map(|name| (String::from(*name), Vec::new())),
        Arc::new(Nothing),
    );
    Collection::new().with_layer(Arc::new(layer))
}

#[test]
fn a_family_found_twice_is_one_key() {
    let collection = collection(&["Roboto", "Noto Sans"]);
    let mut seen = HashSet::new();
    assert!(seen.insert(collection.family("Roboto").expect("present")));
    assert!(!seen.insert(collection.family("ROBOTO").expect("present")));
    assert!(seen.insert(collection.family("Noto Sans").expect("present")));
    assert_eq!(seen.len(), 2);
}

#[test]
fn the_same_name_in_two_collections_is_two_keys() {
    // Identity is the family, not its name: two separately built layers are
    // two families, even called the same thing. Compare names with
    // `fontwich::names_match` where that is the question.
    let a = collection(&["Roboto"]).family("Roboto").expect("present");
    let b = collection(&["Roboto"]).family("Roboto").expect("present");
    assert_ne!(a, b);
    assert!(fontwich::names_match(a.name(), b.name()));
    let keys: HashSet<_> = [a, b].into_iter().collect();
    assert_eq!(keys.len(), 2);
}
