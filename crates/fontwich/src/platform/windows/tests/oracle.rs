//! Differential tests against Chrome and against Unicode itself.
//!
//! In the crate rather than in `tests/`, because the tables it checks are
//! crate-internal: they are what fallback is made of, not something a caller
//! chooses. Data against data, no system call anywhere, so every host runs it
//! whatever fonts it has.
//!
//! Two invariants:
//!
//! 1. Every script that occurs in Unicode has fonts of its own in the
//!    script table, or is registered below as one Windows cannot render.
//! 2. For every script Chrome has an entry for, our script table names at
//!    least one of the families Chrome would have chosen, or the divergence is
//!    registered below.
//!
//! The point is that adding a script to Unicode, or quietly reordering a
//! table, has to be acknowledged rather than silently absorbed.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::Script;
use crate::platform::windows::scripts;
use icu_properties::CodePointMapData;
use icu_properties::props::{GeneralCategory, NamedEnumeratedProperty, Script as IcuScript};

/// The families our table offers for `script` itself, before Latin or
/// anything else a key adds.
fn script_tier(script: Script) -> Vec<String> {
    scripts::script_fonts(script)
        .iter()
        .map(|family| String::from(*family))
        .collect()
}

/// Scripts with no script-table entry, on purpose.
///
/// `Zyyy`, `Zinh` and `Zzzz` are not scripts and are answered by the Common
/// key. The rest are scripts Windows ships no font for
/// at all; a caller rendering them needs a font of its own, and the tail is
/// the honest answer rather than a family name that will never resolve.
const UNRENDERABLE: &[&str] = &[
    // Not scripts. Answered by the Common key.
    "Zinh", "Zyyy", "Zzzz",
    // Scripts we have no font for. Generated from the tables rather than
    // verified one by one, so an entry means "we have nothing", not
    // "Microsoft ships nothing" — if you find a font for one, delete the line
    // and add the table entry. Check the candidate font's `dlng` metadata
    // first; a plausible name is not evidence.
    "Aghb", "Ahom", "Avst", "Bali", "Bamu", "Bass", "Batk", "Berf", "Bhks", "Buhd", "Cham", "Chrs",
    "Cpmn", "Diak", "Dogr", "Dupl", "Elba", "Elym", "Gara", "Gong", "Gonm", "Gran", "Gukh", "Hano",
    "Hatr", "Hluw", "Hmng", "Hmnp", "Hung", "Kali", "Kawi", "Khoj", "Kits", "Krai", "Kthi", "Lana",
    "Lepc", "Limb", "Lina", "Linb", "Mahj", "Maka", "Mand", "Mani", "Marc", "Medf", "Mend", "Mero",
    "Modi", "Mroo", "Mult", "Nagm", "Nand", "Narb", "Nbat", "Newa", "Nshu", "Onao", "Ougr", "Palm",
    "Pauc", "Perm", "Phlp", "Plrd", "Rjng", "Rohg", "Samr", "Saur", "Sgnw", "Shrd", "Sidd", "Sidt",
    "Sind", "Sogd", "Sogo", "Soyo", "Sund", "Sunu", "Sylo", "Tagb", "Takr", "Tang", "Tavt", "Tayo",
    "Tglg", "Tirh", "Tnsa", "Todr", "Tols", "Toto", "Tutg", "Vith", "Wara", "Wcho", "Yezi", "Zanb",
];

#[test]
fn every_unicode_script_is_covered_or_registered() {
    let scripts = CodePointMapData::<IcuScript>::new();
    let categories = CodePointMapData::<GeneralCategory>::new();

    let mut seen = BTreeSet::new();
    for cp in 0..=0x10FFFFu32 {
        let Some(ch) = char::from_u32(cp) else {
            continue;
        };
        if categories.get(ch) == GeneralCategory::Unassigned {
            continue;
        }
        seen.insert(scripts.get(ch).short_name());
    }

    let mut uncovered = BTreeSet::new();
    for name in &seen {
        if UNRENDERABLE.contains(name) {
            continue;
        }
        let Ok(script) = Script::parse(name) else {
            panic!("ICU short name {name:?} is not a well-formed script tag");
        };
        if script_tier(script).is_empty() {
            uncovered.insert(*name);
        }
    }

    assert!(
        uncovered.is_empty(),
        "{} of {} Unicode scripts have no script-tier answer.\n\
         Add a font for each, or register it in UNRENDERABLE:\n{:#?}",
        uncovered.len(),
        seen.len(),
        uncovered
    );
}

