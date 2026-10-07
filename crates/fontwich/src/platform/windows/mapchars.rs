//! DirectWrite's system fallback as a table: the family
//! `IDWriteFontFallback::MapCharacters` draws each character with, for the
//! characters Chrome's own lists leave to it.
//!
//! Chrome on Windows asks `MapCharacters` for a character its hardcoded
//! choices miss, before its pan-Unicode lists. Here a missed character's key
//! and the Common key come first, and the characters neither answers on the
//! reference machine are the table's: Windows 11's Sans Serif Collection for
//! the historic scripts, the supplementary-plane Han fonts, and the private
//! use characters a few fonts map. One table for text with no Han tradition,
//! and for each tradition (Chrome passes the page's locale) only where its
//! answer differs.
//!
//! It is one Windows build's fonts, recorded: a family it names that a
//! machine lacks is skipped by the collection, and DirectWrite is then asked
//! at run time ([`directwrite::map_character`](super::directwrite)), before
//! the walk of every installed family that backstops both.
//!
//! **Regenerating.** On Windows, with the fonts of the build to record:
//! `cargo test -p fontwich --release --lib -- --ignored regenerate_mapchars_table`
//! rewrites `mapchars_table.rs`, with the build in its header. `git diff`
//! shows what moved. Every family it names must be one DirectWrite lists, and
//! at most 254 of them.

use crate::fallback::Han;

/// No family: DirectWrite maps nothing here.
const NONE: u8 = 0xFF;
/// In a tradition's table: the base table's answer.
const BASE_ANSWER: u8 = 0xFE;

/// The family the table names for `c` in text of `han`'s tradition, or of
/// none.
pub(crate) fn family(c: char, han: Option<Han>) -> Option<&'static str> {
    let c = u32::from(c);
    let base = || answer(BASE, c, NONE);
    let at = match han {
        None => base(),
        Some(han) => match answer(TRADITIONS[tradition(han)], c, BASE_ANSWER) {
            BASE_ANSWER => base(),
            at => at,
        },
    };
    FAMILIES.get(usize::from(at)).copied()
}

/// The locale DirectWrite is asked in for text of `han`'s tradition, or of
/// none: Chrome passes the page's locale, and its Han locale for Han.
// Asked of DirectWrite, which only a Windows build with `system` has.
#[cfg_attr(not(all(windows, feature = "system")), allow(dead_code))]
pub(crate) fn locale(han: Option<Han>) -> &'static str {
    match han {
        None => "en-us",
        Some(Han::Hans) => "zh-cn",
        Some(Han::Hant) => "zh-tw",
        Some(Han::HantHK) => "zh-hk",
        Some(Han::Jpan) => "ja-jp",
        Some(Han::Kore) => "ko-kr",
    }
}

/// Where `han`'s table is in [`TRADITIONS`].
fn tradition(han: Han) -> usize {
    match han {
        Han::Hans => 0,
        Han::Hant => 1,
        Han::HantHK => 2,
        Han::Jpan => 3,
        Han::Kore => 4,
    }
}

/// The family index the run holding `c` names, from runs of `start << 8 |
/// family`, each lasting until the next begins; `before` ahead of the first.
fn answer(runs: &[u32], c: u32, before: u8) -> u8 {
    match runs.partition_point(|&run| run >> 8 <= c) {
        0 => before,
        n => runs[n - 1] as u8,
    }
}

