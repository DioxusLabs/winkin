use alloc::format;

use parlance::{GenericFamily, Language, Script};

use crate::fallback::emoji::Presentation;
use crate::fallback::key::*;
use crate::fallback::unicode;
use crate::hash::HashMap;
use crate::script as sc;

/// No backend reads anything: the key keeps scripts apart and drops the
/// class.
const PLAIN: BackendFacts = BackendFacts {
    reads_serif: false,
    reads_monospace: false,
    per_language: false,
    reads_language: false,
};

fn script(tag: &[u8; 4]) -> Script {
    Script::from_bytes(*tag)
}

/// The id of a script that has one.
fn id(script: Script) -> ScriptId {
    let id = ScriptId::new(script);
    assert!(id.is_some(), "{script:?} should have an id");
    id.unwrap_or(ScriptId::COMMON)
}

fn script_key(script: Script, class: GenericClass, han: Option<Han>) -> FallbackKey {
    FallbackKey(KeyKind::Script(id(script), class, han, None))
}

fn han_key(han: Han, class: GenericClass) -> FallbackKey {
    FallbackKey(KeyKind::Han(han, class))
}

fn generic_key(family: GenericFamily, bucket: GenericBucket) -> FallbackKey {
    FallbackKey(KeyKind::Generic(family, bucket))
}

fn emoji_key(presentation: Presentation) -> FallbackKey {
    FallbackKey(KeyKind::Emoji(presentation))
}

fn language(tag: &str) -> Option<Language> {
    let parsed = parse_language(tag);
    assert!(parsed.is_some(), "{tag} should parse");
    parsed
}

fn text(
    tag: &[u8; 4],
    lang: Option<&str>,
    generic: GenericClass,
    facts: BackendFacts,
) -> FallbackKey {
    FallbackKey::new(
        &FallbackRequest::Text {
            script: script(tag),
            language: lang.and_then(parse_language),
            generic,
        },
        facts,
    )
}

fn plain(tag: &[u8; 4], lang: Option<&str>) -> FallbackKey {
    text(tag, lang, GenericClass::Plain, PLAIN)
}

fn generic(family: GenericFamily, lang: Option<&str>) -> FallbackKey {
    FallbackKey::new(
        &FallbackRequest::Generic(family, lang.and_then(parse_language)),
        PLAIN,
    )
}

const EVERY_GENERIC: [GenericFamily; 13] = [
    GenericFamily::Serif,
    GenericFamily::SansSerif,
    GenericFamily::Monospace,
    GenericFamily::Cursive,
    GenericFamily::Fantasy,
    GenericFamily::SystemUi,
    GenericFamily::UiSerif,
    GenericFamily::UiSansSerif,
    GenericFamily::UiMonospace,
    GenericFamily::UiRounded,
    GenericFamily::Emoji,
    GenericFamily::Math,
    GenericFamily::FangSong,
];

#[test]
fn the_key_is_six_bytes() {
    // A one-byte script id, a one-byte class and a one-byte tradition, whose
    // spare values hold the discriminant, and a two-byte language whose zero
    // is none, at alignment two.
    assert_eq!(size_of::<FallbackKey>(), 6);
    assert_eq!(align_of::<FallbackKey>(), 2);
    assert_eq!(size_of::<ScriptId>(), 1);
    assert_eq!(size_of::<Option<LanguageId>>(), 2);
}

#[test]
fn every_language_fontconfig_has_an_id_of_its_own() {
    assert_eq!(LanguageId::all().count(), LanguageId::COUNT);
    const { assert!(LanguageId::COUNT > 240, "fontconfig's orthographies") };
    for pair in FC_LANGUAGES.windows(2) {
        assert!(pair[0] < pair[1], "{pair:?} out of order");
    }
    for id in LanguageId::all() {
        let tag = id.tag();
        let language = language(tag);
        assert_eq!(
            LanguageId::new(&language.expect("parsed")),
            Some(id),
            "{tag}"
        );
    }
}

