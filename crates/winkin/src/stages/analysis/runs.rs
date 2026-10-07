//! Stored runs from text analysis.

use parlance::Script;

use crate::data::{Run, Runs};
use crate::stages::content::LanguageId;
use crate::style::BaseDirection;

use super::{ClusterId, ScriptRunId};

/// A resolved Unicode bidi level: even for LTR, odd for RTL.
///
/// Returned by [`Line::level`](crate::Line::level),
/// [`TextRun::level`](crate::TextRun::level),
/// [`BoxFragment::level`](crate::BoxFragment::level) and
/// [`Atomic::level`](crate::Atomic::level). Higher levels represent deeper
/// embedding. Use [`is_rtl`](Self::is_rtl) to query direction.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BidiLevel(u8);

impl BidiLevel {
    /// Level 0, a left-to-right paragraph's: what stands where nothing
    /// resolved one, as before any paragraph.
    pub(crate) const LTR: Self = Self(0);

    /// The level `level`, as the analysis resolved it.
    pub(crate) const fn new(level: u8) -> Self {
        Self(level)
    }

    /// The level a base direction sets its paragraphs at: 0 left to right,
    /// 1 right to left, and `None` under `auto`.
    ///
    /// Under `auto`, each paragraph's first strong character decides (UAX #9
    /// rules P2 and P3). This lives on the level because `BaseDirection` is
    /// parlance's and cannot take this crate's methods.
    pub(crate) const fn from_direction(direction: BaseDirection) -> Option<Self> {
        match direction {
            BaseDirection::Ltr => Some(Self(0)),
            BaseDirection::Rtl => Some(Self(1)),
            BaseDirection::Auto => None,
        }
    }

    /// Returns the numeric level, from 0 to 126.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Returns `true` if the level is odd (RTL).
    pub const fn is_rtl(self) -> bool {
        self.0 % 2 == 1
    }
}

/// A maximal run of clusters with one script, language, level and
/// orientation, in 12 bytes.
///
/// Runs tile the clusters, and never cross a paragraph. Each ends where the
/// next starts, the last where the clusters do.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct ScriptRun {
    /// Its first cluster.
    pub(crate) start: ClusterId,
    /// The script, resolved as UAX #24 does, with Script_Extensions and
    /// paired brackets (see `scripts`).
    pub(crate) script: Script,
    /// The language, as the content's own id for it.
    pub(crate) language: LanguageId,
    /// Its bidi level.
    pub(crate) level: BidiLevel,
    /// How its glyphs stand.
    pub(crate) orientation: RunOrientation,
}

impl Run for ScriptRun {
    type Position = ClusterId;

    #[inline]
    fn start(&self) -> ClusterId {
        self.start
    }
}

/// The runs of script, language, level and orientation, in text order:
/// which run a cluster is in, and what each is.
///
/// They tile the clusters, the last ending at the text's end. A stage
/// walking the clusters forward keeps a cursor into them and passes each
/// run once.
pub(crate) type ScriptRuns = Runs<ScriptRunId, ScriptRun>;

/// The rendering orientation of a text run.
///
/// Returned by [`TextRun::orientation`](crate::layout::TextRun::orientation).
/// Positions remain in the line frame for every writing mode. Map that
/// frame to the page, rotating sideways glyphs with the line and leaving
/// upright or combined glyphs upright at their origins.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum RunOrientation {
    /// Text in a horizontal line (`horizontal-tb`).
    Horizontal,
    /// Upright text in a vertical line, shaped along the line.
    ///
    /// Each glyph is centered across the line. Its origin lies half the glyph
    /// width under the center of the line, and the height of its vertical
    /// origin along the line. Applies to ideographs and kana under
    /// `text-orientation: mixed`, and to all characters except those of
    /// vertical scripts under `text-orientation: upright`.
    Upright,
    /// Sideways text in a vertical line, shaped as horizontal text and
    /// rotated with the line.
    ///
    /// Applies to Latin text under `text-orientation: mixed`, to all
    /// characters under `text-orientation: sideways`, and to all characters
    /// in the `sideways-rl` and `sideways-lr` writing modes.
    Sideways,
    /// Combined text (`text-combine-upright`), upright within one em.
    ///
    /// Shaped as horizontal text, fitted to one em, and set upright within
    /// one em of the line, centered across it.
    Combined,
}
