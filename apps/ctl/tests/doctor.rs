//! `orchctl doctor` as an operator runs it: the built program against a real database, a
//! loopback API under a real authority, and a loopback stand-in for the edge in front of a stack.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use axum::Router;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use orch_db::heartbeat;
use orch_pki::ca::{CertificateAuthority, Holder};
use orch_pki::certificates::ServiceName;
use orch_pki::verification::{TlsIdentity, plain_server_config};
use orch_testkit::database::TestDb;
use orch_types::SystemClock;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

/// What a run of the program printed.
struct Ran {
    status: i32,
    lines: Vec<String>,
}

impl Ran {
    fn line(&self, id: &str) -> &str {
        self.lines
            .iter()
            .find(|line| line.split_whitespace().nth(1) == Some(id))
            .unwrap_or_else(|| panic!("no line for {id} in {:#?}", self.lines))
    }

    fn says(&self, id: &str, status: &str) -> bool {
        self.line(id).starts_with(status)
    }
}

fn doctor(arguments: &[&str], environment: &[(&str, &str)]) -> Ran {
    let output = Command::new(env!("CARGO_BIN_EXE_orchctl"))
        .arg("doctor")
        .args(arguments)
        .envs(environment.iter().copied())
        .env_remove("ORCH_CONFIG")
        .env_remove("ORCH_SECRETS_DIR")
        .env_remove("ORCH_ACCESS_TOKEN")
        .output()
        .expect("the program runs");
    Ran {
        status: output.status.code().unwrap_or(-1),
        lines: String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect(),
    }
}

fn migrations_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations")
}

/// A configuration file for a deployment whose API listens on `bind`.
fn configuration(dir: &Path, bind: &str) -> PathBuf {
    let path = dir.join("orch.toml");
    std::fs::write(
        &path,
        format!(
            "version = 1\n[api]\nbind = \"{bind}\"\n[database]\nmigrations_dir = \"{}\"\n",
            migrations_dir().display()
        ),
    )
    .unwrap();
    path
}

async fn run_doctor(arguments: Vec<String>, environment: Vec<(String, String)>) -> Ran {
    tokio::task::spawn_blocking(move || {
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
        let environment: Vec<(&str, &str)> = environment
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        doctor(&arguments, &environment)
    })
    .await
    .unwrap()
}

#[cfg(unix)]
fn generated_secret_arguments(directory: &Path) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let secrets = directory.join("secrets");
    let made = Command::new(env!("CARGO_BIN_EXE_orchctl"))
        .args(["compose-secrets", "--secrets-dir"])
        .arg(&secrets)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let env = directory.join(".env");
    std::fs::write(&env, "POSTGRES_PASSWORD=DO_NOT_PRINT_THIS_CREDENTIAL\n").unwrap();
    std::fs::set_permissions(&env, std::fs::Permissions::from_mode(0o600)).unwrap();
    let config = configuration(directory, "127.0.0.1:1");
    vec![
        "--only".into(),
        "secrets".into(),
        "--config".into(),
        config.display().to_string(),
        "--secrets-dir".into(),
        secrets.display().to_string(),
        "--env-file".into(),
        env.display().to_string(),
    ]
}

