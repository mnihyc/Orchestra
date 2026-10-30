//! Serve contract documentation rendered once and cached as bytes.

use bytes::Bytes;
use orch_api_contract::routes::Route;
use serde_json::Value;
use std::sync::OnceLock;

/// The wire types a route refers to that its own contract is below: the browser's socket and an agent program's
/// descriptor.
fn schemas(generator: &mut schemars::SchemaGenerator) {
    orch_realtime_contract::schemas(generator);
    orch_agent_contract::schemas(generator);
}

/// The OpenAPI document of `routes`, for the release `version`, including realtime wire types.
#[must_use]
pub fn document(routes: &[Route], version: &str) -> Value {
    orch_api_contract::openapi::document(routes, version, schemas)
}

static SERVED: OnceLock<Bytes> = OnceLock::new();

/// The document of every request the contract declares, for the release `version`, as it is
/// served: made on the first call, the same bytes from then on.
#[must_use]
pub fn served(version: &str) -> Bytes {
    SERVED
        .get_or_init(|| {
            let text = serde_json::to_vec(&document(&orch_api_contract::routes::all(), version))
                .unwrap_or_default();
            Bytes::from(text)
        })
        .clone()
}
