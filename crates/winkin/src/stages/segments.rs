//! The walk over the clusters in reading order, segment by segment.

use core::ops::Range;

use super::LineStages;
use crate::data::{Id, RunCursor};
use crate::stages::analysis::{Analysis, ClusterId, Paragraph, ParagraphId, ScriptRun};
use crate::stages::content::{Item, ItemId, TextFactsId};
use crate::stages::shape::{ShapedRun, ShapedRunId, ShapedRuns, ShapedText};

/// What a walk over clusters ([`Segments`]) meets next: an item holding no
/// cluster, or a segment of clusters.
///
/// One type keeps both in the one order every reader needs. A box's edge, a
/// float's anchor and a ruby mark sit between clusters, and what a walk does
/// there depends on the clusters around it. Blink's breaker also meets its
/// items one after another. The measure scan's look-aheads copy the walk.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Step {
    /// An item holding no cluster, at boundary `at`: a box's edge, a
    /// float's anchor, a ruby mark. Those at a boundary come before the
    /// clusters after it, as the items do.
    Item { at: ClusterId, id: ItemId },
    /// Clusters over which every run the walk holds is constant.
    Segment(Segment),
}

/// A range of clusters in one paragraph, one item and one shaping run.
///
/// The shaping run lies inside one script run and one font, so those are
/// constant too. A rule asked of a cluster in a walk takes the segment, so
/// it never derives these runs from a cluster id.
///
/// A segment holds ids, not rows, so it is twenty bytes. A reader loads only
/// the rows it wants, each with one indexed load (`item_row`, `run_row`,
/// `paragraph_row`, and through the run `script_row`). The measure scan runs
/// before a `LineStages` is whole and reads the same rows through its own
/// tables.
///
/// No table stores segments. The walk makes each as it passes the runs. It
/// cuts a shaping run at each item, even where items shape alike and the
/// stored run spans them.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Segment {
    /// Its first cluster.
    pub(crate) start: ClusterId,
    /// One past its last.
    pub(crate) end: ClusterId,
    /// The paragraph it is in: its base level and flags.
    pub(crate) paragraph: ParagraphId,
    /// The item it is in: its node, and through it the text facts.
    pub(crate) item: ItemId,
    /// The shaping run it is in: its used font, its shaping facts, and its
    /// script run's script, language, level and orientation.
    pub(crate) run: ShapedRunId,
}

impl Segment {
    /// Its item's row, in `stages`, which it was walked in.
    #[inline]
    pub(crate) fn item_row<'a>(&self, stages: &LineStages<'a>) -> Option<&'a Item> {
        stages.content.items.get(self.item)
    }

    /// Its shaping run's row, in `stages`, which it was walked in.
    #[inline]
    pub(crate) fn run_row<'a>(&self, stages: &LineStages<'a>) -> Option<&'a ShapedRun> {
        stages.shaped.runs.get(self.run)
    }

    /// Its paragraph's row.
    #[inline]
    pub(crate) fn paragraph_row<'a>(&self, stages: &LineStages<'a>) -> Option<&'a Paragraph> {
        stages.analysis.paragraphs.get(self.paragraph)
    }

    /// Its script run's row, by its shaping run's link: its script,
    /// language, level and orientation.
    #[inline]
    pub(crate) fn script_row<'a>(&self, stages: &LineStages<'a>) -> Option<&'a ScriptRun> {
        stages.analysis.runs.get(self.run_row(stages)?.script_run)
    }

    /// Its item's node's text facts, in `stages`' variant.
    #[inline]
    pub(crate) fn text_facts(&self, stages: &LineStages<'_>) -> Option<TextFactsId> {
        self.item_row(stages)
            .map(|item| stages.text_facts(item.node))
    }
}

/// A cluster's place in the flow: the cluster with the item and shaping run
/// that hold it.
///
/// The item is the one a walk from the cluster's start meets first. An item
/// holding no cluster that sits at that boundary, a box's edge or a float's
/// anchor, comes before the item holding the cluster. The run is the shaping
/// run of the cluster, or one past the last where the cluster is past the
/// clusters shaped.
///
/// A function handed a position takes a slot, not a bare cluster, so it
/// finds no item or run again. A slot is valid only in the prepared layout
/// and the first-line variant it was made in: neither its item nor its run
/// names the same thing in another.
///
/// [`Slot::new`] decides the item and run at a boundary, by search. A walk
/// that holds them steps to the next with no search ([`Slot::next`]). It is
/// twelve bytes and `Copy`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Slot {
    cluster: ClusterId,
    item: ItemId,
    run: ShapedRunId,
}

