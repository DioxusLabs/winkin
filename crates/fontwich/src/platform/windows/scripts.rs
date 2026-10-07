//! The Windows fonts that draw each script, best first.
//!
//! Derived from Chrome's `font_fallback_win.cc`, with corrections noted
//! inline. A caller walks the entries until one is installed.
//!
//! Entries claiming Segoe UI Historic are checked against that font's `dlng`
//! metadata, the authoritative list of what it draws. Do not add one on the
//! strength of the name alone. Fonts that do not ship with Windows (Code2000,
//! DejaVu, the SIL fonts) are absent here; Chrome's catch-all lists in
//! `tail.rs` carry some of them.

use parlance::Script;

use crate::script::key;

type Entry = ([u8; 4], &'static [&'static str]);

/// Sorted by tag so lookup can binary search. The sort is checked by a test.
#[rustfmt::skip]
static SCRIPT_FONTS: &[Entry] = &[
    // Ebrima gained Adlam in Windows 10 1809. Chrome has no entry.
    (*b"Adlm", &["Ebrima"]),
    (*b"Arab", &["Tahoma", "Segoe UI", "Arial"]),
    (*b"Armi", &["Segoe UI Historic"]),
    (*b"Armn", &["Segoe UI", "Sylfaen"]),
    (*b"Beng", &["Nirmala UI", "Vrinda"]),
    // Chrome routes Bopomofo through its Traditional Han list, which works
    // but loses the ordering; the Ming fonts have the better zhuyin.
    (*b"Bopo", &["PMingLiU", "MingLiU", "Microsoft JhengHei"]),
    (*b"Brah", &["Segoe UI Historic"]),
    (*b"Brai", &["Segoe UI Symbol"]),
    (*b"Bugi", &["Leelawadee UI"]),
    (*b"Cakm", &["Nirmala UI"]),
    (*b"Cans", &["Gadugi", "Euphemia"]),
    (*b"Cari", &["Segoe UI Historic"]),
    // Chrome says "Plantagenet", which is not a family name on any Windows.
    (*b"Cher", &["Gadugi", "Plantagenet Cherokee"]),
    (*b"Copt", &["Segoe UI Symbol"]),
    (*b"Cprt", &["Segoe UI Historic"]),
    (*b"Cyrl", &["Times New Roman", "Segoe UI"]),
    (*b"Deva", &["Nirmala UI", "Mangal"]),
    (*b"Dsrt", &["Segoe UI Symbol"]),
    (*b"Egyp", &["Segoe UI Historic"]),
    (*b"Ethi", &["Nyala", "Ebrima"]),
    (*b"Geor", &["Sylfaen", "Segoe UI"]),
    (*b"Glag", &["Segoe UI Historic"]),
    (*b"Goth", &["Segoe UI Historic"]),
    (*b"Grek", &["Times New Roman", "Segoe UI"]),
    (*b"Gujr", &["Nirmala UI", "Shruti"]),
    (*b"Guru", &["Nirmala UI", "Raavi"]),
    (*b"Hang", &["Noto Sans KR", "Noto Sans CJK KR", "Malgun Gothic", "Gulim", "Batang"]),
    // The Han key asks its tradition's entry instead. This one gives
    // Unicode's `Hani` an entry, as the oracle requires of every script.
    (*b"Hani", &["Microsoft YaHei", "SimSun"]),
    (*b"Hans", &["Noto Sans SC", "Noto Sans CJK SC", "Microsoft YaHei", "SimSun"]),
    // Chrome pairs each of the historic scripts below with "Segoe UI Symbol"
    // as a second candidate. That was right on Windows 7, where Segoe UI
    // Symbol carried them; Windows 10 moved them to Segoe UI Historic and the
    // entries have covered nothing since. `tests/directwrite.rs` measures 0%.
    //
    // Chrome's entry reads "pmingli", a truncated family name that never
    // matches anything.
    (*b"Hant", &["Noto Sans TC", "Noto Sans CJK TC", "Microsoft JhengHei", "PMingLiU", "MingLiU"]),
    (*b"Hebr", &["David", "Segoe UI", "Arial"]),
    (*b"Hira", &["Noto Sans JP", "Noto Sans CJK JP", "Yu Gothic", "Meiryo", "MS PGothic"]),
    (*b"Hrkt", &["Noto Sans JP", "Noto Sans CJK JP", "Yu Gothic", "Meiryo", "MS PGothic"]),
    (*b"Ital", &["Segoe UI Historic"]),
    (*b"Java", &["Javanese Text"]),
    (*b"Jpan", &["Noto Sans JP", "Noto Sans CJK JP", "Yu Gothic", "Meiryo", "MS PGothic"]),
    (*b"Kana", &["Noto Sans JP", "Noto Sans CJK JP", "Yu Gothic", "Meiryo", "MS PGothic"]),
    (*b"Khar", &["Segoe UI Historic"]),
    (*b"Khmr", &["Leelawadee UI", "Khmer UI", "DaunPenh", "MoolBoran"]),
    // Chrome puts Tunga first. Nirmala UI is the shaping-aware modern font
    // and Windows has shipped it since 8; same for Orya, Sinh and Taml below.
    (*b"Knda", &["Nirmala UI", "Tunga"]),
    (*b"Kore", &["Noto Sans KR", "Noto Sans CJK KR", "Malgun Gothic", "Gulim", "Batang"]),
    (*b"Laoo", &["Leelawadee UI", "Lao UI", "DokChampa"]),
    (*b"Latn", &["Times New Roman", "Segoe UI", "Arial"]),
    (*b"Lisu", &["Segoe UI"]),
    (*b"Lyci", &["Segoe UI Historic"]),
    (*b"Lydi", &["Segoe UI Historic"]),
    (*b"Merc", &["Segoe UI Historic"]),
    (*b"Mlym", &["Nirmala UI", "Kartika"]),
    (*b"Mong", &["Mongolian Baiti"]),
    (*b"Mtei", &["Nirmala UI"]),
    (*b"Mymr", &["Myanmar Text", "Padauk"]),
    (*b"Nkoo", &["Ebrima"]),
    (*b"Ogam", &["Segoe UI Historic"]),
    (*b"Olck", &["Nirmala UI"]),
    (*b"Orkh", &["Segoe UI Historic"]),
    (*b"Orya", &["Nirmala UI", "Kalinga"]),
    // Gadugi gained Osage in Windows 10 1709. Chrome has no entry.
    (*b"Osge", &["Gadugi"]),
    (*b"Osma", &["Ebrima"]),
    (*b"Phag", &["Microsoft PhagsPa"]),
    (*b"Phli", &["Segoe UI Historic"]),
    (*b"Phnx", &["Segoe UI Historic"]),
    (*b"Prti", &["Segoe UI Historic"]),
    (*b"Runr", &["Segoe UI Historic"]),
    (*b"Sarb", &["Segoe UI Historic"]),
    (*b"Shaw", &["Segoe UI Historic"]),
    (*b"Sinh", &["Nirmala UI", "Iskoola Pota"]),
    (*b"Sora", &["Nirmala UI"]),
    (*b"Syrc", &["Estrangelo Edessa", "Segoe UI Historic"]),
    (*b"Tale", &["Microsoft Tai Le"]),
    (*b"Talu", &["Microsoft New Tai Lue"]),
    (*b"Taml", &["Nirmala UI", "Latha"]),
    (*b"Telu", &["Nirmala UI", "Gautami"]),
    (*b"Tfng", &["Ebrima"]),
    (*b"Thaa", &["MV Boli"]),
    (*b"Thai", &["Tahoma", "Leelawadee UI", "Leelawadee"]),
    (*b"Tibt", &["Microsoft Himalaya"]),
    (*b"Ugar", &["Segoe UI Historic"]),
    (*b"Vaii", &["Ebrima"]),
    (*b"Xpeo", &["Segoe UI Historic"]),
    (*b"Xsux", &["Segoe UI Historic"]),
    (*b"Yiii", &["Microsoft Yi Baiti"]),
    (*b"Zmth", &["Cambria Math", "Segoe UI Symbol"]),
    (*b"Zsye", &["Segoe UI Emoji", "Segoe UI Symbol"]),
    (*b"Zsym", &["Segoe UI Symbol", "Segoe UI Emoji"]),
];

/// Fonts known to cover `script`, best first. Empty if we have no entry.
pub fn script_fonts(script: Script) -> &'static [&'static str] {
    match SCRIPT_FONTS.binary_search_by_key(&key(script), |(tag, _)| u32::from_be_bytes(*tag)) {
        Ok(i) => SCRIPT_FONTS[i].1,
        Err(_) => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_nonempty() {
        for pair in SCRIPT_FONTS.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "table out of order at {:?} / {:?}",
                Script::from_bytes(pair[0].0),
                Script::from_bytes(pair[1].0)
            );
        }
        assert!(SCRIPT_FONTS.iter().all(|(_, f)| !f.is_empty()));
    }

    #[test]
    fn lookup_hits_and_misses() {
        assert_eq!(
            script_fonts(Script::from_bytes(*b"Deva")),
            &["Nirmala UI", "Mangal"]
        );
        assert!(script_fonts(Script::parse("Xyzw").unwrap()).is_empty());
    }
}
