//! The Responses protocol, relayed the way the provider role does it, to the scripted model server.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Response, StatusCode};
use orch_model_contract::credentials::Credential;
use orch_model_contract::endpoint::EndpointOptions;
use orch_model_contract::protocol::{SecretHeaders, WireProtocol};
use orch_model_contract::usage::ObservedUsage;
use orch_plugin_contract::Secret;
use orch_protocol_openai_responses::OpenAiResponses;
use orch_testkit::doubles::model::{
    ClientRequest, MockConfig, MockLlm, MockModel, MockPricing, ModelClient, Outcome, Presentation,
    RecordedRequest, Verdict, fingerprint,
};
use serde_json::{Value, json};
use tracing_subscriber::fmt::MakeWriter;

/// An upstream endpoint of a provider: its options, its key and its secret headers.
struct Endpoint {
    options: Value,
    key: Option<&'static str>,
    secrets: Vec<(&'static str, &'static str)>,
}

impl Default for Endpoint {
    /// An endpoint that takes a bearer key, as the protocol's own way is.
    fn default() -> Self {
        Self::with_key("sk-real")
    }
}

impl Endpoint {
    fn with_key(key: &'static str) -> Self {
        Self {
            options: Value::Null,
            key: Some(key),
            secrets: Vec::new(),
        }
    }

    /// An endpoint that takes no key.
    fn keyless() -> Self {
        Self {
            key: None,
            ..Self::with_key("")
        }
    }

    fn options(mut self, options: Value) -> Self {
        self.options = options;
        self
    }

    fn secret(mut self, name: &'static str, value: &'static str) -> Self {
        self.secrets.push((name, value));
        self
    }
}

/// What came of relaying one request.
struct Relayed {
    refusal: Option<Response<Bytes>>,
    model: Option<String>,
    status: Option<StatusCode>,
    chunks: Vec<Bytes>,
    broke: bool,
    usage: Option<ObservedUsage>,
    presented: HeaderMap,
}

/// The harness's own headers: its token, which must never go upstream, and some it may rightly send.
fn harness_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("authorization", "Bearer harness-token"),
        ("x-api-key", "harness-token"),
        ("cookie", "session=1"),
        ("content-type", "application/json"),
        ("user-agent", "harness/1.0"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers
}

/// Relays a request as the provider role does: refuses what the protocol does not let through, presents the
/// credential as the endpoint says, sends it on, and reads the answer through the usage observer as it streams.
async fn relay(
    mock: &MockLlm,
    method: Method,
    path: &str,
    body: &Value,
    endpoint: &Endpoint,
) -> Relayed {
    log();
    let protocol: Box<dyn WireProtocol> = Box::new(OpenAiResponses);
    let bytes = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    if !protocol.allowed(&method, path) {
        return Relayed {
            refusal: Some(protocol.deny(
                StatusCode::NOT_FOUND,
                "path_not_allowed",
                "not served here",
            )),
            model: None,
            status: None,
            chunks: Vec::new(),
            broke: false,
            usage: None,
            presented: HeaderMap::new(),
        };
    }
    let model = protocol.model_of(path, bytes.as_bytes());
    let mut presented = harness_headers();
    let secrets: SecretHeaders = endpoint
        .secrets
        .iter()
        .map(|(name, value)| ((*name).to_owned(), Secret::new(*value)))
        .collect();
    let query = protocol
        .authorize(
            &mut presented,
            endpoint.key.map(Credential::new).as_ref(),
            &EndpointOptions::parse(&endpoint.options).unwrap(),
            &secrets,
        )
        .unwrap();
    let mut target = path.to_owned();
    for (at, (name, value)) in query.iter().enumerate() {
        target.push(if at == 0 { '?' } else { '&' });
        target.push_str(&format!("{name}={value}"));
    }
    let request = ClientRequest::new(method, &target)
        .with_headers(presented.clone())
        .with_body(bytes.as_bytes());
    let mut open = below_base(mock).open(request).await.unwrap();
    let mut observer = protocol.usage_observer();
    let (mut chunks, mut broke) = (Vec::new(), false);
    loop {
        match open.next_chunk().await {
            Ok(Some(chunk)) => {
                observer.observe(&chunk);
                chunks.push(chunk);
            }
            Ok(None) => break,
            Err(_) => {
                broke = true;
                break;
            }
        }
    }
    Relayed {
        refusal: None,
        model,
        status: Some(open.status),
        chunks,
        broke,
        usage: observer.usage(),
        presented,
    }
}

/// A client of the server at the base of a Responses endpoint: requests go to `<base>/responses`.
fn below_base(mock: &MockLlm) -> ModelClient {
    ModelClient::new(&mock.responses_base_url())
}