include!("mapchars_table.rs");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_are_in_order_and_name_families() {
        for runs in core::iter::once(BASE).chain(TRADITIONS) {
            for pair in runs.windows(2) {
                assert!(pair[0] >> 8 < pair[1] >> 8, "{:X} {:X}", pair[0], pair[1]);
            }
            for &run in runs {
                let at = run as u8;
                assert!(
                    usize::from(at) < FAMILIES.len() || at == NONE || at == BASE_ANSWER,
                    "{run:X}"
                );
            }
        }
        assert!(BASE.iter().all(|&run| run as u8 != BASE_ANSWER));
    }

    #[test]
    fn a_tradition_without_its_own_answer_takes_the_base_one() {
        // Samaritan, which Windows 11 draws from Sans Serif Collection in
        // every locale; and a Latin letter, which the keys answer.
        for han in [None, Some(Han::Jpan), Some(Han::HantHK)] {
            assert_eq!(family('\u{0800}', han), Some("Sans Serif Collection"));
            assert_eq!(family('A', han), None);
        }
    }

    #[test]
    fn every_character_has_an_answer_or_none() {
        for c in (0..=0x10FFFF).filter_map(char::from_u32) {
            for han in [
                None,
                Some(Han::Hans),
                Some(Han::Hant),
                Some(Han::HantHK),
                Some(Han::Jpan),
                Some(Han::Kore),
            ] {
                let _ = family(c, han);
            }
        }
    }
}

/// Regenerates `mapchars_table.rs` from this machine's DirectWrite.
#[cfg(all(test, windows, feature = "system"))]
mod generate {
    use alloc::string::String;
    use alloc::vec::Vec;
    use std::format;

    use icu_properties::CodePointMapData;
    use icu_properties::props::GeneralCategory;

    use super::*;
    use crate::fallback::Presentation;
    use crate::{Collection, FallbackRequest, GenericClass, parse_language};

    /// What the table records for a character in one tradition.
    #[derive(Copy, Clone, PartialEq, Eq)]
    enum Recorded {
        /// The character's key or the Common key already maps it, or it is
        /// unassigned: whatever its neighbours have.
        Free,
        /// A family index, or [`NONE`].
        At(u8),
    }

    /// Runs over `values`, changing only where a recorded value differs from
    /// the run it is in: a free character takes its neighbours'.
    fn runs(values: &[Recorded], first: u8) -> Vec<u32> {
        let mut runs = Vec::new();
        let mut current = first;
        for (c, value) in values.iter().enumerate() {
            if let Recorded::At(at) = *value
                && at != current
            {
                runs.push((c as u32) << 8 | u32::from(at));
                current = at;
            }
        }
        runs
    }

    /// The family index `runs` gives `c`, as [`answer`] reads them.
    fn read(runs: &[u32], c: u32, before: u8) -> u8 {
        answer(runs, c, before)
    }

