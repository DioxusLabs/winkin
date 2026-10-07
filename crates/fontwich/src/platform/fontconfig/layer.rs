//! The system layer on Linux and the BSDs: fontconfig's fonts.
//!
//! Built the way fontique's fontconfig backend builds its name map, and for
//! the same reason: fontconfig will list every family name at once for the
//! cost of walking its own cache, so the layer's name set is complete from the
//! start and nothing is read from a font file. A family's files are asked for
//! the first time its fonts are wanted.
//!
//! The layer's fallback backend is fontconfig's, so listing the fonts and
//! choosing fallback read one view of the same configuration.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::CStr;
use core::ptr;
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use fontconfig_sys::constants::{
    FC_FAMILY, FC_FAMILYLANG, FC_FILE, FC_FULLNAME, FC_INDEX, FC_POSTSCRIPT_NAME,
};
use fontconfig_sys::{FcFontSet, FcPattern, FcResultMatch};

use crate::platform::fontconfig::ffi::{Library, fc_call, library, serialized};

use crate::font::{FileFont, instance_from_file};
use crate::{Font, Layer, LoadFamily, Role};

/// The system layer; see [`Layer::system`].
pub(crate) fn layer() -> Layer {
    let (families, primaries) = families();
    Layer::with_secondary(Role::System, families, primaries, Arc::new(FontconfigFonts))
        .with_fallback(crate::backend::Backend::platform())
}

/// Every family fontconfig knows, as `(name, aliases)`.
///
/// A font carries its family's name in several languages, each with its own
/// `familylang`. The English one is the family's name where there is one, as
/// `family_names` prefers English when reading a font directly; the rest
/// are aliases. Fonts of one family can list different sets, so the aliases of
/// every font filed under one name are gathered together.
///
/// Not every alias is another language. fontconfig lists a font's legacy
/// per-style family name beside its typographic one, so on a stock Fedora 44
/// Noto Sans CJK JP has aliases "Noto Sans CJK JP Light", "… Thin" and the
/// rest of its weights. That is what fontique does too, and it means a
/// stylesheet naming "Noto Sans CJK JP Light" finds the family — the whole
/// family, not only its Light weight, which is matching's job to pick.
///
/// Returns them with how many come first as some font's own family; the
/// rest are only ever a font's second name in the same language, and are
/// secondary.
fn families() -> (Vec<(String, Vec<String>)>, usize) {
    let Some(fc) = library() else {
        return (Vec::new(), 0);
    };
    let mut gathered: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Sorted, both: the order they are listed in is the layer's.
    let mut seconds: BTreeSet<String> = BTreeSet::new();
    serialized(|| unsafe {
        let set = list(fc, None, &[FC_FAMILY, FC_FAMILYLANG]);
        if set.is_null() {
            return;
        }
        for font in fonts(set) {
            let names = strings(fc, font, FC_FAMILY);
            let langs = strings(fc, font, FC_FAMILYLANG);
            let english = langs
                .iter()
                .position(|lang| lang.eq_ignore_ascii_case("en"))
                .filter(|&at| at < names.len())
                .unwrap_or(0);
            let Some(name) = names.get(english) else {
                continue;
            };
            // A font's other names in its family name's own language are
            // families of their own: "DejaVu Sans Condensed" on DejaVu's
            // condensed fonts, which a page asks for by that name and
            // fontconfig answers with those fonts alone, as a load by name
            // here does too. In another language, the same family's name
            // there: an alias.
            let language = langs.get(english);
            for (at, other) in names.iter().enumerate() {
                if at != english && language.is_some() && langs.get(at) == language {
                    seconds.insert(other.clone());
                }
            }
            let aliases = gathered.entry(name.clone()).or_default();
            for (at, other) in names.iter().enumerate() {
                let own = language.is_some() && langs.get(at) == language;
                if at != english && !own && !aliases.contains(other) {
                    aliases.push(other.clone());
                }
            }
        }
        fc_call!(fc, FcFontSetDestroy(set));
    });
    let primaries = gathered.len();
    // A name some font has as its own family is that family, not secondary.
    let seconds: Vec<String> = seconds
        .into_iter()
        .filter(|name| !gathered.contains_key(name))
        .collect();
    let mut families: Vec<(String, Vec<String>)> = gathered.into_iter().collect();
    families.extend(seconds.into_iter().map(|name| (name, Vec::new())));
    (families, primaries)
}

