//! A fake extension reaches an actual HTTP service through the credential-keeping provider role.
use super::*;
use axum::{Json as HttpJson, Router, extract::State, http::HeaderMap, routing::post};
use orch_provider_contract::capabilities::{
    Binding, Counter, Declarations, Endpoint, EndpointRule, Interface, Offering, Support,
};
use std::{collections::BTreeMap, sync::Mutex};

type Requests = Arc<Mutex<Vec<(HeaderMap, serde_json::Value)>>>;

struct ServiceFixture {
    provider: ProviderRig,
    requests: Requests,
    declarations: Declarations,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for ServiceFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl ServiceFixture {
    async fn new(allow_service: bool) -> Self {
        Self::with_transport(allow_service, "http-json", None).await
    }
    async fn with_transport(
        allow_service: bool,
        implementation: &str,
        encoding: Option<&str>,
    ) -> Self {
        Self::with_binding(allow_service, implementation, encoding, |_| {}).await
    }
    async fn with_binding(
        allow_service: bool,
        implementation: &str,
        encoding: Option<&str>,
        customize: impl FnOnce(&mut Binding),
    ) -> Self {
        let requests = Requests::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route(
                "/search",
                post(
                    |State(requests): State<Requests>,
                     headers: HeaderMap,
                     HttpJson(body): HttpJson<serde_json::Value>| async move {
                        let fail = body["query"] == "fail";
                        let empty = body["query"] == "empty";
                        requests.lock().unwrap().push((headers, body));
                        if fail {
                            (
                                axum::http::StatusCode::BAD_GATEWAY,
                                HttpJson(json!({"error":"private upstream words"})),
                            )
                        } else {
                            (
                                axum::http::StatusCode::OK,
                                HttpJson(json!({"results":if empty {json!([])} else {json!([
                    {"title":"First", "url":"https://first.example.invalid/"},
                    {"title":"Second", "url":"https://second.example.invalid/"}
                ])},"cost":"0.0000000000000000000123456789"})),
                            )
                        }
                    },
                ),
            )
            .route(
                "/multipart",
                post(
                    |State(requests): State<Requests>,
                     headers: HeaderMap,
                     body: axum::body::Bytes| async move {
                        requests.lock().unwrap().push((
                            headers,
                            json!({"raw":String::from_utf8(body.to_vec()).unwrap()}),
                        ));
                        HttpJson(json!({"data":[{"b64_json":"YQ=="},{"b64_json":"Yg=="}]}))
                    },
                ),
            )
            .with_state(Arc::clone(&requests));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut binding = Binding {
            input_schema: json!({"type":"object","required":["query"],"properties":{"query":{"type":"string","minLength":1}},"additionalProperties":false}),
            request: json!({"method":"POST","path":"/search","headers":{"authorization":"must-be-removed","cookie":"must-be-removed"},"body":{"query":{"from_input":"/query"}},"success_statuses":[200]}),
            counters: BTreeMap::from([
                (
                    "operations".into(),
                    Counter {
                        constant: Some(1),
                        only_when: json!({"required":["results"],"properties":{"results":{"type":"array","minItems":1}}}),
                        otherwise: Some(0),
                        successful_only: true,
                        unit: "search".into(),
                        ..Counter::default()
                    },
                ),
                (
                    "unknown".into(),
                    Counter {
                        pointer: "/unreported".into(),
                        unit: "page".into(),
                        ..Counter::default()
                    },
                ),
                (
                    "cost".into(),
                    Counter {
                        pointer: "/cost".into(),
                        unit: "USD".into(),
                        reported_cost: true,
                        ..Counter::default()
                    },
                ),
            ]),
            ..Binding::default()
        };
        if let Some(encoding) = encoding {
            binding.request["encoding"] = json!(encoding);
        }
        customize(&mut binding);
        let endpoints: BTreeMap<String, Endpoint> = [
            ("private", Some("model")),
            ("public", None),
            ("admin", Some("admin")),
            ("no-role-key", None),
        ]
        .into_iter()
        .map(|(id, credential_role)| {
            (
                id.into(),
                Endpoint {
                    address: address.clone(),
                    interface: "fixture-http-service".into(),
                    credential_role: credential_role.map(str::to_owned),
                    options: if id == "no-role-key" {
                        json!({"auth":"bearer"})
                    } else {
                        serde_json::Value::Null
                    },
                    ..Endpoint::default()
                },
            )
        })
        .collect();
        let mut declarations = Declarations {
            interfaces: BTreeMap::from([(
                "fixture-http-service".into(),
                Interface {
                    implementation: implementation.into(),
                    shape: binding,
                    ..Interface::default()
                },
            )]),
            endpoint_rules: vec![EndpointRule {
                when: json!({"required":["protocol"],"properties":{"protocol":{"const":RESPONSES}}}),
                offerings: endpoints
                    .keys()
                    .map(|id| {
                        (
                            id.clone(),
                            Offering {
                                kind: "service".into(),
                                endpoint: id.clone(),
                                capabilities: BTreeMap::from([(
                                    "web_search".into(),
                                    Support {
                                        enabled: true,
                                        ..Support::default()
                                    },
                                )]),
                                ..Offering::default()
                            },
                        )
                    })
                    .collect(),
                endpoints,
                ..EndpointRule::default()
            }],
            ..Declarations::default()
        };
        let mut public_binding = declarations.interfaces["fixture-http-service"]
            .shape
            .clone();
        public_binding.request["body"]["offering"] = json!("public");
        declarations.endpoint_rules[0]
            .offerings
            .get_mut("public")
            .unwrap()
            .native_tools
            .insert("web_search".into(), Some(public_binding));
        let mut sources = orch_provider_contract::builtins::BuiltinSources::default();
        sources
            .register_capabilities("fixture-http-service", declarations.clone())
            .unwrap();
        let sources = Arc::new(sources);
        let provider = ProviderRig::with(move |rig, _| {
            if allow_service {
                rig.config.provider.upstreams.push(address);
            }
            rig.plug = Some(Arc::new(move |builder, roles, data, key, clock| {
                let parts = orch_connector_core::roles::provider::ProviderParts::new(
                    common::model::protocols(),
                    data,
                    key,
                    clock,
                )
                .unwrap();
                orch_connector_core::roles::provider::plug_in(
                    builder.builtin_sources(Arc::clone(&sources)),
                    roles,
                    parts,
                )
            }));
        })
        .await;
        provider.start_responses_provider().await;
        Self {
            provider,
            requests,
            declarations,
            server,
        }
    }

    fn command(
        &self,
        offering: &str,
        query: &str,
    ) -> orch_link_contract::commands::ProviderService {
        let endpoint = &self.declarations.endpoint_rules[0].endpoints[offering];
        let selected = &self.declarations.endpoint_rules[0].offerings[offering];
        let binding = orch_provider_contract::services::service_binding(
            &self.declarations,
            endpoint,
            selected,
            "web_search",
        )
        .unwrap();
        orch_link_contract::commands::ProviderService {
            input_images: Vec::new(),
            provider_id: self.provider.rig.provider,
            offering: offering.into(),
            capability: "web_search".into(),
            input: json!({"query":query}),
            definition: orch_provider_contract::services::definition_hash(endpoint, binding)
                .unwrap(),
        }
    }
}

#[tokio::test]
async fn service_transport_keeps_private_and_anonymous_credentials_separate_and_counts_actual_calls()
 {
    let f = ServiceFixture::new(true).await;
    let declared_base = &f.declarations.endpoint_rules[0].endpoints["private"].address;
    assert!(
        f.provider
            .rig
            .config
            .provider
            .upstreams
            .contains(declared_base)
    );
    assert!(
        !f.provider
            .rig
            .config
            .provider
            .upstreams
            .contains(&format!("{declared_base}/search")),
        "the fixture permits the exact base, not a complete per-method address"
    );
    for offering in ["private", "public"] {
        let command = f.command(offering, "two links");
        assert!(
            !serde_json::to_string(&command)
                .unwrap()
                .contains(&f.provider.key)
        );
        let result = f
            .provider
            .rig
            .succeed(&f.provider.provider_key(), &command)
            .await;
        assert_eq!(result["output"]["results"].as_array().unwrap().len(), 2);
        assert_eq!(result["counters"]["operations"], 1);
        assert_eq!(result["counters"]["unknown"], serde_json::Value::Null);
        assert_eq!(
            result["timing"]["reported_cost"]["value"]["amount"],
            "0.0000000000000000000123456789"
        );
        assert!(!result.to_string().contains(&f.provider.key));
    }
    let requests = f.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].0["authorization"],
        format!("Bearer {}", f.provider.key)
    );
    assert!(!requests[1].0.contains_key("authorization"));
    assert!(
        requests
            .iter()
            .all(|(headers, _)| !headers.contains_key("cookie"))
    );
    assert_eq!(requests[0].1, json!({"query":"two links"}));
    assert_eq!(
        requests[1].1,
        json!({"query":"two links","offering":"public"}),
        "the same runtime interprets the selected offering's fixed codec"
    );
}

