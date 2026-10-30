//! The API as a program, started the way a deployment starts it: from a configuration file and a directory of secrets, with the browser's listener in plain HTTP. What it must do from that start alone is serve the first setup, make an org, and seal with the master key it was given every kind of secret a person can store: an authenticator, a secret of a node, and the key of a provider. To run it by hand, `cargo test -p orch-api --test program -- --nocapture`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
// The program is stopped the way a service manager stops it, with a termination signal.
#![cfg(unix)]

use orch_pki::ca::{CA_VALIDITY_DAYS, CertificateAuthority, Holder};
use orch_pki::certificates::{ServiceName, write_file, write_private_file};
use orch_testkit::database::TestDb;
use orch_types::SystemClock;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

/// How long the test waits for something that has to happen, in real time.
const PATIENCE: Duration = Duration::from_secs(60);

/// How long a certificate of the test's authority lasts, in days.
const LEAF_DAYS: u64 = 30;

/// The password of the one person the test makes: long enough, and nothing like a name.
const PASSWORD: &str = "correct horse battery";

/// The range reserved for local integration listeners on the shared test machine.
const TEST_PORTS: std::ops::RangeInclusive<u16> = 41000..=41999;

/// A free local integration port, different from the other listener of this program.
fn free_port(other: Option<u16>) -> u16 {
    TEST_PORTS
        .filter(|port| Some(*port) != other)
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .expect("a local integration listener is available")
}

/// What `orchctl compose-secrets` makes, as far as the API reads it: the authority, the master key
/// (returned, in hexadecimal) and the API's own certificate.
fn secrets_in(dir: &Path) -> String {
    let clock = SystemClock;
    let authority = CertificateAuthority::create("Test", CA_VALIDITY_DAYS, &clock).unwrap();
    write_private_file(&dir.join("orch_ca_key.pem"), &authority.key_pem()).unwrap();
    write_file(&dir.join("orch_ca_cert.pem"), &authority.certificate_pem()).unwrap();
    let master = "5c".repeat(32);
    write_private_file(&dir.join("orch_master_key"), &format!("{master}\n")).unwrap();
    let (key, certificate) = authority
        .issue_with_new_key(
            &Holder::Service {
                service: ServiceName::Api,
                names: vec!["127.0.0.1".to_owned()],
            },
            LEAF_DAYS,
            &clock,
        )
        .unwrap();
    write_private_file(&dir.join("service_api_key.pem"), &key.to_pem()).unwrap();
    write_file(&dir.join("service_api_cert.pem"), &certificate.pem()).unwrap();
    master
}

/// The program, running, and what it needs to be stopped.
struct Api {
    child: Child,
    base: String,
    client: reqwest::Client,
}

