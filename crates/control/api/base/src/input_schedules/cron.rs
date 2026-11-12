//! Explicit UTC cron timing; a Gregorian calendar repeats after 400 years, not an arbitrary search cap.

use super::timing::Occurrence;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use chrono::DateTime;

use chrono::Datelike;

use chrono::Duration;

use chrono::NaiveDate;

use chrono::Utc;

use std::collections::BTreeSet;
const CALENDAR_DAYS: i64 = 146_097;

#[derive(Debug)]
pub struct Cron {
    minutes: Vec<u32>,
    hours: Vec<u32>,
    days: Vec<u32>,
    months: Vec<u32>,
    weekdays: Vec<u32>,
    any_day: bool,
    any_weekday: bool,
}

fn invalid() -> ApiError {
    ApiError::invalid(
        "cron",
        "use five UTC numeric fields with *, lists, ranges or positive /steps",
    )
}

fn field(raw: &str, min: u32, max: u32) -> ApiResult<Vec<u32>> {
    let mut values = BTreeSet::new();
    for part in raw.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (
                range,
                step.parse::<u32>()
                    .ok()
                    .filter(|step| *step > 0)
                    .ok_or_else(invalid)?,
            ),
            None => (part, 1),
        };
        let (start, end) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (
                a.parse::<u32>().map_err(|_| invalid())?,
                b.parse::<u32>().map_err(|_| invalid())?,
            )
        } else {
            let start = range.parse::<u32>().map_err(|_| invalid())?;
            (start, if part.contains('/') { max } else { start })
        };
        if start < min || end > max || start > end {
            return Err(invalid());
        }
        let mut value = start;
        loop {
            values.insert(value);
            let Some(next) = value.checked_add(step).filter(|next| *next <= end) else {
                break;
            };
            value = next;
        }
    }
    if values.is_empty() {
        return Err(invalid());
    }
    Ok(values.into_iter().collect())
}

impl Cron {
    pub(super) fn parse(expression: &str) -> ApiResult<Self> {
        if expression.len() > 128 {
            return Err(invalid());
        }
        let parts: Vec<_> = expression.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = parts.as_slice() else {
            return Err(invalid());
        };
        let weekdays = field(weekday, 0, 7)?
            .into_iter()
            .map(|day| day % 7)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(Self {
            minutes: field(minute, 0, 59)?,
            hours: field(hour, 0, 23)?,
            days: field(day, 1, 31)?,
            months: field(month, 1, 12)?,
            weekdays,
            any_day: *day == "*",
            any_weekday: *weekday == "*",
        })
    }

    fn matches(&self, date: NaiveDate) -> bool {
        if !self.months.contains(&date.month()) {
            return false;
        }
        let dom = self.days.contains(&date.day());
        let dow = self
            .weekdays
            .contains(&date.weekday().num_days_from_sunday());
        match (self.any_day, self.any_weekday) {
            (true, _) => dow,
            (_, true) => dom,
            _ => dom || dow,
        }
    }

    fn found(date: NaiveDate, hour: u32, minute: u32) -> ApiResult<Occurrence> {
        let at = date
            .and_hms_opt(hour, minute, 0)
            .ok_or_else(invalid)?
            .and_utc();
        Ok(Occurrence {
            key: at.to_rfc3339(),
            scheduled_for: Some(at),
            due: at,
            date,
        })
    }

    pub(super) fn next(&self, after: DateTime<Utc>) -> ApiResult<Occurrence> {
        let mut date = after.date_naive();
        // Every valid combination must recur within one complete Gregorian period.
        for _ in 0..=CALENDAR_DAYS {
            if self.matches(date) {
                for hour in &self.hours {
                    for minute in &self.minutes {
                        let found = Self::found(date, *hour, *minute)?;
                        if found.due > after {
                            return Ok(found);
                        }
                    }
                }
            }
            let Some(next) = date.succ_opt() else {
                break;
            };
            date = next;
        }
        Err(ApiError::invalid(
            "cron",
            "the expression has no future calendar occurrence",
        ))
    }

