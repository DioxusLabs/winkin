//! Stored items measurements.

use crate::data::{Id, Table, find_sorted};
use crate::stages::content::{ItemId, NodeId};
use crate::work;

use super::{EmBoxId, Extent, KeptBoxId};

/// How far each item reaches either side of its baseline, and the strut.
///
/// A line box's extent is the union of the strut and its items' extents.
/// Each item takes 8 bytes.
pub(crate) struct ItemExtents {
    pub(super) extents: Table<ItemId, Extent>,
    /// The strut: the block's own font and line height, which every line
    /// box starts from.
    pub(crate) strut: Extent,
}

impl ItemExtents {
    pub(super) const fn new() -> Self {
        Self {
            extents: Table::new(),
            strut: Extent::NONE,
        }
    }

    pub(super) fn clear(&mut self) {
        self.extents.clear();
        self.strut = Extent::NONE;
    }

    /// Makes room for `count` items' extents.
    pub(super) fn reserve(&mut self, count: usize) {
        self.extents.reserve(count);
    }

    /// Gives the next item `extent`.
    pub(super) fn push(&mut self, extent: Extent) {
        self.extents
            .push_bounded(extent, "an extent an item, which an ItemId names");
    }

    /// Gives `item` `extent` again, where it has one.
    pub(super) fn set(&mut self, item: ItemId, extent: Extent) {
        if let Some(kept) = self.extents.get_mut(item) {
            *kept = extent;
        }
    }

    /// How far `item` reaches either side of its baseline, or
    /// [`Extent::NONE`] past the last.
    #[inline]
    pub(crate) fn get(&self, item: ItemId) -> Extent {
        self.extents.get(item).copied().unwrap_or(Extent::NONE)
    }

    /// How many items have an extent, which the tests check is every one.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.extents.len()
    }
}

/// The em box of each text item's text, as [`em_box`](super::ruby::em_box)
/// finds it.
///
/// A line reckons the room for its ruby annotations and emphasis marks from
/// these boxes. They depend on the text and its fonts, not the width, so they
/// are worked out once here rather than for each line on every relayout.
///
/// The table is kept only where the content has ruby or emphasis marks, and
/// only for items whose fonts all share one em box. A line holding part of
/// an item drawn in two fonts asks [`em_box`](super::ruby::em_box) of its
/// part. Items share a few distinct boxes, one per font and size, so each
/// item names its box in a table of them: a byte per item and 8 per box.
pub(crate) struct ItemEmBoxes {
    /// Each item's em box in `boxes`.
    pub(super) items: Table<ItemId, EmBoxId>,
    /// The distinct em boxes, the first [`Extent::NONE`], which an item
    /// keeping none names and no text's em box is.
    pub(super) boxes: Table<EmBoxId, Extent>,
}

impl ItemEmBoxes {
    pub(super) const fn new() -> Self {
        Self {
            items: Table::new(),
            boxes: Table::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.items.clear();
        self.boxes.clear();
    }

    /// Gives the next item `em`, or none: none too where the text has more
    /// distinct em boxes than an `EmBoxId` names, which a reader then asks
    /// [`em_box`](super::ruby::em_box) of.
    pub(super) fn push(&mut self, em: Option<Extent>) {
        let boxes = self.boxes.as_slice();
        let found = em.and_then(|em| boxes.iter().rposition(|&kept| kept == em));
        let id = match (em, found) {
            (_, Some(at)) => EmBoxId::new(at),
            (Some(em), None) => self.boxes.push(em).unwrap_or(EmBoxId::NONE),
            (None, None) => EmBoxId::NONE,
        };
        self.items
            .push_bounded(id, "an em box an item, which an ItemId names");
    }

    /// Starts the table with none, which the items keeping none name.
    pub(super) fn start(&mut self) {
        if self.boxes.push(Extent::NONE) != Some(EmBoxId::NONE) {
            debug_assert!(false, "a table filled once a build, from empty");
        }
    }

    /// The em box of whatever of `item`'s text a line holds, where it is
    /// kept: `None` for an item that is not text, whose fonts' em boxes
    /// differ, of content with neither ruby nor marks, or past the last.
    #[inline]
    pub(crate) fn get(&self, item: ItemId) -> Option<Extent> {
        let id = *self.items.get(item)?;
        self.boxes.get(id).copied().filter(|em| !em.is_none())
    }
}

/// The inline boxes that keep a fragment of their own, each with its extent
/// around its baseline.
///
/// The extent is the primary font's ascent and descent plus the padding and
/// border across the line. Boxes are in node order, which is the order they
/// open in. A box with no entry is culled.
pub(crate) struct KeptBoxes {
    pub(super) boxes: Table<KeptBoxId, (NodeId, Extent)>,
}

impl KeptBoxes {
    pub(super) const fn new() -> Self {
        Self {
            boxes: Table::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.boxes.clear();
    }

    /// The extent of `node`'s box fragment around its baseline, by halving,
    /// or `None` where the box is culled, or `node` is no inline box.
    pub(crate) fn get(&self, node: NodeId) -> Option<Extent> {
        find_sorted(self.boxes.as_slice(), node).map(|&(_, extent)| extent)
    }

    /// The extent of kept box `at` around its baseline, or `None` past the
    /// last.
    #[inline]
    pub(crate) fn extent(&self, at: KeptBoxId) -> Option<Extent> {
        self.boxes.get(at).map(|&(_, extent)| extent)
    }

    /// The first kept box at or after `node`, by halving: one past the last
    /// where none is.
    ///
    /// A walk over boxes seeks this once, then moves with
    /// [`walk_to`](Self::walk_to).
    pub(crate) fn first_from(&self, node: NodeId) -> KeptBoxId {
        work::seek();
        let boxes = self.boxes.as_slice();
        KeptBoxId::new(boxes.partition_point(|&(kept, _)| kept < node))
    }

    /// The first kept box at or after `node`, walked to from `from` a row at
    /// a time, forward or back.
    ///
    /// A reader meeting boxes near one another, as line layout opens them
    /// in node order, passes each row once. Box `node` is kept where the
    /// row found is its own.
    #[inline]
    pub(crate) fn walk_to(&self, from: KeptBoxId, node: NodeId) -> KeptBoxId {
        let mut at = from;
        while self.node(at).is_some_and(|kept| kept < node) {
            work::step();
            at = KeptBoxId::new(at.get() + 1);
        }
        while let Some(before) = at.get().checked_sub(1).map(KeptBoxId::new)
            && self.node(before).is_some_and(|kept| kept >= node)
        {
            work::step();
            at = before;
        }
        at
    }

    /// The node of kept box `at`, or `None` past the last.
    #[inline]
    pub(crate) fn node(&self, at: KeptBoxId) -> Option<NodeId> {
        self.boxes.get(at).map(|&(node, _)| node)
    }

    /// Every kept box's node and extent, in node order, which the tests
    /// check.
    #[cfg(test)]
    pub(super) fn iter(&self) -> impl ExactSizeIterator<Item = (NodeId, Extent)> {
        self.boxes.as_slice().iter().copied()
    }
}
