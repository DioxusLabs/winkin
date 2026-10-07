//! A line's kept boxes: their box parts in visual order, the room their
//! edges take, and the links that set their paint order.
//!
//! This follows Chrome's `InlineLayoutStateStack`. A box's parts are the
//! runs of its pieces, its placeholder and its descendants', that
//! reordering leaves side by side (`UpdateBoxDataFragmentRange`).
//!
//! Its edges are physical (`AddBoxData`, `UpdateFragmentEdges`). The left
//! margin, border and padding sit at the leftmost part's left, the right
//! ones at the rightmost part's right. The start side is the left for a
//! left-to-right box and the right otherwise. A box opening on a line takes
//! its start edge there, and one closing takes its end edge. A cloned box
//! (`box-decoration-break: clone`) has both on every line.

use alloc::vec::Vec;
use core::iter;

use super::place::{
    BoxPart, BoxPartId, LineBox, LineBoxId, LineBoxes, LinePieces, PieceId, VisualId,
};
use super::{FragmentItem, FragmentItemFlags, FragmentItemId, Fragments, Placer};
use crate::data::IdRange;
use crate::data::{Id, Table, heap_bytes, index_to_u32, sort_by_key, u32_to_index};
use crate::stages::analysis::BidiLevel;
use crate::stages::content::BoxFlags;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// Where each box part's item goes among the line's items, and their
/// paint order.
pub(super) struct PaintLinks {
    /// How many items the pieces before each visual place write, with the
    /// total after the last.
    leaves: Table<VisualId, u32>,
    /// The box parts in paint order: by box in tree order, then by
    /// where they start. Used only where reordering left the items in
    /// another order.
    painted: Vec<BoxPartId>,
}

impl PaintLinks {
    /// Makes empty links, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            leaves: Table::new(),
            painted: Vec::new(),
        }
    }

    /// Counts the items `pieces` write before each visual place, and in all.
    ///
    /// A leaf writes one. The piece `split` a cut divides writes two. The
    /// line's `hyphen` writes one per font of its generated text.
    pub(super) fn count_leaves(
        &mut self,
        pieces: &LinePieces,
        split: Option<PieceId>,
        hyphen: Option<(PieceId, usize)>,
    ) {
        self.leaves.clear();
        let mut leaves = 0u32;
        let mut counted = self.leaves.push(0).is_some();
        for &logical in pieces.order.as_slice() {
            if pieces
                .logical
                .get(logical)
                .is_some_and(|piece| piece.kind.is_leaf())
            {
                leaves += 1;
            }
            counted &= self.leaves.push(leaves).is_some();
        }
        debug_assert!(counted, "no more pieces on a line than a VisualId names");
        if let Some(split) = split {
            self.widen(pieces, split, 1);
        }
        if let Some((hyphen, items)) = hyphen
            && items > 1
        {
            self.widen(pieces, hyphen, items - 1);
        }
    }

    /// Adds `more` items for the leaf `piece` to every count past its visual
    /// place.
    ///
    /// The extra items are a cut piece's second part, or a multi-font
    /// hyphen's later parts. Kept out of line, since few lines need it.
    #[inline(never)]
    fn widen(&mut self, pieces: &LinePieces, piece: PieceId, more: usize) {
        let Some((at, _)) = pieces.order.iter().find(|&(_, &logical)| logical == piece) else {
            return;
        };
        let more = index_to_u32(more);
        let past = VisualId::new(at.get() + 1)..self.leaves.next_id();
        for leaves in self.leaves.get_slice_mut(past).unwrap_or_default() {
            work::step();
            *leaves = leaves.saturating_add(more);
        }
    }

    /// Returns how many items the line's pieces write in all.
    #[inline]
    pub(super) fn total(&self) -> usize {
        self.leaves.last().copied().map_or(0, u32_to_index)
    }
}

heap_bytes! {
    PaintLinks { leaves, painted }
}

