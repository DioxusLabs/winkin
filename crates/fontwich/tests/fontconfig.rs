//! Checks the fontconfig backend against the fonts actually installed on this
//! machine.
//!
//! Like `tests/coretext.rs`, there is no table here to verify — the backend
//! asks fontconfig at query time. What this checks is that the plumbing works
//! and that the two things we put in the pattern actually steer the answer.
//!
//! Every assertion that depends on a particular font being installed skips
//! instead of failing when it is not. A bare container has almost no fonts,
//! and a test suite that fails there would say nothing about this crate.

#![cfg(all(unix, not(target_vendor = "apple"), feature = "system"))]

use fontconfig_sys::constants::{FC_FAMILY, FC_MONO, FC_SPACING};
use fontconfig_sys::{FcMatchPattern, FcResultMatch};
use std::sync::Arc;

use fontwich::backend::{Backend, Fontconfig};
use fontwich::{
    Collection, FallbackRequest, GenericClass, GenericFamily, LayerBuilder, Role, Script,
};

/// The library's own `source::fontconfig::ffi` is crate-private, so this test
/// carries its own copy of the two-mode dispatch. It is a few lines, and the
/// alternative — exporting the macro — would put an implementation detail of
/// the backend in the public API forever.
macro_rules! fc_call {
    ($lib:expr, $func:ident($($arg:expr),* $(,)?)) => {{
        #[cfg(fontconfig_dlopen)]
        let result = ($lib.$func)($($arg),*);
        #[cfg(not(fontconfig_dlopen))]
        let result = {
            let _ = $lib;
            fontconfig_sys::$func($($arg),*)
        };
        result
    }};
}

/// The loaded library, or a placeholder when it is linked instead.
#[cfg(fontconfig_dlopen)]
fn library() -> Option<&'static fontconfig_sys::Fc> {
    fontconfig_sys::statics::LIB_RESULT.as_ref().ok()
}

/// The linked build has nothing to load, so this always answers.
#[cfg(not(fontconfig_dlopen))]
fn library() -> Option<&'static ()> {
    Some(&())
}

/// `script::DEVA` and friends are crate-private, so tests name tags directly.
fn sc(tag: &[u8; 4]) -> Script {
    Script::from_bytes(*tag)
}

/// Resolves `family` the way the backend does, and reads `take` off the font
/// fontconfig lands on.
///
/// The public API only ever yields family *names*, so every question about
/// what a name actually denotes — which font won, how it is spaced — is the
/// same match with a different value pulled out of it.
fn resolved<T>(
    family: &str,
    take: impl FnOnce(*mut fontconfig_sys::FcPattern) -> Option<T>,
) -> Option<T> {
    let fc = library()?;
    let name = std::ffi::CString::new(family).ok()?;
    unsafe {
        let pattern = fc_call!(fc, FcPatternCreate());
        if pattern.is_null() {
            return None;
        }
        fc_call!(
            fc,
            FcPatternAddString(pattern, FC_FAMILY.as_ptr(), name.as_ptr().cast())
        );
        fc_call!(
            fc,
            FcConfigSubstitute(std::ptr::null_mut(), pattern, FcMatchPattern)
        );
        fc_call!(fc, FcDefaultSubstitute(pattern));

        let mut result = FcResultMatch;
        let matched = fc_call!(fc, FcFontMatch(std::ptr::null_mut(), pattern, &mut result));
        fc_call!(fc, FcPatternDestroy(pattern));
        if matched.is_null() {
            return None;
        }

        let value = take(matched);
        fc_call!(fc, FcPatternDestroy(matched));
        value
    }
}

/// Fontconfig's `FC_SPACING` for the font it resolves `family` to, or `None`
/// when it does not report one.
///
/// `None` is genuinely "fontconfig did not say", not "proportional": its scan
/// records a spacing only when it positively decides one, and plenty of
/// monospaced fonts never get one. On a stock Fedora 44, DejaVu Sans
/// (proportional) and Noto Sans Mono (monospaced, and fontconfig's own answer
/// for `monospace`) are alike unset, while DejaVu Sans Mono reports 100. So
/// only a value that is present and below [`FC_MONO`] is evidence of anything.
fn family_spacing(family: &str) -> Option<i32> {
    resolved(family, |font| {
        let fc = library()?;
        unsafe {
            let mut spacing = 0;
            let spacing_key = FC_SPACING.as_ptr();
            let found = fc_call!(fc, FcPatternGetInteger(font, spacing_key, 0, &mut spacing));
            (found == FcResultMatch).then_some(spacing)
        }
    })
}