#[test]
fn a_language_takes_its_region_its_script_or_itself() {
    let id = |tag: &str| {
        language(tag)
            .and_then(|tag| LanguageId::new(&tag))
            .map(LanguageId::tag)
    };
    for (tag, expected) in [
        ("fa", Some("fa")),
        ("FA-ir", Some("fa")),
        ("pa", Some("pa")),
        ("pa-PK", Some("pa-pk")),
        ("pa-Arab-PK", Some("pa-pk")),
        ("zh", Some("zh-cn")),
        ("zh-Hans", Some("zh-cn")),
        ("zh-Hant", Some("zh-tw")),
        ("zh-TW", Some("zh-tw")),
        ("zh-HK", Some("zh-hk")),
        ("zh-Hant-HK", Some("zh-hk")),
        ("zh-SG", Some("zh-sg")),
        ("yue", Some("yue")),
        ("und-Zsye", Some("und-zsye")),
        ("en-US", Some("en")),
        ("ja-JP", Some("ja")),
        ("ku", None),
        ("und", None),
        ("qaa", None),
        ("xyz", None),
    ] {
        assert_eq!(id(tag), expected, "{tag}");
    }
}

/// The pseudo-scripts beside Unicode's.
const PSEUDO: [&[u8; 4]; 4] = [b"Zyyy", b"Zsym", b"Zmth", b"Zsye"];

#[test]
fn every_script_has_an_id_of_its_own() {
    let tags = unicode::script_tags();
    assert!(tags.len() > 150, "the script table is every script");
    assert_eq!(ScriptId::COUNT, tags.len() + PSEUDO.len());
    let mut seen = HashMap::default();
    for &tag in tags.iter().chain(PSEUDO) {
        let id = id(Script::from_bytes(tag));
        assert_eq!(id.tag(), Script::from_bytes(tag));
        assert_eq!(seen.insert(id, tag), None, "{id:?} is taken twice");
    }
    assert_eq!(seen.len(), ScriptId::COUNT);
    // Every id is one of them, and names itself back.
    for id in ScriptId::all() {
        assert_eq!(ScriptId::new(id.tag()), Some(id));
        assert_eq!(seen.get(&id), Some(&id.tag().to_bytes()));
    }
    assert_eq!(ScriptId::all().count(), ScriptId::COUNT);
    assert_eq!(ScriptId::new(Script::COMMON), Some(ScriptId::COMMON));
    assert_eq!(format!("{:?}", id(sc::LATN)), "ScriptId(Latn)");
}

#[test]
fn a_tag_outside_the_set_has_no_id() {
    for tag in [
        // Inherited and Unknown have no fonts of their own.
        b"Zinh", b"Zzzz",
        // The combined CLDR tags key by their Han tradition instead.
        b"Hans", b"Hant", b"Jpan", b"Kore", b"Hrkt",
        // ISO 15924 tags Unicode does not assign, and private use.
        b"Latf", b"Latg", b"Syre", b"Cyrs", b"Geok", b"Zxxx", b"Qaaa", b"Qabx",
        // Plausible fakes.
        b"Abcd", b"Latm", b"Arob", b"Zzzy", b"Aaaa", b"Qqqq",
    ] {
        assert_eq!(ScriptId::new(script(tag)), None, "{:?}", script(tag));
    }
    for tag in [
        [0xFF; 4],
        [0; 4],
        *b"Lat1",
        *b"La n",
        [b'L', b'a', b't', 0xC3],
    ] {
        assert_eq!(ScriptId::new(Script::from_bytes(tag)), None, "{tag:?}");
    }
}

#[test]
fn a_script_id_ignores_case() {
    for tag in unicode::script_tags().iter().chain(PSEUDO) {
        let lower = tag.map(|b| b.to_ascii_lowercase());
        let upper = tag.map(|b| b.to_ascii_uppercase());
        let expected = ScriptId::new(Script::from_bytes(*tag));
        assert_eq!(ScriptId::new(Script::from_bytes(lower)), expected);
        assert_eq!(ScriptId::new(Script::from_bytes(upper)), expected);
    }
}

