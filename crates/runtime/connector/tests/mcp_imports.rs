//! The tool servers of the harnesses' own configuration: found and reported by name, one definition copied into the connector's own directory when it is imported, run from that copy, and forgotten.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[cfg(target_os = "linux")]
mod rig_environment;

#[cfg(target_os = "linux")]
use orch_link_contract::commands::{
    AgentStart, McpForget, McpImport, McpStart, McpStartLocal, McpStop,
};
#[cfg(target_os = "linux")]
use orch_link_contract::session::Role;
#[cfg(target_os = "linux")]
use orch_link_contract::tunnels::{Service, TicketSource};
#[cfg(target_os = "linux")]
use orch_types::{Json, new_id};
#[cfg(target_os = "linux")]
use rig_environment::tunnels::{messages, post};
#[cfg(target_os = "linux")]
use rig_environment::{EnvRig, stand_in};
#[cfg(target_os = "linux")]
use serde_json::json;

/// The program of the tests when it is run as a tool server: no test at all otherwise.
#[test]
#[cfg(target_os = "linux")]
fn stand_in_for_a_tool_server() {
    if std::env::var(stand_in::FLAG).is_ok() {
        stand_in::run();
    }
}

#[cfg(target_os = "linux")]
const SECRET: &str = "ghp_the_owners_own_token";

/// A rig whose machine plays the agent and the environment roles, and whose scripted harness has a tool
/// server configured, with a secret in it.
#[cfg(target_os = "linux")]
async fn rig_with_a_configured_server() -> (EnvRig, std::path::PathBuf) {
    let mut rig = EnvRig::with_roles(vec![Role::Agent, Role::Environment]);
    rig.with_the_scripted_harness();
    rig.parts.ports = Some(41000..=41999);
    let workspace = rig.directory("work");
    rig.start().await;
    let configured = json!([{
        "name": "github", "command": stand_in::command(),
        "env": { stand_in::FLAG: "1" }, "secrets": { "STAND_IN_SECRET": SECRET }
    }]);
    let start: AgentStart = serde_json::from_value(json!({
        "provider_id": rig.provider, "agent_id": rig.agent, "harness_kind": "fake",
        "profile": { "harness_version": null, "config": { "mcp_servers": configured }, "env": {} },
        "home": null
    }))
    .unwrap();
    rig.succeed(&start).await;
    (rig, workspace)
}

#[cfg(target_os = "linux")]
fn import_of(rig: &EnvRig, server_id: orch_types::Id, name: &str) -> McpImport {
    McpImport {
        expected_fingerprint: None,
        stored: None,
        transfer: None,
        environment_id: Some(rig.environment),
        server_id,
        source: "fake".to_owned(),
        scope: "user".to_owned(),
        name: name.to_owned(),
        directory: None,
    }
}

