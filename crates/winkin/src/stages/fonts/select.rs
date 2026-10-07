//! Chooses a font for each cluster and writes the font runs.
//!
//! The walk goes in text order, one **span** at a time. A span is the
//! clusters of one text item within one script run, so it has one style and
//! one pair of candidate lists. Each cluster is handled by the first rule
//! that applies:
//!
//! 1. A cluster that draws nothing takes the font of the cluster before it.
//!    Tabs, separators, an atomic inline's U+FFFC and a lone
//!    default-ignorable draw nothing.
//! 2. A grapheme that a style boundary divides follows Chrome. If the parts
//!    shape alike (one [`ShapingFactsId`]), the first part chooses with the
//!    whole grapheme's text and the later parts take its font. Otherwise each
//!    part chooses by its own style, and a mark may leave its base.
//! 3. The cluster takes the first candidate, in CSS order, that the memo does
//!    not rule out, that accepts the presentation and that covers the grapheme.
//!    A fallback font's run ends where an earlier font covers again, so a CJK
//!    fallback takes no digits or spaces from the family the text asked for.
//! 4. If no matched font covers it, the walk matches the next family. Once
//!    every family is matched, it walks fontwich's fallback for each missing
//!    character, once per list and character. A character that no installed
//!    family maps is noted once per context and never walked again. Then it
//!    tries a forced presentation's base form, then the candidate that draws
//!    most of it, and last the primary font, which draws `.notdef` as Chrome
//!    does.
//! 5. A grapheme holding a variation sequence first looks over the whole list for
//!    a font that also has the sequence. Only if none has it does it take
//!    the first font that covers the base. Blink matches clusters in these
//!    same two passes.
//! 6. The chosen font gives a used font: the instance for the style's
//!    variations and features at its used size. If the font lacks small
//!    capitals the style asks for, the text splits where Blink's
//!    `SmallCapsIterator` splits it, and the uppercased side gets a smaller
//!    used font. So a font run also ends where the case changes.
//! 7. Under `font-variant-position: super` or `sub` that may be synthesized,
//!    the cluster's font waits for the end of its **position run**. The
//!    feature is used only if the fonts' `sups` or `subs` covers every
//!    character of the run (CSS Fonts 4, section 6.5). Otherwise the whole
//!    run is synthesized (see `positions`). The run spans script runs and
//!    inline box edges while the text shapes alike.
//!
//! The **uncovered memo** keeps, per candidate, the next cluster it covers.
//! A candidate that misses scans ahead once and is skipped until then. So
//! text that only the k-th candidate covers costs one bit test per
//! character, not k. The memo records coverage of the base characters, so a
//! candidate that lacks a variation sequence stays in for the second pass.
//!
//! The walk reads the context's candidate lists without changing them. When
//! it needs a change (a family matched, a fallback walked, a font found
//! unusable, a position feature's coverage read), it stops and returns the
//! change. The caller applies it and restarts the walk at that cluster with
//! the memo kept. Each change happens once per context, so a warm build
//! never stops.

use alloc::vec::Vec;
#[cfg(test)]
use core::cell::Cell;
use core::mem;
use core::ops::Range;

use fontwich::{Charset, CharsetPage, FaceId, Family, Font, Presentation};
use hashbrown::HashTable;

use super::FontInput;
use super::context::{CacheChange, FontCaches, FontContext};
use super::coverage;
use super::features::{self, CapsPlan, CaseSide, OfferedFeatures};
use super::instance::variations_left_to_matching;
use super::instance::{FaceOverrides, InstanceRequest};
use super::lists::{
    Candidate, CandidateId, CandidateList, CandidateListId, CandidateLists, MatchRequest,
    NamedListId,
};
use super::positions::FeatureCoverages;
use super::used::{UsedFontInterner, used_size};
use super::{
    CaseMap, DefaultLanguage, FontResolution, FontRun, FontRunId, FontRuns, UsedFontId,
    UsedSynthesis,
};
use crate::config::PositionSynthesis;
use crate::data::{HeapBytes, Id, IdRange, Table, hash_one, heap_bytes};
use crate::stages::analysis::{ClusterClass, ClusterId, Clusters, ScriptRunId};
use crate::stages::content::{
    FamilyListId, FontKey, FontRequest, FontRequestId, ItemId, ItemKind, NodeId, ShapingFactsId,
    TextCursor, TextFactsId,
};
use crate::style::{FirstLineVariant, FontVariantCaps, FontVariantEmoji, FontVariantPosition};
use crate::unicode::is_variation_selector;
use crate::unicode::{self, GraphemeClusterBreak};
use crate::unit::{self, LayoutUnit};
use crate::work;

/// How many times one span's walk may stop for a change before the rest
/// of it is set in its primary font.
///
/// Each change happens once per context, a few per family, so this is never
/// reached. It only guarantees that no input makes the loop endless.
pub(super) const MAX_STOPS: u32 = 1 << 16;

/// How many candidates of a list keep their last [`CharsetPage`] while a
/// span is walked. Past these a candidate searches its charset once a
/// character; the first covers nearly everything.
const PAGES: usize = 8;

/// The charset page each of a list's first [`PAGES`] candidates last found.
///
/// Kept while a span is walked, so text in one candidate costs a bit test
/// per character, and a search only where the text leaves the page.
struct Pages<'a>([Option<CharsetPage<'a>>; PAGES]);

impl<'a> Pages<'a> {
    /// None found yet.
    const fn new() -> Self {
        Self([None; PAGES])
    }

    /// Where `candidate` keeps the page it last found, or `None` for a
    /// candidate past the first [`PAGES`], which keeps none.
    fn get_mut(&mut self, candidate: CandidateId) -> Option<&mut Option<CharsetPage<'a>>> {
        self.0.get_mut(candidate.get())
    }

    /// The page `candidate` last found, if it keeps one.
    fn get(&self, candidate: CandidateId) -> Option<Option<CharsetPage<'a>>> {
        self.0.get(candidate.get()).copied()
    }
}

/// The run loop's inputs besides the context.
///
/// It holds the stage's input, the variant being chosen for, and what the
/// stage resolved before any run.
pub(super) struct Input<'a> {
    pub(super) stage: &'a FontInput<'a>,
    /// The text's or the first line's. The variant picks the nodes' facts
    /// and the characters drawn, which a first-line transform may change.
    pub(super) variant: FirstLineVariant,
    /// Each of the content's family lists, as the context names it.
    pub(super) lists: &'a Table<FamilyListId, NamedListId>,
    /// What each font request resolves to.
    pub(super) resolutions: &'a Table<FontRequestId, FontResolution>,
    /// The text the clusters are chosen for in the variant, read in order
    /// across the spans.
    pub(super) source: &'a TextCursor<'a>,
}

impl<'a> Input<'a> {
    /// The text facts `node` has in the variant.
    fn text(&self, node: NodeId) -> TextFactsId {
        self.stage.content.nodes.text_facts(node, self.variant)
    }

    /// The font request text with the text facts `text` is set in.
    pub(super) fn request(&self, text: TextFactsId) -> FontRequestId {
        self.stage.content.facts.text_request(text)
    }

    /// The primary font of `request`, or the first used font for a request
    /// the content does not have, which none is.
    pub(super) fn primary(&self, request: FontRequestId) -> UsedFontId {
        self.resolutions
            .get(request)
            .map_or(UsedFontId::new(0), |resolution| resolution.primary)
    }

    /// The language text whose style names none chooses its fonts in.
    pub(super) fn default_language(&self) -> DefaultLanguage {
        DefaultLanguage::new(self.stage.default_language)
    }
}

/// The run loop's working memory, in the stage's scratch.
///
/// It holds nothing between calls and keeps its capacity.
pub(super) struct RunScratch {
    /// What a span's walk keeps while it tries candidates.
    pub(super) walk: SpanScratch,
    /// The run of text in a `font-variant-position` whose fonts wait on
    /// whether the feature covers all of it.
    position: PositionRun,
}

impl RunScratch {
    /// Empty scratch, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            walk: SpanScratch::new(),
            position: PositionRun::new(),
        }
    }

    /// Forgets everything, keeping the capacity.
    pub(super) fn clear(&mut self) {
        self.walk.clear();
        self.position.clear();
    }
}