    pub(super) fn latest(&self, first: DateTime<Utc>, now: DateTime<Utc>) -> ApiResult<Occurrence> {
        let mut date = now.date_naive();
        for _ in 0..=CALENDAR_DAYS {
            if self.matches(date) {
                for hour in self.hours.iter().rev() {
                    for minute in self.minutes.iter().rev() {
                        let found = Self::found(date, *hour, *minute)?;
                        if found.due <= now && found.due >= first {
                            return Ok(found);
                        }
                    }
                }
            }
            let Some(previous) = date.pred_opt().filter(|date| *date >= first.date_naive()) else {
                break;
            };
            date = previous;
        }
        Err(ApiError::invalid(
            "cron",
            "the stored occurrence is no longer due",
        ))
    }

    pub(super) fn missed(&self, first: DateTime<Utc>, last: DateTime<Utc>) -> ApiResult<i64> {
        if last <= first {
            return Ok(0);
        }
        let begin = first.date_naive();
        let end = last.date_naive();
        let count_day = |date: NaiveDate| -> ApiResult<i64> {
            if !self.matches(date) {
                return Ok(0);
            }
            let mut count = 0;
            for hour in &self.hours {
                for minute in &self.minutes {
                    let at = Self::found(date, *hour, *minute)?.due;
                    if at >= first && at < last {
                        count += 1;
                    }
                }
            }
            Ok(count)
        };
        if begin == end {
            return count_day(begin);
        }
        let mut total = count_day(begin)?.saturating_add(count_day(end)?);
        let start = begin.succ_opt().ok_or_else(invalid)?;
        let days = (end - start).num_days().max(0);
        let cycles = days / CALENDAR_DAYS;
        let remainder = days % CALENDAR_DAYS;
        let count_dates = |days: i64| -> ApiResult<i64> {
            let mut count = 0_i64;
            for offset in 0..days {
                let date = start
                    .checked_add_signed(Duration::days(offset))
                    .ok_or_else(invalid)?;
                if self.matches(date) {
                    count += 1;
                }
            }
            Ok(count)
        };
        let dates = count_dates(remainder)?.saturating_add(if cycles == 0 {
            0
        } else {
            count_dates(CALENDAR_DAYS)?.saturating_mul(cycles)
        });
        let slots = i64::try_from(self.hours.len() * self.minutes.len()).map_err(|_| invalid())?;
        total = total.saturating_add(dates.saturating_mul(slots));
        Ok(total)
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {
    #![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

    use super::*;
    fn at(value: &str) -> DateTime<Utc> {
        value.parse().unwrap()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn explicit_cron_steps_bounds_and_calendar_rules_have_stable_occurrences() {
        let cron = Cron::parse("*/15 9-10 * * 1-5").unwrap();
        let next = cron.next(at("2026-11-06T10:45:00Z")).unwrap();
        assert_eq!(next.due, at("2026-11-09T09:00:00Z"));
        let latest = cron
            .latest(at("2026-11-06T09:00:00Z"), at("2026-11-06T10:35:00Z"))
            .unwrap();
        assert_eq!(latest.due, at("2026-11-06T10:30:00Z"));
        assert_eq!(
            cron.missed(at("2026-11-06T09:00:00Z"), latest.due).unwrap(),
            6
        );
        assert!(
            Cron::parse("0 0 31 2 *")
                .unwrap()
                .next(at("2026-01-01T00:00:00Z"))
                .is_err()
        );
        assert_eq!(
            Cron::parse("0 0 29 2 *")
                .unwrap()
                .next(at("2026-01-01T00:00:00Z"))
                .unwrap()
                .due,
            at("2028-02-29T00:00:00Z")
        );
        for expression in [
            "* * * *",
            "*/0 * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "* * * * 8",
        ] {
            assert!(Cron::parse(expression).is_err());
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_explicit_cron_steps_bounds_and_calendar_rules_have_stable_occurrences() {
        self::explicit_cron_steps_bounds_and_calendar_rules_have_stable_occurrences()
    }
}