async fn open_server() -> MockLlm {
    MockLlm::start(MockConfig::default()).await.unwrap()
}

fn create(input: &str, stream: bool) -> Value {
    json!({ "model": "mock-large", "input": input, "stream": stream })
}

async fn recorded(mock: &MockLlm, count: usize) -> Vec<RecordedRequest> {
    let records = mock.settled(count).await;
    assert_eq!(records.len(), count);
    records
}

#[tokio::test]
async fn only_the_requests_the_format_lets_through_reach_the_upstream_and_the_rest_get_the_vendors_error()
 {
    let mock = open_server().await;
    let endpoint = Endpoint::with_key("sk-real");
    let body = create("hi", false);
    for (method, path) in [
        (Method::POST, "/responses"),
        (Method::POST, "/responses/compact"),
        (Method::GET, "/models"),
    ] {
        let relayed = relay(&mock, method, path, &body, &endpoint).await;
        assert!(relayed.refusal.is_none());
        assert_eq!(relayed.status, Some(StatusCode::OK));
    }
    let reached = mock.requests().len();
    assert_eq!(reached, 3);
    for (method, path) in [
        (Method::GET, "/responses/resp_1"),
        (Method::DELETE, "/responses/resp_1"),
        (Method::POST, "/responses/resp_1/cancel"),
        (Method::POST, "/chat/completions"),
        (Method::POST, "/v1/messages"),
        (Method::POST, "/files"),
        (Method::POST, "/responses/"),
    ] {
        let relayed = relay(&mock, method.clone(), path, &body, &endpoint).await;
        let refusal = relayed
            .refusal
            .unwrap_or_else(|| panic!("{method} {path} was let through"));
        assert_eq!(refusal.status(), StatusCode::NOT_FOUND);
        let error: Value = serde_json::from_slice(refusal.body()).unwrap();
        assert_eq!(error["error"]["code"], "path_not_allowed");
        assert_eq!(error["error"]["type"], "invalid_request_error");
    }
    assert_eq!(
        mock.requests().len(),
        reached,
        "nothing refused reached the upstream"
    );
}

#[tokio::test]
async fn the_model_is_read_from_the_body_and_is_the_one_the_upstream_receives() {
    let mock = open_server().await;
    let body = json!({ "model": "openai/gpt-5.6-luna:free", "input": "hi" });
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &body,
        &Endpoint::default(),
    )
    .await;
    assert_eq!(relayed.model.as_deref(), Some("openai/gpt-5.6-luna:free"));
    let seen = recorded(&mock, 1).await;
    assert_eq!(seen[0].model.as_deref(), Some("openai/gpt-5.6-luna:free"));
    let none = relay(
        &mock,
        Method::GET,
        "/models",
        &Value::Null,
        &Endpoint::default(),
    )
    .await;
    assert_eq!(none.model, None, "a list request names no model");
}

#[tokio::test]
async fn the_key_is_presented_in_the_way_the_options_say_and_the_harnesss_own_token_never_goes_upstream()
 {
    let cases: Vec<(Value, Vec<Presentation>, Presentation)> = vec![
        (
            Value::Null,
            vec![Presentation::Bearer],
            Presentation::Bearer,
        ),
        (
            json!({ "auth": "bearer" }),
            vec![Presentation::Bearer],
            Presentation::Bearer,
        ),
        (
            json!({ "auth": "x-api-key" }),
            vec![Presentation::XApiKey],
            Presentation::XApiKey,
        ),
        (
            json!({ "auth": "header", "auth_header": "api-key" }),
            vec![Presentation::Header("api-key".into())],
            Presentation::Header("api-key".into()),
        ),
    ];
    for (options, accepted, expected) in cases {
        let config = MockConfig::default()
            .with_credential("sk-real")
            .accepting(&accepted);
        let mock = MockLlm::start(config).await.unwrap();
        let endpoint = Endpoint::with_key("sk-real").options(options.clone());
        let relayed = relay(
            &mock,
            Method::POST,
            "/responses",
            &create("hi", false),
            &endpoint,
        )
        .await;
        assert_eq!(relayed.status, Some(StatusCode::OK), "{options}");
        let seen = &recorded(&mock, 1).await[0];
        assert_eq!(seen.credential.verdict, Verdict::Accepted, "{options}");
        assert_eq!(seen.credential.presented_by, [expected], "{options}");
        let everything = serde_json::to_string(seen).unwrap();
        assert!(
            !everything.contains("harness-token"),
            "{options}: {everything}"
        );
        assert!(seen.header("cookie").is_none(), "{options}");
        assert_eq!(
            seen.header("user-agent"),
            Some("harness/1.0"),
            "what is not named passes on"
        );
    }
}

