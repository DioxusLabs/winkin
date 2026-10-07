//! The sample letters against the fonts fontconfig lists.

use fontconfig_sys::constants::{FC_CHARSET, FC_FAMILY};

use crate::fallback::report_outliers;
use crate::platform::fontconfig::ffi::{fc_call, library};

/// How many installed fonts carry `codepoint`.
fn covering(codepoint: u32) -> usize {
    let Some(fc) = library() else { return 0 };
    unsafe {
        let pattern = fc_call!(fc, FcPatternCreate());
        let objects = fc_call!(fc, FcObjectSetCreate());
        if pattern.is_null() || objects.is_null() {
            return 0;
        }
        let charset = fc_call!(fc, FcCharSetCreate());
        fc_call!(fc, FcCharSetAddChar(charset, codepoint));
        fc_call!(
            fc,
            FcPatternAddCharSet(pattern, FC_CHARSET.as_ptr(), charset)
        );
        fc_call!(fc, FcCharSetDestroy(charset));
        fc_call!(fc, FcObjectSetAdd(objects, FC_FAMILY.as_ptr()));

        let set = fc_call!(fc, FcFontList(core::ptr::null_mut(), pattern, objects));
        let count = if set.is_null() {
            0
        } else {
            (*set).nfont.max(0) as usize
        };
        if !set.is_null() {
            fc_call!(fc, FcFontSetDestroy(set));
        }
        fc_call!(fc, FcObjectSetDestroy(objects));
        fc_call!(fc, FcPatternDestroy(pattern));
        count
    }
}

#[test]
fn no_sample_codepoint_is_an_outlier() {
    let suspect = report_outliers(covering);
    assert!(
        suspect.is_empty(),
        "{} script(s) have a sample far less supported than its siblings, \
         which will demote good fonts:\n  {}\n\nReplace it with a more \
         established letter from the same script.",
        suspect.len(),
        suspect.join("\n  ")
    );
}
