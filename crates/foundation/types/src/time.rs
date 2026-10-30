//! Time values and the controllable clock boundary.
//!
//! A moment is UTC and is written as RFC 3339 with exactly three digits of
//! a second: `2026-10-08T17:04:05.123Z`. Code that needs to know what time
//! it is asks a [`Clock`], never the system, so that every rule about time
//! can be tested without waiting.

use chrono::{DateTime, Duration, SecondsFormat, SubsecRound, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::{Arc, Mutex};

/// A moment, in UTC, to the millisecond.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time(DateTime<Utc>);

impl Time {
    /// The moment `at`, cut to the millisecond.
    #[must_use]
    pub fn new(at: DateTime<Utc>) -> Self {
        Self(at.trunc_subsecs(3))
    }

    /// The moment as `chrono` has it.
    #[must_use]
    pub fn get(self) -> DateTime<Utc> {
        self.0
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub fn unix_millis(self) -> i64 {
        self.0.timestamp_millis()
    }
}

impl From<DateTime<Utc>> for Time {
    fn from(at: DateTime<Utc>) -> Self {
        Self::new(at)
    }
}

impl From<Time> for DateTime<Utc> {
    fn from(at: Time) -> Self {
        at.0
    }
}

impl std::fmt::Display for Time {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.to_rfc3339_opts(SecondsFormat::Millis, true))
    }
}

impl std::str::FromStr for Time {
    type Err = chrono::ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        DateTime::parse_from_rfc3339(text).map(|at| Self::new(at.with_timezone(&Utc)))
    }
}

impl Serialize for Time {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Time {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Time".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "format": "date-time" })
    }
}

/// Where the time comes from.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// The present moment.
    fn now(&self) -> DateTime<Utc>;

    /// The present moment, as it is written.
    fn time(&self) -> Time {
        Time::new(self.now())
    }
}

/// The clock of the machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that stands still until it is moved. Clones share one time.
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<Mutex<DateTime<Utc>>>);

impl ManualClock {
    /// A clock that shows `at`.
    #[must_use]
    pub fn new(at: DateTime<Utc>) -> Self {
        Self(Arc::new(Mutex::new(at)))
    }

    /// Moves the clock forward by `by`.
    pub fn advance(&self, by: Duration) {
        let mut at = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *at += by;
    }

    /// Sets the clock to `at`.
    pub fn set(&self, to: DateTime<Utc>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = to;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn written_with_three_digits_of_a_second() {
        let whole = Time::new(Utc.with_ymd_and_hms(2026, 10, 8, 17, 4, 5).unwrap());
        assert_eq!(
            serde_json::to_string(&whole).unwrap(),
            "\"2026-10-08T17:04:05.000Z\""
        );
        let fine: Time = "2026-10-08T17:04:05.123456Z".parse().unwrap();
        assert_eq!(fine.to_string(), "2026-10-08T17:04:05.123Z");
    }

    #[test]
    fn read_from_any_offset_as_utc() {
        let read: Time = serde_json::from_str("\"2026-10-08T19:04:05.5+02:00\"").unwrap();
        assert_eq!(read.to_string(), "2026-10-08T17:04:05.500Z");
    }

    #[test]
    fn a_manual_clock_moves_only_when_moved() {
        let clock = ManualClock::new(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());
        let other = clock.clone();
        assert_eq!(clock.now(), other.now());
        other.advance(Duration::seconds(90));
        assert_eq!(clock.time().to_string(), "2026-01-01T00:01:30.000Z");
    }
}
