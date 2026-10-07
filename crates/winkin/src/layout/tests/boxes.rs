//! Box tests. They pin:
//! - the room between items, the edges of the boxes between them;
//! - culled boxes answered from what they hold, split by reordering;
//! - atomic inlines as their margin boxes.

use super::*;

/// A box's edges' room along a line, `left` at its left and `right` at its
/// right.
fn edge_room(left: f32, right: f32) -> InlineEdges {
    InlineEdges { left, right }
}

/// The room between two items is exactly the edges of the boxes between
/// them, which each kept box's part says: margin, border and padding on
/// its own sides, and nothing on an open one.
#[test]
fn the_room_between_items_is_the_edges_of_the_boxes_between_them() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 10.0);
    let roomy = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::all(2.0),
            border: Sides::all(1.0),
            padding: Sides::all(3.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let odd = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides {
                left: 0.3,
                right: -1.2,
                ..Sides::ZERO
            },
            padding: Sides::all(0.7),
            ..EdgesGroup::INITIAL
        },
        paints: true,
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &roomy, None);
        b.text(NodeKey(2), "XX ");
        b.open_box(NodeKey(3), &odd, None);
        b.text(NodeKey(4), "YY ZZ");
        b.close_box();
        b.close_box();
        b.text(NodeKey(5), " WW");
        b.open_box(NodeKey(6), &roomy, None);
        b.close_box();
    });
    for width in [1000.0, 70.0, 45.0, 10.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        walk(&layout);
    }
    // All on one line: the walk ends past the empty box at the end.
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    let end = walk(&layout)[0];
    // The odd box's edges truncated onto the grid, as Chrome holds them:
    // 0.3 is 19/64, 0.7 is 44/64 and -1.2 is -76/64.
    let expected = 6.0 + 30.0 + (0.296875 + 0.6875) + 50.0 + (0.6875 - 1.1875) + 6.0 + 30.0 + 12.0;
    assert!((end - expected).abs() < 1e-3, "{end} against {expected}");
    // Broken inside the outer box, its first part is open to the right
    // and its second to the left, with no room there.
    layout.break_lines(&mut cx, Area::new(45.0), &mut NoExclusions);
    let first: Vec<_> = layout
        .box_fragments(NodeKey(1))
        .map(|piece| {
            (
                piece.line(),
                piece.edges(),
                piece.is_open_left(),
                piece.is_open_right(),
            )
        })
        .collect();
    assert_eq!(first[0], (0, edge_room(6.0, 0.0), false, true));
    assert_eq!(first[1].1.left, 0.0);
    assert!(first[1].2);
}

/// A box that keeps a fragment is answered with its items; one that is
/// culled, from its descendants', a part a line, as wide as they are and
/// as tall as its text, open where it goes on past a line.
#[test]
fn a_culled_box_is_answered_from_what_it_holds() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 10.0);
    let paints = ComputedStyle {
        paints: true,
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "AA ");
        b.open_box(NodeKey(2), &root, None);
        b.text(NodeKey(3), "BB CC ");
        b.open_box(NodeKey(4), &paints, None);
        b.text(NodeKey(5), "DD");
        b.close_box();
        b.close_box();
        b.text(NodeKey(6), " EE");
    });
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    let culled: Vec<_> = layout.box_fragments(NodeKey(2)).collect();
    assert_eq!(culled.len(), 1);
    let piece = culled[0];
    assert!(piece.is_culled());
    assert_eq!((piece.line(), piece.key()), (0, NodeKey(2)));
    assert_eq!(
        piece.inline(),
        along(30.0, 110.0),
        "`BB CC ` and the kept box's `DD`"
    );
    assert_eq!(piece.block(), across(0.0, 10.0));
    assert_eq!(piece.baseline(), 8.0);
    assert!(!piece.is_open_left() && !piece.is_open_right());
    assert_eq!(piece.edges(), edge_room(0.0, 0.0));
    let kept: Vec<_> = layout.box_fragments(NodeKey(4)).collect();
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].is_culled());
    assert_eq!(kept[0].inline(), along(90.0, 110.0));
    assert_eq!(kept[0].descendants(), 1);
    // The same as the line's own box item.
    let boxes: Vec<_> = layout
        .line(0)
        .expect("a line")
        .items()
        .filter_map(|item| match item {
            Item::Box(piece) => Some(piece.inline()),
            _ => None,
        })
        .collect();
    assert_eq!(boxes, [kept[0].inline()]);
    // Across two lines: a part on each, open on the side it goes on
    // past, and none for a key no box has.
    layout.break_lines(&mut cx, Area::new(60.0), &mut NoExclusions);
    let texts: Vec<&str> = layout
        .lines()
        .map(|line| &layout.text()[line.text_range()])
        .collect();
    assert_eq!(texts, ["AA BB ", "CC DD ", "EE"]);
    let culled: Vec<_> = layout
        .box_fragments(NodeKey(2))
        .map(|piece| {
            (
                piece.line(),
                piece.inline(),
                piece.is_open_left(),
                piece.is_open_right(),
            )
        })
        .collect();
    assert_eq!(
        culled,
        [
            (0, along(30.0, 50.0), false, true),
            (1, along(0.0, 50.0), true, false)
        ]
    );
    assert_eq!(layout.box_fragments(NodeKey(3)).count(), 0, "a text node");
    assert_eq!(layout.box_fragments(NodeKey(99)).count(), 0);
}

