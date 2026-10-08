//! Lays out one line at a time and writes its items once.
//!
//! `Placer::line` runs each line's phases in order:
//! - the pieces, in logical order (`pieces`);
//! - `line-padding`, justification and alignment;
//! - reordering by bidi level;
//! - box parts and the room their edges take (`boxes`);
//! - positions, left to right;
//! - truncation, on a line cut for an ellipsis (`cut`);
//! - the items, then the ruby annotation lines (`ruby`).
//!
//! **Alignment** is Chrome's `LineOffsetForTextAlign`. It sees the content,
//! and only the fitting part of preserved white space before a forced break
//! or the block's end. It moves the whole line, tabs included.
//!
//! **Bidi** follows Chrome.
//! - L1: the line's trailing preserved white space takes the paragraph's
//!   level. Under `break-spaces` it splits from the text before it, as
//!   Blink's `SplitTrailingBidiPreservedSpace` does, only where it ends
//!   its item or is all its item holds on the line. Inside one item's text
//!   at one level it keeps that level.
//! - L2: the pieces reorder by level with the resolver's `reorder`, as
//!   Blink's `BidiReorder` does. A piece with no level of its own takes the
//!   next piece's, or the paragraph's at the line's end.
//!
//! **Each phase has its own scratch:** [`LinePieces`], [`LineBoxes`],
//! [`PaintLinks`] and [`LineRubies`]. Each goes only to the phases that use
//! it, and is cleared, never dropped.

use super::{Piece, PieceKind, Placer};
use alloc::vec::Vec;
use core::iter;
use core::ops::Range;

use super::boxes::{PaintLinks, paint_order};
use super::cut::LineEllipsis;
use super::pieces::TabLine;
use super::ruby::{BoxesInAnnotation, LineColumn};
use super::{
    BlockResult, FragmentItem, FragmentItemFlags, FragmentItemId, FragmentItemKind, Fragments,
    Justified, LineJustification, PlaceInput,
};
use crate::config::TabJustification;
use crate::data::IdRange;
use crate::data::{Id, Table, define_id, heap_bytes, u32_to_index};
use crate::stages::analysis::{BidiLevel, ClusterId, ParagraphFlags, ParagraphId};
use crate::stages::content::{BoxFacts, ContentFlags, ItemId, NodeId, TextFlags};
use crate::stages::lines::{LevelBands, LineBand, LineFlags, LineId, LineView};
use crate::stages::measure::{AutospaceRules, JustifyOpportunities, KeptBoxId, RubyColumnId};
use crate::stages::shape::ShapedRunId;
use crate::style::{FirstLineVariant, TextAlign, TextGroupAlign};
use crate::unicode::reorder_bidi;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

define_id! {
    /// Names a kept box of the line being laid out, in [`LineBoxes`].
    pub(super) struct LineBoxId(u32);
}

define_id! {
    /// Names a box part on the line being laid out, in [`LineBoxes`].
    pub(super) struct BoxPartId(u32);
}

define_id! {
    /// Names a piece of the line being laid out by its logical place, in
    /// [`LinePieces`].
    pub(super) struct PieceId(u32);
}

define_id! {
    /// Names a place in the line's visual order, counted from the left.
    ///
    /// [`LinePieces`] says which piece stands there. [`LineBoxes`] holds the
    /// room box edges take beside it.
    pub(super) struct VisualId(u32);
}

/// The piece an ellipsis cut divides: the piece, the boundary, and the kept
/// part's advance.
///
/// It is written as two items, the kept part and the hidden part, as Chrome
/// hides its item and draws a truncated one in its place. A line has one at
/// most, so the pieces don't carry it.
#[derive(Copy, Clone, Debug)]
pub(super) struct PieceSplit {
    pub(super) piece: PieceId,
    pub(super) at: ClusterId,
    pub(super) kept: InlineLayoutUnit,
}

/// How a line is cut for an ellipsis: the ellipsis, and the piece the cut
/// divides, if any.
#[derive(Copy, Clone, Debug)]
pub(super) struct LineCut {
    pub(super) ellipsis: LineEllipsis,
    pub(super) split: Option<PieceSplit>,
}

/// A box that keeps a fragment, as one line has it.
#[derive(Copy, Clone, Debug)]
pub(super) struct LineBox {
    pub(super) node: NodeId,
    /// Its row among the kept boxes of the line's variant, which its box
    /// parts name.
    pub(super) kept: Option<KeptBoxId>,
    /// The box around it on the line, and how deep it is among them.
    pub(super) parent: Option<LineBoxId>,
    pub(super) depth: u32,
    /// How far its baseline is raised from the line's: its own shift plus
    /// every box's around it.
    pub(super) shift: LayoutUnit,
    /// How far the baseline of the nearest `top` or `bottom` box, itself or
    /// one around it, is raised from the line's; zero where there is none.
    ///
    /// A `top` or `bottom` box inside it is placed from this.
    pinned: LayoutUnit,
    /// Whether it opens on the line, rather than continuing from the line
    /// before, and whether it closes on it.
    pub(super) opens: bool,
    pub(super) closes: bool,
    /// Whether its left and right sides are its own edges on the line.
    ///
    /// Its direction decides which of start and end is on the left. A box
    /// that clones has both on every line.
    pub(super) has_left: bool,
    pub(super) has_right: bool,
    /// The room its left and right edges take, margins included, and the
    /// margins alone; zero on a side that is not its edge.
    pub(super) left_room: InlineLayoutUnit,
    pub(super) right_room: InlineLayoutUnit,
    pub(super) left_margin: InlineLayoutUnit,
    pub(super) right_margin: InlineLayoutUnit,
    /// Where its leftmost box part starts and its rightmost ends,
    /// visually, and the box part being found.
    pub(super) first_left: Option<VisualId>,
    pub(super) last_right: Option<VisualId>,
    pub(super) active: Option<BoxPartId>,
}

/// Which ends of the line a ruby column standing there sits flush with.
///
/// These are Chrome's `on_start_edge` and `on_end_edge`
/// (`LogicalLineBuilder::PlaceRubyColumn`). They apply on a justified line
/// in a block with no bidi. The start also applies on a paragraph's last
/// line when the block's `text-align` is `justify`, as in Chrome. The end
/// applies only when the line itself is justified.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct RubyEdges {
    pub(super) start: bool,
    pub(super) end: bool,
    /// The line's room, which a column holding the whole line spreads its
    /// base over.
    pub(super) room: LayoutUnit,
}

