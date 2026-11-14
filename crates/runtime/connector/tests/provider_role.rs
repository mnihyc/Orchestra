//! The provider role as the system drives it: a provider started with a key that arrives sealed, kept sealed and found again after a restart, forgotten when it is stopped, a key that a person gave to the machine, an upstream that takes none, the models and the state of a provider, who answers for a provider on a machine that is both a provider's runtime and a harness's.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
#[path = "provider_role/native_capability.rs"]
mod native_capability;
#[path = "provider_role/service.rs"]
mod service;
#[path = "provider_role/standalone_search.rs"]
mod standalone_search;

use std::sync::Arc;

use common::model::{MESSAGES, MODEL, ProviderRig, RESPONSES, files_holding, responses_request};
use orch_connector_core::credentials::{
    SecretStore, local_key_name, provider_custodian_name, provider_record_name,
};
use orch_connector_core::roles::Keys;
use orch_link::commands::Progress;
use orch_link_contract::commands::{
    CompletionPurpose, LeaseClose, ProviderComplete, ProviderDiscoverModels, ProviderRefresh,
    ProviderStop, SerialKey,
};
use orch_testkit::doubles::model::Verdict;
use orch_types::new_id;
use serde_json::json;

fn failed_with(progress: &Progress, code: &str) -> bool {
    matches!(progress, Progress::Failed(error) if error.code == code)
}

#[tokio::test]
async fn owned_generation_streams_and_returns_tools_without_exporting_the_provider_key() {
    use common::model::CHAT;
    use orch_link_contract::commands::ProviderGenerate;
    use orch_model_contract::generation::{
        GenerationInput, GenerationMessage, GenerationPart, GenerationRole, GenerationTool,
    };
    let p = ProviderRig::new().await;
    let started = p
        .start_provider(&[
            (RESPONSES, p.mock.responses_base_url(), json!({})),
            (CHAT, p.mock.responses_base_url(), json!({})),
            (MESSAGES, p.mock.messages_base_url(), json!({})),
        ])
        .await;
    assert!(matches!(started, Progress::Succeeded(_)));
    for protocol in [RESPONSES, CHAT, MESSAGES] {
        p.mock
            .queue("say Reading the source.\ntool read {\"operation\":\"ThreadGet\"}\nusage 100 20")
            .unwrap();
        let request = ProviderGenerate {
            native_capability: None,
            capture_native_result: false,
            provider_id: p.rig.provider,
            protocol: protocol.into(),
            model_name: MODEL.into(),
            params: None,
            model_parameters: common::model::enabled_parameters(),
            input: GenerationInput {
                system: "Use only the supplied read tools.".into(),
                messages: vec![GenerationMessage {
                    role: GenerationRole::User,
                    content: vec![GenerationPart::Text {
                        text: "Read this thread".into(),
                    }],
                    continuation: vec![],
                }],
                tools: vec![GenerationTool {
                    name: "read".into(),
                    description: "Read a permitted operation".into(),
                    parameters: json!({"type":"object","properties":{"operation":{"type":"string"}},"required":["operation"]}),
                }],
                max_output_tokens: 128,
            },
        };
        assert!(!serde_json::to_string(&request).unwrap().contains(&p.key));
        let reply = p.rig.succeed(&SerialKey::None, &request).await;
        assert_eq!(reply["output"]["stop"], "tools");
        assert_eq!(reply["usage"]["input_tokens"], 100);
        assert_eq!(reply["usage"]["output_tokens"], 20);
        assert!(!reply.to_string().contains(&p.key));
    }
    let events = p.wait_provider_event("provider.generation.delta", 3).await;
    assert!(
        events
            .iter()
            .all(|event| !event.body.to_string().contains(&p.key))
    );
    assert!(
        p.model_requests()
            .iter()
            .all(|request| request.credential.verdict == Verdict::Accepted)
    );
    assert!(files_holding(p.rig.data.path(), &p.key).is_empty());
}

#[tokio::test]
async fn synthetic_capacity_access_and_successful_refusals_share_the_same_classifier() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    for (protocol, script, code, reason, delay) in [
        (
            RESPONSES,
            "error overloaded retry-after=7",
            "overloaded",
            None,
            Some(7),
        ),
        (
            RESPONSES,
            r#"body 200 {"error":{"type":"overloaded_error","message":"Model at capacity"}}"#,
            "overloaded",
            None,
            None,
        ),
        (
            RESPONSES,
            "error invalid_request status=403 Daybreak isn't available for this model",
            "model_unavailable",
            Some("access"),
            None,
        ),
        (
            MESSAGES,
            "say Request declined\nstop refusal\nusage 100 5",
            "model_refused",
            Some("safety"),
            None,
        ),
        (
            RESPONSES,
            r#"body 200 {"status":"completed","output":[{"type":"message","content":[{"type":"refusal","refusal":"Safety policy"}]}],"usage":{"input_tokens":100,"output_tokens":5,"total_tokens":105}}"#,
            "model_refused",
            Some("safety"),
            None,
        ),
    ] {
        p.mock.queue(script).unwrap();
        let result = p
            .rig
            .send(
                &p.provider_key(),
                &ProviderComplete {
                    model_parameters: Some(common::model::enabled_parameters()),
                    params: None,
                    provider_id: p.rig.provider,
                    purpose: CompletionPurpose::Summary,
                    protocol: protocol.to_owned(),
                    model_name: MODEL.to_owned(),
                    system: "Summarize".to_owned(),
                    input: "Synthetic work".to_owned(),
                    max_output_tokens: 120,
                },
            )
            .await;
        let Progress::Failed(error) = result else {
            panic!("a synthetic refusal must remain a recoverable failure");
        };
        assert_ne!(error.code, "login_required");
        let details = error.details.unwrap();
        assert_eq!(details["failure"]["code"], code);
        assert_eq!(details["failure"]["reason"].as_str(), reason);
        assert_eq!(details["failure"]["retry_after_secs"].as_i64(), delay);
        let request: orch_agent_contract::requests::UsageRequest =
            serde_json::from_value(details["model_request"].clone()).unwrap();
        request.check().unwrap();
        assert_eq!(request.outcome.as_ref().unwrap().code(), code);
        if code == "model_refused" {
            assert_eq!(request.tokens.output_tokens, Some(5));
        }
    }
    assert_eq!(
        p.model_requests().len(),
        5,
        "only the five scripted failures reached the model server"
    );
}

