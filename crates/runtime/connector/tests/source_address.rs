//! A source address receives only its own authorization, under the scheme and machine policies, with bounded replies and no redirect.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use axum::{
    Json as HttpJson, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use orch_connector_core::sources::address::{
    AddressCall, AddressClient, AddressOptions, AddressSource, OpenAuthorization,
};
use orch_connector_core::upstream::Upstream;
use orch_link_contract::policy::LocalPolicy;
use orch_plugin_contract::Secret;
use orch_provider_contract::source::{
    SourceAskCredential, SourceAskProvider, SourceCredential, SourceDescriptorFacts,
};
use orch_provider_contract::{
    AddressAuth, FactsSource, SourceAddress, SourceAsk, SourceDescriptor,
};
use orch_types::{Clock, Json, SystemClock, new_id};
use serde_json::json;
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(HeaderMap, Json)>>>;
#[derive(Clone)]
struct Reply {
    status: StatusCode,
    value: Json,
    location: Option<String>,
    seen: Seen,
    descriptor: Option<Json>,
}
struct Server {
    url: String,
    seen: Seen,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn reply(
    State(state): State<Reply>,
    headers: HeaderMap,
    HttpJson(body): HttpJson<Json>,
) -> (StatusCode, HeaderMap, HttpJson<Json>) {
    let describes = body["method"] == "describe";
    state.seen.lock().unwrap().push((headers, body));
    let mut headers = HeaderMap::new();
    if let Some(location) = state.location {
        headers.insert("location", location.parse().unwrap());
    }
    let value = if describes {
        state.descriptor.unwrap_or(state.value)
    } else {
        state.value
    };
    (state.status, headers, HttpJson(value))
}

async fn serve(status: StatusCode, value: Json, location: Option<String>) -> Server {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().route("/", post(reply)).with_state(Reply {
        status,
        value,
        location,
        seen: Arc::clone(&seen),
        descriptor: None,
    });
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server { url, seen, task }
}

fn client() -> AddressClient {
    AddressClient::new(Upstream::new(Arc::new(SystemClock)).unwrap())
}

fn ask<'a>(server: &'a Server, params: &'a Json) -> AddressCall<'a> {
    AddressCall {
        url: &server.url,
        authorization: OpenAuthorization::Bearer("source-token"),
        method: "facts.read",
        params,
        allow_http: true,
        local_policy: None,
        secrets: &[],
    }
}

#[tokio::test]
async fn a_bearer_address_receives_the_contract_envelope_and_no_cookie() {
    let server = serve(StatusCode::OK, json!({"result":{"plan":"pro"}}), None).await;
    let params = json!({"provider":{"id":"example","name":"Team"},"config":{},"secrets":{}});
    assert_eq!(
        client().call(ask(&server, &params)).await.unwrap(),
        json!({"plan":"pro"})
    );
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (headers, body) = &seen[0];
    assert_eq!(headers["authorization"], "Bearer source-token");
    assert_eq!(headers["accept"], "application/json");
    assert_eq!(headers["content-type"], "application/json");
    assert!(
        headers["user-agent"]
            .to_str()
            .unwrap()
            .starts_with("orch-connector/")
    );
    assert!(!headers.contains_key("cookie"));
    assert_eq!(
        body,
        &json!({"contract":1,"method":"facts.read","params":params})
    );
}

#[tokio::test]
async fn a_named_header_token_is_sent_without_an_extra_bearer_or_cookie() {
    let server = serve(StatusCode::OK, json!({"result":{"models":[]}}), None).await;
    let params = json!({});
    let mut call = ask(&server, &params);
    call.method = "models.list";
    call.authorization = OpenAuthorization::Header {
        name: "x-source-token",
        value: "header-token",
    };
    client().call(call).await.unwrap();
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen[0].0["x-source-token"], "header-token");
    assert!(!seen[0].0.contains_key("authorization"));
    assert!(!seen[0].0.contains_key("cookie"));
}

