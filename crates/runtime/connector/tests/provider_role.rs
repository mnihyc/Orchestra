//! The provider role as the system drives it: a provider started with a key that arrives sealed, kept sealed and found again after a restart, forgotten when it is stopped, a key that a person gave to the machine, an upstream that takes none, the models and the state of a provider, who answers for a provider on a machine that is both a provider's runtime and a harness's.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

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
async fn synthetic_capacity_access_and_successful_refusals_share_the_same_classifier() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    for (protocol, script, code, reason, delay) in [
        (
            RESPONSES,
            "error overloaded retry-after=7",
            "provider_busy",
            None,
            Some(7),
        ),
        (
            RESPONSES,
            r#"body 200 {"error":{"type":"overloaded_error","message":"Model at capacity"}}"#,
            "provider_busy",
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
