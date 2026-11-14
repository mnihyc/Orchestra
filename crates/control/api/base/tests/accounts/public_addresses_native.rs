//! The shipped watcher and real nginx workers adopt the prepared certificate before publication.

use super::*;
use std::path::Path;

// Bound daemon and fixture startup waits match the existing native Docker fixtures.
const DAEMON_WAIT: Wait = Wait::from_secs(120);
const START_WAIT: Wait = Wait::from_secs(30);
const POLL: Wait = Wait::from_millis(25);
const IMAGE: &str =
    "nginx:1.30-alpine@sha256:0985e772fb9f729e6fa0980da05fca5d9c468e870eed43071545afa9d2e27d94";

async fn docker(args: &[&str]) -> String {
    let output = tokio::time::timeout(
        DAEMON_WAIT,
        tokio::process::Command::new("docker")
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

struct Front {
    name: String,
}
impl Drop for Front {
    fn drop(&mut self) {
        // Only this fixture's intrinsic container name is removed, including assertion unwinds.
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}
fn unused_ports() -> (u16, u16) {
    let first = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let second = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    (
        first.local_addr().unwrap().port(),
        second.local_addr().unwrap().port(),
    )
}

fn mount(path: &Path, target: &str) -> String {
    format!("{}:{target}:ro", path.display())
}

async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(START_WAIT, async {
        while !ready() {
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn real_frontend_workers_must_reload_before_publication_and_a_failed_reload_keeps_the_old_address()
 {
    let root = tempfile::tempdir().unwrap();
    let storage = root.path().join("storage");
    let api_listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let (first_port, second_port) = unused_ports();
    let mut config = orch_config::ServerConfig::default();
    config.storage.root = storage.to_string_lossy().into_owned();
    config.system.public_url = format!("https://127.0.0.1:{first_port}");
    let world = World::with(config.clone()).await;
    world
        .app
        .app
        .link
        .certificates()
        .keep_frontend_certificate(&config)
        .await
        .unwrap();
    let initial = effective::current(&world.app.app).await.unwrap();
    let api_port = api_listener.local_addr().unwrap().port();
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = axum::Router::new()
        .nest("/api/v1", registry.into_router())
        .with_state(world.app.app.clone());
    let _api = Edge {
        url: String::new(),
        serving: tokio::spawn(async move {
            axum::serve(api_listener, router).await.unwrap();
        }),
    };
    let script = root.path().join("frontend.sh");
    std::fs::write(
        &script,
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../deploy/frontend.sh"
        )),
    )
    .unwrap();
    let nginx = root.path().join("nginx.conf");
    std::fs::write(
        &nginx,
        format!(
            r#"
pid /tmp/nginx.pid;
user nginx;
error_log /dev/stderr warn;
events {{}}
http {{
  include /tmp/frontend-generation.conf;
  server {{
    listen 127.0.0.1:8081;
    location = /frontend-generation {{ return 200 $orch_frontend_generation; }}
    location / {{ return 204; }}
  }}
  server {{
    listen 80;
    include /tmp/frontend-https.conf;
    location = /api/v1/public-address/proof {{
      proxy_pass http://host.docker.internal:{api_port}/api/v1/public-address/proof;
      add_header X-Orch-Frontend-Generation $orch_frontend_generation always;
    }}
  }}
}}
"#
        ),
    )
    .unwrap();
    let gate = root.path().join("gate");
    let bin = root.path().join("bin");
    std::fs::create_dir(&gate).unwrap();
    std::fs::create_dir(&bin).unwrap();
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    std::fs::set_permissions(&gate, std::fs::Permissions::from_mode(0o770)).unwrap();
    let wrapper = bin.join("nginx");
    std::fs::write(
        &wrapper,
        r#"#!/bin/sh
if [ "$#" -eq 2 ] && [ "$1" = '-s' ] && [ "$2" = 'reload' ]; then
  if [ -e /test-gate/fail ]; then exit 1; fi
  if [ -e /test-gate/pretend ]; then printf '%s\n' accepted > /test-gate/accepted; exit 0; fi
fi
exec /usr/sbin/nginx "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let front = Front {
        name: format!("orch-address-proof-{}", orch_types::new_id().simple()),
    };
    let p1 = format!("127.0.0.1:{first_port}:443");
    let p2 = format!("127.0.0.1:{second_port}:443");
    docker(&[
        "run",
        "--detach",
        "--name",
        &front.name,
        "--read-only",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/var/cache/nginx",
        "--cap-drop",
        "ALL",
        "--cap-add",
        "CHOWN",
        "--cap-add",
        "SETGID",
        "--cap-add",
        "SETUID",
        "--cap-add",
        "NET_BIND_SERVICE",
        "--security-opt",
        "no-new-privileges:true",
        "--group-add",
        &std::fs::metadata(&storage).unwrap().gid().to_string(),
        "--add-host",
        "host.docker.internal:host-gateway",
        "--publish",
        &p1,
        "--publish",
        &p2,
        "--volume",
        &mount(&storage.join("frontend-tls"), "/etc/orch/frontend-tls"),
        "--volume",
        &mount(&script, "/test-frontend.sh"),
        "--volume",
        &mount(&nginx, "/etc/nginx/nginx.conf"),
        "--volume",
        &format!("{}:/test-gate", gate.display()),
        "--volume",
        &mount(&bin, "/test-bin"),
        "--env",
        "ORCH_FRONTEND_HTTPS_BIND=443",
        "--env",
        "PATH=/test-bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        IMAGE,
        "/bin/sh",
        "/test-frontend.sh",
    ])
    .await;
    let ca = reqwest::Certificate::from_pem(
        world
            .app
            .app
            .link
            .certificates()
            .authority_certificate()
            .pem()
            .as_bytes(),
    )
    .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .tls_certs_merge([ca])
        .timeout(START_WAIT)
        .build()
        .unwrap();
    tokio::time::timeout(START_WAIT, async {
        loop {
            if client
                .get(format!(
                    "{}/api/v1/public-address/proof?challenge={}",
                    initial.addresses.public_url,
                    orch_types::new_id()
                ))
                .send()
                .await
                .is_ok_and(|reply| reply.status().is_success())
            {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .unwrap();
    let pointer = storage.join("frontend-tls/current");
    let old = std::fs::read_link(&pointer).unwrap();
    std::fs::write(gate.join("pretend"), "acknowledge without reloading").unwrap();
    let primary = format!("https://localhost:{first_port}");
    let candidate = PublicAddresses {
        public_url: primary.clone(),
        alternate_urls: Vec::new(),
    };
    let app = world.app.app.clone();
    let options = PrepareOptions {
        managed_frontend: Some(initial.addresses.public_url.clone()),
        timeout: START_WAIT,
        poll: POLL,
    };
    let preparing = tokio::spawn(async move {
        effective::prepare(&app, candidate, initial.revision, options).await
    });
    until(|| {
        std::fs::read_link(&pointer)
            .ok()
            .as_ref()
            .is_some_and(|current| current != &old)
    })
    .await;
    assert!(
        !preparing.is_finished(),
        "a staged pointer is not a worker acknowledgement"
    );
    assert_eq!(
        effective::current(&world.app.app)
            .await
            .unwrap()
            .addresses
            .public_url,
        config.system.public_url
    );
    until(|| gate.join("accepted").exists()).await;
    assert!(
        !preparing.is_finished(),
        "a successful reload signal is not a loaded worker"
    );
    let contender_app = world.app.app.clone();
    let contender_origin = format!("https://localhost:{second_port}");
    let contender_revision = effective::current(&world.app.app).await.unwrap().revision;
    let contender = tokio::spawn(async move {
        effective::prepare(
            &contender_app,
            PublicAddresses {
                public_url: contender_origin,
                alternate_urls: Vec::new(),
            },
            contender_revision,
            PrepareOptions {
                managed_frontend: None,
                timeout: START_WAIT,
                poll: POLL,
            },
        )
        .await
    });
    std::fs::rename(gate.join("pretend"), gate.join("released")).unwrap();
    let mut prepared = preparing.await.unwrap().unwrap();
    assert_eq!(
        effective::current(&world.app.app)
            .await
            .unwrap()
            .addresses
            .public_url,
        config.system.public_url
    );
    let mut tx = world.app.app.db.begin().await.unwrap();
    let published = prepared.publish(&mut tx, None, world.now()).await.unwrap();
    tx.commit().await.unwrap();
    prepared.release().await.unwrap();
    assert_eq!(published.addresses.public_url, primary);
    let stale = contender
        .await
        .unwrap()
        .err()
        .expect("a waiting draft is checked against the committed winner");
    assert_eq!(stale.code(), ErrorCode::ConflictStaleRevision);

    std::fs::write(gate.join("fail"), "refuse reload").unwrap();
    let refused = effective::prepare(
        &world.app.app,
        PublicAddresses {
            public_url: format!("https://localhost:{second_port}"),
            alternate_urls: Vec::new(),
        },
        published.revision,
        PrepareOptions {
            managed_frontend: Some(format!("https://127.0.0.1:{first_port}")),
            timeout: Wait::from_secs(3),
            poll: POLL,
        },
    )
    .await;
    let refused = refused
        .err()
        .expect("a failed worker reload cannot publish even a port-only change");
    assert_eq!(refused.code(), ErrorCode::PublicAddressFrontendNotReady);
    assert_eq!(effective::current(&world.app.app).await.unwrap(), published);
    assert!(
        client
            .get(format!(
                "{}/api/v1/public-address/proof?challenge={}",
                published.addresses.public_url,
                orch_types::new_id()
            ))
            .send()
            .await
            .unwrap()
            .status()
            .is_success(),
        "the old browser address remains usable"
    );
}
