//! The characters every script shares: Unicode's Common and Inherited, and
//! which scripts each one belongs to by Script_Extensions.
//!
//! A shaper merges these into whatever run surrounds them, so `。` after Latin
//! text arrives in a Latin run and `।` after an English word in whatever run
//! that word was. A run's fallback is keyed on the run's script, so these
//! are the characters it can be asked for without having been built for
//! them, and a miss on one asks the scripts Script_Extensions gives it.
//!
//! Only characters a font could draw are listed: not controls, format
//! characters, separators, surrogates, private use, unassigned code points or
//! variation selectors.
//!
//! Beside them, every other script's own characters, by the Script property:
//! what a run of that script holds besides the shared ones. Unassigned code
//! points between two ranges of one script are counted as that script's,
//! since no font maps them and the table is a third the size for it.
//!
//! `generated_tables_still_match_unicode` regenerates every table and fails if
//! they drift; `print_tables` prints them.

/// The scripts `c` belongs to beyond Common and Inherited, by
/// Script_Extensions. Empty for a character every script uses alike.
pub(super) fn extensions(c: u32) -> &'static [[u8; 4]] {
    let at = EXTENSIONS.partition_point(|&(_, end, _)| end < c);
    match EXTENSIONS.get(at) {
        Some(&(start, _, set)) if start <= c => SETS[set as usize],
        _ => &[],
    }
}

/// The characters of the script tagged `tag`, as inclusive ranges in order.
/// Empty for a tag no character has.
#[cfg(test)]
fn script_ranges(tag: [u8; 4]) -> impl Iterator<Item = (u32, u32)> {
    let index = SCRIPT_TAGS.binary_search(&tag).ok();
    SCRIPTS
        .iter()
        .filter(move |&&(_, _, script)| Some(usize::from(script)) == index)
        .map(|&(start, end, _)| (start, end))
}

/// The script `c` belongs to by the Script property, or `None` for Common,
/// Inherited and Unknown.
pub(crate) fn script(c: u32) -> Option<[u8; 4]> {
    let at = SCRIPTS.partition_point(|&(_, end, _)| end < c);
    match SCRIPTS.get(at) {
        Some(&(start, _, index)) if start <= c => SCRIPT_TAGS.get(usize::from(index)).copied(),
        _ => None,
    }
}

/// Every script a character has, beyond Common and Inherited, sorted.
pub(super) fn script_tags() -> &'static [[u8; 4]; SCRIPT_COUNT] {
    &SCRIPT_TAGS
}

