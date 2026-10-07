//! Fitting a line to its band, as more of [`Breaker`]'s `impl`.
//!
//! **Chrome's grid.** A boundary's fitting position is its prefix sum from
//! the paragraph's start, rounded up to 1/64. A line from `s` to `e` is
//! `⌈P[e] − P[p]⌉ − ⌈P[s] − P[p]⌉` wide. It fits a band `W` wide when that
//! is at most `W + 1/64`, as in Chrome's `AvailableWidthToFit`. A reshaped
//! line adds the exact difference its pieces make, rounded the same way.
//!
//! **Searching.** The furthest boundary that fits is the last whose prefix
//! sum is under one threshold. A gallop from the line's start and a bisection
//! find it in O(log n) of the line's length. A paragraph with tabs, or whose
//! prefix is not monotone, is walked a cluster at a time instead, as Chrome
//! walks every paragraph.
//!
//! **Tabs.** A tab takes nothing in the prefix. Its width depends on where it
//! lands on the line, so the walk sizes each tab as it meets it. It uses the
//! same tab function as line layout. A candidate adds its tabs to its reach,
//! and takes a hanging tab off with the rest of what hangs. Stops count from
//! the block's content edge.
//!
//! **The opportunity.** From the furthest boundary, the fit steps forward
//! over what hangs, since a space that overflows still hangs. It then steps
//! back through the opportunity bitset, measuring each candidate exactly
//! until one fits. If none fits, it takes an `overflow-wrap` break inside the
//! word where allowed, or the first opportunity, overflowing.

use super::{
    Breaker, BreakerParagraph, EdgeMark, EdgeShapeId, Fitted, Fitting, LineBand, LineFlags,
    LineHang, Lines, ReshapedPieces, StartWindow,
};
use core::ops::Range;

use super::fit::{Candidate, Hold};
use super::fit_reshape::EndPiece;
use crate::data::Id;
use crate::data::IdRange;
use crate::data::SortedCursor;
use crate::stages::analysis::ClusterId;
use crate::stages::content::{ItemFlags, ItemId, TextFlags};
use crate::stages::measure::{LineEdgeCost, LineEdgeFlags, tab_advance_reached};
use crate::stages::shape::ShapingEdges;
use crate::stages::{Segment, Segments, Step};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

