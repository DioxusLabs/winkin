//! The float driver, as more of [`Breaker`]'s `impl`.
//!
//! The fit never calls the host: it is a pure function of a band.
//! [`Breaker::line`] wraps it and places the floats the chosen line reaches,
//! as Blink's breaker and `InlineLayoutAlgorithm` do between them.
//! - Floats anchored at a paragraph's start are placed at the line's top
//!   before it is fitted, fit or not (`PositionLeadingFloats`).
//! - A float anchored on the line is placed at its top where it fits beside
//!   the text before its anchor and no earlier float waits. The line is then
//!   fitted again in what is left.
//! - Otherwise that float and every later one on the line go below the line
//!   once it is kept (`ShouldPushFloatAfterLine`, `PlaceFloatingObjects`).
//! - A line whose own box reaches a narrower band is fitted again there.
//! - In a wrapping block, a line the floats narrow until it overflows moves
//!   down to where the bands next change.
//!
//! A rejected trial rewinds what it placed through the host's checkpoint,
//! as Chrome's `RewindFloats` does. The driver is generic over whether the
//! content has floats. Without them it still refits and moves down past the
//! host's floats, but asks for no checkpoint and places nothing. An initial
//! letter alone is such content.

use super::annotate::Carry;
use super::fit::{Anchor, SETTLES, Trial};
use super::{
    Breaker, BreakerParagraph, Cursor, EdgeMark, Exclusions, ExclusionsCheckpoint, Fitted,
    FloatRequest, LineBand, LineFlags, LineFloat, LineFloatId, Lines, PendingLine, PlacedFloat,
    StartWindow,
};
use crate::build::FloatSide;
use crate::data::Id;
use crate::stages::analysis::{ClusterClass, ClusterId};
use crate::stages::content::{BreakClearanceId, FloatId, ItemId};
use crate::stages::measure::InitialLetter;
use crate::unit::{InlineLayoutUnit, LayoutUnit};

/// The most times one line moves down past the floats beside it, before it
/// stays and overflows.
///
/// A host could report each band change a hair lower, forever. Each move
/// is one fit, so this bounds a line's work. A real line moves once per
/// float bottom it has no room beside, a handful on a page.
pub(super) const MAX_MOVES_DOWN: usize = 256;

/// Returns where a line at `top` moves when the floats leave it no room.
///
/// That is where the host says the bands next change, rounded up onto the
/// grid. Returns `None` where the host's answer is missing, not finite, or
/// not below `top`.
fn moves_to(exclusions: &dyn Exclusions, top: LayoutUnit) -> Option<LayoutUnit> {
    let px = exclusions.below(top.to_px())?;
    if !px.is_finite() {
        return None;
    }
    let next = LayoutUnit::from_px_ceil(px);
    (next > top).then_some(next)
}

/// Returns whether the line fitted as `fitted` from `start` holds the float
/// anchored at `at`.
///
/// It does if the anchor is inside the line, or at its end, where Blink
/// treats the float as a trailing item. An anchor after a forced break
/// belongs to the next line.
fn holds(para: &BreakerParagraph, start: ClusterId, fitted: &Fitted, at: ClusterId) -> bool {
    at >= start && (at < fitted.end || (at == fitted.end && !(at == para.end && para.forced)))
}

/// Where a trial started, which a rejected trial rewinds to: the host's
/// checkpoint, the floats kept, and the first float not yet placed.
#[derive(Copy, Clone, Debug)]
struct Placing {
    checkpoint: ExclusionsCheckpoint,
    kept: LineFloatId,
    next: FloatId,
}

impl Placing {
    /// Returns where placing stands now, `next` being the first float not
    /// yet placed.
    fn new(exclusions: &dyn Exclusions, out: &Lines, next: FloatId) -> Self {
        Self {
            checkpoint: exclusions.checkpoint(),
            kept: out.floats.next_id(),
            next,
        }
    }

    /// Returns whether a float has been kept since.
    fn placed_since(&self, out: &Lines) -> bool {
        out.floats.next_id() != self.kept
    }