#[test]
fn registered_unrenderable_scripts_really_are_uncovered() {
    // Keeps the list from rotting: once we add a font for something here, the
    // entry has to come back out.
    for name in UNRENDERABLE {
        let Ok(script) = Script::parse(name) else {
            continue;
        };
        assert!(
            script_tier(script).is_empty(),
            "{name} is registered as unrenderable but now has an entry: {:?}",
            script_tier(script)
        );
    }
}

/// Chrome's `kScriptToFontFamilies`, rekeyed from `UScriptCode` to ISO 15924
/// so it lines up with ICU's short names. Verbatim, typos included.
const CHROME: &[(&str, &[&str])] = &[
    ("Arab", &["Tahoma", "Segoe UI"]),
    ("Armi", &["Segoe UI Historic"]),
    ("Armn", &["Segoe UI", "Sylfaen"]),
    ("Beng", &["Nirmala UI", "Vrinda"]),
    (
        "Bopo",
        &[
            "Noto Sans TC",
            "Noto Sans CJK TC",
            "Microsoft JhengHei",
            "pmingli",
        ],
    ),
    ("Brah", &["Segoe UI Historic"]),
    ("Brai", &["Segoe UI Symbol"]),
    ("Bugi", &["Leelawadee UI"]),
    ("Cans", &["Gadugi", "Euphemia"]),
    ("Cari", &["Segoe UI Historic"]),
    ("Cher", &["Gadugi", "Plantagenet"]),
    ("Copt", &["Segoe UI Symbol"]),
    ("Cprt", &["Segoe UI Historic"]),
    ("Cyrl", &["Times New Roman"]),
    ("Deva", &["Nirmala UI", "Mangal"]),
    ("Dsrt", &["Segoe UI Symbol"]),
    ("Egyp", &["Segoe UI Historic"]),
    (
        "Ethi",
        &[
            "Nyala",
            "Abyssinica SIL",
            "Ethiopia Jiret",
            "Visual Geez Unicode",
            "GF Zemen Unicode",
            "Ebrima",
        ],
    ),
    ("Geor", &["Sylfaen", "Segoe UI"]),
    ("Glag", &["Segoe UI Historic", "Segoe UI Symbol"]),
    ("Goth", &["Segoe UI Historic", "Segoe UI Symbol"]),
    ("Grek", &["Times New Roman"]),
    ("Gujr", &["Nirmala UI", "Shruti"]),
    ("Guru", &["Nirmala UI", "Raavi"]),
    (
        "Hang",
        &["Noto Sans KR", "Noto Sans CJK KR", "Malgun Gothic", "Gulim"],
    ),
    (
        "Hans",
        &[
            "Noto Sans SC",
            "Noto Sans CJK SC",
            "Microsoft YaHei",
            "simsun",
        ],
    ),
    (
        "Hant",
        &[
            "Noto Sans TC",
            "Noto Sans CJK TC",
            "Microsoft JhengHei",
            "pmingli",
        ],
    ),
    ("Hebr", &["David", "Segoe UI"]),
    (
        "Hira",
        &[
            "Noto Sans JP",
            "Noto Sans CJK JP",
            "Meiryo",
            "Yu Gothic",
            "MS PGothic",
            "Microsoft YaHei",
        ],
    ),
    ("Ital", &["Segoe UI Historic", "Segoe UI Symbol"]),
    ("Java", &["Javanese Text"]),
    (
        "Kana",
        &[
            "Noto Sans JP",
            "Noto Sans CJK JP",
            "Meiryo",
            "Yu Gothic",
            "MS PGothic",
            "Microsoft YaHei",
        ],
    ),
    ("Khar", &["Segoe UI Historic"]),
    (
        "Khmr",
        &[
            "Leelawadee UI",
            "Khmer UI",
            "Khmer OS",
            "MoolBoran",
            "DaunPenh",
        ],
    ),
    ("Knda", &["Tunga", "Nirmala UI"]),
    (
        "Laoo",
        &[
            "Leelawadee UI",
            "Lao UI",
            "DokChampa",
            "Saysettha OT",
            "Phetsarath OT",
            "Code2000",
        ],
    ),
    ("Latn", &["Times New Roman"]),
    ("Lisu", &["Segoe UI"]),
    ("Lyci", &["Segoe UI Historic"]),
    ("Lydi", &["Segoe UI Historic"]),
    ("Merc", &["Segoe UI Historic", "Segoe UI Symbol"]),
    ("Mlym", &["Nirmala UI", "Kartika"]),
    ("Mong", &["Mongolian Baiti"]),
    ("Mtei", &["Nirmala UI"]),
    (
        "Mymr",
        &["Myanmar Text", "Padauk", "Parabaik", "Myanmar3", "Code2000"],
    ),
    ("Nkoo", &["Ebrima"]),
    ("Ogam", &["Segoe UI Historic", "Segoe UI Symbol"]),
    ("Olck", &["Nirmala UI"]),
    ("Orkh", &["Segoe UI Historic", "Segoe UI Symbol"]),
    ("Orya", &["Kalinga", "ori1Uni", "Lohit Oriya", "Nirmala UI"]),
    ("Osma", &["Ebrima"]),
    ("Phag", &["Microsoft PhagsPa"]),
    ("Phli", &["Segoe UI Historic"]),
    ("Prti", &["Segoe UI Historic"]),
    ("Runr", &["Segoe UI Historic", "Segoe UI Symbol"]),
    ("Sarb", &["Segoe UI Historic"]),
    ("Shaw", &["Segoe UI Historic"]),
    ("Sinh", &["Iskoola Pota", "AksharUnicode", "Nirmala UI"]),
    ("Sora", &["Nirmala UI"]),
    (
        "Syrc",
        &["Estrangelo Edessa", "Estrangelo Nisibin", "Code2000"],
    ),
    ("Tale", &["Microsoft Tai Le"]),
    ("Talu", &["Microsoft New Tai Lue"]),
    ("Taml", &["Nirmala UI", "Latha"]),
    ("Telu", &["Nirmala UI", "Gautami"]),
    ("Tfng", &["Ebrima"]),
    ("Thaa", &["MV Boli"]),
    ("Thai", &["Tahoma", "Leelawadee UI", "Leelawadee"]),
    (
        "Tibt",
        &["Microsoft Himalaya", "Jomolhari", "Tibetan Machine Uni"],
    ),
    ("Vaii", &["Ebrima"]),
    ("Xpeo", &["Segoe UI Historic"]),
    ("Xsux", &["Segoe UI Historic"]),
    ("Yiii", &["Microsoft Yi Baiti", "Nuosu SIL", "Code2000"]),
    ("Zsym", &["Segoe UI Symbol"]),
];

