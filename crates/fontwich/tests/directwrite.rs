//! Checks our family tables against the fonts actually installed on this
//! machine, via DirectWrite.
//!
//! The other tests verify the tables against Chrome and against Unicode. This
//! one verifies them against reality: it resolves each family name we claim
//! and asks the font itself, through its cmap, whether it covers the script we
//! listed it for.
//!
//! Results depend on what is installed — Windows ships many of these fonts
//! only as optional features — so a *missing* family is never a failure. A
//! family that is present and demonstrably does not cover its script is,
//! because that is a claim we got wrong.

#![cfg(windows)]

use std::collections::BTreeMap;

use fontwich::backend::Backend;
use fontwich::{Collection, FallbackRequest, GenericClass, Presentation, Script};
use icu_properties::CodePointMapData;
use icu_properties::props::{GeneralCategory, NamedEnumeratedProperty, Script as IcuScript};
use icu_properties::script::ScriptWithExtensions;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{BOOL, HSTRING};

/// A run's request for `script` in `language`.
fn run(script: Script, language: Option<&str>) -> FallbackRequest {
    FallbackRequest::Text {
        script,
        language: language.and_then(fontwich::parse_language),
        generic: GenericClass::Plain,
    }
}

/// The names the Windows tables give `request`'s key, installed or not.
fn names(request: &FallbackRequest) -> Vec<String> {
    let mut names = Vec::new();
    Backend::platform().families(&Collection::new().key(request), |name| {
        names.push(String::from(name));
    });
    names
}

/// The system font collection, wrapped in just enough API to ask the two
/// questions we care about: is this family here, and does it have this
/// character.
struct SystemFonts {
    collection: IDWriteFontCollection,
}

impl SystemFonts {
    fn new() -> Self {
        unsafe {
            let factory: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("a DirectWrite factory");
            let mut collection = None;
            factory
                .GetSystemFontCollection(&mut collection, false)
                .expect("the system font collection");
            Self {
                collection: collection.expect("a non-null collection"),
            }
        }
    }

    /// The regular font of `family`, or `None` if the name does not resolve.
    ///
    /// `FindFamilyName` matches case-insensitively and understands localized
    /// family names, so this is the same resolution a real text stack does.
    fn regular(&self, family: &str) -> Option<IDWriteFont> {
        unsafe {
            let mut index = 0;
            let mut exists = BOOL(0);
            self.collection
                .FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)
                .ok()?;
            if !exists.as_bool() {
                return None;
            }
            self.collection
                .GetFontFamily(index)
                .ok()?
                .GetFirstMatchingFont(
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                )
                .ok()
        }
    }
}

fn covered(font: &IDWriteFont, codepoints: &[u32]) -> usize {
    codepoints
        .iter()
        .filter(|&&c| unsafe { font.HasCharacter(c) }.is_ok_and(|b| b.as_bool()))
        .count()
}

/// Up to `LIMIT` codepoints per script, spread evenly across its assigned
/// range so the sample is not all from one block.
const LIMIT: usize = 64;

fn samples() -> BTreeMap<&'static str, Vec<u32>> {
    let scripts = CodePointMapData::<IcuScript>::new();
    let categories = CodePointMapData::<GeneralCategory>::new();

    let mut all: BTreeMap<&'static str, Vec<u32>> = BTreeMap::new();
    for cp in 0..=0x10FFFF_u32 {
        let Some(ch) = char::from_u32(cp) else {
            continue;
        };
        // Format and control characters say nothing about whether a font can
        // render a script.
        if matches!(
            categories.get(ch),
            GeneralCategory::Unassigned
                | GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
                | GeneralCategory::PrivateUse
        ) {
            continue;
        }
        all.entry(scripts.get(ch).short_name())
            .or_default()
            .push(cp);
    }

    for points in all.values_mut() {
        if points.len() > LIMIT {
            let stride = points.len() / LIMIT;
            *points = points.iter().copied().step_by(stride).take(LIMIT).collect();
        }
    }
    all
}

