//! Measurement: everything about the text that doesn't depend on the line
//! width.
//!
//! In: [`MeasureInput`], the content, analysis, fonts and shaped glyphs, and
//! shaping's [`Advances`] by value. Out: [`Measured`].
//! Start at: [`measure_text`], then `Scan::walk` in `scan`.
//!
//! - `scan` turns the advances into prefix sums, and finds the intrinsic
//!   widths, item extents, line-edge costs and ruby columns.
//! - `metrics` resolves each text's primary-font metrics.
//! - `boxes` works out the boxes' fixed shifts and extents.
//! - `edges`, `spacing`, `autospace`, `tabs`, `hang`, `justify` and `ruby`
//!   hold rules that line breaking and line layout ask too.
//! - `pen` answers where the pen stands at a cluster, and what a cluster
//!   adds along the line.
//! - `extent` holds [`Extent`], the reach over and under a baseline.
//! - `prefix`, `items`, `ruby_columns` and `generated` own the stored tables.
//!
//! [`Measured`] holds the text's measurements and the first line's. It stays
//! frozen until the next preparation.

mod autospace;
mod boxes;
mod edges;
mod extent;
mod generated;
mod hang;
mod items;
mod justify;
mod metrics;
mod pen;
mod prefix;
mod ruby;
mod ruby_columns;
mod scan;
mod scan_em_boxes;
mod scan_ends;
mod scan_initial_letter;
mod scan_intrinsic;
mod scan_ruby;
mod scan_walk;
mod spacing;
mod tabs;
#[cfg(test)]
mod tests;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::mem;
use core::ops::Range;

use crate::config::{
    DominantBaselines, RubyBreakWithin, RubyOverhangRule, SuperSubPosition, WordSpacing,
};
use crate::data::{Id, Keyed, SortedTable, Table, define_flags, define_id, heap_bytes, make_room};
use crate::stages::analysis::{Analysis, ClusterId, ParagraphId};
use crate::stages::content::{
    AtomicId, Content, FloatId, Item, ItemFlags, ItemId, ItemKind, NodeId, TextFacts, TextFactsId,
    VariantText,
};
use crate::stages::fonts::{FontLineMetrics, Fonts, LineBaseline, UsedFontId};
use crate::stages::shape::{Advances, ShapeSession, Shaped, ShapedText};
use crate::stages::{Segment, Segments, Step};
use crate::style::{FirstLine, FirstLineState, FirstLineVariant, WritingMode};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use autospace::SeamContext;
use scan_ends::AnchoredFloats;
use scan_walk::OverhangNeighbour;

pub(super) use autospace::AutospaceRules;
pub(crate) use edges::{BoundarySplit, edge_room, items_after_break};
pub(crate) use extent::Extent;
use generated::GeneratedTexts;
pub(crate) use generated::{GeneratedPieces, ShapedGenerated};
pub(crate) use items::KeptBoxes;
use items::{ItemEmBoxes, ItemExtents};
pub(crate) use justify::{JustifyOpportunities, JustifySummary};
pub(super) use metrics::normal_extent;
use metrics::text_metrics;
use prefix::LineEdgeCosts;
pub(crate) use prefix::PrefixAdvances;
pub(crate) use ruby::{RubySpread, em_box};
pub(crate) use ruby_columns::{ColumnWalk, RubyColumn, RubyColumns, RubyLevel, RubySide};
pub(crate) use spacing::{LetterWordSpacing, WordSpacingRule};
#[cfg(test)]
use tabs::tab_advance;
pub(crate) use tabs::tab_advance_reached;
use tabs::{TabReach, TabStops};

/// What the measure stage reads: the stages before it and some config.
///
/// Of the shaping, it reads the glyph table, which says which clusters
/// continue a ligature and take no spacing. The advances come in by value.
/// The config says which clusters take word-spacing, how far a superscript
/// or subscript moves, and whether `dominant-baseline` moves a box.
pub(crate) struct MeasureInput<'a> {
    pub(crate) content: &'a Content,
    pub(crate) analysis: &'a Analysis,
    pub(crate) fonts: &'a Fonts,
    pub(crate) shaped: &'a Shaped,
    pub(crate) word_spacing: WordSpacing,
    /// How far `vertical-align: super` and `sub` move a box.
    pub(crate) super_sub: SuperSubPosition,
    /// Whether `dominant-baseline` moves a box.
    pub(crate) dominant_baseline: DominantBaselines,
    /// How far a ruby annotation may reach over the text beside it.
    pub(crate) ruby_overhang: RubyOverhangRule,
    pub(crate) ruby_break_within: RubyBreakWithin,
}

impl<'a> MeasureInput<'a> {
    /// Returns the clusters the scan measures in `variant`.
    ///
    /// They are the text's, or the first paragraph's for the first line's
    /// variant.
    fn reach(&self, variant: FirstLineVariant) -> Range<ClusterId> {
        let all = ClusterId::new(0)..self.analysis.clusters.end_id();
        match variant {
            FirstLineVariant::Standard => all,
            FirstLineVariant::FirstLine => {
                self.analysis.first_line_reach(self.content).unwrap_or(all)
            }
        }
    }

