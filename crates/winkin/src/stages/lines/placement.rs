//! Facts breaking hands to line layout's placement.
//!
//! They live in context scratch, not in every layout. A warm relayout reuses
//! the table's capacity.

use core::ops::{Deref, DerefMut};

use super::{LineId, LineRecord};
use crate::data::Table;
use crate::stages::analysis::ClusterId;
use crate::unit::LayoutUnit;

/// The two line facts placement needs until it writes the line's items.
#[derive(Copy, Clone, Debug)]
pub(crate) struct PlacementFacts {
    pub(super) content_end: ClusterId,
    pub(super) block_start: LayoutUnit,
}

pub(crate) type LinePlacements = Table<LineId, PlacementFacts>;

/// A fitted line before it is kept: its kept record and its placement facts.
pub(super) struct PendingLine {
    pub(super) record: LineRecord,
    pub(super) facts: PlacementFacts,
}

impl Deref for PendingLine {
    type Target = LineRecord;
    fn deref(&self) -> &LineRecord {
        &self.record
    }
}
impl DerefMut for PendingLine {
    fn deref_mut(&mut self) -> &mut LineRecord {
        &mut self.record
    }
}

/// A line as placement reads it, borrowing the record without copying it.
pub(crate) struct LineView<'a> {
    pub(crate) id: LineId,
    record: &'a LineRecord,
    pub(crate) content_end: ClusterId,
    pub(crate) block_start: LayoutUnit,
}
impl<'a> LineView<'a> {
    pub(crate) fn new(id: LineId, record: &'a LineRecord, facts: PlacementFacts) -> Self {
        Self {
            id,
            record,
            content_end: facts.content_end,
            block_start: facts.block_start,
        }
    }
}
impl Deref for LineView<'_> {
    type Target = LineRecord;
    fn deref(&self) -> &LineRecord {
        self.record
    }
}
