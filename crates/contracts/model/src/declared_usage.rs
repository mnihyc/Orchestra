//! Endpoint counter declarations enrich the existing protocol parser, never parse another byte stream.
use crate::usage::{ObservedUsage, ResponseFacts, UsageObserver};
use orch_provider_contract::capabilities::{Binding, Counter};
use orch_types::Json;
use serde_json::json;
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet};

/// Freeze only public price terms selected by the final native body, without retaining that body.
#[must_use]
pub fn native_prices(
    bindings: &BTreeMap<String, Option<Binding>>,
    body: &Json,
) -> BTreeMap<String, Option<orch_provider_contract::capabilities::pricing::NativePrice>> {
    bindings
        .iter()
        .filter_map(|(capability, binding)| {
            let binding = binding.as_ref()?;
            if !binding.extensions.contains_key("price_rules") {
                return None;
            }
            let selected = serde_json::from_value::<crate::web_search::WebSearchDeclaration>(
                binding.request.clone(),
            )
            .ok()
            .and_then(|declaration| declaration.selection(body).ok());
            if selected
                .as_ref()
                .is_some_and(|selection| selection["mode"] == "disabled")
            {
                return None;
            }
            let quote = selected
                .as_ref()
                .and_then(|selection| {
                    orch_provider_contract::capabilities::pricing::selected(binding, selection)
                })
                .flatten();
            Some((capability.clone(), quote))
        })
        .collect()
}

/// Per-request cumulative observations, retaining only declared quantities and their actual evidence.
#[derive(Debug, Clone, Default)]
pub struct DeclaredUsage {
    counters: BTreeMap<String, Counter>,
    values: BTreeMap<String, Option<i64>>,
    explicit: BTreeSet<String>,
}
impl DeclaredUsage {
    /// An endpoint's selected native bindings use the same counter shape as API services.
    pub fn configure(&mut self, bindings: &BTreeMap<String, Option<Binding>>) {
        self.counters = bindings
            .values()
            .flatten()
            .flat_map(|binding| {
                binding
                    .counters
                    .iter()
                    .map(|(name, counter)| (name.clone(), counter.clone()))
            })
            .collect();
        self.values = self
            .counters
            .iter()
            .filter(|(_, counter)| !counter.optional && !counter.reported_cost)
            .map(|(name, _)| (name.clone(), None))
            .collect();
        self.explicit.clear();
    }
    /// A decoded standard usage object, on the original parser's ordinary usage seam.
    pub fn observe_usage(&mut self, usage: &Json) {
        let response = json!({"usage":usage});
        for (name, counter) in self
            .counters
            .iter()
            .filter(|(_, counter)| !counter.reported_cost)
        {
            if !counter.valid_when.is_null()
                && orch_types::schema::validate(&counter.valid_when, &response).is_err()
            {
                continue;
            }
            let explicit =
                counter.constant.is_some() || response.pointer(&counter.pointer).is_some();
            if !explicit && counter.cumulative && self.explicit.contains(name) {
                continue;
            }
            if counter.optional && !explicit {
                continue;
            }
            self.values.insert(name.clone(), counter.count(&response));
            if explicit {
                self.explicit.insert(name.clone());
            }
        }
    }
    /// Replace incompatible generic search/item counts with declared actual facts, including unknown.
    pub fn apply(&self, usage: &mut ObservedUsage) {
        if self.counters.contains_key("web_searches") {
            usage.web_searches = self.values.get("web_searches").copied().flatten();
        }
        if self.counters.contains_key("web_fetches") {
            usage.web_fetches = self.values.get("web_fetches").copied().flatten();
        }
        usage.declared_counters.extend(self.values.clone());
    }
    /// A configured cost source owns that quantity even when a partial frame omits it.
    #[must_use]
    pub fn has_reported_cost(&self) -> bool {
        self.counters.values().any(|counter| counter.reported_cost)
    }