    /// Returns the line metrics of the primary font of text with the facts
    /// `text`.
    ///
    /// The primary font is its font request's first resolved font, and its
    /// lines are measured by it. Returns `None` where it has none.
    fn primary(&self, text: TextFactsId) -> Option<&'a FontLineMetrics> {
        let request = self.content.facts.text_request(text);
        self.fonts.primary_font(request).map(|used| &used.metrics)
    }

    /// Returns the computed font size of text with the facts `text`.
    fn size(&self, text: TextFactsId) -> f32 {
        let facts = &self.content.facts;
        facts.request(facts.text_request(text)).font.computed_size()
    }
}

/// Measures the text of `input` into `out`, turning shaping's `advances`
/// into prefix sums in place.
///
/// Writes each text's primary-font metrics first, once for both variants.
/// Then shapes the hyphens and ellipses font selection chose fonts for, with
/// the shaping caches in `cx`, so that a line ending at a soft hyphen is
/// charged its hyphen's width. Where `::first-line` restyles something, it
/// measures the first paragraph again in its first-line styles, from the
/// first line's advances or a copy of the text's.
///
/// Clears and fills `out`; nothing else writes it. Each advance buffer
/// becomes a prefix, so ownership passes and each buffer keeps one writer.
pub(crate) fn measure_text(
    input: &MeasureInput<'_>,
    cx: &mut ShapeSession<'_, '_>,
    scratch: &mut MeasureScratch,
    advances: Advances,
    out: &mut Measured,
) {
    let (advances, mut first_advances) = advances.into_parts();
    out.clear();
    out.word_spacing_rule = WordSpacingRule::new(input.word_spacing, input.content);
    text_metrics(input, &mut out.text_metrics);
    // The generated texts, whichever line they are on, where the font
    // stage chose fonts for one.
    if input.fonts.generated().iter().next().is_some() {
        out.generated_mut().fill(input, cx);
    }
    let first = input.analysis.first_line_reach(input.content);
    if let Some(reach) = &first
        && first_advances.is_empty()
    {
        // Set in the text's fonts: its advances, before they are summed,
        // with room for the end's entry the prefix adds, as shaping makes
        // room for it.
        let own = advances.get(..reach.end.get()).unwrap_or_default();
        make_room(&mut first_advances, own.len() + 1);
        first_advances.extend_from_slice(own);
    }
    let Measured {
        text,
        text_metrics,
        generated,
        intrinsic,
        word_spacing_rule,
    } = out;
    let generated = generated.as_deref().unwrap_or(&NO_GENERATED);
    let scan = |variant| Scan::new(input, variant, *word_spacing_rule, generated, text_metrics);
    let widths = scan(FirstLineVariant::Standard).measure(cx, scratch, advances, text.text_mut());
    if first.is_none() {
        *intrinsic = widths.finish(None);
        // Kept for the next build that has a first line, where one had.
        if let Some(fl) = text.kept_first_line_mut() {
            fl.prefix.sums = first_advances;
            fl.prefix.sums.clear();
        }
        return;
    }
    let first_widths = scan(FirstLineVariant::FirstLine).measure(
        cx,
        scratch,
        first_advances,
        text.first_line_mut(),
    );
    *intrinsic = widths.finish(Some(&first_widths));
}

/// Preparation's reusable cursors for intrinsic ruby pieces, one per level.
#[derive(Default)]
pub(crate) struct MeasureScratch {
    ruby_starts: Vec<ClusterId>,
}
heap_bytes! { MeasureScratch { ruby_starts } }

define_id! {
    /// Names an annotation level of a ruby column in a layout's table of
    /// [`RubyLevel`]s.
    pub(crate) struct RubyLevelId(u32);
}

define_id! {
    /// Names a ruby column in a layout's table of [`RubyColumn`]s, which
    /// are in text order.
    pub(crate) struct RubyColumnId(u32);
}

define_id! {
    /// Names an inline box that keeps a fragment in a layout's table of
    /// them, [`KeptBoxes`], which is in node order.
    pub(crate) struct KeptBoxId(u32);
}

define_id! {
    /// Names an em box in a layout's table of the distinct em boxes of its
    /// text items, [`ItemEmBoxes`]: few, one a font and size.
    struct EmBoxId(u8);
}

impl EmBoxId {
    /// The first, which is none: what an item keeping none names.
    const NONE: Self = Self(0);
}

define_id! {
    /// Names a generated text in a layout's table of
    /// [`ShapedGenerated`]s.
    pub(crate) struct ShapedGeneratedId(u32);
}

