//! Exact amounts counted to a millionth: read from and written as the decimal numbers the ledger and the caps keep, added and compared without a floating-point number.
//!
//! A credit is kept to six decimal places, and tokens and turns are whole
//! numbers, so every amount that is added, compared or shown anywhere in the
//! usage figures fits one integer counting millionths. Nothing here rounds
//! except when a number with more places than six is read, which rounds half
//! up once, as the credits of a row are.

use std::fmt;
use std::ops::{Add, AddAssign, Sub};

/// How many decimal places an amount keeps: the six a credit has.
pub const PLACES: u32 = 6;
/// A whole one, counted in millionths.
const ONE: i128 = 1_000_000;
/// The longest a number may be, in characters: far more than any amount, and
/// short enough that its digits cannot overflow the integer they are held in.
const MAX_CHARS: usize = 30;

/// An amount in millionths: tokens, turns or credits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Micros(i128);

impl Micros {
    /// Nothing.
    pub const ZERO: Self = Self(0);

    /// A whole number of tokens, turns or credits.
    #[must_use]
    pub fn whole(count: i64) -> Self {
        Self(i128::from(count) * ONE)
    }

    /// The amount a plain decimal number such as `12.5` says: digits, then
    /// optionally a point and more digits; no sign, no exponent. More than six
    /// places are rounded half up.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() || text.len() > MAX_CHARS {
            return None;
        }
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        if whole.is_empty() && fraction.is_empty() {
            return None;
        }
        if !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let whole: i128 = if whole.is_empty() {
            0
        } else {
            whole.parse().ok()?
        };
        let places = PLACES as usize;
        let (kept, rest) = if fraction.len() > places {
            fraction.split_at(places)
        } else {
            (fraction, "")
        };
        let mut digits = String::from(kept);
        while digits.len() < places {
            digits.push('0');
        }
        let mut millionths: i128 = digits.parse().ok()?;
        if rest.bytes().next().is_some_and(|digit| digit >= b'5') {
            millionths += 1;
        }
        Some(Self(whole.checked_mul(ONE)?.checked_add(millionths)?))
    }

    /// Like [`Micros::parse`], but a number with more than six places is
    /// refused rather than rounded: what a person types as a limit is exact.
    #[must_use]
    pub fn parse_exact(text: &str) -> Option<Self> {
        let text = text.trim();
        let places = text
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len());
        if places > PLACES as usize {
            return None;
        }
        Self::parse(text)
    }

    /// Whether it is a whole number.
    #[must_use]
    pub const fn is_whole(self) -> bool {
        self.0 % ONE == 0
    }

    /// The whole number it is, where it is one that fits.
    #[must_use]
    pub fn to_whole(self) -> Option<i64> {
        if self.is_whole() {
            i64::try_from(self.0 / ONE).ok()
        } else {
            None
        }
    }

    /// This less `other`, and nothing where `other` is the larger.
    #[must_use]
    pub fn floor_sub(self, other: Self) -> Self {
        if self.0 > other.0 {
            Self(self.0 - other.0)
        } else {
            Self::ZERO
        }
    }

    /// Whether this is at least `percent` of `whole`, with no rounding.
    #[must_use]
    pub fn at_least_percent_of(self, percent: u32, whole: Self) -> bool {
        self.0.saturating_mul(100) >= whole.0.saturating_mul(i128::from(percent))
    }

    /// Its share of `whole` as a ratio, for a figure that is a fraction by
    /// nature. Zero where there is no whole.
    #[must_use]
    pub fn share_of(self, whole: Self) -> f64 {
        if whole.0 == 0 {
            0.0
        } else {
            self.0 as f64 / whole.0 as f64
        }
    }

    /// The number as a decimal without trailing zeros: `12.5`, `3`, `0`.
    #[must_use]
    pub fn text(self) -> String {
        let negative = self.0 < 0;
        let magnitude = self.0.unsigned_abs();
        let one = ONE.unsigned_abs();
        let (whole, fraction) = (magnitude / one, magnitude % one);
        let sign = if negative { "-" } else { "" };
        if fraction == 0 {
            return format!("{sign}{whole}");
        }
        let digits = format!("{fraction:0width$}", width = PLACES as usize);
        format!("{sign}{whole}.{}", digits.trim_end_matches('0'))
    }

    /// The number with exactly six places, as the ledger keeps credits.
    #[must_use]
    pub fn fixed(self) -> String {
        let negative = self.0 < 0;
        let magnitude = self.0.unsigned_abs();
        let one = ONE.unsigned_abs();
        format!(
            "{}{}.{:0width$}",
            if negative { "-" } else { "" },
            magnitude / one,
            magnitude % one,
            width = PLACES as usize
        )
    }
}

impl Add for Micros {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl AddAssign for Micros {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Micros {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

impl std::iter::Sum for Micros {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

impl fmt::Display for Micros {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_amount_is_read_and_written_exactly() {
        for text in ["0", "3", "12.5", "0.000001", "200.123456", "1000000"] {
            assert_eq!(Micros::parse(text).unwrap().text(), text);
        }
        assert_eq!(Micros::parse("2.500000").unwrap().text(), "2.5");
        assert_eq!(Micros::parse("2.5").unwrap().fixed(), "2.500000");
        assert_eq!(Micros::parse(".5").unwrap().text(), "0.5");
    }

    #[test]
    fn a_number_that_is_not_a_plain_decimal_is_not_an_amount() {
        for text in ["", ".", "-1", "1e3", "1,5", "abc", "1.2.3", "+1"] {
            assert_eq!(Micros::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn more_places_than_six_are_rounded_half_up_or_refused_as_a_limit() {
        assert_eq!(Micros::parse("0.0000005").unwrap().text(), "0.000001");
        assert_eq!(Micros::parse("0.0000004").unwrap().text(), "0");
        assert_eq!(Micros::parse_exact("0.0000005"), None);
        assert_eq!(Micros::parse_exact("0.000005").unwrap().text(), "0.000005");
    }

    #[test]
    fn sums_are_exact_where_floating_point_numbers_are_not() {
        let tenth = Micros::parse("0.1").unwrap();
        let total: Micros = (0..10).map(|_| tenth).sum();
        assert_eq!(total, Micros::whole(1));
        assert_eq!(
            (Micros::parse("0.2").unwrap() + Micros::parse("0.1").unwrap()).text(),
            "0.3"
        );
    }

    #[test]
    fn what_is_left_never_goes_below_nothing() {
        let (amount, used) = (Micros::whole(5), Micros::whole(7));
        assert_eq!(amount.floor_sub(used), Micros::ZERO);
        assert_eq!(used.floor_sub(amount), Micros::whole(2));
    }

    #[test]
    fn a_percent_is_reached_without_rounding() {
        let amount = Micros::whole(1_000);
        assert!(Micros::whole(800).at_least_percent_of(80, amount));
        assert!(
            !Micros::parse("799.999999")
                .unwrap()
                .at_least_percent_of(80, amount)
        );
        assert!(Micros::ZERO.at_least_percent_of(80, Micros::ZERO));
    }

    #[test]
    fn a_whole_number_is_told_from_a_fraction() {
        assert_eq!(Micros::parse("4").unwrap().to_whole(), Some(4));
        assert_eq!(Micros::parse("4.5").unwrap().to_whole(), None);
    }
}
