//! A shared projection of measured plan usage or a cap's counted period use to its reset.

use chrono::{DateTime, Duration, Utc};
use orch_types::Time;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Short windows follow the current hour; longer windows include a whole daily rhythm.
pub const FORECAST_SHORT_LOOKBACK: Duration = Duration::hours(1);
/// A full day avoids counting part of the day-and-night cycle twice.
pub const FORECAST_LONG_LOOKBACK: Duration = Duration::days(1);
/// Two missed hourly idle readings make an observation too old to project.
pub const FORECAST_MAX_AGE: Duration = Duration::hours(2);
/// A quarter of the chosen look-back gives the pace time to be observed.
const MINIMUM_COVERAGE_DIVISOR: i32 = 4;
/// Percentages use a whole limit as one hundred.
const FULL_PERCENT: f64 = 100.0;

/// One observed percentage of a plan window or cap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ForecastPoint {
    /// When the observation was made.
    pub at: Time,
    /// The percentage used, before any display clipping.
    pub used_percent: f64,
}

/// The observed stretch whose pace was projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ForecastBasis {
    /// The last hour.
    Hour,
    /// The last day.
    Day,
    /// The current window began inside the usual look-back.
    Window,
    /// The cap's average since this period began.
    Period,
}

/// A projection at the observed pace, with no promise that future use will follow it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageForecast {
    /// Projected use at the reset, which may exceed one hundred percent.
    pub percent_at_end: f64,
    /// When the limit is reached, only when that precedes the reset.
    pub runs_out_at: Option<Time>,
    /// The stretch that supplies the pace.
    pub basis: ForecastBasis,
}

/// SQL can summarize a bounded look-back without returning its stored observations to the application.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingPace {
    /// The earliest retained observation inside the look-back.
    pub first_at: Time,
    /// Its percentage, used when the window's known zero origin precedes it.
    pub first_percent: f64,
    /// The latest actual observation.
    pub last_at: Time,
    /// Its percentage.
    pub last_percent: f64,
    /// The sum of positive rises between observations; falls contribute nothing.
    pub rise: f64,
}

/// How the caller can supply its observations without a second counting mechanism.
#[derive(Debug, Clone, Copy)]
pub enum ForecastSource<'a> {
    /// Actual readings; a fall contributes no use.
    Readings(&'a [ForecastPoint]),
    /// The same measured evidence aggregated by the store, with no whole-history load.
    Aggregate(ReadingPace),
    /// A cap already has its exact period count; use that period's average.
    PeriodAverage(f64),
}

