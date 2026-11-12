//! Generate OpenAPI from route metadata.
//!
//! API contracts own the renderer used by the served document and these release artifacts.
//! Keep contract regressions here; the implementation has one home.

use orch_api_contract::routes::Route;
use serde_json::Value;

/// Existing realtime wire and agent descriptor schemas shared by key clients and the frontend.
fn schemas(generator: &mut schemars::SchemaGenerator) {
    orch_realtime_contract::schemas(generator);
    orch_agent_contract::schemas(generator);
}

/// The OpenAPI document of `routes`, for the release `version`, including realtime wire types.
#[must_use]
pub fn document(routes: &[Route], version: &str) -> Value {
    orch_api_contract::openapi::document(routes, version, schemas)
}

/// Unpublished full input for the frontend, from the same contract renderer.
pub fn internal_document(routes: &[Route], version: &str) -> Value {
    orch_api_contract::openapi::internal_document(routes, version, schemas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_is_told_that_only_a_created_registration_carries_a_session() {
        let document = internal_document(orch_api_contract::auth::ROUTES, "test");
        let replies = &document["paths"]["/auth/register"]["post"]["responses"];
        let created = &replies["201"]["content"]["application/json"]["schema"]["$ref"];
        let pending = &replies["202"]["content"]["application/json"]["schema"]["$ref"];
        assert_eq!(created, "#/components/schemas/AuthRegisterCreatedReply");
        let schemas = &document["components"]["schemas"];
        for (name, full) in [
            ("AuthRegisterCreatedReply", "AuthRegisterResult"),
            ("AuthRegisterPendingReply", "VerificationRequired"),
        ] {
            let alternatives = schemas[name]["anyOf"].as_array().unwrap();
            let references: Vec<_> = alternatives
                .iter()
                .map(|schema| schema["$ref"].as_str().unwrap())
                .collect();
            assert_eq!(
                references,
                [
                    format!("#/components/schemas/{full}"),
                    "#/components/schemas/AccountCreatedReceipt".to_owned(),
                ],
                "each status advertises only its original body and a session-free receipt"
            );
        }
        let registered = &document["components"]["schemas"]["AuthRegisterResult"]["properties"];
        assert!(registered.get("access_token").is_some());
        assert!(registered.get("user").is_some());
        assert_eq!(
            registered["accepted_invitation"]["anyOf"][0]["$ref"],
            "#/components/schemas/InvitationAcceptReply"
        );
        assert_eq!(pending, "#/components/schemas/AuthRegisterPendingReply");
        let required = &document["components"]["schemas"]["VerificationRequired"];
        assert_eq!(
            required["properties"]["verification_required"]["const"],
            true
        );
        assert!(required["properties"].get("access_token").is_none());
        let receipt = &schemas["AccountCreatedReceipt"]["properties"];
        assert_eq!(receipt["sign_in_required"]["const"], true);
        assert!(receipt.get("user_id").is_some());
        for secret in [
            "access_token",
            "refresh_token",
            "user",
            "accepted_invitation",
        ] {
            assert!(receipt.get(secret).is_none());
        }
    }
}
