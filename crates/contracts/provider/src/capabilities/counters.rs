//! Native per-model quantities share one decoder and cumulative baseline, without request or clock inference.
use super::Counter;
use orch_types::Json;
use std::collections::BTreeMap;

impl Counter {
    /// A missing/invalid vendor count is unknown; a declared false-condition value may explicitly be zero.
    #[must_use]
    pub fn count(&self, response: &Json) -> Option<i64> {
        if !self.valid_when.is_null()
            && orch_types::schema::validate(&self.valid_when, response).is_err()
        {
            return None;
        }
        if !self.only_when.is_null()
            && orch_types::schema::validate(&self.only_when, response).is_err()
        {
            return self.otherwise.filter(|value| *value >= 0);
        }
        if let Some(items) = &self.items {
            use base64::Engine as _;
            let rows = response.pointer(&items.pointer)?.as_array()?;
            let mut count = 0i64;
            for row in rows {
                if !items.only_when.is_null()
                    && orch_types::schema::validate(&items.only_when, row).is_err()
                {
                    continue;
                }
                let encoded = row.pointer(&items.base64)?.as_str()?;
                if encoded.is_empty()
                    || base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .ok()?
                        .is_empty()
                {
                    return None;
                }
                count = count.checked_add(1)?;
            }
            return Some(count);
        }
        self.constant
            .or_else(|| response.pointer(&self.pointer).and_then(Json::as_i64))
            .filter(|value| *value >= 0)
    }
}

/// Known per-capability quantities. Missing is unknown; zero is a measurement.
pub type Counts = BTreeMap<String, i64>;
/// The model identity is the exact one reported by the source, not the thread's requested model.
pub type ModelCounts = BTreeMap<String, Counts>;

/// Existing numeric request measurements use the same capability identities as native aggregates.
/// Only declared, actually told quantities participate; unsupported and unread remain absent.
#[must_use]
pub fn from_measurements(
    capabilities: &BTreeMap<String, super::Capability>,
    measurements: &Json,
) -> Counts {
    capabilities
        .iter()
        .filter_map(|(capability, definition)| {
            let measurement = definition
                .counter_accounting
                .as_ref()?
                .measurement
                .as_deref()?;
            let source = measurements.get(measurement)?;
            if source.get("state").and_then(Json::as_str) != Some("told") {
                return None;
            }
            let counter = Counter {
                pointer: "/value".into(),
                ..Counter::default()
            };
            Some((capability.clone(), counter.count(source)?))
        })
        .collect()
}

/// One ordered native process's cumulative snapshots; history readers never feed the usage ledger.
#[derive(Debug, Clone, Default)]
pub struct NativeCounters {
    bases: BTreeMap<String, BTreeMap<String, Option<i64>>>,
    new_model_is_empty: bool,
    last_ref: Option<String>,
}

impl NativeCounters {
    /// Only a genuinely fresh process/conversation proves that its first totals grow from zero.
    pub fn start(&mut self, fresh: bool) {
        self.bases.clear();
        self.new_model_is_empty = fresh;
        self.last_ref = None;
    }

    /// `models` is the program's complete model snapshot at one ordered turn boundary. A missing model
    /// or count invalidates its interval; a resumed first snapshot only establishes the baseline.
    /// The caller supplies the actual native boundary identity where the program reports one.
    pub fn observe(
        &mut self,
        models: Option<&Json>,
        declarations: &BTreeMap<String, Counter>,
        source_ref: Option<&str>,
    ) -> ModelCounts {
        if source_ref.is_some() && source_ref == self.last_ref.as_deref() {
            return ModelCounts::new();
        }
        self.last_ref = source_ref.map(str::to_owned);
        let Some(models) = models
            .and_then(Json::as_object)
            .filter(|models| !models.is_empty())
        else {
            self.bases
                .values_mut()
                .for_each(|counts| counts.values_mut().for_each(|value| *value = None));
            self.new_model_is_empty = false;
            return ModelCounts::new();
        };
        for (model, counts) in &mut self.bases {
            if !models.contains_key(model) {
                counts.values_mut().for_each(|value| *value = None);
            }
        }
        let mut result = ModelCounts::new();
        let new_model_is_empty = self.new_model_is_empty;
        for (model, report) in models {
            let baseline = self.bases.entry(model.clone()).or_default();
            for (capability, declaration) in declarations
                .iter()
                .filter(|(_, counter)| !counter.reported_cost)
            {
                let current = declaration.count(report);
                let previous = baseline
                    .entry(capability.clone())
                    .or_insert_with(|| new_model_is_empty.then_some(0));
                let quantity = current.and_then(|current| {
                    if !declaration.cumulative {
                        return Some(current);
                    }
                    previous.and_then(|before| {
                        if current >= before {
                            Some(current - before)
                        } else {
                            declaration.reset_on_decrease.then_some(current)
                        }
                    })
                });
                *previous = current;
                if let Some(quantity) = quantity {
                    result
                        .entry(model.clone())
                        .or_default()
                        .insert(capability.clone(), quantity);
                }
            }
        }
        // A complete snapshot establishes that a newly appearing model was not previously used.
        // Previously seen missing counters remain unknown in their own slots.
        self.new_model_is_empty = true;
        result
    }
}