/// Asks fontconfig for a family's files when its fonts are first wanted.
#[derive(Debug)]
struct FontconfigFonts;

impl LoadFamily for FontconfigFonts {
    fn load(&self, name: &str) -> Vec<Font> {
        let (Some(fc), Ok(name)) = (library(), CString::new(name)) else {
            return Vec::new();
        };
        let mut located: Vec<(Arc<Path>, u32)> = Vec::new();
        serialized(|| unsafe {
            let set = list(fc, Some((FC_FAMILY, &name)), &[FC_FILE, FC_INDEX]);
            if set.is_null() {
                return;
            }
            for font in fonts(set) {
                let Some(file) = bytes(fc, font, FC_FILE) else {
                    continue;
                };
                let mut index = 0;
                let _ = fc_call!(
                    fc,
                    FcPatternGetInteger(font, FC_INDEX.as_ptr(), 0, &mut index)
                );
                // fontconfig lists each named instance of a variable font as a
                // font of its own, and says which in FC_INDEX's upper sixteen
                // bits. Its place within a collection is the lower sixteen. A
                // variable font is one font, whose axes give its range, so the
                // instances fold into it.
                let entry = (
                    Arc::<Path>::from(Path::new(OsStr::from_bytes(file))),
                    (index.max(0) as u32) & 0xFFFF,
                );
                if !located.contains(&entry) {
                    located.push(entry);
                }
            }
            fc_call!(fc, FcFontSetDestroy(set));
        });
        // Read outside fontconfig's lock: this is our own file access, not
        // fontconfig's.
        located
            .into_iter()
            .filter_map(|(path, index)| Font::from_path(path, index))
            .collect()
    }

    fn local(&self, name: &str) -> Option<Font> {
        let (Some(fc), Ok(name)) = (library(), CString::new(name)) else {
            return None;
        };
        // The full name, then the PostScript name. Either may be a named
        // instance's, and fontconfig says which instance in the upper half of
        // `FC_INDEX`: the font is held there, since that is what the name
        // means. Asked of the file as it is opened, and kept nowhere.
        let located = serialized(|| unsafe {
            [FC_FULLNAME, FC_POSTSCRIPT_NAME]
                .into_iter()
                .find_map(|object| {
                    let set = list(fc, Some((object, &name)), &[FC_FILE, FC_INDEX]);
                    if set.is_null() {
                        return None;
                    }
                    let found = fonts(set).find_map(|font| {
                        let file = bytes(fc, font, FC_FILE)?;
                        let mut index = 0;
                        let _ = fc_call!(
                            fc,
                            FcPatternGetInteger(font, FC_INDEX.as_ptr(), 0, &mut index)
                        );
                        let index = index.max(0) as u32;
                        Some((
                            Arc::<Path>::from(Path::new(OsStr::from_bytes(file))),
                            index & 0xFFFF,
                            index >> 16,
                        ))
                    });
                    fc_call!(fc, FcFontSetDestroy(set));
                    found
                })
        });
        let (path, index, instance) = located?;
        if instance == 0 {
            return Font::from_path(path, index);
        }
        let mut file = FileFont::open(&path, index)?;
        let coordinates = instance_from_file(&mut file, instance);
        let mut font = Font::from_file(path, index, &mut file);
        font.pin(&coordinates);
        Some(font)
    }
}

