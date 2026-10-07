//! The parse, arranged for lookup.
//!
//! Building this touches no files. `fonts.xml` says which fonts answer for a
//! script and which answer for a generic, and that arrangement is all static
//! once the XML is read — only turning a font into a *family name* costs
//! anything, and that is deferred to [`FamilyNames`](super::FamilyNames).

use alloc::boxed::Box;
use alloc::vec::Vec;

use parlance::Script;

use crate::fallback::{Han, parse_language, resolve_han};
use crate::platform::android::config::{Config, Font};
use crate::script as sc;

/// One font, as `fonts.xml` names it.
///
/// The pair is the identity: `zh-Hans`, `ja` and `ko` are all
/// `NotoSansCJK-Regular.ttc` and differ only by the index.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct FontRef {
    pub file: Box<str>,
    pub index: u32,
}

impl FontRef {
    fn new(font: &Font) -> Self {
        Self {
            file: font.file.as_str().into(),
            index: font.index,
        }
    }
}

/// What answers for one script.
#[derive(Debug)]
struct ScriptEntry {
    script: Script,
    /// The default fonts, in file order.
    fonts: Vec<FontRef>,
    /// Fonts marked `fallbackFor="serif"`. Usually empty; where it is not,
    /// this is the only per-script serif any backend here can offer besides
    /// Noto's.
    serif: Vec<FontRef>,
}

/// `fonts.xml`, arranged so a query is a lookup.
#[derive(Debug)]
pub(super) struct Index {
    scripts: Vec<ScriptEntry>,
    /// The named families — `sans-serif` and its nine siblings — in file
    /// order, with whether the file pins their fonts somewhere in a variable
    /// font, which makes the name a point rather than a family.
    named: Vec<(Box<str>, Vec<FontRef>, bool)>,
    /// The one anonymous family with no `lang`, which carries the subsetted
    /// symbols font.
    symbols: Vec<FontRef>,
    /// `<alias name="arial" to="sans-serif">`, without the ones that name a
    /// weight: a name can say which family, not which weight of it.
    aliases: Vec<(Box<str>, Box<str>)>,
}

impl Index {
    pub(super) fn build(config: &Config) -> Self {
        let mut index = Self {
            scripts: Vec::new(),
            named: Vec::new(),
            symbols: Vec::new(),
            aliases: config
                .aliases
                .iter()
                .filter(|alias| alias.weight.is_none())
                .map(|alias| (Box::from(alias.name.as_str()), Box::from(alias.to.as_str())))
                .collect(),
        };

        for family in &config.families {
            // `fallbackFor` is an attribute of `<font>`, not of `<family>`,
            // so one element holds both the sans and serif fonts for a
            // script: `und-Ethi` carries Noto Sans Ethiopic and Noto Serif
            // Ethiopic together.
            let (serif, fonts): (Vec<&Font>, Vec<&Font>) = family
                .fonts
                .iter()
                .partition(|font| font.fallback_for.as_deref() == Some("serif"));

            if let Some(name) = &family.name {
                index.named.push((
                    name.as_str().into(),
                    fonts.iter().map(|f| FontRef::new(f)).collect(),
                    fonts.iter().any(|font| font.varied),
                ));
                continue;
            }
            if family.langs.is_empty() {
                index.symbols.extend(fonts.iter().map(|f| FontRef::new(f)));
                continue;
            }
            for tag in &family.langs {
                let Some(script) = lang_script(tag) else {
                    continue;
                };
                let entry = match index.scripts.iter().position(|e| e.script == script) {
                    Some(at) => &mut index.scripts[at],
                    None => {
                        index.scripts.push(ScriptEntry {
                            script,
                            fonts: Vec::new(),
                            serif: Vec::new(),
                        });
                        index.scripts.last_mut().expect("just pushed")
                    }
                };
                entry.fonts.extend(fonts.iter().map(|f| FontRef::new(f)));
                entry.serif.extend(serif.iter().map(|f| FontRef::new(f)));
            }
        }
        index
    }

    /// Fonts for `script`, preferring the serif ones when asked and when the
    /// file marks any.
    ///
    /// Falling back to the sans fonts where no serif is marked is the same
    /// call `source::noto` makes, and for the same reason: a wrong style
    /// beats a box.
    pub(super) fn script_fonts(&self, script: Script, serif: bool) -> &[FontRef] {
        let Some(entry) = self.scripts.iter().find(|e| e.script == script) else {
            return &[];
        };
        if serif && !entry.serif.is_empty() {
            return &entry.serif;
        }
        &entry.fonts
    }

    /// Every script's fonts, in the order the file first gives each script:
    /// for `serif`, every script's serif fonts first, as Skia searches the
    /// fonts marked `fallbackFor="serif"` before the rest.
    pub(super) fn ordered_fonts(&self, serif: bool) -> impl Iterator<Item = &FontRef> {
        let serif_fonts = self
            .scripts
            .iter()
            .filter(move |_| serif)
            .flat_map(|entry| &entry.serif);
        serif_fonts.chain(self.scripts.iter().flat_map(|entry| &entry.fonts))
    }

