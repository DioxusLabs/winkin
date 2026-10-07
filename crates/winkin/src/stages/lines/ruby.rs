//! The pieces of ruby columns split across lines, and their annotation
//! levels, which advance independently of each other.
use super::{EdgeShapeId, LineId, Lines};
use crate::data::{Id, Table, define_id, heap_bytes};
use crate::stages::analysis::ClusterId;
use crate::stages::content::ItemId;
use crate::stages::measure::{RubyColumnId, RubyColumns, RubyLevelId};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;
use core::ops::Range;

define_id! {
    /// Names a piece of a split ruby column in [`RubyLines`]' table of them.
    pub(crate) struct RubyPieceId(u32);
}

define_id! {
    /// Names one level of a piece in [`RubyLines`]' table of them.
    pub(crate) struct PieceLevelId(u32);
}

/// The part of one annotation level that a piece sets on its line.
#[derive(Clone, Debug)]
pub(crate) struct PieceLevel {
    /// The level in the measure stage's table.
    pub(super) level: RubyLevelId,
    /// Its clusters on this line, collapsed white space at the end included.
    pub(crate) clusters: Range<ClusterId>,
    /// Its width with its edges reshaped.
    pub(crate) width: LayoutUnit,
    /// Where its clusters end once trailing collapsed white space is left out.
    pub(crate) visible_end: ClusterId,
    /// The reshaped annotation edges it keeps in the edge tables.
    pub(super) shapes: Range<EdgeShapeId>,
    /// The first item at its clusters' start, where a walk over them starts.
    pub(crate) item: ItemId,
}

/// The part of a split ruby column that one line sets.
#[derive(Clone, Debug)]
pub(crate) struct RubyPiece {
    /// The line that sets it.
    pub(crate) line: LineId,
    /// The column in the measure stage's table.
    pub(crate) column: RubyColumnId,
    /// Its base clusters on this line.
    pub(crate) base: Range<ClusterId>,
    /// Its levels, one row each, in the column's level order.
    pub(crate) levels: Range<PieceLevelId>,
    /// Its base's width on this line, with reshaped edges and any hyphen.
    pub(crate) base_width: LayoutUnit,
    /// The width the piece takes: its base's or its widest level's.
    pub(crate) width: LayoutUnit,
    /// How far it overhangs the text before it and after it.
    pub(crate) overhang: (LayoutUnit, LayoutUnit),
}

/// Where the ruby tables stand, which a rewind takes them back to.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct RubyMark {
    pieces: RubyPieceId,
    levels: PieceLevelId,
}

/// Every piece of a split ruby column, in line order, and their levels.
///
/// The breaker writes each line's pieces as it fits the line. A line holds
/// at most one piece of a column. Pieces of one line may come in any column
/// order: a split column's piece comes before a continued column's.
#[derive(Default)]
pub(crate) struct RubyLines {
    pub(crate) pieces: Table<RubyPieceId, RubyPiece>,
    pub(crate) levels: Table<PieceLevelId, PieceLevel>,
}

impl RubyLines {
    /// Empties both tables, keeping their allocations.
    pub(super) fn clear(&mut self) {
        self.pieces.clear();
        self.levels.clear();
    }

    /// Returns where the tables stand, to rewind to later.
    pub(super) fn mark(&self) -> RubyMark {
        RubyMark {
            pieces: self.pieces.next_id(),
            levels: self.levels.next_id(),
        }
    }

    /// Takes back every piece and level written since `mark`.
    pub(super) fn rewind(&mut self, mark: RubyMark) {
        self.pieces.truncate(mark.pieces);
        self.levels.truncate(mark.levels);
    }

    /// Takes back the pieces of `line`, the last line written, and of any
    /// line after it.
    ///
    /// They are the table's tail, walked back from its end.
    pub(super) fn truncate(&mut self, line: LineId) {
        let pieces = self.pieces.as_slice();
        let mut count = pieces.len();
        while let Some(last) = count.checked_sub(1)
            && pieces.get(last).is_some_and(|piece| piece.line >= line)
        {
            work::step();
            count = last;
        }
        let count = RubyPieceId::new(count);
        let levels = self
            .pieces
            .get(count)
            .map_or(self.levels.next_id(), |piece| piece.levels.start);
        self.pieces.truncate(count);
        self.levels.truncate(levels);
    }

    /// Returns the first piece on `line` or after it, by binary search.
    fn first_piece_id(&self, line: LineId) -> RubyPieceId {
        work::seek();
        let pieces = self.pieces.as_slice();
        RubyPieceId::new(pieces.partition_point(|piece| piece.line < line))
    }

    /// Returns the pieces of `lines`, in write order.
    fn pieces(&self, lines: Range<LineId>) -> &[RubyPiece] {
        let from = self.first_piece_id(lines.start);
        let to = self.first_piece_id(lines.end).max(from);
        self.pieces.get_slice(from..to).unwrap_or_default()
    }

    /// Returns the pieces of one line, in write order.
    ///
    /// It seeks the line's first piece, and walks its few pieces from there.
    pub(crate) fn line_pieces(&self, line: LineId) -> &[RubyPiece] {
        let from = self.first_piece_id(line);
        let rest = self
            .pieces
            .get_slice(from..self.pieces.next_id())
            .unwrap_or_default();
        let count = rest.iter().take_while(|piece| piece.line == line).count();
        rest.get(..count).unwrap_or_default()
    }

