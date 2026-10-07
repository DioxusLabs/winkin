//! The three fixed-point types lengths are kept in, and the roundings that
//! put a float on their grids.
//!
//! Glyph geometry is 16.16 [`TextUnit`]. Layout geometry is 26.6
//! [`LayoutUnit`]. Running advances are 48.16 [`InlineLayoutUnit`]. These
//! are the three types Chrome uses, rounded where Chrome rounds.
//!
//! Values arrive from the caller and from fonts as `f32`, and leave through
//! accessors as `f32`. Every conversion between a float and an integer is
//! here and nowhere else. So each rounds as Chrome's does at that edge, and
//! a change of rounding is one edit. The index casts live in `data`.

use core::fmt;
use core::ops::{Add, AddAssign, Neg, Rem, Sub};

/// `x` rounded down to a whole number: saturating at `i32`'s ends, and zero
/// for NaN, as every `as` from a float is.
///
/// Written out because `f32::floor` is std's: without std a float has no
/// rounding of its own.
#[inline]
pub(crate) fn floor(x: f32) -> i32 {
    let truncated = x as i32;
    if (truncated as f32) > x {
        truncated.saturating_sub(1)
    } else {
        truncated
    }
}

/// `x` rounded up to a whole count, as C's `ceil` rounds a count of lines:
/// nothing for NaN or below zero, and saturating.
#[inline]
pub(crate) fn ceil_count(x: f32) -> u32 {
    u32::try_from(floor(-x).saturating_neg()).unwrap_or(0)
}

/// `x` rounded to the nearest whole number, halves away from zero, as C's
/// `lroundf` rounds.
#[inline]
fn round(x: f32) -> i32 {
    if x < 0.0 {
        floor(-x + 0.5).saturating_neg()
    } else {
        floor(x + 0.5)
    }
}

/// `px` floored to a hundredth of a pixel, as Chrome's
/// `FontDescription::EffectiveFontSize` keys its fonts by size. A hundredth
/// of the largest size is well inside an `i32`; past it the count
/// saturates, and NaN is zero.
#[inline]
pub(crate) fn floor_to_hundredth(px: f32) -> f32 {
    floor(px * 100.0) as f32 / 100.0
}

/// `px` rounded to a whole pixel, halves away from zero, as C's `lroundf`
/// rounds: how Chrome sizes a synthesized small capital and an emphasis
/// mark. Saturates, and NaN is zero.
#[inline]
pub(crate) fn round_to_whole(px: f32) -> f32 {
    round(px) as f32
}

/// How long the vector `(x, y)` is, worked in `f64` and rounded once to the
/// nearest `f32`: what a path is measured with (`path`). Infinite where a
/// part is, and NaN where one is not a number.
///
/// Written out, as [`floor`] is, because `f32::sqrt` is std's: Newton's
/// method from a first guess read off the square's exponent, whose error of
/// a few per cent four steps take below `f64`'s precision.
#[inline]
pub(crate) fn hypot(x: f32, y: f32) -> f32 {
    let (x, y) = (f64::from(x), f64::from(y));
    let square = x * x + y * y;
    if square.is_nan() || square.is_infinite() || square <= 0.0 {
        // NaN, infinite, or nothing at all: the square is its own root.
        return square as f32;
    }
    // Half the exponent, from the bits: the root's first guess.
    let mut root = f64::from_bits((square.to_bits() >> 1) + (1023 << 51));
    for _ in 0..4 {
        root = 0.5 * (root + square / root);
    }
    root as f32
}

/// A whole number worked out in integers, of pixels, of lines or of font
/// units, as a float to measure with: exact below 2^24, which none here
/// reaches, and rounded to the nearest float past it.
#[inline]
pub(crate) fn whole_to_f32(whole: impl Into<i64>) -> f32 {
    whole.into() as f32
}

/// `x` rounded to the nearest whole number, halves away from zero: saturating
/// at `i64`'s ends, and zero for NaN, as every `as` from a float is.
///
/// Written out, as [`round`] is, because `f64::round` is std's.
#[inline]
fn round_wide(x: f64) -> i64 {
    let truncated = x as i64;
    let rest = x - truncated as f64;
    if rest >= 0.5 {
        truncated.saturating_add(1)
    } else if rest <= -0.5 {
        truncated.saturating_sub(1)
    } else {
        truncated
    }
}