#[tokio::test]
async fn a_system_completion_uses_the_providers_key_without_a_turn_and_returns_text_and_usage_in_both_formats()
 {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    for protocol in [RESPONSES, MESSAGES] {
        p.mock
            .queue("say A concise summary.\nusage 100 20 cached=10 cache_write=5")
            .unwrap();
        let reply = p
            .rig
            .succeed(
                &p.provider_key(),
                &ProviderComplete {
                    model_parameters: Some(common::model::enabled_parameters()),
                    params: None,
                    provider_id: p.rig.provider,
                    purpose: CompletionPurpose::Summary,
                    protocol: protocol.to_owned(),
                    model_name: MODEL.to_owned(),
                    system: "Summarize the input.".to_owned(),
                    input: "The work is ready.".to_owned(),
                    max_output_tokens: 120,
                },
            )
            .await;
        assert_eq!(reply["text"], "A concise summary.");
        assert_eq!(reply["usage"]["input_tokens"], 100);
        assert_eq!(reply["usage"]["output_tokens"], 20);
        assert_eq!(reply["usage"]["cache_write_tokens"], 5);
        assert_eq!(
            reply["timing"]["outcome"],
            json!({"state":"told","value":"done"})
        );
        assert_eq!(reply["timing"]["request_ms"]["state"], "told");
        assert_eq!(reply["timing"]["first_token_ms"]["state"], "unsupported");
        assert_eq!(reply["timing"]["speed_milli"]["state"], "unsupported");
        assert!(
            reply["request_ref"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
        );
    }
    let requests = p.model_requests();
    assert_eq!(
        requests.len(),
        2,
        "one model request per command, without a forwarded turn"
    );
    assert!(
        requests
            .iter()
            .all(|request| request.credential.verdict == Verdict::Accepted)
    );
    assert_eq!(
        requests[0].body["store"], false,
        "own Responses requests explicitly refuse retention"
    );
    assert!(
        requests[1].body.get("store").is_none(),
        "other interfaces retain their native shape"
    );
    assert!(files_holding(p.rig.data.path(), &p.key).is_empty());
}

#[tokio::test]
async fn a_completion_does_not_return_an_upstreams_key_or_error_body() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.mock
        .set_credential(Some("a-key-the-provider-does-not-have"));
    let result = p
        .rig
        .send(
            &p.provider_key(),
            &ProviderComplete {
                model_parameters: Some(common::model::enabled_parameters()),
                params: None,
                provider_id: p.rig.provider,
                purpose: CompletionPurpose::Summary,
                protocol: RESPONSES.to_owned(),
                model_name: MODEL.to_owned(),
                system: "Summarize.".to_owned(),
                input: "Some work.".to_owned(),
                max_output_tokens: 120,
            },
        )
        .await;
    assert!(failed_with(&result, "login_required"));
    let Progress::Failed(error) = &result else {
        panic!("the upstream must refuse this credential");
    };
    let raw: orch_agent_contract::requests::UsageRequest =
        serde_json::from_value(error.details.as_ref().unwrap()["model_request"].clone()).unwrap();
    raw.check().unwrap();
    let measured = raw.measured();
    assert_eq!(
        measured.outcome.value().map(String::as_str),
        Some("provider_unauthorized")
    );
    assert!(measured.request_ms.value().is_some());
    assert!(measured.speed_milli.value().is_none());
    assert!(!format!("{result:?}").contains(&p.key));
}

#[tokio::test]
async fn a_provider_started_before_a_restart_serves_again_without_being_started_and_its_key_is_in_no_file_in_clear()
 {
    let mut p = ProviderRig::new().await;
    p.start_responses_provider().await;
    let data = p.rig.data.path().to_owned();
    assert!(
        data.join("secrets")
            .join(provider_record_name(p.rig.provider))
            .is_file()
    );
    assert!(files_holding(&data, &p.key).is_empty());

    let lease = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let before = p
        .harness(p.thread)
        .post("/v1/responses", &responses_request(MODEL, "before restart"))
        .await;
    assert_eq!(before.status, 200);

    p.rig.kill().await;
    p.rig.start().await;

    // Neither provider.start nor lease.open: both existing identities and accounting survive.
    let reply = p
        .harness(p.thread)
        .post("/v1/responses", &responses_request(MODEL, "hello"))
        .await;
    assert_eq!(reply.status, 200);
    let requests = p.model_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].credential.verdict, Verdict::Accepted);
    assert!(
        files_holding(&data, &p.key).is_empty(),
        "the key is in no file in clear"
    );
    assert_eq!(p.close_lease(lease).await["requests"], 2);
}

