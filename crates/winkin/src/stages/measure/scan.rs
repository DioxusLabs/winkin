//! The measurement scan: one walk over a variant's clusters.
//!
//! In: the stages before measurement and shaping's advance buffer. Out: the
//! prefix, written once an entry into that buffer, and the scan's tables in
//! [`MeasuredText`]. Start at [`Scan::measure`], then `Scan::walk`.
//!
//! - `scan_walk` holds [`ScanWalk`], which crosses the items at each boundary and
//!   enters the segment after them.
//! - `scan_ends` works out what a line starting or ending at a boundary pays.
//! - `scan_ruby` sizes ruby columns by looking ahead.
//! - `scan_intrinsic` adds up the intrinsic widths.
//! - `scan_em_boxes` records each text item's em box.
//! - `scan_initial_letter` measures an initial letter.

use super::Scan;
use alloc::vec::Vec;

use super::autospace::{AutospaceRules, Seams};
use super::spacing::WordSpacingRule;
use super::tabs::TabStops;
use super::{
    Extent, GeneratedTexts, MeasureFlags, MeasureInput, MeasureScratch, MeasuredText, TextMetrics,
};
use crate::data::{Id, Table};
use crate::stages::Segment;
use crate::stages::analysis::{
    ClusterAttrs, ClusterClass, ClusterId, Paragraph, ParagraphFlags, ParagraphId, ScriptRun,
};
use crate::stages::content::{ContentFlags, NodeId, TextFactsId};
use crate::stages::fonts::{LineBaseline, UsedFontId};
use crate::stages::shape::ShapeSession;
use crate::style::FirstLineVariant;
use crate::unit::InlineLayoutUnit;
use crate::work;

use super::EdgeAmounts;
use super::ScanWalk;
use super::boxes::prepare_boxes;
use super::scan_em_boxes::ItemEmScan;
use super::scan_ends::LineEndState;
use super::scan_initial_letter::initial_letter;
use super::scan_intrinsic::{Indent, IntrinsicScan, Tab};
use super::scan_ruby::RubyBoundaries;

