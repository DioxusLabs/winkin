//! Orientation run tests. They pin:
//! - how each cluster stands in a vertical line under `text-orientation`;
//! - the units `text-combine-upright` combines.

use alloc::vec::Vec;

use super::*;
use crate::style::TextCombineUpright;

/// A block in `style` whose lines run as `writing_mode` says.
fn lines_run<'a>(
    writing_mode: WritingMode,
    style: &'a ComputedStyle<'a>,
) -> ComputedBlockStyle<'a> {
    ComputedBlockStyle {
        writing_mode,
        ..ComputedBlockStyle::new(style)
    }
}

/// The initial style set `text-orientation: orientation`.
fn oriented(orientation: TextOrientation) -> ComputedStyle<'static> {
    styled(|style| style.orientation.text_orientation = orientation)
}

/// The initial style set `text-combine-upright: combine`.
fn combining(combine: TextCombineUpright) -> ComputedStyle<'static> {
    styled(|style| style.orientation.text_combine_upright = combine)
}

/// Returns the text of each span the runs stand one way in, and how, in text
/// order. Adjacent runs that stand alike and are not combined form one span,
/// even across a change of script. Each combined unit is a span of its own.
fn oriented_runs(layout: &Layout) -> Vec<(&str, RunOrientation)> {
    let analysis = analysis(layout);
    let runs: Vec<ScriptRun> = analysis.runs.iter().map(|(_, run)| *run).collect();
    let mut merged: Vec<(Range<ClusterId>, RunOrientation)> = Vec::new();
    for (at, run) in runs.iter().enumerate() {
        let end = runs
            .get(at + 1)
            .map_or(analysis.clusters.end_id(), ScriptRun::start);
        let orientation = run.orientation;
        match merged.last_mut() {
            Some((range, last))
                if *last == orientation && orientation != RunOrientation::Combined =>
            {
                range.end = end;
            }
            _ => merged.push((run.start()..end, orientation)),
        }
    }
    merged
        .into_iter()
        .map(|(range, orientation)| (cluster_text(layout, range), orientation))
        .collect()
}

/// Whether every run stands as `orientation` says.
fn all_stand(layout: &Layout, orientation: RunOrientation) -> bool {
    oriented_runs(layout).iter().all(|&(_, o)| o == orientation)
}

/// `text-orientation: mixed` sets each cluster by its first character's
/// Vertical_Orientation, as Blink's `OrientationIterator` does:
/// - ideographs, kana and the ideographic full stop stand upright (U, Tu);
/// - the corner brackets stand upright too (Tr), in the font's vertical forms;
/// - Latin, digits, the space and the parentheses go on their side (R).
///
/// `upright` stands every cluster up, and `sideways` sets every one on its
/// side. The sideways writing modes do too, whatever `text-orientation` says.
/// A horizontal line sets every cluster across.
#[test]
fn mixed_text_stands_by_each_characters_vertical_orientation() {
    use RunOrientation::{Horizontal, Sideways, Upright};
    let text = "「日本」とAB 12(x)。";
    let layout = build_with(
        &lines_run(WritingMode::VerticalRl, &ComputedStyle::initial()),
        |b| {
            b.text(key(1), text);
        },
    );
    assert_eq!(
        oriented_runs(&layout),
        [
            ("「日本」と", Upright),
            ("AB 12(x)", Sideways),
            ("。", Upright),
        ]
    );
    let upright = oriented(TextOrientation::Upright);
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &upright), |b| {
        b.text(key(1), text)
    });
    assert!(all_stand(&layout, Upright), "{:?}", oriented_runs(&layout));
    let sideways = oriented(TextOrientation::Sideways);
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &sideways), |b| {
        b.text(key(1), text)
    });
    assert!(all_stand(&layout, Sideways));
    for mode in [WritingMode::SidewaysRl, WritingMode::SidewaysLr] {
        let layout = build_with(&lines_run(mode, &upright), |b| b.text(key(1), text));
        assert!(all_stand(&layout, Sideways));
    }
    let layout = build_with(&ComputedBlockStyle::new(&upright), |b| b.text(key(1), text));
    assert!(all_stand(&layout, Horizontal));
    // A span stands its own text as its style says, inside the line.
    let layout = build_with(
        &lines_run(WritingMode::VerticalRl, &ComputedStyle::initial()),
        |b| {
            b.text(key(1), "縦の");
            b.open_box(key(2), &upright, None);
            b.text(key(3), "AB");
            b.close_box();
            b.open_box(key(4), &sideways, None);
            b.text(key(5), "漢");
            b.close_box();
            b.text(key(6), "と");
        },
    );
    assert_eq!(
        oriented_runs(&layout),
        [("縦のAB", Upright), ("漢", Sideways), ("と", Upright)]
    );
}