impl Slot {
    /// The place of `cluster` in `stages`' variant, found by search.
    ///
    /// The item is the first at the cluster's start, items holding no
    /// cluster first ([`ItemClusters::cursor_containing`]). The run is
    /// found by its rank. This is the one place that decides which item and
    /// run a boundary belongs to.
    ///
    /// [`ItemClusters::cursor_containing`]:
    /// crate::stages::analysis::ItemClusters::cursor_containing
    pub(crate) fn new(stages: &LineStages<'_>, cluster: ClusterId) -> Self {
        let runs = &stages.shaped.runs;
        Self {
            cluster,
            item: stages
                .analysis
                .item_clusters
                .cursor_containing(cluster)
                .id(),
            run: runs
                .containing(cluster)
                .unwrap_or_else(|| ShapedRunId::new(runs.len())),
        }
    }

    /// The place of the cluster after this one, with no search.
    ///
    /// It steps the item past those ending at the next boundary, and the run
    /// where it ends there.
    #[inline]
    pub(crate) fn next(self, stages: &LineStages<'_>) -> Self {
        let cluster = ClusterId::new(self.cluster.get() + 1);
        let runs = &stages.shaped.runs;
        let run = if runs.run_clusters(self.run).end > cluster {
            self.run
        } else {
            ShapedRunId::new((self.run.get() + 1).min(runs.len()))
        };
        Self {
            cluster,
            item: stages.analysis.item_clusters.walk_to(self.item, cluster),
            run,
        }
    }

    /// The place of `cluster`, at or after this one's, stepping a cluster at
    /// a time with [`next`](Self::next).
    #[inline]
    pub(crate) fn advance(self, stages: &LineStages<'_>, cluster: ClusterId) -> Self {
        let mut slot = self;
        while slot.cluster < cluster {
            slot = slot.next(stages);
        }
        slot
    }

    /// The first item at the cluster's start: one holding no cluster there,
    /// or else the one holding the cluster.
    #[inline]
    pub(crate) fn item(self) -> ItemId {
        self.item
    }

    /// The shaping run of the cluster, in the variant the slot was made in.
    #[inline]
    pub(crate) fn run(self) -> ShapedRunId {
        self.run
    }
}

/// The walk over clusters in reading order, segment by segment, holding each
/// run it is inside.
///
/// The item and the shaping run each have a cursor, and a segment ends at
/// the nearer of their ends. The paragraph cursor moves only where a shaping
/// run ends, since script runs, and so shaping runs, break at every
/// paragraph's start. The script run and the font are the shaping run's
/// links, read only by what wants them. A step reads no row. Moving a cursor
/// reads where the next run ends, once per run. Between the clusters, the
/// walk yields the items that hold none.
///
/// The measure scan, the breaker's line fitting, ruby placement in line
/// layout and visual selection walk with it. The stages that make the runs
/// keep their own walks, and line layout keeps its own run cursor.
///
/// Over the clusters `start..end`, it yields the items at every boundary
/// from `start` up to `end`. It yields those at `end` only at the text's
/// end, where no later walk would. A walk that wants the items after its
/// last cluster walks on and stops itself.
///
/// It reads the text's clusters, items and paragraphs and its variant's
/// shaping. In the first line's variant it ends where that shaping does, at
/// the first paragraph's end. It is `Copy` and six words, so a walk that
/// looks ahead copies it.
#[derive(Copy, Clone)]
pub(crate) struct Segments<'a> {
    analysis: &'a Analysis,
    shaped: &'a ShapedRuns,
    /// The next item to yield, or the one the next segment is in.
    item: RunCursor<ItemId, ClusterId>,
    run: RunCursor<ShapedRunId, ClusterId>,
    /// Moved where `run` ends, which every paragraph's end is.
    paragraph: RunCursor<ParagraphId, ClusterId>,
    /// Where the walk has got to.
    at: ClusterId,
    /// Where it stops.
    end: ClusterId,
}

