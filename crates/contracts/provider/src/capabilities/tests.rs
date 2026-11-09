#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
use super::*;
use orch_types::new_id;

fn fixture() -> (Registry, ThreadContext, OfferingRef) {
    let provider = new_id();
    let reference = OfferingRef {
        provider,
        offering: "model".into(),
    };
    let mut registry = Registry::default();
    let declarations:Declarations=serde_json::from_value(json!({
        "contract":1,
        "interfaces":{"fixture-generation":{"kind":"generation","implementation":"fixture-wire","native_tools":{"lookup":{"request":{"tool":"server-lookup"}}}}},
        "providers":{(provider.to_string()):{"title":"Team account","credential_roles":["model"],"endpoints":{"main":{"interface":"fixture-generation","address":"https://provider.example.invalid/v1","credential_role":"model"}},
            "offerings":{"model":{"endpoint":"main","kind":"model","enabled":true,"parameters":{"context_window":1000,"future_model_field":{"kept":true}},
                "capabilities":{"lookup":{"enabled":true,"modes":["live"]}},"prices":{"search":{"amount":"0.01","currency":"USD","unit":"search","future_private":"never-in-view"}}}}}},
        "capabilities":{"lookup":{"title":"Lookup","setting":"agent.params.lookup","item":"web_search","ledger_unit":"search","modes":{
            "disabled":{"routes":[]},"live":{"routes":["native"],"native_value":"live"},"service":{"routes":["auxiliary"],"native_value":"disabled"}}}},
        "programs":{"fixture-program":{"access":{"forwarded":{"interfaces":["fixture-generation"],"native":{"lookup":{"enabled":true,"modes":["live"],"default_mode":"live"}},"orch_tools":true},
            "subscription":{"native":{"lookup":{"enabled":false}},"orch_tools":true}}}},
        "routes":{"native":{"kind":"native-model","capabilities":["lookup"],"requires_program_support":true,"requires_generation_interface":true,"requires_offering_support":true,"requires_native_tool":true},
            "auxiliary":{"kind":"service","executor":"model-backed","capabilities":["lookup"],"offering_mode":"live","requires_offering_support":true,"requires_native_tool":true,"uses_backend":true,"requires_opt_in":true,"requires_orch_tools":true,"orch_tool":"lookup","disable_native":true}}
    })).unwrap();
    registry.register("fixture", declarations).unwrap();
    let mut context = ThreadContext {
        program: "fixture-program".into(),
        access: "forwarded".into(),
        current: Some(reference.clone()),
        executors: vec!["model-backed".into()],
        interfaces: vec!["fixture-generation".into()],
        ..ThreadContext::default()
    };
    context.availability.entry(provider).or_default().insert(
        "model".into(),
        Availability {
            allowed: true,
            show_provider: true,
            show_offering: true,
            show_price: true,
            ..Availability::default()
        },
    );
    context
        .credential_roles
        .insert(provider, vec!["model".into()]);
    (registry, context, reference)
}

#[test]
fn one_thread_resolution_keeps_native_defaults_and_requires_an_exact_explicit_paid_backend() {
    let (registry, mut context, reference) = fixture();
    let native = registry.resolve(&context);
    assert_eq!(native[0].route.as_ref().unwrap().kind, "native-model");
    assert!(native[0].program_default);
    assert_eq!(native[0].route.as_ref().unwrap().native_value, Json::Null);
    assert!(native[0].route.as_ref().unwrap().orch_tool.is_none());
    assert!(!native[0].modes.contains(&"service".into()));
    context.backends.insert(
        "lookup".into(),
        BackendRef {
            route: "auxiliary".into(),
            offering: Some(reference),
            config: json!({"private":"not-for-inspector"}),
            ..BackendRef::default()
        },
    );
    assert_eq!(
        registry.resolve(&context)[0].route.as_ref().unwrap().kind,
        "native-model",
        "a configured paid backend is not an opt-in"
    );
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    let service = registry.resolve(&context);
    let route = service[0].route.as_ref().unwrap();
    assert!(route.disable_native);
    assert_eq!(route.orch_tool.as_deref(), Some("lookup"));
    assert_eq!(route.kind, "service");
    assert_eq!(route.native_value, json!("disabled"));
    let view = serde_json::to_string(&service).unwrap();
    assert!(
        !view.contains("not-for-inspector")
            && !view.contains("future_private")
            && !view.contains("https://provider")
    );
    context.backends.clear();
    let refused = registry.resolve(&context);
    assert!(refused[0].route.is_none());
    assert_eq!(refused[0].reason.as_deref(), Some("backend_not_selected"));
}

#[test]
fn current_authority_credentials_offering_enable_and_access_kind_are_independent_facts() {
    let (mut registry, mut context, reference) = fixture();
    context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap()
        .allowed = false;
    assert_eq!(
        registry.resolve(&context)[0].reason.as_deref(),
        Some("current_authority_refused")
    );
    context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap()
        .allowed = true;
    context.credential_roles.clear();
    assert_eq!(
        registry.resolve(&context)[0].reason.as_deref(),
        Some("credential_role_unavailable")
    );
    context
        .credential_roles
        .insert(reference.provider, vec!["model".into()]);
    context.access = "subscription".into();
    assert!(
        registry.resolve(&context)[0].route.is_none(),
        "forwarded support never becomes subscription support"
    );
    context.access = "forwarded".into();
    let mut changed = registry.extension("fixture").unwrap().clone();
    changed
        .providers
        .get_mut(&reference.provider)
        .unwrap()
        .offerings
        .get_mut("model")
        .unwrap()
        .enabled = false;
    registry.register("fixture", changed).unwrap();
    assert_eq!(
        registry.resolve(&context)[0].reason.as_deref(),
        Some("offering_not_enabled")
    );
}

