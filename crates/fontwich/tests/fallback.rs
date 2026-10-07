//! Fallback as a collection answers it: a request's key, a key's installed
//! families, a generic's one family, and the walk a missed character makes.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fontwich::{
    Collection, FallbackKey, FallbackOverride, FallbackRequest, Family, Font, FontFamilyName,
    GenericClass, GenericFamily, Layer, LayerBuilder, LoadFamily, Presentation, Role, Script,
    parse_language,
};

/// Answers every key from a list of names, as a host with its own fonts
/// would: `generic` for a generic key, `han` for a Han key, `common` for the
/// Common key and `text` for every other text key.
#[derive(Default)]
struct Names {
    generic: &'static [&'static str],
    han: &'static [&'static str],
    common: &'static [&'static str],
    text: &'static [&'static str],
    asked: AtomicUsize,
}

impl FallbackOverride for Names {
    fn families(&self, key: &FallbackKey, collection: &Collection, out: &mut Vec<Family>) {
        self.asked.fetch_add(1, Ordering::Relaxed);
        let names = if key.generic().is_some() {
            self.generic
        } else if key.han().is_some() && key.script() == Some(Script::from_bytes(*b"Hani")) {
            self.han
        } else if key.script() == Some(Script::COMMON) {
            self.common
        } else if key.script().is_some() {
            self.text
        } else {
            &[]
        };
        out.extend(
            names
                .iter()
                .filter_map(|name| collection.fallback_family(name)),
        );
    }
}

/// Lists names and loads nothing, counting what it is asked to load.
#[derive(Debug, Default)]
struct Counted(AtomicUsize);

impl LoadFamily for Counted {
    fn load(&self, name: &str) -> Vec<Font> {
        self.0.fetch_add(1, Ordering::Relaxed);
        vec![Font::from_data(font_named(name), 0)]
    }
}

fn text(script: &[u8; 4], language: Option<&str>) -> FallbackRequest {
    FallbackRequest::Text {
        script: Script::from_bytes(*script),
        language: language.and_then(parse_language),
        generic: GenericClass::Plain,
    }
}

fn names(families: &[Family]) -> Vec<&str> {
    families.iter().map(Family::name).collect()
}

/// An application layer of `fonts`, answering fallback as `fallback` says.
fn application(fonts: &[Vec<u8>], fallback: Names) -> Arc<Layer> {
    let mut layer = LayerBuilder::new(Role::Application);
    for font in fonts {
        assert!(layer.add_data(font.clone()).is_ok());
    }
    layer.set_fallback_override(fallback);
    layer.snapshot()
}

#[test]
fn a_key_s_families_are_the_installed_ones_in_order_each_once() {
    let fonts = [font_named("Latin A"), font_named("Latin B")];
    let fallback = Names {
        text: &["Latin B", "Not Installed", "Latin A", "Latin B"],
        ..Names::default()
    };
    let collection = Collection::new().with_layer(application(&fonts, fallback));
    let families = collection.fallback(&collection.key(&text(b"Latn", None)));
    assert_eq!(names(&families), ["Latin B", "Latin A"]);
}

#[test]
fn a_key_is_answered_once_and_kept() {
    let fallback = Arc::new(Names {
        text: &["Latin A"],
        ..Names::default()
    });
    struct Shared(Arc<Names>);
    impl FallbackOverride for Shared {
        fn families(&self, key: &FallbackKey, collection: &Collection, out: &mut Vec<Family>) {
            self.0.families(key, collection, out);
        }
    }
    let mut layer = LayerBuilder::new(Role::Application);
    assert!(layer.add_data(font_named("Latin A")).is_ok());
    layer.set_fallback_override(Shared(fallback.clone()));
    let collection = Collection::new().with_layer(layer.snapshot());
    let key = collection.key(&text(b"Latn", Some("en")));
    let first = collection.fallback(&key);
    let second = collection.clone().fallback(&key);
    assert!(Arc::ptr_eq(&first, &second), "a clone shares the answers");
    assert_eq!(fallback.asked.load(Ordering::Relaxed), 1);
    // A document's layer changes no answer; a layer fallback reads does.
    let mut document = collection.clone();
    document.push(Arc::new(LayerBuilder::new(Role::Document).layer().clone()));
    assert!(Arc::ptr_eq(&first, &document.fallback(&key)));
    let mut more = collection.clone();
    more.push(application(&[font_named("Latin B")], Names::default()));
    assert!(!Arc::ptr_eq(&first, &more.fallback(&key)));
}