impl<'a> Segments<'a> {
    /// Starts a walk over `clusters` in `stages`' variant, seeking its start.
    ///
    /// It finds the paragraph and the item by halving and the shaping run by
    /// its rank. A reader starting at a caller's offset or a cluster uses it.
    pub(crate) fn new(stages: &LineStages<'a>, clusters: Range<ClusterId>) -> Self {
        let analysis = stages.analysis;
        let paragraphs = &analysis.paragraphs;
        let at = clusters.start;
        Self::from_cursors(
            stages,
            paragraphs
                .cursor_containing(at)
                .unwrap_or_else(|| paragraphs.cursor(ParagraphId::new(0))),
            analysis.item_clusters.cursor_containing(at),
            clusters,
        )
    }

    /// Starts a walk over `clusters` in `paragraph` from `item`.
    ///
    /// `item` is the first item at the clusters' start, or else the one
    /// holding their first cluster. Only the shaping run is sought, by its
    /// rank. A short walk whose caller holds the first item uses it, such as
    /// a ruby annotation's level from its opening item, where two halvings
    /// would cost more than the walk.
    pub(crate) fn from_item(
        stages: &LineStages<'a>,
        paragraph: ParagraphId,
        item: ItemId,
        clusters: Range<ClusterId>,
    ) -> Self {
        let analysis = stages.analysis;
        Self::from_cursors(
            stages,
            analysis.paragraphs.cursor(paragraph),
            analysis.item_clusters.cursor(item),
            clusters,
        )
    }

    /// Starts a walk over `clusters` with the paragraph and item cursors
    /// already at its start.
    fn from_cursors(
        stages: &LineStages<'a>,
        paragraph: RunCursor<ParagraphId, ClusterId>,
        item: RunCursor<ItemId, ClusterId>,
        clusters: Range<ClusterId>,
    ) -> Self {
        let shaped = &stages.shaped.runs;
        // Past the shaped clusters, take the first run. It ends before the
        // walk's start, so the walk ends where its items do.
        let run = shaped
            .cursor_containing(clusters.start)
            .unwrap_or_else(|| shaped.cursor(ShapedRunId::new(0)));
        Self {
            analysis: stages.analysis,
            shaped,
            item,
            run,
            paragraph,
            at: clusters.start,
            end: clusters.end,
        }
    }

    /// Starts a walk over the whole text, every cursor at its first run.
    ///
    /// The measure scan uses it, since it runs before a `LineStages` is whole.
    pub(super) fn from_text(analysis: &'a Analysis, shaped: &'a ShapedText) -> Self {
        let shaped = &shaped.runs;
        Self {
            analysis,
            shaped,
            item: analysis.item_clusters.cursor(ItemId::new(0)),
            run: shaped.cursor(ShapedRunId::new(0)),
            paragraph: analysis.paragraphs.cursor(ParagraphId::new(0)),
            at: ClusterId::new(0),
            end: analysis.clusters.end_id(),
        }
    }
}

impl Iterator for Segments<'_> {
    type Item = Step;

    /// Returns the next item holding no cluster, or the next segment.
    ///
    /// A step costs two or three comparisons, and moves a cursor for each run
    /// that ends there. Returns `None` at the walk's end, or where the text or
    /// its variant's shaping ends first.
    #[inline]
    fn next(&mut self) -> Option<Step> {
        let (item, item_end) = (self.item.id(), self.item.end());
        let at = self.at;
        // An item ending where the walk stands holds no cluster: it sits
        // here, since the walk passes each item that holds some at its end.
        if item_end <= at {
            let analysis = self.analysis;
            if at >= self.end && at < analysis.clusters.end_id() {
                return None;
            }
            // Past the last item, at the text's end, none is left.
            analysis.item_clusters.get(item)?;
            analysis.item_clusters.step(&mut self.item);
            return Some(Step::Item { at, id: item });
        }
        let run_end = self.run.end();
        let end = self.end.min(item_end).min(run_end);
        if end <= at {
            return None;
        }
        let segment = Segment {
            start: at,
            end,
            paragraph: self.paragraph.id(),
            item,
            run: self.run.id(),
        };
        self.at = end;
        if end == item_end {
            self.analysis.item_clusters.step(&mut self.item);
        }
        if end == run_end {
            self.shaped.step(&mut self.run);
            self.analysis.paragraphs.step_to(&mut self.paragraph, end);
        }
        Some(Step::Segment(segment))
    }
}