define_flags! {
    /// What the measure stage found of a paragraph: gates the breaker uses
    /// to skip work. One byte a paragraph.
    pub(crate) struct MeasureFlags(u8) {
        /// The prefix decreases from one boundary to the next.
        ///
        /// A negative advance, spacing or edge makes it decrease. The breaker
        /// then walks the paragraph instead of searching it. A dip between
        /// two boundaries that recovers before the next does not count, since
        /// the breaker reads only boundaries. Tabs, the other reason to walk,
        /// are analysis's `HAS_TABS`.
        pub(super) const NONMONOTONE = 1 << 0;
        /// Some line-edge cost falls in the paragraph, so the breaker reads
        /// the costs. A cloned box's edges count.
        pub(super) const HAS_EDGE_COSTS = 1 << 1;
        /// `text-autospace` put room at a seam in the paragraph.
        ///
        /// A reshaped line edge spaces the seam as the prefix did, and a line
        /// ending at the seam gives the room back. The breaker looks for
        /// seams only where this is set.
        pub(super) const HAS_AUTOSPACE = 1 << 2;
        /// An inline box open in the paragraph has a strut reaching past the
        /// block's, over or under.
        ///
        /// Only then can the struts of the boxes open across a line's start
        /// grow its line box past the block's strut. The breaker unites them
        /// only where this is set.
        pub(super) const BOXES_PAST_STRUT = 1 << 3;
        /// Some root ruby column offers internal cuts. Other paragraphs skip
        /// continuation fitting.
        pub(super) const HAS_BREAKABLE_RUBY = 1 << 4;
    }
}

define_flags! {
    /// What a line-edge cost says beyond its amounts.
    pub(crate) struct LineEdgeFlags(u8) {
        /// A line ending here hyphenates.
        ///
        /// The end amount is the hyphen's width, and line layout draws the
        /// generated hyphen.
        pub(super) const HYPHEN = 1 << 0;
        /// The start amount gives back a mark hanging at the start of the
        /// block's first line (`hanging-punctuation: first`).
        ///
        /// Only that line pays it. The mark is its first cluster.
        pub(super) const FIRST_LINE_ONLY = 1 << 1;
        /// The flex is a mark that hangs whether or not the line needs it
        /// to (`force-end` or `last`).
        ///
        /// The breaker always takes such a flex. Without this flag, it takes
        /// a flex only where the line would not fit.
        pub(super) const HANG_FORCED = 1 << 2;
    }
}

/// Everything a line pays or gains because it starts or ends at one boundary.
///
/// The costs are 20 bytes each, in a sparse table sorted by boundary. Plain
/// text has none.
///
/// A cost comes from the hyphen at a soft hyphen, cloned box edges,
/// `line-padding`, `hanging-punctuation`, or ruby overhang with no
/// neighbour. Each depends on the boundary, not on the width, so the breaker
/// reads a table instead of measuring. It charges `end + start` for a
/// candidate line. It takes `flex` off where the line would otherwise
/// overflow, or always where [`LineEdgeFlags::HANG_FORCED`] says. The
/// default is a boundary that pays nothing and gives nothing back.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) struct LineEdgeCost {
    /// The boundary, which the costs are sorted by.
    at: ClusterId,
    /// What a line ending here pays.
    pub(super) end: LayoutUnit,
    /// What a line starting here pays.
    pub(super) start: LayoutUnit,
    /// Of `end`, what the breaker may take back if the line fits without it.
    pub(super) flex: LayoutUnit,
    /// What else is true of it.
    pub(super) flags: LineEdgeFlags,
}

impl Keyed for LineEdgeCost {
    type Key = ClusterId;

    /// The boundary, which [`LineEdgeCosts`] is sorted by.
    #[inline]
    fn key(&self) -> ClusterId {
        self.at
    }
}

/// How wide the content asks to be, on layout's grid.
///
/// [`IntrinsicSizes`] hands these to the host in pixels.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) struct IntrinsicWidths {
    /// The widest run of content no line may break inside: min-content.
    min: LayoutUnit,
    /// The widest paragraph: max-content.
    max: LayoutUnit,
}

/// Intrinsic content widths for shrink-to-fit boxes, table cells and flex items.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
#[non_exhaustive]
pub struct IntrinsicSizes {
    /// The minimum nonoverflowing width in pixels: the widest unbreakable content.
    pub min_content: f32,
    /// The width in pixels with only forced breaks: the widest paragraph.
    pub max_content: f32,
}

impl From<IntrinsicWidths> for IntrinsicSizes {
    /// The sizes the measure stage found, in pixels.
    fn from(intrinsic: IntrinsicWidths) -> Self {
        Self {
            min_content: intrinsic.min.to_px(),
            max_content: intrinsic.max.to_px(),
        }
    }
}

