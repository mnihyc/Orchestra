//! Where a block stands among its siblings: decimal numbers of any length, and the shortest number that fits between two.
//!
//! A block placed between two takes a value between theirs, so that nothing
//! is renumbered and two people placing blocks at once never meet. A number
//! of fixed width would run out: typing a hundred paragraphs one after the
//! other, each after the last, halves the room before the next block a
//! hundred times. So a position is a decimal written as text, as long as it
//! needs to be, and a new one is the shortest decimal that lies strictly
//! between its neighbours and as near the lower one as that allows: each
//! digit holds nine blocks put one after the other, so a hundred paragraphs
//! take a dozen digits, not a hundred. At the ends the numbers stay whole,
//! one beyond the last and one before the first.

use std::cmp::Ordering;
use std::fmt;

/// A place among siblings, ascending. Ordered as the numbers they are, not as text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Position {
    negative: bool,
    /// The digits before the point, without leading zeros; empty for zero.
    whole: Vec<u8>,
    /// The digits after the point, without trailing zeros.
    fraction: Vec<u8>,
}

impl Position {
    /// The place of the first block of a parent that has none yet.
    #[must_use]
    pub fn first() -> Self {
        Self::whole_number(false, vec![1])
    }

    /// The place of the `n`th of the blocks that are put in one after the other, from 1.
    #[must_use]
    pub fn nth(n: u64) -> Self {
        let digits: Vec<u8> = n
            .to_string()
            .bytes()
            .map(|digit| digit - ZERO_DIGIT)
            .collect();
        Self::whole_number(false, digits)
    }

    /// Reads a decimal as PostgreSQL writes a `numeric` or as this writes one:
    /// an optional minus, digits, and optionally a point and digits. Trailing
    /// zeros are of no account. Anything else is not a position.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (whole, fraction) = match rest.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (rest, ""),
        };
        if whole.is_empty() && fraction.is_empty() {
            return None;
        }
        let digits = |part: &str| -> Option<Vec<u8>> {
            part.bytes()
                .map(|byte| byte.is_ascii_digit().then(|| byte - ZERO_DIGIT))
                .collect()
        };
        Some(Self::canonical(negative, digits(whole)?, digits(fraction)?))
    }

    fn canonical(negative: bool, mut whole: Vec<u8>, mut fraction: Vec<u8>) -> Self {
        let leading = whole.iter().take_while(|digit| **digit == 0).count();
        whole.drain(..leading);
        while fraction.last() == Some(&0) {
            fraction.pop();
        }
        let zero = whole.is_empty() && fraction.is_empty();
        Self {
            negative: negative && !zero,
            whole,
            fraction,
        }
    }

    fn whole_number(negative: bool, whole: Vec<u8>) -> Self {
        Self::canonical(negative, whole, Vec::new())
    }

    fn is_zero(&self) -> bool {
        self.whole.is_empty() && self.fraction.is_empty()
    }

    /// The place after the last of a parent's children: the next whole number
    /// above it.
    #[must_use]
    pub fn after_last(last: &Self) -> Self {
        // floor(last) + 1
        let floor = if last.negative && !last.fraction.is_empty() {
            Self::whole_number(true, increment(&last.whole))
        } else {
            Self::whole_number(last.negative, last.whole.clone())
        };
        floor.plus_one()
    }

    /// The place before the first of a parent's children: the next whole
    /// number below it.
    #[must_use]
    pub fn before_first(first: &Self) -> Self {
        // ceil(first) - 1
        let ceil = if !first.negative && !first.fraction.is_empty() {
            Self::whole_number(false, increment(&first.whole))
        } else {
            Self::whole_number(first.negative, first.whole.clone())
        };
        ceil.minus_one()
    }

    fn plus_one(&self) -> Self {
        if self.negative {
            Self::whole_number(true, decrement(&self.whole))
        } else {
            Self::whole_number(false, increment(&self.whole))
        }
    }

    fn minus_one(&self) -> Self {
        if self.negative || self.is_zero() {
            Self::whole_number(true, increment(&self.whole))
        } else {
            Self::whole_number(false, decrement(&self.whole))
        }
    }

    /// The smallest multiple of `10^-places` that lies strictly above this.
    fn next_above(&self, places: usize) -> Self {
        // The magnitude cut to `places` digits after the point, as a whole
        // number of units of 10^-places.
        let kept = self.fraction.len().min(places);
        let mut units = self.whole.clone();
        units.extend_from_slice(&self.fraction[..kept]);
        units.extend(std::iter::repeat_n(0, places - kept));
        let cut_off = self.fraction.iter().skip(places).any(|digit| *digit != 0);
        let (negative, units) = if !self.negative {
            (false, increment(&units))
        } else if cut_off {
            // Cutting digits off a negative number moves it up already.
            (true, trim(units))
        } else {
            (true, decrement(&units))
        };
        // Back to a number: the last `places` digits are the fraction.
        let mut digits = vec![0; places.saturating_sub(units.len())];
        digits.extend_from_slice(&units);
        let split = digits.len() - places;
        Self::canonical(negative, digits[..split].to_vec(), digits[split..].to_vec())
    }

    /// The place between two: the shortest decimal strictly above `lower` and
    /// strictly below `upper`, and of those the nearest to `lower`. When
    /// `lower` is not below `upper` there is no room, and the place after
    /// `lower` is given: two blocks put at the same place are kept, in the
    /// order of their identifiers.
    #[must_use]
    pub fn between(lower: &Self, upper: &Self) -> Self {
        if lower >= upper {
            return Self::after_last(lower);
        }
        let longest = lower.fraction.len().max(upper.fraction.len());
        // At one place more than either has, the step is smaller than the
        // gap, so a candidate is found at the latest there.
        for places in 0..=longest + 1 {
            let candidate = lower.next_above(places);
            if &candidate < upper {
                return candidate;
            }
        }
        Self::after_last(lower)
    }

    fn magnitude_cmp(&self, other: &Self) -> Ordering {
        self.whole
            .len()
            .cmp(&other.whole.len())
            .then_with(|| self.whole.cmp(&other.whole))
            .then_with(|| self.fraction.cmp(&other.fraction))
    }
}

