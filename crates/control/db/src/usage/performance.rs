//! Mergeable request performance histograms and their percentile figures.

use orch_api_contract::usage::UsagePerformance;
use orch_types::Json;
use std::collections::BTreeMap;

/// Percentiles use nearest rank: a single slow request remains visible at the tail.
const MEDIAN_PERCENT: i64 = 50;
/// The reported tail percentile.
const TAIL_PERCENT: i64 = 90;
/// Percentages have this many parts.
const PERCENT: i64 = 100;
/// Speed is exposed in thousandths to preserve useful fractions without floating-point wire drift.
const SPEED_SCALE: f64 = orch_api_contract::performance::SPEED_SCALE as f64;

/// The raw daily sums, merged before a percentile is selected.
#[derive(Debug, Clone, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct Sums {
    /// Requests whose thinking was not streamed.
    pub hidden_requests: i64,
    /// Known streams below the sample eligibility rule.
    pub short_requests: i64,
    /// Completed observations whose outcome is known.
    pub requests: i64,
    /// Failures by code.
    pub errors: Json,
    /// Output with measurable generation time.
    pub speed_tokens: i64,
    /// The matching generation time.
    pub generation_ms_sum: i64,
    /// Logarithmic speed buckets.
    pub speed_buckets: Json,
    /// Logarithmic first-token buckets.
    pub first_token_buckets: Json,
}

fn counts(value: &Json) -> BTreeMap<String, i64> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| v.as_i64().map(|n| (k.clone(), n)))
        .collect()
}

/// The selected bucket's geometric middle removes the upper-edge bias (about 4.5% resolution).
pub fn percentile(histogram: &Json, percent: i64) -> Option<f64> {
    let values = counts(histogram);
    let total: i64 = values.values().sum();
    if total <= 0 {
        return None;
    }
    let rank = (total * percent.clamp(1, PERCENT) + PERCENT - 1) / PERCENT;
    let mut reached = values.get("zero").copied().unwrap_or(0);
    if reached >= rank {
        return Some(0.0);
    }
    // These are exactly the SQL histogram's finite power-of-two edges. The zero
    // bucket preserves an actual zero, and overflow is never clipped to a finite value.
    for exponent in
        orch_types::measurements::MODEL_BUCKET_FIRST..=orch_types::measurements::MODEL_BUCKET_LAST
    {
        reached += values.get(&exponent.to_string()).copied().unwrap_or(0);
        if reached >= rank {
            return Some(2.0_f64.powf(
                (f64::from(exponent) - 0.5)
                    / f64::from(orch_types::measurements::MODEL_BUCKET_STEPS),
            ));
        }
    }
    None
}

impl Sums {
    /// Merges raw counts, so combining medians never means averaging medians.
    pub fn plus(&mut self, other: &Self) {
        self.hidden_requests += other.hidden_requests;
        self.short_requests += other.short_requests;
        self.requests += other.requests;
        self.speed_tokens += other.speed_tokens;
        self.generation_ms_sum += other.generation_ms_sum;
        for (a, b) in [
            (&mut self.errors, &other.errors),
            (&mut self.speed_buckets, &other.speed_buckets),
            (&mut self.first_token_buckets, &other.first_token_buckets),
        ] {
            let mut all = counts(a);
            for (key, n) in counts(b) {
                *all.entry(key).or_default() += n;
            }
            *a = serde_json::json!(all);
        }
    }

    /// Public figures in their documented units.
    pub fn figures(&self) -> UsagePerformance {
        UsagePerformance {
            hidden_requests: self.hidden_requests,
            short_requests: self.short_requests,
            requests: self.requests,
            errors: counts(&self.errors)
                .into_iter()
                .filter(|(code, _)| orch_api_contract::usage::provider_error(code))
                .collect(),
            speed_milli: percentile(&self.speed_buckets, MEDIAN_PERCENT)
                .map(|n| (n * SPEED_SCALE).round() as i64),
            speed_p90_milli: percentile(&self.speed_buckets, TAIL_PERCENT)
                .map(|n| (n * SPEED_SCALE).round() as i64),
            first_token_ms: percentile(&self.first_token_buckets, MEDIAN_PERCENT)
                .map(|n| n.ceil() as i64),
            first_token_p90_ms: percentile(&self.first_token_buckets, TAIL_PERCENT)
                .map(|n| n.ceil() as i64),
            speed_samples: counts(&self.speed_buckets).values().sum(),
            first_token_samples: counts(&self.first_token_buckets).values().sum(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn percentiles_merge_days_at_bucket_resolution_and_keep_zero_apart_from_missing() {
        let mut a = Sums {
            requests: 2,
            speed_buckets: json!({"40":1,"48":1}),
            first_token_buckets: json!({"zero":1}),
            ..Sums::default()
        };
        let b = Sums {
            requests: 2,
            errors: json!({"provider_busy":1}),
            speed_buckets: json!({"56":2}),
            first_token_buckets: json!({"80":1}),
            ..Sums::default()
        };
        a.plus(&b);
        let figures = a.figures();
        assert_eq!(figures.speed_milli, Some(61_287));
        assert_eq!(figures.speed_p90_milli, Some(122_573));
        assert_eq!(figures.first_token_ms, Some(0));
        assert_eq!(figures.first_token_p90_ms, Some(981));
        assert_eq!(figures.first_token_samples, 2);
        assert_eq!(figures.errors["provider_busy"], 1);
        assert_eq!(Sums::default().figures().speed_milli, None);
        assert_eq!(percentile(&json!({"overflow":1}), 90), None);
        let slow = percentile(&json!({"-79":1}), 50).unwrap();
        assert!(
            (0.001..0.0011).contains(&slow),
            "the smallest positive encoded speed keeps the same bucket resolution"
        );
    }
}