/// What a span's walk keeps while it tries candidates.
pub(super) struct SpanScratch {
    /// The uncovered memo: per candidate, the next cluster it covers, for
    /// the span's text and emoji lists. Zero means unknown.
    memo: [Table<CandidateId, ClusterId>; 2],
    /// Whether the span's last asking cluster changes when uppercased.
    ///
    /// A cluster that starts with a mark takes this value, as Blink's
    /// `SmallCapsIterator` passes over marks. It survives the walk's stops,
    /// like the memo, so a restart at a mark gives the same answer.
    upper: bool,
    /// The used fonts each candidate gives each request under each script,
    /// found once per build. Many boxes in a few styles ask for the same
    /// choice again and again.
    pub(super) choices: Choices,
}

impl SpanScratch {
    /// Empty, allocating nothing.
    const fn new() -> Self {
        Self {
            memo: [Table::new(), Table::new()],
            upper: false,
            choices: Choices::new(),
        }
    }

    /// Forgets everything, keeping the capacity.
    fn clear(&mut self) {
        self.start_segment();
        self.choices.clear();
    }

    /// Forgets what the walk of the span before kept: the memo and the
    /// side of case.
    fn start_segment(&mut self) {
        for memo in &mut self.memo {
            memo.clear();
        }
        self.upper = false;
    }
}

/// The choices a build has made, and a log of those the text's walk asked
/// for over the first line's reach.
pub(super) struct Choices {
    table: HashTable<(u64, ChoiceKey, Choice)>,
    /// Each choice the text's walk asked for before `logged_until`, with
    /// its cluster, in order. [`map_runs`] replays it for the first line.
    log: Vec<(ClusterId, ChoiceKey)>,
    logged_until: Option<ClusterId>,
}

impl Choices {
    /// None, allocating nothing.
    const fn new() -> Self {
        Self {
            table: HashTable::new(),
            log: Vec::new(),
            logged_until: None,
        }
    }

    /// Forgets them all, keeping the capacity.
    fn clear(&mut self) {
        self.table.clear();
        self.log.clear();
        self.logged_until = None;
    }

    /// The choice made for `key` this build, if one was.
    fn find(&self, key: ChoiceKey) -> Option<Choice> {
        let hash = hash_one(&key);
        self.table
            .find(hash, |&(h, held, _)| h == hash && held == key)
            .map(|&(.., choice)| choice)
    }

    /// Returns the choice made for `key` this build, or makes it with
    /// `choose` and keeps it.
    ///
    /// If `choose` fails because the candidate's bytes are gone, returns the
    /// change that marks it unusable. The walk, generated text and the first
    /// line's mapping all make choices here.
    pub(super) fn find_or_choose(
        &mut self,
        key: ChoiceKey,
        choose: impl FnOnce() -> Option<Choice>,
    ) -> Result<Choice, CacheChange> {
        let hash = hash_one(&key);
        if let Some(&(.., choice)) = self
            .table
            .find(hash, |&(h, held, _)| h == hash && held == key)
        {
            return Ok(choice);
        }
        let choice = choose().ok_or(CacheChange::Unusable(key.list, key.candidate))?;
        self.table
            .insert_unique(hash, (hash, key, choice), |&(h, _, _)| h);
        Ok(choice)
    }

    /// Logs that the walk asked for `key` at `cluster`, if `cluster` is
    /// before the logged end.
    fn log(&mut self, cluster: ClusterId, key: ChoiceKey) {
        if self.logged_until.is_some_and(|end| cluster < end) {
            self.log.push((cluster, key));
        }
    }
}

/// What a [`Choice`] is made for: a font request's text, under a script,
/// in a candidate of a list.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) struct ChoiceKey {
    request: FontRequestId,
    script: [u8; 4],
    list: CandidateListId,
    candidate: CandidateId,
}

/// The used font a cluster is set in, or the two it waits between.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Placed {
    /// This one, whatever else the run holds.
    Font(UsedFontId),
    /// `feature`, shaping with the position's feature, where the position
    /// run's fonts have the feature's glyph for every character of it, and
    /// `synthesized` otherwise.
    Either {
        feature: UsedFontId,
        synthesized: UsedFontId,
    },
}

impl Placed {
    /// The used font, where the position run holding it is `covered` or
    /// not.
    fn font(self, covered: bool) -> UsedFontId {
        match self {
            Self::Font(font) => font,
            Self::Either { feature, .. } if covered => feature,
            Self::Either { synthesized, .. } => synthesized,
        }
    }
}

/// The position run being gathered (see the module documentation).
///
/// Its clusters' fonts wait on whether every character has the feature's
/// glyph, which only the run's end tells. It lives in the stage's scratch,
/// so its capacity is kept.
struct PositionRun {
    /// The shaping facts of the text it was opened in, while one is open:
    /// it goes on while the text shapes alike.
    shaping: Option<ShapingFactsId>,
    /// Every character that draws has the feature's glyph, so far.
    covered: bool,
    /// Each group of its clusters set alike, from its first cluster.
    pieces: Vec<(ClusterId, Placed)>,
}

impl PositionRun {
    /// None open, allocating nothing.
    const fn new() -> Self {
        Self {
            shaping: None,
            covered: true,
            pieces: Vec::new(),
        }
    }

    /// Forgets the run, keeping the capacity.
    fn clear(&mut self) {
        self.shaping = None;
        self.covered = true;
        self.pieces.clear();
    }

    /// Whether a character is still to be asked about: a run is open, and
    /// nothing in it has been found wanting.
    fn asks(&self) -> bool {
        self.shaping.is_some() && self.covered
    }

    /// A character of the run has no glyph of the feature, or no glyph at
    /// all: the run is synthesized.
    fn uncover(&mut self) {
        self.covered = false;
    }
}

heap_bytes! {
    RunScratch { walk, position }
}

impl HeapBytes for SpanScratch {
    fn heap_bytes(&self) -> usize {
        let Self {
            memo,
            upper: _,
            choices,
        } = self;
        let memo: usize = memo.iter().map(HeapBytes::heap_bytes).sum();
        memo + choices.heap_bytes()
    }
}

heap_bytes! {
    Choices { table, log; logged_until }
}

heap_bytes! {
    PositionRun { pieces; shaping, covered }
}

/// What the loop writes: the runs, and the faces wanted, with the position
/// run whose clusters wait for its end.
struct Output<'a> {
    runs: &'a mut FontRuns,
    wanted: &'a mut Vec<FaceId>,
    /// The font the last cluster was given.
    last: Option<Placed>,
    position: &'a mut PositionRun,
}

impl<'a> Output<'a> {
    /// Writing into `runs` and `wanted`, gathering a position run in
    /// `position`.
    fn new(
        runs: &'a mut FontRuns,
        wanted: &'a mut Vec<FaceId>,
        position: &'a mut PositionRun,
    ) -> Self {
        position.clear();
        Self {
            runs,
            wanted,
            last: None,
            position,
        }
    }

    /// `cluster` is set as `placed` says: now, or, in a position run, when
    /// the run ends.
    fn push(&mut self, cluster: ClusterId, placed: Placed) {
        if self.position.shaping.is_some() {
            if self.position.pieces.last().map(|&(_, held)| held) != Some(placed) {
                self.position.pieces.push((cluster, placed));
            }
        } else {
            debug_assert!(
                matches!(placed, Placed::Font(_)),
                "only a position run waits"
            );
            set_from(self.runs, cluster, placed.font(false));
        }
        self.last = Some(placed);
    }

    /// `cluster` draws nothing, and takes the last cluster's font, or
    /// `primary` where there is none.
    fn push_nothing(&mut self, cluster: ClusterId, primary: UsedFontId) {
        self.push(cluster, self.last.unwrap_or(Placed::Font(primary)));
    }

    /// `cluster` is set in `primary` for want of any font that draws it,
    /// which draws `.notdef`: no variant glyph is to be had for it either.
    fn push_missing(&mut self, cluster: ClusterId, primary: UsedFontId) {
        self.position.uncover();
        self.push(cluster, Placed::Font(primary));
    }

