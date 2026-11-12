//! Actual shipped-program declarations through the common resolver; no transport/admission is mocked as executed.
#![allow(
    clippy::unwrap_used,
    reason = "malformed fixture declarations fail the test"
)]

use super::{descriptor, tests::learned};
use orch_provider_contract::capabilities as cap;
use orch_types::new_id;
use serde_json::json;

const FIXTURE: &str = "ACP route fixture";

/// Carry the actual common resolution through serialization into the shipped managed patch.
fn assert_managed_launch(rows: &[cap::Resolution], native_search: bool, native_fetch: bool) {
    let program = crate::programs::dsh().unwrap();
    let model = "fixture-model";
    let captured = orch_agent_contract::session::NativeModelContext::captured(
        model.into(),
        Default::default(),
        rows,
    )
    .unwrap();
    let captured = serde_json::from_value(serde_json::to_value(captured).unwrap()).unwrap();
    let mut variables =
        crate::program::Variables::new(std::path::PathBuf::from("/managed/session"));
    for (key, value) in [
        ("profile_home", "/managed"),
        ("cwd", "/workspace"),
        ("workspace", "/workspace"),
        ("model", model),
        ("protocol", "anthropic-messages"),
        ("api", "anthropic-messages"),
        (
            "llm_url",
            "http://127.0.0.1:41000/llm/0123456789abcdef0123456789abcdef",
        ),
        (
            "web_search_url",
            "http://127.0.0.1:41000/llm/0123456789abcdef0123456789abcdef/v1",
        ),
        ("llm_token", "synthetic-managed-session"),
        ("context_window", "200000"),
    ] {
        variables.set(key, value);
    }
    variables.models = vec![model.into()];
    variables.approval = program.approval(orch_agent_contract::descriptor::ApprovalMode::Ask);
    variables.set(
        "permission_mode",
        variables.approval.permission_mode.clone(),
    );
    variables
        .select_web_search_for_model(
            &json!({"web_search":"live"}),
            Some("disabled"),
            &program,
            model,
            Some(&captured),
        )
        .unwrap();
    variables.select_page_fetch_for_model(Some("on"), &program, model, Some(&captured));
    let plan = program.plan(&variables).unwrap();
    let patch = String::from_utf8(plan.files[0].content.clone()).unwrap();
    assert_eq!(patch.contains("search: true\n"), native_search);
    assert_eq!(patch.contains("fetch: true\n"), native_fetch);
    assert!(
        patch.contains(
            "baseURL: \"http://127.0.0.1:41000/llm/0123456789abcdef0123456789abcdef/v1\""
        )
    );
    assert!(patch.contains("apiKeyEnv: ORCH_LLM_TOKEN"));
    assert!(!patch.contains("search.example.invalid"));
    assert!(!patch.contains("generation.example.invalid"));
    assert!(!patch.contains("api.deepseek.com"));
    assert_eq!(
        plan.env.get("ORCH_LLM_TOKEN").map(String::as_str),
        Some("synthetic-managed-session")
    );
    assert!(!plan.env.contains_key("DEEPSEEK_API_KEY"));
}

fn fixture(protocol: &str) -> (cap::Registry, cap::ThreadContext, cap::OfferingRef) {
    let program = crate::programs::dsh().unwrap();
    let descriptor = descriptor(&program, &learned());
    let reference = cap::OfferingRef {
        provider: new_id(),
        offering: "fixture-model".into(),
    };
    let mut source = cap::Declarations::default();
    source
        .programs
        .insert(program.kind.clone(), descriptor.capability_program());
    source.providers.insert(reference.provider, serde_json::from_value(json!({
        "title":"Generation fixture","credential_roles":["model"],
        "endpoints":{"main":{"interface":protocol,"address":"https://generation.example.invalid/v1","credential_role":"model"}},
        "offerings":{"fixture-model":{"endpoint":"main","kind":"model","enabled":true,
            "parameters":{"web_search":["live"]},
            "prices":{"search":{"amount":"0.002","currency":"USD","unit":"search"}}
        }}
    })).unwrap());
    let mut registry = cap::Registry::default();
    registry
        .register("common", cap::catalogue().unwrap())
        .unwrap();
    registry.register(FIXTURE, source).unwrap();
    let mut context = cap::ThreadContext {
        program: program.kind,
        access: "forwarded".into(),
        current: Some(reference.clone()),
        interfaces: vec![protocol.into()],
        ..Default::default()
    };
    context
        .choices
        .insert("web_search".into(), Some("live".into()));
    context
        .credential_roles
        .insert(reference.provider, vec!["model".into()]);
    context
        .availability
        .entry(reference.provider)
        .or_default()
        .insert(
            reference.offering.clone(),
            cap::Availability {
                allowed: true,
                show_provider: true,
                show_offering: true,
                show_price: true,
                ..Default::default()
            },
        );
    (registry, context, reference)
}