/// A length on layout's grid: 26.6 fixed point, in 1/64 px.
///
/// It is named and rounded as Chrome's `LayoutUnit` is. It is not `Fixed`,
/// which in OpenType and read-fonts is 16.16.
///
/// Every box, line and fitting width is one. Values come in from the caller
/// and from fonts as `f32` and are rounded onto the grid once, here.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub(crate) struct LayoutUnit(i32);

impl LayoutUnit {
    /// Nothing.
    pub(crate) const ZERO: Self = Self(0);

    /// The most negative length the grid holds, which Chrome's
    /// `FontHeight::Empty()` fills both sides of an extent with: the
    /// identity of a union.
    pub(crate) const MIN: Self = Self(i32::MIN);

    /// The longest length the grid holds, about 33.5 million pixels, which a
    /// width past it saturates at, as Chrome's does: the tests that hold a
    /// width to it read it.
    #[cfg(test)]
    pub(crate) const MAX: Self = Self(i32::MAX);

    /// The grid's least step, 1/64 px: what Chrome's `AddEpsilon` adds to
    /// the available width a line is fitted in (`AvailableWidthToFit`), so a
    /// line that rounds up past its band by no more than that still fits.
    pub(crate) const EPSILON: Self = Self(1);

    /// `px` on the grid, to the nearest 1/64, halves away from zero, as
    /// `LayoutUnit::FromFloatRound` rounds. Saturates, and NaN is zero.
    #[inline]
    pub(crate) fn from_px(px: f32) -> Self {
        Self(round(px * 64.0))
    }

    /// `px` on the grid, rounded up to the next 1/64, as
    /// `LayoutUnit::FromFloatCeil` rounds: never short of `px`. Saturates,
    /// and NaN is zero.
    #[inline]
    pub(crate) fn from_px_ceil(px: f32) -> Self {
        Self(floor(-(px * 64.0)).saturating_neg())
    }

    /// `px` on the grid, truncated toward zero, as Chrome's plain
    /// `LayoutUnit(float)` converts.
    ///
    /// Chrome converts these lengths this way:
    /// - a computed font size, as the length a numeric `line-height`
    ///   multiplies, and the product as a line height
    ///   (`MinimumValueForLength`);
    /// - `text-indent` and `line-padding`;
    /// - every length a box takes room by: its margin, border and padding,
    ///   and an atomic inline's or a float's size.
    ///
    /// So 10.012px is 10, 10.99px is 10.984375 and −10.012px is −10.
    /// Saturates, and NaN is zero.
    #[inline]
    pub(crate) fn from_px_truncated(px: f32) -> Self {
        Self((px * 64.0) as i32)
    }

    /// Half the length, truncated toward zero on the grid, as Chrome divides
    /// a `LayoutUnit` by two.
    #[inline]
    pub(crate) fn half(self) -> Self {
        Self(self.0 / 2)
    }

    /// The length over `by`, truncated toward zero on the grid, as Chrome
    /// divides a `LayoutUnit` by an integer: how a superscript's shift is a
    /// third of a font size and a subscript's a fifth. Nothing for a divisor
    /// of zero, or for a quotient past the grid's ends.
    #[inline]
    pub(crate) fn divided(self, by: i32) -> Self {
        Self(self.0.checked_div(by).unwrap_or(0))
    }

    /// The length rounded down to a whole pixel, as Chrome's
    /// `LayoutUnit::Floor()`: toward negative infinity, so a negative half
    /// pixel is a whole one.
    #[inline]
    pub(crate) fn floor_px(self) -> Self {
        Self((self.0 >> 6).saturating_mul(64))
    }

    /// The length rounded up to a whole pixel, as Chrome's
    /// `LayoutUnit::Ceil()`: toward positive infinity, the mirror of
    /// [`floor_px`](Self::floor_px).
    #[inline]
    pub(crate) fn ceil_px(self) -> Self {
        -((-self).floor_px())
    }

    /// `px` rounded to a whole pixel, halves up, as Skia rounds a font's
    /// metrics before Chrome sees them (`SkScalarRoundToScalar`, which is
    /// `floor(x + 0.5)`). Saturates, and NaN is zero.
    #[inline]
    pub(crate) fn from_px_whole(px: f32) -> Self {
        Self(floor(px + 0.5).saturating_mul(64))
    }

    /// The length `count` times over, as Chrome multiplies a `LayoutUnit` by
    /// a whole number: exact on the grid, saturating at its ends.
    #[inline]
    pub(crate) fn times_whole(self, count: u32) -> Self {
        let count = i32::try_from(count).unwrap_or(i32::MAX);
        Self(self.0.saturating_mul(count))
    }