impl<'a: 'c, 'c, 'm, 'provider> Breaker<'a, 'c, 'm, 'provider> {
    /// Returns what fitting the line from `start` holds fixed while its
    /// candidates are measured.
    ///
    /// That is its room, indent, start cost, reshape window and tab origin.
    /// `first` marks the block's first line. A reshaped start leaves its
    /// window's piece in the edge tables for the candidates to share.
    #[inline(always)]
    fn fitting<'p>(
        &mut self,
        out: &mut Lines,
        para: &'p BreakerParagraph,
        (start, first_item): (ClusterId, ItemId),
        band: LineBand,
        area: LineBand,
        first: bool,
    ) -> Fitting<'p>
    where
        'a: 'p,
    {
        let mark = out.edges.mark();
        // A wrapped line starting at an opening mark its style trims is
        // reshaped even where the font did not shape across its start.
        let trims = self.trims_starts && start > para.start && self.trims_start(start);
        let (window, head) = if (self.reshapes
            && start > para.start
            && self.stages.shaped.glyphs.word(start).is_unsafe_to_break())
            || trims
        {
            self.start_window(out, para, start, trims).unzip()
        } else {
            (None, None)
        };
        Fitting {
            shift: if para.breakable_ruby {
                self.continued
                    .as_ref()
                    .map_or_else(Default::default, |continued| continued.shift())
            } else {
                Default::default()
            },
            mark,
            head,
            ..self.held(para, (start, first_item), band, area, first, window)
        }
    }

    /// Returns what [`fitting`](Self::fitting) holds fixed, with `window` as
    /// the start's reshape window, reshaping nothing.
    ///
    /// Used to measure a line again as its fit did. The window's piece is not
    /// in the edge tables. `first_item` is the first item at `start`.
    #[inline(always)]
    pub(super) fn held<'p>(
        &self,
        para: &'p BreakerParagraph,
        (start, first_item): (ClusterId, ItemId),
        band: LineBand,
        area: LineBand,
        first: bool,
        window: Option<StartWindow>,
    ) -> Fitting<'p>
    where
        'a: 'p,
    {
        let indent = self.indent(para, start, first, area);
        let mut costs = self.edge_costs(para, start);
        let (start_cost, hang_start) = self.start_cost(&mut costs, start, first);
        // A balanced paragraph's lines fit in an equally narrowed room, and
        // are set in their full bands.
        let band_width = (band.width() - self.hold.narrowing()).max(LayoutUnit::ZERO);
        Fitting {
            shift: Default::default(),
            para,
            start,
            first_item,
            costs,
            mark: EdgeMark::default(),
            window,
            head: None,
            room: band_width + LayoutUnit::EPSILON - indent,
            start_cost,
            hang_start,
            fit_start: self.stages.measured.prefix.fit_position(para.start, start),
            indent,
            tab_origin: if para.tabs {
                band.start_from(area, para.level)
            } else {
                LayoutUnit::ZERO
            },
        }
    }

    /// Chooses the line starting at `start` in `para`, fitted to `band`.
    ///
    /// `start` comes with the first item there. Leaves the line's reshaped
    /// pieces in the edge tables.
    #[inline(always)]
    pub(super) fn fit(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: (ClusterId, ItemId),
        band: LineBand,
        area: LineBand,
        first: bool,
    ) -> Fitted {
        if !para.breakable_ruby {
            return self.fit_plain(out, para, start, band, area, first);
        }
        self.fit_ruby(out, para, start, band, area, first)
    }

    /// Fits a line of a paragraph with breakable ruby, as
    /// [`fit_plain`](Self::fit_plain) does, and keeps its column pieces.
    ///
    /// It first takes back what an earlier fit of this line kept. It sizes
    /// the column the line continues before the fit, and records that
    /// column's rest after it. It is out of line, as is `fit_plain`, so the
    /// dispatch in [`fit`](Self::fit) inlines and stays small.
    #[inline(never)]
    fn fit_ruby(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        (start, first_item): (ClusterId, ItemId),
        band: LineBand,
        area: LineBand,
        first: bool,
    ) -> Fitted {
        let line = out.lines.next_id();
        let shapes = out
            .lines
            .last()
            .map_or(EdgeShapeId::new(0), |line| line.shapes.end);
        out.edges.shapes.truncate(shapes);
        if let Some(ruby) = &mut out.ruby {
            ruby.truncate(line);
        }
        self.continued = self.continued_column(out, para, start);
        let mut fitted = self.fit_plain(out, para, (start, first_item), band, area, first);
        self.finish_ruby(out, para, start, &mut fitted);
        fitted
    }

    /// Fits the line from `start` as [`fit`](Self::fit) says, splitting a
    /// ruby column where one may break.
    #[inline(never)]
    fn fit_plain(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        (start, first_item): (ClusterId, ItemId),
        band: LineBand,
        area: LineBand,
        first: bool,
    ) -> Fitted {
        let mut fitting = self.fitting(out, para, (start, first_item), band, area, first);
        let window = fitting.window;

        // A planned end is kept if the line fits there. Otherwise the line
        // is fitted as usual.
        if let Hold::End(end) = self.hold
            && end > start
            && end <= para.end
        {
            let planned = self.candidate(out, &mut fitting, end, false);
            if let Some(fitted) = settle(&fitting, &planned) {
                return fitted;
            }
        }

        // The rest of the paragraph, where it fits: one subtraction, and a
        // step back over what hangs. A walked paragraph may overflow on the
        // way, so it skips this.
        if !para.walk {
            let whole = self.candidate(out, &mut fitting, para.end, false);
            if let Some(fitted) = settle(&fitting, &whole) {
                return fitted;
            }
        }

        // The furthest boundary that fits: the last whose prefix sum is under
        // the threshold. The threshold is the line's room from its start,
        // less what the start's reshape adds.
        let delta = window.map_or(InlineLayoutUnit::ZERO, |window| window.delta);
        let threshold = fitting.fit_start
            + InlineLayoutUnit::from_layout(fitting.room - fitting.start_cost)
            + para.origin
            - delta;
        let (reach, probes) = if para.walk {
            self.walk(&fitting, para.end, threshold)
        } else {
            self.search(&fitting, start, para.end, threshold)
        };
        #[cfg(test)]
        {
            out.probes += probes;
        }
        #[cfg(not(test))]
        let _ = probes;

        if para.breakable_ruby
            && let Some(fitted) = self.split_ruby(out, &mut fitting, reach)
        {
            return fitted;
        }
        // Forward over what hangs, then back to each opportunity in turn.
        let mut hung = reach;
        while hung < para.end && self.clusters.hangs(hung) {
            hung = ClusterId::new(hung.get() + 1);
        }
        // A closing mark that does not fit whole may fit trimmed, where a
        // line may break after it.
        if self.trims_ends && hung == reach && self.trims_end(para, reach) {
            let end = ClusterId::new(reach.get() + 1);
            let candidate = self.candidate(out, &mut fitting, end, true);
            if let Some(fitted) = settle(&fitting, &candidate) {
                return fitted;
            }
        }
        // A mark that hangs past the end gives its advance back. So the
        // opportunity after the furthest boundary may fit, though no later
        // one can.
        if para.costs || para.seams {
            let end = self.first_opportunity(hung..para.end).unwrap_or(para.end);
            // A seam's room is given back too where the line ends at it, as
            // Chrome's `AdjustOffsetForAutoSpacing` does.
            let flex =
                para.costs && end_cost(para, &mut fitting.costs, end).flex > LayoutUnit::ZERO;
            let seam = para.seams && self.seam_before(para, end) > InlineLayoutUnit::ZERO;
            if end > hung && (flex || seam) {
                let candidate = self.candidate(out, &mut fitting, end, false);
                if let Some(fitted) = settle(&fitting, &candidate) {
                    return fitted;
                }
            }
        }
        let mut next = if hung == para.end && para.walk {
            Some(para.end)
        } else {
            self.last_opportunity(start..hung)
        };
        while let Some(end) = next {
            let candidate = self.candidate(out, &mut fitting, end, false);
            if let Some(fitted) = settle(&fitting, &candidate) {
                return fitted;
            }
            next = self.last_opportunity(start..ClusterId::new(end.get() - 1));
        }

        // Nothing fits. Where the style allows `overflow-wrap`, take the last
        // emergency break that fits before the first opportunity. Where none
        // fits, take the first, which keeps at least one cluster. `nowrap`
        // sets no emergency break.
        let opportunity = self.first_opportunity(start..para.end);
        let first_break = opportunity.unwrap_or(para.end);
        let before = ClusterId::new(first_break.get() - 1);
        let mut next = self.last_emergency(start..reach.min(before));
        let mut earliest = None;
        while let Some(end) = next {
            let candidate = self.candidate(out, &mut fitting, end, false);
            if let Some(mut fitted) = settle(&fitting, &candidate) {
                fitted.flags.insert(LineFlags::EMERGENCY);
                return fitted;
            }
            earliest = Some(end);
            next = self.last_emergency(start..ClusterId::new(end.get() - 1));
        }
        if let Some(end) = earliest.or_else(|| self.first_emergency(start..before)) {
            let candidate = self.candidate(out, &mut fitting, end, false);
            let mut fitted = overflowing(&fitting, &candidate);
            fitted.flags.insert(LineFlags::EMERGENCY);
            return fitted;
        }

        // Otherwise take the first opportunity, overflowing. Where only
        // spaces follow it, they end this line rather than make their own.
        // If its reshaped edges bring it within the room, it fits after all.
        let end = match opportunity {
            Some(end) if self.clusters.only_spaces(end..para.end) => para.end,
            Some(end) => end,
            None => para.end,
        };
        let candidate = self.candidate(out, &mut fitting, end, false);
        if end == para.end {
            return overflowing(&fitting, &candidate);
        }
        settle(&fitting, &candidate).unwrap_or_else(|| overflowing(&fitting, &candidate))
    }

    /// Returns the last boundary from `start` to `end` whose prefix sum is at
    /// most `threshold`, and how many positions it compared.
    ///
    /// Gallops, then bisects, over a prefix that does not decrease. Returns
    /// `start` where nothing past it fits.
    fn search(
        &self,
        fitting: &Fitting<'_>,
        start: ClusterId,
        end: ClusterId,
        threshold: InlineLayoutUnit,
    ) -> (ClusterId, usize) {
        let fits = |at: usize| {
            self.stages.measured.prefix.get(ClusterId::new(at))
                + fitting.shift.at(ClusterId::new(at))
                <= threshold
        };
        let (mut low, end) = (start.get(), end.get());
        let mut probes = 0;
        let mut step = 1;
        // Gallop: `low` fits (or is the start), `high` does not.
        let mut high = loop {
            let probe = low.saturating_add(step).min(end);
            probes += 1;
            if !fits(probe) {
                break probe;
            }
            if probe == end {
                return (ClusterId::new(end), probes);
            }
            low = probe;
            step = step.saturating_mul(2);
        };
        // Bisect between them.
        while high - low > 1 {
            let middle = low + (high - low) / 2;
            probes += 1;
            if fits(middle) {
                low = middle;
            } else {
                high = middle;
            }
        }
        (ClusterId::new(low), probes)
    }

    /// Does what [`search`](Self::search) does, one boundary at a time,
    /// stopping at the first that does not fit.
    ///
    /// It serves a prefix that may go back, where Chrome stops at the first
    /// overflow, and a paragraph with tabs, sizing each tab as it meets it.
    /// It is out of line so the searching fit around it stays small.
    #[inline(never)]
    fn walk(
        &self,
        fitting: &Fitting<'_>,
        end: ClusterId,
        threshold: InlineLayoutUnit,
    ) -> (ClusterId, usize) {
        let prefix = &self.stages.measured.prefix;
        let mut probes = 0;
        let mut at = fitting.start;
        let mut tabs = InlineLayoutUnit::ZERO;
        // Segment by segment, with each tab's item and the first item at the
        // current boundary.
        let mut boundary = None;
        for step in self.line_segments(fitting, fitting.start..end) {
            let Some(segment) = step_segment(&mut boundary, step) else {
                continue;
            };
            for cluster in (segment.start..segment.end).ids() {
                probes += 1;
                if fitting.para.tabs && self.clusters.is_tab(cluster) {
                    tabs += self.tab_width(fitting, cluster, tabs, &segment, boundary);
                }
                let next = ClusterId::new(cluster.get() + 1);
                if prefix.get(next) + fitting.shift.at(next) + tabs > threshold {
                    return (cluster, probes);
                }
                at = next;
            }
        }
        (at, probes)
    }

    /// Returns how far `tab` reaches on the line being fitted, the tabs
    /// before it reaching `before`.
    ///
    /// Its position counts from the line's start, with the indent and what
    /// the start's reshape moved. `boundary` gives the first item at its
    /// boundary. Line layout sizes the tab with the same function. A tab in
    /// ruby annotation text takes no room on its base's line.
    #[inline(never)]
    fn tab_width(
        &self,
        fitting: &Fitting<'_>,
        tab: ClusterId,
        before: InlineLayoutUnit,
        segment: &Segment,
        boundary: Option<(ClusterId, ItemId)>,
    ) -> InlineLayoutUnit {
        let stages = &self.stages;
        let Some(stops) = segment
            .item_row(stages)
            .filter(|item| !item.flags.contains(ItemFlags::ANNOTATION))
            .and_then(|item| self.measured.text_metrics(stages.text_facts(item.node)))
            .map(|metrics| metrics.tab)
        else {
            return InlineLayoutUnit::ZERO;
        };
        let from = boundary
            .filter(|&(at, _)| at == tab)
            .map_or(segment.item, |(_, first)| first);
        let pen = stages.pen(tab, from);
        // The start's reshape moves what follows its window. A tab ends any
        // shaping run, so a tab past the line's start follows the window.
        let moved = fitting
            .window
            .filter(|window| window.end <= tab)
            .map_or(InlineLayoutUnit::ZERO, |window| window.delta);
        let position = InlineLayoutUnit::from_layout(fitting.indent)
            + (pen - self.stages.measured.prefix.get(fitting.start))
            + moved
            + before;
        let reach = self.stages.measured.ruby_columns().tab_reach(tab);
        InlineLayoutUnit::from_layout(tab_advance_reached(
            fitting.tab_origin,
            position,
            stops,
            reach,
        ))
    }

    /// Returns how far a line's tabs reach: all of them, and those from
    /// `content_end` on, which hang.
    ///
    /// Only a paragraph with tabs asks. It is out of line so text without
    /// tabs pays nothing.
    #[inline(never)]
    pub(super) fn line_tabs(
        &self,
        fitting: &Fitting<'_>,
        end: ClusterId,
        content_end: ClusterId,
    ) -> (InlineLayoutUnit, InlineLayoutUnit) {
        let (mut all, mut hanging) = (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
        let mut boundary = None;
        for step in self.line_segments(fitting, fitting.start..end) {
            let Some(segment) = step_segment(&mut boundary, step) else {
                continue;
            };
            for cluster in (segment.start..segment.end).ids() {
                if self.clusters.is_tab(cluster) {
                    let width = self.tab_width(fitting, cluster, all, &segment, boundary);
                    all += width;
                    if cluster >= content_end {
                        hanging += width;
                    }
                }
            }
        }
        (all, hanging)
    }

    /// Returns the walk over `clusters` of the line being fitted, which
    /// starts from the line's first item and seeks only its shaping run.
    ///
    /// The walk starts at the line's start. It passes the segments and
    /// items before `clusters` without yielding them, a step each.
    pub(super) fn line_segments(
        &self,
        fitting: &Fitting<'_>,
        clusters: Range<ClusterId>,
    ) -> impl Iterator<Item = Step> + use<'a> {
        let (para, from) = (fitting.para.id, clusters.start);
        let line = fitting.start..clusters.end;
        Segments::from_item(&self.stages, para, fitting.first_item, line).filter_map(move |step| {
            match step {
                Step::Item { at, .. } if at < from => None,
                Step::Segment(segment) if segment.end <= from => None,
                Step::Segment(mut segment) => {
                    segment.start = segment.start.max(from);
                    Some(Step::Segment(segment))
                }
                step @ Step::Item { .. } => Some(step),
            }
        })
    }

    /// Measures the line from the fitting's start to `end`, reshaping its
    /// edges where the font shaped across them.
    ///
    /// First takes back what the last candidate reshaped. Where `trims_end`,
    /// the line ends at a closing mark it trims, and is reshaped regardless.
    pub(super) fn candidate(
        &mut self,
        out: &mut Lines,
        fitting: &mut Fitting<'_>,
        end: ClusterId,
        trims_end: bool,
    ) -> Candidate {
        // The last candidate's pieces go; the start's window, which every
        // candidate shares, stays.
        out.edges
            .rewind(fitting.head.map_or(fitting.mark, |head| head.after));
        let para = fitting.para;
        let start = fitting.start;
        let mut pieces = ReshapedPieces::default();
        // The room of a seam the line ends at, which it gives back.
        let seam = if para.seams && end > start {
            self.seam_before(para, end)
        } else {
            InlineLayoutUnit::ZERO
        };
        let gives_back = seam > InlineLayoutUnit::ZERO;
        if self.reshapes || fitting.window.is_some() || trims_end || gives_back {
            // Where the paragraph shaped across collapsible spaces at the
            // line's end, the line is reshaped up to them. So its last letter
            // is not kerned against a space the line drops.
            let removed = if trims_end || gives_back {
                None
            } else {
                self.removed_end(fitting, end)
            };
            let shaped_end = removed.unwrap_or(end);
            let end_unsafe = trims_end
                || removed.is_some()
                || (end < para.end
                    && self.stages.shaped.glyphs.word(end).is_unsafe_to_break()
                    && (para.exact_end || !self.clusters.space_or_tab_before(end)));
            // The line's start is shaped as a start, its opening mark
            // trimmed where the window says, and its end trimmed where asked.
            let whole = ShapingEdges {
                line_start: start > para.start,
                trim_start: fitting.window.is_some_and(|window| window.trims),
                trim_end: trims_end,
            };
            match fitting.window {
                // The line ends inside its start's window: it is one piece,
                // in place of the window's.
                Some(window)
                    if shaped_end <= window.end
                        && (shaped_end < window.end || trims_end || gives_back) =>
                {
                    out.edges.rewind(fitting.mark);
                    fitting.head = None;
                    let line = start..shaped_end;
                    pieces.push(self.reshape(out, para, window.run, line, whole, true));
                }
                Some(window) => {
                    pieces.push(self.head(out, fitting, window));
                    if end_unsafe {
                        let from = window.end;
                        match self.end_piece(out, para, from, shaped_end, false, trims_end) {
                            EndPiece::Piece(piece) => pieces.push(Some(piece)),
                            EndPiece::None if gives_back => {
                                pieces.push(self.given_back(out, end, seam));
                            }
                            EndPiece::None => {}
                            EndPiece::Whole(run) => {
                                out.edges.rewind(fitting.mark);
                                fitting.head = None;
                                pieces = ReshapedPieces::default();
                                let line = start..shaped_end;
                                pieces.push(self.reshape(out, para, run, line, whole, true));
                            }
                        }
                    } else if gives_back {
                        pieces.push(self.given_back(out, end, seam));
                    }
                }
                None if end_unsafe => {
                    if let EndPiece::Piece(piece) =
                        self.end_piece(out, para, start, shaped_end, true, trims_end)
                    {
                        pieces.push(Some(piece));
                    } else if gives_back {
                        pieces.push(self.given_back(out, end, seam));
                    }
                }
                None if gives_back => pieces.push(self.given_back(out, end, seam)),
                None => {}
            }
        }
        // Step back over what hangs, using each cluster's reshaped advance
        // where it has one. At the paragraph's end, the white space decides
        // whether it hangs only where it overflows.
        let (content_end, mut tail) =
            self.hanging_tail(start, end, |cluster| pieces.advance(&*out, cluster));
        let conditional = (end == para.end)
            .then(|| self.conditional_start(fitting, content_end..end))
            .flatten()
            .map(|from| {
                let mut near = None;
                (content_end..from)
                    .ids()
                    .fold(InlineLayoutUnit::ZERO, |sum, cluster| {
                        sum + pieces
                            .advance(&*out, cluster)
                            .unwrap_or_else(|| self.stages.cluster_advance_near(cluster, &mut near))
                    })
            });
        let mut reach = self.stages.measured.prefix.get(end) - para.origin
            + pieces.delta()
            + fitting.shift.at(end);
        if para.tabs {
            // The line's tabs, which the prefix gives nothing. Those that
            // hang come off with the rest of what hangs.
            let (tabs, hanging_tabs) = self.line_tabs(fitting, end, content_end);
            reach += tabs;
            tail += hanging_tabs;
        }
        Candidate {
            end,
            content_end,
            content: ((reach - tail).ceil_to_grid() - fitting.fit_start).to_layout(),
            full: (reach.ceil_to_grid() - fitting.fit_start).to_layout(),
            conditional,
            cost: end_cost(para, &mut fitting.costs, end),
            pieces,
        }
    }

    /// Returns where a line ending at `end` ends without what hangs, and how
    /// far what hangs reaches.
    ///
    /// Steps back no further than `floor`. Each cluster is as wide as
    /// `reshaped` says where it has a reshaped advance, and otherwise its own
    /// advance, walking back with the items at its boundaries. A box's edge
    /// among them stays content. The fit's candidates and the scorer both
    /// use it.
    pub(super) fn hanging_tail(
        &self,
        floor: ClusterId,
        end: ClusterId,
        reshaped: impl Fn(ClusterId) -> Option<InlineLayoutUnit>,
    ) -> (ClusterId, InlineLayoutUnit) {
        let mut content_end = end;
        let mut tail = InlineLayoutUnit::ZERO;
        let mut near = None;
        while content_end > floor {
            work::step();
            let cluster = ClusterId::new(content_end.get() - 1);
            if !self.clusters.hangs(cluster) {
                break;
            }
            tail += reshaped(cluster)
                .unwrap_or_else(|| self.stages.cluster_advance_near(cluster, &mut near));
            content_end = cluster;
        }
        (content_end, tail)
    }

    /// Returns where the white space in `tail`, at a paragraph's end, that
    /// hangs only where it overflows starts.
    ///
    /// That white space is the run of trailing texts that keep white space
    /// and wrap (`pre-wrap`). Where white space collapses, other space
    /// separators hang whole, as Blink's `ComputeTrailingSpaceWidth` hangs
    /// them. `None` where the last text with a space, tab or space separator
    /// collapses. Most paragraph ends have no such space and skip the walk,
    /// which starts from the line's first item, `fitting`'s.
    fn conditional_start(
        &self,
        fitting: &Fitting<'_>,
        tail: Range<ClusterId>,
    ) -> Option<ClusterId> {
        let clusters = self.clusters;
        let spaces = |range: Range<ClusterId>| {
            range
                .ids()
                .any(|cluster| clusters.is_breaking_space(cluster))
        };
        if !spaces(tail.clone()) {
            return None;
        }
        let facts = &self.stages.content.facts;
        let mut start = None;
        for step in self.line_segments(fitting, tail) {
            if let Step::Segment(segment) = step
                && spaces(segment.start..segment.end)
            {
                let conditional = segment
                    .text_facts(&self.stages)
                    .is_some_and(|text| facts.text(text).has(TextFlags::HANGS_CONDITIONALLY));
                start = match (conditional, start) {
                    (true, None) => Some(segment.start),
                    (true, held) => held,
                    (false, _) => None,
                };
            }
        }
        start
    }

    /// Returns a cursor into `para`'s line-edge costs at `at`, sought by
    /// halving, or `None` where `para` has no cost.
    ///
    /// A line's fit seeks it once at the line's start, and its candidates
    /// move it from there.
    pub(super) fn edge_costs(
        &self,
        para: &BreakerParagraph,
        at: ClusterId,
    ) -> Option<SortedCursor<'a, LineEdgeCost>> {
        let costs = self.stages.measured.edge_costs();
        para.costs.then(|| costs.cursor(at))
    }

    /// Returns what a line starting at `start` pays there, and how far a
    /// mark hangs past its start, moving `costs` to `start`.
    ///
    /// Only the block's first line, `first`, hangs a mark under
    /// `hanging-punctuation: first`. The hung mark is its first cluster.
    pub(super) fn start_cost(
        &self,
        costs: &mut Option<SortedCursor<'_, LineEdgeCost>>,
        start: ClusterId,
        first: bool,
    ) -> (LayoutUnit, LayoutUnit) {
        let Some(cost) = costs.as_mut().and_then(|costs| costs.get(start)) else {
            return (LayoutUnit::ZERO, LayoutUnit::ZERO);
        };
        if !cost.flags.contains(LineEdgeFlags::FIRST_LINE_ONLY) {
            return (cost.start, LayoutUnit::ZERO);
        }
        let hang = self.stages.cluster_advance(start).to_layout();
        if first {
            (cost.start, hang)
        } else {
            // Another line starting here pays the rest of the cost.
            (cost.start + hang, LayoutUnit::ZERO)
        }
    }

    /// Returns the `text-indent` a line starting at `start` takes, `first`
    /// marking the block's first line.
    ///
    /// The first line takes it, and under `each-line` every line after a
    /// forced break. `hanging` inverts which lines do. A percentage is of the
    /// area's width, not the band's, truncated onto the grid as Chrome's
    /// `MinimumValueForLength` does. Nothing clamps it, so an indent wider
    /// than the band leaves a stub of a line.
    ///
    /// A first line that opens an initial letter takes it twice: once before
    /// the letter's box and once inside it, where the box's own first line
    /// takes it (`CalculateInitialLetterBoxInlineSize`). Line layout gives
    /// the box the second.
    fn indent(
        &self,
        para: &BreakerParagraph,
        start: ClusterId,
        first: bool,
        area: LineBand,
    ) -> LayoutUnit {
        let indent = self.text_indent;
        let after_break = start == para.start && !first;
        if (first || (indent.each_line && after_break)) == indent.hanging {
            return LayoutUnit::ZERO;
        }
        let length = indent.length(area.width());
        if first && self.has_initial_letter && self.stages.measured.initial_letter().is_some() {
            return length + length;
        }
        length
    }
}

