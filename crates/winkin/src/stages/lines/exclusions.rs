//! What a host answers for a layout's lines: the area they are broken in,
//! and the floats they flow around.
//!
//! Every position here is line-relative, as every position a layout
//! reports is. The inline axis runs along a line from its line-left end.
//! The block axis runs across the lines from the block-start edge. The
//! block's writing mode maps both onto the page. Lengths are `f32` pixels,
//! and the breaker rounds each onto the 1/64 px layout grid as it takes it.

use crate::build::FloatSide;
use crate::stages::content::NodeKey;

/// Extents along a line, from line-left to line-right.
///
/// For vertical writing, these are column top and bottom. Exclusion bands
/// and floats are relative to area line-left. Runs, boxes, carets and
/// selection rectangles are relative to line-box left.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct InlineExtents {
    /// Its line-left end.
    pub left: f32,
    /// Its line-right end.
    pub right: f32,
}

impl InlineExtents {
    /// Unbounded inline space, clipped to [`Area`] during line breaking.
    pub const EVERYTHING: Self = Self {
        left: f32::NEG_INFINITY,
        right: f32::INFINITY,
    };

    /// Returns the inline size, or zero if the endpoints are reversed.
    pub fn size(&self) -> f32 {
        let size = self.right - self.left;
        if size > 0.0 { size } else { 0.0 }
    }
}

/// Flow-relative extents from block-start to block-end.
///
/// Used by floats and exclusions. [`CrossExtents`](crate::CrossExtents)
/// instead measures from line-over. The coordinate systems differ only
/// in `vertical-lr`, where lines stack from the left but line-over is right.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct BlockExtents {
    /// Its block-start end.
    pub start: f32,
    /// Its block-end end.
    pub end: f32,
}

impl BlockExtents {
    /// Returns the block size, or zero if the endpoints are reversed.
    pub fn size(&self) -> f32 {
        let size = self.end - self.start;
        if size > 0.0 { size } else { 0.0 }
    }
}

/// The available area for line breaking.
///
/// Change the area and call [`break_lines`](crate::Layout::break_lines)
/// to relayout without repeating preparation.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct Area {
    /// The block content-box inline extents.
    ///
    /// Defines the tab-stop origin and percentage `text-indent` basis.
    pub inline: InlineExtents,
    /// Where the first line box begins across the block.
    pub block_start: f32,
    /// The block-end limit from `height` or `max-height`, if specified.
    ///
    /// Used only by `line-clamp: auto` to retain fitting lines. Other content
    /// is not constrained by this limit. `None` allows content to determine
    /// block size.
    pub block_end: Option<f32>,
    /// Space above block-start available to first-line annotations.
    ///
    /// Ruby and emphasis use this space before shifting the first line down.
    /// Includes start padding and, if no border intervenes, the collapsed
    /// start margin and preceding [`Layout::room_below`](crate::Layout::room_below),
    /// matching Chrome.
    ///
    /// Defaults to zero. Negative and NaN values count as zero. This is
    /// layout-dependent space rather than a style property.
    pub room_above: f32,
}

impl Area {
    /// Creates an area with `width` pixels of inline space starting at zero.
    ///
    /// Sets block-start to zero with no space above it. Zero, negative and
    /// NaN widths provide no space; lines retain required content and overflow.
    /// Infinite width saturates the layout grid at about 33.5 million pixels.
    pub fn new(width: f32) -> Self {
        Self {
            inline: InlineExtents {
                left: 0.0,
                right: width,
            },
            block_start: 0.0,
            block_end: None,
            room_above: 0.0,
        }
    }

    /// Returns the available inline size. Equivalent to [`inline.size()`](InlineExtents::size).
    pub fn inline_size(&self) -> f32 {
        self.inline.size()
    }
}

