//! Paint operations and geometry in Chrome paint order.
//!
//! [`Line::paints`] yields [`Paint`] values. The host draws them using colors,
//! backgrounds, borders and decoration styles retrieved by [`NodeKey`].
//! Operations follow CSS 2.1 Appendix E:
//!
//! 1. The block background behind the line content.
//! 2. Retained box fragments, outermost first in tree order.
//! 3. Underlines and overlines: block first, then retained and culled boxes.
//! 4. Ruby boxes and text, with emphasis marks after each run.
//! 5. Text and atomic inlines in visual order, with emphasis marks after each run.
//! 6. The generated hyphen. Ellipses appear with text at their visual position.
//! 7. Line-through decorations.
//!
//! Decorations span runs of text within the decorating box. Box edges,
//! objects, unrelated text and hanging whitespace split bars; color changes
//! do not. Initial letters are objects to enclosing decorations, but retain
//! their own and descendant decorations.
//!
//! Each bar provides reference baseline and font metrics, Chrome-compatible
//! underline helpers, and font underline/strikeout metrics for `from-font`.
//! Culled boxes can decorate text without retaining fragments. Their bars use
//! the parent baseline; block decorations use the line baseline.
//!
//! Only culled boxes marked by [`ComputedStyle::decorates`](crate::ComputedStyle::decorates)
//! are queried for decorations. Retained boxes are always queried.
//! Reading paint operations does not allocate.

use core::ops::Range;

pub use super::emphasis::EmphasisMark;

use super::emphasis::EmphasisMarks;
use super::{BoxFragment, CrossExtents, Line, TextRun};
use crate::data::{Id, Table};
use crate::stages::analysis::{ClusterId, ParagraphFlags};
use crate::stages::content::{
    BoxFlags, Content, ContentFlags, ItemFlags, ItemId, ItemKind, NodeId, NodeKey, NodeKind,
};
use crate::stages::fonts::Generated;
use crate::stages::fragments::{FragmentItem, FragmentItemFlags, FragmentItemId, FragmentItemKind};
use crate::stages::lines::InlineExtents;
use crate::stages::measure::{KeptBoxId, KeptBoxes};
use crate::style::FirstLineVariant;
use crate::unit::{self, InlineLayoutUnit, LayoutUnit};

use super::boxes::Atomic;
use crate::work;

/// Decoration lines grouped by paint phase.
///
/// CSS 2.1 Appendix E paints underlines and overlines before text and
/// line-through after text. [`Line::paints`] queries the host-supplied
/// style for the block and each box.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Decorates {
    /// No decoration line.
    #[default]
    None,
    /// Lines painted before the text: an underline, an overline, or both.
    BeforeText,
    /// A line painted after the text: a line through it.
    AfterText,
    /// Lines both before and after the text.
    Both,
}

impl Decorates {
    /// Whether it draws a line painted before the text.
    fn before_text(self) -> bool {
        matches!(self, Self::BeforeText | Self::Both)
    }

    /// Whether it draws a line painted after the text.
    fn after_text(self) -> bool {
        matches!(self, Self::AfterText | Self::Both)
    }
}

