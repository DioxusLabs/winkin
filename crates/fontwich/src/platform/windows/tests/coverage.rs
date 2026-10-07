//! The sample letters against a panel of DirectWrite's fonts.

use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{BOOL, HSTRING};

use crate::fallback::report_outliers;

/// Fonts broad enough between them to tell a well-established letter from
/// a rarely-implemented one.
///
/// A panel rather than every installed family: enumerating the whole
/// collection per codepoint is far slower, and it is the *spread* across
/// fonts of differing completeness that reveals an outlier. Checking only
/// our own top pick for each script would not — that pick is always the
/// most complete font available, which is why an earlier version of this
/// check found one bad sample where there were four.
const PANEL: &[&str] = &[
    "Segoe UI",
    "Arial",
    "Times New Roman",
    "Nirmala UI",
    "Microsoft YaHei",
    "SimSun",
    "MS Gothic",
    "Yu Gothic",
    "Malgun Gothic",
    "Segoe UI Historic",
    "Segoe UI Symbol",
    "Ebrima",
    "Leelawadee UI",
    "Gadugi",
    "Sylfaen",
    "Microsoft Himalaya",
    "Mongolian Baiti",
    "Myanmar Text",
    "Nyala",
    "Javanese Text",
];

fn panel_fonts() -> alloc::vec::Vec<IDWriteFont> {
    unsafe {
        let factory: IDWriteFactory =
            DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("a DirectWrite factory");
        let mut collection = None;
        factory
            .GetSystemFontCollection(&mut collection, false)
            .expect("the collection");
        let collection = collection.expect("a non-null collection");
        PANEL
            .iter()
            .filter_map(|family| {
                let mut index = 0;
                let mut exists = BOOL(0);
                collection
                    .FindFamilyName(&HSTRING::from(*family), &mut index, &mut exists)
                    .ok()?;
                exists.as_bool().then_some(())?;
                collection
                    .GetFontFamily(index)
                    .ok()?
                    .GetFirstMatchingFont(
                        DWRITE_FONT_WEIGHT_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        DWRITE_FONT_STYLE_NORMAL,
                    )
                    .ok()
            })
            .collect()
    }
}

#[test]
fn no_sample_codepoint_is_an_outlier() {
    let fonts = panel_fonts();
    assert!(
        !fonts.is_empty(),
        "none of the panel is installed; nothing to judge against"
    );
    let suspect = report_outliers(|codepoint| {
        fonts
            .iter()
            .filter(|font| unsafe { font.HasCharacter(codepoint) }.is_ok_and(|b| b.as_bool()))
            .count()
    });
    assert!(
        suspect.is_empty(),
        "{} script(s) have a sample far less supported than its siblings across \
         the panel, which will demote good fonts:\n  {}",
        suspect.len(),
        suspect.join("\n  ")
    );
}