#[tokio::test]
async fn a_key_presented_the_wrong_way_is_refused_by_an_upstream_that_insists() {
    let config = MockConfig::default()
        .with_credential("sk-real")
        .accepting(&[Presentation::XApiKey]);
    let mock = MockLlm::start(config).await.unwrap();
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create("hi", false),
        &Endpoint::with_key("sk-real"),
    )
    .await;
    assert_eq!(
        relayed.status,
        Some(StatusCode::UNAUTHORIZED),
        "the default is a bearer"
    );
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create("hi", false),
        &Endpoint::with_key("sk-real").options(json!({ "auth": "x-api-key" })),
    )
    .await;
    assert_eq!(relayed.status, Some(StatusCode::OK));
}

#[tokio::test]
async fn an_upstream_that_takes_no_key_gets_none_at_all() {
    let mock = open_server().await;
    let endpoint = Endpoint::keyless().options(json!({ "auth": "none" }));
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create("hi", false),
        &endpoint,
    )
    .await;
    assert_eq!(relayed.status, Some(StatusCode::OK));
    let seen = &recorded(&mock, 1).await[0];
    assert!(seen.header("authorization").is_none() && seen.header("x-api-key").is_none());
    assert_eq!(seen.credential.verdict, Verdict::NotChecked);
}

#[tokio::test]
async fn fixed_headers_secret_headers_and_query_reach_the_upstream_and_nothing_else_is_added() {
    let config = MockConfig::default().with_credential("sk-real");
    let mock = MockLlm::start(config).await.unwrap();
    let endpoint = Endpoint::with_key("sk-real")
        .options(json!({
            "headers": { "x-route": "team-a" },
            "secret_headers": ["x-gateway-key"],
            "query": { "api-version": "2025-01-01" }
        }))
        .secret("x-gateway-key", "gw-secret");
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create("hi", false),
        &endpoint,
    )
    .await;
    assert_eq!(relayed.status, Some(StatusCode::OK));
    let seen = &recorded(&mock, 1).await[0];
    assert_eq!(seen.header("x-route"), Some("team-a"));
    assert_eq!(
        seen.header("x-gateway-key"),
        Some(fingerprint("gw-secret").as_str())
    );
    assert_eq!(seen.query.as_deref(), Some("api-version=2025-01-01"));
    let transport = ["accept", "content-length", "content-type", "host"];
    let mut others: Vec<_> = seen
        .headers
        .keys()
        .map(String::as_str)
        .filter(|name| !transport.contains(name))
        .collect();
    others.sort_unstable();
    assert_eq!(
        others,
        ["authorization", "user-agent", "x-gateway-key", "x-route"]
    );
}

/// A place for tracing output to go to, to be read afterwards.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Every log line of every test of this file, at the finest level. It is installed for the whole process
/// before the first test relays anything: a subscriber set for one thread would miss events that another
/// test's thread had already made the callsites remember as unwanted.
fn log() -> &'static Captured {
    static LOG: OnceLock<Captured> = OnceLock::new();
    LOG.get_or_init(|| {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(captured.clone())
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
        captured
    })
}

#[tokio::test]
async fn no_key_and_no_secret_header_value_is_in_a_log_or_in_what_the_upstream_remembers() {
    let config = MockConfig::default().with_credential("sk-real-key-0123");
    let mock = MockLlm::start(config).await.unwrap();
    let endpoint = Endpoint::with_key("sk-real-key-0123")
        .options(json!({ "secret_headers": ["x-gateway-key"], "headers": { "x-route": "team-a" } }))
        .secret("x-gateway-key", "gw-secret-4567");
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create("hi", false),
        &endpoint,
    )
    .await;
    assert_eq!(relayed.status, Some(StatusCode::OK));
    let logged = log().text();
    assert!(
        logged.contains("credential presented"),
        "something was logged: {logged:?}"
    );
    assert!(
        logged.contains("x-gateway-key"),
        "the names of the headers are logged"
    );
    let debug_headers = format!("{:?}", relayed.presented);
    let upstream = serde_json::to_string(&recorded(&mock, 1).await).unwrap();
    for text in [&logged, &debug_headers, &upstream] {
        for secret in [
            "sk-real-key-0123",
            "gw-secret-4567",
            "sk-real",
            "harness-token",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
    }
}