#[test]
fn program_explanations_use_actual_access_and_arrangement_without_changing_a_route() {
    let (mut registry, mut context, _) = fixture();
    let baseline = registry.resolve(&context);
    let mut declaration = registry.extension("fixture").unwrap().clone();
    let program = declaration.programs.get_mut("fixture-program").unwrap();
    program.facts = serde_json::from_value(json!([
        {"id":"elsewhere","capabilities":["other-capability"],"text":{"en":"Not this capability"}},
        {"id":"location","capabilities":["lookup"],"when":{"required":["arrangement"],"properties":{"arrangement":{"const":"separated"}}},
         "text":{"en":"Runs on the provider runtime","zh-CN":"在提供方运行时执行"},"future_member":"not-for-view"}
    ])).unwrap();
    program.access.get_mut("forwarded").unwrap().facts = serde_json::from_value(json!([
        {"id":"forwarded-detail","capabilities":["lookup"],"text":{"en":"The program reports this access limitation","zh-CN":"程序报告此访问方式的限制"}},
        {"id":"future-empty","unknown":"not-for-view"}
    ])).unwrap();
    registry.register("fixture", declaration).unwrap();
    let mut displayed = registry.resolve(&context);
    assert_eq!(
        displayed[0]
            .facts
            .iter()
            .map(|fact| fact.id.as_str())
            .collect::<Vec<_>>(),
        ["forwarded-detail"]
    );
    displayed[0].facts.clear();
    assert_eq!(
        displayed, baseline,
        "explanations do not resolve, enable, price or authorize anything"
    );
    context.facts = json!({"arrangement":"separated"});
    let located = registry.resolve(&context);
    assert_eq!(
        located[0]
            .facts
            .iter()
            .map(|fact| fact.id.as_str())
            .collect::<Vec<_>>(),
        ["location", "forwarded-detail"]
    );
    assert_eq!(registry.matrix([&context])[0].resolution, located[0]);
    let view = serde_json::to_string(&located).unwrap();
    assert!(!view.contains("not-for-view"));
    assert!(!view.contains("Not this capability"));
    assert_eq!(
        serde_json::to_value(registry.extension("fixture").unwrap()).unwrap()["programs"]["fixture-program"]
            ["facts"][1]["future_member"],
        "not-for-view"
    );
    context.access = "subscription".into();
    let subscription = registry.resolve(&context);
    assert_eq!(
        subscription[0]
            .facts
            .iter()
            .map(|fact| fact.id.as_str())
            .collect::<Vec<_>>(),
        ["location"]
    );
    assert!(subscription[0].route.is_none());
}

#[test]
fn unreadable_provider_and_price_facts_stay_out_of_the_inspector_even_when_use_is_allowed() {
    let (registry, mut context, reference) = fixture();
    let state = context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut("model")
        .unwrap();
    state.show_provider = false;
    state.show_offering = false;
    state.show_price = false;
    let result = registry.resolve(&context);
    assert!(result[0].route.is_some());
    assert!(result[0].provider.is_none());
    assert!(result[0].offering.is_none());
    assert!(result[0].prices.is_empty());
    let cells = registry.matrix([&context]);
    assert!(cells[0].current.is_none());
    assert!(
        !serde_json::to_string(&cells)
            .unwrap()
            .contains(&reference.provider.to_string())
    );
    let view = serde_json::to_string(&result).unwrap();
    assert!(
        !view.contains("Team account")
            && !view.contains(&reference.provider.to_string())
            && !view.contains("0.01")
    );
}

#[test]
fn unknown_declaration_members_round_trip_and_a_third_route_needs_no_backend_enum_change() {
    let raw = json!({"contract":9,"future_contract":{"x":true},"interfaces":{"new-shape":{"kind":"later-service","future_interface":7}},
        "providers":{},"capabilities":{},"programs":{},"routes":{"delegated":{"kind":"program-session","executor":"future-executor","future_route":{"kept":1}}}});
    let declaration: Declarations = serde_json::from_value(raw.clone()).unwrap();
    let written = serde_json::to_value(&declaration).unwrap();
    assert_eq!(written["future_contract"], raw["future_contract"]);
    assert_eq!(written["interfaces"]["new-shape"]["future_interface"], 7);
    assert_eq!(
        written["routes"]["delegated"]["future_route"],
        json!({"kept":1})
    );
    let mut registry = Registry::default();
    registry.register("later", declaration).unwrap();
    assert_eq!(
        registry.declarations().routes["delegated"].kind,
        "program-session"
    );
}

#[test]
fn adding_and_removing_a_fake_provider_leaves_existing_resolution_and_stored_parameters_identical()
{
    let (mut registry, context, reference) = fixture();
    let original = serde_json::to_vec(&registry.resolve(&context)).unwrap();
    let model = serde_json::to_vec(
        &registry.declarations().providers[&reference.provider].offerings["model"].parameters,
    )
    .unwrap();
    let mut extra = Declarations::default();
    extra.interfaces.insert(
        "extra-shape".into(),
        Interface {
            kind: "future".into(),
            ..Interface::default()
        },
    );
    extra.providers.insert(
        new_id(),
        Provider {
            title: "Another account".into(),
            ..Provider::default()
        },
    );
    registry.register("extra", extra).unwrap();
    assert_eq!(
        serde_json::to_vec(&registry.resolve(&context)).unwrap(),
        original
    );
    assert_eq!(
        serde_json::to_vec(
            &registry.declarations().providers[&reference.provider].offerings["model"].parameters
        )
        .unwrap(),
        model
    );
    assert_eq!(
        registry.matrix([&context])[0].resolution,
        registry.resolve(&context)[0]
    );
    assert!(registry.remove("extra"));
    assert_eq!(
        serde_json::to_vec(&registry.resolve(&context)).unwrap(),
        original
    );
}