/// A vertical script's characters keep their own orientation under
/// `upright` and go on their side, as CSS Writing Modes 3, section 5.1, has
/// them. Blink stands them up.
///
/// A grapheme stands as its first character does. A keycap goes on its side,
/// as its digit does. A ZWJ sequence stands upright whole, where Blink reads
/// the joiner apart.
#[test]
fn a_vertical_script_stays_on_its_side_when_text_is_stood_up() {
    use RunOrientation::{Sideways, Upright};
    let upright = oriented(TextOrientation::Upright);
    let mongolian = "\u{1820}\u{1821}";
    let text = alloc::format!("A{mongolian}B");
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &upright), |b| {
        b.text(key(1), &text)
    });
    assert_eq!(
        oriented_runs(&layout),
        [("A", Upright), (mongolian, Sideways), ("B", Upright)]
    );
    let layout = build_with(
        &lines_run(WritingMode::VerticalRl, &ComputedStyle::initial()),
        |b| {
            b.text(key(1), "水1\u{FE0F}\u{20E3}\u{1F468}\u{200D}\u{1F469}");
        },
    );
    assert_eq!(
        oriented_runs(&layout),
        [
            ("水", Upright),
            ("1\u{FE0F}\u{20E3}", Sideways),
            ("\u{1F468}\u{200D}\u{1F469}", Upright),
        ]
    );
}

/// Under `upright`, a character of no script of its own stands as the script
/// run it resolves into, as CSS Writing Modes 4 has a typographic character
/// unit of a vertical script keep its intrinsic orientation. The spaces and
/// the Mongolian comma between Mongolian words lie on their side with them,
/// one run with no upright gap. Blink stands them up.
///
/// Digits, punctuation and a bracket before the first Mongolian word, where
/// the script run is not yet known, go on their side too.
#[test]
fn shared_characters_lie_on_their_side_in_a_vertical_only_script() {
    use RunOrientation::{Sideways, Upright};
    let upright = oriented(TextOrientation::Upright);
    let text = "\u{1820}\u{1821} \u{1822}\u{1802} \u{1823}";
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &upright), |b| {
        b.text(key(1), text)
    });
    assert_eq!(oriented_runs(&layout), [(text, Sideways)]);
    assert_eq!(analysis(&layout).runs.iter().count(), 1);
    let text = "1. (\u{1820}\u{1821}) \u{1822}";
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &upright), |b| {
        b.text(key(1), text)
    });
    assert_eq!(oriented_runs(&layout), [(text, Sideways)]);
    assert_eq!(analysis(&layout).runs.iter().count(), 1);
    // A space between Mongolian and Latin is of the Mongolian run before it.
    let text = "\u{1820} AB C";
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &upright), |b| {
        b.text(key(1), text)
    });
    assert_eq!(
        oriented_runs(&layout),
        [("\u{1820} ", Sideways), ("AB C", Upright)]
    );
}

/// Under `upright`, the spaces between words of a horizontal script stand
/// upright with them, in one run.
#[test]
fn shared_characters_stand_up_in_a_horizontal_script() {
    use RunOrientation::Upright;
    let upright = oriented(TextOrientation::Upright);
    let text = "1. (Ab) cd, ef";
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &upright), |b| {
        b.text(key(1), text)
    });
    assert_eq!(oriented_runs(&layout), [(text, Upright)]);
    assert_eq!(analysis(&layout).runs.iter().count(), 1);
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &upright), |b| {
        b.text(key(1), "  ")
    });
    assert!(all_stand(&layout, Upright));
}

