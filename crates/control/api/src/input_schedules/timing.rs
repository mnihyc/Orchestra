//! Civil schedule occurrences with one earlier fold instant, explicit gap skips and no paused-date catch-up.

use crate::middleware::requests::{ApiError, ApiResult};
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use orch_api_contract::input_schedules::{InputFrequency, InputTiming};

/// One distinct local occurrence and when the due sweep may report it.
#[derive(Debug, Clone)]
pub struct Occurrence {
    /// The stable instant or civil date/time key.
    pub key: String,
    /// None means the chosen civil time does not exist.
    pub scheduled_for: Option<DateTime<Utc>>,
    /// The earliest existing instant after a gap, or the ordinary occurrence instant.
    pub due: DateTime<Utc>,
    /// Local occurrence day, for the new thread's visible title.
    pub date: NaiveDate,
}

fn zone(timing: &InputTiming) -> ApiResult<Tz> {
    let name = match timing {
        InputTiming::Once { zone, .. } | InputTiming::Repeat { zone, .. } => zone,
    };
    name.parse()
        .map_err(|_| ApiError::invalid("timing.zone", "choose an IANA time zone"))
}

/// Checks an explicit civil time and a nonempty valid weekly selection.
pub fn validate(timing: &InputTiming) -> ApiResult<()> {
    zone(timing)?;
    if let InputTiming::Repeat {
        frequency,
        time,
        weekdays,
        ..
    } = timing
    {
        if time.len() != 5 || NaiveTime::parse_from_str(time, "%H:%M").is_err() {
            return Err(ApiError::invalid("timing.time", "use HH:MM"));
        }
        if (matches!(frequency, InputFrequency::Daily) && !weekdays.is_empty())
            || (matches!(frequency, InputFrequency::Weekly)
                && (weekdays.is_empty() || weekdays.iter().any(|day| !(1..=7).contains(day))))
        {
            return Err(ApiError::invalid(
                "timing.weekdays",
                "daily has no weekdays; weekly selects ISO weekdays 1 through 7",
            ));
        }
    }
    Ok(())
}

fn included(timing: &InputTiming, date: NaiveDate) -> bool {
    match timing {
        InputTiming::Repeat {
            frequency: InputFrequency::Weekly,
            weekdays,
            ..
        } => weekdays.contains(&(date.weekday().number_from_monday() as u8)),
        _ => true,
    }
}

fn on(timing: &InputTiming, date: NaiveDate, tz: Tz) -> ApiResult<Option<Occurrence>> {
    if !included(timing, date) {
        return Ok(None);
    }
    let InputTiming::Repeat { time, zone, .. } = timing else {
        return Ok(None);
    };
    let time = NaiveTime::parse_from_str(time, "%H:%M").map_err(ApiError::internal)?;
    let local = date.and_time(time);
    let instant = match tz.from_local_datetime(&local) {
        LocalResult::Single(at) => Some(at.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, second) => Some(first.min(second).with_timezone(&Utc)),
        LocalResult::None => None,
    };
    let due = if let Some(at) = instant {
        at
    } else {
        // The skipped occurrence is reported at the first existing civil minute after the gap.
        // Advancing civil minutes also covers whole skipped dates without guessing an offset.
        let mut next = local;
        loop {
            next = next
                .checked_add_signed(Duration::minutes(1))
                .ok_or_else(|| {
                    ApiError::invalid(
                        "timing",
                        "the chosen time lies outside the supported calendar",
                    )
                })?;
            match tz.from_local_datetime(&next) {
                LocalResult::Single(at) => break at.with_timezone(&Utc),
                LocalResult::Ambiguous(first, second) => {
                    break first.min(second).with_timezone(&Utc);
                }
                LocalResult::None => {}
            }
        }
    };
    Ok(Some(Occurrence {
        key: format!("{date}T{}@{zone}", time.format("%H:%M")),
        scheduled_for: instant,
        due,
        date,
    }))
}

/// Next strictly future occurrence; valid daily/weekly schedules have one within the next week.
pub fn next(timing: &InputTiming, after: DateTime<Utc>) -> ApiResult<Occurrence> {
    validate(timing)?;
    let tz = zone(timing)?;
    if let InputTiming::Once { at, .. } = timing {
        if at.get() <= after {
            return Err(ApiError::invalid("timing.at", "choose a future time"));
        }
        return Ok(Occurrence {
            key: at.get().to_rfc3339(),
            scheduled_for: Some(at.get()),
            due: at.get(),
            date: at.get().with_timezone(&tz).date_naive(),
        });
    }
    let today = after.with_timezone(&tz).date_naive();
    for days in 0..=7 {
        let date = today
            .checked_add_signed(Duration::days(days))
            .ok_or_else(|| {
                ApiError::invalid(
                    "timing",
                    "the chosen time lies outside the supported calendar",
                )
            })?;
        if let Some(found) = on(timing, date, tz)?
            && found.due > after
        {
            return Ok(found);
        }
    }
    Err(ApiError::invalid(
        "timing",
        "no future occurrence exists in the supported calendar",
    ))
}