/// Every file under a directory whose content holds `needle`.
#[cfg(target_os = "linux")]
fn files_holding(dir: &std::path::Path, needle: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if std::fs::read(&path).is_ok_and(|bytes| {
                bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_bytes())
            }) {
                found.push(path);
            }
        }
    }
    found
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_server_the_harness_configured_is_found_by_name_imported_run_from_its_copy_and_forgotten()
{
    let (mut rig, workspace) = rig_with_a_configured_server().await;
    let facts = rig.attach(&workspace).await;
    let before: std::collections::BTreeSet<_> =
        files_holding(rig.data.path(), SECRET).into_iter().collect();

    // Found: by name, harness, place and transport; never how it is started, and no secret.
    let found = facts["mcp_found"].as_array().unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["name"], "github");
    assert_eq!(found[0]["source"], "fake");
    assert_eq!(found[0]["scope"], "user");
    assert_eq!(found[0]["transport"], "stdio");
    let shown = facts.to_string();
    assert!(
        !shown.contains(SECRET) && !shown.contains(stand_in::FLAG),
        "{shown}"
    );

    // Imported: the answer says what it is and nothing of the definition.
    let server_id = new_id();
    let imported = rig.succeed(&import_of(&rig, server_id, "github")).await;
    assert_eq!(
        found[0]["digest"], imported["digest"],
        "discovery and the stored copy use the same normalized private definition"
    );
    assert_eq!(imported["transport"], "stdio");
    assert_eq!(imported["tools"], Json::Null);
    assert!(!imported.to_string().contains(SECRET));
    // A copy is kept in the connector's own directory, readable by its user alone, and nowhere else.
    let copy = rig
        .data
        .path()
        .join("mcp")
        .join(format!("{server_id}.json"));
    assert!(copy.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // The import added that file and no other that holds the secret (the others are the harness's own
    // profile, which the scripted harness is configured through).
    let after_import: std::collections::BTreeSet<_> =
        files_holding(rig.data.path(), SECRET).into_iter().collect();
    assert_eq!(
        after_import.difference(&before).collect::<Vec<_>>(),
        [&copy],
        "the import left the secret in more places than its copy"
    );

    // Run from that copy, it is reached through a tunnel and has the secret it was configured with.
    let started = rig
        .succeed(&McpStart::Local(McpStartLocal {
            environment_id: rig.environment,
            name: "github".to_owned(),
            server_id,
            local: orch_link_contract::commands::True,
        }))
        .await;
    assert_eq!(started["service"], "mcp/github");
    let opener = rig.opener().await;
    let ticket = rig.ticket(
        TicketSource::Machine(opener.machine_id),
        Service::Mcp {
            server: "github".to_owned(),
        },
        Some(new_id()),
        None,
    );
    let tunnel = opener.open(&ticket).await.unwrap();
    let (_, kind, body) = post(
        tunnel,
        &[],
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "secret" } }),
    )
    .await;
    assert_eq!(
        messages(&kind, &body)[0]["result"]["content"][0]["text"],
        SECRET
    );
    rig.succeed(&McpStop {
        environment_id: rig.environment,
        name: "github".to_owned(),
    })
    .await;

    // Forgotten, the copy is gone, and what was imported cannot be started again.
    rig.succeed(&McpForget { server_id }).await;
    assert!(!copy.exists());
    let after_forget: std::collections::BTreeSet<_> =
        files_holding(rig.data.path(), SECRET).into_iter().collect();
    assert_eq!(
        after_forget, before,
        "forgetting took the copy and nothing else"
    );
    let again = rig
        .fail(&McpStart::Local(McpStartLocal {
            environment_id: rig.environment,
            name: "github".to_owned(),
            server_id,
            local: orch_link_contract::commands::True,
        }))
        .await;
    assert_eq!(again, "not_found");
    // Forgetting what is not there is no failure: the command may be sent again.
    rig.succeed(&McpForget { server_id }).await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn nothing_is_imported_that_the_harness_does_not_have_and_nothing_is_found_where_the_owner_allows_none()
 {
    let (mut rig, workspace) = rig_with_a_configured_server().await;
    rig.attach(&workspace).await;
    let missing = rig
        .failure(&import_of(&rig, new_id(), "not-configured"))
        .await;
    assert_eq!(missing.0, "not_found", "{missing:?}");
    rig.stop().await;

    let mut closed = EnvRig::with_roles(vec![Role::Agent, Role::Environment]);
    closed.with_the_scripted_harness();
    closed.config.mcp.import = false;
    let workspace = closed.directory("work");
    closed.start().await;
    let configured = json!([{ "name": "github", "command": stand_in::command() }]);
    let start: AgentStart = serde_json::from_value(json!({
        "provider_id": closed.provider, "agent_id": closed.agent, "harness_kind": "fake",
        "profile": { "harness_version": null, "config": { "mcp_servers": configured }, "env": {} },
        "home": null
    }))
    .unwrap();
    closed.succeed(&start).await;
    let facts = closed.attach(&workspace).await;
    assert_eq!(
        facts["mcp_found"],
        json!([]),
        "none is reported where none may be imported"
    );
    assert_eq!(
        closed.fail(&import_of(&closed, new_id(), "github")).await,
        "policy.mcp_import_disabled"
    );
    closed.stop().await;
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn a_stored_native_duplicate_copies_the_saved_file_and_scrubs_a_nonowners_declared_values() {
    use orch_link_contract::commands::{McpImportResult, McpStoredCopy, SerialKey};
    let (mut rig, workspace) = rig_with_a_configured_server().await;
    rig.attach(&workspace).await;
    let original = new_id();
    let reply: McpImportResult =
        serde_json::from_value(rig.succeed(&import_of(&rig, original, "github")).await).unwrap();
    let source = rig.data.path().join("mcp").join(format!("{original}.json"));
    let before = std::fs::read(&source).unwrap();
    assert!(String::from_utf8_lossy(&before).contains(SECRET));
    for include_secrets in [true, false] {
        let target = new_id();
        let body = McpImport {
            expected_fingerprint: None,
            environment_id: None,
            transfer: None,
            stored: Some(McpStoredCopy {
                server_id: original,
                expected_digest: reply.digest.clone(),
                include_secrets,
            }),
            server_id: target,
            source: "fake".into(),
            scope: "user".into(),
            name: "github".into(),
            directory: None,
        };
        let command = new_id();
        rig.blobs.open_command(command);
        rig.controller
            .issue_with_id(command, &SerialKey::Machine, rig.deadline(), &body);
        let result = tokio::time::timeout(
            rig_environment::PATIENCE,
            rig.controller.wait_command(command),
        )
        .await
        .unwrap();
        let orch_link::commands::Progress::Succeeded(result) = result else {
            panic!("the actual stored snapshot copy failed: {result:?}");
        };
        let result: McpImportResult = serde_json::from_value(result).unwrap();
        let actual =
            std::fs::read(rig.data.path().join("mcp").join(format!("{target}.json"))).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&actual).contains(SECRET),
            include_secrets
        );
        assert_eq!(result.digest == reply.digest, include_secrets);
        assert_eq!(std::fs::read(&source).unwrap(), before);
        let definition: orch_agent_contract::discovery::McpDefinition =
            serde_json::from_slice(&actual).unwrap();
        definition.check().unwrap();
        if !include_secrets {
            assert!(
                definition
                    .secrets
                    .values()
                    .all(|value| value.expose().is_empty())
            );
        }
    }
    rig.stop().await;
}

