//! Prints the platform's fallback names for a handful of representative
//! keys: each run's generic, its text key and its emoji key.
//!
//! `cargo run --example dump`

use fontwich::backend::Backend;
use fontwich::{
    Collection, FallbackRequest, GenericClass, GenericFamily, Presentation, Script, parse_language,
};

const CASES: &[(&str, &str, GenericFamily)] = &[
    ("Latn", "en-US", GenericFamily::SansSerif),
    ("Hani", "zh-Hant-TW", GenericFamily::SansSerif),
    ("Hani", "ja-JP", GenericFamily::Serif),
    ("Deva", "hi-IN", GenericFamily::Serif),
    ("Arab", "ar-EG", GenericFamily::Serif),
    ("Zyyy", "en-US", GenericFamily::Monospace),
    ("Arab", "ar-AR", GenericFamily::SansSerif),
    ("Latn", "en-US", GenericFamily::SystemUi),
    ("Arab", "ar", GenericFamily::SystemUi),
    ("Latn", "en-US", GenericFamily::Serif),
    ("Arab", "fa", GenericFamily::SansSerif),
    ("Arab", "ur", GenericFamily::SansSerif),
];

fn main() {
    // The collection only keys the requests: its layers' backends say what a
    // key keeps. The names are the backend's, installed or not.
    let collection = Collection::system();
    let backend = Backend::platform();
    for &(script, language, generic) in CASES {
        let language = parse_language(language);
        let class = GenericClass::from(generic);
        println!("\n{script} / {language:?} / {generic:?}");
        for (label, request) in [
            ("generic", FallbackRequest::Generic(generic, language)),
            (
                "text",
                FallbackRequest::Text {
                    script: Script::parse(script).unwrap(),
                    language,
                    generic: class,
                },
            ),
            ("emoji", FallbackRequest::Emoji(Presentation::Emoji)),
        ] {
            println!("  {label}");
            backend.families(&collection.key(&request), |family| {
                println!("      {family}");
            });
        }
    }
}