#[tokio::test]
async fn usage_is_read_from_a_stream_as_it_arrives() {
    let mock = open_server().await;
    let script = "#!mock\nchunk 4\nthink hm\nsay Hello there\nusage 120 30 cached=100 reasoning=10";
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create(script, true),
        &Endpoint::default(),
    )
    .await;
    assert!(!relayed.broke);
    assert!(relayed.chunks.len() > 5, "the answer came in pieces");
    let usage = relayed.usage.unwrap();
    assert_eq!(usage.model.as_deref(), Some("mock-large"));
    assert_eq!(usage.request_ref.as_deref(), Some("resp_mock_000001"));
    assert_eq!(
        (
            usage.tokens.input_tokens,
            usage.tokens.cached_input_tokens,
            usage.tokens.output_tokens,
            usage.tokens.reasoning_tokens,
            usage.tokens.total_tokens
        ),
        (120, 100, 30, 10, 150)
    );
}

#[tokio::test]
async fn a_gateways_stream_with_no_event_names_and_a_done_marker_is_read_the_same() {
    let mock = open_server().await;
    let script = "chunk 4\nsay Hello there\nusage 120 30 cached=100";
    let vendor = relay(
        &mock,
        Method::POST,
        "/responses",
        &create(&format!("#!mock\n{script}"), true),
        &Endpoint::default(),
    )
    .await;
    let gateway = relay(
        &mock,
        Method::POST,
        "/responses",
        &create(&format!("#!mock\ngateway\n{script}"), true),
        &Endpoint::default(),
    )
    .await;
    let text = String::from_utf8(gateway.chunks.concat()).unwrap();
    assert!(
        !text.contains("event:") && text.ends_with("data: [DONE]\n\n"),
        "{text}"
    );
    assert!(text.starts_with(": \n\n"), "a comment opens the stream");
    let (vendor, gateway) = (vendor.usage.unwrap(), gateway.usage.unwrap());
    assert_eq!(gateway.tokens, vendor.tokens);
    assert_eq!(gateway.model, vendor.model);
}

#[tokio::test]
async fn usage_is_read_from_a_plain_reply_and_from_the_answer_to_a_compaction() {
    let mock = open_server().await;
    let script = "#!mock\nsay Hello\nusage 12 5 cost=0.00021";
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create(script, false),
        &Endpoint::default(),
    )
    .await;
    let usage = relayed.usage.unwrap();
    assert_eq!(
        (usage.tokens.input_tokens, usage.tokens.output_tokens),
        (12, 5)
    );
    assert_eq!(
        usage.cost.as_deref(),
        Some("0.00021"),
        "what the upstream states it charged"
    );
    let compact = json!({ "model": "mock-large", "input": [{ "type": "message", "role": "user", "content": "keep" }] });
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses/compact",
        &compact,
        &Endpoint::default(),
    )
    .await;
    let usage = relayed.usage.unwrap();
    assert!(usage.tokens.total_tokens > 0);
    assert_eq!(usage.model, None, "a compaction names no model");
}

#[tokio::test]
async fn a_stream_that_is_cut_says_nothing_of_use_because_the_upstream_had_said_nothing_yet() {
    let mock = open_server().await;
    let script = "#!mock\nchunk 3\nsay abcdefghijkl\nusage 40 12\ncut";
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create(script, true),
        &Endpoint::default(),
    )
    .await;
    assert!(relayed.broke, "the body broke off");
    assert!(
        !relayed.chunks.is_empty(),
        "what came before the cut was relayed"
    );
    assert_eq!(
        relayed.usage, None,
        "figures come with the last event, which never came"
    );
    assert_eq!(mock.settled(1).await[0].outcome, Outcome::Cut);
}

#[tokio::test]
async fn a_stream_that_ends_early_and_cleanly_says_nothing_of_use_either() {
    let mock = open_server().await;
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create("#!mock\nsay abc\nusage 5 5\ntruncate", true),
        &Endpoint::default(),
    )
    .await;
    assert!(!relayed.broke);
    assert_eq!(relayed.usage, None);
}

#[tokio::test]
async fn an_answer_that_stopped_at_its_limit_and_a_failed_one_are_read_for_what_they_say() {
    let mock = open_server().await;
    let incomplete = relay(
        &mock,
        Method::POST,
        "/responses",
        &create("#!mock\nsay abc\nincomplete\nusage 9 4", true),
        &Endpoint::default(),
    )
    .await;
    assert_eq!(incomplete.usage.unwrap().tokens.total_tokens, 13);
    let failed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create("#!mock\nsay abc\nfail server", true),
        &Endpoint::default(),
    )
    .await;
    assert_eq!(failed.usage, None);
}