/// A box part: the pieces of one box that reordering leaves side by side,
/// from its leftmost to its rightmost place, inclusive.
#[derive(Copy, Clone, Debug)]
pub(super) struct BoxPart {
    /// The box, in the line's table of them.
    pub(super) line_box: LineBoxId,
    pub(super) left: VisualId,
    pub(super) right: VisualId,
    /// The room inner boxes' edges take at its left and right ends, between
    /// its border and its content.
    ///
    /// The left sums the left edges of deeper boxes whose leftmost box
    /// part starts here. The right does the same for right edges ending
    /// here.
    pub(super) inner_left: InlineLayoutUnit,
    pub(super) inner_right: InlineLayoutUnit,
    /// The item of the box part painted after it, or `None` for the last.
    pub(super) next: Option<FragmentItemId>,
}

/// The pieces of a line being laid out and their visual order: the line's
/// own, or an annotation line's.
pub(super) struct LinePieces {
    /// Its pieces in logical order: its content, then what hangs.
    pub(super) logical: Table<PieceId, Piece>,
    /// Each piece's level as the reordering reads it.
    pub(super) levels: Table<PieceId, BidiLevel>,
    /// The pieces in visual order.
    pub(super) order: Table<VisualId, PieceId>,
    /// The resolver's `reorder` output as raw indexes, turned into `order`
    /// in [`reorder`](Self::reorder).
    resolved: Vec<u32>,
    /// The anchors at the line's end that go on to the next line's start,
    /// in order.
    pub(super) carried: Vec<Piece>,
}

impl LinePieces {
    /// Makes an empty table, allocating nothing.
    const fn new() -> Self {
        Self {
            logical: Table::new(),
            levels: Table::new(),
            order: Table::new(),
            resolved: Vec::new(),
            carried: Vec::new(),
        }
    }

    /// Adds the anchors the line before carried on, in the box `owner`
    /// open across the line's start.
    fn take_carried(&mut self, owner: Option<LineBoxId>) {
        for at in 0..self.carried.len() {
            if let Some(&piece) = self.carried.get(at) {
                self.push(Piece { owner, ..piece });
            }
        }
        self.carried.clear();
    }

    /// Returns the advance of the hanging pieces.
    fn hang(&self) -> InlineLayoutUnit {
        (self.logical.as_slice().iter())
            .filter(|piece| piece.flags.contains(FragmentItemFlags::HANGS))
            .fold(InlineLayoutUnit::ZERO, |sum, piece| sum + piece.advance)
    }

    /// Adds `piece` as a piece of its own.
    #[inline]
    pub(super) fn push(&mut self, piece: Piece) {
        self.logical
            .push_bounded(piece, "no more pieces on a line than a PieceId names");
    }

    /// Adds `piece` as [`push_or_join`](Self::push_or_join) does, with a gap
    /// for any room `before` and `after` it.
    ///
    /// A spread ruby base or annotation cuts its text at each opportunity
    /// this way.
    pub(super) fn push_with_room(
        &mut self,
        piece: Piece,
        (before, after): (InlineLayoutUnit, InlineLayoutUnit),
    ) {
        if before > InlineLayoutUnit::ZERO {
            self.push(piece.gap(piece.start, before));
        }
        self.push_or_join(piece);
        if after > InlineLayoutUnit::ZERO {
            self.push(piece.gap(piece.end, after));
        }
    }

    /// Moves the room each spread gap holds into the text of its node beside
    /// it, as Chrome expands the glyph before an opportunity: into the text
    /// before it, or the text after it where none is before.
    ///
    /// The gap keeps its place with no room, and the text takes the room on
    /// the gap's side. Nothing moves along the line. Returns whether any
    /// text took room in.
    pub(super) fn absorb_spread(&mut self) -> bool {
        let mut moved = false;
        let count = self.logical.next_id().get();
        for at in 0..count {
            let Some(gap) = self.logical.get(PieceId::new(at)).copied() else {
                continue;
            };
            if gap.kind != PieceKind::Spread || gap.advance == InlineLayoutUnit::ZERO {
                continue;
            }
            work::step();
            let spread = |piece: &Piece| piece.kind == PieceKind::Spread && piece.node == gap.node;
            // A tab is as wide as the distance to its stop, and takes no room.
            let text = |piece: &Piece| {
                piece.kind == PieceKind::Text
                    && piece.node == gap.node
                    && !piece.flags.contains(FragmentItemFlags::TAB)
            };
            // The text before, past the gaps it already took in.
            let mut back = at;
            while let Some(previous) = back.checked_sub(1)
                && self.logical.get(PieceId::new(previous)).is_some_and(spread)
            {
                back = previous;
            }
            let before = back
                .checked_sub(1)
                .filter(|&previous| self.logical.get(PieceId::new(previous)).is_some_and(text));
            let mut on = at + 1;
            while self.logical.get(PieceId::new(on)).is_some_and(spread) {
                on += 1;
            }
            let after =
                Some(on).filter(|&next| self.logical.get(PieceId::new(next)).is_some_and(text));
            let Some((owner, gap_after)) =
                before.map(|at| (at, true)).or(after.map(|at| (at, false)))
            else {
                continue;
            };
            let Some(text) = self.logical.get_mut(PieceId::new(owner)) else {
                continue;
            };
            text.advance += gap.advance;
            // A gap after the text in logical order is on its right, or its
            // left where it runs right to left.
            if gap_after != text.level.is_rtl() {
                text.room.1 += gap.advance;
            } else {
                text.room.0 += gap.advance;
            }
            if let Some(gap) = self.logical.get_mut(PieceId::new(at)) {
                gap.advance = InlineLayoutUnit::ZERO;
            }
            moved = true;
        }
        moved
    }