impl Api {
    async fn start(dir: &Path, database: &str) -> Self {
        let secrets = dir.join("secrets");
        std::fs::create_dir(&secrets).unwrap();
        secrets_in(&secrets);
        let browser = free_port(None);
        let internal = free_port(Some(browser));
        let config = dir.join("orch.toml");
        std::fs::write(
            &config,
            format!(
                "version = 1\n[api]\nbind = \"127.0.0.1:{browser}\"\ninternal_bind = \"127.0.0.1:{internal}\"\ndb_pool = 4\n\
                 [database]\nmigrations_dir = \"{}\"\n[storage]\nroot = \"{}\"\n",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations"),
                dir.join("storage").display(),
            ),
        )
        .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_orch-api"))
            .arg("--config")
            .arg(&config)
            .arg("--secrets-dir")
            .arg(&secrets)
            .arg("--plain-http")
            .env("ORCH_DATABASE_URL", database)
            .env(
                "ORCH_MIGRATION_STATUS_FILE",
                dir.join("update-status/migrations.json"),
            )
            // Neither a file named by the environment nor the container runtime's path may help:
            // the program is to use the secrets directory it was given.
            .env_remove("ORCH_MASTER_KEY_FILE")
            // The executable must supply its own runtime stack allowance.
            .env_remove("RUST_MIN_STACK")
            .env("RUST_LOG", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let api = Self {
            child,
            base: format!("http://127.0.0.1:{browser}/api/v1"),
            client: reqwest::Client::new(),
        };
        let started = Instant::now();
        loop {
            if let Ok(reply) = api.client.get(format!("{}/meta", api.base)).send().await
                && reply.status().is_success()
            {
                return api;
            }
            assert!(started.elapsed() < PATIENCE, "the API did not come up");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// One request; the answer's status and its body.
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self.client.request(method, format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let reply = request.send().await.unwrap();
        let status = reply.status().as_u16();
        let text = reply.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Stops the program the way a service manager does, and waits for it.
    async fn stop(mut self) {
        let pid = self.child.id().expect("the program is running");
        let status = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        let ended = tokio::time::timeout(PATIENCE, self.child.wait())
            .await
            .expect("the program did not stop");
        assert!(
            ended.unwrap().success(),
            "the program stopped with an error"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_started_from_its_secrets_directory_seals_every_kind_of_secret_with_the_key_it_was_given()
 {
    let database = TestDb::fresh().await;
    let dir = tempfile::tempdir().unwrap();
    let api = Api::start(dir.path(), &database.url).await;
    let status: Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("update-status/migrations.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(status["step"], "complete");
    assert_eq!(status["migrations_done"], status["migrations_total"]);
    assert!(status["migrations_total"].as_u64().unwrap() > 0);
    let post = reqwest::Method::POST;
    let put = reqwest::Method::PUT;

    let (status, setup) = api
        .send(
            post.clone(),
            "/setup",
            None,
            Some(json!({ "username": "ada", "display_name": "Ada", "password": PASSWORD })),
        )
        .await;
    assert_eq!(status, 200, "{setup}");
    let (status, signed_in) = api
        .send(
            post.clone(),
            "/auth/login",
            None,
            Some(json!({ "login": "ada", "password": PASSWORD })),
        )
        .await;
    assert_eq!(status, 200, "first sign-in after setup: {signed_in}");
    assert_eq!(signed_in["user"]["id"], setup["user"]["id"]);
    assert_eq!(signed_in["user"]["is_system_admin"], json!(true));
    let token = signed_in["access_token"].as_str().unwrap().to_owned();
    let token = Some(token.as_str());

    // Setup and sign-in open ordinary sessions. Administrative work is an explicit
    // session choice; the program must make this request and the first creation
    // below without an operator supplying RUST_MIN_STACK.
    let (status, mode) = api
        .send(
            reqwest::Method::PUT,
            "/me/admin-mode",
            token,
            Some(json!({ "as_admin": true })),
        )
        .await;
    assert_eq!(status, 200, "{mode}");

    let (status, org) = api
        .send(
            post.clone(),
            "/orgs",
            token,
            Some(json!({ "name": "Acme", "slug": "acme" })),
        )
        .await;
    assert_eq!(status, 200, "{org}");
    let org = org["id"].as_str().unwrap().to_owned();

    // The key of a provider.
    let (status, provider) = api
        .send(
            post.clone(),
            "/providers",
            token,
            Some(json!({
                "kind": "api",
                "name": "A key",
                "home_id": org,
                "upstreams": [{ "protocol": "openai-responses", "base_url": "https://api.example.com/v1", "options": {} }],
                "api_key": "sk-test-not-a-real-key",
            })),
        )
        .await;
    assert_eq!(status, 200, "a provider's key is sealed: {provider}");
    assert_eq!(provider["key_set"], json!(true));

    // A secret of a node.
    let (status, secret) = api
        .send(
            put,
            &format!("/nodes/{org}/secrets/DEPLOY_TOKEN"),
            token,
            Some(json!({ "value": "not-a-real-token" })),
        )
        .await;
    assert_eq!(status, 200, "a node's secret is sealed: {secret}");

    // An authenticator's secret.
    let (status, authenticator) = api
        .send(
            post,
            "/me/totp/setup",
            token,
            Some(json!({ "password": PASSWORD })),
        )
        .await;
    assert_eq!(
        status, 200,
        "an authenticator's secret is sealed: {authenticator}"
    );

    api.stop().await;
}
