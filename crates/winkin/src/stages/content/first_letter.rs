//! Where `::first-letter` ends.
//!
//! The first letter's text follows CSS Pseudo-Elements 4, section 2.2.1, as
//! Blink's `FirstLetterPseudoElement::FirstLetterLength` finds it:
//!
//! - **White space before it** that collapsing removes at the block's start
//!   is passed over. Kept white space, a tab or a no-break space among it,
//!   is part of it, as Blink counts leading spaces into the letter's length
//!   (`IsSpaceForFirstLetter`).
//! - **Punctuation before the letter** (any General_Category P) is part of
//!   it. Once some has come, so is the typographic space between it and the
//!   letter: Zs, except U+3000 IDEOGRAPHIC SPACE.
//! - **The letter** is one grapheme cluster, whatever it is: a digit or a
//!   symbol as much as a letter. UAX #29's rules find it, as the analysis
//!   finds its clusters, never a list of combining marks.
//! - **Punctuation after it** is part of it, except opening punctuation and
//!   dashes (Ps, Pd). So is the space between the two, except the word
//!   separators U+0020 and U+00A0, and U+3000. That space belongs to the
//!   letter only where punctuation follows it.
//!
//! Each grapheme is classed by its first character, as Blink tests the code
//! point at a grapheme's start. The text is read as white space collapsing
//! leaves it:
//! - A collapsible space, tab or segment break is a space between words.
//! - A kept segment break, or a character that forces a break in every
//!   mode, ends the first line and with it the search.
//! - White space `discard` removes is not there at all.
//!
//! Chrome reads the text as the caller wrote it and simulates collapsing.
//! So in Chrome a segment break after an opening quotation mark ends the
//! search where a space would not. CSS reads the text as drawn, and so does
//! this.
//!
//! **Across texts.** Punctuation that ends a text leaves the letter to be
//! found in the next, as Blink's `LeadingPunctuationState` does. The box
//! then holds only the first text's part, as Chrome's does, which CSS allows
//! where the first-letter text spans elements. A text of white space alone
//! leaves the search where it was.
//!
//! **Chrome 153** reads CSS 2.1's older rule: the punctuation classes Ps,
//! Pe, Pi, Pf and Po on both sides, and no space between the punctuation
//! and the letter. So `“ Once` has no first letter there, and a leading dash
//! is taken for the letter. Blink's current code follows CSS
//! Pseudo-Elements 4, and so does this.

use icu_segmenter::GraphemeClusterSegmenter;

use crate::style::WhiteSpaceCollapse;
use crate::unicode::{self, FirstLetterClass};
use crate::work;

/// What one text holds of the first letter, in its own bytes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum FirstLetterScan {
    /// White space alone. The search goes on in the next text.
    Nothing,
    /// Something that cannot be first-letter text came before any letter:
    /// a forced break, or a space that no punctuation or letter may stand
    /// beside. There is no first letter.
    Ends,
    /// The first letter's text starts at `start` and ends at `end`.
    ///
    /// `found` says whether the letter itself is in it. Where it is not,
    /// the text ended in punctuation before one, and the search goes on in
    /// the next text.
    Text {
        start: usize,
        end: usize,
        found: bool,
    },
}

/// A grapheme as the search reads it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Seen {
    /// White space `discard` removes, which is not there at all.
    Nothing,
    /// A space, a tab or a segment break that collapses, which reads as a
    /// space between words, U+0020.
    Collapsible,
    /// A kept segment break, or a character that forces a line break in
    /// every mode. The first line ends here.
    Break,
    /// A character that is drawn, with its class, whether it is White_Space,
    /// and whether it is a word separator, U+0020 or U+00A0.
    Drawn {
        class: FirstLetterClass,
        white: bool,
        separator: bool,
    },
}