    /// Every font the file names, once each way it is named.
    pub(super) fn every_font(&self) -> impl Iterator<Item = &FontRef> {
        self.scripts
            .iter()
            .flat_map(|entry| entry.fonts.iter().chain(&entry.serif))
            .chain(self.named.iter().flat_map(|(_, fonts, _)| fonts))
            .chain(&self.symbols)
    }

    /// Fonts for a named family, which is how `fonts.xml` states a generic.
    pub(super) fn named_fonts(&self, name: &str) -> &[FontRef] {
        self.named
            .iter()
            .find(|(known, ..)| known.as_ref() == name)
            .map(|(_, fonts, _)| fonts.as_slice())
            .unwrap_or_default()
    }

    /// Every named family the file states as a family rather than as a point
    /// of one, and its fonts, in file order.
    pub(super) fn named_families(&self) -> impl Iterator<Item = (&str, &[FontRef])> {
        self.named
            .iter()
            .filter(|(_, _, varied)| !varied)
            .map(|(name, fonts, _)| (name.as_ref(), fonts.as_slice()))
    }

    /// Every alias and the family name it points at.
    pub(super) fn aliases(&self) -> impl Iterator<Item = (&str, &str)> {
        self.aliases
            .iter()
            .map(|(name, to)| (name.as_ref(), to.as_ref()))
    }

    /// The symbols family.
    pub(super) fn symbols(&self) -> &[FontRef] {
        &self.symbols
    }

    /// The default family, which AOSP's own comment says is the first one in
    /// the file. Used as the last resort, since `fonts.xml` names none.
    pub(super) fn default_family(&self) -> &[FontRef] {
        self.named
            .first()
            .map(|(_, fonts, _)| fonts.as_slice())
            .unwrap_or_default()
    }
}

/// The script a `lang` attribute stands for.
///
/// Almost every value is already a script in `und-Arab` form. The five that
/// are not — `ja`, `ko`, `zh-Bopo`, `zh-Hans`, `zh-Hant`, unchanged across
/// every release surveyed — are the CJK traditions, and two of those carry no
/// script subtag at all, so they go through the same Han resolution the rest
/// of the crate uses rather than a table of their own.
fn lang_script(tag: &str) -> Option<Script> {
    let language = parse_language(tag)?;
    if let Some(script) = language.script().and_then(|s| Script::parse(s).ok()) {
        return Some(script);
    }
    resolve_han(Script::COMMON, Some(language)).map(Han::script)
}

/// Whether `name` is the family name a CSS generic resolves to, and so a
/// name that must stay a generic rather than becoming a family.
pub(super) fn is_generic(name: &str) -> bool {
    matches!(name, "sans-serif" | "serif" | "monospace" | "cursive")
}

