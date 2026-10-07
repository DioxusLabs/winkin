//! What a font is, for matching: width, style, weight, and its variation axes.
//!
//! The shapes are fontique's — [`Attributes`] of parlance's `FontWidth`,
//! `FontStyle` and `FontWeight`, and [`Axis`] per `fvar` axis — so that
//! CSS matching can be lifted from fontique without translation. What is read
//! and how follows fontique's `read_attributes`, with two differences.
//!
//! **The oblique angle is converted.** `post.italicAngle`, like the `slnt`
//! axis, counts counter-clockwise, so a forward-leaning oblique has a negative
//! angle. CSS counts clockwise — the spec says so outright, and notes the
//! OpenType convention is the opposite — and `FontStyle::Oblique` holds a CSS
//! angle. So the angle is negated on the way in. fontique stores it unchanged,
//! which gives a forward-leaning oblique a backward CSS angle.
//!
//! **A font on disk is read in pieces.** Attributes need four small tables, so
//! [`from_file`] reads the table directory and then just those ranges, rather
//! than the whole file: a few hundred bytes out of a CJK collection that may be
//! twenty megabytes. `post` is read only as far as its header, since a version 2
//! table carries every glyph's name and the angle is in its first eight bytes.

use alloc::vec::Vec;

use parlance::{FontStyle, FontWeight, FontWidth, Tag};
use read_fonts::tables::fvar::Fvar;
use read_fonts::tables::head::{Head, MacStyle};
use read_fonts::tables::os2::{Os2, SelectionFlags};
use read_fonts::{FontRef, TableProvider};

/// Font attributes used for CSS matching.
///
/// For a variable font, these describe its default instance. Variation
/// ranges are available through [`Axis`].
#[derive(Copy, Clone, PartialEq, Default, Debug)]
pub struct Attributes {
    /// The font width.
    pub width: FontWidth,
    /// The font style, with oblique angles in clockwise degrees.
    pub style: FontStyle,
    /// The font weight.
    pub weight: FontWeight,
}

/// A variation axis from the font's `fvar` table.
///
/// Values use the axis's native units. In particular, `slnt` uses
/// counter-clockwise degrees, unlike CSS oblique angles.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Axis {
    /// The axis tag.
    pub tag: Tag,
    /// The minimum value.
    pub min: f32,
    /// The maximum value.
    pub max: f32,
    /// The default value.
    pub default: f32,
}

/// Whether `axis` states its values the way CSS does.
///
/// Apple's pre-OpenType GX fonts state `wght` and `wdth` as multipliers of
/// normal rather than as a weight class and a percentage: Skia's `wght` runs
/// 0.48 to 3.2 about a default of 1, and its `wdth` 0.62 to 1.3. OpenType
/// puts both in the CSS domain — a `wght` default is the font's own weight
/// class, a `wdth` default is 100 — so ten separates them with room either
/// side.
pub(super) fn states_css(axis: &Axis) -> bool {
    axis.default >= 10.0
}

/// What one of `axis`'s own units is worth in CSS's: 400 for a GX `wght`,
/// whose 1 is CSS's 400, and 100 for a GX `wdth`, whose 1 is CSS's 100%.
/// One for everything else, which already agrees.
pub(super) fn css_scale(axis: &Axis) -> f32 {
    if states_css(axis) {
        return 1.0;
    }
    match &axis.tag.to_bytes() {
        b"wght" => 400.0,
        b"wdth" => 100.0,
        _ => 1.0,
    }
}

/// A font's attributes and axes, read from the font.
pub(super) fn from_font(font: &FontRef<'_>) -> (Attributes, Vec<Axis>) {
    let angle = font
        .post()
        .ok()
        .map(|post| post.italic_angle().to_f64() as f32);
    from_tables(font.os2().ok(), font.head().ok(), angle, font.fvar().ok())
}

