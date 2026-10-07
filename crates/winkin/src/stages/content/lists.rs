//! The layout's lists, and the one lookup every interned table of the
//! content finds its rows by.
//!
//! There is a table for each kind of list a caller's style names, and each
//! list is stored once. Each table answers one question, by id.
//!
//! Each build clears and refills everything here, keeping the capacity, the
//! hash tables included, so a warm rebuild allocates nothing. Family names
//! go into one byte arena, and the hash tables are hashbrown's, which store
//! their entries inline.
//!
//! Every table refuses what does not fit rather than panicking. A list that
//! cannot be stored takes the initial one. The writer counts each, and the
//! build reports them.
//!
//! [`Lists`] is where every style's lists enter. It interns them into
//! their tables and returns the style keyed by their ids, a [`StyleKey`].
//! The content never stores a key: the writer lowers it into the facts.
//! [`Lists`] also hands each table out to the stages that read it.

use alloc::boxed::Box;
use alloc::string::String;
use core::hash::{Hash, Hasher};
use core::ops::Range;
use parlance::{FontFamilyName, FontFeature, FontVariation, GenericFamily, Language};

use super::memo::{FontKey, StyleKey, TextKey};
use crate::data::{
    FxHasher, HashIndex, HeapBytes, Id, IdRange, Table, define_id, hash_one, heap_bytes,
    make_text_room,
};
use crate::style::ComputedStyle;
use crate::style::{Same, same_by_value};

define_id! {
    /// The id of a distinct `font-family` list in a layout.
    ///
    /// The id has sixteen bits. A document with more distinct family lists
    /// gives the rest the initial one, and reports it.
    pub(crate) struct FamilyListId(u16);
}

define_id! {
    /// Names a distinct `font-feature-settings` list in a layout.
    pub(crate) struct FeatureSettingsId(u16);
}

define_id! {
    /// Names a distinct `font-variation-settings` list in a layout.
    pub(crate) struct VariationSettingsId(u16);
}

define_id! {
    /// The id of a distinct content language in a layout.
    ///
    /// The analysis's runs name languages by this id too, so a run is 12
    /// bytes and every stage asks one table.
    pub(crate) struct LanguageId(u16);
}

define_id! {
    /// Names a distinct `hyphenate-character` string in a layout.
    pub(crate) struct HyphenStringId(u16);
}

define_id! {
    /// Names one entry of an interned family list, in [`FamilyLists`]' table
    /// of every list's entries.
    struct FamilyEntryId(u32);
}

define_id! {
    /// Names one setting of an interned `font-feature-settings` list, in
    /// [`FeatureSettings`]' table of every list's settings.
    pub(crate) struct FontFeatureId(u32);
}

define_id! {
    /// Names one setting of an interned `font-variation-settings` list, in
    /// [`VariationSettings`]' table of every list's settings.
    pub(crate) struct FontVariationId(u32);
}

define_id! {
    /// Names one byte of an interned `hyphenate-character` string, in
    /// [`HyphenStrings`]' table of every string's bytes.
    struct HyphenByteId(u32);
}

same_by_value!(
    FamilyListId,
    FeatureSettingsId,
    VariationSettingsId,
    LanguageId,
    HyphenStringId
);

impl FamilyListId {
    /// The initial `font-family`, `serif`, which every build interns first.
    pub(super) const INITIAL: Self = Self(0);
}

impl FeatureSettingsId {
    /// The empty list, the table's first.
    ///
    /// The table stores it only once it stores another list after it (see
    /// [`InternedLists::intern`]).
    pub(super) const EMPTY: Self = Self(0);
}

impl VariationSettingsId {
    /// The empty list, the table's first.
    ///
    /// The table stores it only once it stores another list after it (see
    /// [`InternedLists::intern`]).
    pub(super) const EMPTY: Self = Self(0);
}

impl LanguageId {
    /// `und`, for no language given, the table's first.
    ///
    /// The table stores it only once it stores another language after it
    /// (see [`Languages::intern`]).
    pub(super) const UNDETERMINED: Self = Self(0);
}

/// Returns `id` where there is one, else `initial`, counting the
/// substitution in `replaced`.
fn or_initial<T>(id: Option<T>, initial: T, replaced: &mut usize) -> T {
    id.unwrap_or_else(|| {
        *replaced = replaced.saturating_add(1);
        initial
    })
}