#[test]
fn filling_a_key_loads_no_font() {
    let loader = Arc::new(Counted::default());
    let layer = Layer::from_names(
        Role::System,
        ["Latin A", "Latin B"].map(|name| (String::from(name), Vec::<String>::new())),
        loader.clone(),
    )
    .with_fallback_override(Names {
        text: &["Latin B", "Latin A"],
        ..Names::default()
    });
    let collection = Collection::new().with_layer(Arc::new(layer));
    let families = collection.fallback(&collection.key(&text(b"Latn", None)));
    assert_eq!(names(&families), ["Latin B", "Latin A"]);
    assert_eq!(loader.0.load(Ordering::Relaxed), 0);
}

#[test]
fn a_generic_is_one_family_and_never_a_document_s() {
    let fonts = [
        font_named("Serif A"),
        font_named("Serif B"),
        font_named("Han A"),
    ];
    let fallback = Names {
        generic: &["Serif A", "Serif B"],
        ..Names::default()
    };
    let mut document = LayerBuilder::new(Role::Document);
    assert!(document.add_data(font_named("Serif A")).is_ok());
    let collection = Collection::new()
        .with_layer(application(&fonts, fallback))
        .with_layer(document.snapshot());
    let request = text(b"Latn", None);
    let serif = collection
        .resolve(&FontFamilyName::Generic(GenericFamily::Serif), &request)
        .expect("a serif");
    assert_eq!(serif.name(), "Serif A");
    assert_eq!(serif.role(), Role::Application);
    let key = collection.key(&FallbackRequest::Generic(GenericFamily::Serif, None));
    assert_eq!(names(&collection.fallback(&key)), ["Serif A"]);
    // A name is looked up from the top, as a `font-family` list looks it up.
    let named = collection
        .resolve(&FontFamilyName::Named("serif a".into()), &request)
        .expect("named");
    assert_eq!(named.role(), Role::Document);
}

#[test]
fn a_missed_character_asks_its_own_key_then_the_common_key() {
    let fonts = [
        font_covering("Han A", &[(0x4E00, 0x9FFF)]),
        font_covering("Common A", &[(0x20, 0x7E)]),
        font_covering("Latin A", &[(0x41, 0x5A)]),
    ];
    let fallback = Names {
        han: &["Han A"],
        common: &["Common A"],
        text: &["Latin A"],
        ..Names::default()
    };
    let collection = Collection::new().with_layer(application(&fonts, fallback));
    let run = text(b"Latn", Some("ja"));
    let walked: Vec<String> = collection
        .char_fallback('漢', Presentation::Text, &run)
        .map(|family| String::from(family.name()))
        .collect();
    // Han's, then Common's, then everything else, each once.
    assert_eq!(walked, ["Han A", "Common A", "Latin A"]);
}

#[test]
fn a_missed_character_reaches_a_family_no_backend_names() {
    // The only font with U+A000 is one nobody names: the walk still reaches
    // it, past the keys' families, so it is not tofu.
    let fonts = [
        font_covering("Common A", &[(0x20, 0x7E)]),
        font_covering("Obscure Yi", &[(0xA000, 0xA48C)]),
        font_covering("Latin A", &[(0x41, 0x5A)]),
    ];
    let fallback = Names {
        common: &["Common A"],
        text: &["Latin A"],
        ..Names::default()
    };
    let collection = Collection::new().with_layer(application(&fonts, fallback));
    let found = collection
        .char_fallback('\u{A000}', Presentation::Text, &text(b"Latn", None))
        .find(|family| family.covers('\u{A000}'))
        .map(|family| String::from(family.name()));
    assert_eq!(found.as_deref(), Some("Obscure Yi"));
    // And with no override at all, as wasm with a folder of fonts has it.
    let mut bare = LayerBuilder::new(Role::Application);
    for font in &fonts {
        assert!(bare.add_data(font.clone()).is_ok());
    }
    let collection = Collection::new().with_layer(bare.snapshot());
    let found = collection
        .char_fallback('\u{A000}', Presentation::Text, &text(b"Yiii", None))
        .find(|family| family.covers('\u{A000}'))
        .map(|family| String::from(family.name()));
    assert_eq!(found.as_deref(), Some("Obscure Yi"));
}

