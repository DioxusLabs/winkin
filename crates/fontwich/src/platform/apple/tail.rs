//! The fonts outside Core Text's cascade.
//!
//! The backend takes nearly everything from Core Text's cascade. This adds
//! the emoji font, the symbol and math fonts, and last-resort fonts every
//! macOS release ships.

/// The emoji key's answer for emoji presentation.
pub static EMOJI: &[&str] = &["Apple Color Emoji"];

/// Symbol and math fonts, after the Common key's cascade. The cascade
/// usually lists them already. Listing them again backstops an empty
/// cascade.
pub static SYMBOLS: &[&str] = &["Apple Symbols", "STIX Two Math"];

/// Fonts every supported macOS release ships, which end the Common key's
/// answer.
pub static LAST_RESORT: &[&str] = &["Helvetica", "Lucida Grande", ".AppleSystemUIFont"];