/// Returns `(fits, width, flex taken)` for a candidate with its costs.
///
/// Takes the end cost's flex back where that makes it fit.
fn weigh(fitting: &Fitting<'_>, candidate: &Candidate) -> (bool, LayoutUnit, bool) {
    let cost = candidate.cost;
    let width = candidate.content + fitting.start_cost + cost.end;
    let flex = cost.flex;
    // A line of one mark hanging at its start hangs it once.
    let hangs_twice = fitting.hang_start > LayoutUnit::ZERO
        && candidate.content_end.get() == fitting.start.get() + 1;
    let forced = cost.flags.contains(LineEdgeFlags::HANG_FORCED);
    if flex > LayoutUnit::ZERO && forced && !hangs_twice {
        let taken = width - flex;
        return (taken <= fitting.room, taken, true);
    }
    if width <= fitting.room {
        return (true, width, false);
    }
    if flex > LayoutUnit::ZERO && !hangs_twice {
        let taken = width - flex;
        if taken <= fitting.room {
            return (true, taken, true);
        }
    }
    (false, width, false)
}

/// Returns what a line of `para` ending at `end` pays there, or nothing.
///
/// Moves `costs` to `end`, except at the paragraph's end, whose cost `para`
/// holds.
pub(super) fn end_cost(
    para: &BreakerParagraph,
    costs: &mut Option<SortedCursor<'_, LineEdgeCost>>,
    end: ClusterId,
) -> LineEdgeCost {
    if end == para.end {
        return para.end_cost;
    }
    costs
        .as_mut()
        .and_then(|costs| costs.get(end))
        .copied()
        .unwrap_or_default()
}

