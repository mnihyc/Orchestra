//! The shipped nginx edge preserves caller addresses and cannot give a forged prefix another sign-in count.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_api_core::registry::Registry;
use orch_api_core::server;
use orch_config::ServerConfig;
use orch_pki::ca::{CertificateAuthority, Holder};
use orch_pki::certificates::ServiceName;
use orch_pki::verification::TlsIdentity;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use support::World;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
}

async fn listener() -> TcpListener {
    // Stay in the shared machine's assigned test-port range, avoiding its
    // development ports; failed binds are retried rather than taken over.
    for _ in 0..100 {
        let port = rand::random_range(41000..42000);
        if (41280..=41299).contains(&port) {
            continue;
        }
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
            return listener;
        }
    }
    panic!("no free loopback port for the forwarded-address edge check");
}

struct Edge {
    name: String,
    port: u16,
    stop: CancellationToken,
    api: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Drop for Edge {
    fn drop(&mut self) {
        self.stop.cancel();
        self.api.abort();
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

fn curl(port: u16, address: &str, name: Option<&str>, forged: Option<&str>) -> (u16, String) {
    let mut command = Command::new("curl");
    command.args([
        "--silent",
        "--show-error",
        "--noproxy",
        "*",
        "--max-time",
        "5",
        "--interface",
        address,
        "--write-out",
        "\n%{http_code}",
    ]);
    let path = if let Some(name) = name {
        command.args([
            "--header",
            "content-type: application/json",
            "--data",
            &json!({"login": name, "password": "wrong password"}).to_string(),
        ]);
        "/api/v1/auth/login"
    } else {
        "/health"
    };
    if let Some(forged) = forged {
        command.args(["--header", &format!("X-Forwarded-For: {forged}")]);
    }
    let output = command
        .arg(format!("http://127.0.0.1:{port}{path}"))
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    let (body, status) = text.rsplit_once('\n').unwrap_or((&text, "0"));
    (status.parse().unwrap_or_default(), body.to_owned())
}

async fn edge(world: &World) -> Edge {
    // This fixture's readiness poll represents a healthy deployment. The worker
    // is not part of this address/auth test, so supply its named held-clock beat.
    let now = world.app.app.clock.now();
    orch_db::heartbeat::beat(
        &mut *world.conn().await,
        "worker",
        orch_types::new_id(),
        "edge-fixture",
        now,
        now,
    )
    .await
    .unwrap();
    let socket = listener().await;
    let api_port = socket.local_addr().unwrap().port();
    let front = listener().await;
    let front_port = front.local_addr().unwrap().port();
    let body = listener().await;
    let body_port = body.local_addr().unwrap().port();
    let names = listener().await;
    let names_port = names.local_addr().unwrap().port();
    let name = format!("fix-forwarded-nginx-{}", orch_types::new_id().simple());
    let directory: PathBuf = root().join(".tmp").join(&name);
    std::fs::create_dir_all(&directory).unwrap();
    let authority =
        CertificateAuthority::create("Forwarded edge test", 3650, &world.app.clock).unwrap();
    let (key, certificate) = authority
        .issue_with_new_key(
            &Holder::Service {
                service: ServiceName::Api,
                names: vec!["api".to_owned()],
            },
            365,
            &world.app.clock,
        )
        .unwrap();
    std::fs::write(directory.join("ca.pem"), authority.certificate_pem()).unwrap();
    let tls = server::browser_tls_config(
        &TlsIdentity {
            chain: vec![certificate],
            key,
        },
        Arc::new(world.app.clock.clone()),
    )
    .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = server::router(world.app.app.clone(), registry);
    let stop = CancellationToken::new();
    let api = tokio::spawn(server::serve_tls(socket, tls, router, stop.clone()));
    let fixture = Edge {
        name,
        port: front_port,
        stop,
        api,
    };

    // Only fixture listener/upstream addresses and worker count differ. The
    // released routing, TLS verification and forwarding directives are used.
    let nginx = std::fs::read_to_string(root().join("deploy/nginx.conf"))
        .unwrap()
        .replace("worker_processes auto;", "worker_processes 1;")
        .replace(
            "listen 80 default_server;",
            &format!("listen 127.0.0.1:{front_port} default_server;"),
        )
        .replace("127.0.0.1:8081", &format!("127.0.0.1:{body_port}"))
        .replace(
            "        listen 80;\n",
            &format!("        listen 127.0.0.1:{names_port};\n"),
        )
        .replace("https://api:8443", &format!("https://127.0.0.1:{api_port}"))
        .replace(
            "https://gateway:9444",
            &format!("https://127.0.0.1:{api_port}"),
        );
    std::fs::write(directory.join("nginx.conf"), nginx).unwrap();
    drop(front);
    drop(body);
    drop(names);
    let output = Command::new("docker")
        .args([
            "run",
            "--detach",
            "--rm",
            "--name",
            &fixture.name,
            "--network",
            "host",
            "--tmpfs",
            "/var/cache/nginx",
            "--tmpfs",
            "/tmp",
            "--mount",
            &format!(
                "type=bind,src={},dst=/etc/nginx/nginx.conf,readonly",
                directory.join("nginx.conf").display()
            ),
            "--mount",
            &format!(
                "type=bind,src={},dst=/etc/orch/secrets/orch_ca_cert.pem,readonly",
                directory.join("ca.pem").display()
            ),
            "--entrypoint",
            "nginx",
            "nginx:1.30-alpine",
            "-g",
            "daemon off;",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for _ in 0..100 {
        if curl(front_port, "127.0.0.2", None, None).0 == 200 {
            return fixture;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let logs = Command::new("docker")
        .args(["logs", &fixture.name])
        .output()
        .unwrap();
    panic!(
        "nginx did not reach the API: {}",
        String::from_utf8_lossy(&logs.stderr)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shipped_edge_counts_the_caller_with_and_without_a_forged_header() {
    let mut config = ServerConfig::default();
    // The test's proxy and caller use distinct loopback addresses, so trust
    // only nginx's address rather than the whole loopback network.
    config.api.trusted_proxy_cidrs = vec!["127.0.0.1/32".to_owned()];
    config.auth.address_failure_limit = 2;
    let world = World::with(config).await;
    let edge = edge(&world).await;
    // A process makes its decoy hash once, at its first check of a name nobody has. Made here, every
    // login the edge passes on checks one password within curl's own limit, as on a warm server; the
    // debug build's hashing is many times slower than a release's.
    orch_api_core::sign_in::passwords::verify("warm", None).await;
    assert_eq!(curl(edge.port, "127.0.0.2", Some("ghost-one"), None).0, 401);
    assert_eq!(
        curl(
            edge.port,
            "127.0.0.2",
            Some("ghost-two"),
            Some("198.51.100.41")
        )
        .0,
        401
    );
    assert_eq!(
        curl(
            edge.port,
            "127.0.0.2",
            Some("ghost-three"),
            Some("198.51.100.42")
        )
        .0,
        429
    );
    assert_eq!(
        curl(
            edge.port,
            "127.0.0.3",
            Some("another-caller"),
            Some("198.51.100.41")
        )
        .0,
        401
    );
    let buckets: Vec<_> = world
        .buckets()
        .await
        .into_iter()
        .filter(|bucket| bucket.starts_with("address|"))
        .collect();
    assert_eq!(buckets, ["address|127.0.0.2", "address|127.0.0.3"]);
    let origins: Vec<String> = sqlx::query_scalar("SELECT DISTINCT host(ip) FROM audit_log WHERE action = 'auth.login_failed' ORDER BY host(ip)")
        .fetch_all(&world.db.pool).await.unwrap();
    assert_eq!(origins, ["127.0.0.2", "127.0.0.3"]);
}

#[test]
fn compose_knows_its_proxy_subnet_and_both_services_trust_that_same_range() {
    for subnet in [None, Some("172.29.240.0/24")] {
        let mut command = Command::new("docker");
        command
            .args(["compose", "--file"])
            .arg(root().join("deploy/compose.yml"))
            .args([
                "--project-name",
                "fix-forwarded-config",
                "config",
                "--format",
                "json",
            ])
            .env("ORCH_UID", "1000")
            .env("ORCH_GID", "1000")
            .env("ORCH_DOCKER_GID", "0")
            .env("POSTGRES_PASSWORD", "test-only")
            .env("ORCH_HOME", root())
            .env_remove("ORCH_CONTROL_CIDR")
            .env_remove("ORCH_API_TRUSTED_PROXY_CIDRS");
        if let Some(subnet) = subnet {
            command.env("ORCH_CONTROL_CIDR", subnet);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let compose: Value = serde_json::from_slice(&output.stdout).unwrap();
        let expected = subnet.unwrap_or("172.30.0.0/24");
        assert_eq!(
            compose["networks"]["control"]["ipam"]["config"][0]["subnet"],
            expected
        );
        for service in ["api", "gateway"] {
            assert_eq!(
                compose["services"][service]["environment"]["ORCH_API_TRUSTED_PROXY_CIDRS"],
                format!("[\"{expected}\"]")
            );
        }
    }
}