/// A paint operation in drawing order.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Paint<'a> {
    /// The block's background behind the line's content.
    Background(Background),
    /// The border box of a retained inline-box fragment.
    ///
    /// Culled boxes are available through
    /// [`Layout::box_fragments`](crate::Layout::box_fragments).
    Box(BoxFragment<'a>),
    /// A decoration line painted before the text: an underline or overline.
    ///
    /// An underline or overline for a run of box or block text.
    DecorationBeforeText(Decoration),
    /// A run of a ruby annotation's text, on its annotation's baseline.
    ///
    /// See [`Line::annotations`].
    Annotation(TextRun<'a>),
    /// A run of text.
    Text(TextRun<'a>),
    /// An emphasis mark, after the run whose text it marks.
    ///
    /// See [`TextRun::emphasis_marks`].
    Emphasis(EmphasisMark),
    /// An atomic inline, which the host paints.
    Atomic(Atomic<'a>),
    /// Text the layout made: a hyphen or an ellipsis.
    ///
    /// A hyphen or an ellipsis follows the text of its line.
    Generated(TextRun<'a>),
    /// A decoration line painted after the text: a line through it.
    ///
    /// A line-through for a run of box or block text.
    DecorationAfterText(Decoration),
}

/// The block background behind one line of content.
///
/// Spans the line-box height and content width, excluding hanging content.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Background {
    inline: InlineExtents,
    block: CrossExtents,
}

impl Background {
    /// Returns inline extents relative to line-box left.
    pub fn inline(&self) -> InlineExtents {
        self.inline
    }

    /// Returns cross-line extents relative to the line-over edge.
    pub fn block(&self) -> CrossExtents {
        self.block
    }
}

/// A decoration bar with its position and reference font metrics.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Decoration {
    key: NodeKey,
    inline: InlineExtents,
    baseline: f32,
    size: f32,
    ascent: f32,
    descent: f32,
    underline: Option<FontDecorationLine>,
    line_through: Option<FontDecorationLine>,
}

/// A font decoration offset and thickness at the used primary-font size.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct FontDecorationLine {
    /// The offset of the top edge below the baseline; negative above it.
    pub offset: f32,
    /// The thickness in pixels, at least one.
    pub thickness: f32,
}

impl Decoration {
    /// Returns the decorating box or block node key.
    pub fn key(&self) -> NodeKey {
        self.key
    }

    /// Returns inline extents relative to line-box left.
    pub fn inline(&self) -> InlineExtents {
        self.inline
    }

    /// Returns the reference baseline offset from line-box top.
    pub fn baseline(&self) -> f32 {
        self.baseline
    }

    /// Returns the used primary-font size, the basis for decoration thickness.
    pub fn font_size(&self) -> f32 {
        self.size
    }

    /// Returns the primary-font ascent above the baseline.
    pub fn ascent(&self) -> f32 {
        self.ascent
    }

    /// Returns the primary-font descent below the baseline.
    pub fn descent(&self) -> f32 {
        self.descent
    }

    /// Returns the Chrome-compatible underline thickness.
    ///
    /// One tenth of the font size, rounded down, with a minimum of one pixel.
    pub fn underline_thickness(&self) -> f32 {
        unit::whole_to_f32(self.thickness())
    }

    /// Returns the offset below the baseline for an `auto` underline.
    ///
    /// Half the thickness, rounded up, with a minimum of one pixel.
    pub fn underline_gap(&self) -> f32 {
        let thickness = self.thickness();
        unit::whole_to_f32((thickness / 2 + thickness % 2).max(1))
    }

    /// Returns underline metrics from `post`, if available.
    ///
    /// The offset is rounded to whole pixels.
    pub fn font_underline(&self) -> Option<FontDecorationLine> {
        self.underline
    }

    /// Returns strikeout metrics from `OS/2`, if available.
    ///
    /// Chrome instead centers line-through one third of the ascent above
    /// the baseline and uses underline thickness.
    pub fn font_line_through(&self) -> Option<FontDecorationLine> {
        self.line_through
    }

    /// Returns thickness for `text-decoration-thickness: from-font`.
    ///
    /// Uses the font underline thickness, falling back to
    /// [`underline_thickness`](Self::underline_thickness), matching Chrome.
    pub fn underline_thickness_from_font(&self) -> f32 {
        self.underline
            .map_or_else(|| self.underline_thickness(), |line| line.thickness)
    }

    /// Returns the underline offset for `text-underline-position: from-font`.
    ///
    /// Uses the font underline offset, falling back to
    /// [`underline_gap`](Self::underline_gap). Chrome applies this only to
    /// text on the alphabetic baseline.
    pub fn underline_gap_from_font(&self) -> f32 {
        self.underline
            .map_or_else(|| self.underline_gap(), |line| line.offset)
    }

    /// The thickness in whole pixels, worked in integers because a float
    /// has no rounding of its own without std.
    fn thickness(&self) -> i32 {
        unit::floor(self.size / 10.0).max(1)
    }
}

/// The phase a line's paint is in.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Phase {
    Background,
    Boxes,
    BeforeText,
    Annotations,
    Text,
    Hyphen,
    AfterText,
    Done,
}

