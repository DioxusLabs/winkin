//! Android's `fonts.xml`, as data.
//!
//! This is the parse only: what the file says, not what it means. Turning a
//! `lang` into a [`Script`](parlance::Script), or a file into a family name,
//! happens above it.
//!
//! # What is kept
//!
//! Less than the file holds. `fontwich` answers in family names and does not
//! pick fonts within a family — weight, slant and width are a separate algorithm over a font
//! collection, which is why [`FallbackRequest`](crate::FallbackRequest) has no style axes — so
//! `weight`, `style`, `<axis>` and `variant` are read past. What is left is
//! what decides *which family* is named:
//!
//! - `name`, on the ten or so named families, which is where the generics
//!   live. Everything else is anonymous.
//! - `lang`, which is a script tag in `und-Arab` form far more often than it
//!   is a language.
//! - the file and its `index`, because that pair is the only handle on a
//!   family name: an anonymous family names no family, and the name has to
//!   come from the font binary. `index` is not decoration — `zh-Hans`, `ja`
//!   and `ko` are all `NotoSansCJK-Regular.ttc` and differ only by it.
//! - `fallbackFor`, which is how one `<family>` holds two families: the
//!   `und-Ethi` element carries both Noto Sans Ethiopic and, marked
//!   `fallbackFor="serif"`, Noto Serif Ethiopic.
//!
//! `postScriptName` is read past deliberately. It is *nearly* a family name
//! — `NotoSerifEthiopic-Regular` against Noto Serif Ethiopic — and that is
//! exactly what makes it a trap. The name comes from the font or it does
//! not come.
//!
//! # What it is checked against
//!
//! `tests/fixtures/android/`, which is AOSP's own file at three releases. Its
//! README says what they cover and, more usefully, what they do not.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// One `<font>`: a font, and what it is offered for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Font {
    /// The file name, with no directory. Resolved against the system font
    /// directory by the caller, since only it knows where that is.
    pub file: String,
    /// Which font within a collection file. Zero unless the file says otherwise.
    pub index: u32,
    /// The generic this font answers for, when it is not the family's
    /// default one — `serif`, in every case seen so far.
    pub fallback_for: Option<String>,
    /// Whether `<axis>` children pin it somewhere in a variable font. The
    /// values themselves are read past — picking within a family is a
    /// separate algorithm — but a family whose fonts are pinned is a *point*
    /// of another family rather than one of its own, which is what
    /// [`Android::names`](super::Android::names) has to know:
    /// `sans-serif-condensed` is Roboto at `wdth` 75.
    pub varied: bool,
}

/// One `<family>`.
///
/// Almost all of them are anonymous: of the 160 in AOSP's current file, ten
/// have a name. A name means a generic; the rest are reached through `langs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XmlFamily {
    /// `sans-serif`, `serif`, `monospace` and the like. `None` for a fallback
    /// family, which is the overwhelming majority.
    pub name: Option<String>,
    /// The `lang` attribute, split on commas. Usually one tag; `und-Geor,
    /// und-Geok` and `zh-Hant,zh-Bopo` are the two that are not. Empty for a
    /// named family, and for the one anonymous family that carries the
    /// subsetted symbols font.
    pub langs: Vec<String>,
    /// In file order, which is priority order.
    pub fonts: Vec<Font>,
}

/// One `<alias>`: another name for a named family.
///
/// Skia leans on these — its NDK font manager reads the XML *only* for them,
/// the NDK having no notion of a generic — and
/// [`Index::aliases`](super::index::Index::aliases) hands them to a layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Alias {
    pub name: String,
    pub to: String,
    /// The weight `<alias weight="300">` asks of the family it points at.
    /// Eight of AOSP's twenty-four have one; they name a weight of a family
    /// rather than the family, which a name alone cannot say.
    pub weight: Option<u16>,
}

/// A parsed `fonts.xml`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Config {
    /// In file order. The order is the fallback order, so it must be kept.
    pub families: Vec<XmlFamily>,
    pub aliases: Vec<Alias>,
}

/// Why a `fonts.xml` could not be read.
#[derive(Debug)]
pub(super) enum Error {
    /// The bytes are not well-formed XML.
    Xml(roxmltree::Error),
    /// Well-formed, but not this file: the root element is not `familyset`.
    /// Worth distinguishing, because it is what reading the wrong path looks
    /// like.
    NotFontsXml,
}

impl From<roxmltree::Error> for Error {
    fn from(error: roxmltree::Error) -> Self {
        Self::Xml(error)
    }
}

