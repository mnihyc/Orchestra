//! Bounded interval aggregates beside the Prometheus recorder, without scraping or parsing the exporter's text.

use metrics::{
    Counter, CounterFn, Gauge, GaugeFn, Histogram, HistogramFn, Key, KeyName, Metadata, Recorder,
    SharedString, Unit,
};
use orch_types::measurements::MetricSample;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Name {
    name: String,
    labels: BTreeMap<String, String>,
}

impl From<&Key> for Name {
    fn from(key: &Key) -> Self {
        Self {
            name: key.name().to_owned(),
            labels: key
                .labels()
                .map(|label| (label.key().to_owned(), label.value().to_owned()))
                .collect(),
        }
    }
}

enum Value {
    Counter {
        whole: u64,
        grew: u64,
        initialized: bool,
    },
    Gauge(f64),
    Absolute {
        whole: f64,
        grew: f64,
        initialized: bool,
    },
    Distribution {
        sum: f64,
        count: u64,
    },
}
struct Cell(Mutex<Value>);

/// A single process's interval aggregates; no raw histogram observation is retained here.
#[derive(Clone, Default)]
pub(super) struct Samples(Arc<Mutex<BTreeMap<Name, Arc<Cell>>>>);

impl Samples {
    fn cell(&self, key: &Key, initial: Value) -> Arc<Cell> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key.into())
            .or_insert_with(|| Arc::new(Cell(Mutex::new(initial))))
            .clone()
    }
    pub(super) fn take(&self) -> Vec<MetricSample> {
        let mut samples = Vec::new();
        for (name, cell) in self.0.lock().unwrap_or_else(PoisonError::into_inner).iter() {
            let mut held = cell.0.lock().unwrap_or_else(PoisonError::into_inner);
            let (value, counter) = match &mut *held {
                Value::Counter { grew, .. } => (std::mem::take(grew) as f64, true),
                Value::Gauge(value) => (*value, false),
                Value::Absolute { grew, .. } => (std::mem::take(grew), true),
                Value::Distribution { sum, count } => {
                    if *count == 0 {
                        continue;
                    }
                    let value = *sum / (*count as f64);
                    *sum = 0.0;
                    *count = 0;
                    (value, false)
                }
            };
            if value.is_finite() {
                samples.push(MetricSample {
                    name: name.name.clone(),
                    labels: name.labels.clone(),
                    value,
                    counter,
                });
            }
        }
        samples
    }
}

struct Count {
    inner: Counter,
    cell: Arc<Cell>,
}
impl CounterFn for Count {
    fn increment(&self, value: u64) {
        self.inner.increment(value);
        if let Value::Counter {
            whole,
            grew,
            initialized,
        } = &mut *self.cell.0.lock().unwrap_or_else(PoisonError::into_inner)
        {
            *initialized = true;
            *whole = whole.saturating_add(value);
            *grew = grew.saturating_add(value);
        }
    }
    fn absolute(&self, value: u64) {
        self.inner.absolute(value);
        if let Value::Counter {
            whole,
            grew,
            initialized,
        } = &mut *self.cell.0.lock().unwrap_or_else(PoisonError::into_inner)
        {
            // A durable total's first observation is a baseline, not all past work in this minute.
            if *initialized {
                *grew = grew.saturating_add(value.saturating_sub(*whole));
            }
            *whole = (*whole).max(value);
            *initialized = true;
        }
    }
}

struct Level {
    inner: Gauge,
    cell: Arc<Cell>,
}
impl GaugeFn for Level {
    fn increment(&self, value: f64) {
        self.inner.increment(value);
        if let Value::Gauge(at) = &mut *self.cell.0.lock().unwrap_or_else(PoisonError::into_inner) {
            *at += value;
        }
    }
    fn decrement(&self, value: f64) {
        self.inner.decrement(value);
        if let Value::Gauge(at) = &mut *self.cell.0.lock().unwrap_or_else(PoisonError::into_inner) {
            *at -= value;
        }
    }
    fn set(&self, value: f64) {
        if !value.is_finite() {
            return;
        }
        let mut cell = self.cell.0.lock().unwrap_or_else(PoisonError::into_inner);
        match &mut *cell {
            Value::Gauge(at) => {
                *at = value;
                self.inner.set(value);
            }
            Value::Absolute {
                whole,
                grew,
                initialized,
            } => {
                if *initialized {
                    *grew += (value - *whole).max(0.0);
                }
                *whole = whole.max(value);
                *initialized = true;
                self.inner.set(*whole);
            }
            _ => {}
        }
    }
}

struct Distribution {
    inner: Histogram,
    cell: Arc<Cell>,
}
impl HistogramFn for Distribution {
    fn record(&self, value: f64) {
        self.inner.record(value);
        if value.is_finite()
            && let Value::Distribution { sum, count } =
                &mut *self.cell.0.lock().unwrap_or_else(PoisonError::into_inner)
        {
            *sum += value;
            *count = count.saturating_add(1);
        }
    }
}

pub(super) struct Mirrored<R> {
    pub(super) inner: R,
    pub(super) samples: Samples,
}