/// A line's paint, in Chrome's order: what [`Line::paints`] walks.
///
/// **Linear in the line's items**, however deep its boxes nest or the
/// reordering splits them. Line layout links each box item to the one painted
/// after it (by box in tree order, a box's parts left to right). So the
/// boxes are walked by following the links, and each decorating box's bars
/// are looked for among its own parts' items only. A line's decorations
/// cost the items each decorating box holds, as Chrome's propagated
/// decorations cost each text fragment every decorating box around it.
pub(super) struct PaintWalk<'a, F> {
    line: Line<'a>,
    /// Every item of the layout, of which the phases read the line's.
    items: &'a Table<FragmentItemId, FragmentItem>,
    /// The line's own items.
    own: Range<FragmentItemId>,
    /// The line's annotation lines, each heading its text.
    annotations: Range<FragmentItemId>,
    /// The line's box item painted first, where it has one: the head of
    /// the links its box items are painted by.
    first: Option<FragmentItemId>,
    /// The items of the hyphen the line owes, which the text phase passes
    /// and the hyphen phase paints after the text: one a font, side by
    /// side. None until the text phase meets them.
    hyphen: Range<FragmentItemId>,
    /// The emphasis marks of the run painted last, still to hand out.
    marks: Option<EmphasisMarks<'a>>,
    decorates: F,
    phase: Phase,
    /// The node being decorated, in tree order: `None` before the first,
    /// which is the block.
    node: Option<NodeId>,
    /// Where among the line's items, or its annotations', the phase has got
    /// to.
    at: FragmentItemId,
    /// The next box item to paint, or the first part of the next box to
    /// decorate: `None` past the last.
    next: Option<FragmentItemId>,
    /// Where the bars of the node being decorated end: one past the last
    /// item of its last part, the line's end for the block.
    end: FragmentItemId,
    /// The baseline of the node being decorated: its parts', or the
    /// line's for the block.
    baseline: LayoutUnit,
    /// Where the phase's walk over the line's culled boxes has got to, once
    /// the kept boxes are done.
    culled: Option<CulledBoxes>,
    /// Where the bars of the culled box being decorated end, where the line
    /// sets its leaves in the order of their clusters: the cluster its text
    /// ends at.
    until: Option<ClusterId>,
    /// Where the walk over the kept boxes, which says which boxes are
    /// culled, last stood: `None` before it has stood anywhere. Both
    /// decoration phases move it.
    kept: Option<KeptBoxId>,
}

/// Where a decoration phase's walk over a line's culled boxes stands: how
/// many of the decorating ones open across the line's start are still to
/// hand out, outermost first, and then the next item of the line to look
/// for a culled box opening at.
#[derive(Copy, Clone, Debug)]
struct CulledBoxes {
    /// The node of the line's first item, which the boxes open across its
    /// start are around.
    start: NodeId,
    /// How many of those that decorate are still to hand out: the outermost
    /// of them next.
    above: usize,
    /// The next item that may open a box on the line.
    item: ItemId,
    /// The line sets its leaves left to right in the order of their
    /// clusters: every cluster of the text is at its paragraph's level,
    /// which reads left to right. A culled box's bars are then looked for
    /// from the first leaf of the box handed out before it, which opens
    /// before it, to where its text ends.
    in_order: bool,
    /// The first leaf of the last box opening on the line, where the next
    /// one's bars are looked for from.
    resume: FragmentItemId,
}

/// What says whether a culled box draws in a decoration phase: the line's
/// tables, the caller's `decorates`, and the side of the text the phase
/// paints on.
struct CulledDrawing<'a, 'f, F> {
    content: &'a Content,
    kept: &'a KeptBoxes,
    /// Where the walk over `kept` stands, moved to each box asked.
    at: KeptBoxId,
    variant: FirstLineVariant,
    decorates: &'f F,
    after_text: bool,
}

impl<'a, 'f, F: Fn(NodeKey) -> Decorates> CulledDrawing<'a, 'f, F> {
    /// Returns what decides for `line`'s phase painted after its text where
    /// `after_text`, before it otherwise, its walk over the kept boxes
    /// standing at `at`.
    fn new(line: Line<'a>, decorates: &'f F, after_text: bool, at: KeptBoxId) -> Self {
        let layout = line.layout();
        Self {
            content: layout.content(),
            kept: &layout.measured().text(line.variant()).kept_boxes,
            at,
            variant: line.variant(),
            decorates,
            after_text,
        }
    }