/// Returns the candidate as the line, where it fits.
#[inline]
pub(super) fn settle(fitting: &Fitting<'_>, candidate: &Candidate) -> Option<Fitted> {
    let (fits, width, flex) = weigh(fitting, candidate);
    fits.then(|| Fitted::new(fitting, candidate, width, flex, false))
}

/// Returns the candidate as the line, whether it fits or not.
pub(super) fn overflowing(fitting: &Fitting<'_>, candidate: &Candidate) -> Fitted {
    let (fits, width, flex) = weigh(fitting, candidate);
    Fitted::new(fitting, candidate, width, flex, !fits)
}

impl Fitted {
    /// Returns the line `candidate` makes, `width` wide with its costs.
    ///
    /// `flex` says the end cost's flex is taken back, and `overflows` that
    /// it overflows its room. Works out what hangs from the costs used.
    fn new(
        fitting: &Fitting<'_>,
        candidate: &Candidate,
        width: LayoutUnit,
        flex: bool,
        overflows: bool,
    ) -> Self {
        let mut flags = LineFlags::NONE;
        if overflows {
            flags.insert(LineFlags::OVERFLOWS);
        }
        if candidate.cost.flags.contains(LineEdgeFlags::HYPHEN) {
            flags.insert(LineFlags::HYPHENATED);
        }
        let space = (candidate.full - candidate.content).max(LayoutUnit::ZERO);
        // Preserved white space ending a paragraph hangs only where it
        // overflows. Other space separators, where white space collapses,
        // hang whole. Only a candidate at its paragraph's end is conditional.
        let unconditional = match candidate.conditional {
            Some(before) if space > LayoutUnit::ZERO => {
                flags.insert(LineFlags::CONDITIONAL_HANG);
                before.ceil_to_grid().to_layout().min(space)
            }
            _ => LayoutUnit::ZERO,
        };
        Fitted {
            end: candidate.end,
            content_end: candidate.content_end,
            indent: fitting.indent,
            width,
            hang: LineHang {
                space,
                unconditional,
                // Hanging punctuation, from the costs used.
                start: fitting.hang_start,
                end: if flex {
                    candidate.cost.flex
                } else {
                    LayoutUnit::ZERO
                },
            },
            flags,
            window: fitting.window,
            pieces: candidate.pieces,
        }
    }
}

/// Returns the segment `step` is, or `None` for an item at a boundary.
///
/// `boundary` records the first item the walk meets at each boundary, which
/// a tab's pen position counts from.
fn step_segment(boundary: &mut Option<(ClusterId, ItemId)>, step: Step) -> Option<Segment> {
    match step {
        Step::Segment(segment) => Some(segment),
        Step::Item { at, id } => {
            if boundary.is_none_or(|(first, _)| first != at) {
                *boundary = Some((at, id));
            }
            None
        }
    }
}