    /// The length times `factor`, truncated onto the grid, as Chrome's
    /// `LayoutUnit(length * factor)` makes one of a `LayoutUnit` times a
    /// float. Saturates, and NaN is zero.
    #[inline]
    pub(crate) fn times_truncated(self, factor: f32) -> Self {
        Self::from_px_truncated(self.to_px() * factor)
    }

    /// The length in pixels.
    #[inline]
    pub(crate) fn to_px(self) -> f32 {
        self.0 as f32 / 64.0
    }
}

impl Neg for LayoutUnit {
    type Output = Self;

    /// The length the other way, saturating at the grid's ends.
    #[inline]
    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

impl Add for LayoutUnit {
    type Output = Self;

    /// The sum, saturating at the grid's ends, as Chrome's `LayoutUnit`
    /// adds: a caller's lengths can reach them, and nothing a caller gives
    /// may panic.
    #[inline]
    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl Sub for LayoutUnit {
    type Output = Self;

    /// The difference, saturating at the grid's ends.
    #[inline]
    fn sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

impl fmt::Debug for LayoutUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.to_px())
    }
}

/// A length on glyph geometry's grid: 16.16 fixed point, in 1/65536 px.
///
/// Shaping runs at harfrust scale `size × 65536`, as Chrome sets HarfBuzz up,
/// so a font size is one of these, and so is every glyph advance and offset.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub(crate) struct TextUnit(i32);

impl TextUnit {
    /// `px` on the grid, truncated toward zero, as Chrome turns a font size
    /// into a HarfBuzz scale (`SkiaScalarToHarfBuzzPosition`, a clamped
    /// `static_cast`). Saturates, and NaN is zero.
    #[inline]
    pub(crate) fn from_px_truncated(px: f32) -> Self {
        Self((px * 65536.0) as i32)
    }

    /// `px` on the grid, to the nearest 1/65536, halves away from zero, as
    /// Skia hands a glyph's advance to HarfBuzz (`SkScalarToHarfBuzzPosition`,
    /// a rounding): how a font's advance read off its tables becomes a glyph
    /// advance. Saturates, and NaN is zero.
    #[inline]
    pub(crate) fn from_px(px: f32) -> Self {
        let units = round_wide(f64::from(px) * 65536.0);
        Self(i32::try_from(units).unwrap_or(if units < 0 { i32::MIN } else { i32::MAX }))
    }

    /// The length in pixels.
    #[inline]
    pub(crate) fn to_px(self) -> f32 {
        self.0 as f32 / 65536.0
    }

    /// The length as harfrust takes a scale and gives a position: a count of
    /// 1/65536 px.
    ///
    /// A font size's raw value is the scale that makes the shaper's positions
    /// come out in these units.
    #[inline]
    pub(crate) const fn raw(self) -> i32 {
        self.0
    }

    /// The length the shaper's position `raw` names, in 1/65536 px.
    #[inline]
    pub(crate) const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }
}

impl fmt::Debug for TextUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.to_px())
    }
}

/// A running advance: 48.16 fixed point in an `i64`, in 1/65536 px.
///
/// It is Chrome's `InlineLayoutUnit`, for the advance of a run and a line.
/// Glyph advances are [`TextUnit`]s, so a sum of them in this type is exact.
/// The paragraph's prefix advances are kept in it. So a line's width is one
/// subtraction, with nothing lost to rounding or ordering.
///
/// Its whole part is 48 bits, so no paragraph reaches its range. Sums
/// saturate rather than wrap all the same: a caller's lengths are in them,
/// and nothing a caller gives may panic.
///
/// Fitting reads a position rounded up to layout's 1/64 grid, as Chrome's
/// cached character positions are (`ToCeil<LayoutUnit>`): see
/// [`ceil_to_grid`](Self::ceil_to_grid).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub(crate) struct InlineLayoutUnit(i64);

impl InlineLayoutUnit {
    /// Nothing.
    pub(crate) const ZERO: Self = Self(0);

    /// The length `raw` counts, in 1/65536 px: a sum of the shaper's
    /// positions, which is how a cluster's glyphs' advances arrive.
    #[inline]
    pub(crate) const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    /// The length as a count of 1/65536 px.
    #[inline]
    pub(crate) const fn raw(self) -> i64 {
        self.0
    }

