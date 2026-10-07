use alloc::vec::Vec;

use parlance::{Language, Script};

use crate::fallback::classify::*;
use crate::fallback::emoji::Presentation;
use crate::fallback::key::{
    BackendFacts, FallbackKey, FallbackRequest, GenericClass, Han, parse_language,
};

/// Chrome's Windows fallback: monospace Arabic and Hebrew.
const WINDOWS: BackendFacts = BackendFacts {
    reads_serif: false,
    reads_monospace: true,
    per_language: false,
    reads_language: false,
};

/// fontconfig: one list per language.
const LINUX: BackendFacts = BackendFacts {
    reads_serif: false,
    reads_monospace: false,
    per_language: true,
    reads_language: true,
};

/// Skia on Android: one list per language, with serif fallbacks.
const ANDROID: BackendFacts = BackendFacts {
    reads_serif: true,
    reads_monospace: false,
    per_language: true,
    reads_language: false,
};

const EVERY_PRESENTATION: [Presentation; 4] = [
    Presentation::Text,
    Presentation::Emoji,
    Presentation::TextOnly,
    Presentation::EmojiOnly,
];

/// The probes' languages, none included.
const LANGUAGES: [Option<&str>; 10] = [
    None,
    Some("en"),
    Some("ja"),
    Some("zh-CN"),
    Some("zh-TW"),
    Some("zh-HK"),
    Some("ko"),
    Some("ar"),
    Some("hi"),
    Some("fa"),
];

/// One case of the grid: a language, a generic and the layers' facts.
#[derive(Copy, Clone, Debug)]
struct Page {
    language: Option<&'static str>,
    generic: GenericClass,
    facts: BackendFacts,
}

impl Page {
    fn new(language: Option<&'static str>, facts: BackendFacts) -> Self {
        Self {
            language,
            generic: GenericClass::Plain,
            facts,
        }
    }

    fn lang(self) -> Option<Language> {
        self.language.and_then(parse_language)
    }

    /// The key `c` asks in `presentation`.
    fn asks(self, c: char, presentation: Presentation) -> Option<FallbackKey> {
        key(c, presentation, self.lang(), self.generic, self.facts)
    }

    /// A script's key on this page.
    fn script(self, tag: &[u8; 4]) -> FallbackKey {
        FallbackKey::new(
            &FallbackRequest::Text {
                script: Script::from_bytes(*tag),
                language: self.lang(),
                generic: self.generic,
            },
            self.facts,
        )
    }

    /// The Han key on this page: its language's tradition, else Simplified.
    fn han(self) -> FallbackKey {
        self.script(b"Hani")
    }

    fn emoji(self, presentation: Presentation) -> FallbackKey {
        FallbackKey::new(&FallbackRequest::Emoji(presentation), self.facts)
    }
}

/// Every page of the grid: each language on each kind of backend.
fn pages() -> impl Iterator<Item = Page> {
    LANGUAGES.into_iter().flat_map(|language| {
        [WINDOWS, LINUX, ANDROID]
            .into_iter()
            .map(move |facts| Page::new(language, facts))
    })
}

/// Asserts that every character of `text` asks `want` on `page` in every
/// presentation in `presentations`.
fn check(page: Page, text: &str, presentations: &[Presentation], want: Option<FallbackKey>) {
    for c in text.chars() {
        for &presentation in presentations {
            assert_eq!(
                page.asks(c, presentation),
                want,
                "{c:?} U+{:04X} in {presentation:?} on {page:?}",
                u32::from(c)
            );
        }
    }
}

fn tradition(language: Option<&str>) -> Han {
    match language {
        Some("ja") => Han::Jpan,
        Some("zh-TW") => Han::Hant,
        Some("zh-HK") => Han::HantHK,
        Some("ko") => Han::Kore,
        _ => Han::Hans,
    }
}

#[test]
fn cjk_punctuation_and_fullwidth_ask_han_in_the_page_tradition() {
    for page in pages() {
        let han = page.han();
        assert_eq!(han.han(), Some(tradition(page.language)), "{page:?}");
        // By Script_Extensions: CJK punctuation, the katakana middle dot
        // (Chrome: the Japanese font) and the square and parenthesized
        // forms that extend to Han (Chrome: MS PGothic on Windows).
        check(
            page,
            "。、【】〔〕「」・㍿㈱",
            &EVERY_PRESENTATION,
            Some(han),
        );
        // The fullwidth forms, the Latin ones included.
        check(page, "！（）Ａ１～", &EVERY_PRESENTATION, Some(han));
        // Han itself, the unification set included.
        check(page, "漢字直骨次今角戸", &EVERY_PRESENTATION, Some(han));
    }
}

