//! Rough timing for repeated queries. `cargo run --release --example bench`
use fontwich::{Collection, FallbackRequest, GenericClass, Script, parse_language};
use std::time::Instant;

/// `script::DEVA` and friends are crate-private, so tests name tags directly.
fn sc(tag: &[u8; 4]) -> Script {
    Script::from_bytes(*tag)
}

fn main() {
    let collection = Collection::system();
    let cases = [
        (sc(b"Latn"), "en-US", GenericClass::Plain),
        (sc(b"Hani"), "ja-JP", GenericClass::Serif),
        (sc(b"Deva"), "hi-IN", GenericClass::Plain),
        (sc(b"Arab"), "ar-EG", GenericClass::Plain),
    ];
    let run = |label: &str, n: usize| {
        let start = Instant::now();
        for i in 0..n {
            let (script, language, generic) = cases[i % cases.len()];
            let request = FallbackRequest::Text {
                script,
                language: parse_language(language),
                generic,
            };
            std::hint::black_box(collection.fallback(&collection.key(&request)).len());
        }
        let each = start.elapsed() / n as u32;
        println!("{label:12} {n:>6} queries   {each:>10.3?} each");
    };
    run("cold", cases.len());
    run("warm", 100_000);
}