#[cfg(target_os = "linux")]
async fn machine_snapshot(rig: &EnvRig, body: &McpImport) -> orch_link::commands::Progress {
    let id = new_id();
    rig.blobs.open_command(id);
    rig.controller.issue_with_id(
        id,
        &orch_link_contract::commands::SerialKey::Machine,
        rig.deadline(),
        body,
    );
    tokio::time::timeout(rig_environment::PATIENCE, rig.controller.wait_command(id))
        .await
        .unwrap()
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn a_saved_mcp_snapshot_crosses_only_to_its_sealed_recipient_and_keeps_one_new_native_file() {
    use orch_link_contract::commands::{
        MachineKey, McpImportResult, McpSnapshotTransfer, McpStoredCopy, SerialKey,
    };
    use std::os::unix::fs::MetadataExt as _;
    let (mut source, workspace) = rig_with_a_configured_server().await;
    source.attach(&workspace).await;
    let mut target = EnvRig::new();
    target.start().await;
    let mut other = EnvRig::new();
    other.start().await;
    let original = new_id();
    let original_reply: McpImportResult = serde_json::from_value(
        source
            .succeed(&import_of(&source, original, "github"))
            .await,
    )
    .unwrap();
    let original_path = source
        .data
        .path()
        .join("mcp")
        .join(format!("{original}.json"));
    let original_bytes = std::fs::read(&original_path).unwrap();
    for include_secrets in [true, false] {
        let created = new_id();
        let read = McpImport {
            expected_fingerprint: None,
            environment_id: None,
            stored: Some(McpStoredCopy {
                server_id: original,
                expected_digest: original_reply.digest.clone(),
                include_secrets,
            }),
            transfer: Some(McpSnapshotTransfer::Issue {
                recipient: MachineKey {
                    machine_id: target.machine_id,
                    public_key: target.recipient.as_ref().unwrap().to_text(),
                },
            }),
            server_id: created,
            source: "fake".into(),
            scope: "user".into(),
            name: "github".into(),
            directory: None,
        };
        let id = new_id();
        source.blobs.open_command(id);
        source
            .controller
            .issue_with_id(id, &SerialKey::Machine, source.deadline(), &read);
        let result = tokio::time::timeout(
            rig_environment::PATIENCE,
            source.controller.wait_command(id),
        )
        .await
        .unwrap();
        let orch_link::commands::Progress::Succeeded(value) = result else {
            panic!("the native read failed: {result:?}");
        };
        assert!(!value.to_string().contains(SECRET));
        let issued: McpImportResult = serde_json::from_value(value).unwrap();
        assert!(
            !source
                .data
                .path()
                .join("mcp")
                .join(format!("{created}.json"))
                .exists(),
            "a sealed read is not a new source-machine file"
        );
        let receive = McpImport {
            transfer: Some(McpSnapshotTransfer::Receive {
                recipient: target.machine_id,
                snapshot: issued.snapshot.clone().unwrap(),
            }),
            ..read.clone()
        };
        assert!(
            matches!(
                machine_snapshot(&other, &receive).await,
                orch_link::commands::Progress::Failed(_)
            ),
            "a different native key cannot open the snapshot"
        );
        let orch_link::commands::Progress::Succeeded(value) =
            machine_snapshot(&target, &receive).await
        else {
            panic!("the actual receiver could not open its own sealed snapshot");
        };
        let result: McpImportResult = serde_json::from_value(value).unwrap();
        assert!(result.snapshot.is_none());
        assert_eq!(result.digest, issued.digest);
        let path = target
            .data
            .path()
            .join("mcp")
            .join(format!("{created}.json"));
        let before = std::fs::read(&path).unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();
        assert_eq!(
            String::from_utf8_lossy(&before).contains(SECRET),
            include_secrets
        );
        assert!(matches!(
            machine_snapshot(&target, &receive).await,
            orch_link::commands::Progress::Succeeded(_)
        ));
        assert_eq!(
            std::fs::metadata(&path).unwrap().ino(),
            inode,
            "a replay never replaces its own accepted file"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let mut wrong = receive.clone();
        wrong.server_id = new_id();
        assert!(matches!(
            machine_snapshot(&target, &wrong).await,
            orch_link::commands::Progress::Failed(_)
        ));
        let mut wrong = receive.clone();
        wrong.stored.as_mut().unwrap().include_secrets = !include_secrets;
        assert!(matches!(
            machine_snapshot(&target, &wrong).await,
            orch_link::commands::Progress::Failed(_)
        ));
        assert_eq!(std::fs::read(&original_path).unwrap(), original_bytes);
    }
    source.stop().await;
    target.stop().await;
    other.stop().await;
}
