//! OpenRouter's dedicated Image API, including its own definitive endpoint discovery and cost.
use orch_provider_contract::capabilities::Declarations;

pub(crate) fn declarations() -> Result<Declarations, serde_json::Error> {
    serde_json::from_str(include_str!("openrouter.json"))
}
