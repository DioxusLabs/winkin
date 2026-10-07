//! The script tags this engine names directly.
//!
//! [`Script`] itself comes from `parlance`, so it is the
//! same type parley and fontique use. These constants are only the handful of
//! tags the resolution logic and the platform tables mention by name;
//! everything else lives in the data tables and needs no constant.
//!
//! `Zyyy`, `Zinh` and `Zzzz` are not here — parlance spells those
//! [`Script::COMMON`], [`Script::INHERITED`] and [`Script::UNKNOWN`].

// The set is complete on purpose: which of these a build actually names
// depends on the backend it compiles, so most are unused on any one platform.
// Trimming to whatever the current target happens to reference would make the
// table platform-specific for no gain.
#![allow(dead_code)]

use parlance::Script;

macro_rules! scripts {
    ($($(#[$doc:meta])* $name:ident = $tag:literal,)*) => {
        $($(#[$doc])* pub const $name: Script = Script::from_bytes(*$tag);)*
    };
}

scripts! {
    ARAB = b"Arab",
    ARMN = b"Armn",
    BENG = b"Beng",
    BOPO = b"Bopo",
    CANS = b"Cans",
    CHER = b"Cher",
    COPT = b"Copt",
    CYRL = b"Cyrl",
    DEVA = b"Deva",
    ETHI = b"Ethi",
    GEOR = b"Geor",
    GOTH = b"Goth",
    GREK = b"Grek",
    GUJR = b"Gujr",
    GURU = b"Guru",
    HANG = b"Hang",
    HANI = b"Hani",
    HANS = b"Hans",
    HANT = b"Hant",
    HEBR = b"Hebr",
    HIRA = b"Hira",
    HRKT = b"Hrkt",
    JPAN = b"Jpan",
    KANA = b"Kana",
    KHMR = b"Khmr",
    KNDA = b"Knda",
    KORE = b"Kore",
    LAOO = b"Laoo",
    LATN = b"Latn",
    MLYM = b"Mlym",
    MONG = b"Mong",
    MYMR = b"Mymr",
    NKOO = b"Nkoo",
    ORYA = b"Orya",
    SINH = b"Sinh",
    SYRC = b"Syrc",
    TAML = b"Taml",
    TELU = b"Telu",
    THAA = b"Thaa",
    THAI = b"Thai",
    TIBT = b"Tibt",
    YIII = b"Yiii",
    /// Math notation. Not an ISO 15924 script, but what a caller has in hand
    /// when it knows it is setting mathematics.
    ZMTH = b"Zmth",
    /// Symbols.
    ZSYM = b"Zsym",
    /// Emoji.
    ZSYE = b"Zsye",
}

/// The tag packed big-endian, for ordering and binary search.
pub(crate) fn key(script: Script) -> u32 {
    u32::from_be_bytes(script.to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_round_trip() {
        assert_eq!(HANT.as_str(), "Hant");
        assert_eq!(Script::parse("hant").unwrap(), HANT);
        assert_eq!(Script::parse("LATN").unwrap(), LATN);
        assert!(Script::parse("Dev4").is_err());
    }

    #[test]
    fn key_orders_like_the_string() {
        assert!(key(HANI) < key(HANS));
        assert!(key(HANS) < key(HANT));
    }
}