/// An aggregate covers already recorded request counts. An inconsistent or negative report is unknown,
/// never a negative charge or a fabricated zero. No request count is inferred from result/item counts.
#[must_use]
pub fn remainder(reported: i64, recorded: i64) -> Option<i64> {
    (reported >= 0 && recorded >= 0)
        .then(|| reported.checked_sub(recorded))
        .flatten()
        .filter(|value| *value >= 0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
    use super::*;
    use serde_json::json;
    fn declarations() -> BTreeMap<String, Counter> {
        BTreeMap::from([
            (
                "lookup".into(),
                Counter {
                    pointer: "/lookups".into(),
                    cumulative: true,
                    ..Counter::default()
                },
            ),
            (
                "pictures".into(),
                Counter {
                    pointer: "/pictures".into(),
                    ..Counter::default()
                },
            ),
        ])
    }
    #[test]
    fn output_cardinality_requires_every_selected_completed_row_to_decode() {
        let counter = Counter {
            items: Some(super::super::CountItems {
                pointer: "/outputs".into(),
                base64: "/bytes".into(),
                only_when: json!({"required":["state"],"properties":{"state":{"const":"complete"}}}),
            }),
            ..Counter::default()
        };
        assert_eq!(counter.count(&json!({"n":9,"outputs":[{"state":"preview","bytes":"invalid"},{"state":"complete","bytes":"YQ=="},{"state":"complete","bytes":"YQ=="}]})),Some(2));
        assert_eq!(counter.count(&json!({"n":9,"outputs":[{"state":"complete","bytes":"YQ=="},{"state":"complete","bytes":"invalid"}]})),None);
        assert_eq!(counter.count(&json!({"outputs":[]})), Some(0));
        assert_eq!(counter.count(&json!({"n":9})), None);
    }
    #[test]
    fn every_programs_request_measurements_join_the_same_declaration_named_counts() {
        let declarations: super::super::Declarations =
            serde_json::from_value(json!({"capabilities":{
                "lookup":{"counter_accounting":{"measurement":"actual_lookups"}},
                "other":{"counter_accounting":{"measurement":"missing"}},
                "future":{"counter_accounting":{"measurement":"future_count"}}
            }}))
            .unwrap();
        let counts = from_measurements(
            &declarations.capabilities,
            &json!({"actual_lookups":{"state":"told","value":0},"future_count":{"state":"told","value":3},"missing":{"state":"unread"}}),
        );
        assert_eq!(
            counts,
            Counts::from([("lookup".into(), 0), ("future".into(), 3)])
        );
        assert!(
            from_measurements(
                &declarations.capabilities,
                &json!({"actual_lookups":{"state":"told","value":-1}})
            )
            .is_empty()
        );
    }
    #[test]
    fn models_counters_zero_missing_and_replayed_boundaries_are_independent() {
        let mut counters = NativeCounters::default();
        counters.start(true);
        let declarations = declarations();
        let a = json!({"main":{"lookups":2,"pictures":0},"helper":{"lookups":3}});
        let first = counters.observe(Some(&a), &declarations, Some("first"));
        assert_eq!(first["main"]["lookup"], 2);
        assert_eq!(first["main"]["pictures"], 0);
        assert_eq!(first["helper"]["lookup"], 3);
        assert!(!first["helper"].contains_key("pictures"));
        assert!(
            counters
                .observe(Some(&a), &declarations, Some("first"))
                .is_empty()
        );
        let b = json!({"main":{"lookups":2},"helper":{"lookups":5},"new":{"lookups":1}});
        let next = counters.observe(Some(&b), &declarations, Some("next"));
        assert_eq!(next["main"]["lookup"], 0);
        assert_eq!(next["helper"]["lookup"], 2);
        assert_eq!(next["new"]["lookup"], 1);
        assert!(!next["main"].contains_key("pictures"));
        let gap = json!({"main":{"lookups":-1}});
        assert!(
            counters
                .observe(Some(&gap), &declarations, Some("gap"))
                .is_empty()
        );
        assert!(
            !counters
                .observe(Some(&b), &declarations, Some("baseline"))
                .contains_key("main")
        );
        let c = json!({"main":{"lookups":4},"helper":{"lookups":6}});
        assert_eq!(
            counters.observe(Some(&c), &declarations, Some("last"))["main"]["lookup"],
            2
        );
    }
    #[test]
    fn resume_unknown_snapshots_and_declared_resets_never_restore_old_spend() {
        let mut counters = NativeCounters::default();
        counters.start(false);
        let mut declarations = declarations();
        assert!(
            counters
                .observe(
                    Some(&json!({"m":{"lookups":40}})),
                    &declarations,
                    Some("resume")
                )
                .is_empty()
        );
        assert_eq!(
            counters.observe(
                Some(&json!({"m":{"lookups":42}})),
                &declarations,
                Some("next")
            )["m"]["lookup"],
            2
        );
        assert!(
            counters
                .observe(
                    Some(&json!({"m":{"lookups":1}})),
                    &declarations,
                    Some("unknown-reset")
                )
                .is_empty()
        );
        declarations.get_mut("lookup").unwrap().reset_on_decrease = true;
        assert_eq!(
            counters.observe(
                Some(&json!({"m":{"lookups":0}})),
                &declarations,
                Some("reset")
            )["m"]["lookup"],
            0
        );
        assert!(counters.observe(None, &declarations, None).is_empty());
        assert!(
            counters
                .observe(
                    Some(&json!({"m":{"lookups":100},"other":{"lookups":7}})),
                    &declarations,
                    Some("unknown-gap")
                )
                .is_empty()
        );
        assert_eq!(remainder(7, 2), Some(5));
        assert_eq!(remainder(0, 0), Some(0));
        assert_eq!(remainder(1, 2), None);
        assert_eq!(remainder(-1, 0), None);
    }
}