#[tokio::test]
async fn a_provider_that_is_stopped_is_forgotten_with_its_key_and_stays_forgotten_after_a_restart()
{
    let mut p = ProviderRig::new().await;
    p.start_responses_provider().await;
    let lease = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let harness = p.harness(p.thread);
    assert_eq!(
        harness
            .post("/v1/responses", &responses_request(MODEL, "hi"))
            .await
            .status,
        200
    );

    let stopped = p
        .rig
        .send(
            &p.provider_key(),
            &ProviderStop {
                provider_id: p.rig.provider,
                purge: None,
            },
        )
        .await;
    assert!(matches!(stopped, Progress::Succeeded(_)), "{stopped:?}");
    // Its leases are closed with it, and a call under one is refused.
    let after = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(after.status, 403);
    assert_eq!(after.json()["error"]["code"], "lease_closed");
    assert!(
        !p.rig
            .data
            .path()
            .join("secrets")
            .join(provider_record_name(p.rig.provider))
            .exists()
    );
    // It is not there to lease.
    let (_, progress) = p
        .try_open_lease(new_id(), &[MODEL], Some(RESPONSES), None)
        .await;
    assert!(failed_with(&progress, "not_found"), "{progress:?}");

    p.rig.kill().await;
    p.rig.start().await;
    let (_, progress) = p
        .try_open_lease(new_id(), &[MODEL], Some(RESPONSES), None)
        .await;
    assert!(failed_with(&progress, "not_found"), "{progress:?}");
    // The lease that was open is still answered, as it was: closed, with what it used.
    let closed = p
        .rig
        .succeed(&p.provider_key(), &LeaseClose { lease_id: lease })
        .await;
    assert_eq!(
        closed["requests"], 1,
        "the closed lease retains its counted request across the machine's restart"
    );
}

#[tokio::test]
async fn a_pause_keeps_the_local_credential_and_a_delete_erases_only_that_providers_credential() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    let keys = Arc::new(Keys::new(p.rig.identity.key.duplicate().unwrap()));
    let store = SecretStore::open(p.rig.data.path(), keys).unwrap();
    let other = new_id();
    for provider in [p.rig.provider, other] {
        store.put(&local_key_name(provider), "local-key").unwrap();
        store
            .put(&provider_custodian_name(provider), "custodian")
            .unwrap();
    }
    p.rig
        .succeed(
            &p.provider_key(),
            &ProviderStop {
                provider_id: p.rig.provider,
                purge: None,
            },
        )
        .await;
    assert_eq!(
        store
            .get(&local_key_name(p.rig.provider))
            .unwrap()
            .as_deref(),
        Some("local-key")
    );
    assert_eq!(
        store
            .get(&provider_custodian_name(p.rig.provider))
            .unwrap()
            .as_deref(),
        Some("custodian")
    );
    p.rig
        .succeed(
            &p.provider_key(),
            &ProviderStop {
                provider_id: p.rig.provider,
                purge: Some(true),
            },
        )
        .await;
    assert!(
        store
            .get(&local_key_name(p.rig.provider))
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get(&provider_custodian_name(p.rig.provider))
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get(&provider_record_name(p.rig.provider))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.get(&local_key_name(other)).unwrap().as_deref(),
        Some("local-key")
    );
    assert_eq!(
        store
            .get(&provider_custodian_name(other))
            .unwrap()
            .as_deref(),
        Some("custodian")
    );
}

#[tokio::test]
async fn a_key_kept_on_the_machine_is_used_when_the_system_sends_none() {
    let p = ProviderRig::new().await;
    // Nothing is kept yet: the provider is started without a key, and a call finds none to present.
    let progress = p
        .start_provider_with_key(&[(RESPONSES, p.mock.responses_base_url(), json!({}))], None)
        .await;
    assert!(matches!(progress, Progress::Succeeded(_)), "{progress:?}");
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let harness = p.harness(p.thread);
    let without = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(without.status, 503, "{}", without.text());
    assert_eq!(without.json()["error"]["code"], "provider_unavailable");
    assert!(without.header("retry-after").is_some());
    assert!(p.model_requests().is_empty());

    // A person gives the machine the key (as `orch-connector provider set-key` does, in a process of its own).
    let keys = Arc::new(Keys::new(p.rig.identity.key.duplicate().unwrap()));
    SecretStore::open(p.rig.data.path(), keys)
        .unwrap()
        .put(&local_key_name(p.rig.provider), &p.key)
        .unwrap();
    let with = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(with.status, 200, "{}", with.text());
    assert_eq!(p.model_requests()[0].credential.verdict, Verdict::Accepted);
    assert!(
        files_holding(p.rig.data.path(), &p.key).is_empty(),
        "kept sealed, never in clear"
    );
}

#[tokio::test]
async fn an_upstream_that_takes_no_key_is_served_with_none_and_the_harnesss_token_goes_no_further()
{
    let p = ProviderRig::with(|_, mock| mock.set_credential(None)).await;
    let progress = p
        .start_provider_with_key(
            &[(
                RESPONSES,
                p.mock.responses_base_url(),
                json!({ "auth": "none" }),
            )],
            None,
        )
        .await;
    assert!(matches!(progress, Progress::Succeeded(_)), "{progress:?}");
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let harness = p.harness(p.thread);
    let reply = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let seen = &p.model_requests()[0];
    assert!(
        !seen.headers.contains_key("authorization") && !seen.headers.contains_key("x-api-key"),
        "the upstream was shown no credential: {:?}",
        seen.headers
    );
    assert!(
        !serde_json::to_string(&seen.headers)
            .unwrap()
            .contains(&harness.token)
    );
}