impl<R: Recorder> Recorder for Mirrored<R> {
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.inner.describe_counter(key, unit, description);
    }
    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.inner.describe_gauge(key, unit, description);
    }
    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, description: SharedString) {
        self.inner.describe_histogram(key, unit, description);
    }
    fn register_counter(&self, key: &Key, metadata: &Metadata<'_>) -> Counter {
        Counter::from_arc(Arc::new(Count {
            inner: self.inner.register_counter(key, metadata),
            cell: self.samples.cell(
                key,
                Value::Counter {
                    whole: 0,
                    grew: 0,
                    initialized: false,
                },
            ),
        }))
    }
    fn register_gauge(&self, key: &Key, metadata: &Metadata<'_>) -> Gauge {
        // The metrics facade's Counter accepts u64 only. Credits have fractional units; retain
        // their monotone ledger total in a floating cell, exposed as a counter by Readings.
        let initial = if key.name() == "orch_usage_credits_total" {
            Value::Absolute {
                whole: 0.0,
                grew: 0.0,
                initialized: false,
            }
        } else {
            Value::Gauge(0.0)
        };
        Gauge::from_arc(Arc::new(Level {
            inner: self.inner.register_gauge(key, metadata),
            cell: self.samples.cell(key, initial),
        }))
    }
    fn register_histogram(&self, key: &Key, metadata: &Metadata<'_>) -> Histogram {
        Histogram::from_arc(Arc::new(Distribution {
            inner: self.inner.register_histogram(key, metadata),
            cell: self
                .samples
                .cell(key, Value::Distribution { sum: 0.0, count: 0 }),
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    #[test]
    fn historical_totals_are_baselines_and_fractional_credit_growth_is_not_rounded() {
        let inner = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let export = inner.handle();
        let samples = super::Samples::default();
        let recorder = super::Mirrored {
            inner,
            samples: samples.clone(),
        };
        metrics::with_local_recorder(&recorder, || {
            metrics::counter!("orch_usage_tokens_total","purpose"=>"turn").absolute(1000);
            metrics::gauge!("orch_usage_credits_total","purpose"=>"turn","source"=>"provider")
                .set(1.25);
        });
        assert!(
            samples
                .take()
                .iter()
                .all(|row| row.counter && row.value == 0.0)
        );
        metrics::with_local_recorder(&recorder, || {
            metrics::counter!("orch_usage_tokens_total","purpose"=>"turn").absolute(1100);
            metrics::gauge!("orch_usage_credits_total","purpose"=>"turn","source"=>"provider")
                .set(1.375);
        });
        let points = samples.take();
        assert_eq!(
            points
                .iter()
                .find(|row| row.name == "orch_usage_tokens_total")
                .unwrap()
                .value,
            100.0
        );
        assert_eq!(
            points
                .iter()
                .find(|row| row.name == "orch_usage_credits_total")
                .unwrap()
                .value,
            0.125
        );
        assert!(samples.take().iter().all(|row| row.value == 0.0));
        assert!(export.render().contains("1.375"));
    }

    #[test]
    fn exporter_and_stored_intervals_show_the_same_work_without_recounting_it_next_minute() {
        let inner = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let export = inner.handle();
        let samples = super::Samples::default();
        let recorder = super::Mirrored {
            inner,
            samples: samples.clone(),
        };
        metrics::with_local_recorder(&recorder, || {
            metrics::counter!("orch_shared_calls_total","tool"=>"read","result"=>"ok").increment(3);
            metrics::gauge!("orch_ws_connections","feed_kind"=>"all").set(2.0);
            metrics::histogram!("orch_http_request_seconds","route"=>"/me","status"=>"200")
                .record(0.1);
            metrics::histogram!("orch_http_request_seconds","route"=>"/me","status"=>"200")
                .record(0.3);
        });
        let first = samples.take();
        assert_eq!(
            first
                .iter()
                .find(|row| row.name == "orch_shared_calls_total")
                .unwrap()
                .value,
            3.0
        );
        assert!(
            (first
                .iter()
                .find(|row| row.name == "orch_http_request_seconds")
                .unwrap()
                .value
                - 0.2)
                .abs()
                < f64::EPSILON
        );
        assert!(export.render().contains("orch_shared_calls_total"));
        let second = samples.take();
        assert_eq!(
            second
                .iter()
                .find(|row| row.name == "orch_shared_calls_total")
                .unwrap()
                .value,
            0.0
        );
        assert_eq!(
            second
                .iter()
                .find(|row| row.name == "orch_ws_connections")
                .unwrap()
                .value,
            2.0
        );
        assert!(
            !second
                .iter()
                .any(|row| row.name == "orch_http_request_seconds")
        );
        metrics::with_local_recorder(&recorder, || {
            metrics::counter!("orch_shared_calls_total","tool"=>"read","result"=>"ok").absolute(2);
            metrics::counter!("orch_shared_calls_total","tool"=>"read","result"=>"ok").absolute(5);
        });
        assert_eq!(
            samples
                .take()
                .iter()
                .find(|row| row.name == "orch_shared_calls_total")
                .unwrap()
                .value,
            2.0
        );
    }
}
