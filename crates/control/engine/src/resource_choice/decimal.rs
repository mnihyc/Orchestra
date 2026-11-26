//! Exact policy weights keep every positive member reachable, including very small fractions.
use std::{cmp::Ordering, fmt};

// Base 10^9 leaves enough room for a limb product and carry in u64. It is representation,
// not a policy precision: extra limbs preserve arbitrarily long authored decimal weights.
const BASE: u64 = 1_000_000_000;
const LIMB_DIGITS: usize = 9;
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Natural(Vec<u32>);
impl Natural {
    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }
    pub fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
    pub fn parse(digits: &str) -> Option<Self> {
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let mut limbs = Vec::new();
        let mut end = digits.len();
        while end > 0 {
            let start = end.saturating_sub(LIMB_DIGITS);
            limbs.push(digits[start..end].parse().ok()?);
            end = start;
        }
        let mut value = Self(limbs);
        value.trim();
        Some(value)
    }
    pub fn digits(&self) -> String {
        let Some(last) = self.0.last() else {
            return "0".into();
        };
        let mut text = last.to_string();
        for limb in self.0.iter().rev().skip(1) {
            use std::fmt::Write as _;
            let _ = write!(text, "{limb:09}");
        }
        text
    }
    fn add(&self, other: &Self) -> Self {
        let mut limbs = Vec::new();
        let mut carry = 0u64;
        for index in 0..self.0.len().max(other.0.len()) {
            let sum = u64::from(self.0.get(index).copied().unwrap_or(0))
                + u64::from(other.0.get(index).copied().unwrap_or(0))
                + carry;
            limbs.push((sum % BASE) as u32);
            carry = sum / BASE;
        }
        if carry != 0 {
            limbs.push(carry as u32);
        }
        Self(limbs)
    }
    /// Its caller compares first, so subtraction cannot manufacture a negative natural.
    fn sub(&self, other: &Self) -> Self {
        let mut limbs = Vec::new();
        let mut borrow = 0i64;
        for (index, one) in self.0.iter().copied().enumerate() {
            let mut value =
                i64::from(one) - i64::from(other.0.get(index).copied().unwrap_or(0)) - borrow;
            borrow = i64::from(value < 0);
            if value < 0 {
                value += BASE as i64;
            }
            limbs.push(value as u32);
        }
        let mut value = Self(limbs);
        value.trim();
        value
    }
    fn shifted(&self, places: usize) -> Self {
        if self.is_zero() {
            return Self::default();
        }
        let whole = places / LIMB_DIGITS;
        let remainder = places % LIMB_DIGITS;
        let factor = 10u64.pow(remainder as u32);
        let mut limbs = vec![0; whole];
        let mut carry = 0u64;
        for limb in &self.0 {
            let value = u64::from(*limb) * factor + carry;
            limbs.push((value % BASE) as u32);
            carry = value / BASE;
        }
        if carry != 0 {
            limbs.push(carry as u32);
        }
        Self(limbs)
    }
}
impl Ord for Natural {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
}
impl PartialOrd for Natural {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct Decimal {
    negative: bool,
    whole: Natural,
    scale: usize,
}
impl Decimal {
    pub fn parse(text: &str) -> Option<Self> {
        let (negative, text) = text
            .strip_prefix('-')
            .map_or((false, text), |value| (true, value));
        let (integer, fraction) = text.split_once('.').unwrap_or((text, ""));
        if integer.is_empty() && fraction.is_empty() {
            return None;
        }
        if !integer
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let fraction = fraction.trim_end_matches('0');
        let whole = Natural::parse(&format!("{integer}{fraction}"))?;
        Some(Self {
            negative: negative && !whole.is_zero(),
            whole,
            scale: fraction.len(),
        })
    }
    pub fn is_zero(&self) -> bool {
        self.whole.is_zero()
    }
    pub fn positive(&self) -> bool {
        !self.negative && !self.is_zero()
    }
    pub fn natural(&self) -> (&Natural, usize) {
        (&self.whole, self.scale)
    }
    pub fn from_natural(whole: Natural, scale: usize) -> Self {
        Self {
            negative: false,
            whole,
            scale,
        }
    }
    fn aligned(&self, scale: usize) -> Natural {
        self.whole.shifted(scale - self.scale)
    }
    pub fn add(&self, other: &Self) -> Self {
        let scale = self.scale.max(other.scale);
        let one = self.aligned(scale);
        let two = other.aligned(scale);
        let (negative, whole) = if self.negative == other.negative {
            (self.negative, one.add(&two))
        } else {
            match one.cmp(&two) {
                Ordering::Less => (other.negative, two.sub(&one)),
                _ => (self.negative, one.sub(&two)),
            }
        };
        let value = Self {
            negative: negative && !whole.is_zero(),
            whole,
            scale,
        };
        // Canonical text keeps stored rotation state independent of an earlier weight's scale.
        Self::parse(&value.to_string()).unwrap_or(value)
    }
    pub fn sub(&self, other: &Self) -> Self {
        let mut negative = other.clone();
        negative.negative = !negative.negative && !negative.is_zero();
        self.add(&negative)
    }
}
impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Decimal {}
impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.negative != other.negative {
            return other.negative.cmp(&self.negative);
        }
        let scale = self.scale.max(other.scale);
        let order = self.aligned(scale).cmp(&other.aligned(scale));
        if self.negative {
            order.reverse()
        } else {
            order
        }
    }
}
impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl fmt::Display for Decimal {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative && !self.is_zero() {
            out.write_str("-")?;
        }
        let digits = self.whole.digits();
        if self.scale == 0 {
            return out.write_str(&digits);
        }
        if digits.len() <= self.scale {
            out.write_str("0.")?;
            for _ in 0..self.scale - digits.len() {
                out.write_str("0")?;
            }
            out.write_str(&digits)
        } else {
            let at = digits.len() - self.scale;
            write!(out, "{}.{}", &digits[..at], &digits[at..])
        }
    }
}

