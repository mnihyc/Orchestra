//! Local program versions are probed without an environment's variables and remain bounded.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use orch_agent_contract::descriptor::SandboxMode;
use orch_executor_codex::server::CodexExecServer;
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::executor::Executor;
use serde_json::json;

fn context(directory: &Path, program: &Path) -> EnvironmentContext {
    serde_json::from_value(json!({
        "environment_id":"0199c1a0-0000-7000-8000-00000000000a","generation":1,
        "data_dir":directory,"workspace_root":directory,"home":null,
        "tools_dir":null,"tools":[],"local_programs":{"codex":program},"mounts":[],
        "user":"agent","shell":{"name":"sh","path":"/bin/sh"},
        "os":"linux","arch":"x86_64","path_style":"posix","limits":{},
        "variables":{"ENVIRONMENT_SECRET":"synthetic","PATH":"SHOULD_NOT_PROPAGATE"},
        "mcp_servers_enabled":false
    }))
    .unwrap()
}

fn program(directory: &Path, body: &str) -> std::path::PathBuf {
    let path = directory.join("controlled-codex");
    std::fs::write(
        &path,
        format!("#!/bin/sh\nif [ \"$1\" != --version ]; then exit 1; fi\n{body}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[tokio::test]
async fn a_local_version_probe_uses_connector_path_and_no_environment_variables() {
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let connector_path = std::env::var("PATH").unwrap();
    let expected_path = format!("'{}'", connector_path.replace('\'', "'\\''"));
    let body = format!(
        r#"
        expected_path={expected_path}
        [ -z "${{ENVIRONMENT_SECRET+x}}" ] || exit 1
        [ "$PATH" = "$expected_path" ] || exit 1
        printf 'controlled v0.165.0 0.166.0\n'
    "#
    );
    let program = program(directory.path(), &body);
    let exec = CodexExecServer::new();
    let available = exec.available(&context(directory.path(), &program)).await;
    assert!(available.available, "{available:?}");
    assert_eq!(available.version.as_deref(), Some("0.165.0"));
    assert_eq!(available.sandbox_modes, [SandboxMode::Open]);
}

#[tokio::test]
async fn an_existing_local_program_with_no_semantic_version_is_still_reported_as_unknown() {
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let program = program(directory.path(), "printf 'controlled vv0.165.0\n'");
    let exec = CodexExecServer::new();
    let available = exec.available(&context(directory.path(), &program)).await;
    assert!(available.available, "{available:?}");
    assert_eq!(available.version.as_deref(), Some("unknown"));
}

#[tokio::test]
async fn a_failed_or_over_limit_local_version_probe_keeps_the_program_with_an_unknown_version() {
    for body in [
        "printf '0.165.0\n'; exit 1",
        "i=0; while [ $i -lt 4097 ]; do printf x; i=$((i+1)); done; printf ' 0.165.0\n'",
        "exec /bin/sleep 5",
    ] {
        let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let program = program(directory.path(), body);
        let exec = CodexExecServer::new();
        let available = exec.available(&context(directory.path(), &program)).await;
        assert!(available.available, "{available:?}");
        assert_eq!(available.version.as_deref(), Some("unknown"));
        assert_eq!(available.sandbox_modes, [SandboxMode::Open]);
    }
}

#[tokio::test]
async fn local_program_availability_reprobes_the_same_path_after_its_version_changes() {
    let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let selected = program(directory.path(), "printf '0.165.0\n'");
    let context = context(directory.path(), &selected);
    let mut exec = CodexExecServer::new();
    assert_eq!(
        exec.available(&context).await.version.as_deref(),
        Some("0.165.0")
    );
    assert_eq!(program(directory.path(), "printf '0.166.0\n'"), selected);
    assert_eq!(
        exec.available(&context).await.version.as_deref(),
        Some("0.166.0")
    );
    assert_eq!(program(directory.path(), "printf '0.167.0\n'"), selected);
    let refused = exec.start(&context).await.unwrap_err();
    assert_eq!(
        refused.detail(),
        "the confirmed program changed after its availability check (checked: 0.166.0; start: 0.167.0)"
    );
}