#[test]
fn a_stored_model_written_before_capability_members_existed_is_projected_without_a_rewrite() {
    let (mut registry, context, reference) = fixture();
    let mut declaration = registry.extension("fixture").unwrap().clone();
    let old = serde_json::json!({"web_search":["live"],"default_web_search":"live","future_model_field":{"still":"here"}});
    let model = declaration
        .providers
        .get_mut(&reference.provider)
        .unwrap()
        .offerings
        .get_mut("model")
        .unwrap();
    model.capabilities.clear();
    model.parameters = old.clone();
    declaration
        .capabilities
        .get_mut("lookup")
        .unwrap()
        .offering_support = Some(SupportProjection {
        modes: Some("/parameters/web_search".into()),
        default_mode: Some("/parameters/default_web_search".into()),
        ..SupportProjection::default()
    });
    registry.register("fixture", declaration).unwrap();
    assert!(registry.resolve(&context)[0].route.is_some());
    assert_eq!(
        registry.declarations().providers[&reference.provider].offerings["model"].parameters,
        old
    );
}

#[test]
fn a_paid_route_needs_an_exact_authorized_offering_and_its_installed_interface() {
    let (mut registry, mut context, reference) = fixture();
    let mut declarations = registry.extension("fixture").unwrap().clone();
    let route = declarations.routes.get_mut("auxiliary").unwrap();
    route.requires_interface = true;
    route.requires_offering = true;
    registry.register("fixture", declarations).unwrap();
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    context.backends.insert(
        "lookup".into(),
        BackendRef {
            route: "auxiliary".into(),
            offering: Some(reference),
            ..BackendRef::default()
        },
    );
    context.interfaces.clear();
    assert_eq!(
        registry.resolve(&context)[0].reason.as_deref(),
        Some("interface_not_installed")
    );
    context.interfaces.push("fixture-generation".into());
    assert!(registry.resolve(&context)[0].route.is_some());
    context.backends.get_mut("lookup").unwrap().offering = None;
    assert_eq!(
        registry.resolve(&context)[0].reason.as_deref(),
        Some("offering_not_selected")
    );
}

#[test]
fn a_new_backend_joins_an_existing_mode_by_declaration_and_removes_without_residue() {
    let (mut registry, mut context, reference) = fixture();
    let before = serde_json::to_vec(&registry.resolve(&context)).unwrap();
    let mut addition = Declarations::default();
    addition.routes.insert(
        "future-lookup".into(),
        Route {
            kind: "future-kind".into(),
            executor: Some("future-executor".into()),
            capabilities: vec!["lookup".into()],
            uses_backend: true,
            requires_offering: true,
            requires_orch_tools: true,
            disable_native: true,
            orch_tool: Some("lookup".into()),
            ..Route::default()
        },
    );
    addition.route_links.insert(
        "lookup".into(),
        BTreeMap::from([("service".into(), vec!["future-lookup".into()])]),
    );
    registry.register("future", addition).unwrap();
    assert_eq!(
        serde_json::to_vec(&registry.resolve(&context)).unwrap(),
        before
    );
    context.executors.push("future-executor".into());
    context.backends.insert(
        "lookup".into(),
        BackendRef {
            route: "future-lookup".into(),
            offering: Some(reference),
            ..BackendRef::default()
        },
    );
    assert_eq!(
        registry.resolve(&context)[0].route.as_ref().unwrap().kind,
        "native-model",
        "a configured new backend never implicitly spends"
    );
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    assert_eq!(
        registry.resolve(&context)[0].route.as_ref().unwrap().kind,
        "future-kind"
    );
    assert!(registry.remove("future"));
    context.choices.clear();
    context.backends.clear();
    assert_eq!(
        serde_json::to_vec(&registry.resolve(&context)).unwrap(),
        before
    );
    assert!(!registry.declarations().routes.contains_key("future-lookup"));
}

#[test]
fn fallback_previews_use_call_rules_without_selecting_a_service_or_exposing_private_configuration()
{
    let (mut registry, mut context, reference) = fixture();
    let mut declaration = registry.extension("fixture").unwrap().clone();
    declaration
        .capabilities
        .get_mut("lookup")
        .unwrap()
        .recovery_purpose = Some("web_search".into());
    registry.register("fixture", declaration).unwrap();
    let candidate = json!({"provider_id":reference.provider,"model":"","route":{"route":"auxiliary","offering":reference,"config":{"private":"never-publish"}},"params":{"private":"never-publish"}});
    context.recovery=Some(RecoveryPolicy { scope:"org".into(),rules:crate::calls::validate(&json!([{"id":"lookup-fallback","when":{"kind":"unavailable","purpose":"web_search"},"steps":[{"type":"failover","candidate":candidate}]}])).unwrap() });
    let rows = registry.resolve(&context);
    assert_eq!(rows[0].route.as_ref().unwrap().kind, "native-model");
    assert_eq!(rows[0].fallbacks.len(), 1);
    assert_eq!(rows[0].fallbacks[0].rule_id, "lookup-fallback");
    assert_eq!(rows[0].fallbacks[0].scope, "org");
    assert!(rows[0].fallbacks[0].provider.is_some());
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("never-publish")
    );
    let state = context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap();
    state.show_provider = false;
    state.show_offering = false;
    state.show_price = false;
    let rows = registry.resolve(&context);
    assert!(rows[0].fallbacks[0].provider.is_none() && rows[0].fallbacks[0].offering.is_none());
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains(&reference.provider.to_string())
    );
}