/// The families our tables claim cover `script`: its key's, less Latin's,
/// which every script's key ends with. A Han key also carries the other
/// traditions' fonts and the supplementary planes', so for Han and the
/// scripts that travel with it only the first is the script's own.
fn claimed(script: Script) -> Vec<String> {
    let latin = names(&run(Script::from_bytes(*b"Latn"), None));
    let mut own = names(&run(script, None));
    // Inherited and Unknown key as Common, whose catch-all fonts are no
    // script's own.
    if own == names(&run(Script::COMMON, None)) {
        return Vec::new();
    }
    if script == Script::from_bytes(*b"Latn") {
        return own;
    }
    if matches!(
        &script.to_bytes(),
        b"Hani" | b"Hira" | b"Kana" | b"Hang" | b"Bopo"
    ) {
        own.truncate(1);
    }
    own.into_iter()
        .filter(|family| !latin.contains(family))
        .collect()
}

#[test]
fn coverage_report() {
    let fonts = SystemFonts::new();
    let samples = samples();

    println!(
        "\n{:<6} {:<26} {:>5} {:>8}",
        "script", "family", "n", "covered"
    );
    println!("{}", "-".repeat(48));
    for (name, points) in &samples {
        let Ok(script) = Script::parse(name) else {
            continue;
        };
        for family in claimed(script) {
            match fonts.regular(&family) {
                None => println!("{name:<6} {family:<26} {:>5} {:>8}", points.len(), "absent"),
                Some(font) => {
                    let hits = covered(&font, points);
                    let pct = 100 * hits / points.len();
                    println!(
                        "{name:<6} {family:<26} {:>5} {hits:>4} ({pct:>2}%)",
                        points.len()
                    );
                }
            }
        }
    }
}

#[test]
fn installed_families_cover_the_script_they_are_listed_for() {
    // The bar is "covers something", not "covers a lot", and deliberately so.
    // The percentages in `coverage_report` swing wildly for reasons that have
    // nothing to do with whether an entry is right: Script=Han spans the
    // Ext B–I planes that only the supplementary-plane fonts carry, Script=Hira
    // includes the plane-1 Kana supplements, Script=Arab includes the
    // presentation forms. A font legitimately listed for a script can measure
    // 25%.
    //
    // Zero is the one unambiguous signal. It means the family cannot draw a
    // single character of the script we recommend it for, which is never
    // anything but a mistake in the table.
    let fonts = SystemFonts::new();
    let mut dead = Vec::new();

    for (name, points) in &samples() {
        let Ok(script) = Script::parse(name) else {
            continue;
        };
        for family in claimed(script) {
            // A family that is not installed tells us nothing either way;
            // Windows ships many of these only as optional features.
            let Some(font) = fonts.regular(&family) else {
                continue;
            };
            if covered(&font, points) == 0 {
                dead.push(format!(
                    "{name}: {family} covers 0 of {} sampled",
                    points.len()
                ));
            }
        }
    }

    assert!(
        dead.is_empty(),
        "{} table entr(y/ies) name an installed font that cannot draw the \
         script at all:\n  {}\n\nRemove them, or replace them with a font \
         that does.",
        dead.len(),
        dead.join("\n  ")
    );
}

#[test]
fn the_emoji_tier_can_actually_draw_emoji() {
    // Emoji is not an ICU script, so `samples()` never produces it and the
    // check above skips the whole tier.
    let fonts = SystemFonts::new();
    let emoji = [0x1F600, 0x1F44D, 0x2764, 0x1F1FA, 0x1F680];
    let emoji_key = names(&FallbackRequest::Emoji(Presentation::Emoji));
    let leader = emoji_key.first().expect("an emoji font");
    let font = fonts
        .regular(leader)
        .unwrap_or_else(|| panic!("{} leads the emoji tier but is absent", leader));
    assert_eq!(
        covered(&font, &emoji),
        emoji.len(),
        "{} leads the emoji tier but is missing some of {emoji:04X?}",
        leader
    );
}