#[tokio::test]
async fn the_models_of_an_api_provider_are_listed_from_its_endpoints_and_its_state_says_whether_its_key_is_taken()
 {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    let key = p.provider_key();
    let provider_id = p.rig.provider;

    let listed = p
        .rig
        .succeed(
            &key,
            &ProviderDiscoverModels {
                provider_id,
                models: None,
            },
        )
        .await;
    let names: Vec<String> = listed["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|suggestion| suggestion["model_name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        ["mock-small", "mock-large"],
        "once each, though two endpoints list them"
    );

    let state = p
        .rig
        .succeed(
            &key,
            &ProviderRefresh {
                provider_id,
                retry: None,
                renew: None,
                facts: None,
            },
        )
        .await;
    assert_eq!(state["status"], "ready");
    assert_eq!(state["facts"]["credential"]["yields"], "key");

    // An unknown vendor has no key-usage read: refreshing does not probe the key with a model-list request.
    p.mock
        .set_credential(Some("a-key-the-provider-does-not-have"));
    let state = p
        .rig
        .succeed(
            &key,
            &ProviderRefresh {
                provider_id,
                retry: None,
                renew: None,
                facts: None,
            },
        )
        .await;
    assert_eq!(state["status"], "ready");
    assert_eq!(state["source"]["supported"], false);
    assert!(state["facts"]["key_usage_supported"].is_null());
    let refused = p
        .rig
        .succeed(
            &key,
            &ProviderDiscoverModels {
                provider_id,
                models: None,
            },
        )
        .await;
    assert_eq!(refused["source"]["error"]["code"], "refused");
    assert_eq!(refused["suggestions"], json!([]));
    let events = p.wait_provider_event("provider.status", 1).await;
    assert_eq!(events[0].body["status"], "error");
}

#[tokio::test]
async fn discovery_follows_the_second_models_page_and_returns_every_model_once() {
    use orch_testkit::doubles::model::MockModel;
    // A catalogue just larger than the protocol's 100-entry page exercises a real second page.
    const MODELS: usize = 150;
    let p = ProviderRig::with(|_, mock| {
        mock.set_models(
            (0..MODELS)
                .map(|number| MockModel::named(&format!("model-{number}")))
                .collect(),
        );
    })
    .await;
    let started = p
        .start_provider(&[(MESSAGES, p.mock.messages_base_url(), json!({}))])
        .await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    let listed = p
        .rig
        .succeed(
            &p.provider_key(),
            &ProviderDiscoverModels {
                provider_id: p.rig.provider,
                models: None,
            },
        )
        .await;
    let names: std::collections::BTreeSet<_> = listed["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|suggestion| suggestion["model_name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), MODELS);
    assert!(names.contains("model-0") && names.contains("model-149"));
    let requests = p.mock.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .query
            .as_deref()
            .unwrap()
            .contains("after_id=model-99")
    );
    assert!(
        requests
            .iter()
            .all(|request| request.credential.verdict == Verdict::Accepted)
    );
}

#[tokio::test]
async fn a_models_list_larger_than_a_frame_of_the_link_reaches_the_system_whole() {
    use orch_provider_contract::limits::TEXT_MAX_CHARS;
    use orch_testkit::doubles::model::MockModel;
    // A real account's list ran to some 440 000 bytes, which the link threw away, and the system waited out
    // the command's thirty seconds and said that the keeping machine could not finish. A routing name is at
    // most `TEXT_MAX_CHARS` characters long, and the suggestion carries it twice (as the name and as the
    // name to show), so a list of this many longest names is larger than a frame of the link.
    const MODELS: usize = 1_000;
    let name = |number: usize| {
        let head = format!("model-{number:05}-");
        format!("{head}{}", "x".repeat(TEXT_MAX_CHARS - 8 - head.len()))
    };
    let p = ProviderRig::with(|_, mock| {
        mock.set_models(
            (0..MODELS)
                .map(|number| MockModel::named(&name(number)))
                .collect(),
        );
    })
    .await;
    let started = p
        .start_provider(&[(MESSAGES, p.mock.messages_base_url(), json!({}))])
        .await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    let listed = p
        .rig
        .succeed(
            &p.provider_key(),
            &ProviderDiscoverModels {
                provider_id: p.rig.provider,
                models: None,
            },
        )
        .await;
    assert!(
        serde_json::to_vec(&listed).unwrap().len() > 262_144,
        "the answer is larger than a frame: {}",
        listed["source"]
    );
    assert!(
        listed["source"]["error"].is_null(),
        "and nothing was left out of it: {}",
        listed["source"]
    );
    let names: std::collections::BTreeSet<_> = listed["suggestions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|suggestion| suggestion["model_name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), MODELS, "every model, once");
    assert!(names.contains(name(MODELS - 1).as_str()));
}