/// Where a family's name is in the string [`FamilyLists`] keeps every name
/// in.
///
/// It is a byte range, not an id range, since a string is bytes and not a
/// table of rows. It takes eight bytes where a `Range<usize>` takes
/// sixteen, and it is `Copy`.
#[derive(Copy, Clone, Debug)]
struct ByteRange {
    start: u32,
    end: u32,
}

impl ByteRange {
    /// Returns the bytes it names, as a range to index the string with.
    fn range(self) -> Range<usize> {
        usize::try_from(self.start).unwrap_or(usize::MAX)
            ..usize::try_from(self.end).unwrap_or(usize::MAX)
    }
}

/// How many values a table holds before it is searched by hash.
const FEW: usize = 8;

/// How the layout's tables of interned values find a value they hold
/// already.
///
/// While a table holds [`FEW`] or fewer values, it compares the value with
/// each, newest first. Past that it searches one hash index shared by every
/// table. A table fills the index with its own values the first time it
/// holds more.
///
/// A layout interns a style's lists and facts for each box, and most
/// layouts hold a few of each. Comparing a few is quicker than hashing a
/// row. A small layout, which a host may keep for every label it shows,
/// keeps no index at all. The one index serves every table, each asking
/// about its own values (see [`HashIndex`]). The builder's memo keeps its
/// own index in the builder's scratch (see `memo`).
pub(super) struct Lookup {
    index: HashIndex,
}

impl Lookup {
    /// Creates a lookup with no index, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            index: HashIndex::new(),
        }
    }

    /// Forgets every value, keeping the index's capacity.
    pub(super) fn clear(&mut self) {
        self.index.clear();
    }

    /// Finds the id, among the `held` values a table holds, of the value
    /// `same` accepts.
    ///
    /// Where there is none, returns the hash `hash` made of the value
    /// asked for, for [`add`](Self::add), or `None` where nothing was
    /// hashed.
    pub(super) fn find<I: Id>(
        &self,
        held: usize,
        hash: impl FnOnce() -> u64,
        same: impl Fn(I) -> bool,
    ) -> Result<I, Option<u64>> {
        if held <= FEW {
            return (I::new(0)..I::new(held))
                .ids()
                .rev()
                .find(|&id| same(id))
                .ok_or(None);
        }
        let hash = hash();
        self.index.find(hash, same).ok_or(Some(hash))
    }

    /// Notes `id`, the value the table has just added.
    ///
    /// `hash` is the hash [`find`](Self::find) made of it, if any. The
    /// value is indexed where the table holds more than a few. The first
    /// time it does, every value before it is indexed too, each hashed by
    /// `hash_of`.
    pub(super) fn add<I: Id>(&mut self, id: I, hash: Option<u64>, hash_of: impl Fn(I) -> u64) {
        let held = id.get() + 1;
        if held <= FEW {
            return;
        }
        if held == FEW + 1 {
            for before in (I::new(0)..id).ids() {
                let h = hash_of(before);
                self.index.insert(h, before);
            }
        }
        let h = hash.unwrap_or_else(|| hash_of(id));
        self.index.insert(h, id);
    }

    /// Returns the id of `value` in `values`, adding it where it is new, or
    /// `None` where the table is full.
    ///
    /// `values` holds each value once: the languages or a table of facts.
    pub(super) fn intern<I: Id, T: PartialEq + Hash>(
        &mut self,
        values: &mut Table<I, T>,
        value: T,
    ) -> Option<I> {
        let held = &*values;
        let found = self.find(
            held.len(),
            || hash_one(&value),
            |id| held.get(id) == Some(&value),
        );
        let hash = match found {
            Ok(id) => return Some(id),
            Err(hash) => hash,
        };
        let id = values.push(value)?;
        let held = &*values;
        self.add(id, hash, |id| held.get(id).map_or(0, hash_one));
        Some(id)
    }
}

heap_bytes! {
    Lookup { index }
}

/// Hashes a family list, a caller's or an interned one: its length, and
/// each family, a generic or a name.
fn hash_names<'a>(names: impl ExactSizeIterator<Item = FamilyName<'a>>) -> u64 {
    let mut fx = FxHasher::new();
    fx.write_usize(names.len());
    for family in names {
        match family {
            FamilyName::Generic(generic) => {
                fx.write_u8(0);
                generic.hash(&mut fx);
            }
            FamilyName::Named(name) => {
                fx.write_u8(1);
                name.hash(&mut fx);
            }
        }
    }
    fx.finish()
}