    /// Text that shapes as `shaping` says, whose `font-variant-position`
    /// may be synthesized, comes next: it goes on the run open for text
    /// that shapes alike, or ends that run and opens one.
    fn open_position(&mut self, shaping: ShapingFactsId) {
        if self.position.shaping != Some(shaping) {
            self.close_position();
            self.position.shaping = Some(shaping);
            self.position.covered = true;
        }
    }

    /// Ends the position run, if one is open: each of its clusters is set
    /// in the font shaping with the feature where the run is covered, and
    /// in the synthesized one otherwise.
    fn close_position(&mut self) {
        if self.position.shaping.take().is_none() {
            return;
        }
        let covered = self.position.covered;
        for &(cluster, placed) in &self.position.pieces {
            set_from(self.runs, cluster, placed.font(covered));
        }
        self.position.pieces.clear();
        self.position.covered = true;
        if let Some(last) = &mut self.last {
            *last = Placed::Font(last.font(covered));
        }
    }
}

/// `cluster` and those after it are set in `font`, in `runs`, from now until
/// another run starts: a run, unless the last is in `font` already.
fn set_from(runs: &mut FontRuns, cluster: ClusterId, font: UsedFontId) {
    if runs.last().map(|run| run.font) != Some(font) {
        let pushed = runs.push(FontRun {
            start: cluster,
            font,
        });
        debug_assert!(pushed.is_some(), "no more runs than clusters");
    }
}

/// Whether `request`'s `font-variant-position` is synthesized where the
/// fonts do not cover a run of it: `super` or `sub`, under
/// [`PositionSynthesis::SynthesizeMissing`], where `font-synthesis-position` allows.
fn decides_position(request: &FontRequest, synthesis: PositionSynthesis) -> bool {
    request.font.variant_position != FontVariantPosition::Normal
        && synthesis == PositionSynthesis::SynthesizeMissing
        && request.font.synthesis.position
}

/// Adds pending `face`, which wants a character of a cluster, to the faces
/// the host fetches. Each face goes in once.
fn want(wanted: &mut Vec<FaceId>, face: FaceId) {
    if !wanted.contains(&face) {
        wanted.push(face);
    }
}

/// A span: the clusters of one text item in one script run.
///
/// It carries the candidate lists they try and what they ask of a font.
struct SelectionSpan<'a> {
    end: ClusterId,
    /// Where its item's graphemes cross the item's edges (see
    /// [`ItemGraphemes`]).
    graphemes: ItemGraphemes,
    /// Its candidate lists for text and for emoji.
    text: CandidateListId,
    emoji: CandidateListId,
    resolved: ResolvedRequest<'a>,
}

/// What a font request's text under a script asks of a font, whichever
/// list the font comes from. [`choose`] reads it.
#[derive(Copy, Clone)]
pub(super) struct ResolvedRequest<'a> {
    /// The request, whose variations and features key each instance it
    /// chooses, and its id.
    pub(super) request: &'a FontRequest,
    pub(super) request_id: FontRequestId,
    /// What it resolves to: its sizes, whether it is set upright, and its
    /// primary font, which draws what no font covers, and stands in where
    /// the used fonts fill.
    pub(super) resolution: FontResolution,
    /// The OpenType script its fonts' capitals and positions are asked for
    /// under.
    pub(super) script: [u8; 4],
}

impl ResolvedRequest<'_> {
    /// Returns the key of this request's choice in `candidate` of `list`.
    pub(super) fn key(&self, list: CandidateListId, candidate: CandidateId) -> ChoiceKey {
        ChoiceKey {
            request: self.request_id,
            script: self.script,
            list,
            candidate,
        }
    }
}

/// The used fonts a candidate sets a span's text in: one, or, where its
/// capitals are synthesized, one for each side of a change of case; and,
/// where a position run decides, the synthesized pair besides.
#[derive(Copy, Clone, Debug)]
pub(super) struct Choice {
    /// For text that does not change when uppercased.
    pub(super) same: UsedFontId,
    /// For text that does: `same` where the two are set alike.
    upper: UsedFontId,
    /// How the candidate sets text in a `font-variant-position`.
    position: PositionPlan,
}

/// How a candidate sets the text of a style whose `font-variant-position`
/// may be synthesized.
#[derive(Copy, Clone, Debug)]
enum PositionPlan {
    /// There is nothing to decide: `same` and `upper` set the text, with the
    /// feature the style asks for, if any, as Chrome sets it.
    Settled,
    /// The font lacks the feature under the script: `same` and `upper` are
    /// synthesized, and so is every run that holds text in the font.
    Lacking,
    /// The font has the feature: `same` and `upper` shape with it, for a run
    /// it covers whole, and these are synthesized, for any other.
    Covering {
        /// `sups` or `subs`.
        feature: [u8; 4],
        same: UsedFontId,
        upper: UsedFontId,
    },
}

/// What a grapheme asks of each candidate it tries.
///
/// A grapheme, to font selection, is a cluster and the continuation
/// clusters that join it ([`ItemGraphemes`]).
#[derive(Copy, Clone)]
pub(super) struct GraphemeNeeds<'t> {
    /// The grapheme's text.
    text: &'t str,
    /// The presentation it asks for.
    pub(super) presentation: Presentation,
    /// Whether it holds a variation sequence cluster matching asks a font
    /// for: the first pass then takes a font only with every such sequence.
    sequences: bool,
    /// The sequence whose presence in a font's `cmap` accepts the font for
    /// the presentation whatever its colour, for a forced one.
    presented: Option<(char, char)>,
}

impl<'t> GraphemeNeeds<'t> {
    /// Returns what a grapheme of `text` asks under `font-variant-emoji`
    /// `variant`.
    ///
    /// `class` is its first cluster's class, and `selector` says whether it
    /// holds a variation selector. A grapheme without one costs nothing beyond
    /// its presentation.
    pub(super) fn new(
        class: ClusterClass,
        text: &'t str,
        selector: bool,
        variant: FontVariantEmoji,
    ) -> Self {
        let presentation = coverage::presentation(class, text, variant);
        Self {
            text,
            presentation,
            sequences: selector && coverage::asks_sequences(text),
            presented: coverage::presentation_sequence(text, presentation),
        }
    }

    /// The same needs in presentation `p`: a forced form's base, tried
    /// where no font takes the form.
    fn with_presentation(self, p: Presentation) -> Self {
        Self {
            presentation: p,
            presented: coverage::presentation_sequence(self.text, p),
            ..self
        }
    }

    /// Whether `font` may draw the grapheme in its presentation.
    ///
    /// It may if its colour agrees, or if its `cmap` has the sequence that
    /// asks for the presentation. Blink likewise accepts a monochrome font
    /// with a VS16 sequence for emoji.
    fn accepts(&self, font: &Font) -> bool {
        self.presentation.accepts(font)
            || self
                .presented
                .is_some_and(|(base, selector)| font.maps_variation_sequence(base, selector))
    }

    /// Whether `font`, which covers the grapheme, has every variation sequence
    /// of it cluster matching asks for: always, where it asks for none.
    fn has_sequences(&self, font: &Font) -> bool {
        !self.sequences || coverage::has_sequences(font, self.text)
    }

    /// Returns how `candidate` fits the grapheme, with `page` its charset
    /// page last found.
    ///
    /// The walk and generated text both test candidates here. A pending face
    /// that wants the grapheme goes in `wanted`.
    #[inline]
    pub(super) fn fit<'l>(
        &self,
        candidate: &'l Candidate,
        wanted: &mut Vec<FaceId>,
        page: &mut Option<CharsetPage<'l>>,
    ) -> CandidateFit<'l> {
        if candidate.is_unusable() {
            return CandidateFit::Passed;
        }
        let Some(font) = candidate.font() else {
            return CandidateFit::Passed;
        };
        if font.is_pending() {
            // Nothing to draw with yet: the face to fetch, if it wants any
            // of this, and the list goes on to draw something meanwhile.
            if self.text.chars().any(|ch| font.wants(ch))
                && let Some(face) = font.face_id()
            {
                want(wanted, face);
            }
            return CandidateFit::Passed;
        }
        if !self.accepts(font) {
            return CandidateFit::Passed;
        }
        if !coverage::covers(font, self.text, page) {
            return CandidateFit::Misses(font);
        }
        if self.has_sequences(font) {
            CandidateFit::Covers
        } else {
            CandidateFit::CoversBase
        }
    }
}