/// Projects a window's usage from current observations, leaving short or stale evidence unprojected.
#[must_use]
pub fn forecast(
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    window_length: Duration,
    now: DateTime<Utc>,
    source: ForecastSource<'_>,
) -> Option<UsageForecast> {
    if starts_at >= ends_at || now < starts_at || now >= ends_at {
        return None;
    }
    let lookback = if window_length < FORECAST_LONG_LOOKBACK {
        FORECAST_SHORT_LOOKBACK
    } else {
        FORECAST_LONG_LOOKBACK
    };
    let (first_at, last_at, last_value, rise, basis) = match source {
        ForecastSource::Aggregate(pace) => {
            let first = pace.first_at.get();
            let last = pace.last_at.get();
            let cutoff = starts_at.max(last - lookback);
            if first < cutoff
                || first > last
                || last > now
                || now - last > FORECAST_MAX_AGE
                || [pace.first_percent, pace.last_percent, pace.rise]
                    .iter()
                    .any(|value| !value.is_finite() || *value < 0.0)
            {
                return None;
            }
            let origin = cutoff == starts_at && first > starts_at;
            let basis = if cutoff == starts_at {
                ForecastBasis::Window
            } else if lookback == FORECAST_SHORT_LOOKBACK {
                ForecastBasis::Hour
            } else {
                ForecastBasis::Day
            };
            (
                if origin { starts_at } else { first },
                last,
                pace.last_percent,
                pace.rise + if origin { pace.first_percent } else { 0.0 },
                basis,
            )
        }
        ForecastSource::PeriodAverage(percent) => {
            if !percent.is_finite() || percent < 0.0 {
                return None;
            }
            (starts_at, now, percent, percent, ForecastBasis::Period)
        }
        ForecastSource::Readings(readings) => {
            let mut points: Vec<_> = readings
                .iter()
                .filter(|point| {
                    point.at.get() >= starts_at
                        && point.at.get() <= now
                        && point.used_percent.is_finite()
                        && point.used_percent >= 0.0
                })
                .map(|point| (point.at.get(), point.used_percent))
                .collect();
            points.sort_by_key(|point| point.0);
            let last_at = points.last()?.0;
            if now - last_at > FORECAST_MAX_AGE {
                return None;
            }
            // The last observed hour/day supplies the pace, even between the hourly idle reads.
            let cutoff = starts_at.max(last_at - lookback);
            points.retain(|point| point.0 >= cutoff);
            // A window starts empty; that known origin is useful only inside this look-back.
            if cutoff == starts_at && points.first().is_none_or(|point| point.0 > starts_at) {
                points.insert(0, (starts_at, 0.0));
            }
            let first = points.first()?;
            let last = points.last()?;
            let rise = points
                .windows(2)
                .map(|pair| (pair[1].1 - pair[0].1).max(0.0))
                .sum();
            let basis = if cutoff == starts_at {
                ForecastBasis::Window
            } else if lookback == FORECAST_SHORT_LOOKBACK {
                ForecastBasis::Hour
            } else {
                ForecastBasis::Day
            };
            (first.0, last.0, last.1, rise, basis)
        }
    };
    let covered = last_at - first_at;
    if covered < lookback / MINIMUM_COVERAGE_DIVISOR || covered <= Duration::zero() {
        return None;
    }
    let pace = rise / covered.num_milliseconds() as f64;
    let left = (ends_at - last_at).num_milliseconds() as f64;
    let percent_at_end = last_value + pace * left;
    if !percent_at_end.is_finite() {
        return None;
    }
    let runs_out_at = if last_value >= FULL_PERCENT {
        Some(last_at)
    } else if pace > 0.0 && percent_at_end > FULL_PERCENT {
        let millis = ((FULL_PERCENT - last_value) / pace).ceil();
        last_at
            .checked_add_signed(Duration::milliseconds(millis as i64))
            .filter(|at| *at < ends_at)
    } else {
        None
    };
    Some(UsageForecast {
        percent_at_end,
        runs_out_at: runs_out_at.map(Time::new),
        basis,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hours: i64) -> DateTime<Utc> {
        DateTime::UNIX_EPOCH + Duration::hours(hours)
    }
    fn point(at: DateTime<Utc>, used_percent: f64) -> ForecastPoint {
        ForecastPoint {
            at: Time::new(at),
            used_percent,
        }
    }

    #[test]
    fn a_short_window_uses_the_last_hour_and_projects_to_the_reset() {
        let points = [point(at(1), 5.0), point(at(2), 20.0), point(at(3), 40.0)];
        let result = forecast(
            at(0),
            at(5),
            Duration::hours(5),
            at(3),
            ForecastSource::Readings(&points),
        )
        .unwrap();
        assert_eq!(result.basis, ForecastBasis::Hour);
        assert_eq!(result.percent_at_end, 80.0);
        assert_eq!(result.runs_out_at, None);
    }

    #[test]
    fn a_long_window_uses_the_last_day() {
        let points = [point(at(1), 1.0), point(at(24), 10.0), point(at(48), 20.0)];
        let result = forecast(
            at(0),
            at(168),
            Duration::hours(168),
            at(48),
            ForecastSource::Readings(&points),
        )
        .unwrap();
        assert_eq!(result.basis, ForecastBasis::Day);
        assert_eq!(result.percent_at_end, 70.0);
    }

    #[test]
    fn a_window_starts_at_zero_and_never_borrows_the_previous_windows_readings() {
        let points = [
            point(at(-1), 99.0),
            point(at(0) + Duration::minutes(30), 10.0),
        ];
        let result = forecast(
            at(0),
            at(5),
            Duration::hours(5),
            at(0) + Duration::minutes(30),
            ForecastSource::Readings(&points),
        )
        .unwrap();
        assert_eq!(result.basis, ForecastBasis::Window);
        assert_eq!(result.percent_at_end, 100.0);
        assert_eq!(result.runs_out_at, None);
    }

    #[test]
    fn a_falling_reading_counts_as_no_use() {
        let points = [
            point(at(2), 30.0),
            point(at(2) + Duration::minutes(30), 20.0),
            point(at(3), 25.0),
        ];
        let result = forecast(
            at(0),
            at(5),
            Duration::hours(5),
            at(3),
            ForecastSource::Readings(&points),
        )
        .unwrap();
        assert_eq!(result.percent_at_end, 35.0);
    }

    #[test]
    fn short_windows_need_fifteen_minutes_of_readings() {
        for minutes in [0, 14, 15] {
            let now = at(0) + Duration::minutes(minutes);
            let points = [point(now, 1.0)];
            assert_eq!(
                forecast(
                    at(0),
                    at(5),
                    Duration::hours(5),
                    now,
                    ForecastSource::Readings(&points)
                )
                .is_some(),
                minutes >= 15
            );
        }
    }

    #[test]
    fn long_windows_need_six_hours_of_readings() {
        for hours in [5, 6] {
            let points = [point(at(hours), 10.0)];
            assert_eq!(
                forecast(
                    at(0),
                    at(168),
                    Duration::hours(168),
                    at(hours),
                    ForecastSource::Readings(&points)
                )
                .is_some(),
                hours >= 6
            );
        }
    }

    #[test]
    fn readings_more_than_two_hours_old_have_no_forecast() {
        let points = [point(at(0), 0.0), point(at(6), 10.0)];
        assert!(
            forecast(
                at(0),
                at(168),
                Duration::hours(168),
                at(8),
                ForecastSource::Readings(&points)
            )
            .is_some()
        );
        assert!(
            forecast(
                at(0),
                at(168),
                Duration::hours(168),
                at(8) + Duration::seconds(1),
                ForecastSource::Readings(&points)
            )
            .is_none()
        );
    }

    #[test]
    fn the_run_out_time_is_present_only_before_the_reset() {
        let points = [point(at(2), 30.0), point(at(3), 60.0)];
        let result = forecast(
            at(0),
            at(5),
            Duration::hours(5),
            at(3),
            ForecastSource::Readings(&points),
        )
        .unwrap();
        assert_eq!(
            result.runs_out_at,
            Some(Time::new(at(3) + Duration::minutes(80)))
        );
        assert_eq!(result.percent_at_end, 120.0);
    }

    #[test]
    fn a_caps_period_average_uses_the_same_projection_and_minimum_evidence() {
        assert!(
            forecast(
                at(0),
                at(24),
                Duration::hours(24),
                at(5),
                ForecastSource::PeriodAverage(20.0)
            )
            .is_none()
        );
        let result = forecast(
            at(0),
            at(24),
            Duration::hours(24),
            at(6),
            ForecastSource::PeriodAverage(20.0),
        )
        .unwrap();
        assert_eq!(result.basis, ForecastBasis::Period);
        assert_eq!(result.percent_at_end, 80.0);
    }

    #[test]
    fn invalid_percentages_and_ended_windows_are_not_projected() {
        for percent in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(
                forecast(
                    at(0),
                    at(5),
                    Duration::hours(5),
                    at(1),
                    ForecastSource::PeriodAverage(percent)
                )
                .is_none()
            );
        }
        assert!(
            forecast(
                at(0),
                at(5),
                Duration::hours(5),
                at(5),
                ForecastSource::PeriodAverage(50.0)
            )
            .is_none()
        );
    }

    #[test]
    fn an_early_reset_does_not_turn_a_weekly_windows_day_basis_into_an_hour() {
        let points = [point(at(160), 0.0), point(at(161), 10.0)];
        assert!(
            forecast(
                at(160),
                at(168),
                Duration::hours(168),
                at(161),
                ForecastSource::Readings(&points)
            )
            .is_none()
        );
    }

    #[test]
    fn an_hourly_idle_read_can_supply_its_last_observed_hour_until_it_is_stale() {
        let points = [point(at(1), 10.0), point(at(2), 20.0)];
        let now = at(3) + Duration::minutes(30);
        let result = forecast(
            at(0),
            at(5),
            Duration::hours(5),
            now,
            ForecastSource::Readings(&points),
        )
        .unwrap();
        assert_eq!(result.basis, ForecastBasis::Hour);
        assert_eq!(result.percent_at_end, 50.0);
    }
    #[test]
    fn aggregated_pace_keeps_the_same_origin_falls_and_projection_as_readings() {
        for (start, end, length, now, points) in [
            (
                at(0),
                at(5),
                Duration::hours(5),
                at(3),
                vec![
                    point(at(2), 20.0),
                    point(at(2) + Duration::minutes(30), 10.0),
                    point(at(3), 30.0),
                ],
            ),
            (
                at(2),
                at(7),
                Duration::hours(5),
                at(3),
                vec![
                    point(at(2) + Duration::minutes(15), 20.0),
                    point(at(3), 60.0),
                ],
            ),
            (
                at(0),
                at(168),
                Duration::hours(168),
                at(48),
                vec![point(at(24), 10.0), point(at(36), 0.0), point(at(48), 20.0)],
            ),
        ] {
            let first = points.first().unwrap();
            let last = points.last().unwrap();
            let pace = ReadingPace {
                first_at: first.at,
                first_percent: first.used_percent,
                last_at: last.at,
                last_percent: last.used_percent,
                rise: points
                    .windows(2)
                    .map(|pair| (pair[1].used_percent - pair[0].used_percent).max(0.0))
                    .sum(),
            };
            assert_eq!(
                forecast(start, end, length, now, ForecastSource::Aggregate(pace)),
                forecast(start, end, length, now, ForecastSource::Readings(&points))
            );
        }
    }
}
