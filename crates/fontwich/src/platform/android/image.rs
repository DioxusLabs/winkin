//! The tables against the fonts an Android system image ships.
//!
//! A device is not needed and was not used. A system image is a GPT disk
//! whose `super` partition holds the dynamic partitions; `lpunpack` gives
//! `system.img`, which mounts read-only, and `/system/etc/fonts.xml` and the
//! 208 files of `/system/fonts` copy straight out.
//!
//! Point `FONTWICH_ANDROID_IMAGE` at a directory holding `fonts.xml` and a
//! `fonts/` beside it, and these run; without it they skip, like every other
//! test here that needs something the machine may not have.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::{Android, SystemFonts};
use crate::fallback::{sample_codepoints, sample_scripts};

/// The extracted image, or `None` where nobody has pointed at one.
fn image() -> Option<(String, String)> {
    let root = std::env::var("FONTWICH_ANDROID_IMAGE").ok()?;
    let xml = std::fs::read_to_string(format!("{root}/fonts.xml")).ok()?;
    Some((xml, format!("{root}/fonts")))
}

#[test]
fn every_font_the_file_names_is_a_file_the_image_ships() {
    let Some((xml, dir)) = image() else {
        return;
    };
    let config = super::config::parse(&xml).expect("the image's fonts.xml parses");
    let mut missing: Vec<String> = Vec::new();
    let mut named = 0;
    for family in &config.families {
        for font in &family.fonts {
            named += 1;
            if !std::path::Path::new(&format!("{dir}/{}", font.file)).exists() {
                let file = font.file.clone();
                if !missing.contains(&file) {
                    missing.push(file);
                }
            }
        }
    }
    assert!(named > 100, "only {named} <font> entries");
    assert!(
        missing.is_empty(),
        "{} named but absent: {missing:?}",
        missing.len()
    );
}