#[tokio::test]
async fn http_off_and_a_connected_machines_upstream_policy_both_refuse_before_any_request() {
    let server = serve(StatusCode::OK, json!({"result":{}}), None).await;
    let params = json!({});
    let mut call = ask(&server, &params);
    call.allow_http = false;
    assert_eq!(
        client().call(call).await.unwrap_err().code.as_str(),
        "refused"
    );
    let policy = LocalPolicy::default();
    let mut call = ask(&server, &params);
    call.local_policy = Some(&policy);
    assert_eq!(
        client().call(call).await.unwrap_err().code.as_str(),
        "policy.upstream_not_allowed"
    );
    assert!(server.seen.lock().unwrap().is_empty());
    let allowed = LocalPolicy {
        provider_upstreams: vec![server.url.clone()],
        ..LocalPolicy::default()
    };
    let mut call = ask(&server, &params);
    call.local_policy = Some(&allowed);
    client().call(call).await.unwrap();
    assert_eq!(
        server.seen.lock().unwrap().len(),
        1,
        "the exact listed address is admitted"
    );
}

#[tokio::test]
async fn a_redirect_never_receives_this_addresses_token_at_its_target() {
    let target = serve(StatusCode::OK, json!({"result":{}}), None).await;
    let server = serve(
        StatusCode::TEMPORARY_REDIRECT,
        json!({}),
        Some(target.url.clone()),
    )
    .await;
    let params = json!({});
    assert_eq!(
        client()
            .call(ask(&server, &params))
            .await
            .unwrap_err()
            .code
            .as_str(),
        "redirected"
    );
    assert_eq!(server.seen.lock().unwrap().len(), 1);
    assert!(target.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_sources_error_sentence_is_scrubbed_of_its_token_key_and_secret_fields() {
    let server = serve(StatusCode::OK,json!({"error":{"code":"not_told","message":"source-token provider-key field-secret","retryable":false}}),None).await;
    let params = json!({"credential":{"key":"provider-key"},"secrets":{"field":"field-secret"}});
    let error = client().call(ask(&server, &params)).await.unwrap_err();
    assert_eq!(error.code.as_str(), "not_told");
    assert_eq!(error.message, "[secret] [secret] [secret]");
    assert!(!error.retryable);
}

#[tokio::test]
async fn wrong_envelopes_and_oversize_answers_are_not_reports() {
    let params = json!({});
    let server = serve(StatusCode::OK, json!({"result":{},"error":{}}), None).await;
    assert_eq!(
        client()
            .call(ask(&server, &params))
            .await
            .unwrap_err()
            .code
            .as_str(),
        "malformed"
    );
    let large = serve(
        StatusCode::OK,
        json!({"result":{"plan":"x".repeat(1024*1024)}}),
        None,
    )
    .await;
    assert_eq!(
        client()
            .call(ask(&large, &params))
            .await
            .unwrap_err()
            .code
            .as_str(),
        "too_large"
    );
}

#[tokio::test]
async fn status_failures_and_an_unavailable_presenter_never_become_ready_facts() {
    let params = json!({});
    for (status, code, retryable) in [
        (StatusCode::UNAUTHORIZED, "unauthorized", false),
        (StatusCode::FORBIDDEN, "unauthorized", false),
        (StatusCode::TOO_MANY_REQUESTS, "unavailable", true),
        (StatusCode::BAD_GATEWAY, "unavailable", true),
    ] {
        let server = serve(status, json!({"error":"source-token"}), None).await;
        let error = client().call(ask(&server, &params)).await.unwrap_err();
        assert_eq!(error.code.as_str(), code);
        assert_eq!(error.retryable, retryable);
        assert!(!error.message.contains("source-token"));
    }
    let server = serve(StatusCode::OK, json!({"result":{}}), None).await;
    let mut call = ask(&server, &params);
    call.method = "credential.present";
    assert_eq!(
        client().call(call).await.unwrap_err().code.as_str(),
        "unsupported"
    );
    assert!(server.seen.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_facts_read_has_one_eight_second_deadline_including_the_reply_body() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/",
        post(|| async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            HttpJson(json!({"result":{}}))
        }),
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let server = Server {
        url,
        seen: Arc::new(Mutex::new(Vec::new())),
        task,
    };
    let params = json!({});
    let start = tokio::time::Instant::now();
    let error = client().call(ask(&server, &params)).await.unwrap_err();
    assert_eq!(error.code.as_str(), "timeout");
    assert_eq!(start.elapsed(), orch_provider_contract::wire::READ_DEADLINE);
}

async fn source_server(value: Json) -> Server {
    let mut descriptor = SourceDescriptor::new(
        "example",
        "Example",
        "A source reached beside the credential.",
    );
    descriptor.facts = Some(SourceDescriptorFacts {
        reports: vec![
            orch_provider_contract::FactMember::Plan,
            orch_provider_contract::FactMember::CredentialVerified,
        ],
        credential: SourceCredential::Key,
        ..SourceDescriptorFacts::default()
    });
    descriptor.fields = serde_json::from_value(json!([{"name":"allowed","label":"Allowed","type":"secret","required":false,"help":null,"choices":null,"default":null,"max_bytes":null}])).unwrap();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().route("/", post(reply)).with_state(Reply {
        status: StatusCode::OK,
        value,
        location: None,
        seen: Arc::clone(&seen),
        descriptor: Some(json!({"result":descriptor})),
    });
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server { url, seen, task }
}

fn source_options(server: &Server, clock: Arc<dyn Clock>, send_credential: bool) -> AddressOptions {
    AddressOptions {
        address: SourceAddress {
            url: server.url.clone(),
            auth: AddressAuth::Bearer,
            header: None,
            send_credential,
        },
        authorization: Some(Secret::new("source-token")),
        allow_http: true,
        policy: Arc::new(orch_connector_core::policy::PolicyStore::managed(
            orch_config::ConnectorConfig::default(),
        )),
        clock,
        known_secrets: Vec::new(),
    }
}

fn source_ask() -> SourceAsk {
    SourceAsk {
        provider: SourceAskProvider {
            id: new_id(),
            name: "Team".to_owned(),
        },
        endpoints: Vec::new(),
        config: Default::default(),
        secrets: [
            ("allowed".to_owned(), Secret::new("allowed-secret")),
            ("hidden".to_owned(), Secret::new("hidden-secret")),
        ]
        .into_iter()
        .collect(),
        credential: Some(SourceAskCredential {
            key: Secret::new("provider-key"),
        }),
    }
}

#[tokio::test]
async fn an_address_gets_a_key_only_with_its_explicit_opt_in_and_gets_only_declared_secret_fields()
{
    let server = source_server(json!({"result":{"plan":"pro","credential_verified":true,"credential":{"method":null,"yields":"key","expires_at":"2027-01-01T00:00:00Z"}}})).await;
    let no_key = AddressSource::connect(
        client(),
        source_options(&server, Arc::new(SystemClock), false),
    )
    .await
    .unwrap();
    let report = no_key.read_facts(source_ask()).await.unwrap();
    assert!(report.0.get("credential_verified").is_none());
    assert!(report.0.get("credential").is_none());
    {
        let seen = server.seen.lock().unwrap();
        let params = &seen
            .iter()
            .find(|(_, body)| body["method"] == "facts.read")
            .unwrap()
            .1["params"];
        assert!(params.get("credential").is_none());
        assert_eq!(params["secrets"], json!({"allowed":"allowed-secret"}));
    }
    let key = AddressSource::connect(
        client(),
        source_options(&server, Arc::new(SystemClock), true),
    )
    .await
    .unwrap();
    assert_eq!(
        key.read_facts(source_ask()).await.unwrap().0["credential_verified"],
        true
    );
    let seen = server.seen.lock().unwrap();
    assert_eq!(
        seen.last().unwrap().1["params"]["credential"]["key"],
        "provider-key"
    );
    assert_eq!(
        seen.last().unwrap().0["authorization"],
        "Bearer source-token"
    );
}

#[tokio::test]
async fn an_address_is_described_after_a_day_and_after_a_malformed_report_without_losing_its_prior_declaration()
 {
    let clock = orch_testkit::clock::manual();
    let server = source_server(json!({"result":{"plan":"pro"}})).await;
    let source = AddressSource::connect(
        client(),
        source_options(&server, Arc::new(clock.clone()), false),
    )
    .await
    .unwrap();
    source.read_facts(source_ask()).await.unwrap();
    clock.advance(chrono::Duration::days(1));
    source.read_facts(source_ask()).await.unwrap();
    let describes = server
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, body)| body["method"] == "describe")
        .count();
    assert_eq!(describes, 2);
    let malformed = source_server(json!({"result":42})).await;
    let source =
        AddressSource::connect(client(), source_options(&malformed, Arc::new(clock), false))
            .await
            .unwrap();
    assert_eq!(
        source
            .read_facts(source_ask())
            .await
            .unwrap_err()
            .code
            .as_str(),
        "malformed"
    );
    assert_eq!(source.describe().title, "Example");
    assert_eq!(
        source
            .read_facts(source_ask())
            .await
            .unwrap_err()
            .code
            .as_str(),
        "malformed"
    );
    assert_eq!(
        malformed
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, body)| body["method"] == "describe")
            .count(),
        2
    );
}