    /// Returns whether `node` is a culled box that decorates, and draws in
    /// this phase.
    ///
    /// Only a box that decorates is looked for among the kept boxes, by the
    /// walk, which the boxes asked keep near one another.
    fn draws(&mut self, node: NodeId) -> bool {
        let (nodes, facts) = (&self.content.nodes, &self.content.facts);
        let decorating = nodes.kind(node).is_some_and(NodeKind::is_inline_box)
            && node != NodeId::BLOCK
            && facts
                .box_facts(nodes.box_facts(node, self.variant))
                .has(BoxFlags::DECORATES);
        if !decorating {
            return false;
        }
        self.at = self.kept.walk_to(self.at, node);
        let culled = self.kept.node(self.at) != Some(node);
        culled && {
            let asked = (self.decorates)(nodes.key(node));
            if self.after_text {
                asked.after_text()
            } else {
                asked.before_text()
            }
        }
    }

    /// Returns the `k`th of the boxes around `start` that draw, the
    /// innermost first, or how many there are where there are fewer.
    ///
    /// It walks up at most one step a node, whatever the parents say.
    fn around(&mut self, start: NodeId, k: usize) -> Result<NodeId, usize> {
        let nodes = &self.content.nodes;
        let mut node = start;
        let mut count = 0;
        for _ in 0..nodes.len() {
            work::step();
            if self.draws(node) {
                count += 1;
                if count == k {
                    return Ok(node);
                }
            }
            let parent = nodes.parent(node);
            if parent == node {
                break;
            }
            node = parent;
        }
        Err(count)
    }
}

impl<'a, F: Fn(NodeKey) -> Decorates> PaintWalk<'a, F> {
    /// What `line` paints, asking `decorates` which decoration lines each key draws.
    pub(super) fn new(line: &Line<'a>, decorates: F) -> Self {
        let fragments = line.layout().fragments();
        let own = fragments.line_item_ids(line.id());
        Self {
            line: *line,
            items: &fragments.items,
            own: own.clone(),
            annotations: fragments.annotation_ids(line.id()),
            first: line.head().and_then(FragmentItem::painted_next),
            hyphen: own.start..own.start,
            marks: None,
            decorates,
            phase: Phase::Background,
            node: None,
            at: own.start,
            next: None,
            end: own.start,
            baseline: LayoutUnit::ZERO,
            culled: None,
            until: None,
            kept: None,
        }
    }
}

impl<'a, F: Fn(NodeKey) -> Decorates> PaintWalk<'a, F> {
    /// Whether `item` is a leaf that is painted, and not what hangs.
    fn content(item: &FragmentItem) -> bool {
        matches!(
            item.kind(),
            FragmentItemKind::Text | FragmentItemKind::Atomic
        ) && !item.flags.contains(FragmentItemFlags::HIDDEN)
            && !item.flags.contains(FragmentItemFlags::HANGS)
    }