/// The script whose family carries the emoji font.
pub(super) const EMOJI: Script = sc::ZSYE;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::android::config::parse;

    const AOSP_MAIN: &str = include_str!("../../../tests/fixtures/android/fonts-aosp-main.xml");
    const ANDROID_10: &str = include_str!("../../../tests/fixtures/android/fonts-android-10.xml");

    fn index(source: &str) -> Index {
        Index::build(&parse(source).expect("parses"))
    }

    fn files(fonts: &[FontRef]) -> Vec<&str> {
        fonts.iter().map(|font| font.file.as_ref()).collect()
    }

    #[test]
    fn a_script_tag_resolves_to_its_script() {
        assert_eq!(lang_script("und-Arab"), Some(Script::from_bytes(*b"Arab")));
        assert_eq!(lang_script("und-Ethi"), Some(Script::from_bytes(*b"Ethi")));
        assert_eq!(lang_script("zh-Hans"), Some(Script::from_bytes(*b"Hans")));
        assert_eq!(lang_script("zh-Bopo"), Some(Script::from_bytes(*b"Bopo")));
    }

    #[test]
    fn a_han_language_tag_with_no_script_subtag_still_resolves() {
        // `ja` and `ko` are the only tags in the file that name no script.
        // They go through Han resolution rather than a table here.
        assert_eq!(lang_script("ja"), Some(Han::Jpan.script()));
        assert_eq!(lang_script("ko"), Some(Han::Kore.script()));
    }

    #[test]
    fn every_lang_in_every_fixture_resolves() {
        // A tag we cannot map is a script whose fonts we silently drop, so
        // this is the check that matters most in this file.
        for (release, source) in [("main", AOSP_MAIN), ("10", ANDROID_10)] {
            let config = parse(source).expect("parses");
            let unmapped: Vec<&str> = config
                .families
                .iter()
                .flat_map(|family| family.langs.iter())
                .filter(|tag| lang_script(tag).is_none())
                .map(alloc::string::String::as_str)
                .collect();
            assert!(unmapped.is_empty(), "{release} could not map {unmapped:?}");
        }
    }

    #[test]
    fn the_arabic_family_is_found_by_script() {
        let index = index(AOSP_MAIN);
        let arabic = index.script_fonts(Script::from_bytes(*b"Arab"), false);
        assert!(
            files(arabic)
                .iter()
                .any(|file| file.starts_with("NotoNaskhArabic")),
            "got {:?}",
            files(arabic)
        );
    }

    #[test]
    fn a_serif_request_finds_the_serif_font_where_one_is_marked() {
        let index = index(AOSP_MAIN);
        let ethiopic = Script::from_bytes(*b"Ethi");
        let serif = files(index.script_fonts(ethiopic, true));
        let sans = files(index.script_fonts(ethiopic, false));
        assert!(
            serif.iter().any(|f| f.starts_with("NotoSerifEthiopic")),
            "serif request got {serif:?}"
        );
        assert!(
            sans.iter().any(|f| f.starts_with("NotoSansEthiopic")),
            "default request got {sans:?}"
        );
        assert_ne!(serif, sans, "serif and default resolved alike");
    }

    #[test]
    fn a_serif_request_falls_back_to_the_default_font_where_none_is_marked() {
        // 110 of the 133 scripts in the current file mark no serif at all.
        // Answering nothing would be worse than answering the sans font,
        // which is the call `noto` also makes. Arabic is the example because
        // it marks none in any release surveyed — Thai looks like it does
        // per family but has a serif in a second `und-Thai` element, which
        // is exactly why these are merged by script rather than by element.
        let index = index(AOSP_MAIN);
        let arabic = Script::from_bytes(*b"Arab");
        assert_eq!(
            files(index.script_fonts(arabic, true)),
            files(index.script_fonts(arabic, false)),
            "Arabic marks no serif, so both should answer alike"
        );
        assert!(!files(index.script_fonts(arabic, false)).is_empty());
    }

    #[test]
    fn the_generics_resolve_to_named_families() {
        let index = index(AOSP_MAIN);
        for (name, expected) in [
            ("sans-serif", "Roboto"),
            ("serif", "NotoSerif"),
            ("monospace", "DroidSansMono"),
        ] {
            let fonts = files(index.named_fonts(name));
            assert!(
                fonts.iter().any(|file| file.starts_with(expected)),
                "{name} gave {fonts:?}"
            );
        }
        assert!(index.named_fonts("fantasy").is_empty());
    }

    #[test]
    fn the_emoji_font_is_reachable_by_script() {
        // No named family is emoji's: its anonymous family is what the
        // emoji key answers with.
        let index = index(AOSP_MAIN);
        let emoji = files(index.script_fonts(EMOJI, false));
        assert!(
            emoji.iter().any(|file| file.contains("Emoji")),
            "got {emoji:?}"
        );
    }

    #[test]
    fn aosp_ships_no_math_font() {
        // Checked, not assumed: `und-Zmth` appears in none of the three
        // fixtures, so `GenericFamily::Math` has nothing to resolve to on
        // Android by either route. If a future release adds one, this fails
        // and the generic mapping should gain it.
        for (release, source) in [("main", AOSP_MAIN), ("10", ANDROID_10)] {
            let index = index(source);
            assert!(
                index
                    .script_fonts(Script::from_bytes(*b"Zmth"), false)
                    .is_empty(),
                "{release} gained a math font"
            );
        }
    }

    #[test]
    fn the_symbols_family_and_the_default_family_are_both_found() {
        let index = index(AOSP_MAIN);
        assert!(
            files(index.symbols())
                .iter()
                .any(|file| file.starts_with("NotoSansSymbols")),
            "got {:?}",
            files(index.symbols())
        );
        // AOSP's own comment says the first family in the file is the
        // default, and it is sans-serif.
        assert!(
            files(index.default_family())
                .iter()
                .any(|file| file.starts_with("Roboto")),
            "got {:?}",
            files(index.default_family())
        );
    }

    #[test]
    fn the_han_traditions_are_told_apart_by_collection_index() {
        // The whole point of keeping `index`: these share one .ttc.
        let index = index(AOSP_MAIN);
        let hans = index.script_fonts(Han::Hans.script(), false);
        let jpan = index.script_fonts(Han::Jpan.script(), false);
        // Compare the shared collection only: `ja` also carries
        // NotoSerifHentaigana as a separate family, which Simplified has no
        // equivalent of.
        let index_of = |fonts: &[FontRef], file: &str| {
            fonts
                .iter()
                .find(|font| font.file.as_ref() == file)
                .map(|font| font.index)
        };
        const CJK: &str = "NotoSansCJK-Regular.ttc";
        assert!(!hans.is_empty() && !jpan.is_empty());
        assert_ne!(
            index_of(hans, CJK),
            index_of(jpan, CJK),
            "Simplified and Japanese resolved to the same font in {CJK}"
        );
        assert!(index_of(hans, CJK).is_some(), "no {CJK} for Simplified");
    }
}
