//! Context cache tests:
//! - each cache stays at its limit under endless distinct variation values,
//!   optical sizes, feature values, languages, family lists and vertical
//!   sizes;
//! - the faces of fonts a new collection lacks go, and their bytes with them;
//! - scratch grown for one huge paragraph is released after smaller ones;
//! - clearing the caches empties them and changes no result;
//! - a layout built before its fonts were evicted breaks as a fresh one does.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use fontwich::{FontBytes, LayerBuilder, Role};
use parlance::{FontFeature, FontVariation, Tag};

use super::{
    AHEM_FAMILY, Fixture, StageCheck, TestAxis, TestFont, ahem, ahem_fallback, allocator,
    collection, sized,
};
use crate::style::{FontFamilyName, Language, TextOrientation, WritingMode};
use crate::{CacheLimits, ComputedBlockStyle, Context, Item, Layout};

/// The limit every cache is set to.
const LIMIT: usize = 4;

/// How many distinct inputs each churn goes through: far past [`LIMIT`].
const CHURN: usize = 24;

/// The family of [`variable`].
const VARIABLE: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test Cache Axes"))];

/// ASCII with a weight axis from 100 to 900 and an optical size axis from 6
/// to 72, each moving every advance.
fn variable() -> TestFont {
    let mut font = TestFont::new("Test Cache Axes", &[(0x20, 0x7E)]);
    font.axes = vec![
        TestAxis {
            tag: *b"wght",
            min: 100.0,
            default: 400.0,
            max: 900.0,
            delta: 200,
        },
        TestAxis {
            tag: *b"opsz",
            min: 6.0,
            default: 12.0,
            max: 72.0,
            delta: 50,
        },
    ];
    font
}

/// Returns a fixture over Ahem and [`variable`], every cache limited to
/// `limit`.
fn fixture(limit: usize) -> Fixture {
    let mut fixture = Fixture::new(&[variable()], ahem_fallback(), StageCheck::Placed(|_| {}));
    fixture.cx.set_cache_limits(limits(limit));
    fixture
}

/// Returns every limit at `limit`.
fn limits(limit: usize) -> CacheLimits {
    CacheLimits {
        family_lists: limit,
        fallback_lists: limit,
        font_instances: limit,
        shape_plans: limit,
    }
}

/// How many entries each cache holds once trimmed: the family lists,
/// candidate lists, instances, faces, offers and coverages, then the
/// languages, plans, trim answers, fonts set down the line and their glyphs.
fn trimmed_counts(cx: &mut Context) -> [usize; 11] {
    // Setting the limits again trims to them, as the next call would.
    cx.set_cache_limits(*cx.cache_limits());
    let fonts = cx.font_context().counts();
    let shaping = cx.shaping().counts();
    let mut counts = [0; 11];
    counts[..6].copy_from_slice(&fonts);
    counts[6..].copy_from_slice(&shaping);
    counts
}

/// Builds and breaks [`CHURN`] layouts, the `n`th made by `build(n)`, in a
/// context limited to `limit`, and returns the most each cache held once
/// trimmed.
fn churn(limit: usize, mut build: impl FnMut(&mut Fixture, &mut Layout, usize)) -> [usize; 11] {
    let mut fixture = fixture(limit);
    let mut layout = Layout::new();
    let mut most = [0; 11];
    for n in 0..CHURN {
        build(&mut fixture, &mut layout, n);
        fixture.lay_out(&mut layout, 100.0);
        for (most, count) in most.iter_mut().zip(trimmed_counts(&mut fixture.cx)) {
            *most = (*most).max(count);
        }
    }
    most
}

/// Asserts that the caches at `bounded` each kept at most [`LIMIT`], the
/// vertical glyphs at most `glyphs` a font, and that those at `churned`
/// went past it without a limit.
fn assert_bounded(
    build: impl Fn(&mut Fixture, &mut Layout, usize),
    churned: &[usize],
    glyphs: usize,
) {
    let free = churn(usize::MAX, &build);
    for &at in churned {
        assert!(free[at] > LIMIT, "cache {at} churns: {free:?}");
    }
    let held = churn(LIMIT, &build);
    for (at, &count) in held.iter().enumerate().take(10) {
        assert!(count <= LIMIT, "cache {at} holds {count}: {held:?}");
    }
    assert!(held[3] <= held[2].max(1), "a face an instance: {held:?}");
    assert!(held[10] <= glyphs * LIMIT, "glyphs of held fonts: {held:?}");
}

/// Distinct `font-variation-settings` each make an instance, and the
/// instances stay at their limit.
#[test]
fn distinct_variation_values_keep_the_instances_at_their_limit() {
    assert_bounded(
        |fixture, layout, n| {
            let settings = [FontVariation::new(
                Tag::new(b"wght"),
                100.0 + 25.0 * n as f32,
            )];
            let mut style = sized(&VARIABLE, 16.0);
            style.font.variations = &settings;
            fixture.text(layout, &style, "abc def");
        },
        &[2],
        0,
    );
}