/// `FcFontList` over every font, or those whose `object` is `value`,
/// fetching `objects`. Null on failure; the caller destroys what it gets.
unsafe fn list(fc: &Library, only: Option<(&CStr, &CStr)>, objects: &[&CStr]) -> *mut FcFontSet {
    unsafe {
        let pattern = fc_call!(fc, FcPatternCreate());
        let set = fc_call!(fc, FcObjectSetCreate());
        if pattern.is_null() || set.is_null() {
            if !pattern.is_null() {
                fc_call!(fc, FcPatternDestroy(pattern));
            }
            if !set.is_null() {
                fc_call!(fc, FcObjectSetDestroy(set));
            }
            return ptr::null_mut();
        }
        if let Some((object, value)) = only {
            fc_call!(
                fc,
                FcPatternAddString(pattern, object.as_ptr(), value.as_ptr().cast())
            );
        }
        for object in objects {
            fc_call!(fc, FcObjectSetAdd(set, object.as_ptr()));
        }
        let fonts = fc_call!(fc, FcFontList(ptr::null_mut(), pattern, set));
        fc_call!(fc, FcObjectSetDestroy(set));
        fc_call!(fc, FcPatternDestroy(pattern));
        fonts
    }
}

/// The patterns in `set`.
unsafe fn fonts(set: *mut FcFontSet) -> impl Iterator<Item = *mut FcPattern> {
    let count = unsafe { (*set).nfont.max(0) as usize };
    (0..count).map(move |at| unsafe { *(*set).fonts.add(at) })
}

/// The first value of `object` in `font`, as the bytes fontconfig holds.
///
/// For file paths, which on Unix need not be UTF-8: going through a `String`
/// would replace what it could not decode and name a file that is not there.
/// Borrowed from the pattern, so it must be used before the set is destroyed.
unsafe fn bytes<'a>(fc: &Library, font: *mut FcPattern, object: &CStr) -> Option<&'a [u8]> {
    let mut value = ptr::null_mut();
    let found = unsafe { fc_call!(fc, FcPatternGetString(font, object.as_ptr(), 0, &mut value)) };
    if found != FcResultMatch || value.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(value.cast()) }.to_bytes())
}

