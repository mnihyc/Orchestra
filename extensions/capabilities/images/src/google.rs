//! Google's documented compatible Images endpoint; it does not inherit unsupported edits/options.
use orch_provider_contract::capabilities::Declarations;

pub(crate) fn declarations() -> Result<Declarations, serde_json::Error> {
    serde_json::from_str(include_str!("google.json"))
}