#[test]
fn only_the_set_keys_a_script_of_its_own() {
    // Every four-letter tag: the members key as themselves (or their Han
    // tradition, or emoji), the CLDR tags as theirs, and the rest, 26⁴ less
    // a few hundred, all as Common.
    let common = script_key(Script::COMMON, GenericClass::Plain, None);
    let mut own = 0;
    for a in b'A'..=b'Z' {
        for b in b'a'..=b'z' {
            for c in b'a'..=b'z' {
                for d in b'a'..=b'z' {
                    let tag = Script::from_bytes([a, b, c, d]);
                    let key = plain(&tag.to_bytes(), None);
                    match ScriptId::new(tag) {
                        Some(_) => own += 1,
                        None => match &tag.to_bytes() {
                            b"Hans" | b"Hant" | b"Jpan" | b"Kore" | b"Hrkt" => {
                                assert!(key.han().is_some(), "{tag:?}");
                            }
                            _ => assert_eq!(key, common, "{tag:?}"),
                        },
                    }
                }
            }
        }
    }
    assert_eq!(own, ScriptId::COUNT);
}

/// A language for each Han tradition a script key can carry, and none.
const TRADITIONS: [Option<&str>; 6] = [
    None,
    Some("zh"),
    Some("zh-TW"),
    Some("zh-HK"),
    Some("ja"),
    Some("ko"),
];

#[test]
fn the_key_space_is_closed() {
    // Every key any request reaches, counted.
    let mut keys = HashMap::default();
    let mut every_fact = [PLAIN; 16];
    for (bits, facts) in every_fact.iter_mut().enumerate() {
        facts.reads_serif = bits & 1 != 0;
        facts.reads_monospace = bits & 2 != 0;
        facts.per_language = bits & 4 != 0;
        facts.reads_language = bits & 8 != 0;
    }
    let combined = [
        b"Hans", b"Hant", b"Jpan", b"Kore", b"Hrkt", b"Zinh", b"Zzzz", b"Abcd",
    ];
    let tags = unicode::script_tags().iter().chain(PSEUDO).chain(combined);
    for tag in tags {
        for lang in TRADITIONS.iter().chain(&[Some("en"), Some("ar")]) {
            for class in [
                GenericClass::Plain,
                GenericClass::Serif,
                GenericClass::Monospace,
            ] {
                for facts in every_fact {
                    keys.insert(text(tag, *lang, class, facts), ());
                }
            }
        }
    }
    let langs = [
        None,
        Some("zh"),
        Some("zh-TW"),
        Some("ja"),
        Some("ko"),
        Some("hi"),
        Some("ar"),
        Some("ru"),
        Some("el"),
    ];
    for family in EVERY_GENERIC {
        for lang in langs {
            keys.insert(generic(family, lang), ());
        }
    }
    for lang in langs {
        let request = FallbackRequest::Standard(lang.and_then(parse_language));
        keys.insert(FallbackKey::new(&request, PLAIN), ());
    }
    for presentation in [
        Presentation::Text,
        Presentation::Emoji,
        Presentation::TextOnly,
        Presentation::EmojiOnly,
    ] {
        keys.insert(
            FallbackKey::new(&FallbackRequest::Emoji(presentation), PLAIN),
            (),
        );
    }
    // Script keys: every id but the five Han scripts and emoji, plain or
    // serif, Arabic and Hebrew monospace too, under eighteen pairs of a
    // tradition and a language: neither; each of the seven languages alone
    // (`zh`, `zh-tw`, `zh-hk`, `ja`, `ko`, `en`, `ar`); each of the five
    // traditions alone; and each with its own language.
    let scripts = ScriptId::COUNT - 6;
    let script_keys = (2 * scripts + 2) * 18;
    // Han keys: five traditions, plain or serif.
    let han_keys = 5 * 2;
    // Generic keys: thirteen generics less `emoji` and the four `ui-*`,
    // in nine buckets.
    let generic_keys = 8 * 9;
    // Standard keys: nine buckets.
    let standard_keys = 9;
    // Emoji keys: two presentations.
    let emoji_keys = 2;
    assert_eq!(
        keys.len(),
        script_keys + han_keys + generic_keys + standard_keys + emoji_keys
    );
    // Within the bound the key's parts give, whatever the requests.
    let bound =
        ScriptId::COUNT * 3 * 6 * (LanguageId::COUNT + 1) + 5 * 3 + EVERY_GENERIC.len() * 9 + 9 + 4;
    assert!(keys.len() <= bound);
    assert_eq!(keys.len(), 6_249);
    assert_eq!(bound, 1_083_601);
}