/// `text-combine-upright: all` makes an element's text one unit, set as one
/// upright character. The unit is one run whatever scripts it holds, no line
/// breaks inside it, and none of its white space hangs.
///
/// The element's text items together are one unit, and anything between two
/// of them ends it. Blink likewise wraps a text node and its text siblings in
/// one `LayoutTextCombine`. Around a unit a line breaks where the text itself
/// may, as Chrome's `LineBreaker` decides beside a combined box from its
/// text. Nothing is combined in a horizontal line.
#[test]
fn combined_text_is_one_unit() {
    use RunOrientation::{Combined, Upright};
    let all = combining(TextCombineUpright::All);
    let layout = build_with(
        &lines_run(WritingMode::VerticalRl, &ComputedStyle::initial()),
        |b| {
            b.text(key(1), "令和");
            b.open_box(key(2), &all, None);
            b.text(key(3), "1");
            b.text(key(4), "2");
            b.close_box();
            b.text(key(5), "年");
            b.open_box(key(6), &all, None);
            b.text(key(7), "A あ");
            b.close_box();
            b.open_box(key(8), &all, None);
            b.text(key(9), "B");
            b.close_box();
            b.text(key(10), "月");
        },
    );
    assert_eq!(
        oriented_runs(&layout),
        [
            ("令和", Upright),
            ("12", Combined),
            ("年", Upright),
            ("A あ", Combined),
            ("B", Combined),
            ("月", Upright),
        ]
    );
    // Breaks, as byte offsets: between the ideographs and the units, and
    // none inside one, `12` from 6 to 8 or `A あ` from 11 to 16.
    assert_eq!(breaks(&layout), [3, 6, 8, 11, 16, 17]);
    assert!(hanging(&layout).is_empty(), "{:?}", hanging(&layout));
    let flags = analysis(&layout).flags;
    assert!(flags.contains(ParagraphFlags::HAS_COMBINED));
    assert!(flags.contains(ParagraphFlags::HAS_UPRIGHT));
    let layout = build_with(&ComputedBlockStyle::new(&all), |b| b.text(key(1), "12"));
    assert!(
        !analysis(&layout)
            .flags
            .contains(ParagraphFlags::HAS_COMBINED)
    );
    // Combined text reads left to right, as upright text does.
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &all), |b| {
        b.text(key(1), "\u{5D0}\u{5D1}")
    });
    assert_eq!(levels(&layout), [0]);
}

/// `text-combine-upright: digits n` goes beyond Chrome, which does not parse
/// it. Each maximal run of ASCII digits in the text, `n` or fewer and all in
/// one text item, is a unit. A longer run, or one an item boundary divides,
/// is set as digits are anywhere else. `n` is clamped to two to four.
#[test]
fn digits_combine_only_runs_short_enough() {
    use RunOrientation::{Combined, Sideways, Upright};
    let digits = |n| combining(TextCombineUpright::Digits(n));
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &digits(2)), |b| {
        b.text(key(1), "第7号と12月と2026年");
    });
    assert_eq!(
        oriented_runs(&layout),
        [
            ("第", Upright),
            ("7", Combined),
            ("号と", Upright),
            ("12", Combined),
            ("月と", Upright),
            ("2026", Sideways),
            ("年", Upright),
        ]
    );
    // Nine is four: five digits are too many.
    let layout = build_with(&lines_run(WritingMode::VerticalRl, &digits(9)), |b| {
        b.text(key(1), "A12345")
    });
    assert_eq!(oriented_runs(&layout), [("A12345", Sideways)]);
    let layout = build_with(
        &lines_run(WritingMode::VerticalRl, &ComputedStyle::initial()),
        |b| {
            b.open_box(key(1), &digits(3), None);
            b.text(key(2), "12");
            b.close_box();
            b.open_box(key(3), &digits(3), None);
            b.text(key(4), "3と");
            b.close_box();
        },
    );
    assert_eq!(oriented_runs(&layout), [("123", Sideways), ("と", Upright)]);
}
