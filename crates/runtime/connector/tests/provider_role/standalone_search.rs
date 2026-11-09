//! An installed endpoint declaration admits native side requests through the ordinary key and meter.
use super::*;
use axum::{Json as HttpJson, Router, extract::State, http::HeaderMap, routing::post};
use orch_link_contract::commands::LeaseOpen;
use orch_provider_contract::capabilities::{Binding, Declarations, EndpointRule};
use std::{collections::BTreeMap, sync::Mutex};

type Requests = Arc<Mutex<Vec<(HeaderMap, serde_json::Value)>>>;

fn binding() -> Binding {
    let mut binding = orch_provider_contract::capabilities::protocol_interfaces()
        .unwrap()
        .remove(RESPONSES)
        .unwrap()
        .native_tools
        .remove("web_search")
        .unwrap();
    let mut form: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../extensions/providers/openai/src/standalone-search.json"
    ))
    .unwrap();
    let mut schema = binding.request["options_schema"].clone();
    schema["properties"]
        .as_object_mut()
        .unwrap()
        .remove("return_token_budget");
    form["options_schema"] = schema;
    binding.request["forms"] = json!([form]);
    binding
}

struct Fixture {
    provider: ProviderRig,
    requests: Requests,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(declared: bool) -> Self {
        Box::pin(Self::with_options(declared, json!({}))).await
    }

    async fn with_options(declared: bool, options: serde_json::Value) -> Self {
        let requests = Requests::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/v1", listener.local_addr().unwrap());
        let app = Router::new()
            .route(
                "/v1/alpha/search",
                post(
                    |State(requests): State<Requests>,
                     headers: HeaderMap,
                     HttpJson(body): HttpJson<serde_json::Value>| async move {
                        let failed = body
                            .pointer("/commands/search_query/0/q")
                            .and_then(serde_json::Value::as_str)
                            == Some("fail");
                        requests.lock().unwrap().push((headers, body));
                        if failed {
                            (
                                axum::http::StatusCode::BAD_GATEWAY,
                                HttpJson(json!({"error":{"message":"search unavailable"}})),
                            )
                        } else {
                            (
                                axum::http::StatusCode::OK,
                                HttpJson(json!({"output":"Actual source output","results":[]})),
                            )
                        }
                    },
                ),
            )
            .with_state(Arc::clone(&requests));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = Box::pin(ProviderRig::with(move |rig, _| {
            rig.config.provider.upstreams.push(address.clone());
            let mut sources = orch_provider_contract::builtins::BuiltinSources::default();
            if declared {
                sources.register_capabilities("fixture-standalone", Declarations {
                    endpoint_rules: vec![EndpointRule {
                        when: json!({"required":["protocol","host"],"properties":{"protocol":{"const":RESPONSES},"host":{"const":"127.0.0.1"}}}),
                        native_tools: BTreeMap::from([("web_search".into(), Some(binding()))]),
                        ..EndpointRule::default()
                    }],
                    ..Declarations::default()
                }).unwrap();
            }
            let sources = Arc::new(sources);
            rig.plug = Some(Arc::new(move |builder, roles, data, key, clock| {
                let parts = orch_connector_core::roles::provider::ProviderParts::new(
                    common::model::protocols(), data, key, clock).unwrap();
                orch_connector_core::roles::provider::plug_in(builder.builtin_sources(Arc::clone(&sources)), roles, parts)
            }));
        })).await;
        // The configured allowlist contains the exact authored service address.
        let configured = provider
            .rig
            .config
            .provider
            .upstreams
            .last()
            .unwrap()
            .clone();
        let started = provider
            .start_provider(&[(RESPONSES, configured, options)])
            .await;
        assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
        Self {
            provider,
            requests,
            server,
        }
    }

    async fn open(
        &self,
        mode: &str,
        route: &str,
        flag: bool,
        carry: bool,
    ) -> (orch_types::Id, orch_types::Id) {
        let thread = new_id();
        let lease = new_id();
        let mut parameters = common::model::enabled_parameters();
        parameters.web_search = Some(vec!["cached".into(), "indexed".into(), "live".into()]);
        // A stale stored binding cannot authorize an endpoint without its current declaration.
        parameters.native_tools = BTreeMap::from([(
            RESPONSES.into(),
            BTreeMap::from([("web_search".into(), Some(binding()))]),
        )]);
        let mut body = json!({
            "lease_id":lease,"provider_id":self.provider.rig.provider,"thread_id":thread,
            "mode":"forwarded","protocol":RESPONSES,"allowed_models":[MODEL],
            "model_parameters":{(MODEL):parameters}
        });
        if carry {
            body["native_model"] = json!({"model":MODEL,"parameters":parameters,"capabilities":[
                {"capability":"generation","native_config":{}},
                {"capability":"web_search","mode":mode,"native_config":{"supports_standalone_web_search":flag},"route_view":{"kind":route,"disable_native":route!="native-model"}}
            ]});
        }
        self.provider
            .rig
            .succeed(
                &self.provider.provider_key(),
                &serde_json::from_value::<LeaseOpen>(body).unwrap(),
            )
            .await;
        (thread, lease)
    }
}