    /// Takes back every float placed since, as Chrome's `RewindFloats`
    /// does for a line it tries again.
    fn rewind(self, exclusions: &mut dyn Exclusions, out: &mut Lines, next: &mut FloatId) {
        exclusions.rewind(self.checkpoint);
        out.floats.truncate(self.kept);
        *next = self.next;
    }
}

/// A line being tried beside floats: its paragraph and start, its top, the
/// height its band is asked at, the area, and whether it is the first line.
#[derive(Copy, Clone)]
struct LineTrial<'p> {
    para: &'p BreakerParagraph,
    start: ClusterId,
    /// The first item at `start`, where the breaker's item cursor stands.
    first_item: ItemId,
    top: LayoutUnit,
    reserve: LayoutUnit,
    area: LineBand,
    first: bool,
}

impl<'a: 'c, 'c, 'm, 'provider> Breaker<'a, 'c, 'm, 'provider> {
    /// Moves `cursor` below the floats that the forced break ending the line
    /// kept before `end` clears, where it clears any.
    ///
    /// The next line, or the block's end, starts where the host's band in
    /// `area` no longer narrows on a cleared side, as Chrome applies a
    /// `<br>`'s clearance after its line. The cursor passes each clearing
    /// break once, in order, and a trial rewinds it with the rest.
    ///
    /// The clearance runs from the line box's bottom. What the line's
    /// annotations reach past it is cut by the clearance: Chrome moves what
    /// follows by the greater of the two, and lends back only the reach past
    /// the clearance (`AddAnyClearanceAfterLine`, `BlockLayoutAlgorithm`).
    pub(super) fn clear_floats(
        &mut self,
        out: &Lines,
        end: ClusterId,
        area: LineBand,
        cursor: &mut Cursor,
    ) {
        let clearances = self.stages.content.clearances();
        if clearances.is_empty() {
            return;
        }
        let Some(last) = end.get().checked_sub(1).map(ClusterId::new) else {
            return;
        };
        let items = &self.stages.analysis.item_clusters;
        let mut found = None;
        while let Some(clearance) = clearances.get(cursor.clearances) {
            let at = items.start(clearance.item);
            if at > last {
                break;
            }
            cursor.clearances = BreakClearanceId::new(cursor.clearances.get() + 1);
            if at == last {
                found = Some(clearance.clear);
            }
        }
        let Some(clear) = found else {
            return;
        };
        // The carry is negative by what the annotations reach past the line
        // box, which the cursor already stands below.
        let reach = (-self.carry).max(LayoutUnit::ZERO);
        let line_end = cursor.top - reach;
        let mut top = line_end - cursor.trimmed;
        for _ in 0..MAX_MOVES_DOWN {
            let band = self.band(out, top, LayoutUnit::EPSILON);
            let narrowed = (clear.clears(FloatSide::Left) && band.left > area.left)
                || (clear.clears(FloatSide::Right) && band.right < area.right);
            if !narrowed {
                break;
            }
            let Some(next) = moves_to(&*self.exclusions, top) else {
                break;
            };
            top = next;
        }
        let clearance = top + cursor.trimmed - line_end;
        if clearance <= LayoutUnit::ZERO {
            return;
        }
        cursor.top = cursor.top.max(top + cursor.trimmed);
        cursor.cleared = Some(top);
        if reach > LayoutUnit::ZERO {
            self.carry = -(reach - clearance).max(LayoutUnit::ZERO);
        }
    }

    /// Returns the line starting at `start`, and the room it leaves the
    /// next at its end.
    ///
    /// The line sits in `area` narrowed by the host's bands, at `cursor` or
    /// below where the floats leave no room. Moves the cursor's items and
    /// floats past the line's. Without `FLOATS`, it places no float and asks
    /// for no checkpoint.
    pub(super) fn line<const FLOATS: bool>(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
        area: LineBand,
        cursor: &mut Cursor,
    ) -> (PendingLine, Carry) {
        let placed = out.floats.next_id();
        let top = cursor.top - cursor.trimmed;
        if FLOATS {
            self.place_leading(out, start, top, &mut cursor.floats);
        }
        let mark = out.edges.mark();
        // Fit where it starts, or below the floats where they narrow it
        // until it overflows, taking back each try's floats. These are
        // Blink's layout opportunities, one after another.
        let mut top = top;
        let mut moves = 0;
        let trial = loop {
            let from = FLOATS.then(|| Placing::new(self.exclusions, out, cursor.floats));
            let line = LineTrial {
                para,
                start,
                first_item: cursor.items,
                top,
                reserve: self.reserve(start),
                area,
                first: out.lines.is_empty(),
            };
            let trial = self.trial::<FLOATS>(out, mark, line, cursor);
            let movable = trial.fitted.flags.contains(LineFlags::OVERFLOWS)
                && self.wraps
                && moves < MAX_MOVES_DOWN
                && trial.band.width() < area.width()
                && self.hold.narrowing() == LayoutUnit::ZERO;
            if !movable {
                break trial;
            }
            // Ask where the bands change below the line as they stood before
            // it. Its own floats would only move it past themselves. Blink's
            // opportunities also use the exclusion space before the line.
            let placed = from.is_some_and(|from| from.placed_since(out));
            if placed && let Some(from) = from {
                from.rewind(self.exclusions, out, &mut cursor.floats);
            }
            match moves_to(self.exclusions, top) {
                Some(next) => {
                    top = next;
                    moves += 1;
                }
                // Nowhere lower: the line stays and overflows. Where it placed
                // floats, it is fitted again to place them again.
                None if placed => moves = MAX_MOVES_DOWN,
                None => break trial,
            }
            // Fit again, taking back what this try reshaped.
            out.edges.rewind(mark);
        };
        // The line's first item, where the cursor stood, and the next's.
        let first_item = cursor.items;
        cursor.items = trial.height.next_item;
        // Floats the line reached but did not place beside it go below it,
        // now that its height is known (Blink's `PlaceFloatingObjects`).
        if FLOATS {
            let bottom = top + trial.height.extent.box_height();
            self.place_after(out, para, start, &trial.fitted, bottom, &mut cursor.floats);
        }
        (
            self.kept(out, para, (start, first_item), top, &trial, placed),
            trial.height.carry,
        )
    }

    /// Fits `line` in the band asked at its reserve height, and returns the
    /// trial.
    ///
    /// Where `FLOATS`, places the line's floats beside it or leaves them to
    /// go below. Where the line's own height meets a narrower band, it is
    /// fitted again there, at most [`SETTLES`] times. Each refit takes back
    /// its floats and the edge tables to `mark`.
    fn trial<const FLOATS: bool>(
        &mut self,
        out: &mut Lines,
        mark: EdgeMark,
        mut line: LineTrial<'_>,
        cursor: &mut Cursor,
    ) -> Trial {
        let LineTrial {
            para,
            start,
            first_item,
            top,
            area,
            first,
            ..
        } = line;
        let mut settles = 0;
        loop {
            let from = FLOATS.then(|| Placing::new(self.exclusions, out, cursor.floats));
            let mut band = area.meet(self.band(out, top, line.reserve));
            let mut fitted = self.fit(out, para, (start, first_item), band, area, first);
            if FLOATS {
                self.place_on_line(out, mark, &line, &mut cursor.floats, &mut band, &mut fitted);
            }
            let height = self.extent(out, para, cursor.items, start, fitted.end);
            let tall = height.extent.box_height();
            if settles < SETTLES
                && tall > line.reserve
                && area.meet(self.band(out, top, tall)).width() < band.width()
            {
                if let Some(from) = from {
                    from.rewind(self.exclusions, out, &mut cursor.floats);
                }
                out.edges.rewind(mark);
                line.reserve = tall;
                settles += 1;
                continue;
            }
            return Trial {
                band,
                fitted,
                height,
            };
        }
    }

    /// Returns the content's float `id` as the breaker places it, or `None`
    /// past the last.
    fn anchor(&self, id: FloatId) -> Option<Anchor> {
        let content = self.stages.content;
        let float = content.floats().get(id)?;
        let at = self.stages.analysis.item_clusters.get(float.item)?;
        let (nodes, node) = (&content.nodes, content.item_node(float.item));
        let (inline, block) = float.margin_box;
        Some(Anchor {
            id,
            at,
            key: nodes.key(node),
            side: float.side,
            inline,
            block,
        })
    }

    /// Places the float `anchor` from `block_start`, and records where the
    /// host puts it.
    fn place(&mut self, out: &mut Lines, anchor: &Anchor, block_start: LayoutUnit) {
        let placed = self.exclusions.place(anchor.request(block_start));
        self.record(out, anchor.id, placed);
    }

    /// Records where the host put `float`, in placement order.
    fn record(&mut self, out: &mut Lines, float: FloatId, placed: PlacedFloat) {
        let record = LineFloat {
            float,
            left: LayoutUnit::from_px(placed.inline.left),
            top: LayoutUnit::from_px(placed.block.start),
        };
        if out.floats.push(record).is_none() {
            debug_assert!(
                false,
                "each float is kept once, and a LineFloatId names a float"
            );
        }
    }

    /// Places the floats anchored at or before a line's `start` at its
    /// `top`, fit or not, as Blink's `PositionLeadingFloats` does.
    ///
    /// Only a line opening a paragraph has any, since a float at a soft
    /// break belongs to the line before. They are placed once, before any
    /// trial, so moving the line down leaves them in place, as in Blink.
    fn place_leading(
        &mut self,
        out: &mut Lines,
        start: ClusterId,
        top: LayoutUnit,
        floats: &mut FloatId,
    ) {
        while let Some(anchor) = self.anchor(*floats)
            && anchor.at <= start
        {
            self.place(out, &anchor, top);
            *floats = FloatId::new(floats.get() + 1);
        }
    }

    /// Places the floats anchored on the fitted line beside it where each
    /// fits, and fits the line again in the band each leaves.
    ///
    /// This follows Blink's `HandleFloat` and `ShouldPushFloatAfterLine`. A
    /// float goes at the line's top where all of these hold:
    /// - its margin box fits beside the text before its anchor, perhaps less
    ///   a trailing collapsible space;
    /// - the host puts it no lower, as clearance or an earlier float could;
    /// - no earlier float of the line went below it.
    ///
    /// The first float that fails stops the placing. It and every later float
    /// go below the line ([`place_after`](Self::place_after)). A float the
    /// refitted line no longer reaches is taken back for the next line, as
    /// Blink does.
    fn place_on_line(
        &mut self,
        out: &mut Lines,
        mark: EdgeMark,
        line: &LineTrial<'_>,
        floats: &mut FloatId,
        band: &mut LineBand,
        fitted: &mut Fitted,
    ) {
        while let Some(anchor) = self.anchor(*floats)
            && holds(line.para, line.start, fitted, anchor.at)
        {
            let (reach, space) = self.anchor_reach(line, *band, fitted.window, anchor.at);
            let size = anchor.inline;
            let room = band.width() + LayoutUnit::EPSILON;
            if reach + size > room && reach - space + size > room {
                return;
            }
            let before = Placing::new(self.exclusions, out, *floats);
            let placed = self.exclusions.place(anchor.request(line.top));
            if LayoutUnit::from_px(placed.block.start) > line.top {
                before.rewind(self.exclusions, out, floats);
                return;
            }
            self.record(out, anchor.id, placed);
            *band = line.area.meet(self.band(out, line.top, line.reserve));
            out.edges.rewind(mark);
            let start = (line.start, line.first_item);
            *fitted = self.fit(out, line.para, start, *band, line.area, line.first);
            if !holds(line.para, line.start, fitted, anchor.at) {
                // The line keeps its break, which fits the narrower band and
                // so fits the wider one left once the float is taken back.
                before.rewind(self.exclusions, out, floats);
                *band = line.area.meet(self.band(out, line.top, line.reserve));
                return;
            }
            *floats = FloatId::new(floats.get() + 1);
        }
    }

    /// Places the kept line's remaining floats below it, from its line box's
    /// `bottom`, as Blink's `PlaceFloatingObjects` does.
    fn place_after(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
        fitted: &Fitted,
        bottom: LayoutUnit,
        floats: &mut FloatId,
    ) {
        while let Some(anchor) = self.anchor(*floats)
            && holds(para, start, fitted, anchor.at)
        {
            self.place(out, &anchor, bottom);
            *floats = FloatId::new(floats.get() + 1);
        }
    }

    /// Places the floats no line reaches at `top`, where the next line would
    /// start.
    ///
    /// These are anchored after a final forced break, or in text with no
    /// line. Blink places them as a line with nothing else on it. They come
    /// after every line's floats.
    pub(super) fn place_unreached(
        &mut self,
        out: &mut Lines,
        top: LayoutUnit,
        floats: &mut FloatId,
    ) {
        while let Some(anchor) = self.anchor(*floats) {
            self.place(out, &anchor, top);
            *floats = FloatId::new(floats.get() + 1);
        }
    }

    /// Returns how far the text before the anchor at `at` reaches on the
    /// line, and the width of a collapsible space just before it.
    ///
    /// The reach includes the indent and start cost, as fitting measures it:
    /// Blink's `position_` when its breaker meets the float. Blink takes the
    /// space off where the float does not fit with it
    /// (`TrailingCollapsibleSpaceWidth`). The start is measured with
    /// `window` and nothing reshaped.
    fn anchor_reach(
        &self,
        line: &LineTrial<'_>,
        band: LineBand,
        window: Option<StartWindow>,
        at: ClusterId,
    ) -> (LayoutUnit, LayoutUnit) {
        let (para, start) = (line.para, line.start);
        let fitting = self.held(
            para,
            (start, line.first_item),
            band,
            line.area,
            line.first,
            window,
        );
        // The start's reshape moves what follows its window. A float ends a
        // shaping run, so its anchor never falls inside the window.
        let moved = fitting
            .window
            .filter(|window| window.end <= at)
            .map_or(InlineLayoutUnit::ZERO, |window| window.delta);
        let mut reach = self.stages.measured.prefix.get(at) - para.origin + moved;
        if para.tabs {
            reach += self.line_tabs(&fitting, at, at).0;
        }
        let reach = (reach.ceil_to_grid() - fitting.fit_start).to_layout()
            + fitting.start_cost
            + fitting.indent;
        let space = at
            .get()
            .checked_sub(1)
            .map(ClusterId::new)
            .filter(|&last| {
                last >= start
                    && self.clusters.class(last) == Some(ClusterClass::Space)
                    && self.spaces_collapse(&fitting, last..at)
            })
            .map_or(LayoutUnit::ZERO, |last| {
                self.stages.cluster_advance(last).to_layout()
            });
        (reach, space)
    }

    /// Reports the initial letter's room to the host once the first line is
    /// kept, the host's positions standing `trimmed` above the records.
    ///
    /// The margin box follows Chrome's
    /// `CreateExclusionSpaceForInitialLetterBox`. Across the lines, it runs
    /// from where measurement put it to its box's end, ink included. Along
    /// them, it starts on the paragraph's start side and grows by a positive
    /// first-line `text-indent`.
    ///
    /// It is not a content float, so no line records it. The host keeps it
    /// with its own floats, and later lines and blocks make room for it.
    pub(super) fn sink_initial_letter(
        &mut self,
        out: &mut Lines,
        letter: &InitialLetter,
        trimmed: LayoutUnit,
    ) {
        let Some(line) = out.lines.as_slice().first() else {
            return;
        };
        let Some(facts) = self.scratch.placements.as_slice().first() else {
            return;
        };
        // The first line's annotations move the letter down, and its
        // exclusion's end with it. Its start stays at the line's top unless
        // the letter's margin box starts above it.
        let shift = out.letter_shift;
        let top = (letter.exclusion_top + shift).min(LayoutUnit::ZERO);
        let height = letter.exclusion_top + letter.exclusion_height + shift - top;
        let width = letter.exclusion_width + line.indent.max(LayoutUnit::ZERO);
        if height <= LayoutUnit::ZERO || width <= LayoutUnit::ZERO {
            return;
        }
        let request = FloatRequest {
            key: self.stages.content.nodes.key(letter.node),
            side: FloatSide::from_level(line.level(&self.stages.analysis.paragraphs)),
            inline_size: width.to_px(),
            block_size: height.to_px(),
            block_start: (facts.block_start - trimmed + top).to_px(),
        };
        self.exclusions.place(request);
    }
}