#[tokio::test]
async fn discovery_refuses_cycles_foreign_next_links_and_catalogues_beyond_its_page_or_model_bound()
{
    use axum::extract::State;
    use orch_model_contract::protocol::{MODEL_DISCOVERY_MAX_MODELS, MODEL_DISCOVERY_MAX_PAGES};
    use std::sync::atomic::{AtomicUsize, Ordering};
    for mode in ["cycle", "foreign", "pages", "models"] {
        let hits = Arc::new(AtomicUsize::new(0));
        let state = (mode, Arc::clone(&hits));
        let app = axum::Router::new().fallback(|State((mode, hits)): State<(&'static str, Arc<AtomicUsize>)>| async move {
            let number = hits.fetch_add(1, Ordering::SeqCst);
            let page = match mode {
                "cycle" => json!({"data":[{"id":"one"}], "next":"/v1/models?limit=100"}),
                "foreign" => json!({"data":[{"id":"one"}], "next":"https://foreign.example/v1/models"}),
                "models" => json!({"data":(0..=MODEL_DISCOVERY_MAX_MODELS).map(|i| json!({"id":format!("model-{i}")})).collect::<Vec<_>>()}),
                _ => json!({"data":[{"id":format!("model-{number}")}], "has_more":true, "last_id":format!("model-{number}")}),
            };
            axum::Json(page)
        }).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut p = ProviderRig::with(|rig, _| {
            rig.config.provider.upstreams = vec![base.clone()];
        })
        .await;
        let started = p
            .start_provider(&[(MESSAGES, base, json!({"auth":"none"}))])
            .await;
        assert!(
            matches!(started, Progress::Succeeded(_)),
            "{mode}: {started:?}"
        );
        let result = p
            .rig
            .send(
                &p.provider_key(),
                &ProviderDiscoverModels {
                    provider_id: p.rig.provider,
                    models: None,
                },
            )
            .await;
        let Progress::Succeeded(result) = result else {
            panic!("{mode}: a source read carries its failure in the answer: {result:?}");
        };
        assert!(result["source"]["error"].is_object(), "{mode}: {result:?}");
        assert_eq!(result["suggestions"], json!([]), "{mode}: {result:?}");
        assert_ne!(result["source"]["supported"], true, "{mode}: {result:?}");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            if mode == "pages" {
                MODEL_DISCOVERY_MAX_PAGES
            } else {
                1
            }
        );
        p.rig.kill().await;
        server.abort();
    }
}

#[tokio::test]
async fn a_provider_whose_key_the_upstream_refuses_is_in_error_for_the_system_until_a_call_is_taken_again()
 {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let harness = p.harness(p.thread);
    p.mock.set_credential(Some("rotated-elsewhere"));
    let refused = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(
        refused.status, 401,
        "the upstream's own answer reaches the harness"
    );
    let statuses = p.wait_provider_event("provider.status", 1).await;
    assert_eq!(statuses[0].body["status"], "error");
    // Once, however many calls are refused.
    let _ = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    p.mock.set_credential(Some(&p.key));
    let taken = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(taken.status, 200);
    let statuses = p.wait_provider_event("provider.status", 2).await;
    assert_eq!(statuses.len(), 2, "{statuses:?}");
    assert_eq!(statuses[1].body["status"], "ready");
}

#[tokio::test]
async fn a_machine_that_is_both_answers_for_a_provider_from_whichever_holds_it() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    // Another provider is a subscription: a harness of this machine holds its sign-in.
    let subscription = new_id();
    let agent = new_id();
    let started: orch_link_contract::commands::AgentStart = serde_json::from_value(json!({
        "provider_id": subscription, "agent_id": agent, "harness_kind": "fake",
        "profile": { "harness_version": null, "config": {}, "env": {} }, "home": null
    }))
    .unwrap();
    p.rig
        .succeed(
            &SerialKey::Harness {
                provider_id: subscription,
                agent_id: agent,
            },
            &started,
        )
        .await;
    let provider_key = |id| SerialKey::Provider(id);

    // The API provider is answered by the provider role.
    let api = p
        .rig
        .succeed(
            &provider_key(p.rig.provider),
            &ProviderRefresh {
                provider_id: p.rig.provider,
                retry: None,
                renew: None,
                facts: None,
            },
        )
        .await;
    assert_eq!(api["status"], "ready");
    assert_eq!(api["facts"]["credential"]["yields"], "key");

    // The subscription by the harness that holds its sign-in: it is not signed in yet.
    let signed_out = p
        .rig
        .succeed(
            &provider_key(subscription),
            &ProviderRefresh {
                provider_id: subscription,
                retry: None,
                renew: None,
                facts: None,
            },
        )
        .await;
    assert_eq!(signed_out["status"], "needs_setup");

    // A provider neither holds is nobody's.
    let nobody = new_id();
    assert_eq!(
        p.rig
            .fail(
                &provider_key(nobody),
                &ProviderRefresh {
                    provider_id: nobody,
                    retry: None,
                    renew: None,
                    facts: None,
                }
            )
            .await,
        "not_found"
    );
    assert_eq!(
        p.rig
            .fail(
                &provider_key(nobody),
                &ProviderDiscoverModels {
                    provider_id: nobody,
                    models: None,
                }
            )
            .await,
        "not_found"
    );
}