/// One entry of an interned family list.
#[derive(Copy, Clone, Debug)]
enum FamilyEntry {
    /// A generic family.
    Generic(GenericFamily),
    /// A named family, whose name is these bytes of the names.
    Named(ByteRange),
}

impl FamilyEntry {
    /// Returns the family, reading a name from `names`, which holds every
    /// named family's name.
    fn name(self, names: &str) -> FamilyName<'_> {
        match self {
            Self::Generic(generic) => FamilyName::Generic(generic),
            Self::Named(name) => FamilyName::Named(names.get(name.range()).unwrap_or_default()),
        }
    }
}

/// One family of an interned list, as font selection reads it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum FamilyName<'a> {
    /// A generic family.
    Generic(GenericFamily),
    /// A named family.
    Named(&'a str),
}

impl<'a> From<&'a FontFamilyName<'_>> for FamilyName<'a> {
    /// Converts a caller's family to the form an interned list holds.
    fn from(family: &'a FontFamilyName<'_>) -> Self {
        match family {
            FontFamilyName::Generic(generic) => Self::Generic(*generic),
            FontFamilyName::Named(name) => Self::Named(name),
        }
    }
}

/// The layout's `font-family` lists, each stored once.
///
/// A font request's [`FamilyListId`] names one. Every list's entries share
/// one table, and every name shares one string, so interning a list
/// allocates nothing once their capacity is there.
pub(crate) struct FamilyLists {
    /// Each list's entries.
    lists: Table<FamilyListId, Range<FamilyEntryId>>,
    entries: Table<FamilyEntryId, FamilyEntry>,
    /// Every named family's name, one after another.
    names: String,
}

impl FamilyLists {
    /// Creates an empty table, allocating nothing.
    fn new() -> Self {
        Self {
            lists: Table::new(),
            entries: Table::new(),
            names: String::new(),
        }
    }

    /// Removes every list and keeps the allocations.
    fn clear(&mut self) {
        self.lists.clear();
        self.entries.clear();
        self.names.clear();
    }

    /// Returns the entries of the list `id` names, as stored, or none past
    /// the last.
    fn entries(&self, id: FamilyListId) -> &[FamilyEntry] {
        self.lists
            .get(id)
            .and_then(|entries| self.entries.get_slice(entries.clone()))
            .unwrap_or(&[])
    }

    /// Returns the id of `list`, found through `lookup` and interned if it
    /// is new, or `None` if it does not fit.
    fn intern(&mut self, list: &[FontFamilyName<'_>], lookup: &mut Lookup) -> Option<FamilyListId> {
        let hash = || hash_names(list.iter().map(FamilyName::from));
        let same = |id: FamilyListId| {
            let stored = self.entries(id);
            stored.len() == list.len()
                && stored
                    .iter()
                    .zip(list)
                    .all(|(stored, family)| match (stored, family) {
                        (FamilyEntry::Generic(a), FontFamilyName::Generic(b)) => a == b,
                        (FamilyEntry::Named(name), FontFamilyName::Named(b)) => {
                            self.names.get(name.range()) == Some(b.as_ref())
                        }
                        _ => false,
                    })
        };
        let hash = match lookup.find(self.lists.len(), hash, same) {
            Ok(id) => return Some(id),
            Err(hash) => hash,
        };
        // Checks that it fits before writing anything: a list is stored
        // whole or not at all.
        let bytes = list.iter().try_fold(0usize, |sum, family| match family {
            FontFamilyName::Named(name) => sum.checked_add(name.len()),
            FontFamilyName::Generic(_) => Some(sum),
        })?;
        u32::try_from(self.names.len().checked_add(bytes)?).ok()?;
        if self.lists.remaining() == 0 || self.entries.remaining() < list.len() {
            return None;
        }
        // Reserves room for the list's entries and names, whose sizes are
        // known, rather than doubling the allocation as they are written.
        self.entries.reserve(self.entries.len() + list.len());
        make_text_room(&mut self.names, bytes);
        let first = self.entries.next_id();
        for family in list {
            let entry = match family {
                FontFamilyName::Generic(generic) => FamilyEntry::Generic(*generic),
                FontFamilyName::Named(name) => {
                    // Both fit in `u32`: the total was checked above.
                    let start = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
                    self.names.push_str(name);
                    let end = u32::try_from(self.names.len()).unwrap_or(u32::MAX);
                    FamilyEntry::Named(ByteRange { start, end })
                }
            };
            self.entries
                .push_bounded(entry, "the entries' room was checked above");
        }
        let id = self.lists.push(first..self.entries.next_id())?;
        let Self {
            lists,
            entries,
            names,
        } = self;
        lookup.add(id, hash, |held| {
            let stored = lists
                .get(held)
                .and_then(|range| entries.get_slice(range.clone()))
                .unwrap_or(&[]);
            hash_names(stored.iter().map(|entry| entry.name(names)))
        });
        Some(id)
    }

    /// Yields every list's id, in the order they were interned.
    pub(crate) fn ids(&self) -> impl ExactSizeIterator<Item = FamilyListId> {
        self.lists.ids()
    }

    /// Yields the entries of the list `id` names, each a generic family or
    /// a name, or none past the last.
    pub(crate) fn get(&self, id: FamilyListId) -> impl ExactSizeIterator<Item = FamilyName<'_>> {
        self.entries(id).iter().map(|entry| entry.name(&self.names))
    }
}

