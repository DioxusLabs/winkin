//! The room a line makes for its ruby annotations and emphasis marks, and
//! where its annotations stand.
//!
//! Where the annotations stand is a fact of the line, not of a column. The
//! first level over every base on a line shares one baseline. That baseline
//! sets the tallest level em box on the tallest base em box. Each level
//! further out sits on the one inside it, and the levels under the bases
//! stack the same way. Chrome's `RubyBlockPositionCalculator` does the same
//! in `PlaceLines`. The breaker and line layout share one function for it,
//! [`LevelBands::stack`].
//!
//! What a line takes for them follows Chrome's `ComputeAnnotationOverflow`
//! and `SetAnnotationOverflow`:
//! - The line's content is each text item's em box, each atomic inline's
//!   margin box, the emphasis marks and the annotations.
//! - What reaches past the line box's top moves the line down. The room the
//!   line before left under its content is taken off. The block's first
//!   line uses the host's `Area::room_above` instead.
//! - What reaches past the bottom moves what follows down. The next line
//!   moves up again as far as the room over its own content goes.
//! - Both fold into the line's extent, so `text-box-trim` trims them with
//!   the line, as Chrome does. The last line's carry is the block's, which
//!   `Layout::room_below` reports.
//!
//! Under `Config::emphasis_room = Uniform`, beyond Chrome, emphasis marks
//! are left out of that. Each marked line grows instead by what its marks
//! and their text need past its line box, split over and under in
//! proportion to the marks. The lines then stay evenly spaced.
//!
//! Only content with ruby or emphasis is looked at line by line, as Chrome
//! checks `Node().HasRuby() || has_text_emphasis`. Other blocks pay a flag
//! test a line, and look at their last line once a break for the room it
//! leaves below, as Chrome's `ContainsAnnotations()` does. Annotations stand
//! on their bases' em boxes as shifts inside each column move them. The rest
//! is reckoned on the line's baseline, and line layout's placement moves it.

use super::{RubyLines, RubyPiece};
use crate::stages::measure::{ColumnWalk, RubyLevelId};
use alloc::vec::Vec;
use core::ops::Range;

use crate::config::EmphasisRoom;
use crate::data::{Id, RunCursor, heap_bytes};
use crate::stages::LineStages;
use crate::stages::analysis::ClusterId;
use crate::stages::content::{
    Content, ContentFlags, Item, ItemFlags, ItemId, ItemKind, NodeId, TextFactsId, TextFlags,
};
use crate::stages::fonts::FontRunId;
use crate::stages::measure::{Extent, Measured, RubyColumnId, RubyColumns, RubySide, em_box};
use crate::style::EmphasisSide;
use crate::style::FirstLineVariant;
use crate::unit::LayoutUnit;
use crate::work;

/// One level of a line's annotations on one side.
///
/// `extent` unites the em boxes of the level's annotations. `raise` is how
/// far its baseline sits above the line's, negative below.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct LevelBand {
    side: RubySide,
    extent: Extent,
    raise: LayoutUnit,
}

/// How far a line's annotations reach over and under its baseline.
///
/// A side with no annotations is `None`.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct AnnotationReach {
    over: Option<LayoutUnit>,
    under: Option<LayoutUnit>,
}

/// The levels of one line's annotations and where each stands.
///
/// The over levels come first, innermost first, then the under ones. The
/// breaker and line layout each keep one as scratch. Every
/// [`stack`](Self::stack) clears it without freeing, so a warm relayout
/// allocates nothing.
pub(crate) struct LevelBands {
    bands: Vec<LevelBand>,
    /// How many of `bands` are over the line, all before those under it.
    over: usize,
}