fn edit(registry: &mut cap::Registry, change: impl FnOnce(&mut cap::Declarations)) {
    let mut source = registry.extension(FIXTURE).unwrap().clone();
    change(&mut source);
    registry.register(FIXTURE, source).unwrap();
}

fn search(registry: &cap::Registry, context: &cap::ThreadContext) -> cap::Resolution {
    registry
        .resolve(context)
        .into_iter()
        .find(|value| value.capability == "web_search")
        .unwrap()
}

fn page_fetch(registry: &cap::Registry, context: &cap::ThreadContext) -> cap::Resolution {
    registry
        .resolve(context)
        .into_iter()
        .find(|value| value.capability == "page_fetch")
        .unwrap()
}

#[test]
fn live_search_uses_the_actual_model_route_and_hides_unreadable_account_metadata() {
    let (registry, mut context, reference) = fixture("anthropic-messages");
    // Actual candidate facts must overwrite a stale generic context fact.
    context.facts = json!({"route_kind":"native-program"});
    let live = search(&registry, &context);
    let route = live.route.as_ref().unwrap();
    assert_eq!(route.kind, "native-model");
    assert_eq!(route.offering.as_ref(), Some(&reference));
    assert_eq!(route.native_value, json!("live"));
    assert_eq!(live.native_config.get("web_search"), Some(&json!("live")));
    assert!(!route.disable_native);
    assert!(route.orch_tool.is_none());
    assert!(live.modes.iter().any(|mode| mode == "live"));
    assert!(!live.modes.iter().any(|mode| mode == "service"));
    assert_eq!(live.provider.as_ref().unwrap().id, reference.provider);
    assert!(!live.prices.is_empty());

    let state = context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap();
    state.show_provider = false;
    state.show_offering = false;
    state.show_price = false;
    let private = search(&registry, &context);
    assert!(
        private.route.is_some(),
        "metadata visibility is separate from current Use"
    );
    assert!(private.provider.is_none() && private.offering.is_none() && private.prices.is_empty());
    assert!(
        !serde_json::to_string(&private)
            .unwrap()
            .contains("generation.example.invalid")
    );
}

#[test]
fn model_refusals_cannot_reopen_search_as_an_independent_program_tool() {
    for case in [
        "absent_modes",
        "disabled_modes",
        "explicit_disabled",
        "offering_tool_null",
        "endpoint_tool_null",
        "offering_tool_refused",
        "endpoint_tool_refused",
    ] {
        let (mut registry, mut context, reference) = fixture("anthropic-messages");
        context.facts = json!({"route_kind":"native-model"});
        edit(&mut registry, |source| {
            let provider = source.providers.get_mut(&reference.provider).unwrap();
            match case {
                "endpoint_tool_null" => {
                    provider
                        .endpoints
                        .get_mut("main")
                        .unwrap()
                        .native_tools
                        .insert("web_search".into(), None);
                }
                "endpoint_tool_refused" => {
                    provider
                        .endpoints
                        .get_mut("main")
                        .unwrap()
                        .native_tools
                        .insert(
                            "web_search".into(),
                            Some(cap::Binding {
                                eligible_when: json!(false),
                                ..Default::default()
                            }),
                        );
                }
                _ => {
                    let model = provider.offerings.get_mut(&reference.offering).unwrap();
                    match case {
                        "absent_modes" => model.parameters = json!({}),
                        "disabled_modes" => model.parameters = json!({"web_search":[]}),
                        "explicit_disabled" => {
                            model.capabilities.insert(
                                "web_search".into(),
                                cap::Support {
                                    enabled: false,
                                    modes: vec!["live".into()],
                                    ..Default::default()
                                },
                            );
                        }
                        "offering_tool_null" => {
                            model.native_tools.insert("web_search".into(), None);
                        }
                        "offering_tool_refused" => {
                            model.native_tools.insert(
                                "web_search".into(),
                                Some(cap::Binding {
                                    eligible_when: json!(false),
                                    ..Default::default()
                                }),
                            );
                        }
                        _ => unreachable!("the fixture case was enumerated"),
                    }
                }
            }
        });
        let refused = search(&registry, &context);
        assert!(
            refused.route.is_none(),
            "{case}: rejected model search cannot become a program fallback"
        );
        assert!(!refused.modes.iter().any(|mode| mode == "live"), "{case}");
    }
    for protocol in ["openai-chat", "openai-responses"] {
        let (registry, context, _) = fixture(protocol);
        assert!(
            search(&registry, &context).route.is_none(),
            "the native auxiliary speaks Messages, not {protocol}"
        );
    }
    for access in ["native", "shared", "handover"] {
        let (registry, mut context, _) = fixture("anthropic-messages");
        context.access = access.into();
        assert!(
            search(&registry, &context).route.is_none(),
            "Forwarded facts cannot authorize {access}"
        );
    }
}