/// What text with one row of facts measures in its primary font.
///
/// These are the facts the builder can't lower, since each needs the font.
/// The measure stage writes one row for every [`TextFactsId`] of the
/// content, the first line's included, so one table serves both variants.
/// Readers use [`Measured::text_metrics`]. A row is 48 bytes, and a document
/// has a few.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct TextMetrics {
    /// The strut: the primary font's ascent and descent around the used line
    /// height.
    ///
    /// The leading is split as Blink's `CalculateLeadingSpace` splits it. A
    /// text item, a `<br>` and a box's opening edge reach this far across
    /// the line. The block's strut is every line's. [`Extent::NONE`] where
    /// the text has no primary font.
    strut: Extent,
    /// The used line height, as Chrome's `ComputedLineHeightAsFixed` makes
    /// it.
    ///
    /// For `normal` it is the primary font's line spacing. For a number it
    /// is that factor of the computed size. A length is rounded onto the
    /// grid. A `vertical-align` percentage is of this height. Zero where the
    /// text has no primary font.
    line_height: LayoutUnit,
    /// Where the tabs stop: `tab-size` in the block container's spaces or as
    /// a length, and the block's letter-spacing.
    pub(super) tab: TabStops,
    /// The em box the emphasis marks are drawn in.
    ///
    /// It is the primary font's em box at half the computed size. The size
    /// rounds to a whole pixel (`lroundf`), the ascent to the nearest 1/64,
    /// and the descent is the rest of the em. Blink normalizes the font it
    /// draws marks in the same way (`SimpleFontData::EmphasisMarkFontData`).
    /// [`Extent::NONE`] where the text sets no marks.
    pub(crate) mark: Extent,
}

/// Measured text: the text's, or the first paragraph's under `::first-line`
/// styles.
///
/// The first paragraph's has the same tables over fewer clusters. Each table
/// has its own type and methods: the prefix, the paragraph flags, the
/// extents and strut, and the kept boxes. The tables only some content has
/// live in a box of their own ([`RareMeasures`]).
pub(crate) struct MeasuredText {
    /// The prefix advances.
    pub(crate) prefix: PrefixAdvances,
    /// What the measure stage found of each paragraph
    /// ([`paragraph`](Self::paragraph)).
    ///
    /// The breaker searches a paragraph whose prefix never goes back. It
    /// looks for line-edge costs and seams only where a paragraph has them.
    paragraphs: Table<ParagraphId, MeasureFlags>,
    /// Each item's extent, and the strut.
    pub(crate) extents: ItemExtents,
    /// The boxes that keep a fragment, with their fragments' extents.
    pub(crate) kept_boxes: KeptBoxes,
    /// The tables only some content has.
    ///
    /// The box is made the first time a build needs it, then cleared with
    /// the rest and kept, never dropped. Most layouts hold only the empty
    /// pointer.
    rare: Option<Box<RareMeasures>>,
}

/// The tables of a [`MeasuredText`] that only some content fills.
///
/// They are the line-edge costs, the baseline shifts, the text's em boxes
/// (for ruby or emphasis marks), the ruby columns and the block's initial
/// letter. Only the measure stage writes them. A text without them reads
/// [`RareMeasures::NONE`].
struct RareMeasures {
    /// The line-edge costs.
    edge_costs: LineEdgeCosts,
    /// The baseline shifts: how far each box or atomic inline is raised from
    /// its parent's baseline, positive up.
    ///
    /// They hold the shifts that don't depend on the line:
    /// `vertical-align: super`, `sub`, a length or a percentage, and
    /// `dominant-baseline` where the config applies it. The table is sorted
    /// by node and sparse: a node with no entry doesn't move
    /// ([`MeasuredText::shift`]). The breaker works out the shifts that
    /// depend on the line (`middle`, `text-top`, `text-bottom`, `top`,
    /// `bottom`) per line.
    shifts: SortedTable<(NodeId, LayoutUnit)>,
    em_boxes: ItemEmBoxes,
    rubies: RubyColumns,
    /// The block's initial letter, where it has one.
    initial_letter: Option<InitialLetter>,
}

impl RareMeasures {
    /// None of them: what a text reads where its build had none.
    const NONE: Self = Self::new();

    const fn new() -> Self {
        Self {
            edge_costs: LineEdgeCosts::new(),
            shifts: SortedTable::new(),
            em_boxes: ItemEmBoxes::new(),
            rubies: RubyColumns::new(),
            initial_letter: None,
        }
    }

    fn clear(&mut self) {
        self.edge_costs.clear();
        self.shifts.clear();
        self.em_boxes.clear();
        self.rubies.clear();
        self.initial_letter = None;
    }
}

impl Default for RareMeasures {
    fn default() -> Self {
        Self::new()
    }
}

/// What a text with none of the rare tables reads them as: empty.
static NO_RARE_MEASURES: RareMeasures = RareMeasures::NONE;

impl MeasuredText {
    const fn new() -> Self {
        Self {
            prefix: PrefixAdvances::new(),
            paragraphs: Table::new(),
            extents: ItemExtents::new(),
            kept_boxes: KeptBoxes::new(),
            rare: None,
        }
    }

    fn clear(&mut self) {
        self.prefix.clear();
        self.paragraphs.clear();
        self.extents.clear();
        self.kept_boxes.clear();
        if let Some(rare) = &mut self.rare {
            rare.clear();
        }
    }

    /// Its rare tables: empty where its build had none.
    #[inline]
    fn rare(&self) -> &RareMeasures {
        self.rare.as_deref().unwrap_or(&NO_RARE_MEASURES)
    }

    /// Returns its rare tables for the scan to write.
    ///
    /// The box is made, empty, the first time a build has any, and kept
    /// after.
    fn rare_mut(&mut self) -> &mut RareMeasures {
        self.rare.get_or_insert_with(Box::default)
    }

