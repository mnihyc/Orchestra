//! Additive declaration evidence uses the same strict observations/matcher as harness compatibility replay.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
use orch_model_contract::credentials::Credential;
use orch_model_contract::endpoint::EndpointOptions;
use orch_model_contract::protocol::WireProtocol;
use orch_provider_contract::capabilities::*;
use orch_testkit::harness_corpus::{Bindings, Observation, matches};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use url::Url;

fn wires() -> Vec<Box<dyn WireProtocol>> {
    vec![
        Box::new(orch_protocol_openai_responses::OpenAiResponses),
        Box::new(orch_protocol_openai_responses::OpenAiChat),
        Box::new(orch_protocol_openai_responses::OpenAiEmbeddings),
        Box::new(orch_protocol_anthropic_messages::AnthropicMessages),
    ]
}
fn fixture() -> (Registry, Vec<ThreadContext>, Vec<Vec<u8>>) {
    let table = orch_provider_builtins::sources().unwrap();
    let mut registry = table.capabilities().clone();
    let presets = orch_db::seeds::provider_presets();
    let mut accounts = Declarations::default();
    let protocols = registry
        .declarations()
        .interfaces
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    accounts.programs.insert("conformance-program".into(),serde_json::from_value(json!({"access":{"forwarded":{"interfaces":protocols,"orch_tools":true,"native":{"generation":{"enabled":true,"modes":["native"],"default_mode":"native"},"embeddings":{"enabled":true,"modes":["native"],"default_mode":"native"},"web_search":{"enabled":true,"modes":["live"],"default_mode":"live"}}}}})).unwrap());
    let mut contexts = Vec::new();
    let mut raw_models = Vec::new();
    for (index, (name, preset)) in presets.as_object().unwrap().iter().enumerate() {
        let id = Id::from_u128(0x1000 + index as u128);
        let mut provider = Provider {
            title: name.clone(),
            credential_roles: vec!["model".into()],
            ..Provider::default()
        };
        for (number, endpoint) in preset["upstreams"].as_array().unwrap().iter().enumerate() {
            let interface = endpoint["protocol"].as_str().unwrap().to_owned();
            let key = format!("endpoint-{number}");
            let model = format!("model-{number}");
            let parameters = json!({"web_search":["live"],"default_web_search":"live","context_window":8192,"input_images":true,"request_options":{(interface.clone()):{"type":"object","properties":{"fixture_option":{"enum":["ordinary"]}}}},"future_stored_member":{"preserved":true}});
            raw_models.push(serde_json::to_vec(&parameters).unwrap());
            provider.endpoints.insert(
                key.clone(),
                Endpoint {
                    interface: interface.clone(),
                    address: endpoint["base_url"].as_str().unwrap().into(),
                    credential_role: Some("model".into()),
                    options: endpoint
                        .get("options")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                    ..Endpoint::default()
                },
            );
            provider.offerings.insert(
                model.clone(),
                Offering {
                    endpoint: key,
                    kind: "model".into(),
                    enabled: true,
                    title: model.clone(),
                    parameters,
                    capabilities: BTreeMap::from([
                        (
                            "generation".into(),
                            Support {
                                enabled: true,
                                modes: vec!["native".into()],
                                ..Support::default()
                            },
                        ),
                        (
                            "embeddings".into(),
                            Support {
                                enabled: true,
                                modes: vec!["native".into()],
                                ..Support::default()
                            },
                        ),
                    ]),
                    ..Offering::default()
                },
            );
            let mut context = ThreadContext {
                program: "conformance-program".into(),
                access: "forwarded".into(),
                current: Some(OfferingRef {
                    provider: id,
                    offering: model.clone(),
                }),
                interfaces: protocols.clone(),
                ..ThreadContext::default()
            };
            context.availability.insert(
                id,
                BTreeMap::from([(
                    model,
                    Availability {
                        allowed: true,
                        show_provider: true,
                        show_offering: true,
                        show_price: true,
                        ..Availability::default()
                    },
                )]),
            );
            context.credential_roles.insert(id, vec!["model".into()]);
            contexts.push(context);
        }
        accounts.providers.insert(id, provider);
    }
    registry.register("existing-accounts", accounts).unwrap();
    (registry, contexts, raw_models)
}