#[cfg(unix)]
#[test]
fn the_real_bootstrap_permissions_pass_the_real_doctor_without_exposing_credentials() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let arguments = generated_secret_arguments(directory.path());
    let secrets = directory.path().join("secrets");
    assert_eq!(
        std::fs::metadata(secrets.join("orch_ca_cert.pem"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    assert_eq!(
        std::fs::metadata(secrets.join("orch_ca_key.pem"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let marker = secrets.join(".gitkeep");
    std::fs::write(&marker, "").unwrap();
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644)).unwrap();
    let refs = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    let ran = doctor(&refs, &[]);
    assert_eq!(ran.status, 0, "{:?}", ran.lines);
    assert!(ran.says("secrets", "ok"));
    assert!(
        !ran.lines
            .join("\n")
            .contains("DO_NOT_PRINT_THIS_CREDENTIAL")
    );
}

#[cfg(unix)]
#[test]
fn public_certificate_writes_and_exposed_private_or_unknown_files_are_still_refused() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let arguments = generated_secret_arguments(directory.path());
    let refs = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    let secrets = directory.path().join("secrets");
    let cases = [
        (secrets.join("orch_ca_cert.pem"), 0o664, 0o644),
        (secrets.join("service_api_cert.pem"), 0o645, 0o644),
        (secrets.join("orch_ca_key.pem"), 0o640, 0o600),
        (secrets.join("node_enrollment_token"), 0o644, 0o600),
        (directory.path().join(".env"), 0o644, 0o600),
        (secrets.join("operator-credential"), 0o644, 0o600),
        (secrets.join(".gitkeep"), 0o644, 0o600),
    ];
    for (file, unsafe_mode, safe_mode) in cases {
        if !file.exists() {
            std::fs::write(&file, "DO_NOT_PRINT_THIS_CREDENTIAL").unwrap();
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(unsafe_mode)).unwrap();
        let ran = doctor(&refs, &[]);
        assert_eq!(ran.status, 1, "{}: {:?}", file.display(), ran.lines);
        assert!(ran.says("secrets", "FAIL"));
        assert!(
            ran.line("secrets")
                .contains(file.file_name().unwrap().to_str().unwrap())
        );
        assert!(
            !ran.lines
                .join("\n")
                .contains("DO_NOT_PRINT_THIS_CREDENTIAL")
        );
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(safe_mode)).unwrap();
    }
    assert_eq!(doctor(&refs, &[]).status, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_config_check_says_which_proxy_ranges_are_trusted_from_the_effective_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let config = configuration(dir.path(), "127.0.0.1:1");
    let arguments = vec![
        "--only".to_owned(),
        "config".to_owned(),
        "--config".to_owned(),
        config.display().to_string(),
    ];
    let unnamed = run_doctor(
        arguments.clone(),
        vec![("ORCH_API_TRUSTED_PROXY_CIDRS".to_owned(), "[]".to_owned())],
    )
    .await;
    assert_eq!(
        unnamed.status, 0,
        "an empty trust list is valid for a direct deployment"
    );
    assert!(
        unnamed
            .line("config")
            .contains("trusted_proxy_cidrs is empty")
    );
    assert!(unnamed.line("config").contains("counted as the proxy"));
    let named = run_doctor(
        arguments,
        vec![(
            "ORCH_API_TRUSTED_PROXY_CIDRS".to_owned(),
            "[\"10.20.0.0/24\",\"192.0.2.10/32\"]".to_owned(),
        )],
    )
    .await;
    assert_eq!(named.status, 0);
    assert!(
        named
            .line("config")
            .contains("trusted_proxy_cidrs: 10.20.0.0/24, 192.0.2.10/32")
    );
    assert!(named.line("config").contains("only from these ranges"));
    assert!(
        named
            .line("config")
            .contains("outside them are counted as the proxy")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stack_whose_services_report_passes_and_a_silent_one_fails_by_name() {
    let test = TestDb::fresh().await;
    let dir = tempfile::tempdir().unwrap();
    let config = configuration(dir.path(), "127.0.0.1:1");
    let arguments = vec![
        "--only".to_owned(),
        "config,database,gateway,worker".to_owned(),
        "--config".to_owned(),
        config.display().to_string(),
    ];
    let environment = vec![("ORCH_DATABASE_URL".to_owned(), test.url.clone())];

    let silent = run_doctor(arguments.clone(), environment.clone()).await;
    assert_eq!(silent.status, 1, "{:#?}", silent.lines);
    assert!(silent.says("config", "ok"));
    assert!(silent.says("database", "ok"), "{}", silent.line("database"));
    assert!(silent.says("gateway", "FAIL"), "{}", silent.line("gateway"));
    assert!(silent.says("worker", "FAIL"), "{}", silent.line("worker"));

    let mut conn = test.pool.acquire().await.unwrap();
    let now = chrono::Utc::now();
    for component in ["gateway", "worker"] {
        heartbeat::beat(
            &mut conn,
            component,
            orch_types::new_id(),
            "0.1.0",
            now,
            now,
        )
        .await
        .unwrap();
    }
    let alive = run_doctor(arguments.clone(), environment.clone()).await;
    assert_eq!(alive.status, 0, "{:#?}", alive.lines);
    assert!(alive.says("gateway", "ok") && alive.says("worker", "ok"));

    let old = now - chrono::Duration::seconds(300);
    heartbeat::beat(&mut conn, "worker", orch_types::new_id(), "0.1.0", old, old)
        .await
        .unwrap();
    let stale = run_doctor(arguments, environment).await;
    assert_eq!(stale.status, 1);
    assert!(
        stale.line("worker").contains("last heard 30"),
        "{}",
        stale.line("worker")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_on_another_release_than_the_one_expected_fails() {
    let test = TestDb::fresh().await;
    let dir = tempfile::tempdir().unwrap();
    let config = configuration(dir.path(), "127.0.0.1:1");
    let mut conn = test.pool.acquire().await.unwrap();
    let now = chrono::Utc::now();
    heartbeat::beat(
        &mut conn,
        "gateway",
        orch_types::new_id(),
        "0.1.0",
        now,
        now,
    )
    .await
    .unwrap();
    let arguments = vec![
        "--only".to_owned(),
        "gateway".to_owned(),
        "--expect-version".to_owned(),
        "0.2.0".to_owned(),
        "--config".to_owned(),
        config.display().to_string(),
    ];
    let ran = run_doctor(
        arguments,
        vec![("ORCH_DATABASE_URL".to_owned(), test.url.clone())],
    )
    .await;
    assert_eq!(ran.status, 1);
    assert!(
        ran.line("gateway").contains("release 0.2.0 was expected"),
        "{}",
        ran.line("gateway")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_node_is_waited_for_until_there_is_a_first_administrator_and_then_it_is_required() {
    let test = TestDb::fresh().await;
    let dir = tempfile::tempdir().unwrap();
    let config = configuration(dir.path(), "127.0.0.1:1");
    let arguments = vec![
        "--only".to_owned(),
        "node".to_owned(),
        "--config".to_owned(),
        config.display().to_string(),
    ];
    let environment = vec![("ORCH_DATABASE_URL".to_owned(), test.url.clone())];

    let before = run_doctor(arguments.clone(), environment.clone()).await;
    assert_eq!(before.status, 0, "{:#?}", before.lines);
    assert!(before.says("node", "wait"));

    let mut conn = test.pool.acquire().await.unwrap();
    orch_db::queries::resources::testing::insert_user(&mut conn, "ada")
        .await
        .unwrap();
    let missing = run_doctor(arguments.clone(), environment.clone()).await;
    assert_eq!(missing.status, 1);
    assert!(missing.line("node").contains("no host has enrolled"));

    let machine =
        orch_db::queries::resources::testing::insert_machine(&mut conn, "bundled", "managed")
            .await
            .unwrap();
    host_of(&test, machine, "bundled").await;
    let offline = run_doctor(arguments.clone(), environment.clone()).await;
    assert_eq!(offline.status, 1);
    assert!(offline.line("node").contains("no host has a session"));

    set_online(&test, machine).await;
    let online = run_doctor(arguments, environment).await;
    assert_eq!(online.status, 0, "{:#?}", online.lines);
    assert!(online.says("node", "ok"), "{}", online.line("node"));
}

async fn host_of(test: &TestDb, machine: orch_types::Id, name: &str) {
    sqlx::query("INSERT INTO hosts (id, machine_id, name) VALUES ($1, $2, $3)")
        .bind(orch_types::new_id())
        .bind(machine)
        .bind(name)
        .execute(&test.pool)
        .await
        .unwrap();
}

async fn set_online(test: &TestDb, machine: orch_types::Id) {
    sqlx::query("UPDATE machines SET online = true, last_seen_at = now() WHERE id = $1")
        .bind(machine)
        .execute(&test.pool)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_database_that_does_not_answer_is_named_without_its_password() {
    let dir = tempfile::tempdir().unwrap();
    let config = configuration(dir.path(), "127.0.0.1:1");
    let arguments = vec![
        "--only".to_owned(),
        "database,worker".to_owned(),
        "--config".to_owned(),
        config.display().to_string(),
    ];
    let environment = vec![(
        "ORCH_DATABASE_URL".to_owned(),
        "postgres://orch:hunter2-secret@127.0.0.1:1/orch".to_owned(),
    )];
    let ran = run_doctor(arguments, environment).await;
    assert_eq!(ran.status, 1);
    assert!(
        ran.line("database").contains("127.0.0.1:1/orch"),
        "{}",
        ran.line("database")
    );
    assert!(
        !ran.lines.join("\n").contains("hunter2"),
        "the password was printed"
    );
    assert!(ran.says("worker", "skip"));
}

/// A server that answers like the API on its own listener, under a certificate of the
/// authority in `secrets`, good for the name `api` and not for the address it listens on.
async fn serve_api_under_an_authority(secrets: &Path, version: &'static str) -> SocketAddr {
    let authority = CertificateAuthority::create("Test", 30, &SystemClock).unwrap();
    std::fs::write(
        secrets.join("orch_ca_cert.pem"),
        authority.certificate_pem(),
    )
    .unwrap();
    let (key, certificate) = authority
        .issue_with_new_key(
            &Holder::Service {
                service: ServiceName::Api,
                names: vec!["api".to_owned()],
            },
            30,
            &SystemClock,
        )
        .unwrap();
    let identity = TlsIdentity {
        chain: vec![certificate],
        key,
    };
    let tls = tokio_rustls::TlsAcceptor::from(
        plain_server_config(&identity, Arc::new(SystemClock)).unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(
            "/api/v1/meta",
            get(move || async move {
                axum::Json(
                    serde_json::json!({ "version": version, "build": "7", "needs_setup": true }),
                )
            }),
        );
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let (tls, app) = (tls.clone(), app.clone());
            tokio::spawn(async move {
                let Ok(stream) = tls.accept(socket).await else {
                    return;
                };
                let service = hyper_util::service::TowerToHyperService::new(app);
                let _ = hyper_util::server::conn::auto::Builder::new(
                    hyper_util::rt::TokioExecutor::new(),
                )
                .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                .await;
            });
        }
    });
    address
}

#[tokio::test(flavor = "multi_thread")]
async fn the_api_is_tried_on_a_loopback_address_by_the_name_its_certificate_is_good_for() {
    let dir = tempfile::tempdir().unwrap();
    let address = serve_api_under_an_authority(dir.path(), "0.1.0").await;
    let config = configuration(dir.path(), &address.to_string());
    let base = |expect: Option<&str>| {
        let mut arguments = vec![
            "--only".to_owned(),
            "api".to_owned(),
            "--config".to_owned(),
            config.display().to_string(),
            "--secrets-dir".to_owned(),
            dir.path().display().to_string(),
        ];
        if let Some(expect) = expect {
            arguments.extend(["--expect-version".to_owned(), expect.to_owned()]);
        }
        arguments
    };
    let good = run_doctor(base(Some("0.1.0")), Vec::new()).await;
    assert_eq!(good.status, 0, "{:#?}", good.lines);
    assert!(
        good.line("api").contains("release 0.1.0 (build 7)"),
        "{}",
        good.line("api")
    );

    let other = run_doctor(base(Some("0.2.0")), Vec::new()).await;
    assert_eq!(other.status, 1);
    assert!(other.line("api").contains("release 0.2.0 was expected"));

    // A certificate of another authority is not the system's.
    let stranger = CertificateAuthority::create("Stranger", 30, &SystemClock).unwrap();
    std::fs::write(
        dir.path().join("orch_ca_cert.pem"),
        stranger.certificate_pem(),
    )
    .unwrap();
    let refused = run_doctor(base(None), Vec::new()).await;
    assert_eq!(refused.status, 1);
    assert!(
        refused.line("api").contains("TLS"),
        "{}",
        refused.line("api")
    );
}

/// What the stand-in for an edge does.
#[derive(Clone, Copy)]
struct Edge {
    /// The most a body may be.
    body_limit: usize,
    /// Whether the page comes with its headers.
    headers: bool,
    /// When the stand-in for the API ends a socket that does not sign in.
    ends_socket_after: Option<Duration>,
}

async fn serve_edge(edge: Edge) -> SocketAddr {
    async fn page(headers: bool) -> Response {
        let mut response = (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html")],
            "<html></html>",
        )
            .into_response();
        if headers {
            for name in [
                "content-security-policy",
                "x-content-type-options",
                "x-frame-options",
                "referrer-policy",
                "permissions-policy",
            ] {
                response
                    .headers_mut()
                    .insert(name, HeaderValue::from_static("set"));
            }
        }
        response
    }
    async fn socket(
        upgrade: WebSocketUpgrade,
        protocol: &'static str,
        ends: Option<Duration>,
    ) -> Response {
        upgrade
            .protocols([protocol])
            .on_upgrade(move |mut socket: WebSocket| async move {
                match ends {
                    Some(after) => {
                        tokio::time::sleep(after).await;
                        let _ = socket
                            .send(Message::Close(Some(CloseFrame {
                                code: 4401,
                                reason: "no auth within the time allowed".into(),
                            })))
                            .await;
                    }
                    None => while socket.recv().await.is_some() {},
                }
            })
    }
    let app = Router::new()
        .route("/", get(move || page(edge.headers)))
        .route("/health", get(|| async { "ok" }))
        .route(
            "/edge-check",
            post(|_body: axum::body::Bytes| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/api/v1/meta",
            get(|| async {
                axum::Json(serde_json::json!({ "version": "0.1.0", "build": "3", "needs_setup": false, "link_endpoints": [] }))
            }),
        )
        .route(
            "/api/v1/ws",
            get(move |upgrade: WebSocketUpgrade| socket(upgrade, "orch.v1", edge.ends_socket_after)),
        )
        .route(
            "/link/v1/session",
            get(|upgrade: WebSocketUpgrade| socket(upgrade, "orch-link.v1", None)),
        )
        // The limit of this edge is the layer's; the extractor has none of its own.
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(edge.body_limit));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    address
}

fn edge_arguments(address: SocketAddr, only: &str, idle_secs: u32) -> Vec<String> {
    vec![
        "--url".to_owned(),
        format!("http://{address}"),
        "--only".to_owned(),
        only.to_owned(),
        "--idle-secs".to_owned(),
        idle_secs.to_string(),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edge_that_does_its_part_passes_every_check_it_is_tried_by() {
    let address = serve_edge(Edge {
        body_limit: 16 * 1024 * 1024,
        headers: true,
        ends_socket_after: None,
    })
    .await;
    let ran = run_doctor(
        edge_arguments(address, "edge-page,edge-api,edge-body,edge-ws,edge-link", 3),
        Vec::new(),
    )
    .await;
    assert_eq!(ran.status, 0, "{:#?}", ran.lines);
    for id in ["edge-page", "edge-api", "edge-body", "edge-ws", "edge-link"] {
        assert!(ran.says(id, "ok"), "{}", ran.line(id));
    }
    assert!(ran.line("edge-api").contains("release 0.1.0"));
    assert!(
        ran.line("edge-body").contains("8388608 bytes"),
        "{}",
        ran.line("edge-body")
    );
    assert!(ran.line("edge-ws").contains("held it idle for 3 s"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_edge_that_removes_headers_or_refuses_a_large_body_is_named() {
    let address = serve_edge(Edge {
        body_limit: 1024 * 1024,
        headers: false,
        ends_socket_after: None,
    })
    .await;
    let ran = run_doctor(
        edge_arguments(address, "edge-page,edge-body", 3),
        Vec::new(),
    )
    .await;
    assert_eq!(ran.status, 1);
    assert!(
        ran.line("edge-page")
            .contains("without content-security-policy"),
        "{}",
        ran.line("edge-page")
    );
    assert!(ran.says("edge-body", "FAIL"), "{}", ran.line("edge-body"));
    // Whether the refusal reaches the client as a 413 or as a connection cut while it is
    // still sending depends on how fast the other end closes; either is a refusal.
    assert!(
        ran.line("edge-body").contains("8 MiB"),
        "{}",
        ran.line("edge-body")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_socket_the_api_ends_itself_is_a_part_of_the_check_and_says_why() {
    let address = serve_edge(Edge {
        body_limit: 16 * 1024 * 1024,
        headers: true,
        ends_socket_after: Some(Duration::from_secs(1)),
    })
    .await;
    let ran = run_doctor(edge_arguments(address, "edge-ws", 6), Vec::new()).await;
    assert_eq!(ran.status, 0, "{:#?}", ran.lines);
    assert!(ran.says("edge-ws", "part"), "{}", ran.line("edge-ws"));
    assert!(
        ran.line("edge-ws").contains("4401"),
        "{}",
        ran.line("edge-ws")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_endpoint_that_cannot_be_reached_is_named_with_its_trust() {
    // An edge that advertises one pinned endpoint at a port nothing listens on.
    let app = Router::new().route(
        "/api/v1/meta",
        get(|| async {
            axum::Json(serde_json::json!({
                "version": "0.1.0",
                "link_endpoints": [ { "url": "wss://127.0.0.1:1", "trust": "public" } ]
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let ran = run_doctor(edge_arguments(address, "edge-endpoints", 3), Vec::new()).await;
    assert_eq!(ran.status, 1);
    let line = ran.line("edge-endpoint-1");
    assert!(line.starts_with("FAIL"), "{line}");
    assert!(line.contains("wss://127.0.0.1:1 (public)"), "{line}");
}