#[test]
fn native_program_support_is_decided_by_each_declared_access_kind_and_current_use() {
    let (mut registry, mut context, reference) = fixture();
    registry
        .register("common", crate::capabilities::catalogue().unwrap())
        .unwrap();
    let mut program = registry.extension("fixture").unwrap().clone();
    for access in ["forwarded", "future-access"] {
        program
            .programs
            .get_mut("fixture-program")
            .unwrap()
            .access
            .entry(access.into())
            .or_default()
            .native
            .insert(
                "page_fetch".into(),
                Support {
                    enabled: true,
                    modes: vec!["on".into()],
                    default_mode: Some("on".into()),
                    ..Support::default()
                },
            );
    }
    registry.register("fixture", program.clone()).unwrap();
    context
        .choices
        .insert("page_fetch".into(), Some("on".into()));
    for access in ["forwarded", "future-access"] {
        context.access = access.into();
        let resolved = registry.resolve(&context);
        let page = resolved
            .iter()
            .find(|capability| capability.capability == "page_fetch")
            .unwrap();
        assert_eq!(
            page.route.as_ref().map(|route| route.id.as_str()),
            Some("page_fetch.program")
        );
    }
    context.access = "forwarded".into();
    program
        .programs
        .get_mut("fixture-program")
        .unwrap()
        .access
        .get_mut("forwarded")
        .unwrap()
        .native
        .get_mut("page_fetch")
        .unwrap()
        .enabled = false;
    registry.register("fixture", program.clone()).unwrap();
    assert!(
        registry
            .resolve(&context)
            .iter()
            .find(|capability| capability.capability == "page_fetch")
            .unwrap()
            .route
            .is_none(),
        "the access declaration still closes unsupported tools"
    );
    program
        .programs
        .get_mut("fixture-program")
        .unwrap()
        .access
        .get_mut("forwarded")
        .unwrap()
        .native
        .get_mut("page_fetch")
        .unwrap()
        .enabled = true;
    registry.register("fixture", program).unwrap();
    context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap()
        .allowed = false;
    assert!(
        registry
            .resolve(&context)
            .iter()
            .find(|capability| capability.capability == "page_fetch")
            .unwrap()
            .route
            .is_none(),
        "native program support never bypasses current Use"
    );
}

#[test]
fn carried_model_support_cannot_turn_into_program_support_when_the_model_tool_is_absent() {
    let (mut registry, mut context, reference) = fixture();
    registry
        .register("common", crate::capabilities::catalogue().unwrap())
        .unwrap();
    let mut declarations = registry.extension("fixture").unwrap().clone();
    let access = declarations
        .programs
        .get_mut("fixture-program")
        .unwrap()
        .access
        .get_mut("forwarded")
        .unwrap();
    access.native.insert("web_search".into(), Support { enabled:true, modes:vec!["live".into()], default_mode:Some("live".into()), eligible_when:json!({"required":["route_kind"],"properties":{"route_kind":{"const":"native-model"}}}), ..Support::default() });
    access.native.insert(
        "page_fetch".into(),
        Support {
            enabled: true,
            modes: vec!["on".into()],
            default_mode: Some("on".into()),
            ..Support::default()
        },
    );
    access.native.insert(
        "image_input".into(),
        Support {
            enabled: true,
            modes: vec!["native".into()],
            default_mode: Some("native".into()),
            ..Support::default()
        },
    );
    let model = declarations
        .providers
        .get_mut(&reference.provider)
        .unwrap()
        .offerings
        .get_mut(&reference.offering)
        .unwrap();
    model.parameters["web_search"] = json!(["live"]);
    model.parameters["input_images"] = json!(false);
    model.native_tools.insert("web_search".into(), None);
    registry.register("fixture", declarations.clone()).unwrap();
    let resolved = registry.resolve(&context);
    assert!(
        resolved
            .iter()
            .find(|capability| capability.capability == "image_input")
            .unwrap()
            .route
            .is_none(),
        "a program cannot reopen a model's explicit image-input prohibition"
    );
    assert!(
        resolved
            .iter()
            .find(|capability| capability.capability == "web_search")
            .unwrap()
            .route
            .is_none(),
        "an unavailable model tool has no invented program fallback"
    );
    assert_eq!(
        resolved
            .iter()
            .find(|capability| capability.capability == "page_fetch")
            .unwrap()
            .route
            .as_ref()
            .unwrap()
            .kind,
        "native-program",
        "the same forwarded program keeps its own page reader"
    );
    declarations
        .providers
        .get_mut(&reference.provider)
        .unwrap()
        .offerings
        .get_mut(&reference.offering)
        .unwrap()
        .native_tools
        .insert("web_search".into(), Some(Binding::default()));
    registry.register("fixture", declarations).unwrap();
    context
        .choices
        .insert("web_search".into(), Some("live".into()));
    assert_eq!(
        registry
            .resolve(&context)
            .iter()
            .find(|capability| capability.capability == "web_search")
            .unwrap()
            .route
            .as_ref()
            .unwrap()
            .kind,
        "native-model"
    );
}

