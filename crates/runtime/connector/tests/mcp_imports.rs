//! The tool servers of the harnesses' own configuration: found and reported by name, one definition copied into the connector's own directory when it is imported, run from that copy, and forgotten.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod rig_environment;

use orch_link_contract::commands::{
    AgentStart, McpForget, McpImport, McpStart, McpStartLocal, McpStop,
};
use orch_link_contract::session::Role;
use orch_link_contract::tunnels::{Service, TicketSource};
use orch_types::{Json, new_id};
use rig_environment::tunnels::{messages, post};
use rig_environment::{EnvRig, stand_in};
use serde_json::json;

/// The program of the tests when it is run as a tool server: no test at all otherwise.
#[test]
fn stand_in_for_a_tool_server() {
    if std::env::var(stand_in::FLAG).is_ok() {
        stand_in::run();
    }
}

const SECRET: &str = "ghp_the_owners_own_token";

/// A rig whose machine plays the agent and the environment roles, and whose scripted harness has a tool
/// server configured, with a secret in it.
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

fn import_of(rig: &EnvRig, server_id: orch_types::Id, name: &str) -> McpImport {
    McpImport {
        expected_fingerprint: None,
        environment_id: rig.environment,
        server_id,
        source: "fake".to_owned(),
        scope: "user".to_owned(),
        name: name.to_owned(),
        directory: None,
    }
}

/// Every file under a directory whose content holds `needle`.
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
