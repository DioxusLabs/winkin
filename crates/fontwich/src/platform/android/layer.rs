//! The system layer on Android: the fonts on disk, named as the platform
//! names them.
//!
//! Android has no font API to enumerate — `<android/font.h>` arrived with
//! Android 10, says nothing about generics, and its locale data was unusable
//! until API 30 — so this reads what the platform reads: the font files
//! themselves, and `fonts.xml` for the arrangement over them. Skia's
//! `SkFontMgr_android` does the same and is what Chrome ships.
//!
//! Four steps, each of which is a piece that stands on its own:
//!
//! 1. **Scan** `$ANDROID_ROOT/fonts`, and `/data/fonts/files` where a device
//!    has updated its fonts. That second directory is why the scan is a
//!    union: since Android 12 the system's fonts update without the system
//!    updating, which is how the emoji font learns new emoji, and the newer
//!    file lands there while the one it replaces stays in a read-only
//!    `/system`.
//! 2. **Drop what is superseded**, by the platform's own rule: same
//!    PostScript name, lower `head.fontRevision`. See
//!    [`Scanned::drop_superseded`](crate::Scanned::drop_superseded).
//! 3. **Read `fonts.xml`** as the fallback backend, which is where the
//!    per-script order and the generics live. The files it names are named
//!    by the family names the scan read, rather than by reading each file
//!    again.
//! 4. **Take the names it gives**, so that a page naming `arial` or `casual`
//!    is answered as the platform answers it.
//!
//! `ANDROID_ROOT` is honoured, as the platform and fontique both do, which is
//! also what lets this be tested against a system image on any host: see
//! `image.rs`.

#[cfg(any(target_os = "android", test))]
use alloc::string::String;
#[cfg(any(target_os = "android", test))]
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "android", test))]
use super::{Android, FamilyNames, SystemFonts};
#[cfg(any(target_os = "android", test))]
use crate::hash::HashMap;
#[cfg(any(target_os = "android", test))]
use crate::{Layer, Role, Scanned, Source};

/// Where Android keeps its fonts, and the fonts a device has updated.
#[cfg(target_os = "android")]
fn directories() -> (String, String, String) {
    let root = std::env::var("ANDROID_ROOT").unwrap_or_else(|_| String::from("/system"));
    let config = alloc::format!("{root}/etc/fonts.xml");
    let fonts = alloc::format!("{root}/fonts");
    (root, config, fonts)
}

/// The updated fonts, which are outside `$ANDROID_ROOT` and readable:
/// `/data/fonts/files(/.*)?` is `font_data_file`, which AOSP's own policy
/// allows every app to read.
#[cfg(target_os = "android")]
const UPDATED: &str = "/data/fonts/files";

/// The system layer; see [`Layer::system`].
#[cfg(target_os = "android")]
pub(crate) fn layer() -> Layer {
    let (_, config, fonts) = directories();
    build(&config, &fonts, Some(std::path::Path::new(UPDATED)))
}

/// The same, from stated paths, so that it can be built against a system
/// image on a host that is not a device: see `image.rs`.
#[cfg(any(target_os = "android", test))]
pub(crate) fn build(config: &str, fonts: &str, updated: Option<&std::path::Path>) -> Layer {
    let mut scanned = Scanned::path(std::path::Path::new(fonts));
    if let Some(updated) = updated.filter(|path| path.is_dir()) {
        scanned.extend(Scanned::path(updated));
        // Only then: a superseded font is only visible once both are in.
        scanned.drop_superseded();
    }

    let names = ScannedNames::new(&scanned);
    let mut builder = crate::LayerBuilder::new(Role::System);
    builder.add_scanned(scanned);

    // `fonts.xml` may be absent — an unusual device, or a root that has no
    // configuration — and a layer of the fonts alone is still worth having.
    let Ok(xml) = std::fs::read_to_string(config) else {
        return builder.layer().clone();
    };
    let Ok(backend) = Android::from_fonts_xml(&xml, fonts, names) else {
        return builder.layer().clone();
    };
    for (name, family) in backend.names() {
        builder.add_alias(&name, &family);
    }
    builder
        .layer()
        .clone()
        .with_fallback(crate::backend::Backend::Android(backend))
}