    /// One declared original cost, extracted from raw JSON without floating-point conversion.
    /// Compatible model formats define these fields within their usage object.
    pub fn reported_cost(&self, raw_usage: &str) -> Option<(Box<RawValue>, Option<String>)> {
        let mut costs = self
            .counters
            .values()
            .filter(|counter| counter.reported_cost);
        let counter = costs.next()?;
        if costs.next().is_some() {
            return None;
        }
        let pointer = counter.pointer.strip_prefix("/usage")?;
        let raw = raw_pointer(raw_usage, pointer)?;
        Some((
            raw,
            (!counter.unit.is_empty()).then(|| counter.unit.clone()),
        ))
    }
}
/// One actual JSON token without floating-point conversion, shared with declared service counters.
#[must_use]
pub fn raw_pointer(raw: &str, pointer: &str) -> Option<Box<RawValue>> {
    let mut value: Box<RawValue> = serde_json::from_str(raw).ok()?;
    if pointer.is_empty() {
        return Some(value);
    }
    for segment in pointer.strip_prefix('/')?.split('/') {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        value = if value.get().trim_start().starts_with('{') {
            serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(value.get())
                .ok()?
                .remove(&segment)?
        } else {
            serde_json::from_str::<Vec<Box<RawValue>>>(value.get())
                .ok()?
                .into_iter()
                .nth(segment.parse().ok()?)?
        };
    }
    Some(value)
}
/// Keeps unsupported extra facts unknown while preserving the existing observer's tokens and failures.
struct UnknownCounters {
    inner: Box<dyn UsageObserver>,
    counters: BTreeMap<String, Counter>,
}
impl UsageObserver for UnknownCounters {
    fn response_status(&mut self, status: u16) {
        self.inner.response_status(status);
    }
    fn finish(&mut self, whole: bool) {
        self.inner.finish(whole);
    }
    fn tokens_reported(&self) -> bool {
        self.inner.tokens_reported()
    }
    fn observe(&mut self, chunk: &[u8]) {
        self.inner.observe(chunk);
    }
    fn usage(&self) -> Option<ObservedUsage> {
        let mut usage = self.inner.usage()?;
        for (name, counter) in &self.counters {
            if counter.reported_cost {
                usage.cost = None;
                usage.cost_unit = None;
            }
            if name == "web_searches" {
                usage.web_searches = None;
            }
            if name == "web_fetches" {
                usage.web_fetches = None;
            }
            if !counter.optional && !counter.reported_cost {
                usage.declared_counters.insert(name.clone(), None);
            }
        }
        Some(usage)
    }
    fn response_facts(&self) -> ResponseFacts {
        self.inner.response_facts()
    }
}
/// Configuration cannot remove a working transport merely because its observer does not know a new counter.
pub fn configured(
    mut observer: Box<dyn UsageObserver>,
    bindings: &BTreeMap<String, Option<Binding>>,
) -> Box<dyn UsageObserver> {
    if observer.configure_capabilities(bindings) {
        return observer;
    }
    let counters = bindings
        .values()
        .flatten()
        .flat_map(|binding| {
            binding
                .counters
                .iter()
                .map(|(name, counter)| (name.clone(), counter.clone()))
        })
        .collect();
    Box::new(UnknownCounters {
        inner: observer,
        counters,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    #[test]
    fn declared_successes_replace_attempts_and_repeat_snapshots_without_summing_or_losing_zero() {
        let bindings=serde_json::from_value(json!({"web_search":{"counters":{"web_searches":{"pointer":"/usage/successes","cumulative":true}}}})).unwrap();
        let mut declared = DeclaredUsage::default();
        declared.configure(&bindings);
        declared.observe_usage(&json!({"successes":2,"attempts":7}));
        declared.observe_usage(&json!({"successes":2,"attempts":7}));
        let mut seen = ObservedUsage {
            web_searches: Some(7),
            ..ObservedUsage::default()
        };
        declared.apply(&mut seen);
        assert_eq!(seen.web_searches, Some(2));
        declared.observe_usage(&json!({"output_tokens":10}));
        declared.apply(&mut seen);
        assert_eq!(seen.web_searches, Some(2));
        declared.observe_usage(&json!({"successes":0}));
        declared.apply(&mut seen);
        assert_eq!(seen.web_searches, Some(0));
        declared.observe_usage(&json!({"successes":-1}));
        declared.apply(&mut seen);
        assert_eq!(seen.web_searches, None);
    }
    #[test]
    fn reported_cost_keeps_every_original_digit_and_the_declared_unit() {
        let bindings=serde_json::from_value(json!({"web_search":{"counters":{"actual":{"pointer":"/usage/account/total","reported_cost":true,"unit":"USD"}}}})).unwrap();
        let mut declared = DeclaredUsage::default();
        declared.configure(&bindings);
        let (amount, unit) = declared
            .reported_cost(r#"{"account":{"total":0.1234567890123456789}}"#)
            .unwrap();
        assert_eq!(amount.get(), "0.1234567890123456789");
        assert_eq!(unit.as_deref(), Some("USD"));
    }
}