#[test]
fn the_last_resort_is_installed_and_covers_basic_latin() {
    // The Common key's first family is Chrome's last resort, which cannot be
    // missing.
    let fonts = SystemFonts::new();
    let latin: Vec<u32> = (0x41..=0x5A).chain(0x61..=0x7A).collect();
    let common = names(&run(Script::COMMON, None));
    let last_resort = common.first().expect("a Common key");
    let font = fonts
        .regular(last_resort)
        .unwrap_or_else(|| panic!("{last_resort} is the last resort but is not installed"));
    assert_eq!(
        covered(&font, &latin),
        latin.len(),
        "{last_resort} is a last resort but lacks basic Latin"
    );
}

// ---------------------------------------------------------------------------
// Common and Inherited
// ---------------------------------------------------------------------------

/// The installed fonts of a run's key and the Common key, in order.
fn installed_chain(fonts: &SystemFonts, query: &FallbackRequest) -> Vec<(String, IDWriteFont)> {
    let mut names = names(query);
    names.extend(common_names(query));
    names
        .iter()
        .filter_map(|f| fonts.regular(f).map(|font| (f.to_string(), font)))
        .collect()
}

/// The Common key's names in `query`'s language.
fn common_names(query: &FallbackRequest) -> Vec<String> {
    let language = match *query {
        FallbackRequest::Text { language, .. } => language,
        _ => None,
    };
    names(&FallbackRequest::Text {
        script: Script::COMMON,
        language,
        generic: GenericClass::Plain,
    })
}

/// Codepoints no font in the chain can draw.
fn uncovered(chain: &[(String, IDWriteFont)], points: &[u32]) -> Vec<u32> {
    points
        .iter()
        .copied()
        .filter(|&c| {
            !chain
                .iter()
                .any(|(_, font)| unsafe { font.HasCharacter(c) }.is_ok_and(|b| b.as_bool()))
        })
        .collect()
}

/// Collapses a sorted codepoint list into contiguous ranges.
fn ranges(points: &[u32]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for &c in points {
        match out.last_mut() {
            Some(last) if last.1 + 1 == c => last.1 = c,
            _ => out.push((c, c)),
        }
    }
    out
}

/// Common and Inherited codepoints, grouped by the scripts that
/// Script_Extensions says actually use them.
///
/// UAX #24: a Common or Inherited character takes the script of its context.
/// `scx` is the machine-readable form of "which contexts" — U+0964 DANDA is
/// `Zyyy`, but its `scx` is every Indic script, because that is where it
/// occurs. So for script S, the chain for S has to be able to draw every
/// Common/Inherited codepoint whose `scx` contains S.
fn shared_by_script() -> BTreeMap<&'static str, Vec<u32>> {
    let scripts = CodePointMapData::<IcuScript>::new();
    let categories = CodePointMapData::<GeneralCategory>::new();
    let scx = ScriptWithExtensions::new();

    let mut out: BTreeMap<&'static str, Vec<u32>> = BTreeMap::new();
    for ch in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
        if !matches!(scripts.get(ch).short_name(), "Zyyy" | "Zinh") {
            continue;
        }
        if matches!(
            categories.get(ch),
            GeneralCategory::Unassigned
                | GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
                | GeneralCategory::PrivateUse
                | GeneralCategory::SpaceSeparator
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        ) {
            continue;
        }
        // Variation selectors are consumed by the shaper, never mapped through
        // cmap, so font coverage of them means nothing.
        if matches!(ch as u32, 0xFE00..=0xFE0F | 0xE0100..=0xE01EF) {
            continue;
        }
        for script in scx.get_script_extensions_val(ch).iter() {
            out.entry(script.short_name()).or_default().push(ch as u32);
        }
    }
    out
}