/// A float or initial letter to place through [`Exclusions::place`].
///
/// The host uses the key to retrieve properties such as `clear` and shape;
/// layout does not retain the float style.
///
/// Initial letters use the key of the box with `initial-letter`. The margin
/// box extends from the line top (or above it for a raised letter) to the
/// bottom of the fitted ink. To match Chrome, handle these separately:
/// - place at `block_start`, at the start of the available band, even above
///   an earlier float;
/// - place subsequent same-side floats and clearing blocks below it,
///   regardless of float `clear`;
/// - place a block beginning with an initial letter below preceding initial
///   letters on either side.
#[derive(Copy, Clone, PartialEq, Debug)]
#[non_exhaustive]
pub struct FloatRequest {
    /// The node key of the float.
    pub key: NodeKey,
    /// The side it floats to.
    pub side: FloatSide,
    /// The nonnegative margin-box size along the line.
    ///
    /// Includes the supplied border box and style margins, truncated to
    /// the 1/64-pixel layout grid.
    pub inline_size: f32,
    /// Its margin box's size across the lines.
    pub block_size: f32,
    /// The minimum block-start position for placement.
    ///
    /// Uses the reached line top if the float fits beside it, otherwise
    /// the line bottom. The host may move it lower to find space.
    pub block_start: f32,
}

/// A float margin-box placement returned by the host.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct PlacedFloat {
    /// Its margin box along the line.
    pub inline: InlineExtents,
    /// Its margin box across the block.
    pub block: BlockExtents,
}

/// A host-defined checkpoint for speculative float placement.
///
/// Used to rewind a trial break. Hosts that only append floats can store
/// the float count.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct ExclusionsCheckpoint(pub u64);

/// Float placement and available inline space for each line.
///
/// Implement on the enclosing block formatting context so floats from
/// adjacent blocks can affect this layout.
///
/// The breaker first queries a band at the block strut height. It queries
/// again for taller lines and refits if the band narrows. Returned bands
/// are clipped to [`Area`]; [`NoExclusions`] supplies unbounded bands.
///
/// Queries identify lines by [`Line::index`](crate::Line::index). The same
/// line may be queried repeatedly and out of order during balancing or
/// pretty wrapping. Providers such as [`path::PathRoom`](crate::path::PathRoom)
/// should use the index rather than count queries.
///
/// Placed floats must remain stable so refitting converges. NaN bands
/// provide no space, nonadvancing [`below`](Self::below) results are ignored,
/// and a line moves down at most 256 times.
///
/// Floats are placed in logical order at the line top or below the line.
/// Rejected trials restore placement using [`checkpoint`](Self::checkpoint)
/// and [`rewind`](Self::rewind), matching Chrome.
pub trait Exclusions {
    /// Returns inline space for line `line` over the block extents `block`.
    fn band(&self, line: usize, block: BlockExtents) -> InlineExtents;

    /// Returns the next block position below `top` where the band changes.
    ///
    /// Returns `None` if the band never changes. Overflowing lines narrowed
    /// by floats may move to this position.
    fn below(&self, top: f32) -> Option<f32>;

    /// Places a float and returns its margin-box position.
    ///
    /// Retain the placement so later band queries account for it. Requests
    /// follow logical order. A placement undone by [`rewind`](Self::rewind)
    /// may be requested again.
    fn place(&mut self, float: FloatRequest) -> PlacedFloat;

    /// Returns a checkpoint for speculative placement.
    fn checkpoint(&self) -> ExclusionsCheckpoint;

    /// Discards placements made after `to`.
    ///
    /// Called when rejecting a trial break, matching Chrome.
    fn rewind(&mut self, to: ExclusionsCheckpoint);
}

/// An exclusion provider that gives each line the full [`Area`].
///
/// Places floats at their anchors without retaining them.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct NoExclusions;

impl Exclusions for NoExclusions {
    fn band(&self, _line: usize, _block: BlockExtents) -> InlineExtents {
        InlineExtents::EVERYTHING
    }

    fn below(&self, _top: f32) -> Option<f32> {
        None
    }

    fn place(&mut self, float: FloatRequest) -> PlacedFloat {
        PlacedFloat {
            inline: InlineExtents {
                left: 0.0,
                right: float.inline_size,
            },
            block: BlockExtents {
                start: float.block_start,
                end: float.block_start + float.block_size,
            },
        }
    }

    fn checkpoint(&self) -> ExclusionsCheckpoint {
        ExclusionsCheckpoint(0)
    }

    fn rewind(&mut self, _to: ExclusionsCheckpoint) {}
}