/// A culled box gives one part for each group of what it holds that
/// reordering leaves side by side, as Chrome answers a culled inline.
///
/// In `A<b>B<i>CD</i></b><i>EF</i>G` at 40 px, with both `i`s overriding
/// right to left, the outer `b` is `[40, 80]` and `[160, 240]`. Each `i` is
/// one part, `[160, 240]` and `[80, 160]`, with no empty part where an edge
/// lands. Its own edges bound its leftmost part's left and its rightmost
/// part's right. The start is on the left where it reads left to right.
#[test]
fn a_culled_box_split_by_reordering_has_a_part_each_side() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 40.0);
    let overriding = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            unicode_bidi: UnicodeBidi::BidiOverride,
        },
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &root, None);
        b.text(NodeKey(3), "B");
        b.open_box(NodeKey(4), &overriding, None);
        b.text(NodeKey(5), "CD");
        b.close_box();
        b.close_box();
        b.open_box(NodeKey(6), &overriding, None);
        b.text(NodeKey(7), "EF");
        b.close_box();
        b.text(NodeKey(8), "G");
    });
    layout.break_lines(&mut cx, Area::new(800.0), &mut NoExclusions);
    let parts = |key: u64| -> Vec<(InlineExtents, bool, bool)> {
        layout
            .box_fragments(NodeKey(key))
            .map(|piece| {
                assert!(piece.is_culled());
                (piece.inline(), piece.is_open_left(), piece.is_open_right())
            })
            .collect()
    };
    assert_eq!(
        parts(2),
        [
            (along(40.0, 80.0), false, true),
            (along(160.0, 240.0), true, false)
        ]
    );
    // Right to left, the start of each `i` is on its right.
    assert_eq!(parts(4), [(along(160.0, 240.0), false, false)]);
    assert_eq!(parts(6), [(along(80.0, 160.0), false, false)]);
}

/// An atomic inline is its margin box: as wide as its size and its margins,
/// its baseline where it said, its top and bottom around it.
#[test]
fn an_atomic_inline_is_its_margin_box() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let margined = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::all(1.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let size = BoxSize {
        inline: 24.0,
        block: 28.0,
        baseline: Some(14.0),
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX");
        b.atomic(NodeKey(2), &margined, None, size);
        b.text(NodeKey(3), "XX");
    });
    layout.break_lines(&mut cx, Area::new(500.0), &mut NoExclusions);
    let atomic = layout
        .line(0)
        .expect("a line")
        .items()
        .find_map(|item| match item {
            Item::Atomic(atomic) => Some(atomic),
            _ => None,
        })
        .expect("the atomic");
    assert_eq!(atomic.key(), NodeKey(2));
    assert_eq!(
        (atomic.inline(), atomic.advance()),
        (along(40.0, 66.0), 26.0)
    );
    assert_eq!(atomic.baseline(), 16.0);
    // Its margin box: 14 over the baseline and a margin, the rest under.
    assert_eq!(atomic.block(), across(1.0, 31.0));
    walk(&layout);
}

/// An empty box after the space a line ends at is on that line, before the
/// collapsed space, as Chrome's `RewindOverflow` keeps the boxes that open
/// and close at a break. Chrome 153, 10px Ahem in 50px, the web platform
/// test `trailing-space-position-001`: in `1234 <span> </span>567` the
/// span's space collapses and the span stands on the first line at 40; in
/// `1234567 <span> </span>567` at 70; an empty span with 3px of padding at
/// 40, 3 wide. A span holding text starts the next line, and so does an
/// empty span inside it: in `1234 <b><span></span>567</b>`, with 2px of
/// border on the left of the `b`, the empty span is on the second line at
/// 2.
#[test]
fn an_empty_box_after_a_lines_last_space_ends_the_line() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides {
                left: 3.0,
                ..Sides::ZERO
            },
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let cases = [
        ("1234 ", " ", &root, (0, along(40.0, 40.0))),
        ("1234567 ", " ", &root, (0, along(70.0, 70.0))),
        ("1234 ", "", &root, (0, along(40.0, 40.0))),
        ("1234 ", "", &padded, (0, along(40.0, 43.0))),
        ("1234 ", "56", &root, (1, along(0.0, 20.0))),
    ];
    for (before, inside, style, expected) in cases {
        let mut cx = context();
        let mut layout = Layout::new();
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), before);
            b.open_box(NodeKey(2), style, None);
            b.text(NodeKey(3), inside);
            b.close_box();
            b.text(NodeKey(4), "567");
        });
        layout.break_lines(&mut cx, Area::new(50.0), &mut NoExclusions);
        let pieces: Vec<_> = layout
            .box_fragments(NodeKey(2))
            .map(|piece| (piece.line(), piece.inline()))
            .collect();
        assert_eq!(pieces, [expected], "{before:?} {inside:?}");
    }
    let bordered = ComputedStyle {
        edges: EdgesGroup {
            border: Sides {
                left: 2.0,
                ..Sides::ZERO
            },
            ..EdgesGroup::INITIAL
        },
        paints: true,
        ..root
    };
    let mut cx = context();
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "1234 ");
        b.open_box(NodeKey(2), &bordered, None);
        b.open_box(NodeKey(3), &root, None);
        b.close_box();
        b.text(NodeKey(4), "567");
        b.close_box();
    });
    layout.break_lines(&mut cx, Area::new(50.0), &mut NoExclusions);
    let pieces = |key| -> Vec<_> {
        layout
            .box_fragments(NodeKey(key))
            .map(|piece| (piece.line(), piece.inline()))
            .collect()
    };
    assert_eq!(pieces(2), [(1, along(0.0, 32.0))]);
    assert_eq!(pieces(3), [(1, along(2.0, 2.0))]);
}