    /// Item `at` where it is one of the line's own, which every phase but
    /// the annotations' walks.
    fn line_item(&self, at: FragmentItemId) -> Option<&'a FragmentItem> {
        if self.own.contains(&at) {
            self.items.get(at)
        } else {
            None
        }
    }

    /// The background: the extent of the line's content, where it has any.
    fn background(&self) -> Option<Background> {
        let mut extent: Option<(InlineLayoutUnit, InlineLayoutUnit)> = None;
        let own = self.items.get_slice(self.own.clone()).unwrap_or_default();
        for item in own.iter().filter(|item| Self::content(item)) {
            work::step();
            let left = item.inline;
            let right = left + item.advance();
            extent = Some(match extent {
                None => (left, right),
                Some((from, to)) => (from.min(left), to.max(right)),
            });
        }
        let (from, to) = extent?;
        let metrics = self.line.metrics();
        Some(Background {
            inline: InlineExtents {
                left: from.to_px(),
                right: to.to_px(),
            },
            block: CrossExtents {
                over: 0.0,
                under: metrics.height(),
            },
        })
    }

    /// The next box part to paint: the box items in tree order, each
    /// box's left to right, as line layout linked them.
    fn next_box_part(&mut self) -> Option<Paint<'a>> {
        work::step();
        let at = self.next?;
        let item = self
            .line_item(at)
            .filter(|item| item.kind() == FragmentItemKind::Box)?;
        self.next = item.painted_next();
        Some(Paint::Box(BoxFragment::from_item(self.line, item)))
    }

    /// The next decoration bar of the phase's kind, painted after the text
    /// where `after_text` and before it otherwise, of the block and then
    /// each box in tree order that says it draws one.
    fn next_decoration(&mut self, after_text: bool) -> Option<Decoration> {
        let nodes = &self.line.layout().content().nodes;
        loop {
            work::step();
            let node = match self.node {
                Some(node) => node,
                None => {
                    // The block: its bars are the whole line's, on its
                    // baseline.
                    self.node = Some(NodeId::BLOCK);
                    self.at = self.own.start;
                    self.end = self.own.end;
                    self.baseline = self.line.record().ascent();
                    self.next = self.first;
                    continue;
                }
            };
            let asked = (self.decorates)(nodes.key(node));
            let draws = if after_text {
                asked.after_text()
            } else {
                asked.before_text()
            };
            if draws && let Some(bar) = self.bar(node) {
                return Some(bar);
            }
            // The next box, its parts following one another in the
            // links: its bars are among their items, on their baseline.
            let Some(first) = self.next else {
                // Then each culled box that decorates, across the line, on
                // its parent's baseline.
                let (node, from, until) = self.next_culled(after_text)?;
                self.node = Some(node);
                self.at = from;
                self.end = self.own.end;
                self.until = until;
                self.baseline = self.culled_baseline(node);
                continue;
            };
            let head = self.line_item(first)?;
            let mut last = first;
            let mut after = head.painted_next();
            while let Some(at) = after
                && let Some(item) = self.line_item(at)
                && item.node == head.node
            {
                work::step();
                last = at;
                after = item.painted_next();
            }
            self.next = after;
            self.node = Some(head.node);
            self.until = None;
            self.at = first;
            self.end = self.line_item(last).map_or(last, |item| {
                FragmentItemId::new(last.get() + 1 + item.descendants()).min(self.own.end)
            });
            self.baseline = head.block;
        }
    }

    /// The baseline of the culled box `node` on the line: its parent's, the
    /// innermost kept box's around it, or the line's where none is around
    /// it, as for a link in a paragraph.
    ///
    /// A kept box around it holds some of the line, so it has a box item
    /// there. The links from the line's item visit only those.
    fn culled_baseline(&self, node: NodeId) -> LayoutUnit {
        let nodes = &self.line.layout().content().nodes;
        let mut innermost: Option<&FragmentItem> = None;
        let mut next = self.first;
        while let Some(at) = next
            && let Some(item) = self.line_item(at)
        {
            work::step();
            if nodes.contains(item.node, node)
                && innermost.is_none_or(|found| found.node < item.node)
            {
                innermost = Some(item);
            }
            next = item.painted_next();
        }
        innermost.map_or_else(|| self.line.record().ascent(), |item| item.block)
    }

    /// The next culled box of the line that draws a decoration of the
    /// phase, painted after the text where `after_text` and before it
    /// otherwise: those open across the line's start first, outermost first,
    /// then those opening on it, in tree order (see the module
    /// documentation).
    fn next_culled(
        &mut self,
        after_text: bool,
    ) -> Option<(NodeId, FragmentItemId, Option<ClusterId>)> {
        let content = self.line.layout().content();
        // Content with no box that decorates has none to look for, and a
        // line no inline box holds any of, none of its own.
        if !content.flags.contains(ContentFlags::DECORATED_BOXES) {
            return None;
        }
        let boxed = self
            .line
            .head()
            .is_some_and(|head| head.flags.contains(FragmentItemFlags::BOXED));
        if !boxed && work::fast_paths() {
            return None;
        }
        let at = self.kept_walk();
        let mut drawing = CulledDrawing::new(self.line, &self.decorates, after_text, at);
        let mut culled = match self.culled {
            Some(culled) => culled,
            None => self.start_culled(&mut drawing),
        };
        // A box open across the line's start is looked for from it.
        let found = if culled.above > 0 {
            let node = drawing.around(culled.start, culled.above).ok();
            culled.above -= 1;
            node.map(|node| (node, self.own.start))
        } else {
            self.next_opening(&mut drawing, &mut culled)
        };
        self.kept = Some(drawing.at);
        let found = found.map(|(node, from)| self.bars_from(&mut culled, node, from));
        self.culled = Some(culled);
        found
    }

    /// Returns where the walk over the kept boxes stands, starting it the
    /// first time.
    ///
    /// It starts at the line's first box item's row, where it has one, and
    /// otherwise at the line's first node, sought once.
    fn kept_walk(&self) -> KeptBoxId {
        if let Some(at) = self.kept {
            return at;
        }
        let first = self
            .first
            .and_then(|at| self.line_item(at))
            .and_then(FragmentItem::kept_box);
        first.unwrap_or_else(|| {
            let layout = self.line.layout();
            let kept = &layout.measured().text(self.line.variant()).kept_boxes;
            let start = layout
                .content()
                .items
                .get(self.line.record().first_item)
                .map_or(NodeId::BLOCK, |item| item.node);
            kept.first_from(start)
        })
    }

    /// Starts the walk over the line's culled boxes: at the node of the item
    /// holding the line's first cluster, with the boxes around it that draw
    /// still to hand out.
    fn start_culled(&self, drawing: &mut CulledDrawing<'a, '_, F>) -> CulledBoxes {
        let layout = self.line.layout();
        let (content, analysis) = (layout.content(), layout.analysis());
        let clusters = self.line.record().clusters();
        let in_order = work::fast_paths()
            && !analysis.flags.contains(ParagraphFlags::MIXED_LEVELS)
            && !self.line.level().is_rtl();
        // The item holding the line's first cluster: from the line's first
        // item, past those sitting at its start, the last whose first
        // cluster is not past it.
        let mut first = self.line.record().first_item;
        let mut next = ItemId::new(first.get() + 1);
        while content.items.get(next).is_some()
            && analysis.item_clusters.start(next) <= clusters.start
        {
            work::step();
            first = next;
            next = ItemId::new(next.get() + 1);
        }
        let start = content
            .items
            .get(first)
            .map_or(NodeId::BLOCK, |item| item.node);
        // How many around the start draw: one walk up, past the outermost.
        let above = drawing
            .around(start, usize::MAX)
            .map_or_else(|count| count, |_| 0);
        CulledBoxes {
            start,
            above,
            item: ItemId::new(first.get() + 1),
            in_order,
            resume: self.own.start,
        }
    }

    /// Returns the next culled box opening on the line that draws, with the
    /// item its bars are looked for from, moving `culled` past it.
    fn next_opening(
        &self,
        drawing: &mut CulledDrawing<'a, '_, F>,
        culled: &mut CulledBoxes,
    ) -> Option<(NodeId, FragmentItemId)> {
        let layout = self.line.layout();
        let (content, analysis) = (layout.content(), layout.analysis());
        let clusters = self.line.record().clusters();
        while let Some(item) = content.items.get(culled.item) {
            work::step();
            if analysis.item_clusters.start(culled.item) >= clusters.end {
                break;
            }
            culled.item = ItemId::new(culled.item.get() + 1);
            let opens = item.kind == ItemKind::Open && !item.flags.contains(ItemFlags::ANNOTATION);
            if opens && drawing.draws(item.node) {
                return Some((item.node, culled.resume));
            }
        }
        None
    }

    /// Returns where the bars of culled box `node`, found from `from`, are
    /// looked for: from its first leaf, to the cluster its text ends at.
    ///
    /// On a line in order, the box after it is looked for from that leaf.
    /// Otherwise its bars are looked for over the whole line.
    fn bars_from(
        &self,
        culled: &mut CulledBoxes,
        node: NodeId,
        from: FragmentItemId,
    ) -> (NodeId, FragmentItemId, Option<ClusterId>) {
        if !culled.in_order {
            return (node, self.own.start, None);
        }
        let layout = self.line.layout();
        let nodes = &layout.content().nodes;
        let ends = layout.analysis().item_clusters.span(nodes.items(node)).end;
        let first = (from.get()..self.own.end.get())
            .map(FragmentItemId::new)
            .find(|&at| {
                work::step();
                self.items
                    .get(at)
                    .is_some_and(|item| Self::leaf(item) && nodes.contains(node, item.node))
            })
            .unwrap_or(self.own.end);
        if culled.above == 0 && first < self.own.end {
            culled.resume = first;
        }
        (node, first, Some(ends))
    }

    /// Whether `item` is a leaf a decoration's run reads: text, an atomic or
    /// a generated text, not hidden.
    fn leaf(item: &FragmentItem) -> bool {
        item.is_leaf() && !item.flags.contains(FragmentItemFlags::HIDDEN)
    }

    /// The next bar of `node`'s decoration from where the phase has got to:
    /// a run of its text and its descendants' that nothing else splits.
    fn bar(&mut self, node: NodeId) -> Option<Decoration> {
        let layout = self.line.layout();
        let nodes = &layout.content().nodes;
        // The initial letter's box, where `node` is outside it. Chrome sets
        // the box as an atomic inline, which the decorations of the boxes
        // around it do not reach (`AppliedTextDecorations`).
        let letter = layout
            .measured()
            .text(self.line.variant())
            .initial_letter()
            .map(|letter| letter.node)
            .filter(|&letter| !nodes.contains(letter, node));
        let mut run: Option<(InlineLayoutUnit, InlineLayoutUnit)> = None;
        // Only its parts hold its text, so its bars are found among
        // their items: what lies outside them would start none of its bars
        // and add to none.
        while self.at < self.end
            && let Some(item) = self.line_item(self.at)
        {
            work::step();
            let leaf = Self::leaf(item);
            if !leaf {
                self.at = FragmentItemId::new(self.at.get() + 1);
                continue;
            }
            // A culled box's text ends here, on a line in order: nothing of
            // it is further on.
            if self
                .until
                .is_some_and(|until| item.clusters().start >= until)
            {
                self.at = self.end;
                break;
            }
            let left = item.inline;
            let right = left + item.advance();
            let decorated = item.kind() == FragmentItemKind::Text
                && !item.flags.contains(FragmentItemFlags::HANGS)
                && nodes.contains(node, item.node)
                && letter.is_none_or(|letter| !nodes.contains(letter, item.node));
            match run {
                None if decorated => run = Some((left, right)),
                None => {}
                // Text of its own right after the run, with no room
                // between: the run goes on.
                Some((from, to)) if decorated && left == to => run = Some((from, right)),
                // Something that takes no room splits nothing.
                Some(_) if !decorated && left == right => {}
                // Anything else ends it here, and is looked at again for
                // the next.
                Some(_) => break,
            }
            self.at = FragmentItemId::new(self.at.get() + 1);
        }
        let (from, to) = run?;
        Some(self.decoration(node, from, to))
    }

    /// The bar of `node`'s decoration from `from` to `to`, with its box's
    /// baseline and font.
    fn decoration(&self, node: NodeId, from: InlineLayoutUnit, to: InlineLayoutUnit) -> Decoration {
        let layout = self.line.layout();
        let content = layout.content();
        // The block's first line decorates in its first-line fonts.
        let text = content.nodes.text_facts(node, self.line.variant());
        let used = layout
            .fonts()
            .primary_font(content.facts.text_request(text));
        let (size, ascent, descent) = used.map_or((0.0, 0.0, 0.0), |used| {
            let metrics = &used.metrics;
            (
                used.size.to_px(),
                metrics.ascent.to_px(),
                metrics.descent.to_px(),
            )
        });
        // A line the font does not give has no thickness.
        let line = |offset: LayoutUnit, thickness: LayoutUnit| {
            (thickness > LayoutUnit::ZERO).then(|| FontDecorationLine {
                offset: offset.to_px(),
                thickness: thickness.to_px(),
            })
        };
        let underline = used.and_then(|used| {
            line(
                used.metrics.underline_offset,
                used.metrics.underline_thickness,
            )
        });
        let line_through = used.and_then(|used| {
            line(
                used.metrics.strikeout_offset,
                used.metrics.strikeout_thickness,
            )
        });
        // A box's baseline is its items', and the block's the line's, found
        // when its bars were.
        let baseline = self.baseline;
        Decoration {
            key: content.nodes.key(node),
            inline: InlineExtents {
                left: from.to_px(),
                right: to.to_px(),
            },
            baseline: baseline.to_px(),
            size,
            ascent,
            descent,
            underline,
            line_through,
        }
    }

    /// The next leaf of the text phase: not the hyphen's items, which it
    /// remembers as it passes them, for the hyphen phase.
    fn next_leaf(&mut self) -> Option<Paint<'a>> {
        while let Some(item) = self.line_item(self.at) {
            work::step();
            let at = self.at;
            self.at = FragmentItemId::new(at.get() + 1);
            if item.flags.contains(FragmentItemFlags::HIDDEN) {
                continue;
            }
            match item.kind() {
                FragmentItemKind::Text => {
                    let run = TextRun::new(self.line, item);
                    self.marks = item.is_marked().then(|| EmphasisMarks::new(&run));
                    return Some(Paint::Text(run));
                }
                FragmentItemKind::Atomic => {
                    return Some(Paint::Atomic(Atomic::new(self.line, item)));
                }
                FragmentItemKind::Generated => {
                    let run = TextRun::new(self.line, item);
                    if run.generated() != Some(Generated::Hyphen) {
                        return Some(Paint::Generated(run));
                    }
                    if self.hyphen.is_empty() {
                        self.hyphen.start = at;
                    }
                    self.hyphen.end = self.at;
                }
                _ => {}
            }
        }
        None
    }

    /// The next of the hyphen's items, which the text phase passed.
    fn next_hyphen(&mut self) -> Option<Paint<'a>> {
        let at = self.hyphen.start;
        if at >= self.hyphen.end {
            return None;
        }
        self.hyphen.start = FragmentItemId::new(at.get() + 1);
        let item = self.line_item(at)?;
        Some(Paint::Generated(TextRun::new(self.line, item)))
    }

    /// The next of the line's annotations' boxes, runs of text or atomic
    /// inlines, each annotation's boxes first, outermost first, then its
    /// runs and atomic inlines, each run with its marks to follow.
    fn next_annotation(&mut self) -> Option<Paint<'a>> {
        while self.annotations.contains(&self.at)
            && let Some(item) = self.items.get(self.at)
        {
            work::step();
            self.at = FragmentItemId::new(self.at.get() + 1);
            match item.kind() {
                FragmentItemKind::Box => {
                    return Some(Paint::Box(BoxFragment::from_item(self.line, item)));
                }
                FragmentItemKind::Text => {
                    let run = TextRun::new(self.line, item);
                    self.marks = item.is_marked().then(|| EmphasisMarks::new(&run));
                    return Some(Paint::Annotation(run));
                }
                FragmentItemKind::Atomic => {
                    return Some(Paint::Atomic(Atomic::new(self.line, item)));
                }
                _ => {}
            }
        }
        None
    }

    /// Moves on to `phase`, from its start: the line's annotations' for
    /// theirs, and its own items' for any other.
    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        self.node = None;
        self.culled = None;
        self.until = None;
        self.at = match phase {
            Phase::Annotations => self.annotations.start,
            _ => self.own.start,
        };
        self.end = self.own.end;
        self.next = match phase {
            Phase::Boxes => self.first,
            _ => None,
        };
    }
}