impl Seen {
    /// Classes the grapheme `grapheme`, in text whose white space collapses
    /// as `mode` says.
    fn new(grapheme: &str, mode: WhiteSpaceCollapse) -> Self {
        use WhiteSpaceCollapse as Ws;
        let Some(first) = grapheme.chars().next() else {
            return Self::Nothing;
        };
        // A kept space is U+0020, a word separator.
        let kept_space = Self::Drawn {
            class: FirstLetterClass::Space,
            white: true,
            separator: true,
        };
        match first {
            // A CRLF reads as its LF. A lone CR is a space in all respects,
            // as white space collapsing treats it.
            '\r' if grapheme.contains('\n') => Self::new("\n", mode),
            ' ' | '\t' | '\r' => match mode {
                Ws::Discard => Self::Nothing,
                Ws::Collapse | Ws::PreserveBreaks => Self::Collapsible,
                Ws::Preserve | Ws::BreakSpaces | Ws::PreserveSpaces if first == '\t' => {
                    Self::Drawn {
                        class: FirstLetterClass::Other,
                        white: true,
                        separator: false,
                    }
                }
                Ws::Preserve | Ws::BreakSpaces | Ws::PreserveSpaces => kept_space,
            },
            '\n' => match mode {
                Ws::Discard => Self::Nothing,
                Ws::Collapse => Self::Collapsible,
                Ws::PreserveSpaces => kept_space,
                Ws::Preserve | Ws::BreakSpaces | Ws::PreserveBreaks => Self::Break,
            },
            // VT, FF, NEL, U+2028 and U+2029 break the line in every mode.
            '\u{B}' | '\u{C}' | '\u{85}' | '\u{2028}' | '\u{2029}' => Self::Break,
            _ => Self::Drawn {
                class: unicode::rare_props(first).first_letter(),
                white: unicode::core_props(first).is_white_space(),
                separator: first == '\u{A0}',
            },
        }
    }
}

impl FirstLetterScan {
    /// Finds the first letter's text in `text`, whose white space collapses
    /// as `mode` says.
    ///
    /// Without `after_punctuation`, nothing of it was found before. White
    /// space before it is passed over where it collapses, and taken where
    /// it is kept. With it, the punctuation
    /// before the letter came in an earlier text whose part the box holds,
    /// and the letter is still to be found.
    ///
    /// The text is read a grapheme at a time. A text ending in punctuation
    /// before the letter gives `Text { found: false, .. }`, and the search
    /// goes on after punctuation. Space after the letter that no
    /// punctuation follows before the text ends is left out. The next text
    /// is another node's, and Blink ignores punctuation after the letter in
    /// the text nodes that follow.
    pub(super) fn new(text: &str, mode: WhiteSpaceCollapse, after_punctuation: bool) -> Self {
        /// Where the scan is inside the text.
        #[derive(Copy, Clone)]
        enum Phase {
            Leading,
            Punctuation,
            /// After the letter, with where the text taken so far ends.
            Trailing(usize),
        }
        let mut phase = if after_punctuation {
            Phase::Punctuation
        } else {
            Phase::Leading
        };
        let mut start = 0;
        let mut boundaries = GraphemeClusterSegmenter::new().segment_str(text);
        // The segmenter says 0 first.
        let mut at = boundaries.next().unwrap_or(0);
        for next in boundaries {
            work::step();
            let seen = Seen::new(text.get(at..next).unwrap_or_default(), mode);
            at = next;
            phase = match (phase, seen) {
                (_, Seen::Nothing) => phase,
                (Phase::Leading | Phase::Punctuation, Seen::Break) => {
                    return Self::Ends;
                }
                (Phase::Leading, Seen::Collapsible) => {
                    start = next;
                    Phase::Leading
                }
                (Phase::Leading, Seen::Drawn { white: true, .. }) => Phase::Leading,
                (Phase::Leading, Seen::Drawn { class, .. }) => match class {
                    FirstLetterClass::Opening | FirstLetterClass::Punctuation => Phase::Punctuation,
                    _ => Phase::Trailing(next),
                },
                (
                    Phase::Punctuation,
                    Seen::Collapsible
                    | Seen::Drawn {
                        class:
                            FirstLetterClass::Space
                            | FirstLetterClass::Opening
                            | FirstLetterClass::Punctuation,
                        ..
                    },
                ) => Phase::Punctuation,
                (Phase::Punctuation, Seen::Drawn { white: true, .. }) => {
                    return Self::Ends;
                }
                (Phase::Punctuation, Seen::Drawn { .. }) => Phase::Trailing(next),
                (
                    Phase::Trailing(_),
                    Seen::Drawn {
                        class: FirstLetterClass::Punctuation,
                        ..
                    },
                ) => Phase::Trailing(next),
                // The letter takes this space only where punctuation follows
                // it, so `phase` still ends before it.
                (
                    Phase::Trailing(_),
                    Seen::Drawn {
                        class: FirstLetterClass::Space,
                        separator: false,
                        ..
                    },
                ) => phase,
                (Phase::Trailing(end), _) => {
                    return Self::Text {
                        start,
                        end,
                        found: true,
                    };
                }
            };
        }
        match phase {
            Phase::Leading => Self::Nothing,
            Phase::Punctuation => Self::Text {
                start,
                end: text.len(),
                found: false,
            },
            Phase::Trailing(end) => Self::Text {
                start,
                end,
                found: true,
            },
        }
    }
}