    /// Moves `room` into the leftmost text piece of `range`, in visual
    /// order, or the rightmost where `!left`, and returns how much it moved:
    /// none where `range` holds no text. Pieces `nested` names are passed
    /// over.
    ///
    /// The text takes the room on that side, as Chrome expands the glyph at
    /// the edge of a spread base or annotation.
    pub(super) fn absorb_end(
        &mut self,
        range: Range<PieceId>,
        (room, left): (InlineLayoutUnit, bool),
        nested: impl Fn(PieceId) -> bool,
    ) -> InlineLayoutUnit {
        if room <= InlineLayoutUnit::ZERO {
            return InlineLayoutUnit::ZERO;
        }
        let logical = &self.logical;
        let holds = |at: &PieceId| {
            range.contains(at)
                && !nested(*at)
                && logical.get(*at).is_some_and(|piece| {
                    piece.kind == PieceKind::Text && !piece.flags.contains(FragmentItemFlags::HANGS)
                })
        };
        let order = self.order.as_slice().iter().copied();
        let found = if left {
            order.inspect(|_| work::step()).find(holds)
        } else {
            order.rev().inspect(|_| work::step()).find(holds)
        };
        let Some(text) = found.and_then(|at| self.logical.get_mut(at)) else {
            return InlineLayoutUnit::ZERO;
        };
        text.advance += room;
        if left {
            text.room.0 += room;
        } else {
            text.room.1 += room;
        }
        room
    }

    /// Inserts `piece` before the piece `at`, moving the rest one place on.
    fn insert(&mut self, at: PieceId, piece: Piece) {
        let end = PieceId::new(self.logical.next_id().get() + 1);
        self.push(piece);
        if let Some(moved) = self.logical.get_slice_mut(at..end) {
            moved.rotate_right(1);
        }
    }

    /// Adds `piece`, joining it to the last piece where they match.
    ///
    /// They match when kind, flags, node, level, owner, orientation and font
    /// agree and the last ends where `piece` starts. A tab never joins, so
    /// its item's size is its width.
    #[inline]
    pub(super) fn push_or_join(&mut self, piece: Piece) {
        work::step();
        if !piece.flags.contains(FragmentItemFlags::TAB)
            && let Some(last) = self.logical.last_mut()
            && last.kind == piece.kind
            && last.flags == piece.flags
            && last.node == piece.node
            && last.level == piece.level
            && last.owner == piece.owner
            && last.end == piece.start
            && last.orientation == piece.orientation
            && (piece.font.is_none() || last.font.is_none() || last.font == piece.font)
        {
            last.end = piece.end;
            last.advance += piece.advance;
            // The run of the first shaped cluster, or of the first.
            if last.run.is_none() || (last.font.is_none() && piece.font.is_some()) {
                last.run = piece.run;
            }
            last.font = last.font.or(piece.font);
            last.opportunities = last.opportunities.saturating_add(piece.opportunities);
            return;
        }
        self.push(piece);
    }

    /// Returns the piece at place `at` in visual order.
    #[inline]
    pub(super) fn visual(&self, at: VisualId) -> Option<&Piece> {
        self.order
            .get(at)
            .and_then(|&logical| self.logical.get(logical))
    }

    /// Puts the pieces in visual order by level with the resolver's
    /// `reorder`; `base` is the lowest level.
    ///
    /// A piece with no level of its own takes the next piece's, or `base` at
    /// the end, as Blink's opaque items do. `levels` keeps the result.
    ///
    /// Where every piece is at `base`, as on most lines, the order is
    /// straight through, or reversed for right-to-left, without the walk.
    pub(super) fn reorder(&mut self, base: BidiLevel) {
        self.levels.clear();
        let count = self.logical.len();
        if self.levels.extend(iter::repeat_n(base, count)).is_none() {
            debug_assert!(false, "no more pieces on a line than a PieceId names");
        }
        let uniform = self
            .logical
            .as_slice()
            .iter()
            .all(|piece| piece.kind.is_opaque() || piece.level == base);
        if !uniform {
            let mut next = base;
            for (at, piece) in self.logical.iter().rev() {
                if !piece.kind.is_opaque() {
                    next = piece.level;
                }
                if let Some(level) = self.levels.get_mut(at) {
                    *level = next;
                }
            }
        }
        self.order.clear();
        let pieces = self.logical.ids();
        let placed = if uniform && !base.is_rtl() {
            self.order.extend(pieces)
        } else if uniform {
            self.order.extend(pieces.rev())
        } else {
            self.resolved.clear();
            self.resolved.resize(self.logical.len(), 0);
            let levels = &self.levels;
            reorder_bidi(&mut self.resolved, |i| {
                levels.get(PieceId::new(i)).copied().unwrap_or(base).get()
            });
            let resolved = self.resolved.iter();
            self.order
                .extend(resolved.map(|&logical| PieceId::new(u32_to_index(logical))))
        };
        if placed.is_none() {
            debug_assert!(false, "no more pieces on a line than a VisualId names");
        }
    }

    /// Places the pieces left to right from `start`, adding the room `rooms`
    /// gives before and after each place.
    pub(super) fn place(
        &mut self,
        start: InlineLayoutUnit,
        rooms: &Table<VisualId, (InlineLayoutUnit, InlineLayoutUnit)>,
    ) {
        let mut pen = start;
        for (at, &logical) in self.order.iter() {
            work::step();
            let (before, after) = rooms.get(at).copied().unwrap_or_default();
            pen += before;
            if let Some(piece) = self.logical.get_mut(logical) {
                piece.inline = pen;
                pen += piece.advance;
            }
            pen += after;
        }
    }
}

/// The kept boxes of the line being laid out.
///
/// It holds each box as the line has it, the boxes open during the piece
/// walk, and after reordering the box parts and their edges' room. It
/// also carries the boxes open at the line's end into the next line.
pub(super) struct LineBoxes {
    /// The boxes that keep a fragment on the line, as they open.
    pub(super) boxes: Table<LineBoxId, LineBox>,
    /// Those open where the walk is, outermost first.
    pub(super) stack: Vec<LineBoxId>,
    /// Their box parts.
    pub(super) parts: Table<BoxPartId, BoxPart>,
    /// The box parts in the order they end, innermost first at a shared
    /// piece. Each one's inner right is summed in this order.
    pub(super) ended: Vec<BoxPartId>,
    /// The room box edges take before and after each place in visual order.
    pub(super) rooms: Table<VisualId, (InlineLayoutUnit, InlineLayoutUnit)>,
    /// The kept boxes open at the last line's end, outermost first, with
    /// their rows, carried to the next.
    open: Vec<(NodeId, Option<KeptBoxId>)>,
}

impl LineBoxes {
    /// Makes an empty table, allocating nothing.
    const fn new() -> Self {
        Self {
            boxes: Table::new(),
            stack: Vec::new(),
            parts: Table::new(),
            ended: Vec::new(),
            rooms: Table::new(),
            open: Vec::new(),
        }
    }