impl LevelBands {
    /// Returns an empty set of levels, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            bands: Vec::new(),
            over: 0,
        }
    }

    /// Stacks the annotation levels of a line's `columns` and returns how far
    /// they reach.
    ///
    /// The columns are those open across the line's start and on it, then
    /// those whose bases start on it. Each level's em box sits on the one
    /// inside it, and the first sits on the bases'. Where every base under a
    /// side is empty, its levels stand on `line`, the line box's extent, as
    /// Chrome's do. `pieces`, the line's pieces with the table of their
    /// levels, skips the empty levels of columns split across lines. Linear
    /// in the levels.
    pub(crate) fn stack(
        &mut self,
        rubies: &RubyColumns,
        columns: impl Iterator<Item = RubyColumnId> + Clone,
        line: Extent,
        pieces: Option<(&RubyLines, &[RubyPiece])>,
    ) -> AnnotationReach {
        let out = &mut self.bands;
        out.clear();
        self.over = 0;
        let mut reach = AnnotationReach::default();
        for side in [RubySide::Over, RubySide::Under] {
            let from = out.len();
            let mut base = Extent::NONE;
            for id in columns.clone() {
                let Some(column) = rubies.get(id) else {
                    continue;
                };
                let mut index = from;
                let piece = pieces
                    .and_then(|(_, on_line)| on_line.iter().rev().find(|piece| piece.column == id));
                for (offset, level) in rubies.levels(column).iter().enumerate() {
                    if let Some(piece) = piece {
                        let empty = pieces
                            .and_then(|(pieces, _)| {
                                pieces.level(
                                    piece,
                                    RubyLevelId::new(column.levels.start.get() + offset),
                                )
                            })
                            .is_none_or(|level| level.clusters.is_empty());
                        if empty {
                            continue;
                        }
                    }
                    work::step();
                    if level.side != side {
                        continue;
                    }
                    index = from + level.depth as usize;
                    while out.len() <= index {
                        out.push(LevelBand {
                            side,
                            extent: Extent::NONE,
                            raise: LayoutUnit::ZERO,
                        });
                    }
                    // None at all is a level of no height at the baseline,
                    // as Chrome takes an empty annotation line.
                    let extent = level.extent.zero_if_none();
                    match out.get_mut(index) {
                        Some(band) => band.extent = band.extent.unite(extent),
                        None => out.push(LevelBand {
                            side,
                            extent,
                            raise: LayoutUnit::ZERO,
                        }),
                    }
                    index += 1;
                }
                if index > from {
                    base = base.unite(column.base_em);
                }
            }
            if side == RubySide::Over {
                self.over = out.len();
            }
            if out.len() == from {
                continue;
            }
            // Every base under the side's levels empty: they stand on the
            // line box, as Chrome's do (`node_metrics = line_box_metrics`).
            if base.is_none() || base.height() == LayoutUnit::ZERO {
                base = line;
            }
            // A depth no level on the line fills, as under a split column
            // whose nested annotations are on other lines, takes no room:
            // Chrome stacks only the annotation lines a line has.
            let bands = out.get_mut(from..).unwrap_or_default();
            match side {
                RubySide::Over => {
                    let mut top = base.ascent();
                    for band in bands {
                        if band.extent.is_none() {
                            band.raise = top;
                            continue;
                        }
                        band.raise = top + band.extent.descent();
                        top = band.raise + band.extent.ascent();
                    }
                    reach.over = Some(top);
                }
                RubySide::Under => {
                    let mut bottom = base.descent();
                    for band in bands {
                        if band.extent.is_none() {
                            band.raise = -bottom;
                            continue;
                        }
                        band.raise = -(bottom + band.extent.ascent());
                        bottom = -band.raise + band.extent.descent();
                    }
                    reach.under = Some(bottom);
                }
            }
        }
        reach
    }

    /// Returns how far the baseline of level `level` on `side` sits above
    /// the line's, negative below.
    ///
    /// Levels count out from the bases from zero. Returns `None` past the
    /// side's last level.
    #[inline]
    pub(crate) fn raise(&self, side: RubySide, level: usize) -> Option<LayoutUnit> {
        let at = match side {
            RubySide::Over => level,
            RubySide::Under => self.over.checked_add(level)?,
        };
        self.bands
            .get(at)
            .filter(|band| band.side == side)
            .map(|band| band.raise)
    }
}

heap_bytes! {
    LevelBands { bands; over }
}

impl Default for LevelBands {
    fn default() -> Self {
        Self::new()
    }
}

/// The rules for a line's annotation and mark room, shared by both
/// variants.
pub(super) struct AnnotationRoom<'a> {
    /// The measurements holding each text's mark em box, where some text
    /// sets marks.
    marks: Option<&'a Measured>,
    rule: EmphasisRoom,
}

/// What a line leaves the next at its block-end side, as Chrome's
/// `BlockStartAnnotationSpace`.
///
/// A positive value is room under the content. A negative value is how far
/// the annotations reach past the line box.
pub(super) type Carry = LayoutUnit;

