//! Authored endpoint facts exercise the real program declaration and the single capability resolver.
use orch_provider_contract::builtins::BuiltinSources;
use orch_provider_contract::capabilities::{
    Availability, Declarations, OfferingRef, Program, Registry, Resolution, ThreadContext,
};
use orch_types::{Id, new_id};
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

fn fixture(address: &str) -> Result<(Registry, ThreadContext, Id), Box<dyn std::error::Error>> {
    let sources = BuiltinSources::new([orch_provider_openai::builtin()])?;
    let endpoint =
        sources.endpoint_capabilities("openai-responses", &Url::parse(address)?, "opaque-offering");
    let mut registry = sources.capabilities().clone();
    let provider = new_id();
    let mut declarations: Declarations = serde_json::from_value(json!({
        "providers":{(provider.to_string()):{
            "title":"Account",
            "credential_roles":["model"],
            "endpoints":{"main":{"interface":"openai-responses","address":address,"credential_role":"model"}},
            "offerings":{"opaque-offering":{
                "endpoint":"main","kind":"model","enabled":true,
                "parameters":{"web_search":["cached","indexed","live"],"requires_standalone_search":true},
                "capabilities":{"generation":{"enabled":true,"modes":["native"]}},
                "native_switches":endpoint.native_switches,
                "native_tools":endpoint.native_tools
            }}
        }}
    }))?;
    let program: Program = serde_json::from_str(include_str!(
        "../../../agents/codex/src/capability-program.json"
    ))?;
    declarations.programs.insert("codex".into(), program);
    registry.register("account", declarations)?;
    let context = ThreadContext {
        program: "codex".into(),
        access: "forwarded".into(),
        current: Some(OfferingRef {
            provider,
            offering: "opaque-offering".into(),
        }),
        interfaces: vec!["openai-responses".into()],
        credential_roles: BTreeMap::from([(provider, vec!["model".into()])]),
        availability: BTreeMap::from([(
            provider,
            BTreeMap::from([(
                "opaque-offering".into(),
                Availability {
                    allowed: true,
                    ..Availability::default()
                },
            )]),
        )]),
        ..ThreadContext::default()
    };
    Ok((registry, context, provider))
}

fn generation(resolved: &[Resolution]) -> Result<&Resolution, Box<dyn std::error::Error>> {
    resolved
        .iter()
        .find(|resolution| resolution.capability == "generation")
        .ok_or_else(|| "generation resolution missing".into())
}

#[test]
fn the_exact_responses_endpoint_declares_v2_at_program_default_without_exposing_its_address()
-> Result<(), Box<dyn std::error::Error>> {
    for address in [
        "https://api.openai.com",
        "https://api.openai.com:443/v1",
        "https://api.openai.com/v1/",
    ] {
        let (registry, context, _) = fixture(address)?;
        let resolved = registry.resolve(&context);
        let result = generation(&resolved)?;
        assert!(result.route.is_some(), "{address}: {:?}", result.reason);
        assert!(result.program_default);
        assert_eq!(result.native_config["capabilities.remote_compaction"], "v2");
        let serialized = serde_json::to_string(result)?;
        assert!(!serialized.contains("api.openai.com"));
        assert!(!serialized.contains("opaque-offering"));
        let carried: Resolution = serde_json::from_str(&serialized)?;
        assert_eq!(carried.native_config, result.native_config);
        assert!(carried.route.is_none());
    }
    Ok(())
}

#[test]
fn compatible_endpoints_and_other_access_kinds_keep_their_own_compaction_behavior()
-> Result<(), Box<dyn std::error::Error>> {
    for address in [
        "https://compatible.example.invalid/v1",
        "https://api.openai.com.attacker.invalid/v1",
        "http://api.openai.com/v1",
        "https://api.openai.com:9443/v1",
        "https://user@api.openai.com/v1",
        "https://api.openai.com/v1?query=1",
        "https://api.openai.com/v1#fragment",
        "https://api.openai.com/private",
    ] {
        let (registry, context, _) = fixture(address)?;
        let resolved = registry.resolve(&context);
        assert_eq!(
            generation(&resolved)?.native_config["capabilities.remote_compaction"],
            "unsupported",
            "{address}"
        );
    }
    let (registry, mut context, _) = fixture("https://api.openai.com/v1")?;
    context.access = "native".into();
    assert!(
        generation(&registry.resolve(&context))?
            .native_config
            .is_empty(),
        "native sign-ins retain their program's own compaction behavior"
    );
    Ok(())
}

#[test]
fn stale_endpoint_projections_and_caller_facts_cannot_enable_remote_compaction()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut registry, mut context, provider) = fixture("https://api.openai.com/v1")?;
    let mut changed = registry
        .extension("account")
        .ok_or("account declaration")?
        .clone();
    changed
        .providers
        .get_mut(&provider)
        .ok_or("provider")?
        .endpoints
        .get_mut("main")
        .ok_or("endpoint")?
        .address = "https://compatible.example.invalid/v1".into();
    registry.register("account", changed)?;
    context.facts = json!({"endpoint_facts":{"host":"api.openai.com","scheme":"https"}});
    assert_eq!(
        generation(&registry.resolve(&context))?.native_config["capabilities.remote_compaction"],
        "unsupported"
    );
    Ok(())
}

#[test]
fn standalone_modes_depend_on_the_current_endpoint_declaration_and_never_the_offerings_name()
-> Result<(), Box<dyn std::error::Error>> {
    let (registry, mut context, _) = fixture("https://api.openai.com/v1")?;
    for mode in ["cached", "indexed", "live"] {
        context
            .choices
            .insert("web_search".into(), Some(mode.into()));
        let resolved = registry.resolve(&context);
        let search = resolved
            .iter()
            .find(|resolution| resolution.capability == "web_search")
            .ok_or("search resolution")?;
        assert!(search.route.is_some(), "{mode}: {:?}", search.reason);
        assert_eq!(search.native_config["supports_standalone_web_search"], true);
        assert_eq!(search.native_config["web_search"], mode);
        assert!(search.modes.iter().any(|offered| offered == mode));
    }
    context.choices.clear();
    let resolved = registry.resolve(&context);
    let search = resolved
        .iter()
        .find(|resolution| resolution.capability == "web_search")
        .ok_or("search")?;
    assert_eq!(search.mode.as_deref(), Some("disabled"));
    assert!(search.program_default);
    assert!(
        search.native_config.is_empty(),
        "the retained program default does not write a native override"
    );
    for mode in ["disabled", "service"] {
        context
            .choices
            .insert("web_search".into(), Some(mode.into()));
        let resolved = registry.resolve(&context);
        let search = resolved
            .iter()
            .find(|resolution| resolution.capability == "web_search")
            .ok_or("search")?;
        assert!(
            !search
                .route
                .as_ref()
                .is_some_and(|route| route.kind == "native-model")
        );
        assert_ne!(
            search.native_config.get("supports_standalone_web_search"),
            Some(&json!(true))
        );
    }
    let (registry, mut context, _) = fixture("https://compatible.example.invalid/v1")?;
    context
        .choices
        .insert("web_search".into(), Some("live".into()));
    let resolved = registry.resolve(&context);
    let search = resolved
        .iter()
        .find(|resolution| resolution.capability == "web_search")
        .ok_or("search")?;
    assert!(search.route.is_none());
    assert!(
        !search
            .modes
            .iter()
            .any(|mode| ["cached", "indexed", "live"].contains(&mode.as_str()))
    );
    assert_ne!(
        search.native_config.get("supports_standalone_web_search"),
        Some(&json!(true))
    );
    Ok(())
}