heap_bytes! {
    FamilyLists { lists, entries, names }
}

/// Lists of fixed-size values, each list stored once.
///
/// It holds the layout's `font-feature-settings` ([`FeatureSettings`]), its
/// `font-variation-settings` ([`VariationSettings`]) and the bytes of its
/// `hyphenate-character` strings ([`HyphenStrings`]). A fact's id for them
/// names one. Every list's values share one table: `V` names each value in
/// it as `I` names each list.
pub(crate) struct InternedLists<I, V, T> {
    /// Each list's values.
    lists: Table<I, Range<V>>,
    values: Table<V, T>,
}

/// The layout's `font-feature-settings` lists.
type FeatureSettings = InternedLists<FeatureSettingsId, FontFeatureId, FontFeature>;

/// The layout's `font-variation-settings` lists.
type VariationSettings = InternedLists<VariationSettingsId, FontVariationId, FontVariation>;

impl<I: Id, V: Id, T: Same + Copy> InternedLists<I, V, T> {
    /// Creates an empty table, allocating nothing.
    const fn new() -> Self {
        Self {
            lists: Table::new(),
            values: Table::new(),
        }
    }

    /// Removes every list and keeps the allocations.
    fn clear(&mut self) {
        self.lists.clear();
        self.values.clear();
    }

    fn hash(list: &[T]) -> u64 {
        let mut fx = FxHasher::new();
        fx.write_usize(list.len());
        for value in list {
            value.feed(&mut fx);
        }
        fx.finish()
    }

    /// Returns the id of `list`, found through `lookup` and interned if it
    /// is new, or `None` if it does not fit.
    ///
    /// The empty list is the first, and is stored only once a list is
    /// stored after it. A layout that sets no list keeps nothing, and
    /// [`get`](Self::get) still answers the empty list's id past the last.
    fn intern(&mut self, list: &[T], lookup: &mut Lookup) -> Option<I> {
        if list.is_empty() {
            return Some(I::new(0));
        }
        let found = lookup.find(
            self.lists.len(),
            || Self::hash(list),
            |id| {
                let stored = self.get(id);
                stored.len() == list.len() && stored.iter().zip(list).all(|(a, b)| a.same(b))
            },
        );
        let hash = match found {
            Ok(id) => return Some(id),
            Err(hash) => hash,
        };
        // A list is stored whole or not at all, and the empty one first.
        let first = self.lists.is_empty();
        if self.lists.remaining() < 1 + usize::from(first) {
            return None;
        }
        if first {
            let none = self.values.next_id();
            self.lists
                .push_bounded(none..none, "the room for two lists was checked above");
        }
        let values = self.values.extend_from_slice(list)?;
        let id = self.lists.push(values)?;
        let Self { lists, values } = self;
        lookup.add(id, hash, |held| {
            let stored = lists
                .get(held)
                .and_then(|range| values.get_slice(range.clone()))
                .unwrap_or(&[]);
            Self::hash(stored)
        });
        Some(id)
    }

    /// Returns the values of the list `id` names, or none past the last.
    pub(crate) fn get(&self, id: I) -> &[T] {
        self.lists
            .get(id)
            .and_then(|values| self.values.get_slice(values.clone()))
            .unwrap_or(&[])
    }
}

impl<I, V, T> HeapBytes for InternedLists<I, V, T> {
    fn heap_bytes(&self) -> usize {
        let Self { lists, values } = self;
        lists.heap_bytes() + values.heap_bytes()
    }
}

