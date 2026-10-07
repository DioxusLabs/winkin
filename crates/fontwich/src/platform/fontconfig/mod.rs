//! The Linux and BSD backend, driven by fontconfig at query time.
//!
//! There is no useful static table for these platforms: the installed set is
//! entirely a matter of what the distribution and the user chose, and any
//! family names hardcoded here would be guesses. fontconfig already knows the
//! answer, and every other text stack on the system is asking it the same
//! question, so agreeing with it is also the point.
//!
//! Chrome's Linux fallback is one `FcFontSort` per content language, with
//! `FC_LANG` and nothing else, searched for the first font that maps the
//! character. This backend does the same. A key's language is the sort's.
//! A script's answer keeps only the families of the sort that map the
//! script's sample letters, which fontconfig's charsets tell without reading
//! a font. The Common key keeps the whole sort.
//!
//! libfontconfig is linked by default. With `RUST_FONTCONFIG_DLOPEN` set at
//! build time it loads at run time instead, and the build needs no
//! pkg-config and no fontconfig headers. When the library cannot be loaded,
//! [`Fontconfig`] answers only its own tail rather than failing.

mod ffi;
mod generics;
mod lang;
mod layer;
mod tail;
#[cfg(test)]
mod tests;

pub(super) use layer::layer;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::{CStr, c_char};
use core::ptr;
use std::ffi::CString;

use fontconfig_sys::constants::{FC_CHARSET, FC_FAMILY, FC_LANG};
use fontconfig_sys::{FcChar8, FcFontSet, FcMatchPattern, FcPattern, FcResult, FcResultMatch};

use crate::fallback::Presentation;
use crate::fallback::{FallbackKey, GenericBucket, sample_codepoints};
use crate::script as sc;

use ffi::{Library, fc_call, library, serialized};

/// A fontconfig fallback backend.
///
/// Queries fontconfig on each call. Collections cache the resolved answers.
#[derive(Debug, Default)]
pub struct Fontconfig {}

impl Fontconfig {
    /// Creates a fontconfig fallback backend.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if libfontconfig is available.
    ///
    /// Always returns `true` when linked at build time. With
    /// `RUST_FONTCONFIG_DLOPEN`, checks whether the library can be loaded.
    /// An unavailable backend still returns its built-in fallback families.
    pub fn is_available() -> bool {
        library().is_some()
    }

    /// The families for `key`, best first:
    ///
    /// - a script: fontconfig's sort for the key's language, kept to the
    ///   families mapping the script's sample letters;
    /// - a Han tradition: the sort for its language, kept to the families
    ///   mapping Han;
    /// - Common: the whole sort for the key's language, then the symbol
    ///   fonts and the last resort;
    /// - a generic, or the Standard font: Chrome's settings for its bucket,
    ///   each followed by the families fontconfig binds to it;
    /// - emoji: the emoji fonts.
    pub(crate) fn families(&self, key: &FallbackKey, name: &mut dyn FnMut(&str)) {
        if let Some(presentation) = key.presentation() {
            if presentation == Presentation::Emoji {
                tail::EMOJI.iter().for_each(|family| name(family));
            }
            return;
        }
        let chrome = match (key.generic(), key.standard()) {
            (Some((generic, bucket)), _) => {
                Some((generics::TABLE.families(generic, bucket), bucket))
            }
            (None, Some(bucket)) => Some((generics::TABLE.standard(bucket), bucket)),
            (None, None) => None,
        };
        if let Some((chrome, bucket)) = chrome {
            // Each of Chrome's names, then the families fontconfig's
            // configuration binds to it: `Times New Roman` to Liberation
            // Serif, `monospace` to DejaVu Sans Mono. Only what it binds,
            // never its best-effort match for a name nothing binds.
            let language = bucket_language(bucket);
            for family in chrome {
                name(family);
                for bound in serialized(|| bindings(family, language)) {
                    name(&bound);
                }
            }
            return;
        }
        let Some(script) = key.script() else {
            return;
        };
        let language = match key.han() {
            Some(han) if script == sc::HANI || key.language().is_none() => {
                Some(lang::han_lang(han))
            }
            _ => key.language(),
        };
        let samples = if key.is_common() {
            &[][..]
        } else {
            sample_codepoints(script)
        };
        let sorted = serialized(|| sorted(None, language, samples));
        if key.is_common() {
            sorted.iter().for_each(|(family, _)| name(family));
            tail::SYMBOLS.iter().for_each(|family| name(family));
            tail::LAST_RESORT.iter().for_each(|family| name(family));
            return;
        }
        for (family, covers) in &sorted {
            if *covers {
                name(family);
            }
        }
    }
}

/// The language fontconfig sorts a generic's bucket by.
fn bucket_language(bucket: GenericBucket) -> Option<&'static str> {
    Some(match bucket {
        GenericBucket::Common => return None,
        GenericBucket::Hans => "zh-cn",
        GenericBucket::Hant => "zh-tw",
        GenericBucket::Jpan => "ja",
        GenericBucket::Kore => "ko",
        GenericBucket::Deva => "hi",
        GenericBucket::Arab => "ar",
        GenericBucket::Cyrl => "ru",
        GenericBucket::Grek => "el",
    })
}

