//! The ICU4X line and word segmenters, as the `dictionaries` feature
//! chooses them. This is the one place the feature is read.
//!
//! **With dictionaries** (the default), lines break by ICU's Southeast Asian
//! dictionaries in Thai, Lao, Khmer and Myanmar (1.77 MB). Words are found
//! by those and its Chinese and Japanese dictionary (2.0 MB). This matches
//! Chrome, which segments with ICU4C and its break dictionaries.
//!
//! **Without**, lines and words break by ICU's LSTM in those four scripts
//! (310.5 KB). ICU4X has no model for Chinese and Japanese words, so they
//! follow UAX #29's own rules. Every ideograph and every hiragana is a word
//! of its own (Word_Break Other, WB999), and a run of katakana is one
//! (WB13). ICU's word segmenter would hand a run of ideographs and hiragana
//! to the missing dictionary and make the whole run one word, allocating to
//! do so. So each such character is replaced by one of the same Word_Break,
//! which its rules segment ([`segmenter_char`]).
//!
//! Baked data is linked only where its constructor is called, so each mode
//! carries only its own data, although icu_segmenter's LSTM code is
//! compiled in both. A release binary that breaks lines and moves by words
//! measures 6.71 MB with dictionaries and 3.24 MB without.
//!
//! **Words are Chrome's** wherever they are found, by `capitalize` and by
//! word motion alike ([`word_boundaries`]). They are UAX #29's, with the
//! tailoring of the `en_US_POSIX` rules Blink breaks words by.

use core::iter;

use icu_segmenter::options::{LineBreakOptions, WordBreakInvariantOptions};
use icu_segmenter::{LineSegmenter, LineSegmenterBorrowed, WordSegmenter, WordSegmenterBorrowed};

use super::{CoreProps, ScriptId, core_props, script};
use crate::work;

/// Whether this build segments by ICU's dictionaries: the `dictionaries`
/// feature.
const DICTIONARIES: bool = cfg!(feature = "dictionaries");

/// The line segmenter for `options`: with the Southeast Asian dictionaries,
/// or without them with the LSTM.
///
/// Making one is four lookups in baked data either way, with no allocation.
pub(crate) fn line_segmenter(options: LineBreakOptions<'static>) -> LineSegmenterBorrowed<'static> {
    #[cfg(feature = "dictionaries")]
    let segmenter = LineSegmenter::new_dictionary(options);
    #[cfg(not(feature = "dictionaries"))]
    let segmenter = LineSegmenter::new_lstm(options);
    segmenter
}

/// The word segmenter: with the Southeast Asian dictionaries and the Chinese
/// and Japanese one, or without them with the LSTM.
///
/// Making one is five or four lookups in baked data, with no allocation, so
/// a caller makes one where it segments.
pub(crate) fn word_segmenter() -> WordSegmenterBorrowed<'static> {
    let options = WordBreakInvariantOptions::default();
    #[cfg(feature = "dictionaries")]
    let segmenter = WordSegmenter::new_dictionary(options);
    #[cfg(not(feature = "dictionaries"))]
    let segmenter = WordSegmenter::new_lstm(options);
    segmenter
}