    /// Empties the line's tables, keeping the boxes open from the line
    /// before.
    fn begin_line(&mut self) {
        self.boxes.clear();
        self.stack.clear();
        self.parts.clear();
        self.ended.clear();
    }

    /// Records the kept boxes still open at the line's end, which go on to
    /// the next line.
    fn carry_open(&mut self) {
        self.open.clear();
        for &index in self.stack.iter() {
            if let Some(still) = self.boxes.get(index) {
                self.open.push((still.node, still.kept));
            }
        }
    }

    /// Returns the innermost kept box open where the walk is, which owns a
    /// piece made there.
    #[inline]
    pub(super) fn innermost(&self) -> Option<LineBoxId> {
        self.stack.last().copied()
    }

    /// Returns the innermost open kept box's shift and pinned shift; zeros
    /// outside every box.
    ///
    /// A culled box has no shift and is never `top` or `bottom`, so the
    /// innermost kept box gives its parent's baseline.
    #[inline]
    pub(super) fn innermost_shifts(&self) -> (LayoutUnit, LayoutUnit) {
        self.innermost()
            .and_then(|id| self.boxes.get(id))
            .map_or((LayoutUnit::ZERO, LayoutUnit::ZERO), |open| {
                (open.shift, open.pinned)
            })
    }

    /// Opens the kept box `node`, row `kept`, at boundary `at`, raised as
    /// `shifts` says.
    ///
    /// It adds the box inside the one open around it and a placeholder piece
    /// to `pieces`. `opens` is false for a box continuing from the line
    /// before.
    pub(super) fn open_box(
        &mut self,
        pieces: &mut LinePieces,
        (node, kept): (NodeId, Option<KeptBoxId>),
        at: ClusterId,
        opens: bool,
        (shift, pinned): (LayoutUnit, LayoutUnit),
    ) {
        let parent = self.innermost();
        let depth = parent
            .and_then(|parent| self.boxes.get(parent))
            .map_or(0, |parent| parent.depth + 1);
        let Some(id) = self.boxes.push(LineBox {
            node,
            kept,
            parent,
            depth,
            shift,
            pinned,
            opens,
            closes: false,
            has_left: false,
            has_right: false,
            left_room: InlineLayoutUnit::ZERO,
            right_room: InlineLayoutUnit::ZERO,
            left_margin: InlineLayoutUnit::ZERO,
            right_margin: InlineLayoutUnit::ZERO,
            first_left: None,
            last_right: None,
            active: None,
        }) else {
            debug_assert!(false, "no more boxes on a line than a LineBoxId names");
            return;
        };
        self.stack.push(id);
        pieces.push(Piece::new(
            PieceKind::Place,
            node,
            BidiLevel::LTR,
            at..at,
            InlineLayoutUnit::ZERO,
            Some(id),
            shift,
        ));
    }
}

/// The ruby columns on the line being laid out, and the annotation lines
/// they set.
pub(super) struct LineRubies {
    /// The ruby columns on the line.
    pub(super) columns: Vec<LineColumn>,
    /// Where its annotation levels stand.
    pub(super) bands: LevelBands,
    /// The pieces of the annotation line being set.
    pub(super) annotation: LinePieces,
    /// The boxes inside the annotation line being set.
    pub(super) boxes: BoxesInAnnotation,
}

impl LineRubies {
    /// Makes an empty table, allocating nothing.
    const fn new() -> Self {
        Self {
            columns: Vec::new(),
            bands: LevelBands::new(),
            annotation: LinePieces::new(),
            boxes: BoxesInAnnotation::new(),
        }
    }
}

/// Line layout's working memory, one part for each phase of a line.
///
/// Every vector is cleared, never dropped, so a warm relayout allocates
/// nothing.
pub(crate) struct PlaceScratch {
    /// The line's pieces and their visual order.
    pieces: LinePieces,
    /// Its kept boxes and their box parts.
    boxes: LineBoxes,
    /// Where its box parts' items go, and their paint order.
    links: PaintLinks,
    /// Its ruby columns and their annotation lines.
    rubies: LineRubies,
}

impl PlaceScratch {
    /// Makes empty scratch, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            pieces: LinePieces::new(),
            boxes: LineBoxes::new(),
            links: PaintLinks::new(),
            rubies: LineRubies::new(),
        }
    }

    /// Forgets the boxes and anchors the last line carried on, so none of
    /// another layout carries onto the first line.
    pub(super) fn begin(&mut self) {
        self.boxes.open.clear();
        self.pieces.carried.clear();
    }
}

heap_bytes! {
    PlaceScratch { pieces, boxes, links, rubies }
}

heap_bytes! {
    LinePieces { logical, levels, order, resolved, carried }
}

heap_bytes! {
    LineBoxes { boxes, stack, parts, ended, rooms, open }
}

heap_bytes! {
    LineRubies { columns, bands, annotation, boxes }
}

/// `text-group-align`: how far each line's band narrows, and how far its
/// left end moves in, to align the lines as one group.
///
/// Chrome doesn't parse it; this follows CSS. `least` is the least room any
/// line spares, found before any line is placed. If a line overflows, the
/// group spares nothing.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct Group {
    least: LayoutUnit,
    left: LayoutUnit,
}

impl Group {
    /// Finds the group for `input`'s lines, as the block's style asks.
    fn new(input: &PlaceInput<'_>) -> Self {
        let block = input.stages.content.block;
        let lines = input.lines.lines.as_slice();
        let Some(first) = lines.first() else {
            return Self::default();
        };
        if block.text_group_align == TextGroupAlign::None {
            return Self::default();
        }
        let least = lines
            .iter()
            .map(|line| line.band.width() - (line.indent + line.width))
            .min()
            .unwrap_or(LayoutUnit::ZERO);
        if least <= LayoutUnit::ZERO {
            return Self::default();
        }
        // A block whose paragraphs each pick their direction uses the first
        // line's.
        let rtl = BidiLevel::from_direction(block.direction)
            .unwrap_or_else(|| first.level(&input.stages.analysis.paragraphs))
            .is_rtl();
        let left = block.text_group_align.left_inset(least, rtl);
        Self { least, left }
    }

    /// Returns `band` narrowed for the group.
    fn narrow(self, band: LineBand) -> LineBand {
        if self.least == LayoutUnit::ZERO {
            return band;
        }
        band.inset(self.left, self.least - self.left)
    }
}

/// Which way a line's content goes in its band.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Side {
    Left,
    Right,
    Center,
}