/// Parses `fonts.xml`.
///
/// Unknown elements and attributes are ignored rather than refused. The
/// format has gained both within the releases we have — Android 13 added
/// `ignore` and `postScriptName` — and a file from a version we predate is
/// the normal case on a device, not an error.
pub(crate) fn parse(xml: &str) -> Result<Config, Error> {
    let document = roxmltree::Document::parse(xml)?;
    let root = document.root_element();
    if !root.has_tag_name("familyset") {
        return Err(Error::NotFontsXml);
    }

    let mut config = Config::default();
    for node in root.children().filter(roxmltree::Node::is_element) {
        match node.tag_name().name() {
            // Android 13 and later mark a family the system should skip.
            // Keeping one would name a font the platform will not use.
            "family" if node.attribute("ignore") != Some("true") => {
                if let Some(family) = family(node) {
                    config.families.push(family);
                }
            }
            "alias" => {
                if let (Some(name), Some(to)) = (node.attribute("name"), node.attribute("to")) {
                    config.aliases.push(Alias {
                        name: name.to_string(),
                        to: to.to_string(),
                        weight: node
                            .attribute("weight")
                            .and_then(|weight| weight.parse().ok()),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(config)
}

/// One `<family>`, or `None` when it names no usable font.
fn family(node: roxmltree::Node<'_, '_>) -> Option<XmlFamily> {
    let fonts: Vec<Font> = node
        .children()
        .filter(|child| child.is_element() && child.tag_name().name() == "font")
        .filter_map(font)
        .collect();
    if fonts.is_empty() {
        return None;
    }
    Some(XmlFamily {
        name: node.attribute("name").map(str::to_string),
        langs: node
            .attribute("lang")
            .into_iter()
            .flat_map(|langs| langs.split(','))
            .map(str::trim)
            .filter(|lang| !lang.is_empty())
            .map(str::to_string)
            .collect(),
        fonts,
    })
}

/// One `<font>`, or `None` when it names no file.
fn font(node: roxmltree::Node<'_, '_>) -> Option<Font> {
    // The file name is text, and `<axis>` children mean it is *mixed* with
    // elements. It also sits on its own line as often as not, so both ends
    // need trimming:
    //
    //     <font weight="400" style="normal" index="2">
    //         NotoSansCJK-Regular.ttc
    //         <axis tag="wght" stylevalue="400"/>
    //     </font>
    let file = node.text().map(str::trim).filter(|file| !file.is_empty())?;
    Some(Font {
        file: file.to_string(),
        // A malformed index is treated as absent. The alternative is
        // refusing a whole file over one attribute, on a device where this
        // is the only font configuration there is.
        index: node
            .attribute("index")
            .and_then(|index| index.parse().ok())
            .unwrap_or(0),
        fallback_for: node.attribute("fallbackFor").map(str::to_string),
        varied: node
            .children()
            .any(|child| child.is_element() && child.has_tag_name("axis")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const AOSP_MAIN: &str = include_str!("../../../tests/fixtures/android/fonts-aosp-main.xml");
    const ANDROID_13: &str = include_str!("../../../tests/fixtures/android/fonts-android-13.xml");
    const ANDROID_10: &str = include_str!("../../../tests/fixtures/android/fonts-android-10.xml");

    fn all() -> [(&'static str, Config); 3] {
        [
            ("main", parse(AOSP_MAIN).expect("main parses")),
            ("13", parse(ANDROID_13).expect("13 parses")),
            ("10", parse(ANDROID_10).expect("10 parses")),
        ]
    }

    #[test]
    fn every_fixture_parses_and_is_populated() {
        for (release, config) in all() {
            assert!(
                config.families.len() > 100,
                "{release} yielded only {} families",
                config.families.len()
            );
            assert_eq!(config.aliases.len(), 24, "{release} alias count");
            assert!(
                config
                    .families
                    .iter()
                    .all(|family| !family.fonts.is_empty()),
                "{release} kept a family with no fonts"
            );
        }
    }

    #[test]
    fn the_generics_are_the_named_families() {
        for (release, config) in all() {
            let named: Vec<&str> = config
                .families
                .iter()
                .filter_map(|family| family.name.as_deref())
                .collect();
            for generic in ["sans-serif", "serif", "monospace"] {
                assert!(named.contains(&generic), "{release} has no {generic}");
            }
            // The point of the whole exercise: the per-script families, which
            // is nearly all of them, name no family at all.
            assert!(
                named.len() < 12 && config.families.len() - named.len() > 100,
                "{release}: {} named of {}",
                named.len(),
                config.families.len()
            );
        }
    }

    #[test]
    fn a_lang_attribute_of_several_tags_is_split() {
        let config = parse(AOSP_MAIN).expect("main parses");
        let georgian = config
            .families
            .iter()
            .find(|family| family.langs.iter().any(|lang| lang == "und-Geor"))
            .expect("a Georgian family");
        assert_eq!(georgian.langs, ["und-Geor", "und-Geok"]);

        let han = config
            .families
            .iter()
            .find(|family| family.langs.iter().any(|lang| lang == "zh-Bopo"))
            .expect("a Bopomofo family");
        assert_eq!(han.langs, ["zh-Hant", "zh-Bopo"]);
    }

    #[test]
    fn a_collection_index_survives() {
        // zh-Hans, ja and ko are all NotoSansCJK-Regular.ttc and differ only
        // by index, so dropping it would collapse them into one font.
        let config = parse(AOSP_MAIN).expect("main parses");
        let indices: Vec<u32> = config
            .families
            .iter()
            .filter(|family| family.langs.iter().any(|lang| lang.starts_with("zh-Hans")))
            .flat_map(|family| family.fonts.iter())
            .filter(|font| font.file.ends_with(".ttc"))
            .map(|font| font.index)
            .collect();
        assert!(!indices.is_empty(), "no CJK collection found");
        assert!(
            indices.iter().all(|&index| index == 2),
            "zh-Hans should be index 2 throughout, got {indices:?}"
        );
    }

    #[test]
    fn fallback_for_marks_the_serif_font_within_a_family() {
        // One <family> holding two families is the case that would be lost by
        // treating the element as the unit.
        let config = parse(AOSP_MAIN).expect("main parses");
        let ethiopic = config
            .families
            .iter()
            .find(|family| family.langs.iter().any(|lang| lang == "und-Ethi"))
            .expect("an Ethiopic family");
        assert!(
            ethiopic
                .fonts
                .iter()
                .any(|font| font.fallback_for.as_deref() == Some("serif")),
            "no serif font marked in und-Ethi"
        );
        assert!(
            ethiopic
                .fonts
                .iter()
                .any(|font| font.fallback_for.is_none()),
            "no default font left in und-Ethi"
        );
    }

    #[test]
    fn an_ignored_family_is_dropped() {
        // Android 13 introduced `ignore`; 10 has none, so this also pins that
        // parsing one without the attribute is unaffected.
        for (release, source) in [("main", AOSP_MAIN), ("13", ANDROID_13), ("10", ANDROID_10)] {
            let ignored = source.matches("ignore=\"true\"").count();
            let config = parse(source).expect("parses");
            let doc = roxmltree::Document::parse(source).expect("parses");
            let present = doc
                .root_element()
                .children()
                .filter(|node| node.is_element() && node.tag_name().name() == "family")
                .count();
            assert_eq!(
                config.families.len(),
                present - ignored,
                "{release}: {ignored} ignored of {present} were not dropped"
            );
        }
    }

    #[test]
    fn the_symbols_family_has_neither_name_nor_lang() {
        // It is the one anonymous family with no lang, and it is the same
        // entry AOSP's newer fallback_order.json names by id rather than by
        // language. Losing it would lose the symbol fonts.
        let config = parse(AOSP_MAIN).expect("main parses");
        let orphans: Vec<&XmlFamily> = config
            .families
            .iter()
            .filter(|family| family.name.is_none() && family.langs.is_empty())
            .collect();
        assert_eq!(orphans.len(), 1, "expected exactly one, got {orphans:?}");
        assert!(
            orphans[0].fonts[0].file.starts_with("NotoSansSymbols"),
            "unexpected orphan: {:?}",
            orphans[0].fonts[0].file
        );
    }

    #[test]
    fn file_names_are_trimmed_of_the_markup_around_them() {
        for (release, config) in all() {
            for family in &config.families {
                for font in &family.fonts {
                    assert_eq!(font.file, font.file.trim(), "{release}: untrimmed");
                    assert!(
                        !font.file.contains(char::is_whitespace),
                        "{release}: {:?} kept whitespace, so the text ran into an <axis>",
                        font.file
                    );
                    assert!(
                        font.file.ends_with(".ttf")
                            || font.file.ends_with(".ttc")
                            || font.file.ends_with(".otf"),
                        "{release}: {:?} does not look like a font file",
                        font.file
                    );
                }
            }
        }
    }

    #[test]
    fn the_wrong_file_is_told_apart_from_a_broken_one() {
        assert!(matches!(
            parse("<html><body/></html>"),
            Err(Error::NotFontsXml)
        ));
        assert!(matches!(parse("<familyset"), Err(Error::Xml(_))));
        // An empty familyset is a valid answer, not an error: it is what a
        // stripped-down or vendor-emptied file looks like.
        let empty = parse("<familyset version=\"23\"/>").expect("empty parses");
        assert!(empty.families.is_empty() && empty.aliases.is_empty());
    }
}