#[test]
fn a_key_reads_back_what_it_holds() {
    let latin = text(
        b"Latn",
        Some("ja"),
        GenericClass::Serif,
        BackendFacts {
            reads_serif: true,
            reads_monospace: false,
            per_language: true,
            reads_language: false,
        },
    );
    assert_eq!(latin.script(), Some(sc::LATN));
    assert_eq!(latin.han(), Some(Han::Jpan));
    assert_eq!(latin.class(), GenericClass::Serif);
    assert_eq!(latin.generic(), None);
    assert_eq!(latin.presentation(), None);
    let fake = plain(b"Abcd", None);
    assert_eq!(fake.script(), Some(Script::COMMON));
    assert_eq!(fake.han(), None);
    let han = plain(b"Hani", Some("zh-HK"));
    assert_eq!(han.script(), Some(sc::HANI));
    assert_eq!(han.han(), Some(Han::HantHK));
    assert_eq!(han.class(), GenericClass::Plain);
    let kana = plain(b"kana", None);
    assert_eq!(kana.script(), Some(sc::HANI));
    assert_eq!(kana.han(), Some(Han::Jpan));
    let serif = generic(GenericFamily::UiSerif, Some("ko"));
    assert_eq!(
        serif.generic(),
        Some((GenericFamily::Serif, GenericBucket::Kore))
    );
    assert_eq!(serif.script(), None);
    assert_eq!(serif.han(), None);
    assert_eq!(serif.class(), GenericClass::Plain);
    assert_eq!(serif.presentation(), None);
    let emoji = FallbackKey::new(&FallbackRequest::Emoji(Presentation::TextOnly), PLAIN);
    assert_eq!(emoji.presentation(), Some(Presentation::Text));
    assert_eq!(emoji.script(), None);
    assert_eq!(emoji.generic(), None);
}

#[test]
fn every_script_keys_itself_whatever_the_language() {
    let scripts = unicode::script_tags();
    let languages = [
        None,
        Some("en"),
        Some("ja"),
        Some("zh-TW"),
        Some("ko"),
        Some("ar"),
    ];
    for &tag in scripts {
        for lang in languages {
            let key = plain(&tag, lang);
            let expected = match &tag {
                // Han has a tradition, and the language picks it.
                b"Hani" => han_key(
                    match lang {
                        Some("ja") => Han::Jpan,
                        Some("zh-TW") => Han::Hant,
                        Some("ko") => Han::Kore,
                        _ => Han::Hans,
                    },
                    GenericClass::Plain,
                ),
                // The scripts that travel with Han have one tradition each.
                b"Hira" | b"Kana" => han_key(Han::Jpan, GenericClass::Plain),
                b"Hang" => han_key(Han::Kore, GenericClass::Plain),
                b"Bopo" => han_key(Han::Hant, GenericClass::Plain),
                // Every other script is its own key, the language dropped.
                _ => script_key(Script::from_bytes(tag), GenericClass::Plain, None),
            };
            assert_eq!(
                key,
                expected,
                "{:?} under {lang:?}",
                Script::from_bytes(tag)
            );
        }
    }
}

#[test]
fn the_tags_no_character_has_key_as_their_kind() {
    let common = script_key(Script::COMMON, GenericClass::Plain, None);
    for lang in [None, Some("ja"), Some("zh-HK")] {
        // Common, Inherited and Unknown have no fonts of their own: the tail.
        assert_eq!(plain(b"Zyyy", lang), common);
        assert_eq!(plain(b"Zinh", lang), common);
        assert_eq!(plain(b"Zzzz", lang), common);
        // Symbols and math keep keys of their own.
        assert_eq!(
            plain(b"Zsym", lang),
            script_key(sc::ZSYM, GenericClass::Plain, None)
        );
        assert_eq!(
            plain(b"Zmth", lang),
            script_key(sc::ZMTH, GenericClass::Plain, None)
        );
        // The emoji script is the emoji key.
        assert_eq!(plain(b"Zsye", lang), emoji_key(Presentation::Emoji));
        // So are the combined CLDR tags, whatever the language.
        assert_eq!(
            plain(b"Hrkt", lang),
            han_key(Han::Jpan, GenericClass::Plain)
        );
        assert_eq!(
            plain(b"Jpan", lang),
            han_key(Han::Jpan, GenericClass::Plain)
        );
        assert_eq!(
            plain(b"Kore", lang),
            han_key(Han::Kore, GenericClass::Plain)
        );
        assert_eq!(
            plain(b"Hans", lang),
            han_key(Han::Hans, GenericClass::Plain)
        );
    }
}