#[tokio::test]
async fn a_lease_is_opened_only_for_what_the_machine_holds_and_can_be_held() {
    let p = ProviderRig::new().await;
    // No provider yet.
    let (_, progress) = p
        .try_open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    assert!(failed_with(&progress, "not_found"), "{progress:?}");
    p.start_responses_provider().await;
    // A protocol this machine has no implementation of, or no endpoint speaks.
    let (_, progress) = p
        .try_open_lease(p.thread, &[MODEL], Some("acme-chat"), None)
        .await;
    assert!(failed_with(&progress, "unsupported"), "{progress:?}");
    let (_, progress) = p
        .try_open_lease(p.thread, &[MODEL], Some("anthropic-messages"), None)
        .await;
    assert!(failed_with(&progress, "unsupported"), "{progress:?}");
    // An allowance that cannot be counted.
    let (_, progress) = p
        .try_open_lease(
            p.thread,
            &[MODEL],
            Some(RESPONSES),
            Some(json!({ "credits": { "amount": "plenty", "rate": {
                "input_per_mtok": "1", "cached_input_per_mtok": "1", "output_per_mtok": "1" } } })),
        )
        .await;
    assert!(failed_with(&progress, "internal"), "{progress:?}");
    // A good one, and a second open of the same lease is the same lease.
    let lease = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let closed = p.close_lease(lease).await;
    assert_eq!(closed["requests"], 0);
    assert_eq!(
        closed["usage"],
        json!(null),
        "nothing was used, and that is not a count of nought"
    );
    assert_eq!(
        p.close_lease(lease).await,
        closed,
        "a lease closed again says the same"
    );
}

#[tokio::test]
async fn neutral_effort_reaches_the_protocol_without_replacing_routing_or_output_bounds() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    for (protocol, path) in [
        (RESPONSES, "/reasoning/effort"),
        (MESSAGES, "/output_config/effort"),
    ] {
        p.mock.queue("say A summary\nusage 100 5").unwrap();
        let request = ProviderComplete {
            model_parameters: Some(common::model::enabled_parameters()),
            provider_id: p.rig.provider,
            purpose: CompletionPurpose::Summary,
            protocol: protocol.to_owned(),
            model_name: MODEL.to_owned(),
            system: "Summarize".to_owned(),
            input: "Synthetic work".to_owned(),
            max_output_tokens: 120,
            params: Some(json!({"effort":"high"})),
        };
        p.rig.succeed(&p.provider_key(), &request).await;
        let observed = p.model_requests();
        let body = &observed.last().unwrap().body;
        assert_eq!(body.pointer(path), Some(&json!("high")));
        assert_eq!(body["model"], MODEL);
        assert!(
            body.get("effort").is_none(),
            "neutral fields are not sent as unknown vendor options"
        );
        for params in [
            json!({"model":"other"}),
            json!({"max_output_tokens":99999}),
            json!({"effort":"low","reasoning":{"effort":"high"}}),
            json!({"tools":[{"type":"web_search"}]}),
            json!({"store":true}),
        ] {
            let result = p
                .rig
                .send(
                    &p.provider_key(),
                    &ProviderComplete {
                        model_parameters: Some(common::model::enabled_parameters()),
                        params: Some(params),
                        ..request.clone()
                    },
                )
                .await;
            assert!(matches!(result, Progress::Failed(_)), "{result:?}");
        }
    }
    assert_eq!(
        p.model_requests().len(),
        2,
        "invalid parameters cannot reach the model server"
    );
}

#[tokio::test]
async fn responses_fast_options_reach_the_actual_upstream_with_unknown_json_intact() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    let mut request = ProviderComplete {
        model_parameters: Some(common::model::enabled_parameters()),
        provider_id: p.rig.provider,
        purpose: CompletionPurpose::Summary,
        protocol: RESPONSES.to_owned(),
        model_name: MODEL.to_owned(),
        system: "Summarize only this material".to_owned(),
        input: "Synthetic work".to_owned(),
        max_output_tokens: 120,
        params: None,
    };
    p.mock.queue("say A summary\nusage 100 5").unwrap();
    p.rig.succeed(&p.provider_key(), &request).await;
    assert_eq!(p.model_requests().len(), 1, "ordinary same-path completion");
    let extra = json!({"enabled":true,"levels":[1,2],"reset":null});
    request.params = Some(json!({"service_tier":"fast","future_vendor_option":extra}));
    p.mock.queue("say A fast summary\nusage 100 5").unwrap();
    p.rig.succeed(&p.provider_key(), &request).await;
    let observed = p.model_requests();
    assert_eq!(observed.len(), 2);
    let body = &observed[1].body;
    assert_eq!(body["service_tier"], "fast");
    assert_eq!(body["future_vendor_option"], extra);
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["instructions"], request.system);
    assert_eq!(body["input"], request.input);
    assert_eq!(body["max_output_tokens"], 120);
    assert_eq!(body["stream"], false);
}

#[tokio::test]
async fn messages_fast_is_speed_and_does_not_borrow_the_capacity_service_tier() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    let mut request = ProviderComplete {
        model_parameters: Some(common::model::enabled_parameters()),
        provider_id: p.rig.provider,
        purpose: CompletionPurpose::Summary,
        protocol: MESSAGES.to_owned(),
        model_name: MODEL.to_owned(),
        system: "Summarize only this material".to_owned(),
        input: "Synthetic work".to_owned(),
        max_output_tokens: 120,
        params: None,
    };
    p.mock.queue("say A summary\nusage 100 5").unwrap();
    p.rig.succeed(&p.provider_key(), &request).await;
    assert_eq!(p.model_requests().len(), 1, "ordinary same-path completion");
    request.params = Some(json!({"speed":"fast","thinking":{"type":"disabled"}}));
    p.mock.queue("say A fast summary\nusage 100 5").unwrap();
    p.rig.succeed(&p.provider_key(), &request).await;
    let observed = p.model_requests();
    assert_eq!(observed.len(), 2);
    let body = &observed[1].body;
    assert_eq!(body["speed"], "fast");
    assert!(body.get("service_tier").is_none(), "capacity is not speed");
    assert_eq!(body["thinking"], json!({"type":"disabled"}));
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["system"], request.system);
    assert_eq!(body["messages"][0]["content"], request.input);
    assert_eq!(body["max_tokens"], 120);
    assert_eq!(body["stream"], false);
}

