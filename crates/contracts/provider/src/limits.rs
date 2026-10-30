//! The bounds every answer of a source is held to, and how long a learned answer is kept.
//!
//! An answer is at most a MiB, as the built-in readers' replies have always been bounded. Text is at most 200
//! characters and an amount at most 40 (decimal text, so that the vendor's precision is kept). A list member
//! past its cap keeps its first items, and the rest are named in the read's note.

use std::time::Duration;

/// The most bytes of a source's answer: a MiB, far above any report and far below what could hurt a machine.
pub const REPORT_MAX_BYTES: usize = 1024 * 1024;
/// The most characters of a text member (an id, a label, a plan, an identity).
pub const TEXT_MAX_CHARS: usize = 200;
/// The most characters of an amount as decimal text: more digits than any currency has.
pub const DECIMAL_MAX_CHARS: usize = 40;
/// The most characters of a source's error kept beside its choice.
pub const ERROR_MESSAGE_MAX_CHARS: usize = 300;
/// The most usage windows a report keeps.
pub const WINDOWS_MAX: usize = 64;
/// The most balances a report keeps.
pub const BALANCES_MAX: usize = 32;
/// The most rows of spending a report keeps.
pub const SPEND_MAX: usize = 64;
/// The most reset credits a report keeps.
pub const RESET_CREDITS_MAX: usize = 64;
/// The most models a list keeps, and as many as an endpoint's own list may have: ten thousand distinct routing
/// names bound what a list takes in memory and the message of its suggestions.
pub const MODELS_MAX: usize = 10_000;
/// The least time between two reads a source may ask for, in seconds: a vendor is never asked more than once
/// a minute on its behalf.
pub const MIN_INTERVAL_LEAST_SECS: u32 = 60;
/// How long what an address said it is holds before it is asked again: a day.
pub const DESCRIBE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long before presented headers lapse they are asked for again: a minute, more than a model request takes
/// to start.
pub const PRESENTED_RENEW_BEFORE: Duration = Duration::from_secs(60);
/// How long presented headers that state no end are used before they are asked for again: five minutes.
pub const PRESENTED_REUSE_WITHOUT_EXPIRY: Duration = Duration::from_secs(5 * 60);
/// What stands in place of a secret in a source's error.
pub const REDACTED: &str = orch_plugin_contract::secret::REDACTED;

/// Whether `text` is an amount as decimal text: digits, a point and digits after it where it has a fraction,
/// a minus sign before where it is negative, at most [`DECIMAL_MAX_CHARS`] characters (`"12.50"`, `"-3"`).
#[must_use]
pub fn is_decimal(text: &str) -> bool {
    if text.len() > DECIMAL_MAX_CHARS {
        return false;
    }
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = match unsigned.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    digits(whole) && fraction.is_none_or(digits)
}

/// `text` with every one of `secrets` replaced by [`REDACTED`], the longest first so that one secret that
/// holds another is replaced whole, cut to [`ERROR_MESSAGE_MAX_CHARS`] characters.
#[must_use]
pub fn scrub(text: &str, secrets: &[&str]) -> String {
    let mut secrets: Vec<&str> = secrets
        .iter()
        .copied()
        .filter(|secret| !secret.is_empty())
        .collect();
    secrets.sort_unstable_by_key(|secret| std::cmp::Reverse(secret.len()));
    let mut scrubbed = text.to_owned();
    for secret in secrets {
        scrubbed = scrubbed.replace(secret, REDACTED);
    }
    scrubbed.chars().take(ERROR_MESSAGE_MAX_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_are_decimal_text_with_the_vendors_precision() {
        for good in ["0", "12.50", "-3", "47668.4131050000", "0.0000001"] {
            assert!(is_decimal(good), "{good}");
        }
        for bad in ["", ".5", "1.", "+2", "1e3", "NaN", "1.2.3", "１２", "- 3"] {
            assert!(!is_decimal(bad), "{bad}");
        }
        assert!(!is_decimal(&"9".repeat(DECIMAL_MAX_CHARS + 1)));
        assert!(is_decimal(&"9".repeat(DECIMAL_MAX_CHARS)));
    }

    #[test]
    fn a_sources_error_names_no_secret_and_is_kept_short() {
        assert_eq!(
            scrub("token abc123 and abc", &["abc", "abc123", ""]),
            "token [secret] and [secret]"
        );
        assert_eq!(
            scrub(&"x".repeat(1000), &[]).chars().count(),
            ERROR_MESSAGE_MAX_CHARS
        );
    }
}