#[test]
fn a_native_program_requiring_a_nonfree_plan_does_not_treat_an_unknown_plan_as_paid() {
    let (mut registry, mut context, _) = fixture();
    registry
        .register("common", crate::capabilities::catalogue().unwrap())
        .unwrap();
    let mut declarations = registry.extension("fixture").unwrap().clone();
    let program: Program = serde_json::from_str(include_str!(
        "../../../../../extensions/agents/codex/src/capability-program.json"
    ))
    .unwrap();
    declarations
        .programs
        .insert("fixture-program".into(), program);
    registry.register("fixture", declarations).unwrap();
    context.access = "native".into();
    for plan in [json!(null), json!(""), json!("free")] {
        context.facts =
            json!({"plan":plan,"credential":{"yields":"login"},"credential_verified":true});
        assert!(
            registry
                .resolve(&context)
                .iter()
                .find(|capability| capability.capability == "image_generation")
                .unwrap()
                .route
                .is_none()
        );
    }
    context.facts =
        json!({"plan":"pro","credential":{"yields":"login"},"credential_verified":false});
    assert!(
        registry
            .resolve(&context)
            .iter()
            .find(|capability| capability.capability == "image_generation")
            .unwrap()
            .route
            .is_none()
    );
    context.facts =
        json!({"plan":"pro","credential":{"yields":"login"},"credential_verified":true});
    assert_eq!(
        registry
            .resolve(&context)
            .iter()
            .find(|capability| capability.capability == "image_generation")
            .unwrap()
            .route
            .as_ref()
            .unwrap()
            .kind,
        "native-program"
    );
}

#[test]
fn serialized_native_configuration_keeps_only_declared_program_flags_from_the_resolved_value() {
    let (mut registry, mut context, reference) = fixture();
    let mut declarations = registry.extension("fixture").unwrap().clone();
    declarations
        .programs
        .get_mut("fixture-program")
        .unwrap()
        .native_switches = serde_json::from_value(json!({
        "native_lookup":{"capability":"lookup","values":{"live":"live","disabled":"disabled"}},
        "tools.picture":{"capability":"picture","values":{"on":true,"off":false}}
    }))
    .unwrap();
    registry.register("fixture", declarations.clone()).unwrap();
    assert!(
        registry.resolve(&context)[0].native_config.is_empty(),
        "unset retains the native program default"
    );
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    context.backends.insert("lookup".into(),BackendRef {route:"auxiliary".into(),offering:Some(reference.clone()),config:json!({"private_key":"do-not-publish","endpoint":"https://private.example","unreadable_id":reference.provider}),..Default::default()});
    let visibility = context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap();
    visibility.show_provider = false;
    visibility.show_offering = false;
    let resolved = registry.resolve(&context);
    assert_eq!(
        resolved[0].native_config,
        BTreeMap::from([("native_lookup".into(), json!("disabled"))])
    );
    let wire = serde_json::to_string(&resolved).unwrap();
    assert!(
        !wire.contains("do-not-publish")
            && !wire.contains("https://private")
            && !wire.contains("private_key")
    );
    let roundtrip: Vec<Resolution> = serde_json::from_str(&wire).unwrap();
    assert!(roundtrip[0].route.is_none());
    assert_eq!(roundtrip[0].native_config, resolved[0].native_config);
    declarations
        .capabilities
        .get_mut("lookup")
        .unwrap()
        .modes
        .get_mut("service")
        .unwrap()
        .native_value = json!({"native_lookup":"disabled","tools.picture":true,"key":"do-not-publish","endpoint":"https://private.example"});
    registry.register("fixture", declarations).unwrap();
    assert_eq!(
        registry.resolve(&context)[0].native_config,
        BTreeMap::from([("native_lookup".into(), json!("disabled"))])
    );
    context
        .choices
        .insert("lookup".into(), Some("disabled".into()));
    assert!(
        registry.resolve(&context)[0].native_config.is_empty(),
        "an undeclared disabled value is not inferred"
    );
}

#[test]
fn an_explicit_off_flag_survives_the_carrier_without_an_executable_or_readable_provider_route() {
    let (mut registry, mut context, reference) = fixture();
    let mut declarations = registry.extension("fixture").unwrap().clone();
    declarations
        .programs
        .get_mut("fixture-program")
        .unwrap()
        .native_switches = serde_json::from_value(json!({
        "native_lookup":{"capability":"lookup","values":{"disabled":false}}
    }))
    .unwrap();
    declarations
        .capabilities
        .get_mut("lookup")
        .unwrap()
        .modes
        .get_mut("disabled")
        .unwrap()
        .native_value = json!("disabled");
    registry.register("fixture", declarations).unwrap();
    context
        .choices
        .insert("lookup".into(), Some("disabled".into()));
    let permission = context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap();
    permission.allowed = false;
    permission.show_provider = false;
    permission.show_offering = false;
    let resolved = registry.resolve(&context);
    assert_eq!(resolved[0].reason.as_deref(), Some("disabled"));
    assert!(resolved[0].route.is_none() && resolved[0].route_view.is_none());
    assert!(resolved[0].provider.is_none() && resolved[0].offering.is_none());
    let wire = serde_json::to_value(&resolved).unwrap();
    assert_eq!(wire[0]["native_config"], json!({"native_lookup":false}));
    assert!(wire[0].get("route").is_none());
    let carried: Vec<Resolution> = serde_json::from_value(wire).unwrap();
    assert_eq!(
        carried[0].native_config,
        BTreeMap::from([("native_lookup".into(), json!(false))])
    );
    assert!(carried[0].route.is_none());
}