#[test]
fn han_takes_its_tradition_from_the_language() {
    for (tag, expected) in [
        ("zh", Han::Hans),
        ("zh-CN", Han::Hans),
        ("zh-SG", Han::Hans),
        ("zh-Hans", Han::Hans),
        ("zh-Hans-CN", Han::Hans),
        ("cmn", Han::Hans),
        ("zh-cmn", Han::Hans),
        ("zh-TW", Han::Hant),
        ("zh-Hant", Han::Hant),
        ("zh-Hant-TW", Han::Hant),
        ("yue", Han::Hant),
        ("zh-yue", Han::Hant),
        ("zh-HK", Han::HantHK),
        ("zh-MO", Han::HantHK),
        ("zh-Hant-HK", Han::HantHK),
        ("zh-Hant-MO", Han::HantHK),
        ("yue-HK", Han::HantHK),
        ("zh-yue-HK", Han::HantHK),
        ("zh_hant_hk", Han::HantHK),
        ("ZH-hk", Han::HantHK),
        ("ja", Han::Jpan),
        ("ja-JP", Han::Jpan),
        ("ja-Jpan", Han::Jpan),
        ("ja-Jpan-JP", Han::Jpan),
        ("ko", Han::Kore),
        ("ko-KR", Han::Kore),
        ("ko-Kore", Han::Kore),
        ("ko-Kore-KR", Han::Kore),
        // The script subtag outranks the region, and the region the
        // language.
        ("zh-Hans-HK", Han::Hans),
        ("zh-Hans-TW", Han::Hans),
        ("zh-Hant-CN", Han::Hant),
        ("en-JP", Han::Jpan),
        ("en-HK", Han::HantHK),
        ("en-TW", Han::Hant),
        // Trailing subtags are read past.
        ("zh-Latn-pinyin", Han::Hans),
        ("ja-JP-u-ca-japanese", Han::Jpan),
        // No tradition: Chrome's final default, Simplified.
        ("en", Han::Hans),
        ("en-US", Han::Hans),
        ("ar", Han::Hans),
    ] {
        assert!(language(tag).is_some());
        assert_eq!(
            plain(b"Hani", Some(tag)),
            han_key(expected, GenericClass::Plain),
            "{tag}"
        );
    }
    assert_eq!(
        plain(b"Hani", None),
        han_key(Han::Hans, GenericClass::Plain)
    );
}

#[test]
fn the_scripts_beside_han_ignore_the_language() {
    let han = |tag, lang| {
        let key = plain(tag, Some(lang));
        assert_eq!(key.script(), Some(sc::HANI), "{key:?}");
        assert_eq!(key.class(), GenericClass::Plain, "{key:?}");
        key.han().expect("a Han key")
    };
    for lang in ["zh", "zh-TW", "zh-HK", "ja", "ko", "en"] {
        assert_eq!(han(b"Hira", lang), Han::Jpan, "{lang}");
        assert_eq!(han(b"Kana", lang), Han::Jpan, "{lang}");
        assert_eq!(han(b"Hrkt", lang), Han::Jpan, "{lang}");
        assert_eq!(han(b"Hang", lang), Han::Kore, "{lang}");
        // Bopomofo is Taiwan's, even in Hong Kong.
        assert_eq!(han(b"Bopo", lang), Han::Hant, "{lang}");
    }
    // A combined tag names its tradition, and a Hong Kong region still
    // makes Traditional Hong Kong's.
    assert_eq!(han(b"Hant", "ja"), Han::Hant);
    assert_eq!(han(b"Hant", "zh-HK"), Han::HantHK);
    assert_eq!(han(b"Hans", "zh-HK"), Han::Hans);
    assert_eq!(han(b"Jpan", "ko"), Han::Jpan);
}