/// Coalesces a repeating backlog to its latest due civil occurrence, instead of bursting old inputs.
pub fn latest_due(
    timing: &InputTiming,
    first: DateTime<Utc>,
    now: DateTime<Utc>,
) -> ApiResult<Occurrence> {
    validate(timing)?;
    let tz = zone(timing)?;
    if let InputTiming::Once { at, .. } = timing {
        return Ok(Occurrence {
            key: at.get().to_rfc3339(),
            scheduled_for: Some(at.get()),
            due: at.get(),
            date: at.get().with_timezone(&tz).date_naive(),
        });
    }
    let today = now.with_timezone(&tz).date_naive();
    for days in 0..=7 {
        let date = today
            .checked_sub_signed(Duration::days(days))
            .ok_or_else(|| {
                ApiError::invalid(
                    "timing",
                    "the chosen time lies outside the supported calendar",
                )
            })?;
        if let Some(found) = on(timing, date, tz)?
            && found.due <= now
            && found.due >= first
        {
            return Ok(found);
        }
    }
    Err(ApiError::invalid(
        "timing",
        "the stored occurrence is no longer due",
    ))
}

/// Counts earlier coalesced civil dates with whole-week arithmetic, bounded remainder work.
pub fn missed(timing: &InputTiming, first: DateTime<Utc>, last: &Occurrence) -> ApiResult<i64> {
    let tz = zone(timing)?;
    let InputTiming::Repeat {
        frequency,
        weekdays,
        ..
    } = timing
    else {
        return Ok(0);
    };
    let observed = first.with_timezone(&tz).date_naive();
    let mut start = observed;
    for days in 0..=7 {
        let Some(date) = observed.checked_sub_signed(Duration::days(days)) else {
            break;
        };
        if on(timing, date, tz)?.is_some_and(|found| found.due == first) {
            start = date;
            break;
        }
    }
    let days = (last.date - start).num_days().max(0);
    if matches!(frequency, InputFrequency::Daily) {
        return Ok(days);
    }
    let mut selected = weekdays.clone();
    selected.sort_unstable();
    selected.dedup();
    let mut count = (days / 7) * i64::try_from(selected.len()).unwrap_or_default();
    for offset in 0..days % 7 {
        let date = start + Duration::days(offset);
        if included(timing, date) {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
    use super::*;
    use orch_types::Time;
    fn at(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }
    fn repeat(time: &str) -> InputTiming {
        InputTiming::Repeat {
            frequency: InputFrequency::Daily,
            time: time.to_owned(),
            zone: "America/New_York".to_owned(),
            weekdays: vec![],
        }
    }

    #[test]
    fn a_gap_is_a_visible_skip_and_a_fold_has_only_the_earlier_occurrence() {
        let gap = next(&repeat("02:30"), at("2026-03-08T05:00:00Z")).unwrap();
        assert!(gap.scheduled_for.is_none());
        assert_eq!(gap.key, "2026-03-08T02:30@America/New_York");
        assert_eq!(gap.due, at("2026-03-08T07:00:00Z"));
        let fold = next(&repeat("01:30"), at("2026-11-01T04:00:00Z")).unwrap();
        assert_eq!(fold.scheduled_for, Some(at("2026-11-01T05:30:00Z")));
        assert_eq!(
            next(&repeat("01:30"), at("2026-11-01T06:00:00Z"))
                .unwrap()
                .date,
            NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
        );
    }
    #[test]
    fn a_backlog_coalesces_and_resume_chooses_only_future_work() {
        let first = at("2026-10-01T13:00:00Z");
        let timing = repeat("09:00");
        let latest = latest_due(&timing, first, at("2026-10-10T14:00:00Z")).unwrap();
        assert_eq!(latest.key, "2026-10-10T09:00@America/New_York");
        assert_eq!(missed(&timing, first, &latest).unwrap(), 9);
        assert_eq!(
            next(&timing, at("2026-10-10T14:00:00Z"))
                .unwrap()
                .scheduled_for,
            Some(at("2026-10-11T13:00:00Z"))
        );
        assert!(
            next(
                &InputTiming::Once {
                    at: Time::new(first),
                    zone: "UTC".into()
                },
                first
            )
            .is_err()
        );
    }
}