/// What a line's room reads past its own items: what the line before left,
/// the line's ruby columns, and its pieces of split columns.
pub(super) struct AnnotationContinuation<'a> {
    pub(super) carry: Carry,
    /// The columns whose bases start on the line.
    pub(super) columns: Range<RubyColumnId>,
    /// The line's pieces, with the table of their levels, where any column
    /// is split.
    pub(super) pieces: Option<(&'a RubyLines, &'a [RubyPiece])>,
}

impl<'a> AnnotationRoom<'a> {
    /// Returns whether the content has ruby or marks, so every line makes
    /// room for them.
    ///
    /// The answer is the same for both variants. Like Chrome's
    /// `Node().HasRuby()`, ruby anywhere makes every line of the block count.
    pub(super) fn wanted(content: &Content, measured: &Measured) -> bool {
        let flags = content.flags;
        (flags.contains(ContentFlags::RUBY)
            && !measured
                .text(FirstLineVariant::Standard)
                .ruby_columns()
                .is_empty())
            || flags.contains(ContentFlags::EMPHASIS)
    }

    /// Returns the room rules for `content`, making room for marks by `rule`.
    ///
    /// Callers build one only where [`wanted`](Self::wanted) holds.
    pub(super) fn new(content: &Content, measured: &'a Measured, rule: EmphasisRoom) -> Self {
        let marks = content
            .flags
            .contains(ContentFlags::EMPHASIS)
            .then_some(measured);
        Self { marks, rule }
    }

    /// Returns the extent of the line over `clusters` with its annotation
    /// and mark room folded in, and what it leaves the next line.
    ///
    /// `natural` is the line's own extent and `from` its first item.
    /// `continuation` carries what the line before left. The module doc
    /// gives the rules.
    pub(super) fn room(
        &self,
        stages: &LineStages<'_>,
        bands: &mut LevelBands,
        from: ItemId,
        clusters: Range<ClusterId>,
        natural: Extent,
        continuation: AnnotationContinuation<'_>,
    ) -> (Extent, Carry) {
        let AnnotationContinuation {
            carry: before,
            columns: line_columns,
            pieces,
        } = continuation;
        if natural.is_none() {
            return (natural, LayoutUnit::ZERO);
        }
        let Range { start, end } = clusters;
        let (ascent, descent) = (natural.ascent(), natural.descent());
        let rubies = stages.measured.ruby_columns();
        // The columns split across the line's start, then those on it.
        let open = pieces
            .map_or(&[][..], |(_, on_line)| on_line)
            .iter()
            .map(|piece| piece.column)
            .filter(|&column| column < line_columns.start);
        let reach = if rubies.is_empty() {
            AnnotationReach::default()
        } else {
            bands.stack(
                rubies,
                open.chain(
                    (line_columns.start.get()..line_columns.end.get()).map(RubyColumnId::new),
                ),
                natural,
                pieces,
            )
        };
        // What the content reaches from the baseline: the em boxes, their
        // marks and the annotations.
        let mut over = LayoutUnit::MIN;
        let mut under = LayoutUnit::MIN;
        let mut marked_over = false;
        let mut marked_under = false;
        // For uniform emphasis room: the tallest marks each side, and the
        // text they mark.
        let mut marks_over = LayoutUnit::ZERO;
        let mut marks_under = LayoutUnit::ZERO;
        let mut marked = Extent::NONE;
        // The line's columns not yet passed. The marked text comes in text
        // order, as the columns do.
        let facts = &stages.content.facts;
        let mut columns = ColumnWalk::from_column(rubies, line_columns.start, start);
        stages.ems_on_line(from, start, end, |item, text, em, first| {
            over = over.max(em.ascent());
            under = under.max(em.descent());
            let Some(measured) = self.marks else {
                return;
            };
            let row = facts.text(text);
            if item.kind != ItemKind::Text || !row.has(TextFlags::EMPHASIS) {
                return;
            }
            let Some(metrics) = measured.text_metrics(text) else {
                return;
            };
            let side = row.emphasis_side;
            let height = metrics.mark.height();
            if self.rule == EmphasisRoom::Uniform {
                marked = marked.unite(em);
                match side {
                    EmphasisSide::Over => marks_over = marks_over.max(height),
                    EmphasisSide::Under => marks_under = marks_under.max(height),
                }
                return;
            }
            // A mark on the side of its column's annotation is set past
            // the annotation.
            let past = annotation_past(rubies, &mut columns, bands, first, side);
            match side {
                EmphasisSide::Over => {
                    over = over.max(past.unwrap_or(em.ascent()) + height);
                    marked_over = true;
                }
                EmphasisSide::Under => {
                    under = under.max(past.unwrap_or(em.descent()) + height);
                    marked_under = true;
                }
            }
        });
        if let Some(reach) = reach.over {
            over = over.max(reach);
        }
        if let Some(reach) = reach.under {
            under = under.max(reach);
        }
        let annotated_over = reach.over.is_some() || marked_over;
        let annotated_under = reach.under.is_some() || marked_under;
        // A line with next to nothing on it keeps room for a letter of the
        // block's size in its line box, as Chrome's does.
        let height = ascent + descent;
        let size = LayoutUnit::from_px(stages.block_size());
        if over == LayoutUnit::MIN || under == LayoutUnit::MIN || over + under < size {
            let leading = ((height - size).half()).max(LayoutUnit::ZERO);
            over = ascent - leading;
            under = descent - leading;
        }
        // Content past the line box counts only where annotations or marks
        // are on that side: a font's letters may reach past it anyway.
        if over > ascent && !annotated_over {
            over = ascent;
        }
        if under > descent && !annotated_under {
            under = descent;
        }
        let overflow_over = (over - ascent).max(LayoutUnit::ZERO);
        let space_over = (ascent - over).max(LayoutUnit::ZERO);
        let overflow_under = (under - descent).max(LayoutUnit::ZERO);
        let space_under = (descent - under).max(LayoutUnit::ZERO);
        // The line moves down by what reaches over it, less the room the
        // line before left. Where the line before reached past its bottom,
        // the line moves up as far as the room over its content goes.
        let mut shift = overflow_over;
        if before < LayoutUnit::ZERO && space_over > LayoutUnit::ZERO {
            shift = -space_over.min(-before);
        }
        if overflow_over > LayoutUnit::ZERO && before > LayoutUnit::ZERO {
            shift = (overflow_over - before).max(LayoutUnit::ZERO);
        }
        let mut extent = Extent::new(ascent + shift, descent + overflow_under);
        let carry = if overflow_under > LayoutUnit::ZERO {
            -overflow_under
        } else {
            space_under
        };
        // Uniform room for marks: what the marks and their text need past
        // the line box, split over and under in proportion to the marks.
        // The first line takes off the room above the block.
        if self.rule == EmphasisRoom::Uniform && !marked.is_none() {
            let wanted = marks_over + marked.height() + marks_under;
            let lent = if start == ClusterId::new(0) {
                before.max(LayoutUnit::ZERO)
            } else {
                LayoutUnit::ZERO
            };
            let short = (wanted - extent.height() - lent).max(LayoutUnit::ZERO);
            let marks = marks_over + marks_under;
            if short > LayoutUnit::ZERO && marks > LayoutUnit::ZERO {
                let up = LayoutUnit::from_px(short.to_px() * marks_over.to_px() / marks.to_px());
                extent = Extent::new(extent.ascent() + up, extent.descent() + (short - up));
            }
        }
        (extent, carry)
    }
}

