//! Machine-program consent is an environment's own, attributed path/version decision.
#![allow(clippy::unwrap_used, reason = "a contract test fails by panicking")]

use orch_settings_contract::registry::{self, Class, Merge, Scope};
use serde_json::json;

const KEY: &str = "exec.version_overrides";

fn consent() -> serde_json::Value {
    json!({"codex":{
        "path":"/controlled/bin/codex", "version":"unknown",
        "by":"0199c1a0-0000-7000-8000-00000000000a", "at":"2026-10-31T00:00:00Z"
    }})
}

#[test]
fn consent_defaults_off_and_can_be_set_only_on_its_environment() {
    let definition = registry::get(KEY).unwrap();
    assert_eq!(definition.class, Class::Parameter);
    assert_eq!(definition.merge, Merge::Replace);
    assert_eq!(definition.scopes, [Scope::Environment]);
    assert_eq!(definition.default_value(), Some(json!({})));
    assert!(registry::validate_at(KEY, Scope::Environment, &consent()).is_ok());
    for scope in [
        Scope::System,
        Scope::Org,
        Scope::Project,
        Scope::Topic,
        Scope::Thread,
        Scope::Provider,
        Scope::Host,
    ] {
        assert!(registry::validate_at(KEY, scope, &consent()).is_err());
    }
}

#[test]
fn stored_consent_has_the_program_tuple_and_server_attribution_without_agent_build_or_hash() {
    assert!(registry::validate_at(KEY, Scope::Environment, &consent()).is_ok());
    let mut missing_writer = consent();
    missing_writer["codex"]
        .as_object_mut()
        .unwrap()
        .remove("by");
    assert!(registry::validate_at(KEY, Scope::Environment, &missing_writer).is_err());
    for field in ["agent_version", "harness_build", "hash"] {
        let mut extra = consent();
        extra["codex"][field] = json!("not part of the decision");
        assert!(registry::validate_at(KEY, Scope::Environment, &extra).is_err());
    }
}