#[test]
fn native_search_still_needs_current_use_an_enabled_offering_and_its_credential_role() {
    for case in ["use_lost", "credential_missing", "offering_disabled"] {
        let (mut registry, mut context, reference) = fixture("anthropic-messages");
        match case {
            "use_lost" => {
                context
                    .availability
                    .get_mut(&reference.provider)
                    .unwrap()
                    .get_mut(&reference.offering)
                    .unwrap()
                    .allowed = false
            }
            "credential_missing" => {
                context.credential_roles.remove(&reference.provider);
            }
            "offering_disabled" => edit(&mut registry, |source| {
                source
                    .providers
                    .get_mut(&reference.provider)
                    .unwrap()
                    .offerings
                    .get_mut(&reference.offering)
                    .unwrap()
                    .enabled = false;
            }),
            _ => unreachable!("the fixture case was enumerated"),
        }
        assert!(search(&registry, &context).route.is_none(), "{case}");
    }
}

#[test]
fn service_uses_its_exact_account_only_after_explicit_choice_and_never_falls_back_to_native_search()
{
    let (mut registry, mut context, _) = fixture("anthropic-messages");
    assert_managed_launch(
        &[search(&registry, &context), page_fetch(&registry, &context)],
        true,
        true,
    );
    let backend = cap::OfferingRef {
        provider: new_id(),
        offering: "fixture-search".into(),
    };
    edit(&mut registry, |source| {
        source.interfaces.insert(
            "fixture-search-api".into(),
            cap::Interface {
                kind: "search".into(),
                implementation: "fixture-search-api".into(),
                ..Default::default()
            },
        );
        source.providers.insert(backend.provider, serde_json::from_value(json!({
            "title":"Independent search fixture","credential_roles":["model"],
            "endpoints":{"search":{"interface":"fixture-search-api","address":"https://search.example.invalid","credential_role":"model"}},
            "offerings":{"fixture-search":{"endpoint":"search","kind":"service","enabled":true,
                "capabilities":{"web_search":{"enabled":true,"modes":["service"]}}
            }}
        })).unwrap());
    });
    context.interfaces.push("fixture-search-api".into());
    // Installed-executor/current-authority facts below are resolver fixtures, not an IO installation or admission claim.
    context.executors.push("api-backed".into());
    context
        .credential_roles
        .insert(backend.provider, vec!["model".into()]);
    context
        .availability
        .entry(backend.provider)
        .or_default()
        .insert(
            backend.offering.clone(),
            cap::Availability {
                allowed: true,
                show_provider: true,
                show_offering: true,
                ..Default::default()
            },
        );
    context.backends.insert(
        "web_search".into(),
        cap::BackendRef {
            route: "web_search.service-api".into(),
            offering: Some(backend.clone()),
            ..Default::default()
        },
    );
    context.choices.remove("web_search");
    let default = search(&registry, &context);
    assert!(default.program_default);
    assert_eq!(default.mode.as_deref(), Some("disabled"));
    assert!(default.native_config.is_empty());
    assert_managed_launch(
        &[default.clone(), page_fetch(&registry, &context)],
        false,
        true,
    );
    assert!(
        default.route.is_none(),
        "selecting a backend alone cannot select a paid mode"
    );
    context
        .choices
        .insert("web_search".into(), Some("service".into()));
    let service = search(&registry, &context);
    let route = service.route.as_ref().unwrap();
    assert_eq!(route.offering.as_ref(), Some(&backend));
    assert_eq!(route.kind, "service");
    assert!(route.disable_native);
    assert_eq!(route.native_value, json!("disabled"));
    assert_eq!(
        service.native_config.get("web_search"),
        Some(&json!("disabled"))
    );
    assert_eq!(route.orch_tool.as_deref(), Some("web_search"));
    assert_eq!(service.provider.as_ref().unwrap().id, backend.provider);
    let reader = page_fetch(&registry, &context);
    assert_managed_launch(&[service.clone(), reader.clone()], false, true);
    assert_eq!(
        reader.route.as_ref().unwrap().kind,
        "native-program",
        "web-search Service alone preserves the independent HTTP reader"
    );
    assert!(
        reader.native_config.is_empty(),
        "an unset reader retains its On program default"
    );
    let fetch = cap::OfferingRef {
        provider: backend.provider,
        offering: "fixture-fetch".into(),
    };
    edit(&mut registry, |source| {
        source.interfaces.insert(
            "fixture-fetch-api".into(),
            cap::Interface {
                kind: "fetch".into(),
                implementation: "fixture-fetch-api".into(),
                ..Default::default()
            },
        );
        let provider = source.providers.get_mut(&fetch.provider).unwrap();
        provider.endpoints.insert(
            "fetch".into(),
            cap::Endpoint {
                interface: "fixture-fetch-api".into(),
                address: "https://fetch.example.invalid".into(),
                credential_role: Some("model".into()),
                ..Default::default()
            },
        );
        provider.offerings.insert(
            fetch.offering.clone(),
            cap::Offering {
                endpoint: "fetch".into(),
                kind: "service".into(),
                enabled: true,
                capabilities: [(
                    "page_fetch".into(),
                    cap::Support {
                        enabled: true,
                        modes: vec!["service".into()],
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            },
        );
    });
    context.interfaces.push("fixture-fetch-api".into());
    context
        .availability
        .entry(fetch.provider)
        .or_default()
        .insert(
            fetch.offering.clone(),
            cap::Availability {
                allowed: true,
                ..Default::default()
            },
        );
    context.backends.insert(
        "page_fetch".into(),
        cap::BackendRef {
            route: "page_fetch.service-api".into(),
            offering: Some(fetch),
            ..Default::default()
        },
    );
    context
        .choices
        .insert("page_fetch".into(), Some("service".into()));
    let reader = page_fetch(&registry, &context);
    assert_eq!(reader.route.as_ref().unwrap().kind, "service");
    assert_eq!(
        reader.route.as_ref().unwrap().orch_tool.as_deref(),
        Some("web_fetch")
    );
    assert!(reader.route.as_ref().unwrap().disable_native);
    assert_eq!(reader.native_config.get("page_fetch"), Some(&json!(false)));
    assert_managed_launch(&[service.clone(), reader.clone()], false, false);
    for case in [
        "backend_use_lost",
        "backend_missing",
        "backend_credential_missing",
        "executor_missing",
        "tools_missing",
    ] {
        let mut registry = registry.clone();
        let mut selected = context.clone();
        match case {
            "backend_use_lost" => {
                selected
                    .availability
                    .get_mut(&backend.provider)
                    .unwrap()
                    .get_mut(&backend.offering)
                    .unwrap()
                    .allowed = false
            }
            "backend_missing" => {
                selected.backends.clear();
            }
            "backend_credential_missing" => {
                selected.credential_roles.remove(&backend.provider);
            }
            "executor_missing" => {
                selected.executors.clear();
            }
            "tools_missing" => edit(&mut registry, |source| {
                source
                    .programs
                    .get_mut(&selected.program)
                    .unwrap()
                    .access
                    .get_mut("forwarded")
                    .unwrap()
                    .orch_tools = false;
            }),
            _ => unreachable!("the fixture case was enumerated"),
        }
        let refused = search(&registry, &selected);
        assert_eq!(
            refused.mode.as_deref(),
            Some("service"),
            "{case}: retain the chosen intent"
        );
        assert!(
            refused.route.is_none(),
            "{case}: generation account authority is no service authority or native fallback"
        );
        assert!(
            !refused.modes.iter().any(|mode| mode == "service"),
            "{case}"
        );
        assert_managed_launch(&[refused, page_fetch(&registry, &selected)], false, false);
    }
}