/// The layout's content languages, each stored once.
///
/// A font request's and a script run's [`LanguageId`] names one.
pub(crate) struct Languages {
    languages: Table<LanguageId, Language>,
}

impl Languages {
    /// Creates an empty table, allocating nothing.
    fn new() -> Self {
        Self {
            languages: Table::new(),
        }
    }

    /// Removes every language and keeps the allocations.
    fn clear(&mut self) {
        self.languages.clear();
    }

    /// Returns the id of a language, `None` being `und`, found through
    /// `lookup` and interned if it is new.
    ///
    /// `und` is the first language, and is stored only once a language is
    /// stored after it. A layout whose text sets none keeps nothing, and
    /// [`get`](Self::get) still answers `und` past the last.
    fn intern(&mut self, language: Option<Language>, lookup: &mut Lookup) -> Option<LanguageId> {
        let language = language.unwrap_or(Language::UND);
        if language == Language::UND {
            return Some(LanguageId::UNDETERMINED);
        }
        if self.languages.is_empty() {
            self.languages
                .push_bounded(Language::UND, "an empty table has room");
        }
        lookup.intern(&mut self.languages, language)
    }

    /// Returns the language `id` names, or `und` past the last.
    pub(crate) fn get(&self, id: LanguageId) -> Language {
        self.languages.get(id).copied().unwrap_or(Language::UND)
    }
}

heap_bytes! {
    Languages { languages }
}

/// The layout's `hyphenate-character` strings, each stored once.
///
/// A text facts row's [`HyphenStringId`] names one. Each is interned as the
/// list of its bytes, which come from a `str` and read back as one.
pub(crate) struct HyphenStrings {
    strings: InternedLists<HyphenStringId, HyphenByteId, u8>,
}

impl HyphenStrings {
    /// Creates an empty table, allocating nothing.
    const fn new() -> Self {
        Self {
            strings: InternedLists::new(),
        }
    }

    /// Removes every string and keeps the allocations.
    fn clear(&mut self) {
        self.strings.clear();
    }

    /// Returns the id of `string`, found through `lookup` and interned if
    /// it is new, or `None` if it does not fit.
    fn intern(&mut self, string: &str, lookup: &mut Lookup) -> Option<HyphenStringId> {
        self.strings.intern(string.as_bytes(), lookup)
    }

    /// Returns the string `id` names, or an empty one past the last.
    ///
    /// Only a `str`'s bytes are interned, so they are always valid UTF-8.
    pub(crate) fn get(&self, id: HyphenStringId) -> &str {
        str::from_utf8(self.strings.get(id)).unwrap_or_default()
    }
}

heap_bytes! {
    HyphenStrings { strings }
}

/// The layout's lists, and the one entry every style's lists come in by.
///
/// Keying a caller's style interns its lists into these tables. It returns
/// the style with each list named by its table's id, a [`StyleKey`]. The
/// writer lowers the key into the facts and keeps it nowhere. The stages
/// read each list from its table, which this hands out.
pub(crate) struct Lists {
    /// How every table here, and every table of the content's facts, finds
    /// a value it holds already.
    lookup: Lookup,
    /// The `font-family` lists the font requests name.
    pub(crate) family_lists: FamilyLists,
    /// The languages the font requests name, which the analysis's script
    /// runs name too.
    pub(crate) languages: Languages,
    /// The lists only some styles name.
    ///
    /// They are made the first time a style names one, then cleared with
    /// the rest and kept, never dropped. Most layouts name none, and hold
    /// only the box's pointer.
    rare: Option<Box<RareLists>>,
}

/// The tables of [`Lists`] that only some styles fill.
///
/// They hold the `font-feature-settings` and `font-variation-settings`
/// lists, and the `hyphenate-character` strings. Only interning writes
/// them. Where no style named one, readers read [`RareLists::NONE`], whose
/// tables answer the initial ids.
struct RareLists {
    feature_settings: FeatureSettings,
    variation_settings: VariationSettings,
    hyphen_strings: HyphenStrings,
}

impl RareLists {
    /// Empty tables, which the lists read where no style names one.
    const NONE: Self = Self::new();

    const fn new() -> Self {
        Self {
            feature_settings: InternedLists::new(),
            variation_settings: InternedLists::new(),
            hyphen_strings: HyphenStrings::new(),
        }
    }

    fn clear(&mut self) {
        self.feature_settings.clear();
        self.variation_settings.clear();
        self.hyphen_strings.clear();
    }
}

