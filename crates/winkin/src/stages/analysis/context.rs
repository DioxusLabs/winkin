//! The segmenters analysis caches in the [`Context`](crate::Context).

use core::array;

use icu_segmenter::{
    GraphemeClusterSegmenter, GraphemeClusterSegmenterBorrowed, LineSegmenterBorrowed,
};

use super::breaks::LineKey;
use crate::unicode::line_segmenter;

/// The segmenters analysis runs, kept in the [`Context`](crate::Context) so
/// they outlive every layout.
///
/// It holds a line segmenter for each [`LineKey`], and the grapheme
/// segmenter. Each is `Copy` and holds only references to ICU's compiled
/// data, so making one is a lookup that allocates nothing.
///
/// Every line segmenter loads the Thai, Lao, Khmer and Myanmar breakers when
/// the context is made (`unicode::line_segmenter`). These are the Southeast Asian
/// dictionaries, or the LSTM without the `dictionaries` feature. Each is
/// four lookups in baked data, and text without those scripts pays nothing.
pub(crate) struct AnalysisContext {
    lines: [LineSegmenterBorrowed<'static>; LineKey::COUNT],
    graphemes: GraphemeClusterSegmenterBorrowed<'static>,
}

impl AnalysisContext {
    /// The segmenters, made.
    pub(crate) fn new() -> Self {
        Self {
            lines: array::from_fn(|index| line_segmenter(LineKey::from_index(index).options())),
            graphemes: GraphemeClusterSegmenter::new(),
        }
    }

    /// The line segmenter for `key`.
    pub(super) fn line(&self, key: LineKey) -> LineSegmenterBorrowed<'static> {
        // Every key's index is a slot: `index` and `COUNT` are written
        // together.
        match self.lines.get(key.index()) {
            Some(segmenter) => *segmenter,
            None => self.lines[0],
        }
    }

    /// The grapheme segmenter (UAX #29 17.0).
    pub(super) fn graphemes(&self) -> GraphemeClusterSegmenterBorrowed<'static> {
        self.graphemes
    }
}

impl Default for AnalysisContext {
    fn default() -> Self {
        Self::new()
    }
}