/// How a candidate fits a grapheme's [`GraphemeNeeds`].
pub(super) enum CandidateFit<'l> {
    /// It is not asked: its bytes are lost, it is a face still pending, or
    /// its colour is not the presentation's.
    Passed,
    /// It covers the grapheme, with every variation sequence of it cluster
    /// matching asks for: the first pass's answer.
    Covers,
    /// It covers the grapheme without every sequence: the second pass's answer,
    /// used where no candidate of the whole list has them.
    CoversBase,
    /// It does not cover the grapheme: its font, for the walk's memo.
    Misses(&'l Font),
}

/// What trying a candidate list on a cluster found.
pub(super) enum ListOutcome {
    /// This candidate covers it.
    Found(CandidateId),
    /// The list's next family is to be matched first.
    Match,
    /// Nothing the list has so far covers it with every sequence it asks
    /// for, and every family the list has is matched: the first
    /// that covers it without them, if any, cluster matching's second pass.
    Exhausted(Option<CandidateId>),
}

/// What `list`, none of whose candidates covers a grapheme with every sequence
/// it asks for, says, `base` the first that covers it without them: that
/// it has matched every family, or that the next is to be matched first.
pub(super) fn second_pass(list: &CandidateList, base: Option<CandidateId>) -> ListOutcome {
    if list.is_exhausted() {
        ListOutcome::Exhausted(base)
    } else {
        ListOutcome::Match
    }
}

/// Returns the candidate of `list` (named `list_id`) that a grapheme with
/// `needs` is set in, using `pick` to try the list.
///
/// It tries, in order:
/// 1. the first candidate that covers the grapheme;
/// 2. the family that fallback finds for a missing character, walked once
///    per list and character;
/// 3. the second pass, which ignores variation sequences;
/// 4. for a forced presentation that nothing takes, the base form;
/// 5. the best partial cover.
///
/// Returns `None` where nothing draws any of it, and `Err` with the change
/// to make where a family must be matched or a fallback walked first.
pub(super) fn resolve(
    lists: &CandidateLists,
    list: &CandidateList,
    list_id: CandidateListId,
    needs: &GraphemeNeeds<'_>,
    mut pick: impl FnMut(&GraphemeNeeds<'_>) -> ListOutcome,
) -> Result<Option<CandidateId>, CacheChange> {
    let base = match pick(needs) {
        ListOutcome::Found(at) => return Ok(Some(at)),
        ListOutcome::Match => return Err(CacheChange::Match(list_id)),
        ListOutcome::Exhausted(base) => base,
    };
    if let Some(c) = lists.unmissed(list, needs.text) {
        return Err(CacheChange::Miss {
            list: list_id,
            c,
            presentation: needs.presentation,
        });
    }
    if let Some(at) = base {
        return Ok(Some(at));
    }
    let p = needs.presentation;
    let base = if p == p.base() {
        ListOutcome::Exhausted(None)
    } else {
        pick(&needs.with_presentation(p.base()))
    };
    Ok(match base {
        ListOutcome::Found(at) | ListOutcome::Exhausted(Some(at)) => Some(at),
        _ => best_partial(list, needs.text),
    })
}

/// Selects fonts for `range`'s clusters into `runs`.
///
/// Each item uses its node's text facts in the input's variant: the node's
/// own, or its `::first-line` facts for the first line. Pending faces a
/// cluster wants go in `wanted`.
pub(super) fn select_runs(
    input: &Input<'_>,
    range: Range<ClusterId>,
    cx: &mut FontContext,
    scratch: &mut RunScratch,
    used: &mut UsedFontInterner<'_>,
    runs: &mut FontRuns,
    wanted: &mut Vec<FaceId>,
) {
    let RunScratch { walk, position } = scratch;
    let out = &mut Output::new(runs, wanted, position);
    let FontInput {
        content, analysis, ..
    } = *input.stage;
    let default_language = input.default_language();
    let item_clusters = &analysis.item_clusters;
    let script_runs = &analysis.runs;
    let text_end = analysis.clusters.end_id();
    let mut script = script_runs.cursor(ScriptRunId::new(0), text_end);
    // The node of the item holding the cluster before the item in hand.
    let mut before: Option<NodeId> = None;
    for (item, entry) in content.items.iter() {
        if !entry.kind.has_text() {
            // An inline box's edge and a float's or an absolutely positioned
            // box's anchor leave the text on either side one position run; a
            // ruby's bounds end it, an annotation being a line of its own.
            if !matches!(
                entry.kind,
                ItemKind::Open | ItemKind::Close | ItemKind::Float | ItemKind::Absolute
            ) {
                out.close_position();
            }
            continue;
        }
        let clusters = item_clusters.range(item);
        let held_before = before;
        if !clusters.is_empty() {
            before = Some(entry.node);
        }
        let start = clusters.start.max(range.start);
        let end = clusters.end.min(range.end);
        if start >= end {
            continue;
        }
        let text = input.text(entry.node);
        let request_id = input.request(text);
        let primary = input.primary(request_id);
        // An atomic's U+FFFC and a `<br>`'s `\n` draw nothing, and end a
        // position run: the text either side of one is not contiguous.
        if entry.kind != ItemKind::Text {
            out.close_position();
            for cluster in (start..end).ids() {
                out.push_nothing(cluster, primary);
            }
            continue;
        }
        let facts = &content.facts;
        let request = facts.request(request_id);
        if decides_position(request, input.stage.position_synthesis) {
            out.open_position(facts.text(text).shaping);
        } else {
            out.close_position();
        }
        let list = input
            .lists
            .get(request.font.families)
            .copied()
            .unwrap_or_default();
        let matching = MatchRequest::from(request);
        let Some(&resolution) = input.resolutions.get(request_id) else {
            for cluster in (start..end).ids() {
                out.push_missing(cluster, primary);
            }
            continue;
        };
        let graphemes = ItemGraphemes::new(input, item, text, clusters, held_before);
        let mut at = start;
        while at < end {
            script_runs.step_to(&mut script, at, text_end);
            let run_end = script.end().min(end).max(ClusterId::new(at.get() + 1));
            let span = script_runs.get(script.id()).and_then(|run| {
                let language =
                    default_language.fonts_language(content.lists.languages.get(run.language));
                let (text, emoji) = cx.segment_lists(list, run.script, language, matching)?;
                Some(SelectionSpan {
                    end: run_end,
                    graphemes,
                    text,
                    emoji,
                    resolved: ResolvedRequest {
                        request,
                        request_id,
                        resolution,
                        script: features::opentype_script(run.script),
                    },
                })
            });
            match span {
                Some(span) => walk_span(input, &span, at, cx, walk, used, out),
                None => {
                    for cluster in (at..run_end).ids() {
                        out.push_missing(cluster, primary);
                    }
                }
            }
            at = run_end;
        }
    }
    out.close_position();
}

/// Turns on logging of the choices the text's walk asks for before `end`.
///
/// [`map_runs`] replays the log to map the first line's runs.
pub(super) fn log_choices_until(scratch: &mut RunScratch, end: ClusterId) {
    scratch.walk.choices.log.clear();
    scratch.walk.choices.logged_until = Some(end);
}

/// Maps the text's font runs (`text`) onto the first line's `range`
/// instead of selecting again, writing `runs`.
///
/// This works when each item's first-line request differs only in size and
/// spacing. Every cluster then asks the same questions, so the walk would
/// pick the same candidates and case sides in the same order. So we replay
/// the text walk's logged choices ([`log_choices_until`]) for the
/// first-line request.
///
/// A text run's used font maps to the same case side of the first-line
/// choice. A font that is neither side is the item's primary, and maps to
/// the first-line primary. Clusters that draw nothing at an item's start,
/// and all of an atomic's or a break's, take the font before them.
///
/// Returns `false`, writing nothing, if a request decides a
/// `font-variant-position` run or an item starts inside a grapheme. The
/// caller also checks that the first line's text is the text's own. On
/// `false` the caller uses [`select_runs`].
pub(super) fn map_runs(
    input: &Input<'_>,
    text: &FontRuns,
    range: Range<ClusterId>,
    cx: &mut FontContext,
    scratch: &mut RunScratch,
    used: &mut UsedFontInterner<'_>,
    runs: &mut FontRuns,
) -> bool {
    let FontInput {
        content, analysis, ..
    } = *input.stage;
    let (facts, nodes) = (&content.facts, &content.nodes);
    let clusters = &analysis.clusters;
    let item_clusters = &analysis.item_clusters;
    if scratch
        .walk
        .choices
        .logged_until
        .is_none_or(|end| end < range.end)
    {
        return false;
    }
    // The clusters of each item the range holds, and the item.
    let pieces = || {
        content.items.iter().filter_map(move |(item, entry)| {
            let held = item_clusters.range(item);
            let (start, end) = (held.start.max(range.start), held.end.min(range.end));
            (entry.kind.has_text() && start < end).then_some((entry, start..end))
        })
    };
    for (entry, held) in pieces() {
        work::step();
        if clusters.is_continuation(held.start) {
            return false;
        }
        if entry.kind == ItemKind::Text {
            let own = facts
                .request(input.request(nodes.text_facts(entry.node, FirstLineVariant::Standard)));
            let first = facts.request(input.request(input.text(entry.node)));
            let decides = decides_position(own, input.stage.position_synthesis);
            if !faces_alike(own, first) || decides {
                return false;
            }
        }
    }
    // The log is read while the choices are asked for again: taken for the
    // mapping and given back after it, whatever it answers, so its
    // capacity is kept.
    let choices = &mut scratch.walk.choices;
    let log = mem::take(&mut choices.log);
    let mut logged = log.iter().copied().peekable();
    let mut last: Option<UsedFontId> = None;
    // The text's font run holding the cluster in hand.
    let mut run = text.cursor(FontRunId::new(0), clusters.end_id());
    let mapped = 'map: {
        for (entry, held) in pieces() {
            let first_id = input.request(input.text(entry.node));
            let primary = input.primary(first_id);
            // The text's choice and the first line's for it, as the walk last
            // asked in the item: until it asks here, a cluster is set in the
            // item's primary font.
            let mut chosen: Option<(Choice, Choice)> = None;
            let mut at = held.start;
            // What draws nothing at an item's start takes the font before it:
            // all of an atomic's or a break's clusters.
            while at < held.end {
                work::step();
                let draws = entry.kind == ItemKind::Text
                    && clusters
                        .class(at)
                        .is_some_and(|class| !coverage::draws_nothing(class));
                if draws {
                    break;
                }
                let font = last.unwrap_or(primary);
                set_from(runs, at, font);
                last = Some(font);
                at = ClusterId::new(at.get() + 1);
            }
            while at < held.end {
                work::step();
                // Each choice the walk asked for here, in turn.
                while let Some((_, key)) = logged.next_if(|&(cluster, _)| cluster <= at) {
                    let first_key = ChoiceKey {
                        request: first_id,
                        ..key
                    };
                    let Some(own) = choices.find(key) else {
                        break 'map false;
                    };
                    let Some(first) = remade(input, first_key, cx, choices, used) else {
                        break 'map false;
                    };
                    chosen = Some((own, first));
                }
                text.step_to(&mut run, at, clusters.end_id());
                let Some(font) = text.get(run.id()).map(|run| run.font) else {
                    break 'map false;
                };
                let next_run = run.end().min(held.end);
                let next = logged
                    .peek()
                    .map_or(next_run, |&(cluster, _)| cluster.min(next_run));
                let mapped = match chosen {
                    Some((own, first)) if font == own.same => first.same,
                    Some((own, first)) if font == own.upper => first.upper,
                    _ => primary,
                };
                set_from(runs, at, mapped);
                last = Some(mapped);
                at = next.max(ClusterId::new(at.get() + 1));
            }
        }
        true
    };
    choices.log = log;
    mapped
}

