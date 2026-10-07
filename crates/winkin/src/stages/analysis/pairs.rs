//! Line-break opportunities in Latin-1 text from a pair table, which the
//! line stream reads before it asks ICU.
//!
//! The stream's opportunities are ICU's, and this table finds most of them
//! without ICU. It is ICU4X's line segmenter read off for the Latin-1
//! characters it decides by pairs. The test holds it to ICU for every pair
//! of the characters it covers, and every string of their classes up to six
//! characters long.
//!
//! It is not Blink's table, whose ASCII pairs follow Blink's own rules, not
//! ICU's. Those rules are `ascii`'s, which the writer asks before the stream
//! and which overrule it where they decide.
//!
//! ICU4X's segmenter is a state machine over a table of pairs of classes.
//! It forgets everything at an opportunity and goes on as from the text's
//! start. For the classes here, its table is a pair table with one extra
//! state: after an opening punctuation and the spaces after it, no line
//! breaks (UAX #14 LB14).
//!
//! The table leaves out everything else ICU keeps state for: the French
//! quotation marks of LB15a and LB15b, a combining mark's LB9, and a class
//! the options remap. It also leaves out the characters `line-break: loose`
//! reads apart in Japanese and Chinese (`¤`, `°`, `±`), and every control.
//! At any of them the table answers nothing, and the stream asks ICU from
//! the last opportunity.

use crate::work;

/// A class of the characters the table covers: the Line_Break property as
/// ICU4X's segmenter reads it (AI as AL), numbered for the table's bits.
type Class = u8;

const AL: Class = 0;
const NU: Class = 1;
const SP: Class = 2;
const BA: Class = 3;
const HY: Class = 4;
const IS: Class = 5;
const SY: Class = 6;
const EX: Class = 7;
const QU: Class = 8;
const OP: Class = 9;
const CP: Class = 10;
const CL: Class = 11;
const GL: Class = 12;
const PR: Class = 13;
const PO: Class = 14;
const BB: Class = 15;

/// A character the table does not cover.
const NONE: Class = 0xFF;

/// The state after an opening punctuation and one or more spaces, in which
/// no line breaks before whatever follows the spaces (LB14): the one state
/// that is not a class.
const OP_SP: u8 = 16;

/// The class of the code point `cp`, a Latin-1 character, or [`NONE`].
const fn class(cp: u8) -> Class {
    match cp {
        b'a'..=b'z'
        | b'A'..=b'Z'
        | b'#'
        | b'&'
        | b'*'
        | b'<'
        | b'='
        | b'>'
        | b'@'
        | b'^'
        | b'_'
        | b'`'
        | b'~'
        | 0xA6..=0xAA
        | 0xAC
        | 0xAE
        | 0xAF
        | 0xB2
        | 0xB3
        | 0xB5..=0xBA
        | 0xBC..=0xBE
        | 0xC0..=0xFF => AL,
        b'0'..=b'9' => NU,
        b' ' => SP,
        // A tab, the vertical line and the soft hyphen.
        b'\t' | b'|' | 0xAD => BA,
        b'-' => HY,
        b',' | b'.' | b':' | b';' => IS,
        b'/' => SY,
        b'!' | b'?' => EX,
        b'"' | b'\'' => QU,
        // And the inverted exclamation and question marks.
        b'(' | b'[' | b'{' | 0xA1 | 0xBF => OP,
        b')' | b']' => CP,
        b'}' => CL,
        // The no-break space.
        0xA0 => GL,
        // And the pound and yen signs.
        b'$' | b'+' | b'\\' | 0xA3 | 0xA5 => PR,
        // And the cent sign.
        b'%' | 0xA2 => PO,
        // The acute accent.
        0xB4 => BB,
        _ => NONE,
    }
}

/// Every Latin-1 code point's class.
static CLASSES: [Class; 256] = {
    let mut classes = [NONE; 256];
    let mut cp = 0;
    while cp < 256 {
        classes[cp] = class(cp as u8);
        cp += 1;
    }
    classes
};

/// A row of the table: a bit for each class a line may break before.
const fn before(classes: &[Class]) -> u16 {
    let mut bits = 0;
    let mut at = 0;
    while at < classes.len() {
        bits |= 1 << classes[at];
        at += 1;
    }
    bits
}

/// ICU4X's pair table for these classes, one row per state.
///
/// A row says whether a line may break between a character in that state,
/// a class or [`OP_SP`], and one of each class after it.
static BREAKS: [u16; 17] = [
    /* AL */ before(&[BB]),
    /* NU */ before(&[BB]),
    /* SP */ before(&[AL, NU, BA, HY, QU, OP, GL, PR, PO, BB]),
    /* BA */ before(&[AL, NU, OP, GL, PR, PO, BB]),
    /* HY */ before(&[AL, OP, GL, PR, PO, BB]),
    /* IS */ before(&[OP, PR, PO, BB]),
    /* SY */ before(&[AL, OP, PR, PO, BB]),
    /* EX */ before(&[AL, NU, OP, PR, PO, BB]),
    /* QU */ before(&[]),
    /* OP */ before(&[]),
    /* CP */ before(&[OP, BB]),
    /* CL */ before(&[AL, NU, OP, BB]),
    /* GL */ before(&[]),
    /* PR */ before(&[PR, PO, BB]),
    /* PO */ before(&[PR, PO, BB]),
    /* BB */ before(&[]),
    /* OP_SP */ before(&[]),
];

/// The class of the character at byte `at` of `bytes` and its length in
/// bytes, where the table covers it: an ASCII byte, or a two-byte sequence
/// for U+0080 to U+00FF.
#[inline]
fn table_class(bytes: &[u8], at: usize) -> Option<(Class, usize)> {
    let lead = *bytes.get(at)?;
    let (cp, len) = if lead < 0x80 {
        (usize::from(lead), 1)
    } else if lead & 0xFE == 0xC2 {
        let trail = *bytes.get(at + 1)?;
        (
            (usize::from(lead & 0x1F) << 6) | usize::from(trail & 0x3F),
            2,
        )
    } else {
        return None;
    };
    let class = *CLASSES.get(cp)?;
    (class != NONE).then_some((class, len))
}

/// The next opportunity after byte `from` of `text`, if the table decides
/// every pair up to it.
///
/// The segmenter would go on from `from` as from the text's start. Returns
/// the text's end if the text ends first. Returns `None` if a character the
/// table does not cover comes first, or starts at `from`.
pub(super) fn next_break(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let (mut state, len) = table_class(bytes, from)?;
    let mut at = from + len;
    while at < bytes.len() {
        work::step();
        let (class, len) = table_class(bytes, at)?;
        let row = BREAKS.get(usize::from(state)).copied().unwrap_or(0);
        if row & (1 << class) != 0 {
            return Some(at);
        }
        state = if class == SP && (state == OP || state == OP_SP) {
            OP_SP
        } else {
            class
        };
        at += len;
    }
    Some(bytes.len())
}

/// Whether the character at byte `at` of `text` may be one the table
/// covers, by its first byte alone.
///
/// The test is cheap enough to make at every opportunity before
/// [`next_break`] is asked, even in text the table covers none of, such as
/// ideographs.
#[inline]
pub(super) fn may_cover(text: &str, at: usize) -> bool {
    text.as_bytes().get(at).is_some_and(|&lead| lead < 0xC4)
}

/// Whether the table covers `ch`: what the test sweeps.
#[cfg(test)]
pub(super) fn covers(ch: char) -> bool {
    u8::try_from(u32::from(ch)).is_ok_and(|cp| class(cp) != NONE)
}