#[tokio::test]
async fn endpoint_object_defaults_keep_nested_json_and_follow_later_explicit_requests() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let harness = p.harness(p.thread);
    p.mock.queue("say Baseline\nusage 100 5").unwrap();
    let reply = harness
        .post("/v1/responses", &responses_request(MODEL, "baseline"))
        .await;
    assert_eq!(
        reply.status,
        200,
        "ordinary same-path forwarding: {}",
        reply.text()
    );
    let future = json!({"enabled":true,"levels":[1,2],"reset":null});
    let started = p
        .start_provider(&[(
            RESPONSES,
            p.mock.responses_base_url(),
            json!({
                "params":{"temperature":0.2,"future_vendor_option":future},
                "headers":{"x-configured-feature":"test-feature"}
            }),
        )])
        .await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    // Keep the same real lease: request options are read from each request, not its first choice.
    for temperature in [0.7, 0.4] {
        let mut body = responses_request(MODEL, "configured use");
        body["temperature"] = json!(temperature);
        p.mock.queue("say A summary\nusage 100 5").unwrap();
        let reply = harness.post("/v1/responses", &body).await;
        assert_eq!(reply.status, 200, "{}", reply.text());
        let seen = p.model_requests();
        let last = seen.last().unwrap();
        assert_eq!(last.body["temperature"], json!(temperature));
        assert_eq!(last.body["future_vendor_option"], future);
        assert_eq!(last.body["model"], body["model"]);
        assert_eq!(last.body["input"], body["input"]);
        assert_eq!(
            last.headers.get("x-configured-feature").map(String::as_str),
            Some("test-feature")
        );
    }
    p.mock.queue("say Default summary\nusage 100 5").unwrap();
    let reply = harness
        .post("/v1/responses", &responses_request(MODEL, "default use"))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        p.model_requests().last().unwrap().body["temperature"],
        json!(0.2)
    );
}

#[tokio::test]
async fn responses_processing_tier_records_actual_downgrade_and_missing_report_separately() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    let mut request = ProviderComplete {
        model_parameters: Some(common::model::enabled_parameters()),
        provider_id: p.rig.provider,
        purpose: CompletionPurpose::Summary,
        protocol: RESPONSES.to_owned(),
        model_name: MODEL.to_owned(),
        system: "Summarize".to_owned(),
        input: "Synthetic work".to_owned(),
        max_output_tokens: 120,
        params: None,
    };
    p.mock.queue("say Ordinary\nusage 100 5").unwrap();
    p.rig.succeed(&p.provider_key(), &request).await;
    assert_eq!(p.model_requests().len(), 1, "ordinary same-path completion");
    request.params = Some(json!({"service_tier":"fast"}));
    for actual in [Some("priority"), Some("default"), None] {
        let mut response = json!({"id":"response-tier-control","status":"completed","model":MODEL,
            "output":[{"type":"message","content":[{"type":"output_text","text":"A summary"}]}],
            "usage":{"input_tokens":100,"output_tokens":5}});
        if let Some(actual) = actual {
            response["service_tier"] = json!(actual);
        }
        p.mock.queue(&format!("body 200 {response}")).unwrap();
        let answer = p.rig.succeed(&p.provider_key(), &request).await;
        assert_eq!(answer["timing"]["requested_service_tier"], "fast");
        match actual {
            Some(actual) => {
                assert_eq!(answer["timing"]["reported_service_tier"]["state"], "told");
                assert_eq!(answer["timing"]["reported_service_tier"]["value"], actual);
            }
            None => assert_eq!(answer["timing"]["reported_service_tier"]["state"], "empty"),
        }
    }
    assert_eq!(p.model_requests().len(), 4);
}

#[tokio::test]
async fn messages_report_inference_speed_independently_from_capacity_class() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    let mut request = ProviderComplete {
        model_parameters: Some(common::model::enabled_parameters()),
        provider_id: p.rig.provider,
        purpose: CompletionPurpose::Summary,
        protocol: MESSAGES.to_owned(),
        model_name: MODEL.to_owned(),
        system: "Summarize".to_owned(),
        input: "Synthetic work".to_owned(),
        max_output_tokens: 120,
        params: None,
    };
    p.mock.queue("say Ordinary\nusage 100 5").unwrap();
    p.rig.succeed(&p.provider_key(), &request).await;
    request.params = Some(json!({"speed":"fast","service_tier":"auto"}));
    let refused = p.rig.send(&p.provider_key(), &request).await;
    assert!(
        matches!(refused,Progress::Failed(ref error) if error.code=="conflict" && error.detail.contains("capacity tier")),
        "{refused:?}"
    );
    assert_eq!(
        p.model_requests().len(),
        1,
        "an unenabled capacity choice never reaches the endpoint"
    );
    request.model_parameters.as_mut().unwrap().capacity_tiers =
        Some(serde_json::from_value(json!([{"id":"auto","label":"Automatic"}])).unwrap());
    let response = json!({"id":"message-speed-control","type":"message","role":"assistant","model":MODEL,
        "content":[{"type":"text","text":"A summary"}],"stop_reason":"end_turn",
        "usage":{"input_tokens":100,"output_tokens":5,"speed":"fast","service_tier":"standard"}});
    p.mock.queue(&format!("body 200 {response}")).unwrap();
    let answer = p.rig.succeed(&p.provider_key(), &request).await;
    assert_eq!(answer["timing"]["requested_service_tier"], "fast");
    assert_eq!(
        answer["timing"]["reported_service_tier"],
        json!({"state":"told","value":"fast"})
    );
    let seen = p.model_requests();
    assert_eq!(seen.last().unwrap().body["speed"], "fast");
    assert_eq!(seen.last().unwrap().body["service_tier"], "auto");
}

