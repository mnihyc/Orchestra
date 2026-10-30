//! Generate OpenAPI from route metadata.
//!
//! API contracts own the renderer used by the served document and these release artifacts.
//! Keep contract regressions here; the implementation has one home.

use orch_api_contract::routes::Route;
use serde_json::Value;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_is_told_that_only_a_created_registration_carries_a_session() {
        let document = document(orch_api_contract::auth::ROUTES, "test");
        let replies = &document["paths"]["/auth/register"]["post"]["responses"];
        let created = &replies["201"]["content"]["application/json"]["schema"]["$ref"];
        let pending = &replies["202"]["content"]["application/json"]["schema"]["$ref"];
        assert_eq!(created, "#/components/schemas/AuthRegisterResult");
        let registered = &document["components"]["schemas"]["AuthRegisterResult"]["properties"];
        assert!(registered.get("access_token").is_some());
        assert!(registered.get("user").is_some());
        assert_eq!(
            registered["accepted_invitation"]["anyOf"][0]["$ref"],
            "#/components/schemas/InvitationAcceptReply"
        );
        assert_eq!(pending, "#/components/schemas/VerificationRequired");
        let required = &document["components"]["schemas"]["VerificationRequired"];
        assert_eq!(
            required["properties"]["verification_required"]["const"],
            true
        );
        assert!(required["properties"].get("access_token").is_none());
    }
}