    #[test]
    #[ignore]
    fn regenerate_mapchars_table() {
        let categories = CodePointMapData::<GeneralCategory>::new();
        let collection = Collection::system();
        let traditions = [
            None,
            Some(Han::Hans),
            Some(Han::Hant),
            Some(Han::HantHK),
            Some(Han::Jpan),
            Some(Han::Kore),
        ];
        let mut families: Vec<String> = Vec::new();
        let mut recorded: Vec<Vec<Recorded>> = Vec::new();
        let mut asked = 0usize;
        for han in traditions {
            let language = han.and_then(|han| parse_language(locale(Some(han))));
            let request = FallbackRequest::Text {
                script: crate::Script::COMMON,
                language,
                generic: GenericClass::Plain,
            };
            let mut values = alloc::vec![Recorded::Free; 0x11_0000];
            for c in (0..=0x10FFFF).filter_map(char::from_u32) {
                let category = categories.get(c);
                if matches!(
                    category,
                    GeneralCategory::Unassigned | GeneralCategory::Control
                ) {
                    continue;
                }
                let (keys, _) =
                    crate::fallback::miss_keys(&collection, c, Presentation::Text, &request);
                let answered = keys.iter().flatten().any(|key| {
                    collection
                        .fallback(key)
                        .iter()
                        .any(|family| family.covers(c))
                });
                if answered {
                    continue;
                }
                asked += 1;
                let mut at = NONE;
                super::super::directwrite::map_character(c, locale(han), &mut |name| {
                    let Some(family) = collection.fallback_family(name) else {
                        std::println!("U+{:04X}: {name} is not listed", u32::from(c));
                        return;
                    };
                    if !family.covers(c) {
                        std::println!("U+{:04X}: {name} does not map it", u32::from(c));
                        return;
                    }
                    let name = String::from(family.name());
                    let index = match families.iter().position(|known| *known == name) {
                        Some(index) => index,
                        None => {
                            families.push(name);
                            families.len() - 1
                        }
                    };
                    at = u8::try_from(index)
                        .ok()
                        .filter(|&at| at < BASE_ANSWER)
                        .expect("at most 254 families");
                });
                values[u32::from(c) as usize] = Recorded::At(at);
            }
            recorded.push(values);
        }

        let base = runs(&recorded[0], NONE);
        let mut traditions_runs = Vec::new();
        for values in &recorded[1..] {
            let differences: Vec<Recorded> = values
                .iter()
                .enumerate()
                .map(|(c, value)| match *value {
                    Recorded::At(at) if at == read(&base, c as u32, NONE) => {
                        Recorded::At(BASE_ANSWER)
                    }
                    other => other,
                })
                .collect();
            traditions_runs.push(runs(&differences, BASE_ANSWER));
        }

        // Every recorded answer reads back.
        for (values, han) in recorded.iter().zip(traditions) {
            for (c, value) in values.iter().enumerate() {
                if let Recorded::At(at) = *value {
                    let c = char::from_u32(c as u32).expect("a character");
                    let base = read(&base, u32::from(c), NONE);
                    let got = match han {
                        None => base,
                        Some(han) => {
                            match read(&traditions_runs[tradition(han)], u32::from(c), BASE_ANSWER)
                            {
                                BASE_ANSWER => base,
                                at => at,
                            }
                        }
                    };
                    assert_eq!(got, at, "U+{:04X} in {han:?}", u32::from(c));
                }
            }
        }

        let build = std::process::Command::new("cmd")
            .args(["/c", "ver"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().into())
            .unwrap_or_else(|_| String::from("an unknown build"));
        let mut out = String::new();
        out += "// Generated by `mapchars::generate::regenerate_mapchars_table`. Do not edit by hand.\n";
        out += &format!("// {build}: {asked} characters asked of `MapCharacters`.\n\n");
        out += "/// The families the table names, by index.\n#[rustfmt::skip]\nstatic FAMILIES: &[&str] = &[\n";
        for name in &families {
            out += &format!("    {name:?},\n");
        }
        out += "];\n\n";
        let table = |out: &mut String, runs: &[u32]| {
            for chunk in runs.chunks(8) {
                let line: Vec<String> = chunk.iter().map(|run| format!("0x{run:08X}")).collect();
                *out += &format!("    {},\n", line.join(", "));
            }
        };
        out += "/// Text with no Han tradition: runs of `start << 8 | family`, each until\n/// the next, `0xFF` for none.\n#[rustfmt::skip]\nstatic BASE: &[u32] = &[\n";
        table(&mut out, &base);
        out += "];\n\n";
        out += "/// Each Han tradition's runs where it differs, Hans, Hant, HantHK, Jpan\n/// and Kore, `0xFE` for the base table's answer.\n#[rustfmt::skip]\nstatic TRADITIONS: [&[u32]; 5] = [\n";
        for runs in &traditions_runs {
            out += "    &[\n";
            let mut inner = String::new();
            table(&mut inner, runs);
            for line in inner.lines() {
                out += &format!("    {line}\n");
            }
            out += "    ],\n";
        }
        out += "];\n";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/platform/windows/mapchars_table.rs");
        std::fs::write(&path, out).expect("writes the table");
        std::println!(
            "{} families, {} base runs, {:?} tradition runs, {asked} asked",
            families.len(),
            base.len(),
            traditions_runs.iter().map(Vec::len).collect::<Vec<_>>()
        );
    }
}