#[tokio::test]
async fn structured_completion_never_remerges_changed_runtime_defaults_into_its_snapshot() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    let request = ProviderComplete {
        model_parameters: Some(common::model::enabled_parameters()),
        provider_id: p.rig.provider,
        purpose: CompletionPurpose::Summary,
        protocol: RESPONSES.to_owned(),
        model_name: MODEL.to_owned(),
        system: "Summarize".to_owned(),
        input: "Issued material".to_owned(),
        max_output_tokens: 120,
        params: Some(json!({"temperature":0.4})),
    };
    p.mock.queue("say Original\nusage 100 5").unwrap();
    p.rig.succeed(&p.provider_key(), &request).await;
    let original = p.model_requests()[0].body.clone();
    assert_eq!(
        original["temperature"],
        json!(0.4),
        "ordinary structured snapshot"
    );
    let started = p
        .start_provider(&[(
            RESPONSES,
            p.mock.responses_base_url(),
            json!({
                "params":{"temperature":0.2,"top_p":0.7}
            }),
        )])
        .await;
    assert!(matches!(started, Progress::Succeeded(_)), "{started:?}");
    p.mock
        .queue("say Same issued request\nusage 100 5")
        .unwrap();
    p.rig.succeed(&p.provider_key(), &request).await;
    assert_eq!(
        p.model_requests()[1].body,
        original,
        "a new runtime default cannot modify the issuer's final snapshot"
    );
    // A constrained command with no explicit sampling options does not pick up runtime defaults.
    p.mock.queue("say Legacy\nusage 100 5").unwrap();
    p.rig
        .succeed(
            &p.provider_key(),
            &ProviderComplete {
                model_parameters: Some(common::model::enabled_parameters()),
                params: None,
                ..request
            },
        )
        .await;
    let seen = p.model_requests();
    assert!(seen[2].body.get("temperature").is_none());
    assert!(seen[2].body.get("top_p").is_none());
}

#[tokio::test]
async fn a_legacy_completion_without_ceiling_metadata_requires_explicit_reissue() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    let request = ProviderComplete {
        model_parameters: None,
        provider_id: p.rig.provider,
        purpose: CompletionPurpose::Summary,
        protocol: RESPONSES.to_owned(),
        model_name: MODEL.to_owned(),
        system: "Summarize".to_owned(),
        input: "Synthetic work".to_owned(),
        max_output_tokens: 120,
        params: None,
    };
    let result = p.rig.send(&p.provider_key(), &request).await;
    assert!(matches!(result, Progress::Failed(_)), "{result:?}");
    assert!(
        p.model_requests().is_empty(),
        "a legacy body cannot bypass a provider schema"
    );
    p.mock.queue("say Reissued\nusage 100 5").unwrap();
    p.rig
        .succeed(
            &p.provider_key(),
            &ProviderComplete {
                model_parameters: Some(common::model::enabled_parameters()),
                ..request
            },
        )
        .await;
    assert_eq!(p.model_requests().len(), 1);
}

#[tokio::test]
async fn an_answer_that_ran_out_of_output_fails_with_the_upstreams_reason_and_what_it_used() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    for (protocol, reason) in [(RESPONSES, "max_output_tokens"), (MESSAGES, "max_tokens")] {
        p.mock
            .queue("think Weighing the work\nsay A part of the summary\nincomplete\nusage 100 120 reasoning=109")
            .unwrap();
        let result = p
            .rig
            .send(
                &p.provider_key(),
                &ProviderComplete {
                    model_parameters: Some(common::model::enabled_parameters()),
                    params: None,
                    provider_id: p.rig.provider,
                    purpose: CompletionPurpose::Summary,
                    protocol: protocol.to_owned(),
                    model_name: MODEL.to_owned(),
                    system: "Summarize".to_owned(),
                    input: "Synthetic work".to_owned(),
                    max_output_tokens: 120,
                },
            )
            .await;
        let Progress::Failed(error) = result else {
            panic!("an answer cut by its output allowance is no completed answer");
        };
        let details = error.details.unwrap();
        assert_eq!(
            details["failure"]["reason"], reason,
            "the upstream's own word, not one fixed sentence ({protocol})"
        );
        assert_eq!(details["failure"]["output_limit"], true);
        let request: orch_agent_contract::requests::UsageRequest =
            serde_json::from_value(details["model_request"].clone()).unwrap();
        assert_eq!(
            request.tokens.output_tokens,
            Some(120),
            "the tokens the cut answer used are counted ({protocol})"
        );
    }
}
