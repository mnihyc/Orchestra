//! Spans of time read from the configuration, bounded so that no number written there can overflow a moment.
//!
//! An operator writes days, hours and seconds as plain numbers. A number so
//! large that adding it to a moment would overflow can mean nothing but "for
//! ever", so it is cut to a century: nobody keeps anything, or looks ahead,
//! for longer than that, and the cut keeps the arithmetic on moments, in the
//! program and in the database, within what it can hold.

use chrono::Duration;

/// The longest span, in days, that a number of the configuration stands for.
const LONGEST_DAYS: u32 = 36_500;
/// Hours in a day.
const HOURS_PER_DAY: u32 = 24;
/// Seconds in a day.
const SECONDS_PER_DAY: u32 = 86_400;

fn bounded(count: u64, longest: u32) -> i64 {
    i64::from(u32::try_from(count).map_or(longest, |count| count.min(longest)))
}

/// `count` days, or a century where that is more.
#[must_use]
pub fn days(count: u64) -> Duration {
    Duration::days(bounded(count, LONGEST_DAYS))
}

/// `count` hours, or a century where that is more.
#[must_use]
pub fn hours(count: u64) -> Duration {
    Duration::hours(bounded(count, LONGEST_DAYS * HOURS_PER_DAY))
}

/// `count` seconds, or a century where that is more.
#[must_use]
pub fn seconds(count: u64) -> Duration {
    Duration::seconds(bounded(count, LONGEST_DAYS * SECONDS_PER_DAY))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn a_number_that_fits_is_the_span_it_names() {
        assert_eq!(days(14), Duration::days(14));
        assert_eq!(hours(24), Duration::hours(24));
        assert_eq!(seconds(10_800), Duration::seconds(10_800));
        assert_eq!(days(0), Duration::zero());
    }

    #[test]
    fn a_number_that_is_too_large_is_a_century_and_a_moment_before_it_is_still_a_moment() {
        let now = Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0).unwrap();
        for span in [
            days(u64::MAX),
            hours(u64::MAX),
            seconds(u64::MAX),
            days(u64::from(u32::MAX) + 1),
        ] {
            assert_eq!(span, Duration::days(36_500));
            assert!(now - span < now && now + span > now);
        }
    }
}