/// Each size of a font with an optical size axis makes an instance under
/// `font-optical-sizing: auto`, and the instances stay at their limit.
#[test]
fn distinct_optical_sizes_keep_the_instances_at_their_limit() {
    assert_bounded(
        |fixture, layout, n| {
            let style = sized(&VARIABLE, 8.0 + n as f32);
            fixture.text(layout, &style, "abc def");
        },
        &[2],
        0,
    );
}

/// Distinct feature values each make an instance and a plan, and both stay
/// at their limits.
#[test]
fn distinct_feature_values_keep_the_instances_and_plans_at_their_limits() {
    assert_bounded(
        |fixture, layout, n| {
            let value = u16::try_from(n).unwrap_or(u16::MAX);
            let features = [FontFeature::new(Tag::new(b"ss01"), value)];
            let mut style = ahem(16.0);
            style.font.features = &features;
            fixture.text(layout, &style, "abc def");
        },
        &[2, 7],
        0,
    );
}

/// Distinct languages each make a language, a plan and fallback lists, and
/// each stays at its limit.
#[test]
fn distinct_languages_keep_the_languages_plans_and_lists_at_their_limits() {
    assert_bounded(
        |fixture, layout, n| {
            let letter = |at: usize| char::from(b'a' + u8::try_from(at % 26).unwrap_or(0));
            let tag = format!("q{}{}", letter(n / 26), letter(n));
            let mut style = ahem(16.0);
            style.text.language = Language::parse(&tag).ok();
            assert!(style.text.language.is_some(), "{tag} is a language");
            fixture.text(layout, &style, "abc def");
        },
        &[1, 6, 7],
        0,
    );
}

/// Distinct `font-family` lists each make a family list and fallback lists,
/// and both stay at their limits.
#[test]
fn distinct_family_lists_keep_the_lists_at_their_limits() {
    assert_bounded(
        |fixture, layout, n| {
            let families = [
                FontFamilyName::Named(Cow::Owned(format!("Missing {n}"))),
                FontFamilyName::named("Ahem"),
            ];
            let style = sized(&families, 16.0);
            fixture.text(layout, &style, "abc def");
        },
        &[0, 1],
        0,
    );
}

/// Each size upright text is set down the line at makes a vertical font,
/// and the vertical fonts stay at their limit, with only their glyphs.
#[test]
fn distinct_vertical_sizes_keep_the_vertical_fonts_at_their_limit() {
    assert_bounded(
        |fixture, layout, n| {
            let mut style = ahem(10.0 + n as f32);
            style.orientation.text_orientation = TextOrientation::Upright;
            let block = ComputedBlockStyle {
                writing_mode: WritingMode::VerticalRl,
                ..ComputedBlockStyle::new(&style)
            };
            fixture.build_spans(layout, &block, &[(&style, "ab")]);
        },
        &[9],
        2,
    );
}

/// A collection without a web font drops its faces and instances, so once
/// no layout uses it, nothing holds its bytes but whoever made them.
#[test]
fn a_collection_without_a_font_releases_its_bytes() {
    let bytes = FontBytes::new(variable().build());
    let mut web = LayerBuilder::new(Role::Document);
    assert!(web.add_data(bytes.clone()).is_ok());
    let fonts = collection(&[], ahem_fallback()).with_layer(web.snapshot());
    drop(web);
    let mut fixture = Fixture::from_collection(fonts, &[], StageCheck::Placed(|_| {}));
    let mut layout = Layout::new();
    fixture.text(&mut layout, &sized(&VARIABLE, 16.0), "abc def");
    fixture.lay_out(&mut layout, 100.0);
    let shared = Arc::strong_count(bytes.arc());
    assert!(shared > 2, "the layer, the face and the layout share it");
    assert!(fixture.cx.heap_bytes().font_data >= bytes.data().len());

    // A new collection with the same layers under one more keeps the face.
    let more = fixture
        .cx
        .collection()
        .clone()
        .with_layer(LayerBuilder::new(Role::Document).snapshot());
    fixture.cx.set_collection(more);
    assert!(fixture.cx.font_context().counts()[3] > 0, "the face stays");

    drop(layout);
    fixture.cx.set_collection(collection(&[], ahem_fallback()));
    assert_eq!(Arc::strong_count(bytes.arc()), 1, "only this test holds it");
    assert_eq!(fixture.cx.heap_bytes().font_data, 0);
}