fn request(mode: serde_json::Value, commands: serde_json::Value) -> serde_json::Value {
    json!({"id":"native-session","model":MODEL,"input":[],"max_output_tokens":100,
        "commands":commands,"settings":{"external_web_access":mode}})
}

#[tokio::test]
async fn native_side_search_uses_the_existing_lease_key_and_counts_only_successful_search_and_page_calls()
 {
    let fixture = Fixture::new(true).await;
    let p = &fixture.provider;
    let (thread, lease) = fixture.open("indexed", "native-model", true, true).await;
    let harness = p.harness(thread);
    for commands in [
        json!({"search_query":[{"q":"one"},{"q":"two"}]}),
        json!({"find":[{"ref_id":"page","pattern":"word"}]}),
    ] {
        let reply = harness
            .post("/v1/alpha/search", &request(json!("indexed"), commands))
            .await;
        assert_eq!(reply.status, 200);
        assert_eq!(
            reply.json(),
            json!({"output":"Actual source output","results":[]})
        );
    }
    let failed = harness
        .post(
            "/v1/alpha/search",
            &request(json!("indexed"), json!({"search_query":[{"q":"fail"}]})),
        )
        .await;
    assert_eq!(failed.status, 502);
    let events = p.wait_provider_event("lease.usage", 3).await;
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event.body["timing"]["web_searches"] == json!({"state":"told","value":1})
            )
            .count(),
        1
    );
    assert_eq!(events.iter().filter(|event| event.body["timing"]["web_fetches"] == json!({"state":"told","value":1})).count(), 1);
    let failed = events
        .iter()
        .find(|event| event.body["timing"]["upstream_status"] == 502)
        .unwrap();
    assert_ne!(failed.body["timing"]["web_searches"]["state"], "told");
    for event in &events {
        assert!(!event.body.to_string().contains(&p.key));
        assert!(!event.body.to_string().contains(&harness.token));
    }
    let closed = p.close_lease(lease).await;
    assert_eq!(closed["requests"], 3);
    assert!(
        closed["usage"].is_null(),
        "no side reply reported generation tokens"
    );
    let groups = closed["usage_groups"].as_array().unwrap();
    assert_eq!(
        groups
            .iter()
            .map(|group| group["search_usage"]["searches"].as_i64().unwrap())
            .sum::<i64>(),
        1
    );
    assert_eq!(
        groups
            .iter()
            .map(|group| group["search_usage"]["fetches"].as_i64().unwrap())
            .sum::<i64>(),
        1
    );
    let recorded = fixture.requests.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    for (headers, body) in recorded.iter() {
        assert_eq!(
            headers["authorization"].to_str().unwrap(),
            format!("Bearer {}", p.key)
        );
        assert_ne!(
            headers["authorization"].to_str().unwrap(),
            format!("Bearer {}", harness.token)
        );
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["settings"]["external_web_access"], "indexed");
        assert!(body.get("tools").is_none());
        assert!(body.get("access_programs").is_none());
    }
}

#[tokio::test]
async fn stale_flags_missing_carriers_services_and_changed_modes_make_no_native_paid_request() {
    let fixture = Fixture::new(true).await;
    for (mode, route, flag, carry) in [
        ("indexed", "native-model", true, false),
        ("indexed", "native-model", false, true),
        ("disabled", "native-model", true, true),
        ("service", "service", true, true),
        ("cached", "native-model", true, true),
    ] {
        let (thread, _) = fixture.open(mode, route, flag, carry).await;
        let reply = fixture
            .provider
            .harness(thread)
            .post(
                "/v1/alpha/search",
                &request(json!("indexed"), json!({"search_query":[{"q":"source"}]})),
            )
            .await;
        assert_eq!(reply.status, 403, "{mode}/{route}/{flag}/{carry}");
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
    let undeclared = Fixture::new(false).await;
    let (thread, _) = undeclared.open("indexed", "native-model", true, true).await;
    let reply = undeclared
        .provider
        .harness(thread)
        .post(
            "/v1/alpha/search",
            &request(json!("indexed"), json!({"search_query":[{"q":"source"}]})),
        )
        .await;
    assert_eq!(reply.status, 403);
    assert!(undeclared.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn endpoint_option_overlays_cannot_replace_the_declared_native_input() {
    let fixture = Fixture::with_options(
        true,
        json!({"params":{"commands":{"search_query":[{"q":"replacement"}]}}}),
    )
    .await;
    let (thread, _) = fixture.open("indexed", "native-model", true, true).await;
    let reply = fixture
        .provider
        .harness(thread)
        .post(
            "/v1/alpha/search",
            &request(json!("indexed"), json!({"search_query":[{"q":"original"}]})),
        )
        .await;
    assert_eq!(reply.status, 400);
    assert!(fixture.requests.lock().unwrap().is_empty());
}