/// Every string value of `object` in `font`, in order. Lossy where a value is
/// not UTF-8, which family names and languages are in practice; file paths go
/// through [`bytes`] instead.
unsafe fn strings(fc: &Library, font: *mut FcPattern, object: &CStr) -> Vec<String> {
    let mut values = Vec::new();
    for at in 0.. {
        let mut value = ptr::null_mut();
        let found = unsafe {
            fc_call!(
                fc,
                FcPatternGetString(font, object.as_ptr(), at, &mut value)
            )
        };
        if found != FcResultMatch || value.is_null() {
            break;
        }
        let text = unsafe { CStr::from_ptr(value.cast()) };
        values.push(String::from_utf8_lossy(text.to_bytes()).into_owned());
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use crate::fallback::FallbackKey;
    use crate::{Collection, FallbackRequest, GenericClass, Source};
    use parlance::Script;

    /// The system layer, or `None` on a machine with no fonts — where every
    /// assertion here would be about an empty list, and a bare container
    /// should skip rather than fail.
    fn layer() -> Option<Arc<Layer>> {
        let layer = super::layer();
        (!layer.is_empty()).then(|| Arc::new(layer))
    }

    fn query(tag: &[u8; 4], lang: Option<&'static str>) -> FallbackRequest {
        FallbackRequest::Text {
            script: Script::from_bytes(*tag),
            language: lang.and_then(crate::parse_language),
            generic: GenericClass::Plain,
        }
    }

    #[test]
    fn listing_the_system_reads_no_font() {
        let Some(layer) = layer() else { return };
        assert!(
            layer.len() > 1,
            "fontconfig listed {} families",
            layer.len()
        );
        assert_eq!(layer.loaded(), 0, "building the layer loaded a family");
    }

    #[test]
    fn a_family_loads_the_files_fontconfig_has_for_it() {
        let Some(layer) = layer() else { return };
        let collection = Collection::new().with_layer(layer.clone());
        let name = String::from(layer.names().next().expect("a family"));
        let family = collection.family(&name).expect("its own name finds it");
        assert!(!family.is_loaded());
        let fonts = family.fonts();
        assert!(!fonts.is_empty(), "{name} loaded no fonts");
        for font in fonts {
            let Source::Path(path) = font.source() else {
                panic!("a system font should be named by path");
            };
            assert!(path.exists(), "{name}: {} does not exist", path.display());
        }
        assert_eq!(layer.loaded(), 1, "loading one family loaded others");
    }

    #[test]
    fn every_font_is_a_font_once_and_reads_the_same_either_way() {
        // Every family on the system, loaded. fontconfig names each named
        // instance of a variable font by an FC_INDEX past 0xFFFF; those must
        // have folded into the font, leaving each (file, index) once. And the
        // tables read from disk must say what the whole font says.
        let Some(layer) = layer() else { return };
        let collection = Collection::new().with_layer(layer.clone());
        let (mut checked, mut variable) = (0, 0);
        for name in layer.names() {
            let family = collection.family(name).expect("listed");
            let fonts = family.fonts();
            for (at, font) in fonts.iter().enumerate() {
                let Source::Path(path) = font.source() else {
                    unreachable!()
                };
                assert!(font.index() < 0x10000, "{name}: index {:#x}", font.index());
                assert!(
                    !fonts[..at].iter().any(|other| {
                        matches!(other.source(), Source::Path(other) if other == path)
                            && other.index() == font.index()
                    }),
                    "{name}: {} #{} twice",
                    path.display(),
                    font.index()
                );
                let data: Arc<[u8]> = std::fs::read(path).expect("readable").into();
                let whole = Font::from_data(data, font.index());
                assert_eq!(
                    (whole.attributes(), whole.axes(), whole.charset()),
                    (font.attributes(), font.axes(), font.charset()),
                    "{} #{}",
                    path.display(),
                    font.index()
                );
                checked += 1;
                variable += usize::from(!font.axes().is_empty());
            }
        }
        std::eprintln!("{checked} fonts checked, {variable} variable");
        assert!(checked >= layer.len());
    }

    #[test]
    fn every_family_fontconfig_falls_back_to_is_listed() {
        // The point of the layer owning the backend: one view of the system.
        // Every family fontconfig's own sort names must be one the listing
        // found. The static tail -- symbols, last resort, emoji -- is left out,
        // since it names families that may not be installed at all.
        let Some(layer) = layer() else { return };
        let collection = Collection::new().with_layer(layer);
        let source = Backend::platform();
        for (tag, lang) in [
            (b"Latn", Some("en-US")),
            (b"Arab", Some("ar")),
            (b"Deva", Some("hi-IN")),
            (b"Hani", Some("ja-JP")),
            (b"Thai", None),
            (b"Hebr", None),
        ] {
            let key = FallbackKey::new(&query(tag, lang), source.facts());
            source.families(&key, |name| {
                assert!(
                    collection.family(name).is_some(),
                    "fontconfig falls back to {name:?} for {} but did not list it",
                    core::str::from_utf8(tag).unwrap_or("?")
                );
            });
        }
    }

    #[test]
    fn a_family_chain_on_the_system_names_only_what_loads() {
        let Some(layer) = layer() else { return };
        let collection = Collection::new().with_layer(layer);
        let families = collection.fallback(&collection.key(&query(b"Latn", Some("en-US"))));
        assert!(!families.is_empty(), "no fallback for Latin at all");
        for family in families.iter() {
            assert_eq!(family.role(), Role::System);
            assert!(!family.fonts().is_empty(), "{} has no fonts", family.name());
        }
    }
}
