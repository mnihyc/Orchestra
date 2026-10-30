//! The methods of a provider source, the version of the contract they are written in, their parameters, and how long each may take.
//!
//! The same methods and JSON serve a program (plugin kind `provider`, over the plugin transport, declared in
//! its `hello`) and an address (in the envelope of [`crate::address`]). `describe` is required; the others are
//! implemented as the source's descriptor says. Members are only ever added: a member a reader does not know
//! is ignored, and a method a source does not implement is answered with the error `unsupported`.
//!
//! | Method | Parameters | Result | Deadline |
//! |--------|------------|--------|----------|
//! | `describe` | `{}` | `SourceDescriptor` | 10 s |
//! | `facts.read` | `SourceAsk` | a facts report | 8 s |
//! | `models.list` | `SourceAsk` | `{"models": […] or null}` | 30 s |
//! | `resets.spend` | `SourceAsk` with `reset_id` and `idempotency_key` | `{"outcome", "facts"?}` | 30 s |
//! | `credential.present` | `PresentAsk` | `{"headers", "expires_at"?}` | 8 s |
//!
//! A read answers within eight seconds so that a Refresh is answered within the API's ten.

use std::time::Duration;

use orch_plugin_contract::PluginKind;
use orch_plugin_contract::lifecycle;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::source::SourceAsk;

/// The version of this contract.
pub const CONTRACT: u32 = 1;

/// What a source is: required.
pub const DESCRIBE: &str = "describe";
/// Reads the provider's facts.
pub const FACTS_READ: &str = "facts.read";
/// Lists the provider's models.
pub const MODELS_LIST: &str = "models.list";
/// Spends one of the resets the facts listed.
pub const RESETS_SPEND: &str = "resets.spend";
/// Says what to present for the provider's key.
pub const CREDENTIAL_PRESENT: &str = "credential.present";

/// Every method, `describe` first.
pub const METHODS: [&str; 5] = [
    DESCRIBE,
    FACTS_READ,
    MODELS_LIST,
    RESETS_SPEND,
    CREDENTIAL_PRESENT,
];

/// How long `describe` may take.
pub const DESCRIBE_DEADLINE: Duration = lifecycle::PROVIDER_DESCRIBE_DEADLINE;
/// How long `facts.read` and `credential.present` may take.
pub const READ_DEADLINE: Duration = lifecycle::PROVIDER_READ_DEADLINE;
/// How long `models.list` and `resets.spend` may take.
pub const LIST_DEADLINE: Duration = lifecycle::PROVIDER_LIST_DEADLINE;

/// How long `method` may take, for a program and for an address alike.
#[must_use]
pub fn deadline(method: &str) -> Duration {
    lifecycle::deadline(PluginKind::Provider, method)
}

/// The parameters of `describe`: none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DescribeParams {}

/// The parameters of `resets.spend`: what every call is given, with the reset to spend and the key that makes
/// a repeated request spend once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResetSpendParams {
    /// What every call is given.
    #[serde(flatten)]
    pub ask: SourceAsk,
    /// The reset to spend, by the id the source listed it with in `available_resets`.
    pub reset_id: String,
    /// The same key given twice spends once.
    pub idempotency_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_read_answers_within_the_apis_ten_seconds_and_a_list_or_a_reset_within_thirty() {
        assert_eq!(deadline(DESCRIBE), Duration::from_secs(10));
        assert_eq!(deadline(FACTS_READ), Duration::from_secs(8));
        assert_eq!(deadline(CREDENTIAL_PRESENT), Duration::from_secs(8));
        assert_eq!(deadline(MODELS_LIST), Duration::from_secs(30));
        assert_eq!(deadline(RESETS_SPEND), Duration::from_secs(30));
        assert_eq!(READ_DEADLINE, deadline(FACTS_READ));
        assert_eq!(LIST_DEADLINE, deadline(RESETS_SPEND));
        assert_eq!(DESCRIBE_DEADLINE, deadline(DESCRIBE));
    }

    #[test]
    fn the_methods_are_those_a_program_declares_with_describe_required() {
        let required: Vec<&str> = orch_plugin_contract::negotiation::methods(PluginKind::Provider)
            .iter()
            .filter(|spec| spec.required)
            .map(|spec| spec.name)
            .collect();
        assert_eq!(required, [DESCRIBE]);
        let all: Vec<&str> = orch_plugin_contract::negotiation::methods(PluginKind::Provider)
            .iter()
            .map(|spec| spec.name)
            .collect();
        assert_eq!(all, METHODS);
    }

    #[test]
    fn a_reset_is_spent_with_the_ask_beside_its_id_and_its_idempotency_key() {
        let params: ResetSpendParams = serde_json::from_value(json!({
            "provider": { "id": "0199c1a0-0000-7000-8000-00000000000a", "name": "Acme" },
            "endpoints": [], "config": {}, "secrets": {},
            "reset_id": "credit-1", "idempotency_key": "0199c1a0-0000-7000-8000-00000000000b"
        }))
        .unwrap();
        assert_eq!(params.reset_id, "credit-1");
        assert_eq!(params.ask.provider.name, "Acme");
        let written = serde_json::to_value(&params).unwrap();
        assert_eq!(
            written["idempotency_key"],
            "0199c1a0-0000-7000-8000-00000000000b"
        );
        assert_eq!(written["provider"]["name"], "Acme");
        assert_eq!(serde_json::to_value(DescribeParams {}).unwrap(), json!({}));
    }
}