#[test]
fn an_unreadable_tag_is_no_language() {
    for tag in [
        "",
        "!!",
        "e",
        "en1",
        "en-La1n",
        "zh-yu3",
        "\0",
        "日本語",
        "--",
        "zh-Hant-TW-x-",
    ] {
        assert_eq!(parse_language(tag), None, "{tag:?} should not parse");
        assert_eq!(plain(b"Hani", Some(tag)), plain(b"Hani", None), "{tag:?}");
        assert_eq!(
            generic(GenericFamily::Serif, Some(tag)),
            generic(GenericFamily::Serif, None)
        );
    }
    let long = "x".repeat(4096);
    assert_eq!(parse_language(&long), None);
}

#[test]
fn a_script_is_read_as_its_letters() {
    // Case does not make a key of its own.
    for tag in [b"latn", b"LATN", b"lAtN"] {
        assert_eq!(plain(tag, None), plain(b"Latn", None));
    }
    assert_eq!(
        plain(b"hani", Some("ja")),
        han_key(Han::Jpan, GenericClass::Plain)
    );
    assert_eq!(plain(b"ZYYY", None), plain(b"Zyyy", None));
    // A tag that is not four letters is Common, so that every key prints.
    let common = script_key(Script::COMMON, GenericClass::Plain, None);
    for tag in [
        [0xFF; 4],
        [0; 4],
        *b"Lat1",
        *b"La n",
        [b'L', b'a', b't', 0xC3],
    ] {
        let key = plain(&tag, Some("ja"));
        assert_eq!(key, common, "{tag:?}");
        let _ = format!("{key:?}");
    }
}

#[test]
fn the_class_survives_only_where_a_backend_reads_it() {
    use GenericClass::{Monospace, Plain, Serif};
    let serif = BackendFacts {
        reads_serif: true,
        ..PLAIN
    };
    let monospace = BackendFacts {
        reads_monospace: true,
        ..PLAIN
    };
    // Nobody reads it: every class is plain.
    for class in [Plain, Serif, Monospace] {
        assert_eq!(
            text(b"Latn", None, class, PLAIN),
            script_key(sc::LATN, Plain, None)
        );
        assert_eq!(
            text(b"Arab", None, class, PLAIN),
            script_key(sc::ARAB, Plain, None)
        );
        assert_eq!(text(b"Hani", None, class, PLAIN), han_key(Han::Hans, Plain));
    }
    // A serif reader keeps serif on every run key, and only serif.
    assert_eq!(
        text(b"Deva", None, Serif, serif),
        script_key(sc::DEVA, Serif, None)
    );
    assert_eq!(
        text(b"Zyyy", None, Serif, serif),
        script_key(Script::COMMON, Serif, None)
    );
    assert_eq!(
        text(b"Hani", Some("ja"), Serif, serif),
        han_key(Han::Jpan, Serif)
    );
    assert_eq!(
        text(b"Arab", None, Monospace, serif),
        script_key(sc::ARAB, Plain, None)
    );
    // A monospace reader keeps monospace for Arabic and Hebrew alone.
    assert_eq!(
        text(b"Arab", None, Monospace, monospace),
        script_key(sc::ARAB, Monospace, None)
    );
    assert_eq!(
        text(b"Hebr", None, Monospace, monospace),
        script_key(sc::HEBR, Monospace, None)
    );
    assert_eq!(
        text(b"Latn", None, Monospace, monospace),
        script_key(sc::LATN, Plain, None)
    );
    assert_eq!(
        text(b"Syrc", None, Monospace, monospace),
        script_key(sc::SYRC, Plain, None)
    );
    assert_eq!(
        text(b"Hani", None, Monospace, monospace),
        han_key(Han::Hans, Plain)
    );
    assert_eq!(
        text(b"Arab", None, Serif, monospace),
        script_key(sc::ARAB, Plain, None)
    );
    // Both readers at once.
    let both = BackendFacts {
        reads_serif: true,
        reads_monospace: true,
        per_language: false,
        reads_language: false,
    };
    assert_eq!(
        text(b"Hebr", None, Serif, both),
        script_key(sc::HEBR, Serif, None)
    );
    assert_eq!(
        text(b"Hebr", None, Monospace, both),
        script_key(sc::HEBR, Monospace, None)
    );
    // The emoji key has no class.
    assert_eq!(
        text(b"Zsye", None, Serif, both),
        emoji_key(Presentation::Emoji)
    );
}