/// The same, from the tables themselves, wherever they were read from.
fn from_tables(
    os2: Option<Os2<'_>>,
    head: Option<Head<'_>>,
    italic_angle: Option<f32>,
    fvar: Option<Fvar<'_>>,
) -> (Attributes, Vec<Axis>) {
    let attributes = if let Some(os2) = os2 {
        // OS/2 where there is one. Bits 0 and 9 of fsSelection are italic and
        // oblique; the angle of an oblique is post's italicAngle.
        let selection = os2.fs_selection();
        let style = if selection.contains(SelectionFlags::ITALIC) {
            FontStyle::Italic
        } else if selection.contains(SelectionFlags::OBLIQUE) {
            // Counter-clockwise in OpenType, clockwise in CSS.
            FontStyle::Oblique(italic_angle.map(|angle| -angle))
        } else {
            FontStyle::Normal
        };
        Attributes {
            width: width_from_class(os2.us_width_class()),
            style,
            weight: weight_from_class(os2.us_weight_class()),
        }
    } else if let Some(head) = head {
        // Otherwise head's macStyle, which knows only bold and italic.
        let style = head.mac_style();
        Attributes {
            width: FontWidth::default(),
            style: if style.contains(MacStyle::ITALIC) {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            weight: if style.contains(MacStyle::BOLD) {
                FontWeight::BOLD
            } else {
                FontWeight::default()
            },
        }
    } else {
        Attributes::default()
    };

    let axes = fvar
        .and_then(|fvar| fvar.axes().ok())
        .map(|axes| {
            axes.iter()
                .map(|axis| Axis {
                    tag: Tag::from_bytes(axis.axis_tag().to_be_bytes()),
                    min: axis.min_value().to_f32(),
                    max: axis.max_value().to_f32(),
                    default: axis.default_value().to_f32(),
                })
                // OpenType: an axis whose minimum, default and maximum are
                // out of order is to be ignored. Matching clamps to the
                // range, and would panic on it.
                .filter(|axis| axis.min <= axis.default && axis.default <= axis.max)
                .collect()
        })
        .unwrap_or_default();
    (attributes, axes)
}

/// OS/2's usWidthClass, 1 to 9, as a CSS width.
/// The weight `usWeightClass` names.
///
/// Not clamped to 1..=1000 at the top: a variable font's default can sit
/// outside it, and a font saying 1000 means it.
///
/// At the bottom it has to be. OS/2 version 0 used a 1..=9 scale, and fonts
/// that predate the 100..=900 one still say so: Skia says 5 and means 500,
/// where reading it literally makes the lightest font on the system out of
/// a regular one. Below 10 is that scale, and a hundred times the number is
/// what it meant — the same rule FreeType applies. Zero is not on either
/// scale and means nothing was set, which Apple Chancery does; that takes
/// the default rather than becoming the lightest font there could be.
fn weight_from_class(class: u16) -> FontWeight {
    match class {
        0 => FontWeight::default(),
        1..10 => FontWeight::new(f32::from(class) * 100.0),
        _ => FontWeight::new(f32::from(class)),
    }
}

pub(crate) fn width_from_class(class: u16) -> FontWidth {
    FontWidth::from_ratio(match class {
        0..=1 => 0.5,
        2 => 0.625,
        3 => 0.75,
        4 => 0.875,
        5 => 1.0,
        6 => 1.125,
        7 => 1.25,
        8 => 1.5,
        _ => 2.0,
    })
}

/// A font's attributes and axes, reading only the tables they are in.
#[cfg(feature = "std")]
pub(crate) fn from_file(font: &mut super::sfnt::FileFont) -> (Attributes, Vec<Axis>) {
    use super::sfnt::read_u32;
    use read_fonts::{FontData, FontRead};

    // OS/2 and head are tens of bytes and fvar a few kilobytes, so each is
    // read whole; post only as far as the angle.
    let os2 = font.table(b"OS/2", u32::MAX);
    let head = font.table(b"head", u32::MAX);
    let post = font.table(b"post", 8);
    let fvar = font.table(b"fvar", u32::MAX);
    let italic_angle = post
        .as_deref()
        .and_then(|post| read_u32(post, 4))
        .map(|fixed| fixed as i32 as f32 / 65536.0);
    from_tables(
        os2.as_deref()
            .and_then(|bytes| Os2::read(FontData::new(bytes)).ok()),
        head.as_deref()
            .and_then(|bytes| Head::read(FontData::new(bytes)).ok()),
        italic_angle,
        fvar.as_deref()
            .and_then(|bytes| Fvar::read(FontData::new(bytes)).ok()),
    )
}

/// Where the named instance `at` of a font sits: its `fvar` instance record,
/// as axis values, or empty where the font has no such instance.
///
/// Counted as fontconfig counts it, from one: an `FC_INDEX` whose upper bits
/// are zero names the font itself, which has no coordinates of its own.
#[cfg(feature = "std")]
// The fontconfig backend's: elsewhere the platform gives coordinates rather
// than an instance number.
#[cfg_attr(not(fontwich_fontconfig), allow(dead_code))]
pub(crate) fn instance_from_file(font: &mut super::sfnt::FileFont, at: u32) -> Vec<([u8; 4], f32)> {
    use read_fonts::{FontData, FontRead};

    let Some(at) = at.checked_sub(1) else {
        return Vec::new();
    };
    let Some(bytes) = font.table(b"fvar", u32::MAX) else {
        return Vec::new();
    };
    let Ok(fvar) = Fvar::read(FontData::new(&bytes)) else {
        return Vec::new();
    };
    let (Ok(axes), Ok(instances)) = (fvar.axes(), fvar.instances()) else {
        return Vec::new();
    };
    let Some(Ok(instance)) = instances.iter().nth(at as usize) else {
        return Vec::new();
    };
    axes.iter()
        .zip(instance.coordinates.iter())
        .map(|(axis, value)| (axis.axis_tag().to_be_bytes(), value.get().to_f32()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fonts::{font_with_tables, os2, post};

    fn attributes(font: &[u8]) -> Attributes {
        from_font(&FontRef::new(font).expect("a font")).0
    }

    #[test]
    fn an_old_weight_class_is_read_on_the_scale_it_was_written_on() {
        // OS/2 version 0 used 1..=9 and fonts that predate the 100..=900
        // scale still say so. Skia says 5 and means 500; read literally it
        // is the lightest font on a Mac. Zero is on neither scale and means
        // nothing was set, which Apple Chancery does.
        let weight = |class| attributes(&font_with_tables(&[(*b"OS/2", os2(class, 5, 0))])).weight;
        assert_eq!(weight(0), FontWeight::default(), "unset");
        assert_eq!(weight(5).value(), 500.0, "the old scale");
        assert_eq!(weight(9).value(), 900.0, "the top of the old scale");
        // Everything from ten up is the modern scale and is taken as
        // written, including a variable font's default outside 1..=1000.
        assert_eq!(weight(10).value(), 10.0);
        assert_eq!(weight(400).value(), 400.0);
        assert_eq!(weight(1000).value(), 1000.0);
    }

    #[test]
    fn an_axis_out_of_order_is_ignored() {
        use crate::test_fonts::fvar;
        let font = font_with_tables(&[(
            *b"fvar",
            fvar(&[
                (*b"wght", 700.0, 400.0, 100.0),
                (*b"wdth", 75.0, 100.0, 100.0),
            ]),
        )]);
        let (_, axes) = from_font(&FontRef::new(&font).expect("a font"));
        assert_eq!(axes.len(), 1);
        assert_eq!(axes[0].tag.to_bytes(), *b"wdth");
    }

    #[test]
    fn weight_width_and_italic_come_from_os2() {
        let font = font_with_tables(&[(*b"OS/2", os2(700, 3, 0x0001))]);
        assert_eq!(
            attributes(&font),
            Attributes {
                width: FontWidth::CONDENSED,
                style: FontStyle::Italic,
                weight: FontWeight::BOLD,
            }
        );
    }

    #[test]
    fn an_oblique_angle_is_turned_into_css_s_direction() {
        // A forward-leaning oblique: italicAngle -12, counter-clockwise. CSS
        // calls that 12 degrees, clockwise.
        let font = font_with_tables(&[(*b"OS/2", os2(400, 5, 0x0200)), (*b"post", post(-12.0))]);
        assert_eq!(attributes(&font).style, FontStyle::Oblique(Some(12.0)));
    }

    #[test]
    fn an_oblique_with_no_angle_leaves_it_to_the_engine() {
        let font = font_with_tables(&[(*b"OS/2", os2(400, 5, 0x0200))]);
        assert_eq!(attributes(&font).style, FontStyle::Oblique(None));
    }

    #[test]
    fn italic_wins_over_oblique() {
        let font = font_with_tables(&[(*b"OS/2", os2(400, 5, 0x0201)), (*b"post", post(-12.0))]);
        assert_eq!(attributes(&font).style, FontStyle::Italic);
    }

    #[test]
    fn without_os2_head_s_mac_style_is_used() {
        let mut head = alloc::vec![0u8; 54];
        head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes()); // magic
        head[44..46].copy_from_slice(&0b11u16.to_be_bytes()); // bold, italic
        let font = font_with_tables(&[(*b"head", head)]);
        assert_eq!(
            attributes(&font),
            Attributes {
                width: FontWidth::NORMAL,
                style: FontStyle::Italic,
                weight: FontWeight::BOLD,
            }
        );
    }

    #[test]
    fn a_font_with_neither_is_a_plain_regular() {
        let font = font_with_tables(&[]);
        assert_eq!(attributes(&font), Attributes::default());
    }

    #[cfg(feature = "std")]
    mod on_disk {
        use super::*;
        use crate::font::sfnt::FileFont;
        use crate::test_fonts::{Temporary, font_collection};

        fn from_file(path: &std::path::Path, index: u32) -> Option<(Attributes, Vec<Axis>)> {
            FileFont::open(path, index).map(|mut font| super::super::from_file(&mut font))
        }

        #[test]
        fn reading_the_tables_from_disk_agrees_with_reading_the_whole_font() {
            let font = font_with_tables(&[(*b"OS/2", os2(300, 7, 0x0200)), (*b"post", post(-9.5))]);
            let file = Temporary::new("single.ttf", &font);
            assert_eq!(
                from_file(file.path(), 0),
                Some(from_font(&FontRef::new(&font).expect("a font")))
            );
            assert_eq!(
                from_file(file.path(), 1),
                None,
                "a single font has no index 1"
            );
        }

        #[test]
        fn each_font_of_a_collection_is_read_from_its_own_tables() {
            let light = font_with_tables(&[(*b"OS/2", os2(300, 5, 0))]);
            let bold = font_with_tables(&[(*b"OS/2", os2(700, 5, 0x0001))]);
            let file = Temporary::new("pair.ttc", &font_collection(&[light, bold]));
            let weight = |index| from_file(file.path(), index).map(|(a, _)| (a.weight, a.style));
            assert_eq!(weight(0), Some((FontWeight::LIGHT, FontStyle::Normal)));
            assert_eq!(weight(1), Some((FontWeight::BOLD, FontStyle::Italic)));
            assert_eq!(weight(2), None, "the collection holds two fonts");
        }

        #[test]
        fn something_that_is_not_a_font_is_none() {
            let file = Temporary::new("junk.bin", b"not a font at all, not even close");
            assert_eq!(from_file(file.path(), 0), None);
            let missing = std::env::temp_dir().join("fontwich-no-such-file.ttf");
            assert_eq!(from_file(&missing, 0), None);
        }
    }
}