    /// What was found of `paragraph`, or nothing past the last.
    #[inline]
    pub(crate) fn paragraph(&self, paragraph: ParagraphId) -> MeasureFlags {
        self.paragraphs
            .get(paragraph)
            .copied()
            .unwrap_or(MeasureFlags::NONE)
    }

    /// Each text item's em box, where the content has ruby or marks.
    #[inline]
    pub(crate) fn em_boxes(&self) -> &ItemEmBoxes {
        &self.rare().em_boxes
    }

    /// Returns how far `node`'s baseline is raised from its parent's by
    /// what doesn't depend on its line.
    ///
    /// A negative shift lowers it. A node with no shift of its own gets
    /// zero. The lookup halves the sorted table.
    #[inline]
    pub(super) fn shift(&self, node: NodeId) -> LayoutUnit {
        self.shifts()
            .get(node)
            .map_or(LayoutUnit::ZERO, |&(_, shift)| shift)
    }

    /// The baseline shifts, sorted by node: none where no box has one.
    #[inline]
    pub(super) fn shifts(&self) -> &SortedTable<(NodeId, LayoutUnit)> {
        &self.rare().shifts
    }

    /// The line-edge costs: none where no line's edge costs anything.
    #[inline]
    pub(crate) fn edge_costs(&self) -> &LineEdgeCosts {
        &self.rare().edge_costs
    }

    /// The ruby columns and their levels.
    #[inline]
    pub(crate) fn ruby_columns(&self) -> &RubyColumns {
        &self.rare().rubies
    }

    /// The block's initial letter, as measured: `None` where it has none.
    #[inline]
    pub(crate) fn initial_letter(&self) -> Option<&InitialLetter> {
        self.rare().initial_letter.as_ref()
    }
}

impl FirstLineState for MeasuredText {
    fn clear(&mut self) {
        MeasuredText::clear(self);
    }

    /// Whether it holds no prefix: a variant the build had no use for.
    fn is_empty(&self) -> bool {
        self.prefix.is_empty()
    }
}

impl Default for MeasuredText {
    fn default() -> Self {
        Self::new()
    }
}

/// The block's initial letter as the measure stage sizes it.
///
/// It is the box that sets `initial-letter` at the block's start: its
/// `::first-letter`, or its first inline box. The box is fitted to the ink
/// of what it holds, as Chrome's `LayoutInitialLetterBox` is
/// (`initial_letter_utils`).
///
/// - **Along the line:** the box's content is the ink, in whole pixels as
///   Skia bounds a glyph. The ink starts at the content's start. So the room
///   its edges take is its margin, border and padding, less the part of the
///   letter's advance outside its ink. The prefix then holds the letter's
///   own advances and the box's margin box exactly.
/// - **Across the lines:** its baseline sits on the paragraph's `size`-th
///   line's baseline, counted from the first line's top. The box stands on
///   no line. It takes no room in the line it opens; that line moves down
///   for a raised letter instead. The lines below give it room as an
///   exclusion the host places.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct InitialLetter {
    /// The box.
    pub(crate) node: NodeId,
    /// What its opening and closing edges take along the line in the prefix
    /// ([`edge_room`](Self::edge_room)).
    ///
    /// Each is the margin, border and padding at that end, less the
    /// letter's advance outside its ink. The start margin is kerned.
    start_room: InlineLayoutUnit,
    end_room: InlineLayoutUnit,
    /// Its margins at its start and its end.
    ///
    /// A negative side bearing kerns the start margin where the box has no
    /// border or padding (Chrome's "inline kerning").
    pub(super) start_margin: LayoutUnit,
    pub(super) end_margin: LayoutUnit,
    /// Its border box around the letter's baseline.
    extent: Extent,
    /// How far the letter's baseline is below the top of the first line's
    /// box, before a raised letter moves the line's text down.
    pub(super) baseline: LayoutUnit,
    /// How far a raised letter moves the first line's text down: the lines
    /// it stands above the paragraph by (`size` less `sink`).
    pub(super) raise: LayoutUnit,
    /// Its margin box, as the exclusion the lines after the first make room
    /// for.
    ///
    /// The top is measured across the lines from the first line's top. The
    /// height is how far it reaches, and the width how wide it is along the
    /// lines.
    pub(super) exclusion_top: LayoutUnit,
    pub(super) exclusion_height: LayoutUnit,
    pub(super) exclusion_width: LayoutUnit,
}

impl InitialLetter {
    /// Returns the room the prefix gave its opening edge (`opens`) or its
    /// closing one.
    pub(crate) fn edge_room(&self, opens: bool) -> InlineLayoutUnit {
        if opens {
            self.start_room
        } else {
            self.end_room
        }
    }
}