impl Scan<'_> {
    /// Measures one variant into `out`, and returns its intrinsic widths.
    ///
    /// It measures the initial letter and the fixed box shifts first, so a
    /// ruby look-ahead can read them. Then the walk turns `advances` into the
    /// prefix and records the item extents and em boxes.
    pub(super) fn measure(
        mut self,
        cx: &mut ShapeSession<'_, '_>,
        scratch: &mut MeasureScratch,
        mut advances: Vec<InlineLayoutUnit>,
        out: &mut MeasuredText,
    ) -> IntrinsicScan {
        let (input, variant) = (self.input, self.variant);
        out.extents.strut = self.strut(input.content.nodes.text_facts(NodeId::BLOCK, variant));
        self.letter = initial_letter(&self, cx.provider(), &advances, out.extents.strut);
        if self.letter.is_some() {
            out.rare_mut().initial_letter = self.letter;
        }
        prepare_boxes(input, variant, self.metrics, out);
        let widths = self.walk(&mut advances, &mut scratch.ruby_starts, out);
        out.prefix.sums = advances;
        widths
    }

    /// Turns `prefix` into the prefix sums, and writes `out`'s paragraph
    /// flags, extents and line-edge costs. Returns the intrinsic widths.
    ///
    /// `prefix` comes in holding each cluster's shaped advance, and grows by
    /// one entry for the end. The text's variant covers the whole text. The
    /// first line's variant covers the first paragraph, in its
    /// `::first-line` styles, with the same tables over the same clusters.
    ///
    /// Each phase at a boundary is a call:
    /// 1. The walk crosses the items there into the next segment
    ///    ([`ScanWalk::cross`]).
    /// 2. A ruby column ending there closes, and one starting there is sized
    ///    ([`RubyBoundaries::cross`]).
    /// 3. The cluster after it is measured ([`ScanWalk::step`]).
    /// 4. A line starting or ending there is charged ([`LineEndState::cost`]).
    /// 5. The paragraphs ending there close ([`end_paragraphs`]), and the
    ///    prefix entry is written here.
    /// 6. What a line ending after the cluster ends with is recorded
    ///    ([`record_line_end`](Self::record_line_end)).
    /// 7. The plain fast path runs on from there where it is open
    ///    ([`plain_fast_path`](Self::plain_fast_path)).
    fn walk(
        &mut self,
        prefix: &mut Vec<InlineLayoutUnit>,
        ruby_starts: &mut Vec<ClusterId>,
        out: &mut MeasuredText,
    ) -> IntrinsicScan {
        let MeasureInput {
            content, analysis, ..
        } = *self.input;
        let clusters = &analysis.clusters;
        let paragraphs = &analysis.paragraphs;
        // The text's scan writes an extent for every item and flags for every
        // paragraph. The first line's writes them up to the first
        // paragraph's end.
        if self.variant == FirstLineVariant::Standard {
            out.extents.reserve(content.items.len());
            out.paragraphs.reserve(paragraphs.len());
        }
        let scan = &*self;
        let (end, text_end) = (scan.end, clusters.end_id());
        // Shaping wrote one advance a cluster and made room for the end's
        // entry. A missing advance is a bug in shaping; its cluster then
        // measures zero.
        prefix.resize(end.get() + 1, InlineLayoutUnit::ZERO);
        // Whether the text's last line ends at its end. Where the text ends
        // in a separator, its last paragraph is empty and holds no line.
        let ends_on_a_line = paragraphs.ends_on_a_line();
        let mut seams = Seams::new(scan.autospace);
        let mut walk = ScanWalk::new(scan);
        let mut em_boxes = ItemEmScan::new(scan, out);
        let mut ends = LineEndState::default();
        let mut widths = IntrinsicScan::new(Indent::new(content, scan.letter.is_some()));
        let mut paragraph = ParagraphId::new(0);
        let mut flags = MeasureFlags::NONE;
        let mut writer = PrefixWriter::default();
        // What a line starting or ending at the last boundary pays, as the
        // intrinsic sizes count it.
        let mut cost = EdgeAmounts::default();
        let mut ruby = RubyBoundaries::new();
        let text_start = ClusterId::new(0);
        let mut next_at = text_start;
        while next_at <= end {
            let at = next_at;
            next_at = ClusterId::new(at.get() + 1);
            let mut room = walk.cross(scan, at, Some(&mut out.extents));
            if let Some(em) = &mut em_boxes
                && let Some(segment) = &walk.segment
                && segment.start == at
                && at < end
            {
                em.enter(scan, segment, out);
            }
            if walk.past_strut > 0 {
                flags.insert(MeasureFlags::BOXES_PAST_STRUT);
            }
            let (held, next) = scan.boundary_paragraphs(&walk, at, paragraph);
            let starts_paragraph = held.is_some_and(|(_, para)| para.start == at);
            if scan.has_ruby {
                let sum = writer.sum;
                ruby.cross(
                    scan,
                    &walk,
                    seams,
                    prefix,
                    ruby_starts,
                    at,
                    sum,
                    &mut room,
                    out,
                );
            }
            if at == text_end && ends_on_a_line {
                // No line starts at the text's end, so nothing there is the
                // next line's: an empty box ending the text, or one left
                // open, is the last line's.
                room.leading_closes += room.rest;
                room.rest = InlineLayoutUnit::ZERO;
            }
            // Measure the cluster after the boundary before the boundary's
            // costs, since a mark hanging at a line's start is that cluster.
            // It adds its advance, the spacing after it, and the room of a
            // seam after it. The shaper puts `text-spacing-trim` in the
            // advance.
            let attrs = clusters.attrs(at);
            let class = attrs.map_or(ClusterClass::Text, ClusterAttrs::class);
            let gap = match held {
                Some((_, para)) if scan.has_seams(para) => {
                    InlineLayoutUnit::from_text(walk.seam_after(scan, &mut seams, at))
                }
                _ => InlineLayoutUnit::ZERO,
            };
            let step = match prefix.get(at.get()) {
                Some(&advance) if at < end => walk.step(scan, class, advance, at) + gap,
                _ => InlineLayoutUnit::ZERO,
            };
            let column = ruby.column;
            room.inside_column = column.is_some_and(|open| open.start < at && at < open.end);
            let (paid, edge) = ends.cost(scan, at, starts_paragraph, &room, &walk, step);
            let has_cost = edge.is_some();
            if let Some(edge) = edge {
                out.rare_mut().edge_costs.push(edge);
                flags.insert(MeasureFlags::HAS_EDGE_COSTS);
            }
            cost = paid;
            // The leading closing edges end the line before a break here,
            // and the paragraph before, where one ends here.
            writer.sum += room.leading_closes;
            // The prefix going back from the last boundary to this one marks
            // the paragraph, which may end here. A dip between the two
            // boundaries that recovers by this one doesn't count: nobody
            // reads it.
            if at > text_start && writer.went_back() {
                flags.insert(MeasureFlags::NONMONOTONE);
            }
            if at == text_start {
                widths.start_text(cost);
            }
            if let Some((whole, first, last, widest)) = ruby.ended.take() {
                widths.ruby(writer.sum, whole, first, last, widest);
            }
            widths.boundary(writer.sum, cost);
            ends.pass_room(room.leading_closes);
            if paragraph < next {
                // The cost is the next paragraph's start's too, and so are
                // the boxes open across it.
                let mut carried = MeasureFlags::NONE;
                if has_cost {
                    carried.insert(MeasureFlags::HAS_EDGE_COSTS);
                }
                if walk.past_strut > 0 {
                    carried.insert(MeasureFlags::BOXES_PAST_STRUT);
                }
                let state = (&mut paragraph, &mut flags, &mut ends);
                end_paragraphs(state, next, carried, writer.sum, cost, &mut widths, out);
            }
            if column.is_some_and(|open| open.may_break) {
                flags.insert(MeasureFlags::HAS_BREAKABLE_RUBY);
            }
            // The floats anchored here sit beside the paragraph the boundary
            // starts. A float after a forced break is the next line's, as
            // Chrome's breaker leaves it.
            widths.floats(room.floats);
            let Some(slot) = prefix.get_mut(at.get()) else {
                break;
            };
            writer.write(slot);
            // The rest start the line after it.
            writer.sum += room.rest;
            ends.pass_room(room.rest);
            // The scan's end has no cluster after it.
            if at == end {
                continue;
            }
            // A tab takes nothing in the prefix. In the intrinsic widths it
            // takes the distance from the pen to its stop.
            let tab = match class {
                ClusterClass::Tab if !walk.annotation() => {
                    let reach = out.ruby_columns().tab_reach(at);
                    let tab = scan
                        .tab_stops(walk.text)
                        .map_or(Tab::NONE, |stops| widths.tab(writer.sum, stops, reach));
                    if scan.has_ruby {
                        ruby.pass_tab(at, tab.paragraph_reach());
                    }
                    tab
                }
                _ => Tab::NONE,
            };
            writer.sum += step;
            if gap != InlineLayoutUnit::ZERO {
                flags.insert(MeasureFlags::HAS_AUTOSPACE);
            }
            // Intrinsic sizing still treats each column as one object.
            let outer_attrs = attrs.map(|attrs| {
                let inside = column.is_some_and(|open| at.get() + 1 < open.end.get());
                if inside {
                    attrs.without(ClusterAttrs::BREAK_AFTER | ClusterAttrs::EMERGENCY_AFTER)
                } else {
                    attrs
                }
            });
            widths.cluster(outer_attrs, step, tab, walk.facts.flags);
            scan.record_line_end(&mut ends, &walk, at, class, attrs, outer_attrs, gap, step);
            // Run the plain fast path from the next boundary where its gate holds:
            // the fast path is open for the content, the item is plain, the
            // paragraph has no autospace seams, and no hyphen is owed.
            if scan.plain_fast_path_open
                && ends.hyphen_owed.is_none()
                && gap == InlineLayoutUnit::ZERO
                && walk.is_plain()
                && held.is_some_and(|(_, para)| !scan.has_seams(para))
            {
                next_at = scan.plain_fast_path(
                    next_at,
                    &mut writer,
                    &mut walk,
                    prefix,
                    &mut flags,
                    &mut widths,
                    out,
                    &mut em_boxes,
                );
            }
        }
        // The last text item's extent, which its fonts widened to the end.
        walk.flush(&mut out.extents);
        if let Some(em) = em_boxes {
            em.finish(scan, out);
        }
        // The last paragraph ends at the text's end, after its closing
        // edges. A line ending there pays what the last boundary charged. A
        // scan that stops at the first paragraph's end has ended it already.
        if end == text_end {
            widths.end_paragraph(prefix.last().copied().unwrap_or_default(), cost);
            if paragraphs.get(paragraph).is_some() {
                out.paragraphs
                    .push_bounded(flags, "no more paragraphs than a ParagraphId names");
            }
        }
        widths
    }
}

