//! Tests of the `fonts.xml` backend, and of the Android generics.

mod generics;

use super::*;
use crate::fallback::{BackendFacts, FallbackRequest};
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;
use parlance::{GenericFamily, Script};

const AOSP_MAIN: &str = include_str!("../../../../tests/fixtures/android/fonts-aosp-main.xml");
const FONT_DIR: &str = "/system/fonts/";

/// Stands in for reading a font's `name` table.
///
/// Deriving the name from the file name is exactly what
/// shipping code must never do, and it is the right
/// thing in a test: it is total, it is deterministic, and it lets these
/// tests check the assembly rather than a font parser.
struct FromFileName;

impl FamilyNames for FromFileName {
    fn family_name(&self, path: &str, index: u32) -> Option<String> {
        assert!(
            path.starts_with(FONT_DIR),
            "the backend should have joined the font directory: {path}"
        );
        let file = &path[FONT_DIR.len()..];
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        let stem = stem.split('-').next().unwrap_or(stem);
        Some(if index == 0 {
            stem.to_string()
        } else {
            format!("{stem}#{index}")
        })
    }
}

/// Resolves nothing, as a device with the file but none of its fonts.
struct Absent;

impl FamilyNames for Absent {
    fn family_name(&self, _path: &str, _index: u32) -> Option<String> {
        None
    }
}

fn source<N: FamilyNames>(names: N) -> Android {
    Android::from_fonts_xml(AOSP_MAIN, FONT_DIR, names).expect("main parses")
}

const FACTS: BackendFacts = BackendFacts {
    reads_serif: true,
    reads_monospace: false,
    per_language: true,
    reads_language: false,
};

fn key_families(source: &Android, request: &FallbackRequest) -> Vec<String> {
    let key = FallbackKey::new(request, FACTS);
    let mut names: Vec<String> = Vec::new();
    source.families(&key, &mut |name| {
        if !names.iter().any(|seen| seen == name) {
            names.push(String::from(name));
        }
    });
    names
}

fn families(source: &Android, script: &[u8; 4], serif: bool) -> Vec<String> {
    let generic = if serif {
        GenericClass::Serif
    } else {
        GenericClass::Plain
    };
    key_families(
        source,
        &FallbackRequest::Text {
            script: Script::from_bytes(*script),
            language: None,
            generic,
        },
    )
}

#[test]
fn a_script_query_names_that_script_s_font() {
    let source = source(FromFileName);
    let arabic = families(&source, b"Arab", false);
    assert!(
        arabic.iter().any(|family| family == "NotoNaskhArabic"),
        "got {arabic:?}"
    );
}

#[test]
fn a_generic_is_its_named_family() {
    let source = source(FromFileName);
    let sans = key_families(
        &source,
        &FallbackRequest::Generic(GenericFamily::SansSerif, None),
    );
    assert_eq!(sans.first().map(String::as_str), Some("Roboto"));
}

#[test]
fn the_han_tradition_s_fonts_lead() {
    let source = source(FromFileName);
    let japanese = key_families(
        &source,
        &FallbackRequest::Text {
            script: Script::from_bytes(*b"Latn"),
            language: crate::parse_language("ja"),
            generic: GenericClass::Plain,
        },
    );
    assert_eq!(
        japanese.first().map(String::as_str),
        Some("NotoSansCJK"),
        "{japanese:?}"
    );
}

#[test]
fn a_serif_request_reaches_the_per_script_serif_first() {
    let source = source(FromFileName);
    let serif = families(&source, b"Ethi", true);
    let at = |name: &str| serif.iter().position(|family| family == name);
    assert!(
        at("NotoSerifEthiopic") < at("NotoSansEthiopic"),
        "got {serif:?}"
    );
}

#[test]
fn the_answer_ends_in_the_default_family() {
    let source = source(FromFileName);
    let answer = families(&source, b"Deva", false);
    assert!(answer.iter().any(|family| family == "Roboto"), "{answer:?}");
}

#[test]
fn a_resolver_that_answers_nothing_names_nothing() {
    // The file is present and parses; the fonts it names are not there.
    // Naming families the caller cannot resolve would be worse.
    let source = source(Absent);
    assert!(families(&source, b"Arab", false).is_empty());
}

#[test]
fn a_map_is_a_resolver_and_is_handed_joined_paths() {
    // The shape a caller with a collection already has. If the key it is
    // asked for were a bare file name, this would need a second index.
    let mut map = BTreeMap::new();
    map.insert(
        (String::from("/system/fonts/NotoNaskhArabic-Regular.ttf"), 0),
        String::from("Noto Naskh Arabic"),
    );
    let source = Android::from_fonts_xml(AOSP_MAIN, FONT_DIR, map).expect("parses");
    assert_eq!(
        families(&source, b"Arab", false),
        ["Noto Naskh Arabic"],
        "only the one font in the map should have resolved"
    );
}

#[test]
fn the_font_directory_takes_a_trailing_separator_or_not() {
    struct Capture;
    impl FamilyNames for Capture {
        fn family_name(&self, path: &str, _index: u32) -> Option<String> {
            assert!(!path.contains("//"), "the join doubled a separator: {path}");
            assert!(path.starts_with("/system/fonts/"), "got {path}");
            None
        }
    }
    for dir in ["/system/fonts", "/system/fonts/"] {
        let _ = Android::from_fonts_xml(AOSP_MAIN, dir, Capture).expect("parses");
    }
}

#[test]
fn a_file_that_is_not_fonts_xml_is_told_apart() {
    assert_eq!(
        Android::from_fonts_xml("<html/>", FONT_DIR, FromFileName).map(|_| ()),
        Err(ConfigError::NotFontsXml)
    );
    assert!(matches!(
        Android::from_fonts_xml("<familyset", FONT_DIR, FromFileName).map(|_| ()),
        Err(ConfigError::Malformed(_))
    ));
}