#[test]
fn shared_punctuation_and_marks_report() {
    let fonts = SystemFonts::new();
    println!(
        "
{:<6} {:>7} {:>8}  gaps",
        "script", "shared", "covered"
    );
    println!("{}", "-".repeat(56));
    for (name, points) in &shared_by_script() {
        let Ok(script) = Script::parse(name) else {
            continue;
        };
        let chain = installed_chain(&fonts, &run(script, None));
        let gaps = uncovered(&chain, points);
        if gaps.is_empty() {
            continue;
        }
        println!(
            "{name:<6} {:>7} {:>8}  {}",
            points.len(),
            points.len() - gaps.len(),
            ranges(&gaps)
                .iter()
                .take(6)
                .map(|(lo, hi)| if lo == hi {
                    format!("U+{lo:04X}")
                } else {
                    format!("U+{lo:04X}..{hi:04X}")
                })
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
}

/// Every renderable Common or Inherited codepoint, regardless of which script
/// it belongs to.
fn all_common_and_inherited() -> (Vec<u32>, Vec<u32>) {
    let scripts = CodePointMapData::<IcuScript>::new();
    let categories = CodePointMapData::<GeneralCategory>::new();
    let mut common = Vec::new();
    let mut inherited = Vec::new();
    for ch in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
        if matches!(
            categories.get(ch),
            GeneralCategory::Unassigned
                | GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::Surrogate
                | GeneralCategory::PrivateUse
                | GeneralCategory::SpaceSeparator
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        ) || matches!(ch as u32, 0xFE00..=0xFE0F | 0xE0100..=0xE01EF)
        {
            continue;
        }
        match scripts.get(ch).short_name() {
            "Zyyy" => common.push(ch as u32),
            "Zinh" => inherited.push(ch as u32),
            _ => {}
        }
    }
    (common, inherited)
}

/// How much of Common and Inherited a chain reaches when the query carries no
/// script at all — the worst case, and the one a caller hits if it does not
/// itemize.
#[test]
fn unscripted_query_coverage_report() {
    let fonts = SystemFonts::new();
    let (common, inherited) = all_common_and_inherited();
    for (label, query) in [
        ("Zyyy, no locale", run(Script::COMMON, None)),
        ("Zyyy, ja-JP", run(Script::COMMON, Some("ja-JP"))),
        ("Zyyy, hi-IN", run(Script::COMMON, Some("hi-IN"))),
    ] {
        let chain = installed_chain(&fonts, &query);
        let c = common.len() - uncovered(&chain, &common).len();
        let i = inherited.len() - uncovered(&chain, &inherited).len();
        println!(
            "{label:<18} {} families   Zyyy {c}/{} ({}%)   Zinh {i}/{} ({}%)",
            chain.len(),
            common.len(),
            100 * c / common.len(),
            inherited.len(),
            100 * i / inherited.len(),
        );
    }
}

/// Shared characters a chain for the given script must be able to draw, each
/// attached to that script by Script_Extensions.
///
/// All are Common or Inherited by the Script property, so a naive script-keyed
/// lookup misses them, yet they belong unambiguously to the listed script in
/// use. These are what show up as tofu in real text.
#[cfg(feature = "system")]
const SHARED: &[(&str, u32, &str)] = &[
    ("Deva", 0x0964, "danda"),
    ("Beng", 0x0964, "danda"),
    ("Taml", 0x0964, "danda"),
    ("Deva", 0x0951, "vedic tone udatta"),
    ("Jpan", 0x3099, "combining voiced sound mark"),
    ("Jpan", 0x3001, "ideographic comma"),
    ("Hans", 0x3001, "ideographic comma"),
    ("Kore", 0x3002, "ideographic full stop"),
    ("Arab", 0x060C, "arabic comma"),
    ("Latn", 0x0301, "combining acute"),
    ("Cyrl", 0x0301, "combining acute"),
    ("Grek", 0x0345, "combining ypogegrammeni"),
    ("Mong", 0x1802, "mongolian comma"),
    ("Lisu", 0x300A, "left double angle bracket"),
    // Scripts with no font of their own at all. Their own letters are tofu
    // here, but the shared punctuation still has to come from somewhere —
    // partial rendering beats a line of boxes. The only possible route is the
    // walk of every installed family past the keys', so these guard it.
    ("Sylo", 0x0964, "danda, via the tail"),
    (
        "Modi",
        0xA830,
        "north indic fraction one quarter, via the tail",
    ),
];

/// Required in practice, but *not* attested by Script_Extensions.
///
/// Unicode gives the fullwidth forms no script extensions at all — their `scx`
/// is bare `Zyyy` — yet they occur only in CJK text and only CJK fonts carry
/// them. Chrome patches over this with an explicit codepoint range, forcing
/// `0xFF00 < c < 0xFF5F` to Han before it looks anything up, and so does a
/// missed character's key; this pins that they stay reachable.
#[cfg(feature = "system")]
const SHARED_BY_CONVENTION: &[(&str, u32, &str)] = &[
    ("Hant", 0xFF0C, "fullwidth comma"),
    ("Hans", 0xFF1F, "fullwidth question mark"),
    ("Jpan", 0xFF01, "fullwidth exclamation mark"),
];

/// The Script property values behind a CLDR combined code such as `Jpan`,
/// which has no Script value of its own.
#[cfg(feature = "system")]
fn components(name: &str) -> Vec<&str> {
    match name {
        "Jpan" => vec!["Hira", "Kana", "Hani"],
        "Hans" | "Hant" => vec!["Hani"],
        "Kore" => vec!["Hang", "Hani"],
        other => vec![other],
    }
}

#[cfg(feature = "system")]
#[test]
fn shared_characters_are_reachable_from_their_scripts_run() {
    let scripts = CodePointMapData::<IcuScript>::new();
    let scx = ScriptWithExtensions::new();
    let parser = icu_properties::PropertyParser::<IcuScript>::new();
    let collection = Collection::system();
    let mut failures = Vec::new();

    for &(name, cp, what) in SHARED.iter().chain(SHARED_BY_CONVENTION) {
        let ch = char::from_u32(cp).unwrap();

        // Guard the premise. If Unicode stops calling this Common or
        // Inherited, the entry is stale and belongs in the ordinary script
        // tables instead — the test should say so rather than quietly
        // checking nothing.
        let actual = scripts.get(ch).short_name();
        assert!(
            matches!(actual, "Zyyy" | "Zinh"),
            "U+{cp:04X} ({what}) is Script={actual}, not Common or Inherited"
        );
        // The same guard on the attachment, for the entries that claim one.
        if SHARED.iter().any(|e| e.1 == cp && e.0 == name) {
            assert!(
                components(name)
                    .iter()
                    .any(|c| parser.get_strict(c).is_some_and(|s| scx.has_script(ch, s))),
                "U+{cp:04X} ({what}) is no longer attached to {name} by Script_Extensions; move it to SHARED_BY_CONVENTION or drop it"
            );
        }

        // The run's families, then the walk a miss makes.
        let request = run(Script::parse(name).unwrap(), None);
        let reached = collection
            .fallback(&collection.key(&request))
            .iter()
            .cloned()
            .chain(collection.char_fallback(ch, Presentation::Text, &request))
            .any(|family| family.covers(ch));
        if !reached {
            failures.push(format!(
                "{name}: U+{cp:04X} {what} — no installed family has it"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "shared characters unreachable from their script's run:
  {}

         These are Common or Inherited codepoints belonging to a specific          script. If the font that carries one is simply not installed here,          that is a gap in this machine's coverage rather than a bug in the          tables.",
        failures.join("
  ")
    );
}

#[cfg(feature = "system")]
#[test]
fn local_finds_an_installed_font_by_full_or_postscript_name() {
    use fontwich::{Collection, FaceDescriptors, FontWeight, LayerBuilder, Role, Source};

    let collection = Collection::system();
    let file = |name: &str| {
        let font = collection.local(name)?;
        let Source::Path(path) = font.source() else {
            return None;
        };
        Some((
            path.file_name()?.to_string_lossy().to_ascii_lowercase(),
            font.weight(),
        ))
    };
    let semibold = file("Segoe UI Semibold").expect("by full name");
    assert_eq!(semibold.0, "seguisb.ttf");
    assert_eq!(semibold.1, FontWeight::new(600.0));
    assert_eq!(file("SegoeUI-Semibold"), Some(semibold));
    assert_eq!(
        file("Arial Bold").map(|found| found.0).as_deref(),
        Some("arialbd.ttf")
    );
    assert_eq!(file("No Such Font Anywhere"), None);

    // As `src: local(...)` uses it: a face over the font found.
    let mut document = LayerBuilder::new(Role::Document);
    let font = collection.local("Arial Bold").expect("installed");
    let id = document.add_face_font(
        "Headline",
        FaceDescriptors {
            weight: Some((FontWeight::new(800.0), FontWeight::new(800.0))),
            ..FaceDescriptors::default()
        },
        font,
    );
    let family = document.family("Headline").expect("declared");
    let face = &family.fonts()[0];
    assert_eq!(face.face_id(), Some(id));
    assert_eq!(face.weight(), FontWeight::new(800.0));
    assert!(face.charset().contains('A'));
    assert!(face.load().is_some());
}

#[cfg(feature = "system")]
#[test]
fn a_family_is_found_by_its_older_name_with_only_its_fonts() {
    // DirectWrite files Arial Black and Arial Narrow under "Arial", and
    // Segoe UI Semibold under "Segoe UI"; a page asks for them by these
    // names, and Chrome draws those fonts alone.
    let collection = fontwich::Collection::system();
    let weights = |name: &str| {
        let family = collection.family(name)?;
        let mut weights: Vec<(u16, u16)> = family
            .fonts()
            .iter()
            .map(|font| {
                (
                    font.weight().value() as u16,
                    (font.width().ratio() * 100.0) as u16,
                )
            })
            .collect();
        weights.sort();
        Some(weights)
    };
    assert_eq!(weights("Arial Black"), Some(vec![(900, 100)]));
    assert_eq!(
        weights("Arial Narrow"),
        Some(vec![(400, 75), (400, 75), (700, 75), (700, 75)])
    );
    assert_eq!(
        weights("Segoe UI Semibold"),
        Some(vec![(600, 100), (600, 100)])
    );
    // Every name listed has fonts behind it: Windows' downloadable fonts,
    // which have names and no files, are not listed.
    let empty = collection
        .layers()
        .flat_map(|layer| layer.names().map(String::from).collect::<Vec<_>>())
        .filter(|name| {
            collection
                .family(name)
                .is_some_and(|family| family.fonts().is_empty())
        })
        .count();
    assert_eq!(empty, 0);
}

#[cfg(feature = "system")]
#[test]
fn math_goes_to_cambria_math_and_symbols_to_segoe_ui_symbol() {
    // A missed character's key: the math alphanumerics ask the math key,
    // Cambria Math first, and the symbol blocks the symbol key, Segoe UI
    // Symbol. Chrome sends arrows and the math operators to Cambria Math by
    // a block table of its own; here they are symbols.
    use fontwich::{Attributes, FontFamilyName, GenericFamily};

    let collection = Collection::system();
    let request = run(Script::from_bytes(*b"Latn"), Some("en"));
    let sans = collection.resolve(&FontFamilyName::Generic(GenericFamily::SansSerif), &request);
    let run_families = collection.fallback(&collection.key(&request));
    let serves = |c: char| {
        let maps = |family: &fontwich::Family| {
            family
                .match_font(Attributes::default(), true)
                .is_some_and(|font| font.charset().contains(c))
        };
        sans.iter()
            .cloned()
            .chain(run_families.iter().cloned())
            .chain(collection.char_fallback(c, Presentation::Text, &request))
            .find(|family| maps(family))
            .map(|family| family.name().to_owned())
    };
    for c in ['𝐀', '𝑥', '𝔸'] {
        assert_eq!(serves(c).as_deref(), Some("Cambria Math"), "{c}");
    }
    for c in ['★', '☀', '✈', '⌀', '⊕', '⤀'] {
        assert_eq!(serves(c).as_deref(), Some("Segoe UI Symbol"), "{c}");
    }
    // What the primary font maps stays its own.
    assert_eq!(serves('∑').as_deref(), Some("Arial"));
}

#[cfg(feature = "system")]
#[test]
fn a_variable_font_s_named_instances_are_not_families() {
    // A Win32 family holds four styles, so a font that does not fit gets a
    // family of its own — Arial Black — and a page asks for it by that name.
    // DirectWrite names a variable font's instances the same way, but
    // "Bahnschrift SemiBold" is a point on an axis, not a font: nothing in
    // the file is called that, Firefox does not resolve it, and Chrome only
    // resolves some of them.
    let collection = fontwich::Collection::system();
    let fonts = |name: &str| {
        collection
            .family(name)
            .map(|family| family.fonts().len())
            .unwrap_or(0)
    };
    if fonts("Bahnschrift") == 0 {
        return; // Not installed; nothing to judge.
    }
    for instance in [
        "Bahnschrift SemiBold",
        "Bahnschrift Light",
        "Bahnschrift Condensed",
        "Bahnschrift SemiLight Condensed",
    ] {
        assert_eq!(
            fonts(instance),
            0,
            "{instance} is an instance, not a family"
        );
    }
    // The font itself stays, with the axes that reach those instances.
    let bahnschrift = collection.family("Bahnschrift").expect("installed");
    let font = &bahnschrift.fonts()[0];
    assert!(font.axis(b"wght").is_some(), "its weight axis");
    assert!(font.axis(b"wdth").is_some(), "its width axis");
}

#[cfg(feature = "system")]
#[test]
fn local_holds_a_named_instance_where_its_name_points() {
    // CSS matches `local()` against a full font name or a PostScript name,
    // and a variable font's named instance has both. The name means that
    // point of the font: "Bahnschrift SemiBold" is its weight axis at 600,
    // not Bahnschrift at its default. Chrome resolves the name and then
    // draws the default, which is the name saying one thing and the glyphs
    // another.
    use fontwich::Attributes;

    let collection = fontwich::Collection::system();
    let Some(semibold) = collection.local("Bahnschrift SemiBold") else {
        return; // Not installed.
    };
    let settings = |font: &fontwich::Font| {
        let synthesis = font.synthesis(Attributes::default());
        let mut settings: Vec<([u8; 4], f32)> = synthesis
            .variation_settings()
            .iter()
            .map(|setting| (setting.tag.to_bytes(), setting.value))
            .collect();
        settings.sort_by_key(|(tag, _)| *tag);
        settings
    };
    assert_eq!(semibold.weight().value(), 600.0);
    assert_eq!(settings(&semibold), [(*b"wght", 600.0)]);

    let condensed = collection
        .local("Bahnschrift Condensed")
        .expect("installed");
    assert_eq!((condensed.width().ratio() * 100.0).round(), 75.0);
    // Its weight is the axis's own default, so there is nothing to set.
    assert_eq!(settings(&condensed), [(*b"wdth", 75.0)]);

    // A full name that is the font's own leaves it whole: a rule declaring a
    // weight range over `local()` needs the axis it varies along.
    let whole = collection.local("Bahnschrift").expect("installed");
    let weight = whole.axis(b"wght").expect("a weight axis");
    assert!(weight.min < weight.max, "{weight:?}");
    assert_eq!(settings(&whole), []);
}