impl<'a> Scan<'a> {
    /// Returns the scan of `input` in `variant`'s styles, with no initial
    /// letter yet.
    ///
    /// Word-spacing goes where `words` says. The scan reads the hyphens in
    /// `generated` and the text metrics in `metrics`.
    pub(super) fn new(
        input: &'a MeasureInput<'a>,
        variant: FirstLineVariant,
        words: WordSpacingRule,
        generated: &'a GeneratedTexts,
        metrics: &'a Table<TextFactsId, TextMetrics>,
    ) -> Self {
        let MeasureInput {
            content,
            analysis,
            fonts,
            shaped,
            ..
        } = *input;
        let flags = content.flags;
        let has_edges = flags.contains(ContentFlags::BOXES_WITH_EDGES);
        let has_padding = flags.contains(ContentFlags::LINE_PADDING);
        let has_hanging = flags.contains(ContentFlags::HANGING_PUNCTUATION);
        let has_ruby = flags.contains(ContentFlags::RUBY);
        Self {
            input,
            content,
            analysis,
            fonts,
            variant,
            shaped: shaped.text(variant),
            source: content.text(variant),
            end: input.reach(variant).end,
            writing_mode: content.block.writing_mode,
            baseline: LineBaseline::from_content(content),
            words,
            generated,
            metrics,
            letter: None,
            autospace: AutospaceRules::new(content),
            has_tabs: analysis.flags.contains(ParagraphFlags::HAS_TABS),
            has_edges,
            has_clones: has_edges && flags.contains(ContentFlags::CLONE_BOXES),
            has_spacing: flags.contains(ContentFlags::NONZERO_SPACING),
            has_padding,
            has_hanging,
            has_ruby,
            has_combined: analysis.flags.contains(ParagraphFlags::HAS_COMBINED),
            plain_fast_path_open: work::fast_paths() && !has_ruby && !has_padding && !has_hanging,
        }
    }