#[tokio::test]
async fn an_error_of_the_upstream_is_passed_on_as_it_is_and_has_no_usage() {
    let mock = open_server().await;
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create("#!mock\nerror rate_limit retry-after=3", true),
        &Endpoint::default(),
    )
    .await;
    assert_eq!(relayed.status, Some(StatusCode::TOO_MANY_REQUESTS));
    assert_eq!(relayed.usage, None);
    let body: Value = serde_json::from_slice(&relayed.chunks.concat()).unwrap();
    assert_eq!(body["error"]["type"], "rate_limit_error");
}

#[tokio::test]
async fn the_figures_do_not_depend_on_how_the_body_was_cut_into_pieces() {
    let mock = open_server().await;
    let script = "#!mock\nchunk 3\nsay Hello there, friend\nusage 77 33 cached=7";
    let relayed = relay(
        &mock,
        Method::POST,
        "/responses",
        &create(script, true),
        &Endpoint::default(),
    )
    .await;
    let whole: Vec<u8> = relayed.chunks.concat();
    let expected = relayed.usage.unwrap();
    for size in [1, 2, 5, 17, 100] {
        let mut observer = OpenAiResponses.usage_observer();
        for piece in whole.chunks(size) {
            observer.observe(piece);
        }
        assert_eq!(observer.usage(), Some(expected.clone()), "pieces of {size}");
    }
}

#[tokio::test]
async fn a_refusal_has_the_members_of_the_vendors_own_errors() {
    let mock = open_server().await;
    let upstream = relay(
        &mock,
        Method::POST,
        "/responses",
        &create("#!mock\nerror overloaded", false),
        &Endpoint::default(),
    )
    .await;
    let theirs: Value = serde_json::from_slice(&upstream.chunks.concat()).unwrap();
    let ours = OpenAiResponses.deny(
        StatusCode::SERVICE_UNAVAILABLE,
        "lease_closed",
        "the lease is closed",
    );
    let ours: Value = serde_json::from_slice(ours.body()).unwrap();
    let members = |value: &Value| -> Vec<String> {
        let mut names: Vec<_> = value["error"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    };
    assert_eq!(members(&ours), members(&theirs));
    assert_eq!(ours["error"]["type"], theirs["error"]["type"]);
    assert_eq!(ours["error"]["code"], "lease_closed");
}

#[tokio::test]
async fn models_are_discovered_from_the_upstreams_list_as_suggestions() {
    let models = vec![
        MockModel {
            id: "openai/gpt-5.6-luna".into(),
            display_name: Some("OpenAI: GPT-5.6 Luna".into()),
            context_length: Some(1_050_000),
            pricing: Some(MockPricing {
                prompt: "0.0000002".into(),
                completion: "0.0000012".into(),
                input_cache_read: Some("0.00000002".into()),
                input_cache_write: None,
            }),
        },
        MockModel::named("plain-model"),
    ];
    let mock = MockLlm::start(MockConfig::default().with_models(models))
        .await
        .unwrap();
    let protocol: Box<dyn WireProtocol> = Box::new(OpenAiResponses);
    let (method, path) = protocol.models_request().unwrap();
    let relayed = relay(&mock, method, path, &Value::Null, &Endpoint::default()).await;
    assert_eq!(relayed.status, Some(StatusCode::OK));
    let found = protocol.parse_models(&relayed.chunks.concat());
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].model_name, "openai/gpt-5.6-luna");
    assert_eq!(found[0].display_name, "OpenAI: GPT-5.6 Luna");
    assert_eq!(
        found[0]
            .parameters
            .offered
            .as_ref()
            .and_then(|o| o.context_max),
        Some(1_050_000)
    );
    assert_eq!(found[0].parameters.context_window, None);
    let rate = found[0].parameters.rate.clone().unwrap();
    assert_eq!(
        (rate.input_per_mtok.as_str(), rate.output_per_mtok.as_str()),
        ("0.2", "1.2")
    );
    assert_eq!(rate.cached_input_per_mtok, "0.02");
    assert_eq!(
        found[1].display_name, "plain-model",
        "the routing name is shown where the list has no name"
    );
    assert_eq!(found[1].parameters.rate, None);
}

#[tokio::test]
async fn the_list_a_lease_answers_is_read_back_as_exactly_its_models() {
    let lease = vec!["gpt-6-sol".to_owned(), "openai/x:free".to_owned()];
    let answer = OpenAiResponses.models_response(&lease);
    let names: Vec<_> = OpenAiResponses
        .parse_models(answer.body())
        .into_iter()
        .map(|suggestion| suggestion.model_name)
        .collect();
    assert_eq!(names, lease);
}
