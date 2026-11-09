//! Hosted search options are declared with the shared protocol metadata.

use orch_model_contract::web_search::WebSearchDeclaration;

/// The configured native search tool, independent of the endpoint's vendor.
#[must_use]
pub fn declaration() -> Option<WebSearchDeclaration> {
    orch_model_contract::web_search::declared(orch_model_contract::protocol::OPENAI_RESPONSES)
}