#[test]
fn a_script_the_tables_answer_is_a_script_the_named_fonts_draw() {
    let Some((xml, dir)) = image() else {
        return;
    };
    let source = Android::from_fonts_xml(&xml, &dir, SystemFonts).expect("parses");
    let (mut answered, mut drawn, mut failures) = (0, 0, Vec::new());
    for script in sample_scripts() {
        // Only where the file has fonts *for the script*: its own entry. A
        // stock image has no font for most of Unicode, and naming the rest
        // for those is right: the caller walks the list, finds nothing, and
        // draws tofu.
        let named: Vec<String> = source
            .index
            .script_fonts(script, false)
            .iter()
            .filter_map(|font| source.family_name(font))
            .map(|family| family.to_string())
            .collect();
        if named.is_empty() {
            continue;
        }
        answered += 1;
        let points = sample_codepoints(script);
        let covered = named.iter().any(|family| covers(&dir, family, points));
        if covered {
            drawn += 1;
        } else {
            failures.push(format!("{script}: {named:?} draw none of {points:04X?}"));
        }
    }
    assert!(answered > 30, "only {answered} scripts answered");
    assert!(
        failures.is_empty(),
        "{} of {answered} scripts named a family that cannot draw them:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
    assert_eq!(answered, drawn);
}

#[test]
fn a_page_naming_a_desktop_font_is_answered_through_the_aliases() {
    let Some((xml, dir)) = image() else {
        return;
    };
    let source = Android::from_fonts_xml(&xml, &dir, SystemFonts).expect("parses");
    let aliases = source.names();
    // Sixteen of AOSP's twenty-four aliases — the eight naming a weight are
    // left out — and four named families: the ten less the four that are CSS
    // generics and the two the file pins to axis values.
    assert_eq!(aliases.len(), 20, "{aliases:?}");
    let target = |name: &str| {
        aliases
            .iter()
            .find(|(alias, _)| alias == name)
            .map(|(_, family)| family.as_str())
    };
    assert_eq!(target("arial"), Some("Roboto"));
    assert_eq!(target("helvetica"), Some("Roboto"));
    assert_eq!(target("times new roman"), Some("Noto Serif"));
    assert_eq!(target("courier"), Some("Cutive Mono"));
    assert_eq!(target("sans-serif-light"), None);
    assert_eq!(target("casual"), Some("Coming Soon"));
    assert_eq!(target("sans-serif-smallcaps"), Some("Carrois Gothic SC"));
    assert_eq!(target("serif-monospace"), Some("Cutive Mono"));
    // Roboto at `wdth` 75, which a name given to a family cannot say.
    assert_eq!(target("sans-serif-condensed"), None);
    assert_eq!(target("roboto-flex"), None);
    // A generic stays a generic.
    assert_eq!(target("sans-serif"), None);
    assert_eq!(target("serif"), None);

    // Applied to a layer of the same fonts, the names answer.
    let mut builder = crate::LayerBuilder::new(crate::Role::System);
    builder.add_path(&dir);
    for (alias, family) in &aliases {
        builder.add_alias(alias, family);
    }
    let collection = crate::Collection::new().with_layer(builder.snapshot());
    let fonts = |name: &str| {
        collection
            .family(name)
            .map(|family| family.fonts().len())
            .unwrap_or(0)
    };
    assert!(fonts("Roboto") > 0);
    assert_eq!(fonts("arial"), fonts("Roboto"));
    assert_eq!(
        fonts("Arial"),
        fonts("Roboto"),
        "names match case-insensitively"
    );
    assert_eq!(fonts("georgia"), fonts("Noto Serif"));
    // An alias is a name, not a family: the layer lists no more families
    // than the scan found.
    let named = collection
        .layers()
        .flat_map(|layer| layer.names().map(String::from).collect::<Vec<_>>())
        .count();
    assert!(named > 100, "{named}");
    assert_eq!(fonts("sans-serif-light"), 0);
}

#[test]
fn the_system_layer_is_the_fonts_the_file_arranges() {
    // What `Layer::system` builds on a device, from the same four steps, on
    // whatever host is running this.
    let Some((_, dir)) = image() else {
        return;
    };
    let root = std::env::var("FONTWICH_ANDROID_IMAGE").expect("checked by `image`");
    let layer = super::layer::build(&format!("{root}/etc/fonts.xml"), &dir, None);
    let collection = crate::Collection::new().with_layer(alloc::sync::Arc::new(layer));

    let fonts = |name: &str| {
        collection
            .family(name)
            .map(|family| family.fonts().len())
            .unwrap_or(0)
    };
    // The fonts themselves, the names the file gives them, and fallback.
    assert!(fonts("Roboto") > 0, "the scan");
    assert_eq!(fonts("arial"), fonts("Roboto"), "the file's names");
    assert_eq!(fonts("casual"), fonts("Coming Soon"));

    let request = |script: [u8; 4]| crate::FallbackRequest::Text {
        script: crate::Script::from_bytes(script),
        language: None,
        generic: crate::GenericClass::Plain,
    };
    for (script, probe) in [(*b"Mymr", 'မ'), (*b"Jpan", 'あ'), (*b"Zsye", '😀')] {
        let request = request(script);
        let presentation = if probe == '😀' {
            crate::Presentation::Emoji
        } else {
            crate::Presentation::Text
        };
        let drawn = collection
            .fallback(&collection.key(&request))
            .iter()
            .cloned()
            .chain(collection.char_fallback(probe, presentation, &request))
            .find(|family| {
                family
                    .match_font(crate::Attributes::default(), true)
                    .is_some_and(|font| font.charset().contains(probe))
            })
            .map(|family| String::from(family.name()));
        assert!(drawn.is_some(), "nothing draws {probe:?}");
    }
}

#[test]
fn local_finds_the_image_s_fonts_by_name() {
    // `src: local("Roboto-Bold")` in an `@font-face`, on a layer that has no
    // loader to ask: Android's.
    let Some((_, dir)) = image() else {
        return;
    };
    let root = std::env::var("FONTWICH_ANDROID_IMAGE").expect("checked by `image`");
    let layer = super::layer::build(&format!("{root}/etc/fonts.xml"), &dir, None);

    let found = |name: &str| layer.local(name).is_some();
    // A PostScript name, and a full name of a font that has a distinct one.
    // Android 36 ships Roboto as one variable font whose full name is just
    // "Roboto", so its weights are axis values rather than files to find.
    assert!(found("Roboto-Regular"), "a PostScript name");
    assert!(found("CarroisGothicSC-Regular"));
    assert!(found("Coming Soon Regular"), "a full name");
    assert!(found("coming soon regular"), "matched as family names are");
    assert!(!found("No Such Font Anywhere"));

    // The full name that is also a family name finds the font, not the
    // family: one font, at the file's own default.
    let roboto = layer.local("Roboto").expect("its full name");
    assert!(roboto.axis(b"wght").is_some(), "the variable font itself");
}

/// Whether `family`, among the image's fonts, maps any of `points`.
fn covers(dir: &str, family: &str, points: &[u32]) -> bool {
    use crate::{Collection, LayerBuilder, Role};
    use std::sync::OnceLock;

    static SCAN: OnceLock<Collection> = OnceLock::new();
    let collection = SCAN.get_or_init(|| {
        let mut builder = LayerBuilder::new(Role::System);
        builder.add_path(dir);
        Collection::new().with_layer(builder.snapshot())
    });
    collection.family(family).is_some_and(|family| {
        family.fonts().iter().any(|font| {
            points
                .iter()
                .filter_map(|&point| char::from_u32(point))
                .any(|c| font.charset().contains(c))
        })
    })
}
