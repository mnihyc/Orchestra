//! `orchctl api`: one request to the API with a token, the answer printed as it came back, and the status of the program from the status of the answer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use orch_api_core::registry::Registry;
use orch_api_core::testing::TestApp;
use orch_pki::certificates::{Certificate, LeafKey};
use orch_pki::verification::TlsIdentity;
use orch_testkit::clock;
use orch_types::SystemClock;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::{Api, Deployment, Run, api_token, person};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// Runs `orchctl` as a person with the address and the token in the
/// environment runs it.
async fn call(world: &Deployment, url: &str, token: &str, args: &[&str]) -> Run {
    let mut command = world.command();
    command.env("ORCH_URL", url).env("ORCH_TOKEN", token);
    support::run(command, args).await
}

/// What a stand-in server was asked.
#[derive(Debug, Clone)]
struct Seen {
    body: Vec<u8>,
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// A server that answers what a test needs to see the program handle: odd
/// bodies, refusals, redirects, and an echo of what it was sent.
struct StandIn {
    url: String,
    seen: Log,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn answer(
    State(log): State<Log>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    log.lock().unwrap().push(Seen {
        body: body.to_vec(),
    });
    match uri.path() {
        // Neither valid JSON nor ended by a line break: it must come out as it is.
        "/api/v1/odd" => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            "{ \"spaced\" :  [1,\n 2] }  ",
        )
            .into_response(),
        "/api/v1/down" => (StatusCode::SERVICE_UNAVAILABLE, "down for the night").into_response(),
        "/api/v1/moved" => (
            StatusCode::FOUND,
            [(header::LOCATION, "http://127.0.0.1:1/elsewhere")],
            "",
        )
            .into_response(),
        "/api/v1/echo" | "/api/other/echo" => {
            let named = |name: header::HeaderName| {
                headers
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned)
            };
            axum::Json(json!({
                "method": method.as_str(),
                "path": uri.path(),
                "query": uri.query(),
                "authorization": named(header::AUTHORIZATION),
                "content_type": named(header::CONTENT_TYPE),
                "body": String::from_utf8_lossy(&body),
            }))
            .into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn stand_in() -> StandIn {
    let seen = Log::default();
    let router = Router::new()
        .fallback(any(answer))
        .with_state(Arc::clone(&seen));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    StandIn { url, seen, task }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_is_made_with_the_token_and_the_answer_is_what_the_api_said() {
    let world = Deployment::new().await;
    let ada = person(&world.db(), "ada").await;
    let token = api_token(&world.db(), ada).await;
    let api = Api::serve(&world.db()).await;

    let run = call(&world, &api.url, &token, &["api", "GET", "/me"]).await;

    let me: Value = serde_json::from_str(run.succeeded()).unwrap();
    assert_eq!(me["username"], "ada");
    assert_eq!(run.err, "", "a success says nothing beside the answer");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_answer_is_printed_exactly_as_it_came_back() {
    let world = Deployment::new().await;
    let stand = stand_in().await;

    let run = call(&world, &stand.url, "opat_any", &["api", "GET", "/odd"]).await;

    assert_eq!(run.succeeded(), "{ \"spaced\" :  [1,\n 2] }  ");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_base_path_of_the_api_is_put_in_front_of_a_path_that_lacks_it() {
    let world = Deployment::new().await;
    let stand = stand_in().await;

    for path in [
        "/echo?x=1&y=two",
        "echo?x=1&y=two",
        "/api/v1/echo?x=1&y=two",
    ] {
        let run = call(&world, &stand.url, "opat_any", &["api", "GET", path]).await;
        let echoed: Value = serde_json::from_str(run.succeeded()).unwrap();
        assert_eq!(echoed["path"], "/api/v1/echo", "{path}");
        assert_eq!(echoed["query"], "x=1&y=two", "{path}");
    }
    let run = call(
        &world,
        &stand.url,
        "opat_any",
        &["api", "get", "/api/other/echo"],
    )
    .await;
    let echoed: Value = serde_json::from_str(run.succeeded()).unwrap();
    assert_eq!(
        echoed["path"], "/api/other/echo",
        "a path that begins /api/ is taken as written"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_token_goes_as_a_bearer_token_and_a_body_goes_as_json_exactly_as_typed() {
    let world = Deployment::new().await;
    let stand = stand_in().await;

    let run = call(
        &world,
        &stand.url,
        "opat_secret-token",
        &["api", "POST", "/echo", "--data", "{\"a\":   1}"],
    )
    .await;

    let echoed: Value = serde_json::from_str(run.succeeded()).unwrap();
    assert_eq!(echoed["method"], "POST");
    assert_eq!(echoed["authorization"], "Bearer opat_secret-token");
    assert_eq!(echoed["content_type"], "application/json");
    assert_eq!(echoed["body"], "{\"a\":   1}");
    let seen = stand.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].body, b"{\"a\":   1}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_can_be_read_from_a_file_or_from_standard_input() {
    let world = Deployment::new().await;
    let ada = person(&world.db(), "ada").await;
    let token = api_token(&world.db(), ada).await;
    let api = Api::serve(&world.db()).await;
    let file = world.dir().join("change.json");
    std::fs::write(&file, "{\"display_name\": \"Ada from a file\"}").unwrap();

    let from_file = call(
        &world,
        &api.url,
        &token,
        &[
            "api",
            "PATCH",
            "/me",
            "--data",
            &format!("@{}", file.display()),
        ],
    )
    .await;
    let me: Value = serde_json::from_str(from_file.succeeded()).unwrap();
    assert_eq!(me["display_name"], "Ada from a file");

    let mut command = world.command();
    command.env("ORCH_URL", &api.url).env("ORCH_TOKEN", &token);
    command.args(["api", "PATCH", "/me", "--data", "-"]);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().unwrap();
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(b"{\"display_name\": \"Ada from the pipe\"}")
            .await
            .unwrap();
    }
    let done = child.wait_with_output().await.unwrap();
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    let name: String = sqlx::query_scalar("SELECT display_name FROM users WHERE username = 'ada'")
        .fetch_one(world.pool())
        .await
        .unwrap();
    assert_eq!(name, "Ada from the pipe");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_that_refuses_is_printed_all_the_same_and_the_status_of_the_program_is_its_class()
{
    let world = Deployment::new().await;
    let api = Api::serve(&world.db()).await;

    let refused = call(&world, &api.url, "opat_not-a-token", &["api", "GET", "/me"]).await;

    assert_eq!(refused.code, 4, "a 401 is a 4");
    let problem: Value = serde_json::from_str(&refused.out).unwrap();
    assert_eq!(
        problem["status"], 401,
        "the API's own answer is on standard output"
    );
    assert_eq!(refused.err.lines().count(), 1, "{}", refused.err);
    assert!(
        refused.err.contains("401") && refused.err.contains("/api/v1/me"),
        "{}",
        refused.err
    );

    let stand = stand_in().await;
    let down = call(&world, &stand.url, "opat_any", &["api", "GET", "/down"]).await;
    assert_eq!(down.code, 5, "a 503 is a 5");
    assert_eq!(down.out, "down for the night");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_is_not_followed_so_that_the_token_goes_to_no_place_nobody_named() {
    let world = Deployment::new().await;
    let stand = stand_in().await;

    let run = call(&world, &stand.url, "opat_any", &["api", "GET", "/moved"]).await;

    assert_eq!(run.code, 3, "a 302 is a 3");
    let seen = stand.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "the place it pointed to was not asked");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_that_cannot_be_made_is_refused_in_one_line_that_says_what_to_do() {
    let world = Deployment::new().await;
    let stand = stand_in().await;
    let ok = (stand.url.as_str(), "opat_any");

    let no_url = {
        let mut command = world.command();
        command.env("ORCH_TOKEN", "opat_any");
        support::run(command, &["api", "GET", "/me"]).await
    };
    no_url.refused_with("ORCH_URL is not set");

    let no_token = {
        let mut command = world.command();
        command.env("ORCH_URL", ok.0);
        support::run(command, &["api", "GET", "/me"]).await
    };
    no_token.refused_with("ORCH_TOKEN is not set");

    call(
        &world,
        "http://orch.example.com",
        ok.1,
        &["api", "GET", "/me"],
    )
    .await
    .refused_with("use https://");
    call(
        &world,
        "ftp://orch.example.com",
        ok.1,
        &["api", "GET", "/me"],
    )
    .await
    .refused_with("ORCH_URL is not an address");
    call(&world, ok.0, "line\nbreak", &["api", "GET", "/me"])
        .await
        .refused_with("ORCH_TOKEN");
    call(&world, ok.0, ok.1, &["api", "FETCH", "/me"])
        .await
        .refused_with("not a method of the API");
    call(&world, ok.0, ok.1, &["api", "GET", "/me", "--data", "{}"])
        .await
        .refused_with("a GET carries no body");
    call(
        &world,
        ok.0,
        ok.1,
        &["api", "POST", "/me", "--data", "{\"a\": "],
    )
    .await
    .refused_with("--data is not JSON");
    call(
        &world,
        ok.0,
        ok.1,
        &["api", "POST", "/me", "--data", "@/no/such/file.json"],
    )
    .await
    .refused_with("--data: cannot read /no/such/file.json");
    assert!(
        stand.seen.lock().unwrap().is_empty(),
        "nothing was sent for a request that was wrong"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_system_that_cannot_be_reached_is_told_with_its_address_and_the_reason() {
    let world = Deployment::new().await;
    // A port nothing listens on: the one a closed listener held.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };

    let run = call(&world, &closed, "opat_any", &["api", "GET", "/me"]).await;

    let line = run.refused_with("the request to");
    assert!(line.contains(&closed), "{line}");
    assert!(!line.contains("opat_any"), "the token is never printed");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_answer_to_a_plain_http_request_to_this_machine_is_taken_by_name_as_well_as_by_number()
{
    let world = Deployment::new().await;
    let stand = stand_in().await;
    let by_name = stand.url.replace("127.0.0.1", "localhost");

    let run = call(&world, &by_name, "opat_any", &["api", "GET", "/odd"]).await;

    assert_eq!(run.succeeded(), "{ \"spaced\" :  [1,\n 2] }  ");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_system_under_the_authority_compose_secrets_made_is_trusted_only_when_that_authority_is_given()
 {
    let world = Deployment::new().await;
    let ada = person(&world.db(), "ada").await;
    let token = api_token(&world.db(), ada).await;
    // The secrets of a deployment, made by the program itself for the name the
    // API is reached by here.
    let secrets = world.dir().join("made-secrets");
    let pki = world.dir().join("pki.toml");
    std::fs::write(&pki, "[pki]\nservice_dns_names = [\"localhost\"]\n").unwrap();
    world
        .run(&[
            "compose-secrets",
            "--secrets-dir",
            secrets.to_str().unwrap(),
            "--config",
            pki.to_str().unwrap(),
        ])
        .await
        .succeeded();
    // The API serves under the certificate that was made for it.
    let read = |name: &str| std::fs::read_to_string(secrets.join(name)).unwrap();
    let identity = TlsIdentity {
        chain: vec![Certificate::from_pem(&read("service_api_cert.pem")).unwrap()],
        key: LeafKey::from_pem(&read("service_api_key.pem")).unwrap(),
    };
    let tls = orch_api_core::server::browser_tls_config(&identity, Arc::new(SystemClock)).unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::new(world.db(), clock::manual(), registry);
    let mut served = Registry::new();
    orch_api_core::routes::mount(&mut served);
    let router = orch_api_core::server::router(app.app.clone(), served);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let stop = CancellationToken::new();
    let serving = tokio::spawn(orch_api_core::server::serve_tls(
        listener,
        tls,
        router,
        stop.clone(),
    ));
    let authority = secrets.join("orch_ca_cert.pem");

    let strangers = call(&world, &url, &token, &["api", "GET", "/me"]).await;
    let given = call(
        &world,
        &url,
        &token,
        &["api", "GET", "/me", "--cacert", authority.to_str().unwrap()],
    )
    .await;

    strangers.refused_with("the request to");
    let me: Value = serde_json::from_str(given.succeeded()).unwrap();
    assert_eq!(me["username"], "ada");
    call(
        &world,
        &url,
        &token,
        &["api", "GET", "/me", "--cacert", "/no/such/authority.pem"],
    )
    .await
    .refused_with("--cacert: /no/such/authority.pem");
    stop.cancel();
    let _ = serving.await;
}