    /// Returns the pieces of `line`, one of the last lines written, in
    /// write order.
    ///
    /// They are walked back from the table's end, past the pieces of any
    /// line after it, with no search. The breaker asks this of the line
    /// it fits and the line before.
    pub(super) fn recent_line_pieces(&self, line: LineId) -> &[RubyPiece] {
        let pieces = self.pieces.as_slice();
        let mut end = pieces.len();
        while let Some(last) = end.checked_sub(1)
            && pieces.get(last).is_some_and(|piece| piece.line > line)
        {
            work::step();
            end = last;
        }
        let mut start = end;
        while let Some(last) = start.checked_sub(1)
            && pieces.get(last).is_some_and(|piece| piece.line == line)
        {
            work::step();
            start = last;
        }
        pieces.get(start..end).unwrap_or_default()
    }

    /// Returns the first annotation edge shape that `line`, the line just
    /// fitted, keeps.
    ///
    /// It stays out of line so that plain line records inline their
    /// construction.
    #[inline(never)]
    pub(super) fn line_first_shape(&self, line: LineId) -> Option<EdgeShapeId> {
        self.recent_line_pieces(line)
            .iter()
            .flat_map(|piece| {
                self.levels
                    .get_slice(piece.levels.clone())
                    .unwrap_or_default()
            })
            .map(|level| level.shapes.start)
            .min()
    }

    /// Returns the pieces that can hold any of `clusters`, in write order.
    ///
    /// A piece is on the line holding its base's start, and its levels
    /// follow its base. So the lines searched run from the one holding the
    /// outermost column's base start to the one holding the last cluster.
    pub(crate) fn around(
        &self,
        lines: &Lines,
        columns: &RubyColumns,
        clusters: Range<ClusterId>,
    ) -> &[RubyPiece] {
        let Some(last) = clusters.end.get().checked_sub(1) else {
            return &[];
        };
        let first = columns.outermost_start(clusters.start);
        let to = lines.at(ClusterId::new(last));
        self.pieces(lines.at(first)..LineId::new(to.get().saturating_add(1)))
    }

    /// Returns the line whose piece sets annotation cluster `cluster`.
    ///
    /// With `upstream`, returns the line whose piece's level ends at
    /// `cluster`.
    pub(crate) fn annotation_line(
        &self,
        lines: &Lines,
        columns: &RubyColumns,
        cluster: ClusterId,
        upstream: bool,
    ) -> Option<LineId> {
        let held = if upstream {
            ClusterId::new(cluster.get().checked_sub(1)?)
        } else {
            cluster
        };
        let around = self.around(lines, columns, held..ClusterId::new(held.get() + 1));
        around.iter().find_map(|piece| {
            self.levels
                .get_slice(piece.levels.clone())
                .unwrap_or_default()
                .iter()
                .any(|level| {
                    if upstream {
                        level.clusters.start < cluster && cluster <= level.clusters.end
                    } else {
                        level.clusters.contains(&cluster)
                    }
                })
                .then_some(piece.line)
        })
    }

    /// Returns `piece`'s row for level `id`.
    pub(crate) fn level(&self, piece: &RubyPiece, id: RubyLevelId) -> Option<&PieceLevel> {
        self.levels
            .get_slice(piece.levels.clone())?
            .iter()
            .find(|level| level.level == id)
    }
}

heap_bytes! { RubyLines { pieces, levels } }

/// One level's cut for the piece being fitted, which the breaker keeps in
/// its scratch across a split's trials.
#[derive(Copy, Clone, Debug)]
pub(super) struct LevelCut {
    /// The level in the measure stage's table.
    pub(super) level: RubyLevelId,
    /// Where the level resumes on the line.
    pub(super) from: ClusterId,
    /// The first item at `from`, where a walk over the level's part starts.
    pub(super) item: ItemId,
    /// Its unshaped width from `from` to its end.
    pub(super) remaining: LayoutUnit,
    /// The target the last trial cut it for.
    pub(super) target: Option<LayoutUnit>,
    /// Where the last trial cut it: its end before any trial.
    pub(super) end: ClusterId,
    /// Its reshaped width from `from` to `end`, once a trial measured it.
    pub(super) width: Option<LayoutUnit>,
}

/// How the rest of a continued column shifts prefix positions at and past
/// the column's end.
///
/// The prefix counts the column whole. The line holds only its rest, whose
/// width differs.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct ContinuationShift {
    /// The column's end, from which the shift applies.
    pub(super) end: Option<ClusterId>,
    /// The shift: the rest's width less what the prefix gives it.
    pub(super) delta: InlineLayoutUnit,
}

impl ContinuationShift {
    /// Returns the shift at boundary `end`.
    #[inline]
    pub(super) fn at(self, end: ClusterId) -> InlineLayoutUnit {
        if self.end.is_some_and(|boundary| end >= boundary) {
            self.delta
        } else {
            InlineLayoutUnit::ZERO
        }
    }
}
