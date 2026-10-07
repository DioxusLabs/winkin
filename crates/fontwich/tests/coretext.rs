//! Checks the macOS backend against the fonts actually installed on this
//! machine, via Core Text.
//!
//! Unlike the Windows tables, there is nothing here to regenerate when Apple
//! ships a new OS: the backend asks Core Text directly, so what this checks
//! is the *plumbing* — that a family name it produced resolves and can draw
//! what it was offered for — not a hand-maintained table's accuracy.
//!
//! These checks require the bundled families used by the backend. Unsupported
//! generics are checked separately from installed-family resolution.

#![cfg(all(target_vendor = "apple", feature = "system"))]

use std::ptr::NonNull;

use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_text::{CTFont, kCTFontTableCFF, kCTFontTableCFF2, kCTFontTableGlyf};

use fontwich::backend::{Backend, MacOs};
use fontwich::{Collection, FallbackRequest, GenericClass, GenericFamily, Presentation, Script};

/// The names `backend` gives `request`'s key, installed or not.
fn names(backend: &Backend, request: &FallbackRequest) -> Vec<String> {
    let mut names = Vec::new();
    backend.families(&Collection::new().key(request), |name| {
        names.push(String::from(name))
    });
    names
}

fn macos() -> Backend {
    Backend::platform()
}

/// The regular font of `family`, or `None` if the name does not resolve.
///
/// `CTFontCreateWithName` never fails outright — an unknown name comes back
/// as some substitute — so the only way to tell a real match from a
/// substitution is to check the family name we get back.
fn regular(family: &str) -> Option<CFRetained<CTFont>> {
    unsafe {
        let name = CFString::from_str(family);
        let font = CTFont::with_name(&name, 12.0, core::ptr::null());
        font.family_name()
            .to_string()
            .eq_ignore_ascii_case(family)
            .then_some(font)
    }
}

/// Whether `font` has a glyph for `ch`.
fn covers(font: &CTFont, ch: char) -> bool {
    let mut units = [0u16; 2];
    let units = ch.encode_utf16(&mut units);
    let mut glyphs = [0u16; 2];
    unsafe {
        font.glyphs_for_characters(
            NonNull::new(units.as_mut_ptr()).unwrap(),
            NonNull::new(glyphs.as_mut_ptr()).unwrap(),
            units.len() as isize,
        );
    }
    // Non-BMP characters are sparse: the glyph for the full character lands
    // on the first UTF-16 unit, and the second is *always* 0 by design, not
    // a sign of missing coverage.
    glyphs[0] != 0
}

/// A run of `script` in `lang`.
fn text(script: &str, lang: Option<&str>) -> FallbackRequest {
    FallbackRequest::Text {
        script: Script::parse(script).unwrap(),
        language: lang.and_then(fontwich::parse_language),
        generic: GenericClass::Plain,
    }
}

/// `generic` resolved in `lang`.
fn generic(generic: GenericFamily, lang: Option<&str>) -> FallbackRequest {
    FallbackRequest::Generic(generic, lang.and_then(fontwich::parse_language))
}

/// Whether `font` has glyph outlines an open-source renderer (FreeType,
/// skrifa, HarfBuzz's own rasterizer) can actually draw — `glyf`, `CFF ` or
/// `CFF2`. A font with none of these, only Apple's proprietary `hvgl`, draws
/// nothing outside Core Text.
fn has_open_outlines(font: &CTFont) -> bool {
    unsafe {
        font.has_table(kCTFontTableGlyf)
            || font.has_table(kCTFontTableCFF)
            || font.has_table(kCTFontTableCFF2)
    }
}

/// The Common key's last resort: its last three names.
fn last_resort() -> Vec<String> {
    let common = names(&macos(), &text("Zyyy", None));
    common[common.len().saturating_sub(3)..].to_vec()
}

#[test]
fn last_resort_families_are_installed() {
    let last_resort = last_resort();
    assert!(!last_resort.is_empty(), "no last resort at all");
    for family in &last_resort {
        assert!(
            regular(family).is_some(),
            "{family} is a last resort but is not installed"
        );
    }
}

#[test]
fn last_resort_families_cover_basic_latin() {
    let latin: Vec<char> = ('A'..='Z').chain('a'..='z').collect();
    for candidate in last_resort() {
        let font = regular(&candidate).expect("checked installed above");
        for &ch in &latin {
            assert!(
                covers(&font, ch),
                "{candidate} is a last resort but lacks '{ch}'"
            );
        }
    }
}