#[tokio::test]
async fn service_transport_refuses_a_changed_codec_input_or_local_upstream_before_http() {
    let f = ServiceFixture::new(true).await;
    let mut changed = f.command("private", "query");
    changed.definition = orch_types::Hash::of(b"other definition");
    assert!(matches!(
        f.provider
            .rig
            .send(&f.provider.provider_key(), &changed)
            .await,
        Progress::Failed(_)
    ));
    let mut malformed = f.command("private", "query");
    malformed.input["endpoint"] = json!("https://another.example.invalid/");
    assert!(matches!(
        f.provider
            .rig
            .send(&f.provider.provider_key(), &malformed)
            .await,
        Progress::Failed(_)
    ));
    assert!(f.requests.lock().unwrap().is_empty());
    for offering in ["admin", "no-role-key"] {
        assert!(matches!(
            f.provider
                .rig
                .send(&f.provider.provider_key(), &f.command(offering, "query"))
                .await,
            Progress::Failed(_)
        ));
    }
    assert!(f.requests.lock().unwrap().is_empty());
    let blocked = ServiceFixture::new(false).await;
    let command = blocked.command("private", "query");
    let Progress::Failed(refusal) = blocked
        .provider
        .rig
        .send(&blocked.provider.provider_key(), &command)
        .await
    else {
        panic!("a service whose declared base is not listed must be refused");
    };
    assert_eq!(refusal.code, "unsupported");
    assert_eq!(
        refusal.detail,
        "provider.upstreams does not allow this service address"
    );
    assert!(blocked.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn service_transport_keeps_unknown_counts_distinct_from_free_empty_and_failed_requests() {
    let f = ServiceFixture::new(true).await;
    let result = f
        .provider
        .rig
        .succeed(&f.provider.provider_key(), &f.command("private", "empty"))
        .await;
    assert_eq!(result["counters"]["operations"], 0);
    assert_eq!(result["counters"]["unknown"], serde_json::Value::Null);
    let failed = f
        .provider
        .rig
        .send(&f.provider.provider_key(), &f.command("private", "fail"))
        .await;
    let Progress::Failed(error) = failed else {
        panic!("HTTP failure cannot become a successful tool result")
    };
    let details = error.details.unwrap();
    assert_eq!(details["service_result"]["counters"]["operations"], 0);
    assert_eq!(
        details["service_result"]["counters"]["unknown"],
        serde_json::Value::Null
    );
    assert_eq!(details["service_result"]["timing"]["upstream_status"], 502);
    assert!(
        !serde_json::to_string(&details)
            .unwrap()
            .contains("private upstream words")
    );
}

#[tokio::test]
async fn the_same_service_transport_accepts_image_api_json_and_refuses_unmaterialized_multipart_before_io()
 {
    let image = ServiceFixture::with_transport(true, "image-api", Some("json")).await;
    let result = image
        .provider
        .rig
        .succeed(
            &image.provider.provider_key(),
            &image.command("private", "image request"),
        )
        .await;
    assert_eq!(result["output"]["results"].as_array().unwrap().len(), 2);
    assert_eq!(image.requests.lock().unwrap().len(), 1);
    let multipart = ServiceFixture::with_transport(true, "image-api", Some("multipart")).await;
    let refused = multipart
        .provider
        .rig
        .send(
            &multipart.provider.provider_key(),
            &multipart.command("private", "edit input"),
        )
        .await;
    assert!(matches!(refused, Progress::Failed(_)), "{refused:?}");
    assert!(multipart.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn declared_multipart_consumes_exact_source_blobs_and_counts_actual_decoded_outputs() {
    let fixture = ServiceFixture::with_binding(true, "image-api", Some("multipart"), |binding| {
        binding.request["path"] = json!("/multipart");
        binding.request["body"] =
            json!({"prompt":{"from_input":"/query"},"image":["source"],"n":9});
        binding.request["array_fields"] = json!({"image":"image[]"});
        binding.counters.insert(
            "image_generation".into(),
            Counter {
                unit: "image".into(),
                items: Some(orch_provider_contract::capabilities::CountItems {
                    pointer: "/data".into(),
                    base64: "/b64_json".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
    })
    .await;
    let bytes = bytes::Bytes::from_static(b"actual native source image");
    let blob = fixture.provider.rig.blobs.insert(bytes.clone());
    let mut request = fixture.command("private", "edit this input");
    request
        .input_images
        .push(orch_model_contract::generation::GenerationPart::ImageRef {
            blob,
            size: bytes.len() as i64,
            media_type: "image/png".into(),
            name: "source".into(),
            url: String::new(),
        });
    let id = new_id();
    fixture.provider.rig.blobs.open_command(id);
    fixture.provider.rig.controller.issue_with_id(
        id,
        &fixture.provider.provider_key(),
        fixture.provider.rig.deadline(),
        &request,
    );
    let result = match tokio::time::timeout(
        fixture.provider.rig.patience,
        fixture.provider.rig.controller.wait_command(id),
    )
    .await
    .unwrap()
    {
        Progress::Succeeded(result) => result,
        other => panic!("the materialized request failed: {other:?}"),
    };
    assert_eq!(
        result["counters"]["image_generation"], 2,
        "requested n=9 is not a measurement"
    );
    {
        let requests = fixture.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let (headers, body) = &requests[0];
        assert!(
            headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with("multipart/form-data; boundary=")
        );
        assert_eq!(
            headers["authorization"].to_str().unwrap(),
            format!("Bearer {}", fixture.provider.key)
        );
        let raw = body["raw"].as_str().unwrap();
        assert!(raw.contains("name=\"image[]\""));
        assert!(raw.contains("actual native source image"));
        assert!(raw.contains("edit this input"));
    }
    request.input_images.clear();
    let refused = fixture
        .provider
        .rig
        .send(&fixture.provider.provider_key(), &request)
        .await;
    assert!(matches!(refused, Progress::Failed(_)), "{refused:?}");
    assert_eq!(
        fixture.requests.lock().unwrap().len(),
        1,
        "missing materialization cannot make another paid HTTP request"
    );
}
