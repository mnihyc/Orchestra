//! Conditional native quotes use declared request facts and actual counters, never result-list lengths.
use super::Binding;
use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Original-currency terms for one selected native capability.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct NativePrice {
    /// Original currency; only the existing matching catalogue conversion may count it as credits.
    pub currency: String,
    /// Additive charges, independently of token prices and processing tiers.
    pub terms: Vec<PriceTerm>,
}

/// One exact price multiplied by a reported counter, optionally after its included allowance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PriceTerm {
    /// Nonnegative exact decimal per charged unit.
    pub amount: String,
    /// Declaration-named actual quantity; an absent counter is unknown.
    pub counter: String,
    /// Units included for each reported use of another counter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub included: Option<IncludedUnits>,
}

/// An allowance such as a declared number of results per reported search.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct IncludedUnits {
    /// Actual quantity supplying the number of allowances.
    pub counter: String,
    /// Included units per actual use; no requested maximum is a measured quantity.
    pub each: u64,
}

/// A provider-owned predicate over the neutral selection lifted from the final outgoing request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PriceRule {
    /// Local JSON schema; a null predicate matches every selection.
    pub when: Json,
    /// Absent means provider-dependent, never free.
    pub quote: Option<NativePrice>,
}

/// A missing declaration retains legacy pricing; a declared but unmatched quote remains unknown.
#[must_use]
pub fn selected(binding: &Binding, selection: &Json) -> Option<Option<NativePrice>> {
    let rules = binding.extensions.get("price_rules")?;
    let Ok(rules) = serde_json::from_value::<Vec<PriceRule>>(rules.clone()) else {
        return Some(None);
    };
    Some(
        rules
            .into_iter()
            .find_map(|rule| {
                (rule.when.is_null() || orch_types::schema::validate(&rule.when, selection).is_ok())
                    .then_some(rule.quote)
            })
            .flatten(),
    )
}