#[test]
fn route_dependent_native_flags_keep_admission_and_native_defaults_distinct() {
    let (mut registry, mut context, reference) = fixture();
    let mut declarations = registry.extension("fixture").unwrap().clone();
    declarations.programs.get_mut("fixture-program").unwrap().native_switches = serde_json::from_value(json!({
        "native_lookup":{"capability":"lookup","value":false,"resolved_value":true,
            "when":{"required":["route_kind"],"properties":{"route_kind":{"const":"native-model"}}}},
        "lookup_mode":{"capability":"lookup","values":{"live":"live","disabled":"disabled"}}
    })).unwrap();
    registry.register("fixture", declarations).unwrap();

    let implicit = &registry.resolve(&context)[0];
    assert!(implicit.route.as_ref().unwrap().native_value.is_null());
    assert_eq!(implicit.native_config["native_lookup"], true);
    assert!(
        !implicit.native_config.contains_key("lookup_mode"),
        "a new admitted-route flag must not force existing native mode defaults"
    );

    context.choices.insert("lookup".into(), Some("live".into()));
    let explicit = &registry.resolve(&context)[0];
    assert_eq!(explicit.native_config["native_lookup"], true);
    assert_eq!(explicit.native_config["lookup_mode"], "live");

    context
        .choices
        .insert("lookup".into(), Some("disabled".into()));
    let off = &registry.resolve(&context)[0];
    assert!(off.route.is_none());
    assert_eq!(off.native_config["native_lookup"], false);

    context.choices.clear();
    context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap()
        .allowed = false;
    let refused = &registry.resolve(&context)[0];
    assert!(refused.route.is_none());
    assert_eq!(refused.native_config["native_lookup"], false);

    context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap()
        .allowed = true;
    context.backends.insert(
        "lookup".into(),
        BackendRef {
            route: "auxiliary".into(),
            offering: Some(reference),
            ..BackendRef::default()
        },
    );
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    let service = &registry.resolve(&context)[0];
    assert_eq!(service.route.as_ref().unwrap().kind, "service");
    assert_eq!(service.native_config["native_lookup"], false);
    assert_eq!(service.native_config["lookup_mode"], "disabled");
}

#[test]
fn opaque_native_values_cannot_be_published_as_nested_configuration() {
    let program: Program = serde_json::from_value(json!({"native_switches":{
        "native_lookup":{"capability":"lookup","resolved_value":{"key":"private-value"},"values":{"disabled":{"endpoint":"https://private.example","key":"private-value"}}},
        "other_switch":{"capability":"other","values":{"disabled":true}}
    }})).unwrap();
    assert!(
        native_configuration(
            Some(&program),
            "lookup",
            &json!("disabled"),
            &json!({}),
            true,
            None
        )
        .is_empty()
    );
    assert!(
        native_configuration(
            Some(&program),
            "lookup",
            &json!({"native_lookup":{"endpoint":"https://private.example","key":"private-value"}}),
            &json!({}),
            true,
            None
        )
        .is_empty()
    );
}

#[test]
fn actual_codex_native_switches_publish_only_its_web_mode_and_boolean_image_flag() {
    let program: Program = serde_json::from_str(include_str!(
        "../../../../../extensions/agents/codex/src/capability-program.json"
    ))
    .unwrap();
    assert_eq!(
        native_configuration(
            Some(&program),
            "web_search",
            &json!("disabled"),
            &json!({}),
            false,
            None
        ),
        BTreeMap::from([("web_search".into(), json!("disabled"))])
    );
    assert_eq!(
        native_configuration(
            Some(&program),
            "image_generation",
            &json!("off"),
            &json!({}),
            false,
            None
        ),
        BTreeMap::from([("features.image_generation".into(), json!(false))])
    );
    assert!(
        native_configuration(
            Some(&program),
            "web_search",
            &json!({"web_search":"private-key","backend":{"key":"private-key"}}),
            &json!({}),
            false,
            None
        )
        .is_empty()
    );
}

#[test]
fn provider_native_features_use_the_same_selected_offering_and_url_facts_without_exposing_them() {
    let (mut registry, mut context, reference) = fixture();
    let visibility = context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap();
    visibility.show_provider = false;
    visibility.show_offering = false;
    visibility.show_price = false;
    let mut declared = registry.extension("fixture").unwrap().clone();
    declared.programs.get_mut("fixture-program").unwrap().native_switches = serde_json::from_value(json!({
        "native_lookup":{"capability":"lookup","values":{"live":"live","disabled":"disabled"}},
        "endpoint_feature":{"capability":"lookup","value":false,"values":{"yes":true,"no":false},"provider_overrides":true,
            "when":{"required":["access"],"properties":{"access":{"const":"forwarded"}}}}
    })).unwrap();
    let provider_declaration: Declarations = serde_json::from_value(json!({"endpoint_rules":[{
        "when":{"required":["host"],"properties":{"host":{"const":"provider.example.invalid"}}},
        "native_switches":{"fixture-program":{
            "endpoint_feature":{"capability":"lookup","value":true,"when":{
                "required":["offering","endpoint_facts"],"properties":{
                    "offering":{"required":["parameters"],"properties":{"parameters":{"required":["context_window"],"properties":{"context_window":{"const":1000}}}}},
                    "endpoint_facts":{"required":["host","scheme","has_credentials","has_query"],"properties":{"host":{"const":"provider.example.invalid"},"scheme":{"const":"https"},"has_credentials":{"const":false},"has_query":{"const":false}}}
                }}},
            "native_lookup":{"capability":"lookup","value":"live","provider_overrides":true},
            "unregistered_setting":{"capability":"lookup","value":"https://private.example/credential"}
        }}
    }]})).unwrap();
    let projected = super::super::project_endpoint(
        &provider_declaration,
        "fixture-generation",
        &url::Url::parse("https://provider.example.invalid/v1").unwrap(),
        "model",
    );
    declared
        .providers
        .get_mut(&reference.provider)
        .unwrap()
        .offerings
        .get_mut("model")
        .unwrap()
        .native_switches = projected.native_switches;
    registry.register("fixture", declared.clone()).unwrap();
    let resolution = registry.resolve(&context);
    assert_eq!(
        resolution[0].native_config,
        BTreeMap::from([("endpoint_feature".into(), json!(true))]),
        "endpoint feature facts are available even when the native mode is left at program default"
    );
    let encoded = serde_json::to_string(&resolution).unwrap();
    assert!(
        !encoded.contains("provider.example.invalid")
            && !encoded.contains("private.example")
            && !encoded.contains("unregistered_setting")
            && !encoded.contains(&reference.provider.to_string())
    );
    let carried: Vec<Resolution> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(carried[0].native_config, resolution[0].native_config);
    assert!(carried[0].route.is_none());

    let mut changed = declared.clone();
    changed
        .providers
        .get_mut(&reference.provider)
        .unwrap()
        .offerings
        .get_mut("model")
        .unwrap()
        .parameters["context_window"] = json!(999);
    registry.register("fixture", changed).unwrap();
    context.facts = json!({"offering":{"parameters":{"context_window":1000}},"endpoint_facts":{"host":"provider.example.invalid","scheme":"https","has_credentials":false,"has_query":false}});
    assert_eq!(
        registry.resolve(&context)[0].native_config["endpoint_feature"],
        false,
        "caller-supplied generic facts cannot replace the actual selected model"
    );
    for address in [
        "http://provider.example.invalid/v1",
        "https://provider.example.invalid.attacker.invalid/v1",
        "https://provider.example.invalid/v1?private=1",
    ] {
        let mut changed = declared.clone();
        changed
            .providers
            .get_mut(&reference.provider)
            .unwrap()
            .endpoints
            .get_mut("main")
            .unwrap()
            .address = address.into();
        registry.register("fixture", changed).unwrap();
        assert_eq!(
            registry.resolve(&context)[0].native_config["endpoint_feature"],
            false
        );
    }
    for bad in [
        json!("https://private.example/credential"),
        json!({"private":"value"}),
    ] {
        let mut changed = declared.clone();
        changed
            .providers
            .get_mut(&reference.provider)
            .unwrap()
            .offerings
            .get_mut("model")
            .unwrap()
            .native_switches
            .get_mut("fixture-program")
            .unwrap()
            .get_mut("endpoint_feature")
            .unwrap()
            .value = Some(bad);
        registry.register("fixture", changed).unwrap();
        assert_eq!(
            registry.resolve(&context)[0].native_config["endpoint_feature"],
            false,
            "only the program's declared public scalar values may cross the carrier"
        );
    }
    registry.register("fixture", declared).unwrap();
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    context.backends.insert(
        "lookup".into(),
        BackendRef {
            route: "auxiliary".into(),
            offering: Some(reference),
            ..BackendRef::default()
        },
    );
    let resolution = registry.resolve(&context);
    assert_eq!(
        resolution[0].native_config["native_lookup"], "disabled",
        "provider features cannot reopen a program-owned mode switch during service execution"
    );
}

