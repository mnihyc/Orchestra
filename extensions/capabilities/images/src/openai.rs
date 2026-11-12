//! OpenAI account mappings. A compatible wire format alone does not identify this account.
use orch_provider_contract::capabilities::Declarations;

pub(crate) fn declarations() -> Result<Declarations, serde_json::Error> {
    serde_json::from_str(include_str!("openai.json"))
}