    /// `length`, exactly: 1/64 px is 1024 of these units.
    #[inline]
    pub(crate) fn from_layout(length: LayoutUnit) -> Self {
        Self(i64::from(length.0) << 10)
    }

    /// `length`, exactly.
    #[inline]
    pub(crate) fn from_text(length: TextUnit) -> Self {
        Self(i64::from(length.0))
    }

    /// `px`, to the nearest 1/65536, halves away from zero: a length a caller
    /// gives in pixels, such as a `tab-size` of a length. Saturates, and NaN
    /// is zero.
    #[inline]
    pub(crate) fn from_px(px: f32) -> Self {
        Self(round_wide(f64::from(px) * 65536.0))
    }

    /// The length `factor` times over, to the nearest 1/65536: a `tab-size`
    /// of a number of spaces, say. Saturates, and a factor that is not a
    /// number makes nothing.
    #[inline]
    pub(crate) fn times(self, factor: f32) -> Self {
        Self(round_wide(self.0 as f64 * f64::from(factor)))
    }

    /// Half the length, truncated toward zero.
    #[inline]
    pub(crate) fn half(self) -> Self {
        Self(self.0 / 2)
    }

    /// The length rounded up to layout's 1/64 grid, as Chrome rounds a
    /// running sum to the character positions it fits lines with
    /// (`LayoutUnit::FromFloatCeil`, `ToCeil<LayoutUnit>`). Still in this
    /// type, so a position past the grid's range keeps its value.
    #[inline]
    pub(crate) fn ceil_to_grid(self) -> Self {
        let whole = (self.0 >> 10) + i64::from(self.0 & 1023 != 0);
        Self(whole.saturating_mul(1024))
    }

    /// The length on layout's grid, rounded up to 1/64 and saturating at the
    /// grid's ends: a width that has been fitted.
    #[inline]
    pub(crate) fn to_layout(self) -> LayoutUnit {
        let whole = (self.0 >> 10) + i64::from(self.0 & 1023 != 0);
        LayoutUnit(i32::try_from(whole).unwrap_or(if whole < 0 { i32::MIN } else { i32::MAX }))
    }

    /// The length rounded up onto layout's grid, as
    /// [`to_layout`](Self::to_layout) rounds it, and how much the rounding
    /// added, in 1/65536 px: less than 1/64, so ten bits. Past the grid's
    /// range the rounding saturates, and the length put back with it.
    #[inline]
    pub(crate) fn to_layout_and_short(self) -> (LayoutUnit, u16) {
        // What takes the length up to the next multiple of 1024: its low
        // bits' complement, none where it is on the grid.
        let short = u16::try_from(self.0.wrapping_neg() & 1023).unwrap_or(0);
        (self.to_layout(), short)
    }

    /// `length` less `short` 1/65536 px: a length
    /// [`to_layout_and_short`](Self::to_layout_and_short) split, exactly.
    #[inline]
    pub(crate) fn from_layout_less(length: LayoutUnit, short: u16) -> Self {
        Self(Self::from_layout(length).0 - i64::from(short))
    }

    /// The length in pixels.
    #[inline]
    pub(crate) fn to_px(self) -> f32 {
        (self.0 as f64 / 65536.0) as f32
    }
}

impl Add for InlineLayoutUnit {
    type Output = Self;

