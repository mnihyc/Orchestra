//! The program itself, run as a process against a real gateway and API on loopback: it enrolls a host with a host token, holds its session and tells the system what the host has, says who it is, and stops when it is told to.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::process::{Command, Stdio};

use common::{World, eventually};
use orch_api_core::certificates::TokenFor;

fn node(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_orch-node"))
        .args(args)
        .output()
        .expect("the program could not be run")
}

// The system under test is served by the runtime of the test, so the test must not hold it still while it
// waits for the program: the runtime has threads to spare.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_program_enrolls_a_host_holds_its_session_tells_what_the_host_has_and_stops_when_told_to()
 {
    let mut world = World::start_on("127.0.0.1", "127.0.0.1").await;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("node-data");
    let config = dir.path().join("orch.toml");
    // The built-in host driver is composed through the same node loader as a container driver.
    // A previous release's documented file must still start, with its operator settings kept.
    std::fs::write(
        &config,
        format!(
            r#"
[node]
drivers = ["host"]
[node.hooks]
pre_up = "/bin/true"
post_up = "/bin/true"
pre_down = "/bin/true"
post_down = "/bin/true"
timeout_secs = 17
[[node.storage]]
name = "fast"
path = {:?}
default = true
[node.docker]
socket = "/var/run/docker.sock"
builds = true
[node.docker.networks]
pool = "10.199.0.0/16"
prefix = 28
[ports]
incoming_address = "vm.example.test"
listen = "127.0.0.1"
ranges = [{{ from = 41000, to = 41010 }}]
"#,
            dir.path().join("storage").to_str().unwrap()
        ),
    )
    .unwrap();
    let (config, data) = (config.to_str().unwrap(), data.to_str().unwrap());

    let version = node(&["version"]);
    assert!(version.status.success());
    assert!(String::from_utf8_lossy(&version.stdout).contains("node"));

    let before = node(&["--config", config, "--data-dir", data, "status"]);
    assert!(
        before.status.success(),
        "{}",
        String::from_utf8_lossy(&before.stderr)
    );
    let said = String::from_utf8_lossy(&before.stdout).into_owned();
    assert!(
        said.contains("not enrolled") && said.contains("host"),
        "{said}"
    );
    let migration = String::from_utf8_lossy(&before.stderr);
    for key in ["hooks", "ports", "storages", "drivers"] {
        assert!(
            migration.contains(&format!("moved: now `orch-node policy set {key}")),
            "{migration}"
        );
    }
    let policy = orch_node_core::machine_policy::open(
        &orch_node::composition::settings_of(std::path::Path::new(config), Some(data.into()))
            .unwrap()
            .1,
    )
    .unwrap()
    .policy()
    .unwrap();
    for key in ["hooks", "ports", "storages", "drivers"] {
        assert!(
            policy.local_values.contains_key(key),
            "{key} must remain a local value"
        );
    }
    assert_eq!(policy.hooks.pre_up.as_deref(), Some("/bin/true"));
    assert_eq!(policy.hooks.timeout_secs, 17);
    assert_eq!(
        policy.ports.publish.as_ref().unwrap().incoming_address,
        "vm.example.test"
    );
    assert_eq!(policy.storages[0].name, "fast");
    assert_eq!(policy.drivers.allowed, ["host"]);

    // Running without an identity and without a token says what to do.
    let nothing = node(&["--config", config, "--data-dir", data, "run"]);
    assert!(!nothing.status.success());
    assert!(
        String::from_utf8_lossy(&nothing.stderr).contains("enroll first"),
        "{}",
        String::from_utf8_lossy(&nothing.stderr)
    );

    let hint = "orch-test-node-program-host";
    let token = world.token(TokenFor::Host { home: None }, hint).await;
    let url = world.gateway_url();
    let enrolled = node(&[
        "--config",
        config,
        "--data-dir",
        data,
        "enroll",
        "--url",
        &url,
        "--token",
        &token,
        "--name",
        "program-host",
    ]);
    let out = String::from_utf8_lossy(&enrolled.stdout).into_owned();
    assert!(
        enrolled.status.success(),
        "{} {}",
        out,
        String::from_utf8_lossy(&enrolled.stderr)
    );
    assert!(out.contains("Enrolled program-host"), "{out}");
    let machine = world.wait_enrolled(hint).await;
    // The previous release had no policy provenance marker. Its serialized connector defaults
    // were not local refusals on a node; upgrade must preserve the host's effective behavior.
    let old_data =
        orch_connector_core::enrollment::DataDir::open(std::path::Path::new(data)).unwrap();
    let mut legacy = old_data.config(|_| None).unwrap();
    let endpoints = legacy.endpoints.clone();
    assert!(
        !endpoints.is_empty(),
        "the enrolled node has its gateway addresses"
    );
    legacy.connector.roles = vec!["node".into()];
    legacy.connector.node_policy_imported = false;
    legacy.connector.node_policy_import_version = 0;
    legacy.policy.local_keys = None;
    legacy.policy.values.clear();
    legacy.environment.remote_write = false;
    legacy.environment.terminal = false;
    legacy.tools.fetch = false;
    // A previous fully serialized config lacks provenance, but retains the enrollment's endpoints.
    let keys = orch_link_contract::policy::PolicyKey::ALL
        .iter()
        .map(|key| key.as_str().to_owned())
        .collect::<Vec<_>>();
    let legacy = orch_connector_core::policy::sparse_config(&legacy, &keys).unwrap();
    std::fs::write(old_data.config_path(), legacy).unwrap();
    assert_eq!(old_data.config(|_| None).unwrap().endpoints, endpoints);
    // The token was single-use: a second enrollment into the same directory is refused before anything is sent.
    let again = node(&[
        "--config",
        config,
        "--data-dir",
        data,
        "enroll",
        "--url",
        &url,
        "--token",
        &token,
    ]);
    assert!(!again.status.success());

    let after = node(&["--config", config, "--data-dir", data, "status"]);
    let said = String::from_utf8_lossy(&after.stdout).into_owned();
    assert!(
        said.contains("program-host") && said.contains(&machine.to_string()),
        "{said}"
    );
    let upgraded = orch_node_core::machine_policy::open(
        &orch_node::composition::settings_of(std::path::Path::new(config), Some(data.into()))
            .unwrap()
            .1,
    )
    .unwrap()
    .policy()
    .unwrap();
    assert!(
        upgraded.environment_remote_write && upgraded.environment_terminal && upgraded.tools_fetch,
        "previously Orch-controlled node features remain available after upgrade"
    );
    assert!(
        upgraded.local_values.contains_key("hooks") && upgraded.local_values.contains_key("ports")
    );

    // It runs: its session stands, and it tells what the host has, with the driver it could not load missing.
    let log = std::fs::File::create(dir.path().join("node.log")).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_orch-node"))
        .args(["--config", config, "--data-dir", data, "run"])
        .stdout(Stdio::null())
        .stderr(log)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    world.wait_online(machine, true).await;
    eventually("the node to tell what the host has", || {
        let world = &world;
        async move {
            world
                .told(machine, "node")
                .await
                .iter()
                .any(|(kind, body)| {
                    kind == "node.facts" && body["capacity"]["cpus"].as_i64().unwrap_or(0) >= 1
                })
        }
    })
    .await;
    let facts = world
        .told(machine, "node")
        .await
        .into_iter()
        .find(|(kind, _)| kind == "node.facts")
        .unwrap()
        .1;
    assert_eq!(facts["drivers"]["host"]["confinement"], "none");
    assert_eq!(facts["drivers"]["host"]["process_scope"], "started");
    assert_eq!(facts["drivers"]["host"]["recreate"], "restart");
    let log_text = std::fs::read_to_string(dir.path().join("node.log")).unwrap();
    assert!(log_text.contains("the driver is loaded"), "{log_text}");

    // Told to stop, it ends its session and exits well.
    let pid = child.id().unwrap().to_string();
    assert!(
        Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap()
            .success()
    );
    let status = child.wait().await.unwrap();
    assert!(
        status.success(),
        "{status}: {}",
        std::fs::read_to_string(dir.path().join("node.log")).unwrap()
    );
    world.wait_online(machine, false).await;
    world.stop().await;
}