impl Side {
    /// Returns the side a line set in `align` goes to.
    ///
    /// A `justify` line here is one that cannot be justified, so it is set
    /// at its start, as Chrome sets it.
    pub(super) fn from_align(align: TextAlign, rtl: bool) -> Self {
        match align {
            TextAlign::Start | TextAlign::Justify if rtl => Self::Right,
            TextAlign::Start | TextAlign::Justify => Self::Left,
            TextAlign::End if rtl => Self::Left,
            TextAlign::End => Self::Right,
            TextAlign::Left => Self::Left,
            TextAlign::Right => Self::Right,
            TextAlign::Center => Self::Center,
        }
    }

    /// Returns how far a line on this side moves in from its band's left,
    /// given `spare` room, as Chrome's `LineOffsetForTextAlign` does.
    ///
    /// A line that overflows spills out on its end side. A centered line
    /// moves by half the room, truncated on the grid.
    pub(super) fn left_offset(self, rtl: bool, spare: LayoutUnit) -> LayoutUnit {
        match self {
            Self::Left if rtl => spare.min(LayoutUnit::ZERO),
            Self::Left => LayoutUnit::ZERO,
            Self::Right if rtl => spare,
            Self::Right => spare.max(LayoutUnit::ZERO),
            Self::Center if rtl && spare < LayoutUnit::ZERO => spare,
            Self::Center => spare.half().max(LayoutUnit::ZERO),
        }
    }
}

/// Where a line is set in its band.
#[derive(Copy, Clone, Debug)]
pub(super) struct LineOffset {
    /// Its line box's left edge, from the area's line-left.
    pub(super) left: LayoutUnit,
    /// Where its first piece in visual order starts, from the line box's
    /// left.
    ///
    /// Left to right, it is the indent. Right to left, it is where hanging
    /// content outside the line box's left starts, so the content ends at
    /// its width.
    start: InlineLayoutUnit,
}

impl<'a> Placer<'a> {
    /// Makes the placer for `input`'s lines, with the block result `block`.
    ///
    /// It starts in the standard variant until [`enter`](Self::enter)
    /// switches it.
    pub(super) fn new(input: &'a PlaceInput<'a>, block: BlockResult) -> Self {
        let stages = input.stages.variant(FirstLineVariant::Standard);
        let (content, analysis, measured) = (stages.content, stages.analysis, stages.measured);
        let style = content.block;
        Self {
            input,
            stages,
            clusters: &analysis.clusters,
            items: &content.items,
            item_clusters: &analysis.item_clusters,
            count: analysis.clusters.end_id(),
            boxes: &measured.kept_boxes,
            lines: input.lines,
            block,
            paragraph: ParagraphId::new(0),
            level: BidiLevel::LTR,
            text_align: style.text_align,
            text_align_last: style.text_align_last,
            has_box_edges: content.flags.contains(ContentFlags::BOXES_WITH_EDGES),
            has_mixed_levels: analysis.flags.contains(ParagraphFlags::MIXED_LEVELS),
            autospace: AutospaceRules::new(content),
            ends_on_a_line: analysis.paragraphs.ends_on_a_line(),
            item: ItemId::new(0),
            controls: analysis.control_levels(),
            run: stages.shaped.runs.cursor(ShapedRunId::new(0)),
            kept: KeptBoxId::new(0),
            shift_rows: None,
            has_tabs: analysis.flags.contains(ParagraphFlags::HAS_TABS),
            area: LineBand::from(input.area.inline),
            tab_line: TabLine::default(),
            justification: input.tab_justification,
            ellipsis_space: input.ellipsis_space,
            has_line_padding: content.flags.contains(ContentFlags::LINE_PADDING),
            group: Group::new(input),
            rubies: measured.ruby_columns(),
            column: RubyColumnId::new(0),
            line_column: RubyColumnId::new(0),
            ruby_pieces: &[],
            spread: None,
            hyphen: None,
            open_column: None,
            ruby_in_bidi: !measured.ruby_columns().is_empty()
                && analysis.flags.contains(ParagraphFlags::RIGHT_TO_LEFT),
            ruby_edges: RubyEdges::default(),
            has_emphasis: content.flags.contains(ContentFlags::EMPHASIS),
            has_upright: analysis.flags.contains(ParagraphFlags::HAS_UPRIGHT),
            has_anchors: !content.absolutes().is_empty(),
            has_decorated_boxes: content.flags.contains(ContentFlags::DECORATED_BOXES),
            open_boxes: 0,
            boxed: false,
        }
    }

    /// Switches to the tables of `variant` and restarts the cursors.
    ///
    /// The block's first line may use the first-line variant. Every other
    /// line uses the standard one. Nothing happens if `variant` is current.
    fn enter(&mut self, variant: FirstLineVariant) {
        if variant == self.stages.variant {
            return;
        }
        self.stages = self.input.stages.variant(variant);
        self.boxes = &self.stages.measured.kept_boxes;
        self.rubies = self.stages.measured.ruby_columns();
        self.run = self.stages.shaped.runs.cursor(ShapedRunId::new(0));
        self.kept = KeptBoxId::new(0);
        self.shift_rows = None;
        self.column = RubyColumnId::new(0);
        self.line_column = RubyColumnId::new(0);
    }

    /// Lays out every line in order and writes their items to `out`.
    pub(super) fn lines(&mut self, scratch: &mut PlaceScratch, out: &mut Fragments) {
        for ((id, line), &facts) in self
            .lines
            .lines
            .iter()
            .zip(self.input.placements.as_slice())
        {
            self.enter(line.variant());
            self.paragraph = line.paragraph;
            self.level = line.level(&self.stages.analysis.paragraphs);
            let line = LineView::new(id, line, facts);
            self.line(scratch, out, id, &line);
        }
        if self.has_anchors {
            self.place_unheld_anchors(out);
        }
    }