/// The families fontconfig's configuration binds to `family` for
/// `language`, in order: the family list a pattern naming it has after
/// config substitution, up to the first generic keyword the configuration
/// appends to every name, unless `family` is that keyword.
///
/// Empty when libfontconfig is unavailable.
fn bindings(family: &str, language: Option<&str>) -> Vec<String> {
    let mut bound = Vec::new();
    let Some(fc) = library() else {
        return bound;
    };
    let Ok(name) = CString::new(family) else {
        return bound;
    };
    let language = language.and_then(|language| CString::new(language).ok());
    // SAFETY: as in `sorted`.
    unsafe {
        let pattern = fc_call!(fc, FcPatternCreate());
        if pattern.is_null() {
            return bound;
        }
        add_string(fc, pattern, FC_FAMILY, &name);
        if let Some(language) = &language {
            add_string(fc, pattern, FC_LANG, language);
        }
        fc_call!(
            fc,
            FcConfigSubstitute(ptr::null_mut(), pattern, FcMatchPattern)
        );
        let keyword = |name: &str| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                "serif"
                    | "sans-serif"
                    | "sans"
                    | "monospace"
                    | "cursive"
                    | "fantasy"
                    | "math"
                    | "system-ui"
                    | "emoji"
                    | "fangsong"
            )
        };
        let mut at = 1;
        loop {
            let mut value: *mut FcChar8 = ptr::null_mut();
            let found = fc_call!(
                fc,
                FcPatternGetString(pattern, FC_FAMILY.as_ptr(), at, &mut value)
            );
            if found != FcResultMatch || value.is_null() {
                break;
            }
            let value = CStr::from_ptr(value as *const c_char).to_string_lossy();
            if keyword(&value) && !value.eq_ignore_ascii_case(family) {
                break;
            }
            bound.push(value.into_owned());
            at += 1;
        }
        fc_call!(fc, FcPatternDestroy(pattern));
    }
    bound
}

/// fontconfig's trimmed sort for `family` and `language`, each family once
/// as first sorted, with whether one of its fonts maps every one of
/// `samples`, as fontconfig's charsets say: all of them do where there are
/// none.
///
/// Empty when libfontconfig is unavailable or fontconfig has nothing to say.
fn sorted(family: Option<&str>, language: Option<&str>, samples: &[u32]) -> Vec<(String, bool)> {
    let mut families = Vec::new();
    // `None` only when loading at runtime and the library is missing; see
    // `ffi`. Either way an unavailable fontconfig must not take the process
    // down, so the caller gets an empty list and falls back to the tail.
    let Some(fc) = library() else {
        return families;
    };
    let family = family.and_then(|family| CString::new(family).ok());
    let language = language.and_then(|language| CString::new(language).ok());

    // SAFETY: every pointer below is either freshly created by fontconfig or
    // borrowed from a `CString` that outlives the call. A null `FcConfig`
    // means "the current configuration", which fontconfig initializes on
    // first use, so there is no config to own or free here.
    unsafe {
        let pattern = fc_call!(fc, FcPatternCreate());
        if pattern.is_null() {
            return families;
        }
        if let Some(family) = &family {
            add_string(fc, pattern, FC_FAMILY, family);
        }
        if let Some(language) = &language {
            add_string(fc, pattern, FC_LANG, language);
        }
        // Config rules run against the language asked for — the Vazirmatn
        // package tests `lang=fa`, for one — so it has to be in the pattern
        // before this. They also run against the *process* locale, whatever
        // the pattern says.
        fc_call!(
            fc,
            FcConfigSubstitute(ptr::null_mut(), pattern, FcMatchPattern)
        );
        fc_call!(fc, FcDefaultSubstitute(pattern));

        let mut result: FcResult = FcResultMatch;
        // `trim` drops fonts that add no coverage the earlier ones lack:
        // every family in the answer earns its place, and the first that
        // maps a character is the same as untrimmed.
        let set = fc_call!(
            fc,
            FcFontSort(ptr::null_mut(), pattern, 1, ptr::null_mut(), &mut result)
        );
        if !set.is_null() {
            families = family_names(fc, set, samples);
            fc_call!(fc, FcFontSetSortDestroy(set));
        }
        fc_call!(fc, FcPatternDestroy(pattern));
    }
    families
}

/// # Safety
/// `pattern` must be a live fontconfig pattern.
unsafe fn add_string(fc: &Library, pattern: *mut FcPattern, object: &CStr, value: &CStr) {
    unsafe {
        let value = value.as_ptr() as *const FcChar8;
        fc_call!(fc, FcPatternAddString(pattern, object.as_ptr(), value));
    }
}

/// Each family in `set` once, as first sorted, and whether any of its fonts
/// maps every one of `samples`.
///
/// # Safety
/// `set` must be a live font set returned by fontconfig.
unsafe fn family_names(fc: &Library, set: *mut FcFontSet, samples: &[u32]) -> Vec<(String, bool)> {
    let mut names: Vec<(String, bool)> = Vec::new();
    unsafe {
        let count = (*set).nfont.max(0) as usize;
        for index in 0..count {
            let font = *(*set).fonts.add(index);
            if font.is_null() {
                continue;
            }
            let mut value: *mut FcChar8 = ptr::null_mut();
            let found = fc_call!(
                fc,
                FcPatternGetString(font, FC_FAMILY.as_ptr(), 0, &mut value)
            );
            if found != FcResultMatch || value.is_null() {
                continue;
            }
            // Borrowed from the pattern, so it has to be copied before the
            // font set is destroyed.
            let name = CStr::from_ptr(value as *const c_char).to_string_lossy();
            let mut charset = ptr::null_mut();
            let covers = samples.is_empty()
                || (fc_call!(
                    fc,
                    FcPatternGetCharSet(font, FC_CHARSET.as_ptr(), 0, &mut charset)
                ) == FcResultMatch
                    && !charset.is_null()
                    && samples
                        .iter()
                        .all(|&c| fc_call!(fc, FcCharSetHasChar(charset, c)) != 0));
            match names.iter_mut().find(|(seen, _)| *seen == name) {
                Some((_, own)) => *own |= covers,
                None => names.push((name.into_owned(), covers)),
            }
        }
    }
    names
}