/// The prepared data a line's room reads, in the line's variant.
impl LineStages<'_> {
    /// Returns the block's computed font size in pixels, in this variant.
    ///
    /// An empty line keeps room for it, and the scorer weighs lines by it.
    /// Content never built has no block and returns 0.
    pub(super) fn block_size(&self) -> f32 {
        let (content, block) = (self.content, NodeId::BLOCK);
        content.nodes.kind(block).map_or(0.0, |_| {
            let request = content.facts.text_request(self.text_facts(block));
            content.facts.request(request).font.computed_size()
        })
    }

    /// Returns what a line without ruby or marks leaves the block after it.
    ///
    /// It equals the carry [`AnnotationRoom::room`] gives such a line, with
    /// nothing left by the line before. It serves
    /// [`Lines::room_below`](super::Lines::room_below) for the last line.
    /// It walks the em boxes once and stacks no levels.
    pub(super) fn plain_carry(
        &self,
        from: ItemId,
        start: ClusterId,
        end: ClusterId,
        natural: Extent,
    ) -> Carry {
        debug_assert!(
            self.measured.ruby_columns().is_empty(),
            "a line with ruby makes its room"
        );
        if natural.is_none() {
            return LayoutUnit::ZERO;
        }
        let (ascent, descent) = (natural.ascent(), natural.descent());
        let (mut over, mut under) = (LayoutUnit::MIN, LayoutUnit::MIN);
        self.ems_on_line(from, start, end, |_, _, em, _| {
            over = over.max(em.ascent());
            under = under.max(em.descent());
        });
        // This settles it as `room` does with nothing past the line box. A
        // nearly empty line keeps room for a letter of the block's size.
        // Content reaching under the line box counts to its bottom.
        let size = LayoutUnit::from_px(self.block_size());
        if over == LayoutUnit::MIN || under == LayoutUnit::MIN || over + under < size {
            let leading = ((ascent + descent - size).half()).max(LayoutUnit::ZERO);
            under = descent - leading;
        }
        (descent - under.min(descent)).max(LayoutUnit::ZERO)
    }

    /// Calls `each` for every item on the line from `start` to `end` that
    /// has an em box, starting at item `from`.
    ///
    /// `each` gets the item, its text facts, its em box from
    /// [`em`](Self::em) and its first cluster. Annotations are skipped,
    /// since they belong to their column. So is the initial letter, which
    /// stands on no line: Chrome's letter box adds no height to the line
    /// its annotation room is measured from (`ComputeAnnotationOverflow`).
    #[inline]
    fn ems_on_line(
        &self,
        from: ItemId,
        start: ClusterId,
        end: ClusterId,
        mut each: impl FnMut(&Item, TextFactsId, Extent, ClusterId),
    ) {
        let items = &self.content.items;
        let item_clusters = &self.analysis.item_clusters;
        let letter = self.measured.initial_letter().map(|letter| letter.node);
        // The font runs, for a text item with no single em box. The cursor
        // is sought once and walks with the items.
        let mut runs = None;
        let mut id = from;
        while let Some(item) = items.get(id) {
            work::step();
            let Range {
                start: first,
                end: next,
            } = item_clusters.range(id);
            if first >= end {
                break;
            }
            let this = id;
            id = ItemId::new(id.get() + 1);
            let on_line = first < next && next > start;
            if !on_line || item.flags.contains(ItemFlags::ANNOTATION) {
                continue;
            }
            if letter.is_some_and(|letter| self.content.nodes.contains(letter, item.node)) {
                continue;
            }
            let text = self.text_facts(item.node);
            let em = self.em(this, item, text, first.max(start)..next.min(end), &mut runs);
            if !em.is_none() {
                each(item, text, em, first);
            }
        }
    }

    /// Returns the em box of item `id` over its `clusters` on a line.
    ///
    /// A text item gives its em box, or its part's from the font runs where
    /// it has no single box. An atomic inline gives its margin box. Anything
    /// else gives none.
    fn em(
        &self,
        id: ItemId,
        item: &Item,
        text: TextFactsId,
        clusters: Range<ClusterId>,
        runs: &mut Option<RunCursor<FontRunId, ClusterId>>,
    ) -> Extent {
        match item.kind {
            ItemKind::Text => self.measured.em_boxes().get(id).unwrap_or_else(|| {
                let (fonts, variant) = (self.fonts, self.variant);
                let (table, end) = (fonts.runs(variant), self.analysis.clusters.end_id());
                let Some(mut run) = runs.or_else(|| table.cursor_containing(clusters.start, end))
                else {
                    return Extent::NONE;
                };
                table.step_to(&mut run, clusters.start, end);
                *runs = Some(run);
                let request = self.content.facts.text_request(text);
                em_box(fonts, request, clusters, variant, run.id()).0
            }),
            ItemKind::Atomic => self.measured.extents.get(id),
            _ => Extent::NONE,
        }
    }
}

/// Returns how far from the baseline the outermost annotation on `side`
/// over `cluster`'s base reaches.
///
/// An emphasis mark in that base is set past it, as Chrome's
/// `SetTextEmphasisAnnotationMetrics` does. The annotation's own em box
/// stands on its level's baseline, as in Chrome's
/// `UpdateColumnLayoutAnnotationMetrics`. The annotation's depth names its
/// band in `bands`.
fn annotation_past(
    rubies: &RubyColumns,
    columns: &mut ColumnWalk<'_>,
    bands: &LevelBands,
    cluster: ClusterId,
    side: EmphasisSide,
) -> Option<LayoutUnit> {
    let side = RubySide::from(side);
    let outermost = columns.outermost(cluster, side)?;
    let level = rubies.level(outermost)?;
    let extent = level.extent.zero_if_none();
    let raise = bands.raise(side, level.depth as usize)?;
    Some(match side {
        RubySide::Over => raise + extent.ascent(),
        RubySide::Under => -raise + extent.descent(),
    })
}
