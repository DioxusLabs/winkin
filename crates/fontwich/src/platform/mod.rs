//! The platforms: each one's fallback backend, and the layer listing its
//! installed fonts, side by side.
//!
//! Private. The backends are public through [`crate::backend`], and the
//! system layer through [`Layer::system`](crate::Layer::system). The rest,
//! such as tail lists, script tables and generic families, is data behind
//! them.

// A feature, so every host builds and tests it, and the target's own
// backend besides. See the `android` feature in Cargo.toml.
#[cfg(fontwich_android)]
mod android;
#[cfg(all(target_vendor = "apple", feature = "system"))]
mod apple;
#[cfg(fontwich_fontconfig)]
mod fontconfig;
// Data, not calls, so every host compiles it under `test` — that is how the
// oracle checks it against Chrome and Unicode from any machine — and no
// other target carries what it cannot reach.
#[cfg(any(windows, test))]
mod windows;

// Public through `crate::backend`.
#[cfg(fontwich_android)]
pub use android::{Android, ConfigError, FamilyNames, SystemFonts};
#[cfg(all(target_vendor = "apple", feature = "system"))]
pub use apple::MacOs;
#[cfg(fontwich_fontconfig)]
pub use fontconfig::Fontconfig;
#[cfg(any(windows, test))]
pub use windows::Windows;

#[cfg(all(any(windows, unix), feature = "system"))]
use crate::Layer;

/// Returns a layer of the platform's installed fonts: [`Layer::system`].
#[cfg(all(any(windows, unix), feature = "system"))]
pub(crate) fn system_layer() -> Layer {
    #[cfg(windows)]
    let layer = windows::layer();
    #[cfg(target_vendor = "apple")]
    let layer = apple::layer();
    // Android is `unix` and has no fontconfig: its fonts are files on
    // disk, arranged by `fonts.xml`.
    #[cfg(target_os = "android")]
    let layer = android::layer();
    #[cfg(fontwich_fontconfig)]
    let layer = fontconfig::layer();
    layer
}