    /// Returns the script run of `segment`, through its shaping run's link.
    ///
    /// Each step is one indexed load.
    pub(super) fn script(&self, segment: &Segment) -> Option<&'a ScriptRun> {
        let run = self.shaped.runs.get(segment.run)?;
        self.analysis.runs.get(run.script_run)
    }

    /// Returns the used font `segment` is shaped in.
    pub(super) fn font(&self, segment: &Segment) -> Option<UsedFontId> {
        self.shaped.runs.get(segment.run).map(|run| run.font)
    }

    /// Returns whether the scan looks for autospace seams in `para`.
    pub(super) fn has_seams(&self, para: &Paragraph) -> bool {
        self.autospace.any() && para.flags.contains(ParagraphFlags::HAS_EAST_ASIAN)
    }

    /// Returns the strut of text with the facts `text`.
    pub(super) fn strut(&self, text: TextFactsId) -> Extent {
        self.metrics
            .get(text)
            .map_or(Extent::NONE, |metrics| metrics.strut)
    }

    /// Returns where the tabs of text with the facts `text` stop.
    ///
    /// Returns `None` where the text has no tab.
    pub(super) fn tab_stops(&self, text: TextFactsId) -> Option<TabStops> {
        if !self.has_tabs {
            return None;
        }
        self.metrics.get(text).map(|metrics| metrics.tab)
    }

    /// Returns the paragraph holding the cluster at `at`, and the paragraph
    /// the boundary at `at` belongs to.
    ///
    /// `paragraph` is the previous boundary's. At the scan's end no cluster
    /// follows, and the boundary belongs to the text's last paragraph, or to
    /// the one starting there.
    #[inline]
    fn boundary_paragraphs(
        &self,
        walk: &ScanWalk<'a>,
        at: ClusterId,
        paragraph: ParagraphId,
    ) -> (Option<(ParagraphId, &'a Paragraph)>, ParagraphId) {
        let paragraphs = &self.analysis.paragraphs;
        let last = ParagraphId::new(paragraphs.len().saturating_sub(1));
        let held = (walk.segment.as_ref())
            .filter(|_| at < self.end)
            .and_then(|segment| {
                let id = segment.paragraph;
                paragraphs.get(id).map(|para| (id, para))
            });
        let next = match held {
            Some((id, _)) => id,
            None if at == self.analysis.clusters.end_id() => last,
            None if at == self.end => ParagraphId::new(paragraph.get() + 1).min(last),
            None => paragraph,
        };
        (held, next)
    }

    /// Records in `ends` what a line ending after the cluster at `at` ends
    /// with.
    ///
    /// - The room of the seam after the cluster, `gap`, which the line gives
    ///   back.
    /// - After a soft hyphen a line may break after, the hyphen, its width
    ///   snapped to the grid as Chrome's `HyphenResult::InlineSize` is.
    /// - Where the cluster doesn't hang, its padding and the mark that may
    ///   hang. `step` is its advance with `gap`.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn record_line_end(
        &self,
        ends: &mut LineEndState,
        walk: &ScanWalk<'a>,
        at: ClusterId,
        class: ClusterClass,
        attrs: Option<ClusterAttrs>,
        outer_attrs: Option<ClusterAttrs>,
        gap: InlineLayoutUnit,
        step: InlineLayoutUnit,
    ) {
        ends.seam_given = gap;
        if class == ClusterClass::SoftHyphen
            && !walk.annotation()
            && outer_attrs.is_some_and(|attrs| attrs.has(ClusterAttrs::BREAK_AFTER))
        {
            ends.hyphen_owed =
                (self.generated.hyphen(walk.text)).map(|hyphen| self.generated.snapped(hyphen));
        }
        let hangs = attrs.is_some_and(|attrs| attrs.has(ClusterAttrs::HANGS));
        if (self.has_padding || self.has_hanging) && !hangs {
            // A mark hanging past a line's end hangs its own advance. A line
            // ending after it gives the seam's room back already.
            let clusters = &self.analysis.clusters;
            ends.content(walk, step - gap, || clusters.first_char(self.source, at));
        }
    }

    /// Runs the plain fast path: a tight loop over boundaries where each cluster
    /// pays only its advance.
    ///
    /// The fast path runs from `from` to the end of the current item, its
    /// paragraph, or the scan. It writes each boundary's sum into `prefix`
    /// through `writer`, and moves `walk` into the item's next segment where
    /// a shaping run ends inside the item. It marks `flags` where the prefix
    /// goes back, and keeps `widths` up to date.
    ///
    /// Returns the boundary it stopped at: the fast path's end, or the boundary
    /// before a sized tab, a soft hyphen or an atomic inline's U+FFFC. The
    /// general body handles those.
    ///
    /// The fast path is exact because of its gate. The content has no ruby, line
    /// padding or hanging punctuation. The paragraph has no autospace seam.
    /// The item has no spacing, cloned edge or annotation, and no hyphen is
    /// owed. So every cost the general body would work out here is zero.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn plain_fast_path(
        &self,
        from: ClusterId,
        writer: &mut PrefixWriter,
        walk: &mut ScanWalk<'a>,
        prefix: &mut [InlineLayoutUnit],
        flags: &mut MeasureFlags,
        widths: &mut IntrinsicScan,
        out: &mut MeasuredText,
        em_boxes: &mut Option<ItemEmScan>,
    ) -> ClusterId {
        let clusters = &self.analysis.clusters;
        let mut at = from;
        loop {
            let fast_end = walk.until.min(self.end);
            while at < fast_end {
                let attrs = clusters.attrs(at);
                let class = attrs.map_or(ClusterClass::Text, ClusterAttrs::class);
                let tab = class == ClusterClass::Tab && self.has_tabs;
                if tab || class == ClusterClass::SoftHyphen || class == ClusterClass::Object {
                    return at;
                }
                let Some(slot) = prefix.get_mut(at.get()) else {
                    return at;
                };
                if writer.went_back() {
                    flags.insert(MeasureFlags::NONMONOTONE);
                }
                widths.boundary(writer.sum, EdgeAmounts::default());
                let advance = *slot;
                writer.write(slot);
                writer.sum += advance;
                widths.cluster(attrs, advance, Tab::NONE, walk.facts.flags);
                at = ClusterId::new(at.get() + 1);
            }
            if at == fast_end && at < self.end && walk.continues(self, &mut out.extents) {
                if let Some(em) = em_boxes
                    && let Some(segment) = &walk.segment
                {
                    em.enter(self, segment, out);
                }
                continue;
            }
            return at;
        }
    }
}

