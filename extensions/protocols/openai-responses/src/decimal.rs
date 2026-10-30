//! Exact decimal text from the numbers that bodies carry.
//!
//! A price or a cost is a decimal figure that a body writes as a JSON number or as a string, perhaps with an
//! exponent (`1.5e-5`). Passing it through a floating-point number would lose digits that a ledger may care
//! about, so the figure is moved as text: digits are shifted and the point is placed, nothing is computed.

/// How far an exponent may reach before a number is no price or cost: far beyond any real figure, and small
/// enough that writing the number out stays small.
const MAX_EXPONENT: i32 = 40;

/// How many places the point moves from a price for one token to a price for a million.
const PER_MILLION_SHIFT: i32 = 6;

/// The number `written` as plain decimal text with no exponent, no sign and no needless zero. `None` for
/// anything that is not a non-negative decimal number; a figure that a body marks as variable with a minus
/// sign is therefore no figure.
#[must_use]
pub fn plain(written: &str) -> Option<String> {
    shifted(written, 0)
}

/// A price for one token as the price for a million of them, in plain decimal text.
#[must_use]
pub fn per_million(written: &str) -> Option<String> {
    shifted(written, PER_MILLION_SHIFT)
}

/// The number `written` times ten to the `shift`, as plain decimal text.
fn shifted(written: &str, shift: i32) -> Option<String> {
    let written = written.trim().trim_matches('"').trim();
    let (mantissa, exponent) = match written.find(['e', 'E']) {
        Some(at) => (&written[..at], written[at + 1..].parse::<i32>().ok()?),
        None => (written, 0),
    };
    if exponent.abs() > MAX_EXPONENT {
        return None;
    }
    let mantissa = mantissa.strip_prefix('+').unwrap_or(mantissa);
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let point = i32::try_from(whole.len()).ok()? + exponent + shift;
    let (integer, decimals) = if point <= 0 {
        (
            "0".to_owned(),
            format!("{}{digits}", "0".repeat(usize::try_from(-point).ok()?)),
        )
    } else {
        let point = usize::try_from(point).ok()?;
        if point >= digits.len() {
            (
                format!("{digits}{}", "0".repeat(point - digits.len())),
                String::new(),
            )
        } else {
            (digits[..point].to_owned(), digits[point..].to_owned())
        }
    };
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    let decimals = decimals.trim_end_matches('0');
    Some(if decimals.is_empty() {
        integer.to_owned()
    } else {
        format!("{integer}.{decimals}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_keeps_every_digit_and_drops_only_needless_zeros() {
        assert_eq!(plain("0.000001").as_deref(), Some("0.000001"));
        assert_eq!(plain("12.50").as_deref(), Some("12.5"));
        assert_eq!(plain("007").as_deref(), Some("7"));
        assert_eq!(plain(".5").as_deref(), Some("0.5"));
        assert_eq!(plain("1.").as_deref(), Some("1"));
        assert_eq!(plain("0").as_deref(), Some("0"));
        assert_eq!(
            plain("0.1234567890123456789").as_deref(),
            Some("0.1234567890123456789")
        );
    }

    #[test]
    fn an_exponent_is_written_out() {
        assert_eq!(plain("1e-6").as_deref(), Some("0.000001"));
        assert_eq!(plain("1.5e-5").as_deref(), Some("0.000015"));
        assert_eq!(plain("2.5E3").as_deref(), Some("2500"));
        assert_eq!(plain("1E2").as_deref(), Some("100"));
    }

    #[test]
    fn a_json_string_holding_a_number_is_the_number() {
        assert_eq!(plain("\"0.25\"").as_deref(), Some("0.25"));
        assert_eq!(plain(" \"3\" ").as_deref(), Some("3"));
    }

    #[test]
    fn what_is_not_a_non_negative_decimal_is_no_figure() {
        for bad in [
            "", "-1", "-0.5", "abc", "1.2.3", "1e", "e5", ".", "0x10", "1 000", "null", "true",
            "1e99", "1e-99", "NaN", "inf",
        ] {
            assert_eq!(plain(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_price_for_a_token_becomes_the_price_for_a_million_exactly() {
        for (per_token, per_million_tokens) in [
            ("0.000001", "1"),
            ("0.0000027", "2.7"),
            ("0.00000005", "0.05"),
            ("0.000000125", "0.125"),
            ("0.0000002", "0.2"),
            ("0", "0"),
            ("1e-6", "1"),
            ("2.5e-6", "2.5"),
            ("0.01", "10000"),
            ("1", "1000000"),
        ] {
            assert_eq!(
                per_million(per_token).as_deref(),
                Some(per_million_tokens),
                "{per_token}"
            );
        }
        assert_eq!(per_million("-1"), None, "a variable price is no price");
    }
}
