//! Text analysis: clusters, break opportunities, paragraphs and runs.
//!
//! In: [`AnalysisInput`], the content. Out: [`Analysis`].
//! Start at: [`analyze`], then `ClusterWriter::visit_item` in `walk`.
//!
//! - `walk` walks the items and the text once and writes every table.
//! - `breaks` holds the line segmenters' keys and the line stream.
//! - `ascii` and `pairs` answer most Latin breaks without ICU.
//! - `scripts` resolves each cluster's script and splits the runs.
//! - `levels` builds each paragraph's bidi input and resolves its levels.
//! - `context` caches the segmenters in the [`Context`](crate::Context).
//! - `clusters`, `items`, `paragraphs` and `runs` own the stored tables.
//!
//! Every per-character table is indexed by [`ClusterId`]. An analyzed
//! content always has at least one paragraph. Later stages borrow the
//! finished [`Analysis`] and never write to it.

mod ascii;
mod breaks;
mod classify;
mod clusters;
mod context;
mod items;
mod levels;
mod pairs;
mod paragraphs;
mod runs;
mod scripts;
#[cfg(test)]
mod tests;
mod walk;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use crate::config::SmallKana;
use crate::data::{define_id, heap_bytes};
use crate::stages::content::{Content, ContentFlags};

#[cfg(test)]
pub(super) use clusters::ClusterEnd;
pub(crate) use clusters::{ClusterAttrs, ClusterClass, Clusters};
pub(crate) use context::AnalysisContext;
pub(super) use items::ItemClusters;
pub(super) use levels::ControlLevel;

pub(crate) use paragraphs::{Paragraph, ParagraphFlags, Paragraphs};
pub use runs::{BidiLevel, RunOrientation};
pub(crate) use runs::{ScriptRun, ScriptRuns};
use walk::ClusterWriter;

/// What the analysis stage reads: the content, and the one choice of the
/// context's config it makes.
pub(crate) struct AnalysisInput<'a> {
    pub(crate) content: &'a Content,
    /// Whether a small kana may start a line under `line-break: normal`
    /// (`Config::small_kana`).
    pub(crate) small_kana: SmallKana,
}

/// The analysis stage's working memory, which holds nothing between calls
/// and keeps its capacity.
pub(crate) struct AnalysisScratch {
    /// The runs found so far, in text order, before their scripts are all
    /// known: a run of Common characters takes the script that follows it,
    /// and its paragraph's levels are known only when the paragraph ends.
    runs: Vec<scripts::PendingRun>,
    /// A paragraph's bidi input and the resolver's working memory.
    bidi: levels::BidiInput,
    /// A ruby column's base and what follows the column, which a break
    /// after the column is decided by.
    column_text: String,
}

impl AnalysisScratch {
    /// Empty scratch, allocating nothing.
    pub(crate) fn new() -> Self {
        Self {
            runs: Vec::new(),
            bidi: levels::BidiInput::new(),
            column_text: String::new(),
        }
    }
}

heap_bytes! {
    AnalysisScratch { runs, bidi, column_text }
}

impl Default for AnalysisScratch {
    fn default() -> Self {
        Self::new()
    }
}

/// Analyzes the content of `input` into `out` in one pass over the items
/// and the text.
///
/// Uses the context's segmenters in `cx` and works in `scratch`. Clears and
/// fills `out`; nothing else writes it.
pub(crate) fn analyze(
    input: &AnalysisInput<'_>,
    cx: &AnalysisContext,
    scratch: &mut AnalysisScratch,
    out: &mut Analysis,
) {
    let content = input.content;
    out.clear();
    scratch.runs.clear();
    scratch.bidi.begin();
    // Room for every item's first cluster, and for the clusters: at most one
    // a character, which in Latin and CJK is one a character exactly.
    out.clusters.reserve(content.text.as_str().chars().count());
    out.item_clusters.reserve(content.items.len());
    let mut writer = ClusterWriter::new(input, cx, scratch, out);
    for (id, item) in content.items.iter() {
        writer.visit_item(id, item);
    }
    writer.finish();
}