#[test]
fn a_character_no_font_maps_walks_every_family_and_ends() {
    let fonts = [font_covering("Common A", &[(0x20, 0x7E)])];
    let collection = Collection::new().with_layer(application(&fonts, Names::default()));
    let request = text(b"Latn", None);
    let mut walk = collection.char_fallback('\u{E000}', Presentation::Text, &request);
    let walked = walk
        .by_ref()
        .filter(|family| !family.covers('\u{E000}'))
        .count();
    assert_eq!(walked, 1);
    assert!(walk.next().is_none());
    assert!(walk.next().is_none());
}

#[test]
fn the_default_language_answers_a_request_with_none() {
    let collection = Collection::new().with_default_language(parse_language("ja"));
    assert_eq!(
        collection.key(&text(b"Hani", None)),
        collection.key(&text(b"Hani", Some("ja")))
    );
    assert_ne!(
        collection.key(&text(b"Hani", None)),
        Collection::new().key(&text(b"Hani", None))
    );
}

/// A font with an English family name and a format 4 cmap mapping `ranges`
/// to consecutive glyphs. The crate's own builders are test-only and private,
/// so this is a copy.
fn font_covering(family: &str, ranges: &[(u16, u16)]) -> Vec<u8> {
    let mut segments: Vec<(u16, u16, u16)> = Vec::new();
    let mut glyph: u16 = 1;
    for &(start, end) in ranges {
        segments.push((start, end, glyph.wrapping_sub(start)));
        glyph += end - start + 1;
    }
    segments.push((0xFFFF, 0xFFFF, 1));
    let count = segments.len() as u16;
    let mut subtable = Vec::new();
    for field in [4, 16 + 8 * count, 0, 2 * count, 0, 0, 0] {
        subtable.extend_from_slice(&field.to_be_bytes());
    }
    subtable.extend(segments.iter().flat_map(|s| s.1.to_be_bytes()));
    subtable.extend_from_slice(&0u16.to_be_bytes());
    subtable.extend(segments.iter().flat_map(|s| s.0.to_be_bytes()));
    subtable.extend(segments.iter().flat_map(|s| s.2.to_be_bytes()));
    subtable.extend(segments.iter().flat_map(|_| 0u16.to_be_bytes()));
    let mut cmap = Vec::new();
    for field in [0u16, 1, 3, 1] {
        cmap.extend_from_slice(&field.to_be_bytes()); // version, count, Windows BMP
    }
    cmap.extend_from_slice(&12u32.to_be_bytes());
    cmap.extend_from_slice(&subtable);

    let named = font_named(family);
    let name = named[28..].to_vec();
    let mut font = Vec::new();
    font.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    for field in [2u16, 32, 1, 0] {
        font.extend_from_slice(&field.to_be_bytes());
    }
    let cmap_at = 12 + 32;
    let name_at = (cmap_at + cmap.len()).next_multiple_of(4);
    for (tag, at, len) in [
        (b"cmap", cmap_at, cmap.len()),
        (b"name", name_at, name.len()),
    ] {
        font.extend_from_slice(tag);
        font.extend_from_slice(&0u32.to_be_bytes());
        font.extend_from_slice(&(at as u32).to_be_bytes());
        font.extend_from_slice(&(len as u32).to_be_bytes());
    }
    font.extend_from_slice(&cmap);
    font.resize(name_at, 0);
    font.extend_from_slice(&name);
    font
}

/// The smallest font with an English family name: a `name` table and nothing
/// else. The crate's own builder is test-only and private, so this is a copy.
fn font_named(family: &str) -> Vec<u8> {
    let text: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut name = Vec::new();
    for field in [0u16, 1, 18] {
        name.extend_from_slice(&field.to_be_bytes()); // format, count, storage offset
    }
    for field in [3u16, 1, 0x0409, 1, text.len() as u16, 0] {
        name.extend_from_slice(&field.to_be_bytes()); // one Windows English family record
    }
    name.extend_from_slice(&text);
    let mut font = Vec::new();
    font.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    for field in [1u16, 16, 0, 0] {
        font.extend_from_slice(&field.to_be_bytes());
    }
    font.extend_from_slice(b"name");
    font.extend_from_slice(&0u32.to_be_bytes());
    font.extend_from_slice(&28u32.to_be_bytes());
    font.extend_from_slice(&(name.len() as u32).to_be_bytes());
    font.extend_from_slice(&name);
    font
}