fn observations(registry: &Registry, contexts: &[ThreadContext]) -> Vec<Observation> {
    let mut rows = Vec::new();
    let wires = wires();
    for context in contexts {
        let reference = context.current.as_ref().unwrap();
        let provider = &registry.declarations().providers[&reference.provider];
        let model = &provider.offerings[&reference.offering];
        let endpoint = &provider.endpoints[&model.endpoint];
        let interface = &registry.declarations().interfaces[&endpoint.interface];
        let wire = wires
            .iter()
            .find(|wire| wire.name() == endpoint.interface)
            .unwrap();
        let url = Url::parse(&endpoint.address).unwrap();
        let projected = project_endpoint(
            registry.declarations(),
            &endpoint.interface,
            &url,
            "fixture-model",
        );
        let options: EndpointOptions = serde_json::from_value(endpoint.options.clone()).unwrap();
        let credential = Credential::new("model-key-fixture");
        let mut headers = http::HeaderMap::new();
        let query = wire
            .authorize(&mut headers, Some(&credential), &options, &BTreeMap::new())
            .unwrap();
        let mut headers = headers
            .iter()
            .map(|(name, value)| (name.as_str().to_owned(), value.to_str().unwrap().to_owned()))
            .collect::<Vec<_>>();
        headers.sort();
        let (method, path, body) = wire
            .completion_request("fixture-model", "fixture system", "fixture input", 128)
            .or_else(|| wire.embedding_request("fixture-model", &["fixture input".into()], Some(8)))
            .expect("every shipped model interface has a real owned-call encoder");
        rows.push(Observation{name:"provider.resolution".into(),body:json!({"public":registry.resolve(context),"dispatch":registry.resolve(context).into_iter().map(|row|row.route).collect::<Vec<_>>()})});
        rows.push(Observation{name:"provider.options".into(),body:json!({"interface":interface.options,"endpoint":endpoint.options,"model":model.parameters,"native":projected.native_tools,"offered_endpoints":projected.endpoints,"offered_services":projected.offerings})});
        let request_bytes: &[u8] = body.as_ref();
        rows.push(Observation{name:"provider.request".into(),body:json!({"method":method.as_str(),"base":endpoint.address,"path":path,"headers":headers,"query":query,"bytes":request_bytes})});
        rows.push(Observation {
            name: "provider.stored_model".into(),
            body: model.parameters.clone(),
        });
    }
    rows
}
fn exact(before: &[Observation], after: &[Observation]) {
    assert_eq!(before.len(), after.len());
    for (before, after) in before.iter().zip(after) {
        assert_eq!(before.name, after.name);
        matches(
            &before.body,
            &after.body,
            false,
            &mut Bindings::new(),
            "provider",
        )
        .unwrap();
        assert_eq!(
            serde_json::to_vec(before).unwrap(),
            serde_json::to_vec(after).unwrap()
        );
    }
}
fn fake() -> (Declarations, Id) {
    let id = Id::from_u128(0xfeed);
    let mut data = Declarations::default();
    let mut provider = Provider {
        title: "An additive fixture account".into(),
        credential_roles: vec!["service".into()],
        ..Provider::default()
    };
    data.extensions
        .insert("future_declaration_member".into(), json!({"kept":true}));
    for (capability, kind, item) in [
        ("web_search", "search", "web_search"),
        ("page_fetch", "fetch", "web_fetch"),
        ("image_generation", "images", "image"),
    ] {
        let name = format!("fixture-{kind}");
        let route = format!("fixture.{kind}");
        let binding:Binding=serde_json::from_value(json!({"input_schema":{"type":"object","required":["value","quirk_mode"],"properties":{"value":{},"quirk_mode":{"enum":["compact","full"]}},"additionalProperties":false},"request":{"method":"POST","path":format!("/{kind}"),"body":{"value":{"from_input":"/value"},"quirk_mode":{"from_input":"/quirk_mode"}}},"result":{"page":{"item":item,"answer":{"from_response":"/answer"}}},"counters":{"successful_calls":{"constant":1,"only_when":{"properties":{"ok":{"const":true}},"required":["ok"]},"otherwise":0,"unit":"call"}},"prices":{"call":{"amount":"0.0123456789","currency":"USD","unit":"call"}},"future_binding_member":{"ignored":true}})).unwrap();
        data.interfaces.insert(name.clone(),Interface{kind:kind.into(),implementation:name.clone(),options:json!({"type":"object","properties":{"quirk_mode":{"enum":["compact","full"]}}}),shape:binding,..Interface::default()});
        provider.endpoints.insert(
            name.clone(),
            Endpoint {
                address: format!("https://new-provider.fixture.invalid/{kind}"),
                interface: name.clone(),
                credential_role: Some("service".into()),
                options: json!({"quirk_mode":"compact"}),
                ..Endpoint::default()
            },
        );
        provider.offerings.insert(
            name.clone(),
            Offering {
                endpoint: name.clone(),
                kind: "service".into(),
                enabled: true,
                capabilities: BTreeMap::from([(
                    capability.into(),
                    Support {
                        enabled: true,
                        modes: vec!["service".into()],
                        ..Support::default()
                    },
                )]),
                options: json!({"quirk_mode":"full"}),
                parameters: json!({"new_optional_model_member":true}),
                ..Offering::default()
            },
        );
        data.routes.insert(
            route.clone(),
            Route {
                kind: "service".into(),
                executor: Some("fixture-codec".into()),
                capabilities: vec![capability.into()],
                requires_interface: true,
                requires_offering: true,
                requires_offering_support: true,
                uses_backend: true,
                requires_opt_in: true,
                requires_orch_tools: true,
                orch_tool: Some(capability.into()),
                disable_native: true,
                ..Route::default()
            },
        );
        data.route_links
            .entry(capability.into())
            .or_default()
            .insert("service".into(), vec![route]);
    }
    data.providers.insert(id, provider);
    (data, id)
}