#[test]
fn kana_asks_japanese_whatever_the_language() {
    for page in pages() {
        let japanese = page.script(b"Jpan");
        assert_eq!(japanese.han(), Some(Han::Jpan));
        // `ー` extends only to kana; `㌔` is Katakana.
        check(page, "ーカな㌔", &EVERY_PRESENTATION, Some(japanese));
    }
}

#[test]
fn hangul_asks_korean_whatever_the_language() {
    for page in pages() {
        let korean = page.script(b"Hang");
        assert_eq!(korean.han(), Some(Han::Kore));
        check(page, "한국어", &EVERY_PRESENTATION, Some(korean));
    }
}

#[test]
fn a_script_asks_its_own_key() {
    for page in pages() {
        for (text, tag) in [
            ("ภาษาไทย", b"Thai"),
            ("हिन्दी", b"Deva"),
            ("עברית", b"Hebr"),
            ("عربي", b"Arab"),
            ("ኢትዮጵያ", b"Ethi"),
            // The tsheg is Tibetan, not Common.
            ("བོད་ཡིག", b"Tibt"),
            ("ᠮᠣᠩᠭᠣᠯ", b"Mong"),
            ("⠁", b"Brai"),
        ] {
            check(page, text, &EVERY_PRESENTATION, Some(page.script(tag)));
        }
    }
}

#[test]
fn shared_characters_ask_one_of_their_scripts() {
    for page in pages() {
        // The danda: Devanagari among a score of scripts.
        check(page, "।॥", &EVERY_PRESENTATION, Some(page.script(b"Deva")));
        // The Arabic comma: Arabic, the first of its extensions.
        check(page, "،؛؟", &EVERY_PRESENTATION, Some(page.script(b"Arab")));
        // A lone combining acute: Latin, of eight (Chrome on Windows:
        // Lucida Sans Unicode through the Common key).
        check(
            page,
            "\u{0301}ʼ",
            &EVERY_PRESENTATION,
            Some(page.script(b"Latn")),
        );
    }
}

#[test]
fn monospace_arabic_and_hebrew_keep_their_class_on_windows() {
    let page = Page {
        generic: GenericClass::Monospace,
        ..Page::new(Some("ja"), WINDOWS)
    };
    for (c, tag) in [('ب', b"Arab"), ('،', b"Arab"), ('א', b"Hebr")] {
        let want = page.script(tag);
        assert_eq!(want.script(), Some(Script::from_bytes(*tag)));
        assert_eq!(want.class(), GenericClass::Monospace);
        assert_eq!(page.asks(c, Presentation::Text), Some(want), "{c:?}");
    }
}

#[test]
fn emoji_symbols_ask_the_text_emoji_key_in_text() {
    for page in pages() {
        let text = page.emoji(Presentation::Text);
        let color = page.emoji(Presentation::Emoji);
        // `▶` and `↩` are in math blocks, and are emoji first, as Chrome's
        // `Character::IsEmoji` makes them.
        let symbols = "▶↩☺✈♠♥⬆";
        // Plain text and VS15 alike.
        check(
            page,
            symbols,
            &[Presentation::Text, Presentation::TextOnly],
            Some(text),
        );
        // VS16, or an emoji run, asks the color fonts.
        check(
            page,
            symbols,
            &[Presentation::Emoji, Presentation::EmojiOnly],
            Some(color),
        );
    }
}

#[test]
fn emoji_by_default_ask_the_color_key_unless_vs15() {
    for page in pages() {
        let color = page.emoji(Presentation::Emoji);
        // Each character of the sequences: the base, the skin tone and the
        // regional indicators.
        let emoji = "😀👍🏽🇯🇵";
        check(
            page,
            emoji,
            &[
                Presentation::Text,
                Presentation::Emoji,
                Presentation::EmojiOnly,
            ],
            Some(color),
        );
        check(
            page,
            emoji,
            &[Presentation::TextOnly],
            Some(page.emoji(Presentation::Text)),
        );
    }
}

#[test]
fn symbols_that_are_not_emoji_ignore_both_selectors() {
    for page in pages() {
        let symbols = page.script(b"Zsym");
        assert_eq!(symbols.script(), Some(Script::from_bytes(*b"Zsym")));
        // Miscellaneous Symbols and Dingbats, as in Chrome.
        check(page, "★☆✠", &EVERY_PRESENTATION, Some(symbols));
        // Arrows and math operators (Chrome on Windows: Cambria Math), the
        // circled digits (MS PGothic), letterlike symbols, control pictures
        // and box drawing (Lucida Sans Unicode).
        check(page, "→⇒∑≤∞∫①ℝ␀─│┌", &EVERY_PRESENTATION, Some(symbols));
        // A playing card, in the supplementary planes' symbols.
        check(page, "🂡", &EVERY_PRESENTATION, Some(symbols));
    }
}