/// Scripts where we deliberately share no family with Chrome.
///
/// Empty today: every correction so far has been a reorder or a rename that
/// still leaves a font in common. An entry here means we decided Chrome's
/// only suggestions are wrong, which is a bigger claim and wants its reason
/// recorded.
const NO_COMMON_FAMILY: &[&str] = &[];

#[test]
fn we_agree_with_chrome_on_at_least_one_family_per_script() {
    let mut disagreements = Vec::new();
    for (name, chrome) in CHROME {
        if NO_COMMON_FAMILY.contains(name) {
            continue;
        }
        let ours = script_tier(Script::parse(name).expect("well-formed tag"));
        let shared = ours
            .iter()
            .any(|f| chrome.iter().any(|c| c.eq_ignore_ascii_case(f.as_str())));
        if !shared {
            disagreements.push(format!("{name}: chrome {chrome:?}, ours {ours:?}"));
        }
    }
    assert!(
        disagreements.is_empty(),
        "no family in common with Chrome for {} script(s):\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}

#[test]
fn we_cover_every_script_chrome_covers() {
    let missing: Vec<_> = CHROME
        .iter()
        .filter(|(name, _)| script_tier(Script::parse(name).unwrap()).is_empty())
        .map(|(name, _)| *name)
        .collect();
    assert!(
        missing.is_empty(),
        "Chrome covers these and we do not: {missing:?}"
    );
}