    /// The sum, saturating.
    #[inline]
    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl AddAssign for InlineLayoutUnit {
    #[inline]
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for InlineLayoutUnit {
    type Output = Self;

    /// The difference, saturating.
    #[inline]
    fn sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

impl Neg for InlineLayoutUnit {
    type Output = Self;

    /// The length the other way, saturating.
    #[inline]
    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

impl Rem for InlineLayoutUnit {
    type Output = Self;

    /// What is left of the length once every whole `other` is taken out,
    /// truncated toward zero as C's `fmod` is, so its sign is the length's:
    /// how Chrome finds how far past a tab stop a position is. Nothing where
    /// `other` is nothing.
    #[inline]
    fn rem(self, other: Self) -> Self {
        Self(self.0.checked_rem(other.0).unwrap_or(0))
    }
}

impl fmt::Debug for InlineLayoutUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.0 as f64 / 65536.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A float lands on each grid as Chrome's own conversion puts it, and
    /// nothing a caller or a font hands in -- NaN, an infinity -- panics.
    #[test]
    fn lengths_round_onto_their_grids() {
        assert_eq!(LayoutUnit::from_px(1.0).to_px(), 1.0);
        // Halves away from zero on the 1/64 grid, as `lroundf`.
        assert_eq!(LayoutUnit::from_px(0.5 / 64.0), LayoutUnit(1));
        assert_eq!(LayoutUnit::from_px(-0.5 / 64.0), LayoutUnit(-1));
        assert_eq!(LayoutUnit::from_px(1.0 / 256.0), LayoutUnit(0));
        // Whole pixels, halves up, as Skia rounds a font's metrics: Times at
        // 30 px is 34.5 px of line, and Chrome steps 35.
        assert_eq!(LayoutUnit::from_px_whole(34.5), LayoutUnit::from_px(35.0));
        assert_eq!(LayoutUnit::from_px_whole(34.49), LayoutUnit::from_px(34.0));
        assert_eq!(LayoutUnit::from_px_whole(-0.5), LayoutUnit::ZERO);
        assert_eq!(LayoutUnit::from_px_whole(-0.51), LayoutUnit::from_px(-1.0));
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
            let _ = (LayoutUnit::from_px(bad), LayoutUnit::from_px_whole(bad));
            let _ = TextUnit::from_px_truncated(bad);
        }
        assert_eq!(LayoutUnit::from_px(f32::NAN), LayoutUnit::ZERO);
        assert_eq!(LayoutUnit::from_px(f32::INFINITY), LayoutUnit(i32::MAX));
        // A font size truncates, as Chrome's HarfBuzz scale does.
        assert_eq!(TextUnit::from_px_truncated(1.5), TextUnit(98_304));
        assert_eq!(TextUnit::from_px_truncated(-1.5), TextUnit(-98_304));
        assert_eq!(TextUnit::from_px_truncated(1.0 / 131_072.0), TextUnit(0));
        assert_eq!(TextUnit::from_px_truncated(f32::NAN), TextUnit(0));
    }

    /// A running advance is exact at 16.16 and past 26.6's range, and
    /// rounds up onto layout's grid as Chrome's fitting positions do.
    #[test]
    fn running_advances_round_up_onto_the_grid() {
        let unit = InlineLayoutUnit::from_raw(1);
        assert_eq!(unit.ceil_to_grid(), InlineLayoutUnit::from_raw(1024));
        assert_eq!(unit.to_layout(), LayoutUnit(1));
        let on = InlineLayoutUnit::from_layout(LayoutUnit(-3));
        assert_eq!(on.raw(), -3 * 1024);
        assert_eq!(on.ceil_to_grid(), on, "a position on the grid stays");
        // Up is toward positive infinity: -1.5/64 px rounds to -1/64.
        let negative = InlineLayoutUnit::from_raw(-1536);
        assert_eq!(negative.ceil_to_grid().raw(), -1024);
        assert_eq!(negative.to_layout(), LayoutUnit(-1));
        // Past layout's range the sum is still exact; only the width
        // saturates.
        let far = InlineLayoutUnit::from_text(TextUnit(i32::MAX)) + InlineLayoutUnit::from_raw(1);
        assert_eq!(far.raw(), i64::from(i32::MAX) + 1);
        let wide = InlineLayoutUnit::from_raw(i64::from(i32::MAX) << 12);
        assert_eq!(wide.to_layout(), LayoutUnit::MAX);
        assert_eq!((-wide.ceil_to_grid()).to_layout(), LayoutUnit::MIN);
        // Nothing overflows.
        let top = InlineLayoutUnit::from_raw(i64::MAX);
        assert_eq!(top + top, top);
        let _ = (top.ceil_to_grid(), top.to_layout(), top.to_px());
    }

    /// A running advance split into its length on the grid and what rounding
    /// up added, as a fragment item keeps a leaf's, comes back exactly
    /// within the grid's range, and nothing at its ends panics.
    #[test]
    fn a_running_advance_splits_and_comes_back() {
        for raw in [0, 1, 1023, 1024, 1025, -1, -1023, -1024, -1536, 123_456_789] {
            let advance = InlineLayoutUnit::from_raw(raw);
            let (layout, short) = advance.to_layout_and_short();
            assert_eq!(layout, advance.to_layout());
            assert!(short < 1024);
            assert_eq!(InlineLayoutUnit::from_layout_less(layout, short), advance);
        }
        for raw in [i64::MAX, i64::MIN] {
            let (layout, short) = InlineLayoutUnit::from_raw(raw).to_layout_and_short();
            let _ = InlineLayoutUnit::from_layout_less(layout, short);
        }
    }

    /// A pixel length, a font's advance or a caller's `tab-size`, lands to
    /// the nearest 1/65536, halves away from zero; a multiple of a running
    /// advance rounds the same; a remainder is truncated toward zero as
    /// `fmod`'s is. Nothing at the ends of the range, and no NaN, panics.
    #[test]
    fn running_advances_scale_and_divide_as_chromes_floats_do() {
        assert_eq!(TextUnit::from_px(7.5), TextUnit(491_520));
        assert_eq!(TextUnit::from_px(0.5 / 65536.0), TextUnit(1));
        assert_eq!(TextUnit::from_px(-0.5 / 65536.0), TextUnit(-1));
        assert_eq!(TextUnit::from_px(f32::NAN), TextUnit(0));
        assert_eq!(TextUnit::from_px(f32::INFINITY), TextUnit(i32::MAX));
        assert_eq!(TextUnit::from_px(f32::NEG_INFINITY), TextUnit(i32::MIN));
        assert_eq!(InlineLayoutUnit::from_px(60.0).raw(), 60 << 16);
        assert_eq!(InlineLayoutUnit::from_px(f32::MAX).raw(), i64::MAX);
        assert_eq!(InlineLayoutUnit::from_px(f32::NAN).raw(), 0);
        let space = InlineLayoutUnit::from_px(7.5);
        assert_eq!(space.times(8.0), InlineLayoutUnit::from_px(60.0));
        assert_eq!(space.times(1e30).raw(), i64::MAX);
        assert_eq!(space.times(f32::NAN), InlineLayoutUnit::ZERO);
        assert_eq!(space.half(), InlineLayoutUnit::from_px(3.75));
        let interval = InlineLayoutUnit::from_px(60.0);
        assert_eq!(
            InlineLayoutUnit::from_px(135.0) % interval,
            InlineLayoutUnit::from_px(15.0)
        );
        assert_eq!(
            InlineLayoutUnit::from_px(-15.0) % interval,
            InlineLayoutUnit::from_px(-15.0)
        );
        assert_eq!(interval % InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
        let low = InlineLayoutUnit::from_raw(i64::MIN);
        assert_eq!(low % InlineLayoutUnit::from_raw(-1), InlineLayoutUnit::ZERO);
    }

    /// Half-leading on the grid is Chrome's: halved toward zero, then
    /// floored to a whole pixel.
    #[test]
    fn a_length_halves_and_floors_as_chromes_does() {
        assert_eq!(LayoutUnit(-3).half(), LayoutUnit(-1));
        assert_eq!(LayoutUnit(3 * 64).half().floor_px(), LayoutUnit(64));
        assert_eq!(LayoutUnit(-96).floor_px(), LayoutUnit(-128));
        assert_eq!(LayoutUnit::from_px_truncated(1.0 / 128.0), LayoutUnit(0));
        assert_eq!(LayoutUnit::from_px_truncated(-1.99 / 64.0), LayoutUnit(-1));
        assert_eq!(LayoutUnit::MAX + LayoutUnit(1), LayoutUnit::MAX);
        assert_eq!(LayoutUnit::MIN - LayoutUnit(1), LayoutUnit::MIN);
    }

    /// A vector's length is the nearest `f32` to it, across the range: a
    /// whole triangle's exactly, a diagonal's to the last bit, a vector too
    /// long or too short for its square in `f32` measured still; nothing
    /// is nothing, and no NaN or infinity panics.
    #[test]
    fn a_vector_is_measured_to_the_nearest_float() {
        use core::f32::consts::SQRT_2;
        use core::f64::consts::SQRT_2 as SQRT_2_WIDE;
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert_eq!(hypot(-5.0, 12.0), 13.0);
        assert_eq!(hypot(1.0, 1.0), SQRT_2);
        assert_eq!(hypot(0.0, 0.0), 0.0);
        assert_eq!(hypot(0.0, -7.25), 7.25);
        assert_eq!(hypot(3e30, 4e30), 5e30);
        assert_eq!(hypot(3e-30, 4e-30), 5e-30);
        assert_eq!(hypot(f32::MAX, f32::MAX), f32::INFINITY);
        assert_eq!(hypot(f32::INFINITY, 0.0), f32::INFINITY);
        assert!(hypot(f32::NAN, 1.0).is_nan());
        for n in 1..2000u16 {
            let side = f32::from(n) * 0.37;
            let exact = f64::from(side) * SQRT_2_WIDE;
            assert_eq!(hypot(side, side), exact as f32, "{side}");
        }
    }
}