#[test]
fn math_alphanumerics_ask_the_math_key() {
    for page in pages() {
        let math = page.script(b"Zmth");
        assert_eq!(math.script(), Some(Script::from_bytes(*b"Zmth")));
        check(page, "𝔸𝐀𝑥", &EVERY_PRESENTATION, Some(math));
    }
}

#[test]
fn the_rest_ask_nothing_of_their_own() {
    // The Common key answers these: on Windows `₹` is Tahoma's and `※`
    // Lucida Sans Unicode's through Chrome's catch-all lists.
    for page in pages() {
        check(page, "₹※䷀𝌆㎡", &EVERY_PRESENTATION, None);
    }
}

#[test]
fn a_per_language_script_key_carries_the_page_tradition() {
    let thai = Page::new(Some("zh-HK"), LINUX).asks('ภ', Presentation::Text);
    assert_eq!(thai.and_then(|key| key.han()), Some(Han::HantHK));
    assert_eq!(
        thai.and_then(|key| key.script()),
        Some(Script::from_bytes(*b"Thai"))
    );
}

#[test]
fn android_keeps_the_serif_class() {
    let page = Page {
        generic: GenericClass::Serif,
        ..Page::new(Some("hi"), ANDROID)
    };
    let asked = page.asks('ह', Presentation::Text);
    assert_eq!(asked, Some(page.script(b"Deva")));
    assert_eq!(asked.map(|key| key.class()), Some(GenericClass::Serif));
}

#[test]
fn no_character_asks_the_common_key_or_panics() {
    for facts in [WINDOWS, LINUX, ANDROID] {
        let page = Page {
            generic: GenericClass::Monospace,
            ..Page::new(Some("zh-HK"), facts)
        };
        let language = page.lang();
        let common = page.script(b"Zyyy");
        for c in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
            for presentation in EVERY_PRESENTATION {
                let asked = key(c, presentation, language, page.generic, facts);
                assert_ne!(asked, Some(common), "U+{:04X}", u32::from(c));
            }
        }
    }
}

#[test]
fn the_key_is_a_copy_value() {
    // Nothing on the heap: the answer is `Copy`, and a few bytes.
    fn copy<T: Copy>(_: T) {}
    let asked = key('。', Presentation::Text, None, GenericClass::Plain, WINDOWS);
    copy(asked);
    assert!(size_of_val(&asked) <= 6);
}

#[test]
fn the_emoji_table_still_matches_unicode() {
    assert!(EMOJI.windows(2).all(|pair| pair[0].1 < pair[1].0));
    assert_eq!(
        EMOJI,
        &generate()[..],
        "EMOJI has drifted; see print_tables"
    );
    for c in "▶↩☺✈♠♥⬆#1©".chars() {
        assert_eq!(emoji(u32::from(c)), Some(false), "{c:?}");
    }
    for c in "😀👍🏽🇯🇵".chars() {
        assert_eq!(emoji(u32::from(c)), Some(true), "{c:?}");
    }
    for c in "★☆→①✠ℝ".chars() {
        assert_eq!(emoji(u32::from(c)), None, "{c:?}");
    }
}

/// The Emoji property's ranges, each with whether it is emoji by default,
/// from Unicode as icu_properties has it.
fn generate() -> Vec<(u32, u32, bool)> {
    use icu_properties::CodePointSetData;
    use icu_properties::props::{Emoji, EmojiPresentation};
    let emoji = CodePointSetData::new::<Emoji>();
    let by_default = CodePointSetData::new::<EmojiPresentation>();
    let mut ranges: Vec<(u32, u32, bool)> = Vec::new();
    for c in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
        if !emoji.contains(c) {
            continue;
        }
        let (flag, c) = (by_default.contains(c), u32::from(c));
        match ranges.last_mut() {
            Some((_, end, last)) if *end + 1 == c && *last == flag => *end = c,
            _ => ranges.push((c, c, flag)),
        }
    }
    ranges
}

/// Prints the table for `classify_table.rs`.
#[test]
#[ignore]
#[cfg(feature = "std")]
fn print_tables() {
    use std::println;
    println!("// Generated by `fallback::tests::classify::print_tables`. Do not edit by hand.");
    println!();
    println!("/// Characters with the Emoji property, as inclusive ranges, each with");
    println!("/// whether it has Emoji_Presentation.");
    println!("#[rustfmt::skip]");
    println!("pub(super) static EMOJI: &[(u32, u32, bool)] = &[");
    for chunk in generate().chunks(4) {
        let line: Vec<std::string::String> = chunk
            .iter()
            .map(|(s, e, p)| std::format!("(0x{s:04X}, 0x{e:04X}, {p})"))
            .collect();
        println!("    {},", line.join(", "));
    }
    println!("];");
}
