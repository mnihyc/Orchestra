//! Recipient-local quiet windows.
//!
//! A person may have quiet hours: a window of the day, in their own time zone,
//! during which nothing leaves the application for them. It is given as the
//! minute of the day it begins and the minute it ends, and it may reach past
//! midnight (from 22:00 to 07:00). What would have gone waits, and goes out as
//! one message when the window ends; the Inbox is not affected.
//!
//! A window whose two ends are the same minute is empty. A time zone that is
//! not known is taken for UTC, so that a person with a name the system cannot
//! read is still kept from being woken at an hour they chose, in the
//! zone the system does know.

use chrono::{DateTime, Duration, LocalResult, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

/// Minutes in a day: the window's ends are minutes of it.
pub const MINUTES_PER_DAY: i32 = 24 * 60;

/// How far past a moment that does not exist in a time zone (the hour lost
/// where clocks go forward) the nearest one that does can be: the shift of
/// any zone in use is an hour or less, and this is more than that.
const LONGEST_GAP_MINUTES: i64 = 120;

/// The zone named, or UTC where the name is not one.
#[must_use]
pub fn zone_of(name: &str) -> Tz {
    name.parse::<Tz>().unwrap_or(Tz::UTC)
}

/// When the window that `now` is in ends, or none if `now` is not in one.
/// `window` is the minutes it begins and ends, in the zone named.
///
/// Where the minute the window ends at does not exist on that day, because
/// the clocks went forward over it, it ends where they came out.
#[must_use]
pub fn held_until(
    timezone: &str,
    window: Option<(i32, i32)>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let (start, end) = window?;
    if !(0..MINUTES_PER_DAY).contains(&start)
        || !(0..MINUTES_PER_DAY).contains(&end)
        || start == end
    {
        return None;
    }
    let zone = zone_of(timezone);
    let local = now.with_timezone(&zone);
    let minute = i32::try_from(local.hour() * 60 + local.minute()).ok()?;
    let wraps = start > end;
    let inside = if wraps {
        minute >= start || minute < end
    } else {
        minute >= start && minute < end
    };
    if !inside {
        return None;
    }
    // The window ends today if it has not yet reached its end today, and
    // tomorrow when it began today and runs past midnight.
    let day = if wraps && minute >= start {
        local.date_naive().succ_opt()?
    } else {
        local.date_naive()
    };
    let ends = day.and_hms_opt(
        u32::try_from(end / 60).ok()?,
        u32::try_from(end % 60).ok()?,
        0,
    )?;
    for forward in 0..=LONGEST_GAP_MINUTES {
        // Where the clock has the moment twice, the later is taken, so that
        // quiet hours are not cut short.
        match zone.from_local_datetime(&(ends + Duration::minutes(forward))) {
            LocalResult::Single(moment) | LocalResult::Ambiguous(_, moment) => {
                return Some(moment.with_timezone(&Utc));
            }
            LocalResult::None => {}
        }
    }
    None
}

/// Whether `now` is in the window.
#[must_use]
pub fn is_quiet(timezone: &str, window: Option<(i32, i32)>, now: DateTime<Utc>) -> bool {
    held_until(timezone, window, now).is_some()
}

/// How many times a pause and a quiet window can hand the hold to each other before it is
/// over: a pause that ends inside a window, the window, and a window that ends inside a later
/// pause cannot follow each other more often than this, since a pause ends once.
const HOLD_HANDOVERS: usize = 4;

/// When everything outward that is held at `now` may go: the first moment at which neither the
/// quiet window nor a pause still ahead holds, or none where neither holds `now`. A pause that has
/// passed holds nothing.
#[must_use]
pub fn outward_held_until(
    timezone: &str,
    window: Option<(i32, i32)>,
    pause_until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let mut at = now;
    for _ in 0..HOLD_HANDOVERS {
        let paused = pause_until.filter(|until| *until > at);
        let quiet = held_until(timezone, window, at);
        match (paused, quiet) {
            (None, None) => break,
            (Some(until), None) | (None, Some(until)) => at = until,
            (Some(pause), Some(quiet)) => at = pause.max(quiet),
        }
    }
    (at > now).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    const NIGHT: Option<(i32, i32)> = Some((22 * 60, 7 * 60));

    #[test]
    fn a_window_that_reaches_past_midnight_holds_until_the_morning_in_the_persons_own_zone() {
        // Berlin is two hours ahead of UTC in October before the 25th.
        let evening = utc(2026, 10, 11, 21, 30);
        assert_eq!(
            held_until("Europe/Berlin", NIGHT, evening),
            Some(utc(2026, 10, 12, 5, 0))
        );
        let small_hours = utc(2026, 10, 12, 3, 0);
        assert_eq!(
            held_until("Europe/Berlin", NIGHT, small_hours),
            Some(utc(2026, 10, 12, 5, 0))
        );
        let day = utc(2026, 10, 11, 12, 0);
        assert_eq!(held_until("Europe/Berlin", NIGHT, day), None);
        assert!(!is_quiet("Europe/Berlin", NIGHT, day));
        // The same moment is in the middle of the afternoon in Los Angeles.
        assert_eq!(held_until("America/Los_Angeles", NIGHT, evening), None);
    }

    #[test]
    fn the_window_begins_at_its_first_minute_and_is_over_at_its_last() {
        assert!(is_quiet("UTC", NIGHT, utc(2026, 10, 11, 22, 0)));
        assert!(is_quiet("UTC", NIGHT, utc(2026, 10, 12, 6, 59)));
        assert!(!is_quiet("UTC", NIGHT, utc(2026, 10, 12, 7, 0)));
        assert!(!is_quiet("UTC", NIGHT, utc(2026, 10, 11, 21, 59)));
        let lunch = Some((12 * 60, 13 * 60 + 30));
        assert_eq!(
            held_until("UTC", lunch, utc(2026, 10, 11, 12, 10)),
            Some(utc(2026, 10, 11, 13, 30))
        );
        assert_eq!(held_until("UTC", lunch, utc(2026, 10, 11, 13, 30)), None);
    }

    #[test]
    fn no_window_an_empty_one_or_an_unreadable_one_holds_nothing() {
        let now = utc(2026, 10, 11, 23, 0);
        assert_eq!(held_until("UTC", None, now), None);
        assert_eq!(
            held_until("UTC", Some((600, 600)), now),
            None,
            "both ends the same minute"
        );
        assert_eq!(held_until("UTC", Some((-1, 60)), now), None);
        assert_eq!(held_until("UTC", Some((60, MINUTES_PER_DAY)), now), None);
    }

    #[test]
    fn a_pause_and_quiet_hours_hold_together_until_neither_does() {
        let evening = utc(2026, 10, 11, 21, 0);
        // Paused until 23:00, inside a night window from 22:00 to 07:00: held until the morning.
        assert_eq!(
            outward_held_until("UTC", NIGHT, Some(utc(2026, 10, 11, 23, 0)), evening),
            Some(utc(2026, 10, 12, 7, 0))
        );
        // A pause that ends after the window holds past it.
        assert_eq!(
            outward_held_until("UTC", NIGHT, Some(utc(2026, 10, 12, 9, 0)), evening),
            Some(utc(2026, 10, 12, 9, 0))
        );
        // A pause that has passed holds nothing, and nor does the day.
        assert_eq!(
            outward_held_until("UTC", NIGHT, Some(utc(2026, 10, 11, 20, 0)), evening),
            None
        );
        assert_eq!(outward_held_until("UTC", None, None, evening), None);
        // Resume clears the pause only: the window still holds at night.
        assert_eq!(
            outward_held_until("UTC", NIGHT, None, utc(2026, 10, 11, 23, 0)),
            Some(utc(2026, 10, 12, 7, 0))
        );
    }

    #[test]
    fn a_zone_the_system_does_not_know_is_taken_for_utc() {
        assert!(is_quiet(
            "Mars/Olympus_Mons",
            NIGHT,
            utc(2026, 10, 11, 23, 0)
        ));
        assert_eq!(zone_of("nonsense"), Tz::UTC);
    }

    #[test]
    fn a_window_that_ends_in_the_hour_clocks_skip_ends_where_they_come_out() {
        // Berlin's clocks went from 02:00 to 03:00 on 29 March 2026: 02:30 never happened.
        let window = Some((23 * 60, 2 * 60 + 30));
        let night = utc(2026, 3, 28, 22, 30);
        assert_eq!(
            held_until("Europe/Berlin", window, night),
            Some(utc(2026, 3, 29, 1, 0))
        );
    }

    #[test]
    fn a_window_that_ends_in_the_hour_clocks_repeat_ends_at_the_later_one() {
        // Berlin's clocks went from 03:00 back to 02:00 on 25 October 2026: 02:30 happened twice.
        let window = Some((22 * 60, 2 * 60 + 30));
        let evening = utc(2026, 10, 24, 20, 30);
        assert_eq!(
            held_until("Europe/Berlin", window, evening),
            Some(utc(2026, 10, 25, 1, 30))
        );
    }
}