    /// Lays out `line` and writes its items to `out`, each phase with its
    /// part of `scratch`.
    fn line(
        &mut self,
        scratch: &mut PlaceScratch,
        out: &mut Fragments,
        id: LineId,
        line: &LineView<'_>,
    ) {
        let PlaceScratch {
            pieces,
            boxes,
            links,
            rubies,
        } = scratch;
        self.begin(pieces, boxes, rubies, line);
        let spread = out.spread_mark();
        // Alignment is decided first. A ruby column at the line's edges reads
        // it while the pieces are made, and justification uses it after.
        let (band, align) = self.alignment(line);
        // Pieces: made in logical order, each tab sized where it lands.
        self.open_carried_boxes(pieces, boxes, line);
        pieces.take_carried(boxes.innermost());
        let shapes = self.lines.edges.line_edges(line);
        self.pieces(pieces, boxes, rubies, line, shapes);
        self.finish_columns(pieces, boxes, rubies, line);
        boxes.carry_open();
        if self.has_line_padding {
            self.pad(pieces);
        }
        // Justification and alignment: justification spends the spare room
        // on the line's opportunities. Otherwise alignment moves the whole
        // line, tabs included, in the band `text-group-align` narrows.
        let justified = self.justify_into(pieces, line, id, band, align, out);
        let letter_indent = self.letter_indent(id);
        let offset = self.align(
            line,
            band,
            align,
            pieces.hang(),
            justified.is_some(),
            letter_indent,
        );
        // Reordering by level. A piece with no level of its own takes the
        // next piece's level, or the paragraph's at the line's end, as
        // Blink's opaque items do.
        pieces.reorder(self.level);
        // A spread ruby base's text takes in its room, which its boxes then
        // reach over.
        let spread_base = !rubies.columns.is_empty() && Placer::absorb_base_spread(pieces, rubies);
        // Box parts: found in visual order, with the room their edges
        // take.
        self.rooms(boxes, letter_indent);
        self.box_parts(pieces, boxes);
        // Positions: assigned left to right. Each box's edges sit at its
        // leftmost and rightmost parts.
        pieces.place(offset.start, &boxes.rooms);
        // Static positions: where the anchors stand, now that they are
        // placed.
        if self.has_anchors {
            self.place_anchors(pieces, out, line, offset);
        }
        // Truncation, on a line cut for an ellipsis: what doesn't fit before
        // the ellipsis is hidden but keeps its place.
        let cut = if line.flags.contains(LineFlags::ELLIPSIS) {
            self.truncate(pieces, line, band, &offset, justified.as_ref())
        } else {
            None
        };
        let split = cut.and_then(|cut| cut.split).map(|split| split.piece);
        links.count_leaves(pieces, split, self.hyphen_items(pieces, line));
        // Items: written once, then the annotation lines.
        self.emit(pieces, boxes, links, out, line, offset, cut);
        if spread_base {
            out.push_spread(id, pieces.logical.as_slice());
        }
        if !rubies.columns.is_empty() {
            self.annotations(pieces, rubies, out, line);
        }
        out.settle_spread(spread);
    }

    /// Empties the line's scratch and resets the placer's state for `line`.
    ///
    /// The boxes open from the line before stay open. The tab stops are
    /// measured from the line's start, and the ruby column cursor goes back
    /// to the line's first column.
    fn begin(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &mut LineBoxes,
        rubies: &mut LineRubies,
        line: &LineView<'_>,
    ) {
        pieces.logical.clear();
        boxes.begin_line();
        rubies.columns.clear();
        self.ruby_pieces = self
            .lines
            .ruby
            .as_deref()
            .map_or(&[], |ruby| ruby.line_pieces(line.id));
        self.spread = None;
        self.hyphen = None;
        self.open_column = None;
        self.boxed = self.open_boxes > 0;
        if self.has_tabs {
            self.tab_line = TabLine::new(
                line,
                &self.stages.analysis.paragraphs,
                self.area,
                &self.stages.measured.prefix,
            );
        }
        self.line_column = self.column;
        if let Some(first) = self.ruby_pieces.iter().min_by_key(|piece| piece.column) {
            self.column = self.column.min(first.column);
        }
    }

    /// Returns `line`'s band and alignment, and sets the ruby edges that
    /// read them.
    fn alignment(&mut self, line: &LineView<'_>) -> (LineBand, TextAlign) {
        let band = self.group.narrow(line.band);
        let ends_paragraph = line.ends_paragraph(&self.stages.analysis.paragraphs);
        let align = if ends_paragraph {
            self.text_align_last.resolve(self.text_align)
        } else {
            self.text_align
        };
        self.ruby_edges = if self.rubies.is_empty() || self.ruby_in_bidi {
            RubyEdges::default()
        } else {
            RubyEdges {
                start: align == TextAlign::Justify
                    || (ends_paragraph && self.text_align == TextAlign::Justify),
                end: align == TextAlign::Justify,
                room: band.width() - line.indent,
            }
        };
        (band, align)
    }

    /// Gives each box open from the line before a placeholder at `line`'s
    /// start, outermost first.
    ///
    /// A box keeps its row, unless the line before was set in the other
    /// variant, whose rows are another table's. Then the kept-box cursor,
    /// restarted for the variant, walks to it: the boxes carried are in
    /// node order, outermost first.
    fn open_carried_boxes(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &mut LineBoxes,
        line: &LineView<'_>,
    ) {
        let start = line.clusters().start;
        for at in 0..boxes.open.len() {
            work::step();
            if let Some(&(node, kept)) = boxes.open.get(at) {
                let kept = match kept {
                    Some(kept) if self.boxes.node(kept) == Some(node) => Some(kept),
                    _ => {
                        let at = self.boxes.walk_to(self.kept, node);
                        (self.boxes.node(at) == Some(node)).then_some(at)
                    }
                };
                let shifts = self.shift(line, node, boxes.innermost_shifts());
                boxes.open_box(pieces, (node, kept), start, false, shifts);
            }
        }
    }

