//! Split ruby tests. They pin:
//! - annotation offsets following their continuation line for carets, hits
//!   and rectangles, while copying keeps the author's order;
//! - annotation edges reshaped inside a ligature.

use super::*;

/// Annotation offsets follow their physical continuation line for carets,
/// hit testing and selection, while copying keeps the author's text order.
#[test]
fn split_ruby_annotations_keep_their_selection_positions() {
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 200.0, |b| {
        b.text(NodeKey(1), "XX ");
        b.open_ruby(NodeKey(2), &ahem(), None);
        b.text(NodeKey(3), "AAAA BBBB CCCC DDDD");
        b.open_annotation(NodeKey(4), &styled(|style| style.font.size = 10.0), None);
        b.text(NodeKey(5), "aaaa bbbb cccc dddd");
        b.close_ruby();
        b.text(NodeKey(6), " ZZ");
    });
    let annotation_start = layout.text().find("aaaa").unwrap();
    for (word, index) in [("aaaa", 0), ("bbbb", 1), ("cccc", 1), ("dddd", 2)] {
        let at = layout.text().find(word).unwrap();
        assert_eq!(layout.caret(Position::from(at)).unwrap().line, index);
        let rects: Vec<_> = layout.selection_rects(at..at + word.len()).collect();
        assert!(!rects.is_empty());
        assert!(rects.iter().all(|rect| rect.line == index));
        let line = layout.line(index).unwrap();
        let run = line
            .annotation_runs()
            .find(|run| run.text_range().contains(&at))
            .unwrap();
        let block = run.block();
        let caret = layout.caret(Position::from(at + 1)).unwrap();
        let hit = layout
            .hit_test(
                line.metrics().left + caret.inline.left,
                line.metrics().top + (block.over + block.under) / 2.0,
                PastLines::Column,
            )
            .unwrap();
        assert_eq!(hit.offset, at + 1);
    }
    assert_eq!(
        layout
            .selected_text(annotation_start..annotation_start + 19, CopyKind::Text)
            .to_string(),
        "aaaa bbbb cccc dddd"
    );
}

/// An annotation break inside a ligature shapes each visible piece with
/// the same advances as that piece laid out on its own.
#[test]
fn split_ruby_annotation_edges_reshape_ligatures() {
    const LATIN: [FontFamilyName<'static>; 1] =
        [FontFamilyName::Named(Cow::Borrowed("Test Latin"))];
    let mut base = ahem();
    base.font.families = &LATIN;
    base.text.word_break = WordBreak::BreakAll;
    let mut small = base;
    small.font.size = 10.0;
    let layout = laid_with(&ComputedBlockStyle::new(&base), 40.0, |b| {
        b.open_ruby(NodeKey(1), &base, None);
        b.text(NodeKey(2), "ABCDEFGHIJKL");
        b.open_annotation(NodeKey(3), &small, None);
        b.text(NodeKey(4), "fifififififififififififi");
        b.close_ruby();
    });
    assert!(layout.lines().count() > 1);
    let mut reconstructed = String::new();
    for line in layout.lines() {
        for annotation in line.annotations() {
            // The glyphs' span, which leaves out the room `ruby-align` puts
            // beside the text.
            let span = |xs: &[f32]| {
                xs.last()
                    .zip(xs.first())
                    .map_or(0.0, |(last, first)| last - first)
            };
            let mut text = String::new();
            let mut xs = Vec::new();
            for run in annotation.runs() {
                text.push_str(&layout.text()[run.text_range()]);
                xs.extend(run.glyphs().map(|glyph| glyph.x));
            }
            let actual = span(&xs);
            reconstructed.push_str(&text);
            let separate = laid_with(&ComputedBlockStyle::new(&small), 1000.0, |b| {
                b.text(NodeKey(1), &text)
            });
            let alone: Vec<f32> = separate
                .line(0)
                .unwrap()
                .items()
                .filter_map(|item| match item {
                    Item::Text(run) => Some(run.glyphs().map(|glyph| glyph.x).collect::<Vec<_>>()),
                    _ => None,
                })
                .flatten()
                .collect();
            let expected = span(&alone);
            assert!(
                (actual - expected).abs() < 0.02,
                "{text}: {actual} versus {expected}"
            );
        }
    }
    assert_eq!(reconstructed, "fifififififififififififi");
}
