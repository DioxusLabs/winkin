//! Shifted boxes, atomic inlines and a trimmed block allocate nothing warm.

use super::count_allocations;
use super::test_fonts::{self, TestFont, ahem_fallback};
use fontwich::Collection;
use winkin::config::{Config, DominantBaselines};
use winkin::style::{
    ComputedStyle, DominantBaseline, EdgesGroup, FontFamilyName, FontGroup, LineGroup, LineHeight,
    Sides, TextBoxEdge, TextBoxTrim, TextEdgeOver, TextEdgeUnder, VerticalAlign,
};
use winkin::{
    Area, BoxSize, BuildOptions, ComputedBlockStyle, Context, Item, Layout, NoExclusions, NodeKey,
};

/// Ahem, and Latin in a font of other metrics.
fn collection() -> Collection {
    let latin = TestFont::new("Test Latin", &[(0x20, 0x7E)]);
    test_fonts::collection(&[latin], ahem_fallback())
}

/// Paragraphs whose boxes and atomic inlines take every value of
/// `vertical-align`, nested and pinned to the line inside each other,
/// with edges, in another font and at other sizes, some on their
/// `dominant-baseline`, `repeat` times over, in a block that trims both
/// its ends: so the measure stage writes shifts, the breaker walks lines
/// box by box and settles what waits, and line layout places each item
/// on its box's baseline.
fn document(layout: &mut Layout, cx: &mut Context, repeat: usize) {
    let families = [
        FontFamilyName::named("Ahem"),
        FontFamilyName::named("Test Latin"),
    ];
    let latin = [FontFamilyName::named("Test Latin")];
    let root = ComputedStyle {
        line: LineGroup {
            height: LineHeight::Factor(1.5),
            ..LineGroup::INITIAL
        },
        font: FontGroup {
            families: &families,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let aligns = [
        VerticalAlign::Super,
        VerticalAlign::Sub,
        VerticalAlign::Middle,
        VerticalAlign::TextTop,
        VerticalAlign::TextBottom,
        VerticalAlign::Top,
        VerticalAlign::Bottom,
        VerticalAlign::Px(3.5),
        VerticalAlign::Fraction(-0.25),
        VerticalAlign::Baseline,
    ];
    let styled = |n: usize| ComputedStyle {
        font: FontGroup {
            families: if n.is_multiple_of(3) {
                &latin
            } else {
                &families
            },
            size: 10.0 + (n % 4) as f32 * 6.0,
            ..FontGroup::INITIAL
        },
        line: LineGroup {
            vertical_align: aligns[n % aligns.len()],
            dominant_baseline: if n.is_multiple_of(5) {
                DominantBaseline::Central
            } else {
                DominantBaseline::Auto
            },
            ..root.line
        },
        edges: EdgesGroup {
            padding: Sides {
                top: 1.0,
                left: 2.0,
                ..Sides::ZERO
            },
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let block = ComputedBlockStyle {
        text_box_trim: TextBoxTrim::TrimBoth,
        text_box_edge: TextBoxEdge {
            over: TextEdgeOver::Cap,
            under: TextEdgeUnder::Alphabetic,
        },
        ..ComputedBlockStyle::new(&root)
    };
    let mut b = layout.builder(NodeKey(0), &block, BuildOptions::default());
    let mut key = 0;
    let mut next = || {
        key += 1;
        NodeKey(key)
    };
    for n in 0..repeat {
        b.text(next(), "plain words ahead ");
        b.open_box(next(), &styled(n), None);
        b.text(next(), "shifted text ");
        b.open_box(next(), &styled(n + 5), None);
        b.text(next(), "pinned inside ");
        b.atomic(
            next(),
            &styled(n + 2),
            None,
            BoxSize {
                inline: 12.0,
                block: 8.0 + (n % 5) as f32 * 9.0,
                baseline: (n % 2 == 0).then_some(6.0),
            },
        );
        b.close_box();
        b.text(next(), " after ");
        b.close_box();
        b.atomic(
            next(),
            &styled(n + 6),
            None,
            BoxSize {
                inline: 20.0,
                block: 30.0,
                baseline: None,
            },
        );
        if n % 4 == 3 {
            b.line_break(next());
        }
    }
    assert!(b.finish(cx).is_complete());
}

/// Reads every line back: its metrics, and every item's place across it.
fn read(layout: &Layout) -> f32 {
    let mut sum = layout.metrics().block_end + layout.metrics().trim_start;
    for line in layout.lines() {
        sum += line.metrics().baseline;
        for item in line.all_items() {
            sum += match item {
                Item::Text(run) | Item::Generated(run) => run.baseline() + run.block().over,
                Item::Atomic(atomic) => atomic.baseline() + atomic.block().under,
                Item::Box(piece) => piece.baseline() + piece.block().over,
            };
        }
    }
    for key in 0..200 {
        for piece in layout.box_fragments(NodeKey(key)) {
            sum += piece.baseline();
        }
    }
    sum
}

/// A warm rebuild of shifted boxes, atomic inlines and a trimmed block
/// allocates nothing, and nor does breaking it again at any width broken
/// at before, nor reading it back, whether `dominant-baseline` moves
/// boxes or not: the shifts, the breaker's stack of open boxes and what
/// waits in it, and the lines' settled shifts keep their capacity.
#[test]
fn shifted_boxes_atomics_and_trim_allocate_nothing_warm() {
    let widths = [40.0, 95.5, 230.0, 700.0, 3.0];
    for baselines in [DominantBaselines::Ignored, DominantBaselines::Applied] {
        let mut cx = Context::new(collection());
        let mut config = Config::chrome_windows();
        config.dominant_baseline = baselines;
        cx.set_config(config);
        let mut layout = Layout::new();
        let cold = count_allocations(|| document(&mut layout, &mut cx, 16));
        assert!(cold > 0, "a cold layout grows");
        let warm = count_allocations(|| document(&mut layout, &mut cx, 16));
        assert_eq!(warm, 0, "{baselines:?}: rebuilding allocated");
        let warm = count_allocations(|| document(&mut layout, &mut cx, 9));
        assert_eq!(
            warm, 0,
            "{baselines:?}: rebuilding something smaller allocated"
        );
        document(&mut layout, &mut cx, 16);
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            read(&layout);
        }
        let warm = count_allocations(|| {
            for &width in &widths {
                layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            }
        });
        assert_eq!(warm, 0, "{baselines:?}: breaking again allocated");
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            let mut sum = 0.0;
            let warm = count_allocations(|| sum = read(&layout));
            assert!(sum.is_finite());
            assert_eq!(warm, 0, "{baselines:?} at {width}: reading back allocated");
        }
    }
}