    /// Justifies `line` where `align` is `justify`, and records it in `out`.
    ///
    /// Returns what justification gave the line, or `None` where it is not
    /// justified.
    fn justify_into(
        &self,
        pieces: &mut LinePieces,
        line: &LineView<'_>,
        id: LineId,
        band: LineBand,
        align: TextAlign,
        out: &mut Fragments,
    ) -> Option<Justified<'a>> {
        if align != TextAlign::Justify {
            return None;
        }
        let justified = self.justify(pieces, line, band)?;
        // Lines are laid out in order, so the table stays sorted.
        out.rare_mut().justified.push(super::JustifiedLine {
            line: id,
            content_end: line.content_end,
            justification: justified.justification,
        });
        Some(justified)
    }

    /// Returns the hyphen's piece on a hyphenated `line`, and how many items
    /// it writes, one per font of its generated text.
    fn hyphen_items(&self, pieces: &LinePieces, line: &LineView<'_>) -> Option<(PieceId, usize)> {
        if !line.flags.contains(LineFlags::HYPHENATED) {
            return None;
        }
        let text = self.hyphen?;
        let (at, _) = pieces
            .logical
            .iter()
            .rev()
            .find(|(_, piece)| piece.kind == PieceKind::Hyphen)?;
        Some((at, text.count()))
    }

    /// Returns `node`'s box facts in the current line's variant.
    #[inline]
    pub(super) fn box_facts(&self, node: NodeId) -> &'a BoxFacts {
        let content = self.stages.content;
        let id = content.nodes.box_facts(node, self.stages.variant);
        content.facts.box_facts(id)
    }

    /// Returns [`FragmentItemFlags::EMPHASIS`] where `node`'s text sets
    /// emphasis marks, else no flag.
    pub(super) fn emphasis(&self, node: NodeId) -> FragmentItemFlags {
        let facts = self.stages.content.facts.text(self.stages.text_facts(node));
        if facts.has(TextFlags::EMPHASIS) {
            FragmentItemFlags::EMPHASIS
        } else {
            FragmentItemFlags::NONE
        }
    }

    /// Returns the part of line `id`'s indent inside the initial letter's
    /// box: the first line's indent, where that line opens one.
    ///
    /// Fitting counts that line's indent twice, as Chrome lays the box out
    /// as a block whose own first line takes the indent too
    /// (`CalculateInitialLetterBoxInlineSize`). The box's start edge takes
    /// the second.
    fn letter_indent(&self, id: LineId) -> LayoutUnit {
        let indent = self.stages.content.block.text_indent;
        if id != LineId::new(0) || indent.hanging || self.stages.measured.initial_letter().is_none()
        {
            return LayoutUnit::ZERO;
        }
        indent.length(self.area.width())
    }

    /// Returns where `line` sits in `band` under `align`.
    ///
    /// `hang` is its hanging pieces' advance. `justified` says the
    /// opportunities already took the room. `letter_indent` is the part of
    /// its indent inside the initial letter's box, where the line starts.
    fn align(
        &self,
        line: &LineView<'_>,
        band: LineBand,
        align: TextAlign,
        hang: InlineLayoutUnit,
        justified: bool,
        letter_indent: LayoutUnit,
    ) -> LineOffset {
        let room = band.width();
        let content = line.indent + line.width;
        // Preserved white space before a forced break or at the block's end
        // hangs only where it overflows, so alignment sees the part that
        // fits. What hangs before it, an ideographic space say, hangs only
        // where none of it fits, as nothing after it then takes room.
        // Elsewhere hanging content is left out. A justified line leaves all
        // of it out, as Chrome hangs trailing spaces under `justify`
        // (`ShouldHangTrailingSpaces`).
        let counted = if line.flags.contains(LineFlags::CONDITIONAL_HANG) && !justified {
            let before = line.hang.unconditional;
            let fits = (line.hang.space - before)
                .min(room - content - before)
                .max(LayoutUnit::ZERO);
            if fits > LayoutUnit::ZERO {
                before + fits
            } else {
                LayoutUnit::ZERO
            }
        } else {
            LayoutUnit::ZERO
        };
        let rtl = self.level.is_rtl();
        // A justified line fills its band, since its opportunities took the
        // room. Otherwise alignment places the indent, the content, and the
        // fitting part of a conditional hang.
        let left = if justified {
            band.left
        } else {
            let side = Side::from_align(align, rtl);
            band.left + side.left_offset(rtl, room - (content + counted))
        };
        // Right to left, content hanging at the end sits at the line's left,
        // outside the line box except the part alignment counted. Left to
        // right, a mark hanging at the start sits outside it on the left.
        let hang_end = InlineLayoutUnit::from_layout(line.hang.end);
        let start = if rtl {
            -(hang + hang_end - InlineLayoutUnit::from_layout(counted)).max(InlineLayoutUnit::ZERO)
        } else {
            InlineLayoutUnit::from_layout(line.indent - letter_indent - line.hang.start)
        };
        LineOffset { left, start }
    }

    /// Justifies `line` in `band`, giving each of `pieces` its share of the
    /// spare room.
    ///
    /// Returns the amounts and which clusters have opportunities. Returns
    /// `None` where the line has no room or no opportunity; it is then set at
    /// its start, as in Chrome.
    ///
    /// The shares add up to the room exactly, as in Chrome. A tab's width
    /// with its share rounds up onto the grid, as Chrome snaps a tab, so its
    /// size stays its advance. Tabs take no share where the config keeps them
    /// at their stops.
    fn justify(
        &self,
        pieces: &mut LinePieces,
        line: &LineView<'_>,
        band: LineBand,
    ) -> Option<Justified<'a>> {
        let extra = band.width() - (line.indent + line.width);
        if extra <= LayoutUnit::ZERO {
            return None;
        }
        let opportunities = JustifyOpportunities::from_line(
            &self.stages,
            line.clusters().start..line.content_end,
            None,
        )?;
        let justify_tabs = self.justification == TabJustification::Stretch;
        let mut count = 0u32;
        for piece in pieces.logical.as_mut_slice() {
            // A tab that doesn't hang takes its opportunity's room only where
            // the config widens tabs.
            let tab = piece.kind == PieceKind::Text
                && piece.flags.contains(FragmentItemFlags::TAB)
                && !piece.flags.contains(FragmentItemFlags::HANGS);
            piece.opportunities = if piece.is_text() {
                opportunities.count_range(piece.start..piece.end)
            } else if tab && justify_tabs {
                opportunities.count(piece.start)
            } else {
                0
            };
            count = count.saturating_add(piece.opportunities);
        }
        let (holder, last) = last_opportunity(&pieces.logical, &opportunities)?;
        let justification = LineJustification::new(
            InlineLayoutUnit::from_layout(extra),
            count,
            last,
            opportunities.summary(),
        )?;
        let ids = (PieceId::new(0)..pieces.logical.next_id()).ids();
        for (at, piece) in ids.zip(pieces.logical.as_mut_slice()) {
            if piece.opportunities == 0 {
                continue;
            }
            piece.advance += justification.share(piece.opportunities, at == holder);
            if piece.flags.contains(FragmentItemFlags::TAB) {
                piece.advance = piece.advance.ceil_to_grid();
            }
        }
        Some(Justified {
            justification,
            opportunities,
        })
    }

    /// Adds `line-padding` pieces inside the innermost box at each end of the
    /// line's content, at the paragraph's level.
    ///
    /// Each end uses the padding of the text holding its first or last leaf
    /// before what hangs. That is what the breaker charged.
    fn pad(&self, pieces: &mut LinePieces) {
        let is_content = |piece: &Piece| {
            matches!(
                piece.kind,
                PieceKind::Text | PieceKind::Atomic | PieceKind::Hyphen
            ) && !piece.flags.contains(FragmentItemFlags::HANGS)
        };
        let slice = pieces.logical.as_slice();
        let (Some(first), Some(last)) = (
            slice.iter().position(is_content).map(PieceId::new),
            slice.iter().rposition(is_content).map(PieceId::new),
        ) else {
            return;
        };
        let facts = &self.stages.content.facts;
        let padding = |piece: &Piece| {
            let text = facts.text(self.stages.text_facts(piece.node));
            InlineLayoutUnit::from_layout(text.padding)
        };
        let (Some(head), Some(tail)) = (pieces.logical.get(first), pieces.logical.get(last)) else {
            return;
        };
        let pad = |of: &Piece, at: ClusterId, advance: InlineLayoutUnit| {
            Piece::new(
                PieceKind::Padding,
                of.node,
                self.level,
                at..at,
                advance,
                of.owner,
                of.shift,
            )
        };
        let end = pad(tail, tail.end, padding(tail));
        let start = pad(head, head.start, padding(head));
        if end.advance != InlineLayoutUnit::ZERO {
            pieces.insert(PieceId::new(last.get() + 1), end);
        }
        if start.advance != InlineLayoutUnit::ZERO {
            pieces.insert(first, start);
        }
    }

    /// Writes `line`'s items to `out`: its `Line` item, then its pieces'
    /// items in visual order.
    ///
    /// Each box part's item comes before the items inside it. `links`
    /// counts the leaf items and links the box items in paint order.
    #[allow(clippy::too_many_arguments)]
    fn emit(
        &self,
        pieces: &LinePieces,
        boxes: &mut LineBoxes,
        links: &mut PaintLinks,
        out: &mut Fragments,
        line: &LineView<'_>,
        offset: LineOffset,
        cut: Option<LineCut>,
    ) {
        let baseline = line.ascent();
        let head = out.items.next_id();
        let ellipsis = cut.map_or(0, |cut| cut.ellipsis.items());
        let total = links.total() + boxes.parts.len() + ellipsis;
        let top = self.block.top(line.block_start);
        // The ellipsis belongs to the line, in no box. It is the first item
        // when drawn at the left (right to left) and the last when drawn at
        // the right, so items stay in visual order. The box items link past
        // it, the first from the line's item.
        let rtl = self.level.is_rtl();
        let leading = cut.filter(|_| rtl);
        let before = FragmentItemId::new(head.get() + if rtl { ellipsis } else { 0 });
        let first = paint_order(links, boxes, before);
        let flags = if self.has_decorated_boxes && self.boxed {
            FragmentItemFlags::BOXED
        } else {
            FragmentItemFlags::NONE
        };
        let pushed = out.items.push(FragmentItem::from_line(
            self.level,
            offset.left,
            top,
            total,
            first,
            flags,
        ));
        if pushed.is_none() || out.line_heads.push(head).is_none() {
            debug_assert!(false, "no more items than a FragmentItemId names");
            return;
        }
        if let Some(cut) = leading {
            self.emit_ellipsis(out, cut.ellipsis, baseline);
        }
        let split = cut.and_then(|cut| cut.split);
        let mut part = BoxPartId::new(0);
        for (at, &logical) in pieces.order.iter() {
            work::step();
            while let Some(next) = boxes.parts.get(part).filter(|next| next.left == at) {
                self.emit_box_part(pieces, boxes, links, out, part, next, baseline);
                part = BoxPartId::new(part.get() + 1);
            }
            let Some(piece) = pieces.logical.get(logical) else {
                continue;
            };
            let Some(kind) = piece.kind.fragment() else {
                continue;
            };
            let flags = if self.has_emphasis && kind == FragmentItemKind::Text {
                piece.flags.union(self.emphasis(piece.node))
            } else {
                piece.flags
            };
            if let Some(split) = split.filter(|split| split.piece == logical) {
                self.emit_split(out, piece, kind, flags, split, baseline);
            } else if piece.kind == PieceKind::Hyphen {
                self.emit_hyphen(out, piece, flags, baseline);
            } else if out.items.push(piece.item(kind, flags, baseline)).is_none() {
                debug_assert!(false, "no more items than a FragmentItemId names");
                return;
            }
        }
        if !rtl && let Some(cut) = cut {
            self.emit_ellipsis(out, cut.ellipsis, baseline);
        }
    }
}