/// Returns the word boundaries of `text` by `segmenter`, in order, as Chrome
/// finds them.
///
/// The boundaries are the segmenter's, plus one either side of each full
/// stop between two letters, or a letter and a digit, inside a segment.
/// ICU's `en_US_POSIX` rules, which Blink breaks words by, join words at a
/// full stop only between digits. So `x.y` is the words `x`, `.` and `y`,
/// and `3.14` is one (measured in Chrome 153).
///
/// Full stops are looked for from byte `from` on. The text before it is
/// context the segmenter reads, and the caller does not ask for its words.
/// `text-transform: capitalize` and word motion both find words by this.
pub(crate) fn word_boundaries<'t>(
    segmenter: WordSegmenterBorrowed<'static>,
    text: &'t str,
    from: usize,
) -> impl Iterator<Item = usize> + 't {
    let mut segments = segmenter.segment_str(text);
    // The segment being cut: its start, the text still to look at for full
    // stops and where that is, the character before it, the cut after the
    // full stop cut last, and the segment's end, which comes after its cuts.
    let mut start = 0;
    let mut rest = "";
    let mut at = 0;
    let mut previous: Option<char> = None;
    let mut after_stop = None;
    let mut end = None;
    iter::from_fn(move || {
        if let Some(after) = after_stop.take() {
            return Some(after);
        }
        loop {
            let mut chars = rest.chars();
            while let Some(ch) = chars.next() {
                work::step();
                let stop = at;
                at += ch.len_utf8();
                rest = chars.as_str();
                let before = previous.replace(ch);
                if ch == '.'
                    && let Some(next) = rest.chars().next()
                    && before.is_some_and(char::is_alphanumeric)
                    && next.is_alphanumeric()
                    && !(before.is_some_and(char::is_numeric) && next.is_numeric())
                {
                    after_stop = Some(at);
                    return Some(stop);
                }
            }
            if let Some(end) = end.take() {
                return Some(end);
            }
            let next = segments.next()?;
            let begin = start.max(from);
            rest = text.get(begin..next).unwrap_or_default();
            at = begin;
            previous = None;
            start = next;
            end = Some(next);
        }
    })
}

/// Whether ICU's word segmenter hands `ch` to its dictionaries or its LSTM.
///
/// Those segment a run of such characters whole, since where a word ends
/// depends on all of it. They are Line_Break SA, and with dictionaries
/// Script Han and Hiragana, as ICU4X's word data marks them (its complex
/// property; `a_complex_character_is_icus`). Without dictionaries,
/// [`segmenter_char`] replaces an ideograph or a hiragana before ICU sees it.
#[inline]
pub(crate) fn is_complex(ch: char) -> bool {
    let props = core_props(ch);
    props.is_complex_context() || (DICTIONARIES && is_han_or_hiragana(props))
}

/// The character ICU's word segmenter is given in `ch`'s place, if it is
/// not `ch`.
///
/// Without dictionaries, an ideograph or a hiragana is replaced by a
/// character that ICU's rules segment. The stand-in has the same Word_Break
/// and UTF-8 length. It is a letter or a digit where `ch` is one, for
/// Chrome's full stops ([`word_boundaries`]), unless UAX #29 already parts
/// it from them.
///
/// Nearly all are Other. `々`, `〻` and U+16FE3 are ALetter, and the two
/// Vietnamese alternate reading marks are Extend
/// (`a_stand_in_has_its_characters_word_break`). Returns `None` for every
/// other character, and for all with dictionaries.
#[inline]
pub(crate) fn segmenter_char(ch: char) -> Option<char> {
    if DICTIONARIES || !is_han_or_hiragana(core_props(ch)) {
        return None;
    }
    Some(match ch {
        // ALetter, as `Ḁ` and `𐀀` are.
        '\u{3005}' | '\u{303B}' => '\u{1E00}',
        '\u{16FE3}' => '\u{10000}',
        // Extend, a spacing mark, as the Brahmi candrabindu is.
        '\u{16FF0}' | '\u{16FF1}' => '\u{11000}',
        // Other, as `〒` and `🄍` are.
        _ if ch.len_utf8() == 4 => '\u{1F10D}',
        _ => '\u{3012}',
    })
}

/// Whether `text` holds a character [`segmenter_char`] stands in for: never
/// with dictionaries, where nothing is read.
#[inline]
pub(crate) fn has_stand_ins(text: &str) -> bool {
    !DICTIONARIES && text.chars().any(|ch| segmenter_char(ch).is_some())
}

/// Whether `props`'s character's Script is Han or Hiragana.
#[inline]
fn is_han_or_hiragana(props: CoreProps) -> bool {
    matches!(script(props), ScriptId::HAN | ScriptId::HIRAGANA)
}