const ZERO_DIGIT: u8 = b'0';

/// A whole number as digits, one more.
fn increment(digits: &[u8]) -> Vec<u8> {
    let mut out = digits.to_vec();
    for digit in out.iter_mut().rev() {
        if *digit == 9 {
            *digit = 0;
        } else {
            *digit += 1;
            return out;
        }
    }
    out.insert(0, 1);
    out
}

/// A whole number as digits, one less; zero stays zero.
fn decrement(digits: &[u8]) -> Vec<u8> {
    let mut out = digits.to_vec();
    for digit in out.iter_mut().rev() {
        if *digit == 0 {
            *digit = 9;
        } else {
            *digit -= 1;
            return trim(out);
        }
    }
    Vec::new()
}

fn trim(mut digits: Vec<u8>) -> Vec<u8> {
    let leading = digits.iter().take_while(|digit| **digit == 0).count();
    digits.drain(..leading);
    digits
}

impl Ord for Position {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => self.magnitude_cmp(other),
            (true, true) => other.magnitude_cmp(self),
        }
    }
}

impl PartialOrd for Position {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        if self.whole.is_empty() {
            f.write_str("0")?;
        }
        for digit in &self.whole {
            write!(f, "{digit}")?;
        }
        if !self.fraction.is_empty() {
            f.write_str(".")?;
            for digit in &self.fraction {
                write!(f, "{digit}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Position {
        Position::parse(text).unwrap()
    }

    #[test]
    fn a_position_is_written_in_one_way_whatever_the_database_wrote() {
        assert_eq!(p("1.50").to_string(), "1.5");
        assert_eq!(p("007").to_string(), "7");
        assert_eq!(p("0.0").to_string(), "0");
        assert_eq!(p("-0").to_string(), "0");
        assert_eq!(p("-2.250").to_string(), "-2.25");
        assert_eq!(p(".5").to_string(), "0.5");
        assert!(Position::parse("").is_none());
        assert!(Position::parse("1e5").is_none());
        assert!(Position::parse("1.2.3").is_none());
        assert!(Position::parse("--1").is_none());
    }

    #[test]
    fn positions_are_ordered_as_numbers_not_as_text() {
        assert!(p("9") < p("10"));
        assert!(p("1.5") > p("1.25000001"));
        assert!(p("1.25") < p("1.3"));
        assert!(p("-3") < p("-2.5"));
        assert!(p("-0.5") < p("0"));
        assert!(p("-10") < p("-9"));
        assert_eq!(p("2.0").cmp(&p("2")), Ordering::Equal);
    }

    #[test]
    fn the_ends_stay_whole_numbers_beyond_the_last_and_before_the_first() {
        assert_eq!(Position::first().to_string(), "1");
        assert_eq!(Position::after_last(&p("3")).to_string(), "4");
        assert_eq!(Position::after_last(&p("3.75")).to_string(), "4");
        assert_eq!(Position::after_last(&p("-0.5")).to_string(), "0");
        assert_eq!(Position::after_last(&p("-3")).to_string(), "-2");
        assert_eq!(Position::before_first(&p("1")).to_string(), "0");
        assert_eq!(Position::before_first(&p("1.5")).to_string(), "1");
        assert_eq!(Position::before_first(&p("0")).to_string(), "-1");
        assert_eq!(Position::before_first(&p("-2")).to_string(), "-3");
        assert_eq!(Position::before_first(&p("-2.5")).to_string(), "-3");
        assert_eq!(Position::nth(1).to_string(), "1");
        assert_eq!(Position::nth(120).to_string(), "120");
    }

    #[test]
    fn a_place_between_two_is_the_shortest_that_fits_and_nearest_the_lower() {
        assert_eq!(Position::between(&p("1"), &p("2")).to_string(), "1.1");
        assert_eq!(Position::between(&p("1"), &p("3")).to_string(), "2");
        assert_eq!(Position::between(&p("1.9"), &p("2")).to_string(), "1.91");
        assert_eq!(
            Position::between(&p("1"), &p("1.001")).to_string(),
            "1.0001"
        );
        assert_eq!(Position::between(&p("-1"), &p("0")).to_string(), "-0.9");
        assert_eq!(Position::between(&p("-2.5"), &p("-2")).to_string(), "-2.4");
        assert_eq!(Position::between(&p("-0.5"), &p("0.5")).to_string(), "0");
    }

    #[test]
    fn blocks_typed_one_after_the_other_in_the_same_gap_cost_a_digit_for_every_nine() {
        let upper = p("2");
        let mut lower = p("1");
        for _ in 0..500 {
            let next = Position::between(&lower, &upper);
            assert!(next > lower && next < upper);
            lower = next;
        }
        // "1." and a digit for each nine blocks: 500 / 9 is 56.
        assert!(
            lower.to_string().len() <= 2 + 500 / 9 + 1,
            "five hundred blocks in a gap take {} characters",
            lower.to_string().len()
        );
    }

    #[test]
    fn the_same_gap_halved_again_and_again_never_runs_out() {
        let lower = p("1");
        let mut upper = p("2");
        for _ in 0..200 {
            let next = Position::between(&lower, &upper);
            assert!(next > lower && next < upper, "{lower} < {next} < {upper}");
            upper = next;
        }
    }

    #[test]
    fn no_room_gives_the_place_after_and_ties_are_left_to_the_identifiers() {
        assert_eq!(Position::between(&p("2"), &p("2")).to_string(), "3");
        assert_eq!(Position::between(&p("5"), &p("2")).to_string(), "6");
    }
}
