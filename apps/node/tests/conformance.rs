//! A scripted cached build runs only inside the node's disposable networkless conformance unit.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use orch_driver_docker::DockerConfig;
use orch_link_contract::commands::NodeToolConformance;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_scripted_build_cannot_certify_another_version_and_its_unit_is_removed() {
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    docker
        .inspect_image("orch-dev-harness")
        .await
        .expect("the pinned harness image is required");
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let program = directory.path().join("codex");
    std::fs::write(
        &program,
        r#"#!/usr/bin/env node
const interfaces = Object.values(require('os').networkInterfaces()).flat();
if (interfaces.some(address => address && !address.internal)) throw new Error('external network is present');
if (process.getuid() === 0) throw new Error('the check runs as root');
if (process.env.HOME !== process.env.CODEX_HOME) throw new Error('the check does not have its own native home');
const readline = require('readline').createInterface({input: process.stdin});
readline.on('line', line => {
  const request = JSON.parse(line);
  if (request.id === undefined) return;
  const result = request.method === 'initialize'
    ? {userAgent: 'orch/0.170.0', codexHome: process.env.CODEX_HOME}
    : {};
  process.stdout.write(JSON.stringify({id: request.id, result}) + '\n');
});
"#,
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = DockerConfig {
        labels: std::collections::BTreeMap::from([(
            "orch.conformance_test".to_owned(),
            directory.path().display().to_string(),
        )]),
        live_helper_binary: Some(
            std::env::var_os("ORCH_TEST_CONFORMANCE_NODE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_orch-node").into()),
        ),
        conformance_image: Some("orch-dev-harness".to_owned()),
        ..Default::default()
    };
    let request = NodeToolConformance {
        name: "codex".to_owned(),
        version: "0.171.0".to_owned(),
        programs: vec!["codex".to_owned()],
        case_timeout_secs: 5,
        timeout_secs: 60,
    };
    let began = std::time::Instant::now();
    let result = orch_node::conformance_unit::run(
        &docker,
        &config,
        &request,
        &program,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.cases.len(),
        orch_agent_programs::Programs::builtin()
            .program("codex")
            .unwrap()
            .checks
            .cases()
            .count()
    );
    assert!(
        result.cases.iter().all(|case| !case.passed
            && case
                .error
                .as_ref()
                .is_some_and(|error| error.contains("0.170.0") && error.contains("0.171.0"))),
        "{result:?}"
    );
    assert!(began.elapsed() < Duration::from_secs(60));
    let units = docker
        .list_containers(Some(
            bollard::query_parameters::ListContainersOptionsBuilder::default()
                .all(true)
                .filters(&std::collections::HashMap::from([(
                    "label".to_owned(),
                    vec![format!(
                        "orch.conformance_test={}",
                        directory.path().display()
                    )],
                )]))
                .build(),
        ))
        .await
        .unwrap();
    assert!(units.is_empty(), "the runner leaves no owned unit behind");
}
