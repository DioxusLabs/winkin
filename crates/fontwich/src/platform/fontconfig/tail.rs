//! The fonts outside fontconfig's sort.
//!
//! None of these is guaranteed: a Linux system ships whatever its packager
//! chose, and a container may ship nothing. These are the families most
//! likely present across mainstream distributions, in that order. They
//! matter most when libfontconfig cannot be loaded, since
//! [`Fontconfig`](super::Fontconfig) then answers with these alone.

/// The emoji key's answer for emoji presentation.
pub static EMOJI: &[&str] = &["Noto Color Emoji", "Twemoji", "JoyPixels", "Noto Emoji"];

/// Symbol and math fonts.
pub static SYMBOLS: &[&str] = &["Noto Sans Symbols 2", "Noto Sans Symbols", "Noto Sans Math"];

/// The closest thing to a guarantee on a Linux system.
///
/// DejaVu is the default on most desktop distributions, Liberation on
/// Red Hat-derived ones, and Noto is what a modern minimal image is likeliest
/// to carry.
pub static LAST_RESORT: &[&str] = &[
    "DejaVu Sans",
    "Liberation Sans",
    "Noto Sans",
    "FreeSans",
    "Bitstream Vera Sans",
];