/// Exact rejection sampling on the decimal grid with uniform input digits; at most ten draws
/// are expected because the next power of ten is at most ten times the positive total.
pub(super) fn ticket(total: &Decimal, mut digit: impl FnMut() -> u8) -> Option<Decimal> {
    if !total.positive() {
        return None;
    }
    let (maximum, scale) = total.natural();
    let length = maximum.digits().len();
    loop {
        let mut text = String::with_capacity(length);
        for _ in 0..length {
            let value = digit();
            if value > 9 {
                return None;
            }
            text.push(char::from(b'0' + value));
        }
        let candidate = Natural::parse(&text)?;
        if &candidate < maximum {
            return Some(Decimal::from_natural(candidate, scale));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a deciding case fails by panicking")]
mod tests {
    use super::*;
    #[test]
    fn very_small_positive_member_weights_keep_a_reachable_ticket() {
        let small = format!("0.{}1", "0".repeat(100));
        let one = Decimal::parse("1").unwrap();
        let small = Decimal::parse(&small).unwrap();
        let total = one.add(&small);
        assert!(total > one);
        assert_eq!(total.sub(&one), small);
        let (_, scale) = total.natural();
        let exact = format!("1{}", "0".repeat(scale));
        let mut digits = exact.bytes().map(|digit| digit - b'0');
        assert_eq!(ticket(&total, || digits.next().unwrap_or(0)), Some(one));
    }
    #[test]
    fn fractional_rotation_keeps_exact_credit_between_reservations() {
        let weight = Decimal::parse("0.125").unwrap();
        let total = weight.add(&Decimal::parse("0.25").unwrap());
        assert_eq!(weight.sub(&total).add(&weight).to_string(), "-0.125");
        assert_eq!(Decimal::parse("0001.2500").unwrap().to_string(), "1.25");
        assert_eq!(Decimal::parse("-0.000").unwrap().to_string(), "0");
    }
}