#[test]
fn unavailable_explicit_service_keeps_native_off_without_publishing_a_route_or_private_backend() {
    let (mut registry, mut context, reference) = fixture();
    let mut declared = registry.extension("fixture").unwrap().clone();
    declared
        .programs
        .get_mut("fixture-program")
        .unwrap()
        .native_switches = serde_json::from_value(json!({
        "native_lookup":{"capability":"lookup","values":{"disabled":false,"live":true}}
    }))
    .unwrap();
    registry.register("fixture", declared).unwrap();
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    for boundary in ["no_backend", "no_executor", "no_use"] {
        let mut current = context.clone();
        if boundary != "no_backend" {
            current.backends.insert(
                "lookup".into(),
                BackendRef {
                    route: "auxiliary".into(),
                    offering: Some(reference.clone()),
                    config: json!({"private_endpoint":"never-in-carrier"}),
                    ..BackendRef::default()
                },
            );
        }
        if boundary == "no_executor" {
            current.executors.clear();
        }
        if boundary == "no_use" {
            current
                .availability
                .get_mut(&reference.provider)
                .unwrap()
                .get_mut(&reference.offering)
                .unwrap()
                .allowed = false;
        }
        let resolved = registry.resolve(&current);
        assert!(resolved[0].reason.is_some(), "{boundary}");
        assert!(
            resolved[0].route.is_none() && resolved[0].route_view.is_none(),
            "{boundary}"
        );
        assert_eq!(
            resolved[0].native_config,
            BTreeMap::from([("native_lookup".into(), json!(false))]),
            "{boundary}"
        );
        let wire = serde_json::to_string(&resolved).unwrap();
        assert!(
            !wire.contains("never-in-carrier") && !wire.contains(&reference.provider.to_string())
        );
    }
    context.choices.insert("lookup".into(), Some("live".into()));
    context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap()
        .allowed = false;
    assert!(
        registry.resolve(&context)[0].native_config.is_empty(),
        "an unavailable native route never gains an affirmative switch"
    );
}