#[test]
fn a_generic_is_a_class() {
    use GenericClass::{Monospace, Plain, Serif};
    for (family, class) in [
        (GenericFamily::Serif, Serif),
        (GenericFamily::UiSerif, Serif),
        (GenericFamily::Monospace, Monospace),
        (GenericFamily::UiMonospace, Monospace),
        (GenericFamily::SansSerif, Plain),
        (GenericFamily::UiSansSerif, Plain),
        (GenericFamily::UiRounded, Plain),
        (GenericFamily::Cursive, Plain),
        (GenericFamily::Fantasy, Plain),
        (GenericFamily::SystemUi, Plain),
        (GenericFamily::Emoji, Plain),
        (GenericFamily::Math, Plain),
        (GenericFamily::FangSong, Plain),
    ] {
        assert_eq!(GenericClass::from(family), class, "{family:?}");
    }
    assert_eq!(GenericClass::default(), Plain);
}

#[test]
fn a_per_language_backend_keys_a_script_with_the_language_tradition() {
    let facts = BackendFacts {
        per_language: true,
        ..PLAIN
    };
    let run = |tag, lang| text(tag, lang, GenericClass::Plain, facts);
    // Every script keeps its own key, ordered by the language's Han
    // tradition, or by none.
    for tag in [
        b"Latn", b"Arab", b"Deva", b"Thai", b"Zsym", b"Zmth", b"Zyyy", b"Tirh",
    ] {
        let key = |han| script_key(script(tag), GenericClass::Plain, han);
        assert_eq!(run(tag, None), key(None));
        assert_eq!(run(tag, Some("en")), key(None));
        assert_eq!(run(tag, Some("ar")), key(None));
        assert_eq!(run(tag, Some("ja")), key(Some(Han::Jpan)));
        assert_eq!(run(tag, Some("en-JP")), key(Some(Han::Jpan)));
        assert_eq!(run(tag, Some("zh")), key(Some(Han::Hans)));
        assert_eq!(run(tag, Some("zh-TW")), key(Some(Han::Hant)));
        assert_eq!(run(tag, Some("zh-HK")), key(Some(Han::HantHK)));
        assert_eq!(run(tag, Some("ko-KR")), key(Some(Han::Kore)));
        // Without a per-language backend the language is dropped.
        assert_eq!(plain(tag, Some("ja")), key(None));
    }
    // Inherited, Unknown and bad bytes are Common, with the tradition.
    let common = |han| script_key(Script::COMMON, GenericClass::Plain, han);
    assert_eq!(run(b"Zinh", Some("ja")), common(Some(Han::Jpan)));
    assert_eq!(run(b"Zzzz", Some("ko")), common(Some(Han::Kore)));
    assert_eq!(run(&[0xFF; 4], Some("zh-HK")), common(Some(Han::HantHK)));
    // Han's scripts key as they do anywhere, by their own tradition.
    assert_eq!(run(b"Hani", None), han_key(Han::Hans, GenericClass::Plain));
    assert_eq!(
        run(b"Hani", Some("ar")),
        han_key(Han::Hans, GenericClass::Plain)
    );
    assert_eq!(
        run(b"Hani", Some("ja")),
        han_key(Han::Jpan, GenericClass::Plain)
    );
    assert_eq!(
        run(b"Hang", Some("ja")),
        han_key(Han::Kore, GenericClass::Plain)
    );
    assert_eq!(
        run(b"Kana", Some("zh-TW")),
        han_key(Han::Jpan, GenericClass::Plain)
    );
    // Emoji is still its own.
    assert_eq!(run(b"Zsye", Some("ja")), emoji_key(Presentation::Emoji));
    // Android reads serif as well; the script key keeps it beside the
    // tradition.
    let android = BackendFacts {
        reads_serif: true,
        ..facts
    };
    assert_eq!(
        text(b"Deva", None, GenericClass::Serif, android),
        script_key(sc::DEVA, GenericClass::Serif, None)
    );
    assert_eq!(
        text(b"Latn", Some("ja"), GenericClass::Serif, android),
        script_key(sc::LATN, GenericClass::Serif, Some(Han::Jpan))
    );
    assert_eq!(
        text(b"Hani", Some("ja"), GenericClass::Serif, android),
        han_key(Han::Jpan, GenericClass::Serif)
    );
    // Monospace Arabic keeps its key where a backend reads it.
    let both = BackendFacts {
        reads_monospace: true,
        ..facts
    };
    assert_eq!(
        text(b"Arab", Some("ja"), GenericClass::Monospace, both),
        script_key(sc::ARAB, GenericClass::Monospace, Some(Han::Jpan))
    );
}