/// Ends each paragraph from `paragraph` up to `next`, the one the boundary
/// belongs to.
///
/// Each paragraph ended writes its `flags` and its intrinsic widths at `sum`
/// with the boundary's `cost`. The next starts with fresh line-end state
/// `ends` and the flags `carried` across the boundary.
fn end_paragraphs(
    (paragraph, flags, ends): (&mut ParagraphId, &mut MeasureFlags, &mut LineEndState),
    next: ParagraphId,
    carried: MeasureFlags,
    sum: InlineLayoutUnit,
    cost: EdgeAmounts,
    widths: &mut IntrinsicScan,
    out: &mut MeasuredText,
) {
    while *paragraph < next {
        widths.end_paragraph(sum, cost);
        out.paragraphs
            .push_bounded(*flags, "no more paragraphs than a ParagraphId names");
        *flags = carried;
        *paragraph = ParagraphId::new(paragraph.get() + 1);
        *ends = LineEndState::default();
    }
}

/// The one writer of the prefix sums, in the general body and the plain fast path
/// alike.
///
/// It holds the running sum and the prefix entry at the last boundary. The
/// next boundary is compared with that entry, since the breaker reads the
/// prefix at boundaries only.
#[derive(Copy, Clone, Debug, Default)]
struct PrefixWriter {
    sum: InlineLayoutUnit,
    last: InlineLayoutUnit,
}

impl PrefixWriter {
    /// Returns whether the sum is under the prefix at the last boundary,
    /// which means the prefix goes back.
    fn went_back(self) -> bool {
        self.sum < self.last
    }

    /// Writes the sum into `slot`, the prefix entry at the current
    /// boundary.
    fn write(&mut self, slot: &mut InlineLayoutUnit) {
        *slot = self.sum;
        self.last = self.sum;
    }
}
