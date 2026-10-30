//! The Messages protocol, relayed the way the provider role does it, to the scripted model server.
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
use orch_protocol_anthropic_messages::AnthropicMessages;
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
        ("anthropic-version", "2023-06-01"),
        ("user-agent", "harness/1.0"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers
}

/// A client of the server at the base of a Messages endpoint: requests go to `<base>/v1/messages`.
fn below_base(mock: &MockLlm) -> ModelClient {
    ModelClient::new(&mock.messages_base_url())
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
    let protocol: Box<dyn WireProtocol> = Box::new(AnthropicMessages);
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
    for (name, value) in &query {
        target.push(if target.contains('?') { '&' } else { '?' });
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

async fn open_server() -> MockLlm {
    MockLlm::start(MockConfig::default()).await.unwrap()
}

fn create(text: &str, stream: bool) -> Value {
    json!({ "model": "mock-large", "max_tokens": 64, "stream": stream,
            "messages": [{ "role": "user", "content": text }] })
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
        (Method::POST, "/v1/messages"),
        (Method::POST, "/v1/messages?beta=true"),
        (Method::POST, "/v1/messages/count_tokens"),
        (Method::GET, "/v1/models"),
    ] {
        let relayed = relay(&mock, method, path, &body, &endpoint).await;
        assert!(relayed.refusal.is_none());
        assert_eq!(relayed.status, Some(StatusCode::OK), "{path}");
    }
    let reached = mock.requests().len();
    assert_eq!(reached, 4);
    for (method, path) in [
        (Method::POST, "/v1/messages/batches"),
        (Method::GET, "/v1/messages/batches/msgbatch_1"),
        (Method::POST, "/v1/complete"),
        (Method::POST, "/v1/files"),
        (Method::GET, "/v1/models/claude-opus-5"),
        (Method::POST, "/responses"),
        (Method::POST, "/v1/messages/"),
    ] {
        let relayed = relay(&mock, method.clone(), path, &body, &endpoint).await;
        let refusal = relayed
            .refusal
            .unwrap_or_else(|| panic!("{method} {path} was let through"));
        assert_eq!(refusal.status(), StatusCode::NOT_FOUND);
        let error: Value = serde_json::from_slice(refusal.body()).unwrap();
        assert_eq!(error["type"], "error");
        assert_eq!(error["error"]["type"], "not_found_error");
        assert_eq!(error["error"]["code"], "path_not_allowed");
    }
    assert_eq!(
        mock.requests().len(),
        reached,
        "nothing refused reached the upstream"
    );
}

#[tokio::test]
async fn the_query_a_harness_adds_passes_on_with_the_request() {
    let mock = open_server().await;
    let endpoint = Endpoint::default().options(json!({ "query": { "api-version": "2025-01-01" } }));
    relay(
        &mock,
        Method::POST,
        "/v1/messages?beta=true",
        &create("hi", false),
        &endpoint,
    )
    .await;
    let seen = &recorded(&mock, 1).await[0];
    assert_eq!(seen.path, "/v1/messages");
    assert_eq!(
        seen.query.as_deref(),
        Some("beta=true&api-version=2025-01-01")
    );
}

#[tokio::test]
async fn the_model_is_read_from_the_body_and_is_the_one_the_upstream_receives() {
    let mock = open_server().await;
    let body = json!({ "model": "anthropic/claude-haiku-5.5", "max_tokens": 8, "messages": [] });
    let relayed = relay(
        &mock,
        Method::POST,
        "/v1/messages",
        &body,
        &Endpoint::default(),
    )
    .await;
    assert_eq!(relayed.model.as_deref(), Some("anthropic/claude-haiku-5.5"));
    let counted = relay(
        &mock,
        Method::POST,
        "/v1/messages/count_tokens",
        &body,
        &Endpoint::default(),
    )
    .await;
    assert_eq!(counted.model.as_deref(), Some("anthropic/claude-haiku-5.5"));
    let seen = recorded(&mock, 2).await;
    assert!(
        seen.iter()
            .all(|r| r.model.as_deref() == Some("anthropic/claude-haiku-5.5"))
    );
    let none = relay(
        &mock,
        Method::GET,
        "/v1/models",
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
            "/v1/messages",
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
            seen.header("anthropic-version"),
            Some("2023-06-01"),
            "the harness's own version passes on"
        );
    }
}