/// Returns the first-line choice `key` names, making it in `choices` as the
/// walk would. `None` where the candidate or its bytes are gone.
fn remade(
    input: &Input<'_>,
    key: ChoiceKey,
    cx: &mut FontContext,
    choices: &mut Choices,
    used: &mut UsedFontInterner<'_>,
) -> Option<Choice> {
    let made = choices.find_or_choose(key, || {
        let resolved = ResolvedRequest {
            request: input.stage.content.facts.request(key.request),
            request_id: key.request,
            resolution: input.resolutions.get(key.request).copied()?,
            script: key.script,
        };
        let (lists, _, mut fonts) = cx.walk_caches();
        let held = lists.get(key.list)?.get(key.candidate)?;
        choose(
            input,
            &resolved,
            held.font()?,
            held.family(),
            &mut fonts,
            used,
        )
    });
    made.ok()
}

/// Whether the first-line request `first` picks the same fonts as the
/// node's own request `own` (see [`map_runs`]).
///
/// They may differ only in size, in whether letter spacing turns optional
/// ligatures off, and in the initial letter, which `::first-line` does not
/// change. The choices remade for `first` pick up the size and spacing.
fn faces_alike(own: &FontRequest, first: &FontRequest) -> bool {
    let sized = FontRequest {
        font: FontKey {
            size: own.font.size,
            ..first.font
        },
        spaced: own.spaced,
        initial_letter: own.initial_letter,
        ..*first
    };
    sized == *own
}

/// Walks `span` from `from`, making each change the walk stops for.
fn walk_span(
    input: &Input<'_>,
    span: &SelectionSpan<'_>,
    from: ClusterId,
    cx: &mut FontContext,
    scratch: &mut SpanScratch,
    used: &mut UsedFontInterner<'_>,
    out: &mut Output<'_>,
) {
    scratch.start_segment();
    let mut at = from;
    for _ in 0..MAX_STOPS {
        let Some((change, cluster)) = walk(input, span, at, cx, scratch, used, out) else {
            return;
        };
        cx.apply(change);
        at = cluster;
    }
    debug_assert!(false, "a span's walk stopped {MAX_STOPS} times");
    for cluster in (at..span.end).ids() {
        out.push_missing(cluster, span.resolved.resolution.primary);
    }
}

/// Where a text item's graphemes cross its edges.
///
/// A grapheme that a style boundary divides has its later parts as
/// continuation clusters. It is one grapheme where the parts' items shape alike
/// (one [`ShapingFactsId`]), and the later parts take the first part's font.
/// Read once per item from its shaping facts and its neighbours', so the
/// walk never looks up a cluster's item.
#[derive(Copy, Clone)]
struct ItemGraphemes {
    /// The item's first cluster, where it joins the part of its grapheme
    /// before it: a continuation, after an item that shapes alike.
    joined: Option<ClusterId>,
    /// The item's last cluster.
    last: ClusterId,
    /// The last cluster of the grapheme `last` starts: `last`, or past the
    /// item the later parts of its grapheme that join it, each starting an
    /// item that shapes alike.
    tail: ClusterId,
}

impl ItemGraphemes {
    /// Returns the graphemes at the edges of `item`.
    ///
    /// `text` is the item's text facts in the input's variant, `clusters`
    /// its non-empty cluster range, and `before` the node of the item holding
    /// the cluster before it. Later items are read only when the cluster
    /// after the item is a divided grapheme's later part.
    fn new(
        input: &Input<'_>,
        item: ItemId,
        text: TextFactsId,
        clusters: Range<ClusterId>,
        before: Option<NodeId>,
    ) -> Self {
        let FontInput {
            content, analysis, ..
        } = *input.stage;
        let divided = |cluster: ClusterId| analysis.clusters.is_continuation(cluster);
        let facts = &content.facts;
        let shaping = |node: NodeId| facts.text(input.text(node)).shaping;
        let own = facts.text(text).shaping;
        let joined = (divided(clusters.start) && before.is_some_and(|node| shaping(node) == own))
            .then_some(clusters.start);
        let last = ClusterId::new(clusters.end.get().saturating_sub(1));
        let mut tail = last;
        if divided(clusters.end) {
            let items = &content.items;
            for next in (ItemId::new(item.get() + 1)..ItemId::new(items.len())).ids() {
                work::step();
                let held = analysis.item_clusters.range(next);
                if held.is_empty() {
                    continue;
                }
                let joins = held.start.get() == tail.get() + 1
                    && divided(held.start)
                    && items
                        .get(next)
                        .is_some_and(|entry| entry.kind.has_text() && shaping(entry.node) == own);
                if !joins {
                    break;
                }
                tail = held.start;
                // A later cluster of the item is no grapheme's later part.
                if held.end.get() > tail.get() + 1 {
                    break;
                }
            }
        }
        Self { joined, last, tail }
    }