#[test]
fn every_generic_resolves_in_its_language_bucket() {
    let buckets: [(GenericBucket, &[Option<&str>]); 9] = [
        (
            GenericBucket::Common,
            &[
                None,
                Some("en"),
                Some("fr-CA"),
                Some("sr-Latn"),
                Some("he"),
                Some("th"),
                Some("en-JP"),
            ],
        ),
        (
            GenericBucket::Hans,
            &[
                Some("zh"),
                Some("zh-CN"),
                Some("zh-SG"),
                Some("zh-Hans"),
                Some("zh-Hans-HK"),
            ],
        ),
        (
            GenericBucket::Hant,
            &[
                Some("zh-TW"),
                Some("zh-Hant"),
                Some("zh-HK"),
                Some("zh-MO"),
                Some("zh-Hant-HK"),
                Some("yue"),
            ],
        ),
        (
            GenericBucket::Jpan,
            &[Some("ja"), Some("ja-JP"), Some("ja-Jpan")],
        ),
        (GenericBucket::Kore, &[Some("ko"), Some("ko-KR")]),
        (
            GenericBucket::Deva,
            &[Some("hi"), Some("mr"), Some("ne"), Some("sa")],
        ),
        (
            GenericBucket::Arab,
            &[Some("ar"), Some("fa"), Some("ur"), Some("uz-Arab")],
        ),
        (
            GenericBucket::Cyrl,
            &[Some("ru"), Some("uk"), Some("sr"), Some("uz-Cyrl")],
        ),
        (GenericBucket::Grek, &[Some("el"), Some("el-GR")]),
    ];
    for family in EVERY_GENERIC {
        for (bucket, langs) in buckets {
            for &lang in langs {
                let expected = match family {
                    // The `ui-*` generics are their plain ones.
                    GenericFamily::UiSerif => generic_key(GenericFamily::Serif, bucket),
                    GenericFamily::UiSansSerif | GenericFamily::UiRounded => {
                        generic_key(GenericFamily::SansSerif, bucket)
                    }
                    GenericFamily::UiMonospace => generic_key(GenericFamily::Monospace, bucket),
                    // The emoji generic is the emoji fonts.
                    GenericFamily::Emoji => emoji_key(Presentation::Emoji),
                    family => generic_key(family, bucket),
                };
                assert_eq!(generic(family, lang), expected, "{family:?} under {lang:?}");
            }
        }
    }
}

#[test]
fn a_generic_ignores_what_the_backends_read() {
    let every = BackendFacts {
        reads_serif: true,
        reads_monospace: true,
        per_language: true,
        reads_language: true,
    };
    for family in EVERY_GENERIC {
        for lang in ["ar", "ja", "zh-HK"] {
            let request = FallbackRequest::Generic(family, parse_language(lang));
            assert_eq!(
                FallbackKey::new(&request, every),
                FallbackKey::new(&request, PLAIN)
            );
        }
    }
}

#[test]
fn emoji_keys_by_its_presentation_unforced() {
    for (presentation, expected) in [
        (Presentation::Emoji, Presentation::Emoji),
        (Presentation::EmojiOnly, Presentation::Emoji),
        (Presentation::Text, Presentation::Text),
        (Presentation::TextOnly, Presentation::Text),
    ] {
        let request = FallbackRequest::Emoji(presentation);
        assert_eq!(FallbackKey::new(&request, PLAIN), emoji_key(expected));
        let every = BackendFacts {
            reads_serif: true,
            reads_monospace: true,
            per_language: true,
            reads_language: true,
        };
        assert_eq!(FallbackKey::new(&request, every), emoji_key(expected));
    }
}