impl<'a> Placer<'a> {
    /// Finds the box parts of the line's kept boxes in visual order, and
    /// where each box's edges go.
    ///
    /// A box's left edge room goes before its leftmost part, and its right
    /// edge room after its rightmost.
    ///
    /// A piece belongs to its owner and every box around it. Walking in
    /// visual order, boxes the piece is in and the previous piece is not
    /// start a part. Boxes the previous piece is in and this one is not
    /// end one.
    ///
    /// **Linear in the pieces and the parts**, however deep the boxes
    /// nest. Each walk step starts or ends a part. Parts starting or
    /// ending at one piece do so innermost first. So a running sum of the
    /// edges met at that piece gives the room deeper boxes take inside the
    /// next.
    pub(super) fn box_parts(&self, pieces: &LinePieces, boxes: &mut LineBoxes) {
        let end = pieces.order.next_id();
        boxes.rooms.clear();
        // With no box, no piece has room beside it, and positions read none.
        if boxes.boxes.is_empty() {
            return;
        }
        let none = (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
        let count = pieces.order.len();
        if boxes.rooms.extend(iter::repeat_n(none, count)).is_none() {
            debug_assert!(false, "no more pieces on a line than a VisualId names");
        }
        let LineBoxes {
            boxes,
            parts,
            ended,
            rooms,
            ..
        } = boxes;
        let mut previous: Option<LineBoxId> = None;
        for at in (VisualId::new(0)..VisualId::new(end.get() + 1)).ids() {
            let owner = if at < end {
                pieces.visual(at).and_then(|piece| piece.owner)
            } else {
                None
            };
            // Walk both box chains up to the innermost box they share. End
            // the parts only the previous piece is in, and start those
            // only this one is in. The guard bounds the walk to a step per
            // box, whatever the links say.
            let (mut a, mut b) = (previous, owner);
            let mut guard = 2 * boxes.len() + 2;
            // The left edges of boxes whose leftmost part starts here,
            // summed innermost first. It is the room deeper boxes take inside
            // each outer part starting here.
            let mut inside = InlineLayoutUnit::ZERO;
            while a != b && guard > 0 {
                work::step();
                guard -= 1;
                if depth(boxes, a) >= depth(boxes, b) {
                    let Some(open) = a.and_then(|index| boxes.get_mut(index)) else {
                        break;
                    };
                    if let Some(active) = open.active.take()
                        && let Some(part) = parts.get_mut(active)
                    {
                        part.right = VisualId::new(at.get().saturating_sub(1));
                        open.last_right = Some(part.right);
                        if self.has_box_edges {
                            ended.push(active);
                        }
                    }
                    a = open.parent;
                } else {
                    let Some(index) = b else {
                        break;
                    };
                    let Some(open) = boxes.get_mut(index) else {
                        break;
                    };
                    open.active = parts.push(BoxPart {
                        line_box: index,
                        left: at,
                        right: at,
                        inner_left: inside,
                        inner_right: InlineLayoutUnit::ZERO,
                        next: None,
                    });
                    if open.first_left.is_none() {
                        open.first_left = Some(at);
                        inside += open.left_room;
                    }
                    b = open.parent;
                }
            }
            previous = owner;
        }
        // Each part's inner right: the right edges of deeper boxes whose
        // rightmost part ends where it does. Every box's rightmost is
        // known now. Parts ending at one piece were ended together,
        // innermost first. Without box edges, `ended` is empty.
        let mut inside = InlineLayoutUnit::ZERO;
        let mut at = None;
        for &index in ended.iter() {
            work::step();
            let Some(part) = parts.get_mut(index) else {
                continue;
            };
            if at != Some(part.right) {
                at = Some(part.right);
                inside = InlineLayoutUnit::ZERO;
            }
            part.inner_right = inside;
            if let Some(open) = boxes.get(part.line_box)
                && open.last_right == Some(part.right)
            {
                inside += open.right_room;
            }
        }
        // Each box's edges go at its leftmost and rightmost parts.
        for open in boxes.as_slice() {
            if let Some((before, _)) = open.first_left.and_then(|at| rooms.get_mut(at)) {
                *before += open.left_room;
            }
            if let Some((_, after)) = open.last_right.and_then(|at| rooms.get_mut(at)) {
                *after += open.right_room;
            }
        }
        // Put them in item order: by where they start, outermost first. The
        // walk started each piece's parts innermost first, so reversing
        // each run gives that order without a sort.
        let mut from = BoxPartId::new(0);
        while let Some(first) = parts.get(from) {
            let left = first.left;
            let mut to = BoxPartId::new(from.get() + 1);
            while parts.get(to).is_some_and(|next| next.left == left) {
                to = BoxPartId::new(to.get() + 1);
            }
            if let Some(run) = parts.get_slice_mut(from..to) {
                run.reverse();
            }
            from = to;
        }
        debug_assert!(
            parts.as_slice().windows(2).all(|pair| matches!(
                pair,
                [a, b] if (a.left, depth(boxes, Some(a.line_box))) < (b.left, depth(boxes, Some(b.line_box)))
            )),
            "the parts go in the order their items do"
        );
    }

    /// Works out which sides of each kept box are its own edges on the line,
    /// and the room each takes.
    ///
    /// A box has its start edge where it opens and its end edge where it
    /// closes; a cloned box has both on every line. The start is on the left
    /// for left-to-right and on the right otherwise, as Chrome's `AddBoxData`
    /// swaps them.
    ///
    /// Each side's room is the box facts' room on that side
    /// (`EdgesGroup::inline`). It matches what the prefix sums, or the
    /// breaker for a clone, charged for the edge. The initial letter's start
    /// takes `letter_indent` more, the indent inside its box, which the
    /// line's indent holds.
    pub(super) fn rooms(&self, boxes: &mut LineBoxes, letter_indent: LayoutUnit) {
        for open in boxes.boxes.as_mut_slice() {
            work::step();
            let facts = self.box_facts(open.node);
            let direction = facts.direction();
            let clones = facts.has(BoxFlags::CLONES);
            let (starts, ends) = (open.opens || clones, open.closes || clones);
            (open.has_left, open.has_right) = direction.line_order(starts, ends);
            if !self.has_box_edges {
                continue;
            }
            let exact = |room: LayoutUnit| InlineLayoutUnit::from_layout(room);
            // An initial letter's edges take what measurement gave them,
            // which the prefix sums hold.
            let letter = self
                .stages
                .measured
                .initial_letter()
                .filter(|letter| letter.node == open.node);
            let ((left, right), (margin_left, margin_right)) = match letter {
                Some(letter) => {
                    let (start, end) = (letter.start_margin, letter.end_margin);
                    let order = |start, end| direction.line_order(start, end);
                    let opens = letter.edge_room(true) + exact(letter_indent);
                    (
                        order(opens, letter.edge_room(false)),
                        order(exact(start), exact(end)),
                    )
                }
                None => {
                    let ((left, right), (margin_left, margin_right)) =
                        (facts.room, facts.margin_line);
                    (
                        (exact(left), exact(right)),
                        (exact(margin_left), exact(margin_right)),
                    )
                }
            };
            if open.has_left {
                open.left_room = left;
                open.left_margin = margin_left;
            }
            if open.has_right {
                open.right_room = right;
                open.right_margin = margin_right;
            }
        }
    }

    /// Writes the item of box part `index`, `part`: its border box,
    /// heading the items `links` counts inside it.
    ///
    /// A side with the box's own edge reaches to the edge, inside its margin.
    /// An open side reaches to the pieces it holds, around inner boxes'
    /// edges. This is how Chrome's `ComputeInlinePositions` places a box.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn emit_box_part(
        &self,
        pieces: &LinePieces,
        boxes: &LineBoxes,
        links: &PaintLinks,
        out: &mut Fragments,
        index: BoxPartId,
        part: &BoxPart,
        baseline: LayoutUnit,
    ) {
        let (Some(leftmost), Some(rightmost), Some(own)) = (
            pieces.visual(part.left),
            pieces.visual(part.right),
            boxes.boxes.get(part.line_box),
        ) else {
            return;
        };
        // Whether it is its box's leftmost part, and whether its rightmost.
        let first = own.first_left == Some(part.left);
        let last = own.last_right == Some(part.right);
        // Inner boxes' edges sit between its border and its content, as the
        // walk summed them. A deeper box whose part starts at the same
        // piece is inside this one.
        let mut flags = FragmentItemFlags::NONE;
        let mut left = leftmost.inline - part.inner_left;
        if first && own.has_left {
            left = left - (own.left_room - own.left_margin);
        } else {
            flags.insert(FragmentItemFlags::OPEN_LEFT);
        }
        let mut right = rightmost.inline + rightmost.advance + part.inner_right;
        if last && own.has_right {
            right += own.right_room - own.right_margin;
        } else {
            flags.insert(FragmentItemFlags::OPEN_RIGHT);
        }
        // Its items: the leaves among its pieces, and the nested parts,
        // which follow it in the list and start inside it.
        let leaves = links
            .leaves
            .get(VisualId::new(part.right.get() + 1))
            .zip(links.leaves.get(part.left))
            .map_or(0, |(to, from)| u32_to_index(to.saturating_sub(*from)));
        let nested = boxes
            .parts
            .get_slice(BoxPartId::new(index.get() + 1)..boxes.parts.next_id())
            .unwrap_or_default()
            .partition_point(|inner| inner.left <= part.right);
        let level_at = |at: VisualId| {
            pieces
                .order
                .get(at)
                .and_then(|&logical| pieces.levels.get(logical))
                .copied()
                .unwrap_or(BidiLevel::LTR)
        };
        let level = level_at(part.left).min(level_at(part.right));
        let item = FragmentItem::from_box_part(
            own.node,
            level,
            left,
            (right - left).to_layout().max(LayoutUnit::ZERO),
            baseline - own.shift,
            flags,
            leaves + nested,
            part.next,
        );
        let item = match own.kept {
            Some(kept) => item.with_kept_box(kept),
            None => item,
        };
        out.items
            .push_bounded(item, "no more items than a FragmentItemId names");
    }
}