/// Returns the justification opportunity Chrome fills last, which takes the
/// leftover: its piece and its cluster. `None` where no piece has one.
///
/// Chrome spends the room item by item in logical order (`JustifyResults`).
/// Within an item it goes in glyph order
/// (`ShapeResult::ApplySpacingOrExpansion`), which is visual.
///
/// So the last is in the logically last piece with an opportunity. Left to
/// right, it is that piece's last. Right to left, it is the first of the
/// whole item: Chrome's shape result spans pieces of the same node and level
/// that only a font change divides. A tab is an item of its own, as in
/// Chrome.
fn last_opportunity(
    pieces: &Table<PieceId, Piece>,
    opportunities: &JustifyOpportunities<'_>,
) -> Option<(PieceId, ClusterId)> {
    let (holder, piece) = pieces
        .iter()
        .rev()
        .find(|(_, piece)| piece.opportunities > 0)?;
    if !piece.is_text() || !piece.level.is_rtl() {
        let last = opportunities.last(piece.start..piece.end)?;
        return Some((holder, last));
    }
    let mut first = holder;
    while let Some(at) = first.get().checked_sub(1).map(PieceId::new)
        && let (Some(before), Some(after)) = (pieces.get(at), pieces.get(first))
        && before.is_text()
        && before.node == piece.node
        && before.level == piece.level
        && before.end == after.start
    {
        first = at;
    }
    let (at, piece) = (first..PieceId::new(holder.get() + 1))
        .ids()
        .filter_map(|at| pieces.get(at).map(|piece| (at, piece)))
        .find(|(_, piece)| piece.opportunities > 0)?;
    let last = opportunities.first(piece.start..piece.end)?;
    Some((at, last))
}