/// One huge paragraph grows the scratch; a run of small ones releases it,
/// after which small relayouts keep their scratch and allocate nothing.
#[test]
fn scratch_grown_for_one_huge_paragraph_is_released_after_small_ones() {
    let mut fixture = fixture(LIMIT);
    let mut layout = Layout::new();
    let style = ahem(16.0);
    let huge = "word ".repeat(40_000);
    fixture.text(&mut layout, &style, &huge);
    fixture.lay_out(&mut layout, 400.0);
    let grown = fixture.cx.heap_bytes().scratch;
    for _ in 0..12 {
        fixture.text(&mut layout, &style, "a few small words");
        fixture.lay_out(&mut layout, 400.0);
    }
    let small = fixture.cx.heap_bytes().scratch;
    assert!(small * 8 < grown, "{small} bytes kept of {grown}");
    for _ in 0..20 {
        fixture.text(&mut layout, &style, "a few small words");
        fixture.lay_out(&mut layout, 400.0);
    }
    assert_eq!(fixture.cx.heap_bytes().scratch, small, "steady");
    let allocations = allocator::count_allocations(|| fixture.lay_out(&mut layout, 300.0));
    assert_eq!(allocations, 0, "a small relayout allocates nothing");
}

/// Every glyph of `layout` as a painter reads it, with its line's text.
fn drawn(layout: &Layout) -> Vec<(String, u32, f32, f32, f32)> {
    let text = &layout.content().text;
    let mut out = Vec::new();
    for line in layout.lines() {
        let words = String::from(&text[line.text_range()]);
        for item in line.items() {
            if let Item::Text(run) = item {
                out.extend(
                    run.glyphs()
                        .map(|g| (words.clone(), g.id, g.x, g.y, g.advance)),
                );
            }
        }
    }
    out
}

/// Clearing the caches empties every cache and all scratch, and a layout
/// breaks and builds again as before.
#[test]
fn clearing_the_caches_empties_them_and_changes_no_result() {
    let mut fixture = fixture(64);
    let mut layout = Layout::new();
    let style = sized(&VARIABLE, 20.0);
    let text = "office flat waffle 漢字 fit";
    fixture.text(&mut layout, &style, text);
    fixture.lay_out(&mut layout, 60.0);
    let before = drawn(&layout);
    assert!(fixture.cx.heap_bytes().total() > 0);

    fixture.cx.clear_caches();
    let heap = fixture.cx.heap_bytes();
    assert_eq!((heap.total(), heap.font_data), (0, 0), "{heap:?}");
    assert_eq!(trimmed_counts(&mut fixture.cx), [0; 11]);
    assert_eq!(*fixture.cx.cache_limits(), limits(64), "the limits stay");

    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(drawn(&layout), before, "broken again");
    fixture.cx.clear_caches();
    fixture.text(&mut layout, &style, text);
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(drawn(&layout), before, "built again");
}

/// A layout built before its fonts were evicted breaks at its old width as
/// it did, and at new widths as a fresh layout in a fresh context does: it
/// holds its fonts, and the misses are made again from them.
#[test]
fn a_relayout_after_eviction_equals_a_fresh_layout() {
    let style = |weight: f32| {
        let settings = vec![FontVariation::new(Tag::new(b"wght"), weight)];
        let mut style = sized(&VARIABLE, 20.0);
        style.font.variations = settings.leak();
        style
    };
    let text = "office AVAVA flat fit ffi AVAIL waffle";
    let mut fixture = fixture(1);
    let mut layout = Layout::new();
    fixture.text(&mut layout, &style(700.0), text);
    fixture.lay_out(&mut layout, 60.0);
    let warm = drawn(&layout);

    // Other weights push its instance, plans and lists out.
    let mut other = Layout::new();
    for n in 0..8 {
        fixture.text(&mut other, &style(200.0 + 50.0 * n as f32), text);
        fixture.lay_out(&mut other, 60.0);
    }
    let counts = trimmed_counts(&mut fixture.cx);
    assert!(counts[2] <= 1, "{counts:?}");

    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(drawn(&layout), warm, "at the old width");
    for width in [28.0, 40.0, 90.0] {
        fixture.lay_out(&mut layout, width);
        let mut fresh = Fixture::new(&[variable()], ahem_fallback(), StageCheck::Placed(|_| {}));
        let mut new = Layout::new();
        fresh.text(&mut new, &style(700.0), text);
        fresh.lay_out(&mut new, width);
        assert_eq!(drawn(&layout), drawn(&new), "at {width} px");
    }
}

/// A family the collection has is set in, not the fallback, so the tests
/// above churn the caches they name.
#[test]
fn the_churned_families_are_set_in_their_fonts() {
    let mut fixture = fixture(LIMIT);
    let mut layout = Layout::new();
    fixture.text(&mut layout, &sized(&VARIABLE, 16.0), "abc");
    let ahem_drawn = {
        let mut other = Layout::new();
        fixture.text(&mut other, &sized(&AHEM_FAMILY, 16.0), "abc");
        fixture.lay_out(&mut other, 100.0);
        drawn(&other)
    };
    fixture.lay_out(&mut layout, 100.0);
    assert_ne!(drawn(&layout), ahem_drawn);
}