/// The measurements: the text as measured, its first line's variant, and
/// what is measured once for both.
pub(crate) struct Measured {
    /// The text, and the first line's measure.
    ///
    /// The first line's measure is the first paragraph under `::first-line`
    /// styles, where they change a measurement. It covers the same clusters
    /// and items as the text, up to the paragraph's end. It is absent when
    /// the first line restyles nothing. Once made it is kept, so a document
    /// that gains and loses `::first-line` keeps its capacity, as shaping's
    /// variant does.
    text: FirstLine<MeasuredText>,
    /// What each text's facts measure in its primary font, whichever line
    /// it is on: one row a [`TextFactsId`], both variants' rows among them.
    text_metrics: Table<TextFactsId, TextMetrics>,
    /// The hyphens and ellipses, shaped, whichever line they are on.
    ///
    /// The box is made the first time a build has any, and kept, as the
    /// text's rare tables are.
    generated: Option<Box<GeneratedTexts>>,
    /// How wide the content asks to be.
    pub(crate) intrinsic: IntrinsicWidths,
    /// Which clusters took word-spacing in the prefix, as the config said at
    /// build time.
    ///
    /// The breaker spaces a reshaped edge by the same rule.
    pub(super) word_spacing_rule: WordSpacingRule,
}

/// What measurements with no generated text read it as: empty.
static NO_GENERATED: GeneratedTexts = GeneratedTexts::new();

impl Measured {
    /// No measurements, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            text: FirstLine::new(MeasuredText::new()),
            text_metrics: Table::new(),
            generated: None,
            intrinsic: IntrinsicWidths {
                min: LayoutUnit::ZERO,
                max: LayoutUnit::ZERO,
            },
            word_spacing_rule: WordSpacingRule::CHROME,
        }
    }

    /// Empties the measurements, keeping every allocation. Also what a layout
    /// holds when its builder was dropped without finishing.
    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.text_metrics.clear();
        if let Some(generated) = &mut self.generated {
            generated.clear();
        }
        self.intrinsic = IntrinsicWidths::default();
        self.word_spacing_rule = WordSpacingRule::CHROME;
    }

    /// Returns the generated texts for the measure stage to write.
    ///
    /// The box is made, empty, the first time a build has any, and kept
    /// after.
    fn generated_mut(&mut self) -> &mut GeneratedTexts {
        self.generated
            .get_or_insert_with(|| Box::new(GeneratedTexts::new()))
    }

    /// Empties the measurements and hands back the buffers the prefixes
    /// were kept in.
    ///
    /// Shaping fills them with the next build's advances, and
    /// [`measure_text`] takes them back. One buffer each goes round, so a
    /// warm rebuild allocates none.
    pub(crate) fn recycle(&mut self) -> Advances {
        self.clear();
        let text = mem::take(&mut self.text.text_mut().prefix.sums);
        let first_line = self
            .text
            .kept_first_line_mut()
            .map_or_else(Vec::new, |first| mem::take(&mut first.prefix.sums));
        Advances::new(text, first_line)
    }

    /// Measures atomic inline `atomic`'s extent again from its row in
    /// `content`, as the scan measures it.
    ///
    /// The content holds no ruby and no initial letter, so an atomic
    /// inline's extent reaches its own item and nothing else, in the text's
    /// measure and the first line's. What runs along the line is unchanged.
    pub(crate) fn remeasure_atomic(&mut self, content: &Content, atomic: AtomicId) {
        let Some(atomic) = content.atomics().get(atomic) else {
            return;
        };
        let baseline = LineBaseline::from_content(content);
        let extent = Extent::from_atomic(atomic, content.block.writing_mode, baseline);
        self.text.text_mut().extents.set(atomic.item, extent);
        if let Some(first_line) = self.text.kept_first_line_mut() {
            first_line.extents.set(atomic.item, extent);
        }
    }

    /// Returns the first line's measure: the first paragraph under
    /// `::first-line` styles.
    ///
    /// Returns `None` where `::first-line` restyles nothing the lines are
    /// measured by.
    pub(crate) fn first_line(&self) -> Option<&MeasuredText> {
        self.text.first_line()
    }

    /// Returns the text as `variant` reads it.
    ///
    /// The first line reads the first line's measure where there is one.
    /// Everything else reads the text's.
    #[inline]
    pub(crate) fn text(&self, variant: FirstLineVariant) -> &MeasuredText {
        self.text.get(variant)
    }

    /// Returns what text with the facts `text` measures in its primary font.
    ///
    /// See [`TextMetrics`]. Returns `None` for facts the content doesn't
    /// have.
    #[inline]
    pub(crate) fn text_metrics(&self, text: TextFactsId) -> Option<&TextMetrics> {
        self.text_metrics.get(text)
    }

    /// Returns the shaped hyphens and ellipses the lines may draw.
    ///
    /// The text and its first line share them. Each row of text facts has
    /// its own. They are empty where the build had none.
    pub(crate) fn generated(&self) -> &GeneratedTexts {
        self.generated.as_deref().unwrap_or(&NO_GENERATED)
    }

    /// Returns the bytes the first line's measure takes on the heap.
    ///
    /// [`heap_bytes`](crate::data::HeapBytes::heap_bytes) counts them among
    /// the stage's.
    pub(crate) fn first_line_heap_bytes(&self) -> usize {
        self.text.first_line_heap_bytes()
    }
}