/// The family name the scan read of each font, by its file and index: what
/// [`Android`] asks of the files `fonts.xml` names, answered without opening
/// any of them again.
///
/// A file the scan did not keep — a font an update superseded, which
/// `fonts.xml` still names in the system's directory — is read as
/// [`SystemFonts`] reads it. Keyed by path rather than by string, so that a
/// name the backend joins with `/` finds the file the scan joined with the
/// platform's separator.
#[cfg(any(target_os = "android", test))]
#[derive(Debug)]
struct ScannedNames(HashMap<(PathBuf, u32), String>);

#[cfg(any(target_os = "android", test))]
impl ScannedNames {
    fn new(scanned: &Scanned) -> Self {
        let names = scanned
            .fonts
            .iter()
            .filter_map(|(name, _, font)| match font.source() {
                Source::Path(path) => Some(((path.to_path_buf(), font.index()), name.clone())),
                _ => None,
            })
            .collect();
        Self(names)
    }
}

#[cfg(any(target_os = "android", test))]
impl FamilyNames for ScannedNames {
    fn family_name(&self, path: &str, index: u32) -> Option<String> {
        match self.0.get(&(Path::new(path).to_path_buf(), index)) {
            Some(name) => Some(name.clone()),
            None => SystemFonts.family_name(path, index),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fonts::font_named;

    /// A root of its own in the temporary directory, removed when dropped.
    struct Root(PathBuf);

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_files_fonts_xml_names_are_named_by_the_scan() {
        let root = Root(
            std::env::temp_dir().join(alloc::format!("fontwich-{}-android", std::process::id())),
        );
        let fonts = root.0.join("fonts");
        std::fs::create_dir_all(&fonts).expect("a directory");
        std::fs::write(fonts.join("Alpha.ttf"), font_named("Alpha")).expect("a font");
        let xml = root.0.join("fonts.xml");
        std::fs::write(
            &xml,
            r#"<familyset version="23">
                <family name="sans-serif"><font weight="400" style="normal">Alpha.ttf</font></family>
                <family lang="und-Grek"><font weight="400" style="normal">Beta.ttf</font></family>
                <alias name="arial" to="sans-serif" />
            </familyset>"#,
        )
        .expect("a fonts.xml");
        let fonts = fonts.to_str().expect("a UTF-8 path");

        // The layer's backend names Alpha from the scan, and its alias finds it.
        let layer = build(xml.to_str().expect("a UTF-8 path"), fonts, None);
        let at = layer.find("arial").expect("the alias");
        assert_eq!(layer.record(at).name(), "Alpha");

        // Asked as the backend asks, the directory joined with `/`: from the
        // scan, with the file gone. One the scan did not see is read.
        let names = ScannedNames::new(&Scanned::path(Path::new(fonts)));
        std::fs::remove_file(Path::new(fonts).join("Alpha.ttf")).expect("removed");
        std::fs::write(Path::new(fonts).join("Beta.ttf"), font_named("Beta")).expect("a font");
        let path = |file: &str| alloc::format!("{fonts}/{file}");
        assert_eq!(
            names.family_name(&path("Alpha.ttf"), 0).as_deref(),
            Some("Alpha")
        );
        assert_eq!(names.family_name(&path("Alpha.ttf"), 1), None);
        assert_eq!(
            names.family_name(&path("Beta.ttf"), 0).as_deref(),
            Some("Beta")
        );
        assert_eq!(
            SystemFonts.family_name(&path("Beta.ttf"), 0).as_deref(),
            Some("Beta")
        );
        assert_eq!(names.family_name(&path("Gamma.ttf"), 0), None);
    }
}