/// Links each box part to the one painted after it, and returns the
/// first.
///
/// Paint order is by box in tree order, outermost first whatever reordering
/// did, then each box's parts left to right. `links` is the sort
/// scratch.
///
/// Each link is the id the next part's item will have. It counts from
/// `head`, the item just before the pieces' items: the line's own, or a
/// leading ellipsis on a right-to-left line. The leaves before the part
/// and the parts before it in item order come first.
///
/// The returned first link goes in the line's item; `None` where the line
/// has no part. A painter follows the links instead of searching, so
/// painting a line is linear in its items.
pub(super) fn paint_order(
    links: &mut PaintLinks,
    boxes: &mut LineBoxes,
    head: FragmentItemId,
) -> Option<FragmentItemId> {
    let PaintLinks { leaves, painted } = links;
    let LineBoxes { boxes, parts, .. } = boxes;
    let node = |part: &BoxPart| boxes.get(part.line_box).map(|open| open.node);
    // The id the item of part `index` will have.
    let item = |parts: &Table<BoxPartId, BoxPart>, index: BoxPartId| {
        let before = parts
            .get(index)
            .and_then(|part| leaves.get(part.left))
            .map_or(0, |&leaves| u32_to_index(leaves));
        FragmentItemId::new(head.get() + before + index.get() + 1)
    };
    // On a line that is not reordered, as on most lines, the items are in
    // tree order already, so each paints after the one before and nothing
    // is sorted. The links are written while the order is checked.
    let mut ordered = true;
    for index in (BoxPartId::new(0)..parts.next_id()).ids() {
        work::step();
        let after = BoxPartId::new(index.get() + 1);
        let link = match (parts.get(index), parts.get(after)) {
            (Some(this), Some(next)) if node(this) > node(next) => {
                ordered = false;
                break;
            }
            (Some(_), Some(_)) => Some(item(parts, after)),
            _ => None,
        };
        if let Some(part) = parts.get_mut(index) {
            part.next = link;
        }
    }
    let first = BoxPartId::new(0);
    if ordered {
        return parts.get(first).map(|_| item(parts, first));
    }
    painted.clear();
    painted.extend(parts.ids());
    sort_by_key(painted, |&index| {
        parts.get(index).map(|part| (node(part), part.left))
    });
    for pair in painted.windows(2) {
        work::step();
        if let [this, next] = *pair {
            let link = item(parts, next);
            if let Some(part) = parts.get_mut(this) {
                part.next = Some(link);
            }
        }
    }
    if let Some(&last) = painted.last()
        && let Some(part) = parts.get_mut(last)
    {
        part.next = None;
    }
    painted.first().map(|&first| item(parts, first))
}

/// Returns how deep the box `id` is, counting from one; zero for no box.
fn depth(boxes: &Table<LineBoxId, LineBox>, id: Option<LineBoxId>) -> u32 {
    id.and_then(|id| boxes.get(id))
        .map_or(0, |open| open.depth + 1)
}