heap_bytes! {
    /// Its texts' tables, and the generated texts' box and theirs where a
    /// build made them.
    Measured { text, text_metrics, generated; intrinsic, word_spacing_rule }
}

heap_bytes! {
    RareMeasures { edge_costs, shifts, em_boxes, rubies; initial_letter }
}

heap_bytes! {
    /// Its tables, and its rare tables' box and theirs where a build made
    /// them.
    MeasuredText { prefix, paragraphs, extents, kept_boxes, rare }
}

heap_bytes! {
    PrefixAdvances { sums }
}

heap_bytes! {
    ItemExtents { extents; strut }
}

heap_bytes! {
    ItemEmBoxes { items, boxes }
}

heap_bytes! {
    KeptBoxes { boxes }
}

heap_bytes! {
    RubyColumns { columns, levels, steps, floors, rooms, tab_reaches }
}

heap_bytes! {
    GeneratedTexts { texts, glyphs }
}

impl Default for Measured {
    fn default() -> Self {
        Self::new()
    }
}

// The scan's state and what its files share: `scan` drives it, and
// `scan_walk`, `scan_ends`, `scan_ruby`, `scan_em_boxes`, `scan_initial_letter`
// and `scan_intrinsic` each add a part of it.

/// What one variant's scan reads, fixed for that variant.
///
/// It holds the stages before measurement, the variant, where the scan
/// stops, the text metrics and hyphens made earlier in the stage, and the
/// block's initial letter. Its `has_*` gates say which phases the content
/// needs at all. The walk ([`ScanWalk`]) moves and this doesn't, so a
/// look-ahead copies the walk alone.
struct Scan<'a> {
    input: &'a MeasureInput<'a>,
    content: &'a Content,
    analysis: &'a Analysis,
    fonts: &'a Fonts,
    /// Whose styles the text is measured in.
    variant: FirstLineVariant,
    /// The variant's shaping: the glyphs, which say which clusters continue
    /// a ligature, and the shaping runs the walk holds.
    shaped: &'a ShapedText,
    /// The text its clusters are read in.
    source: VariantText<'a>,
    /// Where the scan stops: the text's end, or the first paragraph's.
    end: ClusterId,
    writing_mode: WritingMode,
    /// The baseline the lines are set on. An atomic inline without a
    /// baseline of its own sits on it.
    baseline: LineBaseline,
    /// Which clusters take word-spacing: the build's rule.
    words: WordSpacingRule,
    /// The hyphens, shaped, which a line ending at a soft hyphen pays.
    generated: &'a GeneratedTexts,
    /// What each text's facts measure in its primary font.
    metrics: &'a Table<TextFactsId, TextMetrics>,
    /// The block's initial letter.
    ///
    /// Its box's edges take what its measure says, and its items take no
    /// room across the line. The look-ahead that measures it reads a scan
    /// with none.
    letter: Option<InitialLetter>,
    /// Where `text-autospace` puts room.
    ///
    /// The scan looks for seams only where some style sets it, and then only
    /// in paragraphs with an ideograph.
    autospace: AutospaceRules,
    /// The text has a tab. Tab stops are sized only then.
    has_tabs: bool,
    /// Some box has an edge that takes room. Without one, no boundary takes
    /// any.
    has_edges: bool,
    /// Some box with an edge clones its decoration where it breaks.
    has_clones: bool,
    /// Some style spaces its letters or words. Without one, the scan works
    /// out no spacing.
    has_spacing: bool,
    /// Some style sets line padding. The scan tracks it only then.
    has_padding: bool,
    /// Some style hangs punctuation. The scan tracks it only then.
    has_hanging: bool,
    /// The content has ruby. The scan looks for columns and their overhang
    /// only then.
    has_ruby: bool,
    /// Some text is combined (`text-combine-upright`). Combined text takes
    /// no spacing and ends every autospace seam.
    has_combined: bool,
    /// The plain fast path is open ([`plain_fast_path`](Self::plain_fast_path)).
    ///
    /// No style sets anything that a cluster inside a text item could pay
    /// for beyond its advance.
    plain_fast_path_open: bool,
}

