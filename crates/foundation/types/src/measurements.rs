//! One bounded aggregate measurement, shared by service exporters and the installation's stored minute samples.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A counter's growth or a gauge's current value; a distribution is its mean over this sampling interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MetricSample {
    /// Stable metric name, without object identifiers.
    pub name: String,
    /// The declared bounded category labels.
    pub labels: BTreeMap<String, String>,
    /// Counter growth, current gauge or interval distribution mean.
    pub value: f64,
    /// Counter samples add within a minute; gauges replace a process's reading.
    pub counter: bool,
}

/// Eight buckets per doubling give about 4.5% relative error with geometric-middle estimates.
pub const MODEL_BUCKET_STEPS: i32 = 8;
/// 2^-10 lies below the smallest positive milli-token/second value (0.001),
/// so even a very slow measured stream keeps that relative resolution.
pub const MODEL_BUCKET_FIRST: i32 = -10 * MODEL_BUCKET_STEPS;
/// Last finite edge is 2^20; larger observations stay in the overflow bucket.
pub const MODEL_BUCKET_LAST: i32 = 20 * MODEL_BUCKET_STEPS;

/// Fixed model-performance edges, including zero; scale converts the stored unit for an exporter.
#[must_use]
pub fn model_performance_buckets(scale: f64) -> Vec<f64> {
    std::iter::once(0.0)
        .chain(
            (MODEL_BUCKET_FIRST..=MODEL_BUCKET_LAST)
                .map(|step| 2.0_f64.powf(f64::from(step) / f64::from(MODEL_BUCKET_STEPS)) * scale),
        )
        .collect()
}