#[test]
fn the_emoji_key_can_actually_draw_emoji() {
    let emoji = [
        '\u{1F600}',
        '\u{1F44D}',
        '\u{2764}',
        '\u{1F1FA}',
        '\u{1F680}',
    ];
    let names = names(&macos(), &FallbackRequest::Emoji(Presentation::Emoji));
    let leader = names.first().expect("an emoji font");
    let font =
        regular(leader).unwrap_or_else(|| panic!("{leader} leads the emoji key but is absent"));
    for ch in emoji {
        assert!(
            covers(&font, ch),
            "{leader} leads the emoji key but is missing U+{:04X}",
            ch as u32
        );
    }
}

#[test]
fn supported_generic_families_resolve() {
    // Check the supported generic defaults against installed Core Text families.
    let cases = [
        ("Zyyy", None, GenericFamily::SansSerif),
        ("Hans", Some("zh-CN"), GenericFamily::SansSerif),
        ("Hant", Some("zh-TW"), GenericFamily::SansSerif),
        ("Jpan", Some("ja"), GenericFamily::SansSerif),
        ("Kore", Some("ko"), GenericFamily::SansSerif),
        ("Zyyy", None, GenericFamily::Serif),
        ("Hans", Some("zh-CN"), GenericFamily::Serif),
        ("Hant", Some("zh-TW"), GenericFamily::Serif),
        ("Jpan", Some("ja"), GenericFamily::Serif),
        ("Kore", Some("ko"), GenericFamily::Serif),
        ("Zyyy", None, GenericFamily::Monospace),
        ("Zyyy", None, GenericFamily::Cursive),
        ("Zyyy", None, GenericFamily::Fantasy),
        ("Zyyy", None, GenericFamily::SystemUi),
        // Not `GenericFamily::Emoji`: it keys as emoji — see
        // `the_emoji_key_can_actually_draw_emoji`.
        ("Zyyy", None, GenericFamily::Math),
    ];

    let mut missing = Vec::new();
    for (script, lang, family) in cases {
        let primary = names(&macos(), &generic(family, lang));
        assert!(
            !primary.is_empty(),
            "{family:?}/{script}: no generic answer at all"
        );
        if !primary.iter().any(|family| regular(family).is_some()) {
            missing.push(format!(
                "{family:?}/{script}: none of {primary:?} are installed"
            ));
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[test]
fn default_backend_avoids_hvgl_only_chinese_fonts() {
    // PingFang SC/TC ship, on current macOS, with glyph outlines only in
    // Apple's proprietary `hvgl` table — verified below, not assumed. No
    // open-source renderer can draw that, so the default backend (unlike
    // `.allowing_hvgl_only_fonts()`) must neither offer PingFang nor lead
    // with a substitute that turns out to have the same problem.
    for (script, lang, substitute) in [("Hans", "zh-CN", "Heiti SC"), ("Hant", "zh-TW", "Heiti TC")]
    {
        let mut names = names(&macos(), &generic(GenericFamily::SansSerif, Some(lang)));
        names.extend(self::names(&macos(), &text(script, Some(lang))));
        assert!(
            !names.iter().any(|f| f.starts_with("PingFang")),
            "{script}: default chain offers PingFang: {names:?}"
        );
        assert!(
            names.iter().any(|name| name == substitute),
            "{script}: {substitute} missing from {names:?}"
        );
        let font = regular(substitute).unwrap_or_else(|| panic!("{substitute} is not installed"));
        assert!(
            has_open_outlines(&font),
            "{substitute} has no glyf/CFF/CFF2 either"
        );
    }
}

#[test]
fn allowing_hvgl_only_fonts_restores_pingfang() {
    let source = Backend::Apple(MacOs::new().allowing_hvgl_only_fonts());
    for (script, lang, pingfang) in [
        ("Hans", "zh-CN", "PingFang SC"),
        ("Hant", "zh-TW", "PingFang TC"),
    ] {
        let names = names(&source, &generic(GenericFamily::SansSerif, Some(lang)));
        let _ = script;
        assert!(
            names.iter().any(|f| f == pingfang),
            "{script}: opted into hvgl-only fonts but {pingfang} is still missing"
        );
    }
}

#[test]
fn pingfang_is_still_hvgl_only() {
    // A regression guard in the other direction: if a future macOS ships
    // PingFang with real outlines again, `apple::prefs` should go back to
    // naming it unconditionally instead of carrying this workaround forever.
    for family in ["PingFang SC", "PingFang TC"] {
        let Some(font) = regular(family) else {
            continue;
        };
        assert!(
            !has_open_outlines(&font),
            "{family} now has glyf/CFF/CFF2 outlines — the Apple backend's \
             `is_hvgl_only` no longer needs to special-case it"
        );
    }
}

#[test]
fn a_script_with_a_dedicated_font_reaches_it_before_anything_can_draw_it() {
    // Confirms the actual bug this module exists to fix: a Serif or
    // SansSerif query for a script the Latin-default font can't draw
    // properly used to fall through to Helvetica Neue/Times New Roman and
    // never name the real font at all.
    //
    // The dedicated font no longer *leads*: `font-family: sans-serif` asks
    // for Helvetica, so Helvetica goes in front of it, and the script's
    // font is there for the letters Helvetica lacks. That is what Chrome
    // does and it is worth 43 characters of the browser comparison. What
    // makes it safe is the second assertion, which is the stronger claim
    // and the one worth pinning: nothing ahead of the dedicated font draws
    // a single character of the script, so leading with the generic's own
    // font cannot take the letters away from it.
    let cases = [
        ("Arab", GenericFamily::SansSerif, "Geeza Pro", 'م'),
        ("Hebr", GenericFamily::SansSerif, "Lucida Grande", 'ש'),
        ("Thai", GenericFamily::SansSerif, "Thonburi", 'ส'),
        ("Deva", GenericFamily::SansSerif, "Kohinoor Devanagari", 'न'),
        ("Armn", GenericFamily::SansSerif, "Noto Sans Armenian", 'Բ'),
        ("Khmr", GenericFamily::SansSerif, "Khmer Sangam MN", 'ស'),
    ];
    for (script, generic, expected, letter) in cases {
        let _ = generic;
        let primary = names(&macos(), &text(script, None));
        assert!(
            primary.iter().any(|f| f == expected),
            "{script}/{generic:?}: the key's answer is {primary:?}, without the dedicated font"
        );
        let at = primary
            .iter()
            .position(|f| f == expected)
            .expect("just checked");
        for ahead in &primary[..at] {
            let Some(font) = regular(ahead) else { continue };
            assert!(
                !covers(&font, letter),
                "{script}/{generic:?}: {ahead} is ahead of {expected} and draws {letter}"
            );
        }
    }
}

#[test]
fn script_tier_is_nonempty_for_common_scripts() {
    // Every script here has real, everyday text behind it. If Core Text's
    // cascade ever comes back empty for one of these, something upstream
    // broke — a bad language tag, a cascade call failing outright — not a
    // missing table entry, since there are no tables.
    let scripts = [
        "Latn", "Cyrl", "Grek", "Arab", "Hebr", "Thai", "Deva", "Beng", "Taml", "Telu", "Knda",
        "Mlym", "Sinh", "Khmr", "Laoo", "Mymr", "Geor", "Armn", "Hans", "Hant", "Jpan", "Kore",
    ];
    let empty: Vec<_> = scripts
        .iter()
        .filter(|s| names(&macos(), &text(s, None)).is_empty())
        .collect();
    assert!(empty.is_empty(), "no script-tier answer for: {empty:?}");
}

#[test]
fn coverage_report() {
    // Not an assertion — a printed snapshot of what the cascade actually
    // leads with today, for a human to sanity-check with `cargo test
    // coverage_report -- --nocapture`.
    for (script, lang) in [
        ("Hans", Some("zh-CN")),
        ("Hant", Some("zh-TW")),
        ("Jpan", Some("ja")),
        ("Kore", Some("ko")),
        ("Deva", Some("hi-IN")),
        ("Arab", Some("ar-EG")),
        ("Thai", Some("th")),
    ] {
        let lead: Vec<_> = names(&macos(), &text(script, lang))
            .into_iter()
            .take(5)
            .collect();
        println!("{script:<6} {lang:?} -> {}", lead.join(", "));
    }
}

#[test]
fn unsupported_fangsong_generic_has_no_family() {
    assert!(names(&macos(), &generic(GenericFamily::FangSong, None)).is_empty());
}

#[test]
fn character_fallback_reaches_loadable_fonts() {
    let collection = Collection::system();
    for (script, language, letter) in [
        ("Latn", "en", 'A'),
        ("Arab", "ar", 'م'),
        ("Hebr", "he", 'ש'),
        ("Deva", "hi", 'न'),
        ("Thai", "th", 'ส'),
        ("Hans", "zh-CN", '汉'),
        ("Hant", "zh-TW", '漢'),
        ("Jpan", "ja", 'あ'),
        ("Kore", "ko", '한'),
    ] {
        let request = text(script, Some(language));
        let found = collection
            .char_fallback(letter, Presentation::Text, &request)
            .flat_map(|family| family.fonts().to_vec())
            .find(|font| font.charset().contains(letter));
        let font = found.unwrap_or_else(|| panic!("no fallback maps {script}: {letter}"));
        assert!(font.load().is_some(), "{script}: fallback font cannot load");
    }
}
