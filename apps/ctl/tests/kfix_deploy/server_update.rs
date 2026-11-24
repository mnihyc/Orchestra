//! A web-owned update uses the release's real transaction and records its rollback outcome.
use super::*;

#[test]
fn a_failed_server_health_check_uses_the_existing_rollback_and_keeps_its_reason_for_the_returning_page()
 {
    let root = tempfile::tempdir().unwrap();
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/update.sh"
    ))
    .unwrap();
    std::fs::write(
        root.path().join("functions.sh"),
        source.rsplit_once("main ").unwrap().0,
    )
    .unwrap();
    let script = root.path().join("case.sh");
    std::fs::write(
        &script,
        r#"#!/bin/bash
set -euo pipefail
source ./functions.sh
ORCH_UPDATE_JOB=71c0cf93-f81d-42d3-9288-96f4569e0e97
mkdir -p runtime/update runtime/node/server-updates/$ORCH_UPDATE_JOB
J[from_tag]=v1.2.2; J[to_tag]=v1.2.3; J[kind]=update
app_stop() { printf 'stop\n' >> steps; }
take_dump() { printf 'dump\n' >> steps; }
activate_payloads() { printf 'activate\n' >> steps; }
app_start() { printf 'start\n' >> steps; }
wait_ready() { printf 'health %s\n' "$1" >> steps; return 1; }
undo_with_database() { printf 'rollback\n' >> steps; }
J[leaving_dump]=runtime/update-backups/before.dump
transact
"#,
    )
    .unwrap();
    let result = Command::new("bash")
        .arg(&script)
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(
        std::fs::read_to_string(root.path().join("steps")).unwrap(),
        "stop\ndump\nactivate\nstart\nhealth v1.2.3\nrollback\n"
    );
    let status = std::fs::read_to_string(
        root.path()
            .join("runtime/node/server-updates/71c0cf93-f81d-42d3-9288-96f4569e0e97/status"),
    )
    .unwrap();
    assert!(status.contains("stage=rolled_back\n"), "{status}");
    assert!(status.contains("reason=health_check_failed\n"), "{status}");
    assert!(status.contains("from=v1.2.2\n"), "{status}");
    assert!(status.contains("to=v1.2.3\n"), "{status}");
}

#[test]
fn an_unfinished_server_update_keeps_its_job_identity_in_the_same_power_cut_journal() {
    let root = tempfile::tempdir().unwrap();
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/update.sh"
    ))
    .unwrap();
    std::fs::write(
        root.path().join("functions.sh"),
        source.rsplit_once("main ").unwrap().0,
    )
    .unwrap();
    std::fs::write(
        root.path().join("case.sh"),
        r#"#!/bin/bash
set -euo pipefail
source ./functions.sh
ORCH_UPDATE_JOB=71c0cf93-f81d-42d3-9288-96f4569e0e97
mkdir -p runtime/update runtime/node/server-updates/$ORCH_UPDATE_JOB
link_get() { printf 'release\n'; }
begin_transaction update v1.2.2 v1.2.3
J=(); journal_load
test "${J[job]}" = "$ORCH_UPDATE_JOB"
"#,
    )
    .unwrap();
    let result = Command::new("bash")
        .arg(root.path().join("case.sh"))
        .current_dir(root.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