impl<'a, F: Fn(NodeKey) -> Decorates> Iterator for PaintWalk<'a, F> {
    type Item = Paint<'a>;

    fn next(&mut self) -> Option<Paint<'a>> {
        loop {
            // The marks of the run painted last come right after it.
            if let Some(marks) = self.marks.as_mut() {
                if let Some(mark) = marks.next() {
                    return Some(Paint::Emphasis(mark));
                }
                self.marks = None;
            }
            match self.phase {
                Phase::Background => {
                    self.enter(Phase::Boxes);
                    if let Some(background) = self.background() {
                        return Some(Paint::Background(background));
                    }
                }
                Phase::Boxes => match self.next_box_part() {
                    Some(item) => return Some(item),
                    None => self.enter(Phase::BeforeText),
                },
                Phase::BeforeText => match self.next_decoration(false) {
                    Some(bar) => return Some(Paint::DecorationBeforeText(bar)),
                    None => self.enter(Phase::Annotations),
                },
                Phase::Annotations => match self.next_annotation() {
                    Some(item) => return Some(item),
                    None => self.enter(Phase::Text),
                },
                Phase::Text => match self.next_leaf() {
                    Some(item) => return Some(item),
                    None => self.enter(Phase::Hyphen),
                },
                Phase::Hyphen => match self.next_hyphen() {
                    Some(item) => return Some(item),
                    None => self.enter(Phase::AfterText),
                },
                Phase::AfterText => match self.next_decoration(true) {
                    Some(bar) => return Some(Paint::DecorationAfterText(bar)),
                    None => self.enter(Phase::Done),
                },
                Phase::Done => return None,
            }
        }
    }
}