    /// Whether `cluster` is one grapheme with the part of its grapheme before
    /// it, whose font it takes.
    fn joins(&self, cluster: ClusterId) -> bool {
        self.joined == Some(cluster)
    }

    /// The last cluster of the grapheme `cluster` starts.
    fn grapheme_last(&self, cluster: ClusterId) -> ClusterId {
        if cluster == self.last {
            self.tail
        } else {
            cluster
        }
    }
}

/// Walks `span` from `from` to its end, or to the first cluster the walk
/// must stop at, which it returns with the change it stopped for.
fn walk(
    input: &Input<'_>,
    span: &SelectionSpan<'_>,
    from: ClusterId,
    cx: &mut FontContext,
    scratch: &mut SpanScratch,
    used: &mut UsedFontInterner<'_>,
    out: &mut Output<'_>,
) -> Option<(CacheChange, ClusterId)> {
    let clusters = &input.stage.analysis.clusters;
    let resolved = &span.resolved;
    let (lists, coverages, mut fonts) = cx.walk_caches();
    let (Some(text_list), Some(emoji_list)) = (lists.get(span.text), lists.get(span.emoji)) else {
        for cluster in (from..span.end).ids() {
            out.push_missing(cluster, resolved.resolution.primary);
        }
        return None;
    };
    let span_text = SpanText {
        source: input.source,
        clusters,
        end: span.end,
        graphemes: span.graphemes,
    };
    let [text_memo, emoji_memo] = &mut scratch.memo;
    let mut text_pages = Pages::new();
    let mut emoji_pages = Pages::new();
    // The last candidate chosen here and the used fonts it sets text in, so
    // that a run of clusters in one font costs no lookup a cluster: the
    // span is one style, so a candidate is the same one or two used
    // fonts throughout.
    let mut chosen: Option<((CandidateListId, CandidateId), Choice)> = None;
    // What the chosen candidate's position feature covers, where it has one
    // a run decides on, and the page of it last found: one bit test a
    // character while the run stays in the candidate.
    let mut feature_chars: Option<&Charset> = None;
    let mut feature_page: Option<CharsetPage<'_>> = None;
    let mut next = from;
    while next < span.end {
        let cluster = next;
        next = ClusterId::new(cluster.get() + 1);
        // The later part of a grapheme that shapes like the part before it
        // was chosen for with that part, and takes its font.
        if span_text.graphemes.joins(cluster)
            && let Some(last) = out.last
        {
            out.push(cluster, last);
            continue;
        }
        let class = match clusters.class(cluster) {
            Some(class) if !coverage::draws_nothing(class) => class,
            _ => {
                out.push_nothing(cluster, resolved.resolution.primary);
                continue;
            }
        };
        // What the cluster is chosen for with: it, and the later parts of its
        // grapheme that join it.
        let (chars, selector) = span_text.grapheme(cluster);
        let variant = resolved.request.font.variant_emoji;
        let needs = GraphemeNeeds::new(class, chars, selector, variant);
        let (list, list_id, memo, pages) = if needs.presentation.base() == Presentation::Emoji {
            (emoji_list, span.emoji, &mut *emoji_memo, &mut emoji_pages)
        } else {
            (text_list, span.text, &mut *text_memo, &mut text_pages)
        };
        let pick = |needs: &GraphemeNeeds<'_>| {
            span_text.pick(list, memo, pages, out.wanted, cluster, needs)
        };
        let found = match resolve(lists, list, list_id, &needs, pick) {
            Ok(found) => found,
            Err(change) => return Some((change, cluster)),
        };
        // Nothing draws any of it: the primary font's `.notdef`.
        let Some(at) = found else {
            out.push_missing(cluster, resolved.resolution.primary);
            continue;
        };
        let candidate = (list_id, at);
        let choice = match chosen {
            Some((held, choice)) if held == candidate => choice,
            _ => {
                let Some((font, family)) = list
                    .get(at)
                    .and_then(|held| Some((held.font()?, held.family())))
                else {
                    out.push_missing(cluster, resolved.resolution.primary);
                    continue;
                };
                let key = resolved.key(list_id, at);
                let made = scratch.choices.find_or_choose(key, || {
                    choose(input, resolved, font, family, &mut fonts, used)
                });
                let choice = match made {
                    Ok(choice) => choice,
                    Err(change) => return Some((change, cluster)),
                };
                scratch.choices.log(cluster, key);
                feature_page = None;
                feature_chars = match feature_coverage(coverages, choice, font, key) {
                    Ok(chars) => chars,
                    Err(change) => return Some((change, cluster)),
                };
                chosen = Some((candidate, choice));
                choice
            }
        };
        let upper_side = on_upper_side(choice, chars, &mut scratch.upper);
        let placed = place(
            choice,
            upper_side,
            chars,
            feature_chars,
            &mut feature_page,
            out.position,
        );
        out.push(cluster, placed);
        // The clusters after it that the text list would set in the same
        // font are set so without asking it (see `KeptCandidate`), where the
        // font sets every cluster alike and nothing waits on a position run.
        if list_id == span.text
            && shortcuts()
            && choice.same == choice.upper
            && matches!(choice.position, PositionPlan::Settled)
            && let Some(mut kept) = KeptCandidate::new(
                text_list,
                text_memo,
                &text_pages,
                at,
                cluster,
                span.end,
                variant != FontVariantEmoji::Normal,
            )
        {
            next = span_text.kept_end(&mut kept, next);
        }
    }
    None
}

/// Returns what the position feature of `choice`, made for `key` in `font`,
/// covers, where a position run decides on it.
///
/// What the feature covers is read once per font, script and feature, the
/// first time a run asks. Returns the change that reads it where it is not
/// read yet. `None` where nothing decides, or where the font has no key.
fn feature_coverage<'c>(
    coverages: &'c FeatureCoverages,
    choice: Choice,
    font: &Font,
    key: ChoiceKey,
) -> Result<Option<&'c Charset>, CacheChange> {
    let PositionPlan::Covering { feature, .. } = choice.position else {
        return Ok(None);
    };
    let Some(font) = font.key() else {
        return Ok(None);
    };
    match coverages.find(font, key.script, feature) {
        Some(chars) => Ok(Some(chars)),
        None => Err(CacheChange::Coverage {
            list: key.list,
            candidate: key.candidate,
            script: key.script,
            feature,
        }),
    }
}

/// Returns whether `chars` is set on the uppercased side of `choice`.
///
/// Only where the font sets the two sides of a change of case apart is a
/// cluster's case asked: most text is set alike either way. `upper` keeps
/// the answer, which a cluster that starts with a mark takes.
#[inline]
fn on_upper_side(choice: Choice, chars: &str, upper: &mut bool) -> bool {
    choice.same != choice.upper && {
        *upper = match chars.chars().next() {
            Some(ch) if is_mark(ch) => *upper,
            Some(ch) => features::changes_when_uppercased(ch),
            None => false,
        };
        *upper
    }
}

/// Returns how a cluster saying `chars` is set under `choice`, on its
/// uppercased side where `upper_side` says so.
///
/// Under a position plan, a character that lacks the feature's glyph
/// uncovers the open `position` run. `feature_chars` is what the feature
/// covers and `feature_page` the page of it last found.
#[inline]
fn place<'c>(
    choice: Choice,
    upper_side: bool,
    chars: &str,
    feature_chars: Option<&'c Charset>,
    feature_page: &mut Option<CharsetPage<'c>>,
    position: &mut PositionRun,
) -> Placed {
    let font = if upper_side {
        choice.upper
    } else {
        choice.same
    };
    match choice.position {
        PositionPlan::Settled => Placed::Font(font),
        PositionPlan::Lacking => {
            position.uncover();
            Placed::Font(font)
        }
        PositionPlan::Covering {
            same: made_same,
            upper: made_upper,
            ..
        } => {
            if position.asks() && !feature_covers(feature_chars, chars, feature_page) {
                position.uncover();
            }
            Placed::Either {
                feature: font,
                synthesized: if upper_side { made_upper } else { made_same },
            }
        }
    }
}