/// The scan's walk over the text, and the state that moves with it.
///
/// It holds:
/// - the segment holding the cluster after the scan's boundary, with its
///   item's text facts;
/// - cursors on the atomic inlines and floats;
/// - the edges of the cloned boxes open;
/// - the last leaf passed, for a ruby column's overhang.
///
/// It is `Copy`. A look-ahead over a ruby column or the initial letter
/// moves a copy, leaving the scan's own in place. Both read the same
/// [`Scan`].
#[derive(Copy, Clone)]
struct ScanWalk<'a> {
    /// The walk over the segments and the items between them.
    segments: Segments<'a>,
    /// What the walk yields next. The scan takes it when it reaches its
    /// boundary.
    ahead: Option<Step>,
    /// The segment holding the clusters from the scan's boundary to
    /// `until`.
    segment: Option<Segment>,
    /// The segment's item row, read where the scan enters the item.
    item: Option<&'a Item>,
    /// The segment's clusters are combined text.
    ///
    /// The scan reads the script run's orientation on entering the segment,
    /// and only where some text is combined.
    combined: bool,
    /// The autospace facts and direction, resolved on entering a segment.
    seam: Option<SeamContext>,
    /// Where the segment ends. No item sits at a boundary before it.
    until: ClusterId,
    /// The text facts of the segment's item, in the variant.
    text: TextFactsId,
    /// The row `text` names.
    facts: &'a TextFacts,
    /// What the item's text adds after each of its clusters.
    ///
    /// This holds on whichever line the clusters are set: an annotation's
    /// own or the base's. It is zero unless some style spaces text.
    spacing: LetterWordSpacing,
    /// The width of the U+FFFC where the item is an atomic inline: its
    /// margin box.
    atomic: Option<InlineLayoutUnit>,
    /// The edges of the cloned boxes open so far.
    cloned: EdgeAmounts,
    /// The next atomic inline not passed.
    atomics: AtomicId,
    /// The next float not passed.
    floats: FloatId,
    /// The extent of the text item the scan is in under `line-height:
    /// normal`, and the last font that widened it.
    ///
    /// The font of each of the item's segments widens it. The scan pushes it
    /// once it leaves the item.
    pending: Option<(Extent, UsedFontId)>,
    /// The last thing the scan passed that holds a place, for a ruby
    /// column's overhang.
    leaf: OverhangNeighbour,
    /// The item holding the cluster after the scan's boundary, as a leaf,
    /// and its first cluster.
    ///
    /// Once the scan is past that cluster, the item becomes `leaf`. So a
    /// ruby column starting at the boundary reads the leaf before it, not
    /// its own base's first item.
    entered: Option<(ClusterId, OverhangNeighbour)>,
    /// How many open inline boxes have struts that reach past the block's.
    ///
    /// Each paragraph where it is above zero gets `BOXES_PAST_STRUT`.
    past_strut: u32,
}

/// Amounts paid at a line's two ends: `start` by a line starting at a
/// boundary, `end` by one ending there.
///
/// They hold the edges of the cloned boxes open across a break, and the
/// line padding at each end. What the intrinsic sizes pay also gives back
/// the marks that hang there.
#[derive(Copy, Clone, Debug, Default)]
struct EdgeAmounts {
    start: InlineLayoutUnit,
    end: InlineLayoutUnit,
    /// What a min-content line ending here gives back beyond `end`.
    ///
    /// It is a mark that hangs only where the line would not fit with it
    /// (`allow-end`), which max-content counts.
    min_end_hang: InlineLayoutUnit,
}

/// The room the items at one boundary take along the line.
///
/// The edges that end the line before a break there ([`BoundarySplit`])
/// come before the prefix entry there. The rest come after it. A ruby
/// column ending there adds its width past its base to the leading closes.
#[derive(Copy, Clone, Debug, Default)]
struct BoundaryRoom {
    /// The boundary is inside a whole ruby column, so no line edge falls
    /// here.
    inside_column: bool,
    /// The edges that end the line before a break here.
    leading_closes: InlineLayoutUnit,
    /// The edges that start the line after a break here.
    rest: InlineLayoutUnit,
    /// Where a break falls among the items.
    split: BoundarySplit,
    /// The opening item of a ruby container that opens here.
    ruby: Option<ItemId>,
    /// The edges of the cloned boxes open across a break here.
    ///
    /// A break falls where [`BoundarySplit`] places it.
    across: EdgeAmounts,
    /// How far the ruby column ending here overhangs the text before it,
    /// and the one starting here the text after it.
    ///
    /// A line that ends or starts here has no such text, so it pays the
    /// overhang.
    overhang: EdgeAmounts,
    /// The floats anchored here. They take no room along the line and sit
    /// beside it.
    floats: AnchoredFloats,
}

impl EdgeAmounts {
    /// Returns whether both amounts are zero.
    fn is_empty(self) -> bool {
        self.start == InlineLayoutUnit::ZERO && self.end == InlineLayoutUnit::ZERO
    }
}

impl BoundaryRoom {
    /// Returns no room at boundary `at`, before any of its items.
    fn from_boundary(analysis: &Analysis, at: ClusterId) -> Self {
        Self {
            split: BoundarySplit::new(analysis, at),
            ..Self::default()
        }
    }

    /// Adds the room `item` takes.
    ///
    /// Annotation items, the annotation's own marks included, take nothing
    /// on the base's line. Floats and empty text take nothing and leave the
    /// leading closes leading. So a float anchored between two closing
    /// edges doesn't part them.
    fn add(&mut self, content: &Content, letter: Option<&InitialLetter>, item: &Item) {
        if item.flags.contains(ItemFlags::ANNOTATION) {
            return;
        }
        let kind = item.kind;
        let (before, after) = (&mut self.leading_closes, &mut self.rest);
        if kind.is_open() || kind.is_close() {
            let room = edge_room(content, letter, item, kind.is_open());
            self.split.take(kind, room, before, after);
        } else if kind == ItemKind::Absolute {
            // An anchor takes no room, but may hold what follows it on the
            // line after.
            self.split.take(kind, InlineLayoutUnit::ZERO, before, after);
        }
    }
}
