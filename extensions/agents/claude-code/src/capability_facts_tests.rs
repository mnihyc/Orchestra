//! The adapter owns its actual access facts; the common resolver only projects their words.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
use orch_provider_contract::capabilities::{Declarations, Registry, ThreadContext, catalogue};
use serde_json::json;

#[test]
fn forwarded_host_limits_and_separated_page_fetch_are_declared_without_faking_support() {
    let descriptor =
        crate::descriptor::descriptor(crate::config::PORTABLE_ARCHIVE_CANDIDATE, false);
    let program = descriptor.capability_program();
    let mut registry = Registry::default();
    registry.register("common", catalogue().unwrap()).unwrap();
    let mut declaration = Declarations::default();
    declaration
        .programs
        .insert(descriptor.kind.clone(), program);
    registry.register("program", declaration).unwrap();
    let mut context = ThreadContext {
        program: descriptor.kind,
        access: "forwarded".into(),
        facts: json!({"arrangement":"key_separated"}),
        ..ThreadContext::default()
    };
    let forwarded = registry.resolve(&context);
    let generation = forwarded
        .iter()
        .find(|row| row.capability == "generation")
        .unwrap();
    assert_eq!(
        generation
            .facts
            .iter()
            .map(|fact| fact.id.as_str())
            .collect::<Vec<_>>(),
        [
            "forwarded-host-betas",
            "forwarded-cache-keepalive",
            "forwarded-model-catalogue"
        ]
    );
    let search = forwarded
        .iter()
        .find(|row| row.capability == "web_search")
        .unwrap();
    assert_eq!(search.facts[0].id, "forwarded-search-options");
    assert!(
        forwarded
            .iter()
            .find(|row| row.capability == "page_fetch")
            .unwrap()
            .facts
            .is_empty()
    );
    assert!(
        forwarded.iter().all(|row| row.route.is_none()),
        "explanations do not manufacture an offering or its authority"
    );
    for access in ["native", "shared", "handover"] {
        context.access = access.into();
        context.facts = json!({"arrangement":"separated"});
        let native = registry.resolve(&context);
        assert!(
            native
                .iter()
                .find(|row| row.capability == "generation")
                .unwrap()
                .facts
                .is_empty()
        );
        let fetch = native
            .iter()
            .find(|row| row.capability == "page_fetch")
            .unwrap();
        assert_eq!(fetch.facts[0].id, "separated-web-fetch-location");
        assert!(
            fetch.facts[0].text.contains_key("en") && fetch.facts[0].text.contains_key("zh-CN")
        );
        context.facts = json!({"arrangement":"together"});
        assert!(
            registry
                .resolve(&context)
                .iter()
                .all(|row| row.facts.is_empty())
        );
    }
    let mut profile = std::collections::BTreeMap::from([(
        "_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL".into(),
        "1".into(),
    )]);
    assert!(crate::sessions::validate_profile_environment(&profile).is_err());
    crate::sessions::clear_provider_environment(&mut profile);
    assert!(
        profile.is_empty(),
        "a host explanation must never be defeated by a profile's provider selector"
    );
    let arguments = crate::tools::tool_arguments(true, false, true);
    let tools: Vec<&str> = arguments[1].split(',').collect();
    assert!(tools.contains(&"WebFetch"));
    for tool in ["Bash", "Read", "Write", "Edit", "WebSearch"] {
        assert!(!tools.contains(&tool));
    }
    assert!(
        crate::tools::tool_arguments(true, true, true)[1]
            .split(',')
            .any(|tool| tool == "WebSearch")
    );
}
