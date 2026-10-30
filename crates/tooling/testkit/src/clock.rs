//! Controlled time for product lifecycle behavior.
//!
//! Tests of anything that depends on time hold a clock that stands still
//! until they move it.

use chrono::{DateTime, TimeZone, Utc};
pub use orch_types::{Clock, ManualClock};

/// The moment a test's clock shows unless the test says otherwise: a
/// Thursday afternoon, far from any boundary of a day, a month or a year.
#[must_use]
pub fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
        .single()
        .unwrap_or_default()
}

/// A clock that shows [`start`].
#[must_use]
pub fn manual() -> ManualClock {
    ManualClock::new(start())
}