#[tokio::test]
async fn a_vendor_that_documents_x_api_key_refuses_a_bearer_and_takes_the_key_where_the_options_put_it()
 {
    let config = MockConfig::default()
        .with_credential("sk-real")
        .accepting(&[Presentation::XApiKey]);
    let mock = MockLlm::start(config).await.unwrap();
    let relayed = relay(
        &mock,
        Method::POST,
        "/v1/messages",
        &create("hi", false),
        &Endpoint::default(),
    )
    .await;
    assert_eq!(
        relayed.status,
        Some(StatusCode::UNAUTHORIZED),
        "the default is a bearer"
    );
    let error: Value = serde_json::from_slice(&relayed.chunks.concat()).unwrap();
    assert_eq!(error["error"]["type"], "authentication_error");
    let endpoint = Endpoint::with_key("sk-real").options(json!({ "auth": "x-api-key" }));
    let relayed = relay(
        &mock,
        Method::POST,
        "/v1/messages",
        &create("hi", false),
        &endpoint,
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
        "/v1/messages",
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
        "/v1/messages",
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
        [
            "anthropic-version",
            "authorization",
            "user-agent",
            "x-gateway-key",
            "x-route"
        ]
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
        "/v1/messages",
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
async fn usage_is_read_from_a_stream_as_it_arrives_and_agrees_with_what_the_script_declared() {
    let mock = open_server().await;
    let script = "#!mock\nchunk 4\nthink hm\nsay Hello there\nusage 120 30 cached=100 reasoning=10";
    let relayed = relay(
        &mock,
        Method::POST,
        "/v1/messages",
        &create(script, true),
        &Endpoint::default(),
    )
    .await;
    assert!(!relayed.broke);
    assert!(relayed.chunks.len() > 5, "the answer came in pieces");
    let usage = relayed.usage.unwrap();
    assert_eq!(usage.model.as_deref(), Some("mock-large"));
    assert_eq!(usage.request_ref.as_deref(), Some("msg_mock_000001"));
    // The vendor counts the 100 cached tokens apart from the 20 others; the contract counts them in the input.
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
async fn a_gateways_stream_that_ends_with_a_done_event_is_read_the_same() {
    let mock = open_server().await;
    let script = "chunk 4\nsay Hello there\nusage 120 30 cached=100";
    let vendor = relay(
        &mock,
        Method::POST,
        "/v1/messages",
        &create(&format!("#!mock\n{script}"), true),
        &Endpoint::default(),
    )
    .await;
    let gateway = relay(
        &mock,
        Method::POST,
        "/v1/messages",
        &create(&format!("#!mock\ngateway\n{script}"), true),
        &Endpoint::default(),
    )
    .await;
    let text = String::from_utf8(gateway.chunks.concat()).unwrap();
    assert!(text.ends_with("event: data\ndata: [DONE]\n\n"), "{text}");
    let (vendor, gateway) = (vendor.usage.unwrap(), gateway.usage.unwrap());
    assert_eq!(gateway.tokens, vendor.tokens);
    assert_eq!(gateway.model, vendor.model);
}

#[tokio::test]
async fn usage_is_read_from_a_plain_reply() {
    let mock = open_server().await;
    let script = "#!mock\nsay Hello\nusage 12 5 cache_write=4 cost=0.00021";
    let relayed = relay(
        &mock,
        Method::POST,
        "/v1/messages",
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
}

#[tokio::test]
async fn a_count_of_tokens_is_not_use() {
    let mock = open_server().await;
    let relayed = relay(
        &mock,
        Method::POST,
        "/v1/messages/count_tokens",
        &create("0123456789abcdef", false),
        &Endpoint::default(),
    )
    .await;
    let counted: Value = serde_json::from_slice(&relayed.chunks.concat()).unwrap();
    assert!(counted["input_tokens"].as_i64().unwrap() > 0);
    assert_eq!(
        relayed.usage, None,
        "counting tokens costs nothing and is not charged"
    );
}

#[tokio::test]
async fn a_stream_that_is_cut_still_reports_the_prompt_and_what_the_upstream_had_counted_by_then() {
    let mock = open_server().await;
    let script = "#!mock\nchunk 3\nsay abcdefghijkl\nusage 40 12\ncut";
    let relayed = relay(
        &mock,
        Method::POST,
        "/v1/messages",
        &create(script, true),
        &Endpoint::default(),
    )
    .await;
    assert!(relayed.broke, "the body broke off");
    assert!(
        !relayed.chunks.is_empty(),
        "what came before the cut was relayed"
    );
    let usage = relayed
        .usage
        .expect("the start of the message had said what the prompt took");
    assert_eq!(usage.tokens.input_tokens, 40);
    assert_eq!(
        usage.tokens.output_tokens, 1,
        "the vendor's own first count of the output"
    );
    assert_eq!(usage.model.as_deref(), Some("mock-large"));
    assert_eq!(mock.settled(1).await[0].outcome, Outcome::Cut);
}

#[tokio::test]
async fn a_stream_that_ends_early_and_cleanly_is_read_the_same_way() {
    let mock = open_server().await;
    let relayed = relay(
        &mock,
        Method::POST,
        "/v1/messages",
        &create("#!mock\nsay abc\nusage 5 5\ntruncate", true),
        &Endpoint::default(),
    )
    .await;
    assert!(!relayed.broke);
    assert_eq!(relayed.usage.unwrap().tokens.input_tokens, 5);
}

#[tokio::test]
async fn a_stream_that_fails_inside_keeps_the_prompt_it_had_reported() {
    let mock = open_server().await;
    let relayed = relay(
        &mock,
        Method::POST,
        "/v1/messages",
        &create("#!mock\nsay abc\nusage 9 4\nstream_error overloaded", true),
        &Endpoint::default(),
    )
    .await;
    assert_eq!(relayed.usage.unwrap().tokens.input_tokens, 9);
}

#[tokio::test]
async fn an_error_of_the_upstream_is_passed_on_as_it_is_and_has_no_usage() {
    let mock = open_server().await;
    let relayed = relay(
        &mock,
        Method::POST,
        "/v1/messages",
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
        "/v1/messages",
        &create(script, true),
        &Endpoint::default(),
    )
    .await;
    let whole: Vec<u8> = relayed.chunks.concat();
    let expected = relayed.usage.unwrap();
    for size in [1, 2, 5, 17, 100] {
        let mut observer = AnthropicMessages.usage_observer();
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
        "/v1/messages",
        &create("#!mock\nerror overloaded", false),
        &Endpoint::default(),
    )
    .await;
    let theirs: Value = serde_json::from_slice(&upstream.chunks.concat()).unwrap();
    let ours = AnthropicMessages.deny(
        StatusCode::from_u16(529).unwrap(),
        "lease_closed",
        "the lease is closed",
    );
    let ours: Value = serde_json::from_slice(ours.body()).unwrap();
    assert_eq!(ours["type"], theirs["type"]);
    assert_eq!(ours["error"]["type"], theirs["error"]["type"]);
    for member in ["type", "message"] {
        assert!(ours["error"].get(member).is_some() && theirs["error"].get(member).is_some());
    }
    assert_eq!(
        ours["error"]["code"], "lease_closed",
        "the forwarder's code is one more member"
    );
}

#[tokio::test]
async fn models_are_discovered_from_the_upstreams_list_as_suggestions() {
    let models = vec![
        MockModel {
            id: "anthropic/claude-haiku-5.5".into(),
            display_name: Some("Anthropic: Claude Haiku 5.5".into()),
            context_length: Some(1_000_000),
            pricing: Some(MockPricing {
                prompt: "0.0000001".into(),
                completion: "0.0000005".into(),
                input_cache_read: Some("0.00000001".into()),
                input_cache_write: Some("0.000000125".into()),
            }),
        },
        MockModel::named("plain-model"),
    ];
    let mock = MockLlm::start(MockConfig::default().with_models(models))
        .await
        .unwrap();
    let protocol: Box<dyn WireProtocol> = Box::new(AnthropicMessages);
    let (method, path) = protocol.models_request().unwrap();
    let relayed = relay(&mock, method, path, &Value::Null, &Endpoint::default()).await;
    assert_eq!(relayed.status, Some(StatusCode::OK));
    let found = protocol.parse_models(&relayed.chunks.concat());
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].model_name, "anthropic/claude-haiku-5.5");
    assert_eq!(found[0].display_name, "Anthropic: Claude Haiku 5.5");
    assert_eq!(
        found[0]
            .parameters
            .offered
            .as_ref()
            .and_then(|o| o.context_max),
        Some(1_000_000)
    );
    assert_eq!(found[0].parameters.context_window, None);
    let rate = found[0].parameters.rate.clone().unwrap();
    assert_eq!(
        (rate.input_per_mtok.as_str(), rate.output_per_mtok.as_str()),
        ("0.1", "0.5")
    );
    assert_eq!(rate.cache_write_per_mtok.as_deref(), Some("0.125"));
    assert_eq!(found[1].display_name, "plain-model");
    assert_eq!(found[1].parameters.rate, None);
}

#[tokio::test]
async fn one_request_discovers_every_model_though_the_vendor_pages_its_list_by_twenty() {
    let models: Vec<_> = (0..45)
        .map(|n| MockModel::named(&format!("model-{n:02}")))
        .collect();
    let mock = MockLlm::start(MockConfig::default().with_models(models))
        .await
        .unwrap();
    let protocol = AnthropicMessages;
    let (method, path) = protocol.models_request().unwrap();
    let relayed = relay(&mock, method, path, &Value::Null, &Endpoint::default()).await;
    assert_eq!(protocol.parse_models(&relayed.chunks.concat()).len(), 45);
    // Without the parameter the vendor gives its first page and no more.
    let bare = relay(
        &mock,
        Method::GET,
        "/v1/models",
        &Value::Null,
        &Endpoint::default(),
    )
    .await;
    assert_eq!(protocol.parse_models(&bare.chunks.concat()).len(), 20);
}

#[tokio::test]
async fn the_list_a_lease_answers_is_read_back_as_exactly_its_models() {
    let lease = vec!["claude-opus-5".to_owned(), "vendor/x:free".to_owned()];
    let answer = AnthropicMessages.models_response(&lease);
    let names: Vec<_> = AnthropicMessages
        .parse_models(answer.body())
        .into_iter()
        .map(|suggestion| suggestion.model_name)
        .collect();
    assert_eq!(names, lease);
}