#[test]
fn a_selected_backend_uses_its_actual_endpoint_and_ceiling_without_rebinding_the_primary_offering()
{
    let (mut registry, mut context, reference) = fixture();
    let primary = registry.declarations().providers[&reference.provider].clone();
    let interface = registry.declarations().interfaces["fixture-generation"].clone();
    registry
        .register(
            "auxiliary-interface",
            Declarations {
                interfaces: BTreeMap::from([("fixture-auxiliary-generation".into(), interface)]),
                ..Declarations::default()
            },
        )
        .unwrap();
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    context.backends.insert(
        "lookup".into(),
        BackendRef {
            route: "auxiliary".into(),
            offering: Some(reference.clone()),
            ..BackendRef::default()
        },
    );
    let mut offering = primary.offerings[&reference.offering].clone();
    offering.parameters["context_window"] = json!(2000);
    context.backend_offerings.insert(
        "lookup".into(),
        SelectedBackend {
            reference: reference.clone(),
            endpoint: Some(Endpoint {
                interface: "fixture-auxiliary-generation".into(),
                address: "https://private-auxiliary.example.invalid/v2".into(),
                credential_role: Some("model".into()),
                ..Endpoint::default()
            }),
            offering,
            availability: Availability {
                allowed: true,
                ..Availability::default()
            },
        },
    );
    let access = registry.declarations().programs["fixture-program"]
        .access
        .get("forwarded");
    let selected = candidate(
        registry.declarations(),
        &context,
        access,
        "lookup",
        "service",
        "auxiliary",
        true,
    )
    .unwrap();
    assert_eq!(
        selected.facts["endpoint"]["interface"],
        "fixture-auxiliary-generation"
    );
    assert_eq!(
        selected.facts["offering"]["parameters"]["context_window"],
        2000
    );
    let resolved = registry.resolve(&context);
    assert_eq!(
        resolved[0].route.as_ref().unwrap().offering.as_ref(),
        Some(&reference)
    );
    assert_eq!(resolved[0].route_view.as_ref().unwrap().kind, "service");
    assert!(
        resolved[0].provider.is_none()
            && resolved[0].offering.is_none()
            && resolved[0].prices.is_empty()
    );
    let wire = serde_json::to_string(&context).unwrap();
    assert!(!wire.contains("private-auxiliary.example.invalid"));
    context.choices.insert("lookup".into(), Some("live".into()));
    let native = candidate(
        registry.declarations(),
        &context,
        access,
        "lookup",
        "live",
        "native",
        true,
    )
    .unwrap();
    assert_eq!(native.facts["endpoint"]["interface"], "fixture-generation");
    assert_eq!(
        native.facts["offering"]["parameters"]["context_window"],
        1000
    );
    assert_eq!(
        registry.declarations().providers[&reference.provider],
        primary
    );
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    context
        .backend_offerings
        .get_mut("lookup")
        .unwrap()
        .availability
        .allowed = false;
    assert!(
        registry.resolve(&context)[0].route.is_none(),
        "backend-specific cap/Use refusal cannot borrow the primary offering's permission"
    );
    let backend = context.backend_offerings.get_mut("lookup").unwrap();
    backend.availability.allowed = true;
    backend.reference.offering = "a-previous-private-selection".into();
    assert!(
        registry.resolve(&context)[0].route.is_none(),
        "a stale backend snapshot never falls back to the primary offering's facts"
    );
}

#[test]
fn a_chosen_backend_keeps_its_own_refusal_instead_of_an_unrelated_executors_failure() {
    let (mut registry, mut context, reference) = fixture();
    let mut declared = registry.extension("fixture").unwrap().clone();
    let mut other = declared.routes["auxiliary"].clone();
    other.executor = Some("an-uninstalled-unselected-runner".into());
    declared.routes.insert("other-backend".into(), other);
    declared
        .capabilities
        .get_mut("lookup")
        .unwrap()
        .modes
        .get_mut("service")
        .unwrap()
        .routes
        .push("other-backend".into());
    registry.register("fixture", declared).unwrap();
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    assert_eq!(
        registry.resolve(&context)[0].reason.as_deref(),
        Some("backend_not_selected")
    );
    context.backends.insert(
        "lookup".into(),
        BackendRef {
            route: "auxiliary".into(),
            offering: Some(reference.clone()),
            ..BackendRef::default()
        },
    );
    context
        .availability
        .get_mut(&reference.provider)
        .unwrap()
        .get_mut(&reference.offering)
        .unwrap()
        .allowed = false;
    assert_eq!(
        registry.resolve(&context)[0].reason.as_deref(),
        Some("current_authority_refused")
    );
    context.executors.clear();
    assert_eq!(
        registry.resolve(&context)[0].reason.as_deref(),
        Some("executor_not_installed")
    );
}

#[test]
fn a_native_delegated_backend_has_no_wire_endpoint_and_keeps_its_own_denial() {
    let (mut registry, mut context, reference) = fixture();
    let mut declared = registry.extension("fixture").unwrap().clone();
    declared.routes.insert(
        "auxiliary".into(),
        serde_json::from_value(json!({
            "kind":"delegation", "executor":"program-session", "capabilities":["lookup"],
            "uses_backend":true, "requires_offering":true, "requires_opt_in":true,
            "requires_orch_tools":true, "orch_tool":"lookup", "disable_native":true
        }))
        .unwrap(),
    );
    let primary = declared.providers[&reference.provider].clone();
    registry.register("fixture", declared).unwrap();
    context.executors = vec!["program-session".into()];
    context
        .choices
        .insert("lookup".into(), Some("service".into()));
    context.backends.insert(
        "lookup".into(),
        BackendRef {
            route: "auxiliary".into(),
            offering: Some(reference.clone()),
            ..BackendRef::default()
        },
    );
    context.backend_offerings.insert(
        "lookup".into(),
        SelectedBackend {
            reference: reference.clone(),
            endpoint: None,
            offering: primary.offerings[&reference.offering].clone(),
            availability: Availability {
                allowed: true,
                ..Availability::default()
            },
        },
    );
    let result = registry.resolve(&context);
    assert_eq!(result[0].route.as_ref().unwrap().kind, "delegation");
    assert_eq!(
        result[0].route.as_ref().unwrap().offering.as_ref(),
        Some(&reference)
    );
    assert_eq!(
        result[0].route.as_ref().unwrap().orch_tool.as_deref(),
        Some("lookup")
    );
    context
        .backend_offerings
        .get_mut("lookup")
        .unwrap()
        .availability
        .allowed = false;
    let refused = registry.resolve(&context);
    assert!(
        refused[0].route.is_none(),
        "the helper cannot borrow the primary model's positive allowance"
    );
    assert!(context.availability[&reference.provider][&reference.offering].allowed);
    context.choices.insert("lookup".into(), Some("live".into()));
    assert_eq!(
        registry.resolve(&context)[0].route.as_ref().unwrap().kind,
        "native-model"
    );
    assert_eq!(
        registry.declarations().providers[&reference.provider],
        primary
    );
}