include!("unicode_table.rs");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::HashMap;
    use alloc::vec::Vec;
    use icu_properties::CodePointMapData;
    use icu_properties::props::{GeneralCategory, NamedEnumeratedProperty, Script as IcuScript};
    use icu_properties::script::ScriptWithExtensions;
    use parlance::Script;

    fn extends_to(c: u32, script: Script) -> bool {
        extensions(c).contains(&script.to_bytes())
    }

    type Tables = (Vec<(u32, u32)>, Vec<(u32, u32, usize)>, Vec<Vec<[u8; 4]>>);

    /// Both tables, from Unicode as icu_properties has it.
    fn generate() -> Tables {
        let scripts = CodePointMapData::<IcuScript>::new();
        let categories = CodePointMapData::<GeneralCategory>::new();
        let scx = ScriptWithExtensions::new();
        let drawable = |c: char| {
            !matches!(
                categories.get(c),
                GeneralCategory::Unassigned
                    | GeneralCategory::Control
                    | GeneralCategory::Format
                    | GeneralCategory::Surrogate
                    | GeneralCategory::PrivateUse
                    | GeneralCategory::SpaceSeparator
                    | GeneralCategory::LineSeparator
                    | GeneralCategory::ParagraphSeparator
            ) && !matches!(u32::from(c), 0xFE00..=0xFE0F | 0xE0100..=0xE01EF)
        };
        let mut shared: Vec<(u32, u32)> = Vec::new();
        let mut extensions: Vec<(u32, u32, usize)> = Vec::new();
        let mut sets: Vec<Vec<[u8; 4]>> = Vec::new();
        let mut index: HashMap<Vec<[u8; 4]>, usize> = HashMap::default();
        for c in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
            if !drawable(c) || !matches!(scripts.get(c).short_name(), "Zyyy" | "Zinh") {
                continue;
            }
            let c32 = u32::from(c);
            match shared.last_mut() {
                Some((_, end)) if *end + 1 == c32 => *end = c32,
                _ => shared.push((c32, c32)),
            }
            let mut set: Vec<[u8; 4]> = scx
                .get_script_extensions_val(c)
                .iter()
                .map(|script| {
                    let name = script.short_name();
                    name.as_bytes().try_into().expect("four letters")
                })
                .filter(|tag| tag != b"Zyyy" && tag != b"Zinh")
                .collect();
            if set.is_empty() {
                continue;
            }
            set.sort_unstable();
            let next = sets.len();
            let at = *index.entry(set.clone()).or_insert_with(|| {
                sets.push(set);
                next
            });
            match extensions.last_mut() {
                Some((_, end, last)) if *end + 1 == c32 && *last == at => *end = c32,
                _ => extensions.push((c32, c32, at)),
            }
        }
        (shared, extensions, sets)
    }

    /// Every script's ranges but Common's, Inherited's and Unknown's, with
    /// unassigned gaps inside one script's run absorbed into it.
    /// Script ranges, each with an index into the tags, and the tags, sorted.
    type Scripts = (Vec<(u32, u32, usize)>, Vec<[u8; 4]>);

    fn generate_scripts() -> Scripts {
        let scripts = CodePointMapData::<IcuScript>::new();
        let mut ranges: Vec<(u32, u32, usize)> = Vec::new();
        let mut tags: Vec<[u8; 4]> = Vec::new();
        // The script of the run so far, if an unassigned gap could still
        // belong to it.
        let mut open: Option<usize> = None;
        for c in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
            let tag: [u8; 4] = scripts
                .get(c)
                .short_name()
                .as_bytes()
                .try_into()
                .expect("four letters");
            match &tag {
                b"Zyyy" | b"Zinh" => open = None,
                b"Zzzz" => {}
                _ => {
                    let index = tags.iter().position(|&t| t == tag).unwrap_or_else(|| {
                        tags.push(tag);
                        tags.len() - 1
                    });
                    match ranges.last_mut() {
                        Some((_, end, last)) if open == Some(index) && *last == index => {
                            *end = u32::from(c);
                        }
                        _ => ranges.push((u32::from(c), u32::from(c), index)),
                    }
                    open = Some(index);
                }
            }
        }
        // Sorted, so that a tag is found by binary search and its index is
        // its `ScriptId`.
        let mut sorted = tags.clone();
        sorted.sort_unstable();
        let at = |index: usize| sorted.binary_search(&tags[index]).expect("a tag");
        let ranges = ranges
            .into_iter()
            .map(|(start, end, index)| (start, end, at(index)))
            .collect();
        (ranges, sorted)
    }

    #[test]
    fn every_assigned_character_is_found_under_its_script() {
        let scripts = CodePointMapData::<IcuScript>::new();
        let (ranges, tags) = generate_scripts();
        assert_eq!(
            SCRIPTS
                .iter()
                .map(|&(start, end, index)| (start, end, SCRIPT_TAGS[index as usize]))
                .collect::<Vec<_>>(),
            ranges
                .iter()
                .map(|&(start, end, index)| (start, end, tags[index]))
                .collect::<Vec<_>>(),
            "SCRIPTS has drifted; see print_tables"
        );
        for c in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
            let name = scripts.get(c).short_name();
            if matches!(name, "Zyyy" | "Zinh" | "Zzzz") {
                continue;
            }
            let tag: [u8; 4] = name.as_bytes().try_into().expect("four letters");
            let c = u32::from(c);
            assert!(
                script_ranges(tag).any(|(start, end)| start <= c && c <= end),
                "U+{c:04X} is not found under {name}"
            );
        }
    }

    #[test]
    fn the_script_tags_are_sorted_and_distinct() {
        // Binary search, and so `ScriptId`, needs both.
        assert!(SCRIPT_TAGS.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn generated_tables_still_match_unicode() {
        let (shared, extensions, sets) = generate();
        assert_eq!(SHARED, &shared[..], "SHARED has drifted; see print_tables");
        let ours: Vec<(u32, u32, Vec<[u8; 4]>)> = EXTENSIONS
            .iter()
            .map(|&(start, end, set)| (start, end, SETS[set as usize].to_vec()))
            .collect();
        let theirs: Vec<(u32, u32, Vec<[u8; 4]>)> = extensions
            .iter()
            .map(|&(start, end, set)| (start, end, sets[set].clone()))
            .collect();
        assert_eq!(ours, theirs, "EXTENSIONS has drifted; see print_tables");
    }

    #[test]
    fn the_ideographic_full_stop_extends_to_han_and_kana_and_not_latin() {
        assert!(extends_to(0x3002, Script::from_bytes(*b"Hani")));
        assert!(extends_to(0x3002, Script::from_bytes(*b"Hira")));
        assert!(!extends_to(0x3002, Script::from_bytes(*b"Latn")));
        assert!(extensions(u32::from('→')).is_empty());
        assert!(extends_to(0x0964, Script::from_bytes(*b"Deva")));
    }

    /// Prints both tables for `unicode_table.rs`.
    #[test]
    #[ignore]
    #[cfg(feature = "std")]
    fn print_tables() {
        use std::println;
        let (shared, extensions, sets) = generate();
        println!("// Generated by `unicode::tests::print_tables`. Do not edit by hand.");
        println!();
        println!("/// Drawable Common and Inherited characters, as inclusive ranges.");
        println!("#[cfg(test)]");
        println!("#[rustfmt::skip]");
        println!("static SHARED: &[(u32, u32)] = &[");
        for chunk in shared.chunks(6) {
            let line: Vec<std::string::String> = chunk
                .iter()
                .map(|(s, e)| std::format!("(0x{s:04X}, 0x{e:04X})"))
                .collect();
            println!("    {},", line.join(", "));
        }
        println!("];");
        println!();
        println!("/// Ranges of `SHARED` with Script_Extensions, and which of [`SETS`].");
        println!("#[rustfmt::skip]");
        println!("static EXTENSIONS: &[(u32, u32, u8)] = &[");
        for chunk in extensions.chunks(5) {
            let line: Vec<std::string::String> = chunk
                .iter()
                .map(|(s, e, i)| std::format!("(0x{s:04X}, 0x{e:04X}, {i})"))
                .collect();
            println!("    {},", line.join(", "));
        }
        println!("];");
        println!();
        let (scripts, tags) = generate_scripts();
        println!("/// Every other script's characters, as inclusive ranges, and which of");
        println!("/// [`SCRIPT_TAGS`] each is.");
        println!("#[rustfmt::skip]");
        println!("static SCRIPTS: &[(u32, u32, u8)] = &[");
        for chunk in scripts.chunks(5) {
            let line: Vec<std::string::String> = chunk
                .iter()
                .map(|(s, e, i)| std::format!("(0x{s:04X}, 0x{e:04X}, {i})"))
                .collect();
            println!("    {},", line.join(", "));
        }
        println!("];");
        println!();
        println!("/// How many scripts [`SCRIPTS`] names.");
        println!("pub(crate) const SCRIPT_COUNT: usize = {};", tags.len());
        println!();
        println!("/// The scripts [`SCRIPTS`] names, sorted.");
        println!("#[rustfmt::skip]");
        println!("static SCRIPT_TAGS: [[u8; 4]; SCRIPT_COUNT] = [");
        for chunk in tags.chunks(10) {
            let line: Vec<std::string::String> = chunk
                .iter()
                .map(|tag| std::format!("*b\"{}\"", core::str::from_utf8(tag).expect("ascii")))
                .collect();
            println!("    {},", line.join(", "));
        }
        println!("];");
        println!();
        println!("/// Each distinct Script_Extensions set, sorted by tag.");
        println!("#[rustfmt::skip]");
        println!("static SETS: &[&[[u8; 4]]] = &[");
        for set in &sets {
            let tags: Vec<std::string::String> = set
                .iter()
                .map(|tag| std::format!("*b\"{}\"", core::str::from_utf8(tag).expect("ascii")))
                .collect();
            println!("    &[{}],", tags.join(", "));
        }
        println!("];");
    }
}