/// Every family fontconfig itself classifies as monospaced.
///
/// The independent oracle for "does this system have a monospaced font at
/// all". Asking the *answer* whether it is monospaced cannot distinguish
/// "this font is proportional" from "there was nothing better available":
/// proportional fonts mostly leave `FC_SPACING` unset entirely, so a missing
/// value means both.
fn monospace_families() -> Vec<String> {
    let Some(fc) = library() else {
        return Vec::new();
    };
    let mut families = Vec::new();
    unsafe {
        let pattern = fc_call!(fc, FcPatternCreate());
        let objects = fc_call!(fc, FcObjectSetCreate());
        if pattern.is_null() || objects.is_null() {
            return families;
        }
        fc_call!(
            fc,
            FcPatternAddInteger(pattern, FC_SPACING.as_ptr(), FC_MONO)
        );
        fc_call!(fc, FcObjectSetAdd(objects, FC_FAMILY.as_ptr()));

        let set = fc_call!(fc, FcFontList(std::ptr::null_mut(), pattern, objects));
        if !set.is_null() {
            for index in 0..(*set).nfont.max(0) as usize {
                let font = *(*set).fonts.add(index);
                let mut value = std::ptr::null_mut();
                if fc_call!(
                    fc,
                    FcPatternGetString(font, FC_FAMILY.as_ptr(), 0, &mut value)
                ) == FcResultMatch
                    && !value.is_null()
                {
                    families.push(
                        std::ffi::CStr::from_ptr(value.cast())
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
            fc_call!(fc, FcFontSetDestroy(set));
        }
        fc_call!(fc, FcObjectSetDestroy(objects));
        fc_call!(fc, FcPatternDestroy(pattern));
    }
    families
}

/// The names fontconfig's backend gives `request`'s key, keyed as a
/// collection of the backend's layer keys it: by the language, which
/// fontconfig sorts by.
fn names(request: &FallbackRequest) -> Vec<String> {
    let layer = LayerBuilder::new(Role::System)
        .layer()
        .clone()
        .with_fallback(Backend::platform());
    let collection = Collection::new().with_layer(Arc::new(layer));
    let key = collection.key(request);
    let mut names: Vec<String> = Vec::new();
    Backend::platform().families(&key, |name| {
        if !names.iter().any(|seen| seen == name) {
            names.push(String::from(name));
        }
    });
    names
}

fn text(script: Script, lang: Option<&str>) -> FallbackRequest {
    FallbackRequest::Text {
        script,
        language: lang.and_then(fontwich::parse_language),
        generic: GenericClass::Plain,
    }
}

/// The first name a generic's key gives, which is the generic's family where
/// it is installed.
fn primary(lang: Option<&str>, generic: GenericFamily) -> Option<String> {
    let request = FallbackRequest::Generic(generic, lang.and_then(fontwich::parse_language));
    let collection = fontwich::Collection::system();
    collection
        .fallback(&collection.key(&request))
        .first()
        .map(|family| String::from(family.name()))
}

#[test]
fn the_library_loads() {
    // Not an assertion: a machine without libfontconfig is a supported
    // configuration, the backend just falls back to its tail there. This
    // reports which case the rest of the file is running in.
    println!("libfontconfig available: {}", Fontconfig::is_available());
}

#[test]
fn every_query_produces_a_usable_answer() {
    let cases: &[(Script, Option<&str>)] = &[
        (sc(b"Latn"), Some("en-US")),
        (sc(b"Hani"), Some("zh-Hant-TW")),
        (sc(b"Deva"), Some("hi-IN")),
        (sc(b"Arab"), Some("ar-EG")),
        (Script::COMMON, None),
    ];
    for &(script, lang) in cases {
        let families = names(&text(script, lang));
        if families.is_empty() {
            println!("skipping {script}: nothing installed for it");
            continue;
        }
        for family in &families {
            assert!(!family.is_empty(), "{script} produced an empty family name");
        }
    }
    assert!(
        !names(&text(Script::COMMON, None)).is_empty(),
        "the Common key ends in the tail"
    );
}

#[test]
fn the_language_distinguishes_the_han_traditions() {
    // This is what `FC_LANG` is for: `Hans` and `Hant` carry identical
    // sample codepoints, so a charset alone cannot tell them apart.
    let hant = names(&text(sc(b"Hani"), Some("zh-Hant-TW")));
    let ja = names(&text(sc(b"Hani"), Some("ja-JP")));
    if hant.is_empty() || ja.is_empty() {
        println!("skipping: no CJK fonts installed");
        return;
    }
    if hant.len() == 1 && hant == ja {
        println!("skipping: one CJK font for every tradition");
        return;
    }
    assert_ne!(
        hant.first(),
        ja.first(),
        "Traditional Chinese and Japanese lead with the same family; \
         the pattern is not carrying the language"
    );
}

#[test]
fn a_script_s_answer_maps_its_letters() {
    // Primed: a script key keeps only the families fontconfig's charsets say
    // map the script's sample letters, so a Latin font never answers for
    // Devanagari.
    let latin = names(&text(sc(b"Latn"), Some("en-US")));
    for (script, lang) in [
        (sc(b"Deva"), "hi-IN"),
        (sc(b"Arab"), "ar-EG"),
        (sc(b"Hebr"), "he-IL"),
        (sc(b"Thai"), "th-TH"),
    ] {
        let answer = names(&text(script, Some(lang)));
        if let (Some(first), Some(latin)) = (answer.first(), latin.first()) {
            assert_ne!(first, latin, "{script} leads with Latin's font");
        }
    }
}

#[test]
fn the_spacing_probe_can_tell_the_two_kinds_apart() {
    // Guards the helper below: a probe that answers "monospaced" for
    // everything, or for nothing, would make the next test meaningless.
    let monospaced = monospace_families();
    let Some(mono) = monospaced.first() else {
        println!("skipping: no monospaced font installed");
        return;
    };
    assert!(
        family_spacing(mono).is_some_and(|s| s >= FC_MONO),
        "{mono} should read as monospaced"
    );

    let proportional =
        primary(Some("en-US"), GenericFamily::SansSerif).expect("a sans-serif family");
    assert!(
        !family_spacing(&proportional).is_some_and(|s| s >= FC_MONO),
        "{proportional} is the sans-serif answer but reads as monospaced"
    );
}

#[test]
fn a_generic_family_is_honoured() {
    let serif = primary(Some("en-US"), GenericFamily::Serif);
    let sans = primary(Some("en-US"), GenericFamily::SansSerif);
    let mono = primary(Some("en-US"), GenericFamily::Monospace);
    assert!(serif.is_some() && sans.is_some() && mono.is_some());

    if serif == sans && sans == mono {
        println!("skipping: only one Latin family installed");
        return;
    }
    assert_ne!(sans, mono, "sans-serif and monospace resolved alike");

    // A monospace request that lands on a *proportional* font renders
    // misaligned text. Only a spacing that is present and proportional
    // proves that; unset proves nothing either way (see `family_spacing`).
    let mono = mono.expect("a monospace family");
    let monospaced = monospace_families();
    if monospaced.is_empty() {
        println!("skipping: this system has no monospaced font for the request to find");
        return;
    }
    assert!(
        !family_spacing(&mono).is_some_and(|spacing| spacing < FC_MONO),
        "monospace resolved to {mono}, which fontconfig reports as proportional, \
         though it knows of {} monospaced {}: {monospaced:?}",
        monospaced.len(),
        if monospaced.len() == 1 {
            "family"
        } else {
            "families"
        }
    );
}

#[test]
fn fontconfig_only_names_families_it_has() {
    // fontconfig returns patterns for real fonts, so anything it named
    // should be a plausible family name rather than, say, a mis-decoded
    // string. Cheap guard on the FFI reading `FC_FAMILY`.
    for family in names(&text(sc(b"Latn"), Some("en-US"))) {
        assert!(
            family.chars().any(|c| c.is_alphanumeric()),
            "implausible family name from fontconfig: {family:?}"
        );
        assert!(!family.contains('\0'), "family name kept a NUL: {family:?}");
    }
}

#[test]
fn one_backend_answers_the_same_from_many_threads() {
    // A collection shares its layers across threads, and every call into
    // fontconfig is serialized. Several threads asking in different orders
    // must each get what one thread alone gets.
    let queries: [(&[u8; 4], Option<&str>); 6] = [
        (b"Latn", Some("en-US")),
        (b"Arab", Some("ar")),
        (b"Deva", Some("hi-IN")),
        (b"Hani", Some("zh-Hant-TW")),
        (b"Hani", Some("ja-JP")),
        (b"Thai", None),
    ];
    let expected: Vec<Vec<String>> = queries
        .iter()
        .map(|&(script, lang)| names(&text(sc(script), lang)))
        .collect();
    std::thread::scope(|scope| {
        for thread in 0..8 {
            let (queries, expected) = (&queries, &expected);
            scope.spawn(move || {
                for round in 0..3 {
                    for step in 0..queries.len() {
                        let at = (step + thread + round) % queries.len();
                        let (script, lang) = queries[at];
                        let got = names(&text(sc(script), lang));
                        assert_eq!(
                            got, expected[at],
                            "thread {thread} round {round} query {at}"
                        );
                    }
                }
            });
        }
    });
}

#[test]
fn local_finds_an_installed_font_by_full_or_postscript_name() {
    let collection = fontwich::Collection::system();
    // DejaVu is installed on every desktop Linux worth testing on; where it
    // is not, there is nothing to find.
    if collection.family("DejaVu Sans").is_none() {
        return;
    }
    let weight = |name: &str| collection.local(name).map(|font| font.weight().value());
    assert_eq!(weight("DejaVu Sans Bold"), Some(700.0));
    assert_eq!(weight("DejaVuSans-Bold"), Some(700.0));
    assert_eq!(weight("No Such Font Anywhere"), None);
}

#[test]
fn local_holds_a_named_instance_where_its_name_points() {
    // fontconfig lists a variable font's named instances by their own full
    // names, with the instance in the upper half of `FC_INDEX`. The name
    // means that point of the font, so the font comes back held there:
    // "Vazirmatn ExtraLight" is its weight axis at 200, not Vazirmatn at its
    // default.
    use fontwich::Attributes;

    let collection = fontwich::Collection::system();
    let Some(light) = collection.local("Vazirmatn ExtraLight") else {
        return; // Not installed.
    };
    assert_eq!(light.weight().value(), 200.0);
    let synthesis = light.synthesis(Attributes::default());
    let settings: Vec<([u8; 4], f32)> = synthesis
        .variation_settings()
        .iter()
        .map(|setting| (setting.tag.to_bytes(), setting.value))
        .collect();
    assert_eq!(settings, [(*b"wght", 200.0)]);

    // The font's own full name leaves it whole, with the axis it varies
    // along, which a rule declaring a range over `local()` needs.
    let whole = collection
        .local("Vazirmatn Regular")
        .expect("its own full name");
    let weight = whole.axis(b"wght").expect("a weight axis");
    assert!(weight.min < weight.max, "{weight:?}");
}

#[test]
fn a_second_family_name_is_a_family_of_only_its_fonts() {
    // fontconfig names DejaVu's condensed fonts both "DejaVu Sans" and
    // "DejaVu Sans Condensed", in English both; asked for the second, it
    // answers with those fonts alone, and so does the layer.
    let collection = fontwich::Collection::system();
    let Some(condensed) = collection.family("DejaVu Sans Condensed") else {
        return;
    };
    assert_eq!(condensed.name(), "DejaVu Sans Condensed");
    assert!(!condensed.fonts().is_empty());
    assert!(
        condensed
            .fonts()
            .iter()
            .all(|font| font.width().ratio() < 1.0)
    );
    let all = collection.family("DejaVu Sans").unwrap();
    assert!(all.fonts().len() > condensed.fonts().len());
}