/// Whether the run loop takes its shortcuts: [`KeptCandidate`] and [`map_runs`].
///
/// Always true, except in tests that compare them with the walk that asks
/// the list at every cluster.
pub(super) fn shortcuts() -> bool {
    #[cfg(test)]
    {
        !PER_CLUSTER.with(Cell::get)
    }
    #[cfg(not(test))]
    {
        true
    }
}

#[cfg(test)]
std::thread_local! {
    /// When set, the run loop on this thread takes no shortcut and asks the
    /// list at every cluster, the first line's too.
    pub(super) static PER_CLUSTER: Cell<bool> = const { Cell::new(false) };
}

/// The clusters after one the text list just set in a candidate, which keep
/// that candidate without asking the list.
///
/// The list gives a cluster the first candidate that the memo does not rule
/// out and that covers it. If every earlier candidate is ruled out until
/// `limit` (or is unusable), and this one covers a cluster as written, the
/// list would give it this one. GeneratedRequest would change nothing either, since a
/// found candidate memoizes nothing. So text in its requested font costs a
/// bit test per character. A cluster that draws nothing takes the font
/// before it, as in the walk.
///
/// The walk asks the list again at the first cluster that:
/// - this candidate does not cover as written;
/// - may ask for emoji or a forced presentation;
/// - holds a variation selector, which may want a font with its sequence;
/// - is `limit`, or the cluster before it when a divided grapheme's later
///   part follows, since that cluster chooses with the part.
struct KeptCandidate<'l> {
    /// The candidate's charset, and the page of it last found.
    charset: &'l Charset,
    page: Option<CharsetPage<'l>>,
    /// The first cluster an earlier candidate may be asked about again.
    limit: ClusterId,
    /// The style's `font-variant-emoji` forces a presentation, which a
    /// keycap base takes: `#`, `*` or a digit, whose cluster is text.
    keycaps: bool,
}

impl<'l> KeptCandidate<'l> {
    /// Returns the kept clusters after `cluster`, which `list` gave candidate
    /// `at`, in a span ending at `end`.
    ///
    /// `memo` and `pages` are the list's memo and pages. `keycaps` says the
    /// style's `font-variant-emoji` forces a presentation on a keycap base.
    /// Returns `None` where it holds nothing, or where the memo does not
    /// rule out an earlier candidate past `cluster`. That happens for a
    /// pending face or a font a forced presentation passed over.
    fn new(
        list: &'l CandidateList,
        memo: &Table<CandidateId, ClusterId>,
        pages: &Pages<'l>,
        at: CandidateId,
        cluster: ClusterId,
        end: ClusterId,
        keycaps: bool,
    ) -> Option<Self> {
        let charset = list.get(at)?.font()?.charset();
        let mut limit = end;
        for (before, candidate) in list.iter().take(at.get()) {
            work::step();
            // Never asked: the list passes over it wherever it stands.
            if candidate.is_unusable() || candidate.font().is_none() {
                continue;
            }
            match memo.get(before) {
                Some(&next) if next > cluster => limit = limit.min(next),
                _ => return None,
            }
        }
        (limit.get() > cluster.get() + 1).then(|| Self {
            charset,
            keycaps,
            page: pages.get(at).flatten(),
            limit,
        })
    }
}

/// Whether the position feature covering `chars` has a glyph for every
/// drawn character of `text`.
///
/// `page` is the page of `chars` last found. Default-ignorables are skipped:
/// the shaper hides them, so there is no glyph to vary. Returns `false`
/// where the font has no key, so its coverage could not be read.
fn feature_covers<'a>(
    chars: Option<&'a Charset>,
    text: &str,
    page: &mut Option<CharsetPage<'a>>,
) -> bool {
    let Some(chars) = chars else {
        return false;
    };
    text.chars()
        .all(|ch| unicode::rare_props(ch).is_default_ignorable() || coverage::maps(chars, ch, page))
}

/// Whether `ch` extends the grapheme before it: a mark, which takes the case
/// of the letter it is on.
fn is_mark(ch: char) -> bool {
    unicode::rare_props(ch).grapheme_cluster_break() == GraphemeClusterBreak::Extend
}

/// Returns the used fonts that `font`, a candidate of `family`, sets the
/// text `resolved` describes in. `None` where its bytes are gone.
///
/// Makes the instances and used fonts in `fonts` and `used` the first time
/// a build asks. The font's offered features under the script decide how it
/// sets capitals ([`CapsPlan`]) and whether it has the position feature. The
/// style and the `@font-face` descriptors decide its size and instance.
///
/// Where a position run decides and the font has the feature, both versions
/// are made, since only the run's end says which one is used. The
/// synthesized one uses an instance without the feature, whose glyphs are
/// the ones the feature would replace (CSS Fonts 4, section 6.5). Firefox's
/// `gfxFontStyle::AdjustForSubSuperscript` clears the position the same way.
pub(super) fn choose(
    input: &Input<'_>,
    resolved: &ResolvedRequest<'_>,
    font: &Font,
    family: &Family,
    fonts: &mut FontCaches<'_>,
    used: &mut UsedFontInterner<'_>,
) -> Option<Choice> {
    let request = resolved.request;
    let caps = request.font.variant_caps;
    let position = request.font.variant_position;
    let stage = input.stage;
    let decides = decides_position(request, stage.position_synthesis);
    let offered = if caps != FontVariantCaps::Normal || decides {
        fonts
            .offers
            .find_or_read(family, font, resolved.script, fonts.instances)
    } else {
        OfferedFeatures::NONE
    };
    let plan = CapsPlan::new(caps, offered, request.font.synthesis.small_caps);
    let feature = features::position_tag(position);
    let has_feature = offered.has_position(position);
    let overrides = FaceOverrides::new(font);
    let px = resolved.resolution.font_px(overrides);
    let size = used_size(px);
    let matching_only = variations_left_to_matching(family, stage.platform_font_variations);
    let lists = &stage.content.lists;
    let variations = lists.variation_settings().get(request.font.variations);
    let settings = lists.feature_settings().get(request.font.features);
    // The used font setting `side`: with the position's feature, or
    // synthesized without it.
    let mut used_for = |side: CaseSide, synthesized: bool| -> Option<UsedFontId> {
        let instance_request = InstanceRequest {
            font_request: request,
            variations,
            settings,
            upright: resolved.resolution.upright,
            caps: side.caps(),
            position: if synthesized {
                FontVariantPosition::Normal
            } else {
                position
            },
            size: px,
            overrides,
            matching_only,
        };
        let instance = fonts
            .instances
            .find_or_make(family, font, &instance_request)?;
        let unscaled = fonts.instances.get(instance)?.unscaled();
        let moved = if synthesized {
            unscaled.synthesized_position(position, px)
        } else {
            None
        };
        let synthesis = side_synthesis(side, moved, resolved.resolution.computed, px);
        let instances = &*fonts.instances;
        Some(used.intern_or(
            Some(instance),
            size,
            synthesis,
            instances,
            resolved.resolution.primary,
        ))
    };
    let mut pair = |synthesized: bool| -> Option<(UsedFontId, UsedFontId)> {
        let same = used_for(plan.same(), synthesized)?;
        let upper = if plan.is_uniform() {
            same
        } else {
            used_for(plan.upper(), synthesized)?
        };
        Some((same, upper))
    };
    let ((same, upper), position) = match feature {
        Some(feature) if decides && has_feature => {
            let with = pair(false)?;
            let (same, upper) = pair(true)?;
            (
                with,
                PositionPlan::Covering {
                    feature,
                    same,
                    upper,
                },
            )
        }
        Some(_) if decides => (pair(true)?, PositionPlan::Lacking),
        _ => (pair(false)?, PositionPlan::Settled),
    };
    Some(Choice {
        same,
        upper,
        position,
    })
}