#[test]
fn adding_and_removing_a_provider_preserves_every_shipped_resolution_request_options_and_model_bytes()
 {
    let (mut registry, contexts, raw) = fixture();
    let before = observations(&registry, &contexts);
    let matrix = serde_json::to_vec(&registry.matrix(&contexts)).unwrap();
    let expected = registry
        .declarations()
        .providers
        .iter()
        .flat_map(|(id, provider)| {
            provider
                .offerings
                .keys()
                .map(move |offering| (*id, offering.clone()))
        })
        .collect::<BTreeSet<_>>();
    let covered = contexts
        .iter()
        .map(|context| {
            let current = context.current.as_ref().unwrap();
            (current.provider, current.offering.clone())
        })
        .collect::<BTreeSet<_>>();
    assert!(!expected.is_empty());
    assert_eq!(
        covered, expected,
        "every declared offering has its own conformance context"
    );
    assert_eq!(
        contexts.len(),
        covered.len(),
        "no duplicated context can hide a missing offering"
    );
    let wires = wires();
    assert!(contexts.iter().all(|context| {
        let current = context.current.as_ref().unwrap();
        let provider = &registry.declarations().providers[&current.provider];
        let offering = &provider.offerings[&current.offering];
        let endpoint = &provider.endpoints[&offering.endpoint];
        let wire = wires
            .iter()
            .find(|wire| wire.name() == endpoint.interface)
            .unwrap();
        let capability = if wire
            .completion_request("fixture-model", "fixture system", "fixture input", 128)
            .is_some()
        {
            "generation"
        } else {
            assert!(
                wire.embedding_request("fixture-model", &["fixture input".into()], Some(8))
                    .is_some()
            );
            "embeddings"
        };
        registry
            .resolve(context)
            .iter()
            .any(|row| row.capability == capability && row.route.is_some())
    }));
    assert!(before.iter().any(|row| {
        row.name == "provider.request"
            && row.body["bytes"]
                .as_array()
                .is_some_and(|bytes| !bytes.is_empty())
    }));
    let (extra, id) = fake();
    registry.register("additive-provider", extra).unwrap();
    exact(&before, &observations(&registry, &contexts));
    assert_eq!(
        serde_json::to_vec(&registry.matrix(&contexts)).unwrap(),
        matrix
    );
    assert!(
        registry
            .extension("additive-provider")
            .unwrap()
            .extensions
            .contains_key("future_declaration_member")
    );
    for (capability, kind) in [
        ("web_search", "search"),
        ("page_fetch", "fetch"),
        ("image_generation", "images"),
    ] {
        let name = format!("fixture-{kind}");
        let mut context = contexts[0].clone();
        context.executors.push("fixture-codec".into());
        context.interfaces.push(name.clone());
        context
            .choices
            .insert(capability.into(), Some("service".into()));
        context.backends.insert(
            capability.into(),
            BackendRef {
                route: format!("fixture.{kind}"),
                offering: Some(OfferingRef {
                    provider: id,
                    offering: name.clone(),
                }),
                ..BackendRef::default()
            },
        );
        context.credential_roles.insert(id, vec!["service".into()]);
        context.availability.entry(id).or_default().insert(
            name.clone(),
            Availability {
                allowed: true,
                show_provider: true,
                show_offering: true,
                show_price: true,
                ..Availability::default()
            },
        );
        let resolutions = registry.resolve(&context);
        let resolved = resolutions
            .iter()
            .find(|row| row.capability == capability)
            .unwrap();
        assert_eq!(resolved.route.as_ref().unwrap().kind, "service");
        assert!(resolved.route.as_ref().unwrap().disable_native);
        let binding = &registry.declarations().interfaces[&name].shape;
        let input = json!({"value":{"from_input":"/not-executed"},"quirk_mode":"full"});
        let request = render_request(binding, &input).unwrap();
        assert_eq!(request["body"]["value"], input["value"]);
        assert_eq!(request["body"]["quirk_mode"], "full");
        assert!(render_request(binding, &json!({"value":"input","quirk_mode":"unknown"})).is_err());
        assert_eq!(binding.prices["call"].amount, "0.0123456789");
        assert_eq!(
            binding.extensions["future_binding_member"],
            json!({"ignored":true})
        );
        let result = project_result(
            binding,
            &input,
            &json!({"ok":true,"answer":{"kept":true}}),
            1,
        )
        .unwrap();
        assert_eq!(result["answer"], json!({"kept":true}));
        assert_eq!(
            binding.counters["successful_calls"].count(&json!({"ok":true})),
            Some(1)
        );
        assert_eq!(
            binding.counters["successful_calls"].count(&json!({"ok":false})),
            Some(0)
        );
    }
    assert!(registry.remove("additive-provider"));
    exact(&before, &observations(&registry, &contexts));
    assert!(registry.extension("additive-provider").is_none());
    assert!(!registry.declarations().providers.contains_key(&id));
    assert!(
        !registry
            .declarations()
            .interfaces
            .keys()
            .any(|id| id.starts_with("fixture-"))
    );
    let after_raw = contexts
        .iter()
        .map(|context| {
            let reference = context.current.as_ref().unwrap();
            serde_json::to_vec(
                &registry.declarations().providers[&reference.provider].offerings
                    [&reference.offering]
                    .parameters,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(raw, after_raw);
}

#[test]
fn an_old_stored_model_still_reads_and_dispatches_when_an_optional_declaration_member_is_added() {
    let (mut registry, contexts, _) = fixture();
    let before = observations(&registry, &contexts);
    let mut addition = Declarations::default();
    addition.extensions.insert(
        "a_future_optional_contract_field".into(),
        json!({"schema_version":2}),
    );
    registry.register("future", addition).unwrap();
    exact(&before, &observations(&registry, &contexts));
    for context in &contexts {
        let reference = context.current.as_ref().unwrap();
        let model =
            &registry.declarations().providers[&reference.provider].offerings[&reference.offering];
        assert!(
            model
                .parameters
                .get("a_future_optional_contract_field")
                .is_none()
        );
        assert_eq!(
            model.parameters["future_stored_member"],
            json!({"preserved":true})
        );
    }
}

#[test]
fn the_shared_comparison_rejects_resolution_request_option_and_stored_model_mutations() {
    let (registry, contexts, _) = fixture();
    let before = observations(&registry, &contexts);
    for kind in [
        "provider.resolution",
        "provider.request",
        "provider.options",
        "provider.stored_model",
    ] {
        let row = before.iter().find(|row| row.name == kind).unwrap();
        let changed = json!({"silently_changed":row.body});
        assert!(matches(&row.body, &changed, false, &mut Bindings::new(), "provider").is_err());
    }
}

mod gained_fact {
    use super::*;
    use async_trait::async_trait;
    use orch_plugin_contract::Secret;
    use orch_provider_contract::FactMember;
    use orch_provider_contract::builtins::{BuiltinSource, BuiltinSources, VendorClient};
    use orch_provider_contract::facts::{MemberState, ProviderFacts, member_state};
    use orch_provider_contract::source::{
        FactsReport, FactsSource, SourceAsk, SourceDescriptor, SourceDescriptorFacts, SourceError,
    };
    use orch_types::{Clock, ManualClock, Time};
    use std::sync::Arc;

    struct GainingSource {
        balance: bool,
    }
    fn descriptor(balance: bool) -> SourceDescriptor {
        let mut value = SourceDescriptor::new(
            "additive-facts",
            "Fixture account facts",
            "A fixture adds a native member.",
        );
        value.facts = Some(SourceDescriptorFacts {
            reports: if balance {
                vec![FactMember::Spend, FactMember::Balances]
            } else {
                vec![FactMember::Spend]
            },
            ..SourceDescriptorFacts::default()
        });
        value
    }
    fn before_descriptor() -> SourceDescriptor {
        descriptor(false)
    }
    fn after_descriptor() -> SourceDescriptor {
        descriptor(true)
    }
    #[async_trait]
    impl FactsSource for GainingSource {
        fn describe(&self) -> SourceDescriptor {
            descriptor(self.balance)
        }
        async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError> {
            assert!(ask.credential.is_none() && ask.secrets.is_empty());
            let mut report =
                json!({"spend":[{"id":"day","unit":"USD","period":"day","used":"0.123456789"}]});
            if self.balance {
                report["balances"] =
                    json!([{"id":"account","unit":"USD","remaining":"1.000000001"}]);
            }
            Ok(FactsReport(report))
        }
    }
    fn make_before(_: Arc<dyn VendorClient>, _: Arc<dyn Clock>) -> Arc<dyn FactsSource> {
        Arc::new(GainingSource { balance: false })
    }
    fn make_after(_: Arc<dyn VendorClient>, _: Arc<dyn Clock>) -> Arc<dyn FactsSource> {
        Arc::new(GainingSource { balance: true })
    }
    fn entry(balance: bool) -> BuiltinSource {
        BuiltinSource {
            name: "additive-facts",
            hosts: &["new-provider.fixture.invalid"],
            descriptor: if balance {
                after_descriptor
            } else {
                before_descriptor
            },
            matches: |base| base.host_str() == Some("new-provider.fixture.invalid"),
            make: if balance { make_after } else { make_before },
        }
    }
    struct NoTransport;
    #[async_trait]
    impl VendorClient for NoTransport {
        async fn get(&self, _: &Url, _: &str, _: &Secret, _: usize) -> Option<Json> {
            panic!("the authored source owns its report without network access")
        }
    }
    #[tokio::test]
    async fn a_source_gains_a_native_balance_without_changing_other_sources_or_stored_models() {
        let (registry, contexts, raw) = fixture();
        let ordinary = observations(&registry, &contexts);
        let mut sources = orch_provider_builtins::entries();
        sources.push(entry(false));
        let before = BuiltinSources::new(sources.clone()).unwrap();
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(
            "2026-11-08T00:00:00Z".parse::<Time>().unwrap().get(),
        ));
        let read = |table: &BuiltinSources| {
            (table.get("additive-facts").unwrap().make)(Arc::new(NoTransport), clock.clone())
        };
        let ask = || {
            serde_json::from_value(json!({"provider":{"id":Id::from_u128(0xfeed),"name":"Fixture"},"endpoints":[],"secrets":{}})).unwrap()
        };
        let old = read(&before);
        let old_report = old.read_facts(ask()).await.unwrap();
        let old_facts: ProviderFacts = serde_json::from_value(old_report.0.clone()).unwrap();
        assert_eq!(
            member_state(FactMember::Balances, &old_facts, old.describe().reports()),
            MemberState::Unsupported
        );
        assert!(!old_report.0.as_object().unwrap().contains_key("balances"));
        *sources.last_mut().unwrap() = entry(true);
        let after = BuiltinSources::new(sources).unwrap();
        let new = read(&after);
        let new_report = new.read_facts(ask()).await.unwrap();
        let new_facts: ProviderFacts = serde_json::from_value(new_report.0.clone()).unwrap();
        assert_eq!(
            member_state(FactMember::Balances, &new_facts, new.describe().reports()),
            MemberState::Told
        );
        assert_eq!(new_report.0["balances"][0]["remaining"], "1.000000001");
        assert_eq!(new_report.0["spend"], old_report.0["spend"]);
        for ordinary_source in orch_provider_builtins::entries() {
            assert_eq!(
                serde_json::to_vec(&(before.get(ordinary_source.name).unwrap().descriptor)())
                    .unwrap(),
                serde_json::to_vec(&(after.get(ordinary_source.name).unwrap().descriptor)())
                    .unwrap()
            );
        }
        assert_eq!(
            serde_json::to_vec(before.capabilities().declarations()).unwrap(),
            serde_json::to_vec(after.capabilities().declarations()).unwrap()
        );
        exact(&ordinary, &observations(&registry, &contexts));
        let retained = contexts
            .iter()
            .map(|context| {
                let reference = context.current.as_ref().unwrap();
                serde_json::to_vec(
                    &registry.declarations().providers[&reference.provider].offerings
                        [&reference.offering]
                        .parameters,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(raw, retained);
    }
}