impl Default for RareLists {
    fn default() -> Self {
        Self::new()
    }
}

heap_bytes! {
    RareLists { feature_settings, variation_settings, hyphen_strings }
}

/// The empty rare lists, read where no style names one.
static NO_RARE_LISTS: RareLists = RareLists::NONE;

impl Lists {
    /// Creates empty tables, allocating nothing.
    pub(super) fn new() -> Self {
        Self {
            lookup: Lookup::new(),
            family_lists: FamilyLists::new(),
            languages: Languages::new(),
            rare: None,
        }
    }

    /// Returns the rare lists, empty where no style names one.
    #[inline]
    fn rare(&self) -> &RareLists {
        self.rare.as_deref().unwrap_or(&NO_RARE_LISTS)
    }

    /// The `font-feature-settings` lists the font requests name.
    pub(crate) fn feature_settings(&self) -> &FeatureSettings {
        &self.rare().feature_settings
    }

    /// The `font-variation-settings` lists the font requests name.
    pub(crate) fn variation_settings(&self) -> &VariationSettings {
        &self.rare().variation_settings
    }

    /// The `hyphenate-character` strings the text facts name, which font
    /// selection sets a hyphen in.
    pub(crate) fn hyphen_strings(&self) -> &HyphenStrings {
        &self.rare().hyphen_strings
    }

    /// Empties every table, keeping its capacity, and interns the initial
    /// family list first, so that its id is a constant.
    ///
    /// The other initial values, no settings and `und`, take the first ids
    /// of their tables without being stored, until a value is stored after
    /// them.
    pub(super) fn clear(&mut self) {
        self.lookup.clear();
        self.family_lists.clear();
        self.languages.clear();
        if let Some(rare) = &mut self.rare {
            rare.clear();
        }
        let seeded = self
            .family_lists
            .intern(ComputedStyle::initial().font.families, &mut self.lookup);
        debug_assert_eq!(
            seeded,
            Some(FamilyListId::INITIAL),
            "the initial family list takes the first id"
        );
    }

    /// Keys `style` for the writer, interning its lists where they are new
    /// and naming each by its id.
    ///
    /// A list that does not fit its table is replaced by the initial one,
    /// and counted in `replaced`.
    pub(super) fn key(&mut self, style: &ComputedStyle<'_>, replaced: &mut usize) -> StyleKey {
        let (font, text) = (&style.font, &style.text);
        let lookup = &mut self.lookup;
        let families = self.family_lists.intern(font.families, lookup);
        let language = self.languages.intern(text.language, lookup);
        // Makes the rare lists' box the first time a style names one. A
        // style naming none takes their initial ids, which are never stored.
        let names_rare = !font.features.is_empty()
            || !font.variations.is_empty()
            || text.hyphenate_character.is_some();
        let (features, variations, hyphenate_character) = if names_rare {
            let rare = self.rare.get_or_insert_with(Box::default);
            let hyphenate_character = match text.hyphenate_character {
                // `auto`, whatever does or does not fit.
                None => None,
                Some(string) => or_initial(
                    rare.hyphen_strings.intern(string, lookup).map(Some),
                    None,
                    replaced,
                ),
            };
            (
                rare.feature_settings.intern(font.features, lookup),
                rare.variation_settings.intern(font.variations, lookup),
                hyphenate_character,
            )
        } else {
            (
                Some(FeatureSettingsId::EMPTY),
                Some(VariationSettingsId::EMPTY),
                None,
            )
        };
        StyleKey {
            font: FontKey::new(
                font,
                or_initial(families, FamilyListId::INITIAL, replaced),
                or_initial(features, FeatureSettingsId::EMPTY, replaced),
                or_initial(variations, VariationSettingsId::EMPTY, replaced),
            ),
            text: TextKey::new(
                text,
                or_initial(language, LanguageId::UNDETERMINED, replaced),
                hyphenate_character,
            ),
            line: style.line,
            bidi: style.bidi,
            orientation: style.orientation,
            edges: style.edges,
            ruby: style.ruby,
            paints: style.paints,
            decorates: style.decorates,
        }
    }

    /// Returns the languages and the lookup the content's facts find their
    /// rows by, which the writer lowers a style with (see `facts`).
    pub(super) fn lowering(&mut self) -> (&Languages, &mut Lookup) {
        (&self.languages, &mut self.lookup)
    }
}

heap_bytes! {
    Lists { lookup, family_lists, languages, rare }
}