define_id! {
    /// Names a cluster, the finest unit, which every per-character table is
    /// indexed by. A position between clusters is the id of
    /// the cluster after it, and the end of the text is the number of
    /// clusters.
    pub(crate) struct ClusterId(u32);
}

define_id! {
    /// Names a paragraph, in text order.
    pub(crate) struct ParagraphId(u32);
}

define_id! {
    /// Names a script run, in text order.
    pub(crate) struct ScriptRunId(u32);
}

/// The analysis of a content: its clusters, items, paragraphs and runs.
///
/// Each table answers its own questions. The flags of every paragraph are
/// kept together too.
pub(crate) struct Analysis {
    /// The clusters.
    pub(crate) clusters: Clusters,
    /// Where each item's clusters are.
    pub(crate) item_clusters: ItemClusters,
    /// The paragraphs, in text order: one at least, but in an analysis
    /// cleared.
    pub(crate) paragraphs: Paragraphs,
    /// The runs, in text order.
    pub(crate) runs: ScriptRuns,
    /// The levels of the bidi controls that line layout reorders with, in
    /// item order: those below the clusters on both sides.
    ///
    /// Boxed and made the first time an analysis keeps one, since only
    /// isolates beside right-to-left text do. The box keeps the layout's
    /// record a pointer wide where the table is empty.
    #[allow(clippy::box_collection)]
    control_levels: Option<Box<Vec<ControlLevel>>>,
    /// Every paragraph's flags together, for a stage to decline on the
    /// whole content at once.
    pub(crate) flags: ParagraphFlags,
}

impl Analysis {
    /// Returns an empty analysis, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            clusters: Clusters::new(),
            item_clusters: ItemClusters::new(),
            paragraphs: Paragraphs::new(),
            runs: ScriptRuns::new(),
            control_levels: None,
            flags: ParagraphFlags::NONE,
        }
    }

    /// Empties the analysis, keeping every allocation.
    ///
    /// Also what a layout holds when its builder was dropped without
    /// finishing: its content, and no analysis, which has no paragraph at
    /// all where an analyzed content always has one.
    pub(crate) fn clear(&mut self) {
        self.clusters.clear();
        self.item_clusters.clear();
        self.paragraphs.clear();
        self.runs.clear();
        if let Some(levels) = &mut self.control_levels {
            levels.clear();
        }
        self.flags = ParagraphFlags::NONE;
    }

    /// Returns the levels of the bidi controls that line layout reorders
    /// with, in item order.
    pub(super) fn control_levels(&self) -> &[ControlLevel] {
        self.control_levels.as_deref().map_or(&[], Vec::as_slice)
    }

    /// Records `levels` as the bidi controls' levels, making the box the
    /// first time there are any.
    fn set_control_levels(&mut self, levels: &[ControlLevel]) {
        if levels.is_empty() && self.control_levels.is_none() {
            return;
        }
        let kept = self.control_levels.get_or_insert_default();
        kept.clear();
        kept.extend_from_slice(levels);
    }

    /// Returns the clusters the block's first line can reach, if `content`
    /// restyles anything under `::first-line`.
    ///
    /// `content` is the content this analysis is of. The range is the first
    /// paragraph, since no line crosses a forced break. Returns `None` if
    /// nothing is restyled or there is no text.
    ///
    /// Blink shapes the whole text again for `::first-line`
    /// (`ShapeTextForFirstLineIfNeeded`). Bounding it by the first paragraph
    /// loses nothing the first line can hold. The font, shaping and measure
    /// stages each run their first-line variant over this range.
    pub(crate) fn first_line_reach(&self, content: &Content) -> Option<Range<ClusterId>> {
        if !content.flags.contains(ContentFlags::FIRST_LINE_RESTYLE) {
            return None;
        }
        let first = self.paragraphs.first_clusters()?;
        (!first.is_empty()).then_some(first)
    }
}

heap_bytes! {
    Analysis { clusters, item_clusters, paragraphs, runs, control_levels; flags }
}

impl Default for Analysis {
    fn default() -> Self {
        Self::new()
    }
}