/// Returns the synthesis for a used font that sets the text on `side` of a
/// change of case.
///
/// `position` is a synthesized position's glyph size in pixels and raise,
/// if any. `computed` is the style's computed size and `px` the font's used
/// size.
///
/// Synthesized small capitals are uppercase at 0.7 of the computed size,
/// rounded to a whole pixel, whatever `font-size-adjust` did. This matches
/// Blink's `SimpleFontData::SmallCapsFontData` (`kSmallCapsFontSizeMultiplier`,
/// `lroundf`). With a synthesized position too, they are 0.7 of its size.
fn side_synthesis(
    side: CaseSide,
    position: Option<(f32, LayoutUnit)>,
    computed: f32,
    px: f32,
) -> UsedSynthesis {
    let case = side.case_map();
    if case == CaseMap::Keep && position.is_none() {
        return UsedSynthesis::None;
    }
    let (glyph_px, raise) = position.unwrap_or((px, LayoutUnit::ZERO));
    let glyph_px = match (side.is_synthetic(), position) {
        (true, None) => unit::round_to_whole(computed * SMALL_CAPS),
        (true, Some(_)) => glyph_px * SMALL_CAPS,
        (false, _) => glyph_px,
    };
    UsedSynthesis::Synthesized {
        case,
        glyph_size: used_size(glyph_px),
        raise,
    }
}

/// How much smaller than the size synthesized small capitals are drawn:
/// Blink's `kSmallCapsFontSizeMultiplier`.
const SMALL_CAPS: f32 = 0.7;

/// What trying a candidate list on a cluster reads, besides the list.
struct SpanText<'w> {
    source: &'w TextCursor<'w>,
    clusters: &'w Clusters,
    /// The span's end, which the memo's scans stop at.
    end: ClusterId,
    /// Where its item's graphemes cross the item's edges.
    graphemes: ItemGraphemes,
}

impl<'w> SpanText<'w> {
    /// The text `cluster` is chosen for with: its own, and that of the later
    /// parts of its grapheme that join it.
    fn grapheme_text(&self, cluster: ClusterId) -> &'w str {
        let start = self.clusters.range(cluster).start;
        let end = self
            .clusters
            .range(self.graphemes.grapheme_last(cluster))
            .end;
        self.source.slice(start..end)
    }

    /// Returns the grapheme's text, as [`grapheme_text`](Self::grapheme_text) gives it,
    /// and whether it holds a variation selector.
    ///
    /// The analysis flags a selector in the cluster's own characters. A
    /// joined later part may start with one, so its text is scanned.
    fn grapheme(&self, cluster: ClusterId) -> (&'w str, bool) {
        let last = self.graphemes.grapheme_last(cluster);
        let own = self.clusters.range(cluster);
        let end = self.clusters.range(last).end;
        let mut selector = self.clusters.is_variation_selector(cluster);
        if last != cluster {
            let joined = self.source.slice(own.end..end);
            selector |= joined.chars().any(is_variation_selector);
        }
        (self.source.slice(own.start..end), selector)
    }

    /// Tries `list` for the grapheme at `cluster`, memoizing each candidate
    /// that misses.
    ///
    /// Finds the first candidate that `memo` does not rule out, that accepts
    /// the presentation and that covers the grapheme. A pending face that wants
    /// the grapheme goes in `wanted`.
    ///
    /// A grapheme with a variation sequence wants a candidate that also has the
    /// sequence, searched over the whole list as far as matching and
    /// fallback go. Only if none has it does the first that covers the bases
    /// win, drawing them and hiding the selectors. These are Blink's two
    /// passes. Such a base-only candidate is not memoized, since it covers
    /// the cluster.
    fn pick<'l>(
        &self,
        list: &'l CandidateList,
        memo: &mut Table<CandidateId, ClusterId>,
        pages: &mut Pages<'l>,
        wanted: &mut Vec<FaceId>,
        cluster: ClusterId,
        needs: &GraphemeNeeds<'_>,
    ) -> ListOutcome {
        // The first candidate covering the grapheme without every sequence: the
        // second pass's answer.
        let mut base: Option<CandidateId> = None;
        for (at, candidate) in list.iter() {
            if memo.get(at).is_some_and(|&next| next > cluster) {
                continue;
            }
            let mut spare: Option<CharsetPage<'l>> = None;
            let page = pages.get_mut(at).unwrap_or(&mut spare);
            match needs.fit(candidate, wanted, page) {
                CandidateFit::Covers => return ListOutcome::Found(at),
                CandidateFit::CoversBase => {
                    base.get_or_insert(at);
                }
                CandidateFit::Misses(font) => {
                    let next = self.next_covered(font, page, cluster);
                    memo.set_growing(at, next, ClusterId::new(0));
                }
                CandidateFit::Passed => {}
            }
        }
        second_pass(list, base)
    }

    /// Returns the first cluster from `from` that `kept` does not hold.
    /// The clusters before it keep the font of the one before `from`.
    fn kept_end(&self, kept: &mut KeptCandidate<'_>, from: ClusterId) -> ClusterId {
        let clusters = self.clusters;
        // A grapheme's later part, which an item boundary divided from the
        // part before it, and so starts a span: inside one, none is.
        let divided = |cluster: ClusterId| clusters.is_continuation(cluster);
        // Each cluster starts where the one before it ends, so each end is
        // read once, with what it says of a selector.
        let mut start = clusters.start(from);
        for cluster in (from..kept.limit).ids() {
            work::step();
            debug_assert!(
                !divided(cluster),
                "a divided grapheme's part starts an item"
            );
            let (Some(attrs), Some(end)) = (clusters.attrs(cluster), clusters.end(cluster)) else {
                return cluster;
            };
            let range = start..end.end();
            start = range.end;
            let class = attrs.class();
            if coverage::draws_nothing(class) {
                continue;
            }
            // An emoji or a pictograph asks for its presentation, and a
            // variation selector for a presentation or a sequence. The list
            // must answer those.
            if matches!(class, ClusterClass::Emoji | ClusterClass::Symbol)
                || end.is_variation_selector()
            {
                return cluster;
            }
            // One ASCII character, most of a Latin text's, read as a byte:
            // no default-ignorable is ASCII, so the font covers it as
            // written where it maps it, as `covers_as_written` would say.
            if let Some(ch) = self.source.ascii(range.clone())
                && !(kept.keycaps && coverage::is_keycap_base(ch))
                && work::fast_paths()
            {
                if coverage::maps(kept.charset, ch, &mut kept.page) {
                    continue;
                }
                return cluster;
            }
            let text = self.source.slice(range);
            if kept.keycaps && text.starts_with(coverage::is_keycap_base) {
                return cluster;
            }
            if !coverage::covers_as_written(kept.charset, text, &mut kept.page) {
                return cluster;
            }
        }
        // The part after the kept clusters may be the last cluster's own,
        // where an item boundary divides a grapheme: that cluster is chosen
        // for with it, and so is asked about again.
        if divided(kept.limit) {
            return ClusterId::new(kept.limit.get() - 1).max(from);
        }
        kept.limit
    }

    /// The next cluster of the span after `cluster` that `font` covers, or
    /// the span's end: until then the font is skipped. Clusters that draw
    /// nothing are passed over, since they never ask.
    fn next_covered<'l>(
        &self,
        font: &'l Font,
        page: &mut Option<CharsetPage<'l>>,
        cluster: ClusterId,
    ) -> ClusterId {
        let after = ClusterId::new(cluster.get() + 1);
        for cluster in (after..self.end).ids() {
            let draws = !self.graphemes.joins(cluster)
                && self
                    .clusters
                    .class(cluster)
                    .is_some_and(|class| !coverage::draws_nothing(class));
            let text = self.grapheme_text(cluster);
            if draws && coverage::covers(font, text, page) {
                return cluster;
            }
        }
        self.end
    }
}

/// Returns the candidate of `list` that draws the most of `chars`.
///
/// Covering the base character ranks first, then the count. `None` where
/// none draws any of it.
fn best_partial(list: &CandidateList, chars: &str) -> Option<CandidateId> {
    let mut best: Option<(CandidateId, coverage::Score)> = None;
    for (at, candidate) in list.iter() {
        if candidate.is_unusable() {
            continue;
        }
        let Some(score) = candidate
            .font()
            .filter(|font| !font.is_pending())
            .and_then(|font| coverage::score(font, chars))
        else {
            continue;
        };
        if best.is_none_or(|(_, held)| score.beats(held)) {
            best = Some((at, score));
        }
    }
    best.map(|(at, _)| at)
}
