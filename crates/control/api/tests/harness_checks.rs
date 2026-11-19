//! The check of a build of an agent program: a system admin asks for it, a host of the build's system checks it in
//! the background, and what it found stays with the build: a capability switched off, or why it could not run.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
#[path = "harness_checks/channels.rs"]
mod channel_cases;
mod places;
mod support;

use std::time::Duration;

use admin_support::cast;
use orch_api_contract::admin::{
    AdminHarnessCheck, AdminHarnessList, HarnessArch, HarnessBuild, HarnessBuildSource,
    HarnessBuildStatus,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::MachineOs;
use orch_types::Id;
use places::Who;
use serde_json::json;
use support::World;

const VERSION: &str = "0.170.0";

fn channel_build() -> HarnessBuild {
    HarnessBuild {
        kind: "codex".into(),
        version: VERSION.into(),
        os: MachineOs::Linux,
        arch: HarnessArch::X86_64,
        url: format!("https://example.com/codex-{VERSION}.tar.gz"),
        sha256: "c".repeat(64),
        is_default: false,
        source: HarnessBuildSource::Channel,
        status: HarnessBuildStatus::Untested,
        check: None,
        carry: None,
    }
}

async fn keep(world: &World, build: &HarnessBuild) {
    let mut conn = world.conn().await;
    orch_api_core::harness_channels::keep(&mut conn, std::slice::from_ref(build), world.now())
        .await
        .unwrap();
}

async fn listed(world: &World, by: &Who) -> HarnessBuild {
    world
        .app
        .call(AdminHarnessList, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
        .into_iter()
        .find(|build| build.kind == "codex" && build.version == VERSION)
        .unwrap()
}

/// The build as it stands once its check has ended.
async fn checked(world: &World, by: &Who) -> HarnessBuild {
    for _ in 0..400 {
        let build = listed(world, by).await;
        if build.status != HarnessBuildStatus::Checking {
            return build;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the check did not end");
}

/// A host of the system on Linux x86_64, online.
async fn system_host(world: &World) -> Id {
    let mut conn = world.conn().await;
    let host = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "host",
        None,
        None,
        "check-host",
    )
    .await
    .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id = $1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET online = true WHERE id = $1")
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    machine
}

/// Answers the machine's tool commands as a node does: the build is in its cache, and lacks `missing`.
fn node_answering(
    world: &World,
    machine: Id,
    missing: &'static [&'static str],
) -> tokio::task::JoinHandle<Vec<String>> {
    node_with_case(world, machine, missing, None)
}

fn node_with_case(
    world: &World,
    machine: Id,
    missing: &'static [&'static str],
    failed_case: Option<&'static str>,
) -> tokio::task::JoinHandle<Vec<String>> {
    let pool = world.db.pool.clone();
    tokio::spawn(async move {
        let mut answered = Vec::new();
        for _ in 0..800 {
            let waiting: Vec<(Id, String)> = sqlx::query_as(
                "SELECT id, kind FROM commands WHERE machine_id = $1 AND state IN ('queued', 'sent', 'accepted')",
            )
            .bind(machine)
            .fetch_all(&pool)
            .await
            .unwrap();
            for (id, kind) in waiting {
                if failed_case == Some("verify_previous_off")
                    && matches!(kind.as_str(), "node.tool.check" | "node.tool.conformance")
                {
                    let off: serde_json::Value = sqlx::query_scalar("SELECT value->'check'->'off' FROM settings WHERE node_id IS NULL AND value->>'version' = $1 AND value->>'kind' = 'codex'")
                        .bind(VERSION).fetch_one(&pool).await.unwrap();
                    assert_eq!(
                        off,
                        json!(["approvals.user_input"]),
                        "a running recheck keeps known failed features off"
                    );
                }
                let result = match kind.as_str() {
                    "node.tool.ensure" => json!({ "path": format!("/tools/codex/{VERSION}") }),
                    "node.tool.check" => json!({ "missing": missing }),
                    "node.tool.conformance" => {
                        let stage: String = sqlx::query_scalar("SELECT value->'check'->>'stage' FROM settings WHERE node_id IS NULL AND value->>'version' = $1 AND value->>'kind' = 'codex'")
                            .bind(VERSION).fetch_one(&pool).await.unwrap();
                        assert_eq!(
                            stage, "conformance",
                            "the row names its second stage before it is run"
                        );
                        if failed_case == Some("omit_cases") {
                            json!({"cases":[]})
                        } else {
                            json!({ "cases": orch_agent_programs::Programs::builtin().program("codex").unwrap().checks.cases().map(|(name, _)| json!({
                            "name": name, "passed": failed_case != Some(name), "feature": "steer",
                            "error": if failed_case == Some(name) { Some(if name == "native_import" { "native import unsupported" } else { "the native question was not observed" }) } else { None },
                            "unsupported": if (failed_case == Some("unavailable_import") && name == "history.native_import")
                                || (failed_case == Some("unavailable_question") && name == "user_input") { Some("the native feature is unavailable") } else { None }, "elapsed_ms": 12
                        })).collect::<Vec<_>>() })
                        }
                    }
                    _ => continue,
                };
                sqlx::query("UPDATE commands SET state = 'succeeded', result = $2 WHERE id = $1")
                    .bind(id)
                    .bind(&result)
                    .execute(&pool)
                    .await
                    .unwrap();
                answered.push(kind);
            }
            let expected = if missing.contains(&"turn/start") {
                2
            } else {
                3
            };
            if answered.len() == expected {
                return answered;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        answered
    })
}

#[tokio::test]
async fn a_kept_build_is_checked_on_a_host_of_its_system_and_keeps_what_the_check_found() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::ERROR)
        .try_init();
    let world = World::new().await;
    let people = cast(&world).await;
    keep(&world, &channel_build()).await;
    assert_eq!(
        listed(&world, &people.root).await.status,
        HarnessBuildStatus::Untested
    );

    // With no host to run it on, the build stays untested and says why.
    let asked = world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    let marked = asked.iter().find(|build| build.version == VERSION).unwrap();
    assert_eq!(
        marked.status,
        HarnessBuildStatus::Checking,
        "the list says which build is being checked"
    );
    let waiting = checked(&world, &people.root).await;
    assert_eq!(waiting.status, HarnessBuildStatus::Untested);
    assert!(
        waiting
            .check
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("no host"),
        "{waiting:?}"
    );

    // A host of the system checks it: the build is fetched, then run, and only plan mode is switched off.
    let machine = system_host(&world).await;
    let node = node_answering(&world, machine, &["TurnStartParams.collaborationMode"]);
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert_eq!(
        node.await.unwrap(),
        [
            "node.tool.ensure",
            "node.tool.check",
            "node.tool.conformance"
        ]
    );
    let found = checked(&world, &people.root).await;
    assert_eq!(found.status, HarnessBuildStatus::Checked);
    let check = found.check.unwrap();
    assert_eq!(check.missing, ["TurnStartParams.collaborationMode"]);
    assert_eq!(check.off, ["plan_mode"]);
    assert_eq!(check.machine.as_deref(), Some("check-host"));
    assert_eq!(check.error, None);
    assert_eq!(
        check.stage,
        orch_api_contract::admin::HarnessCheckStage::Complete
    );
    assert!(check.cases.iter().all(|case| case.passed));
    let programs = orch_agent_programs::Programs::builtin();
    let expected_cases = &programs.program("codex").unwrap().checks.conformance;
    assert_eq!(check.cases.len(), expected_cases.len());
    assert!(
        check
            .cases
            .iter()
            .any(|case| case.name == "history.native_import")
    );
    let asked: serde_json::Value = sqlx::query_scalar(
        "SELECT body FROM commands WHERE machine_id = $1 AND kind = 'node.tool.check'",
    )
    .bind(machine)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    // The row keeps the body as sent, inside its envelope or not.
    let body = if asked.get("names").is_some() {
        &asked
    } else {
        &asked["body"]
    };
    let names = body["names"].as_array().cloned().unwrap_or_default();
    assert!(names.contains(&json!("turn/start")), "{asked}");
    assert!(
        names.contains(&json!("TurnStartParams.collaborationMode")),
        "{asked}"
    );

    // A build that lacks what the adapter needs as a whole fails.
    let node = node_answering(&world, machine, &["turn/start"]);
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    assert_eq!(
        checked(&world, &people.root).await.status,
        HarnessBuildStatus::Failed
    );
}

#[tokio::test]
async fn only_a_system_admin_checks_and_only_a_kept_build() {
    let world = World::new().await;
    let people = cast(&world).await;
    keep(&world, &channel_build()).await;
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.boss.token)
        .send()
        .await
        .problem();
    assert_eq!(
        listed(&world, &people.root).await.status,
        HarnessBuildStatus::Untested
    );
    let unknown = world
        .app
        .call(AdminHarnessCheck, &[&"codex", &"9.9.9"])
        .token(&people.root.token)
        .send()
        .await
        .problem();
    assert_eq!(unknown.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn an_empty_native_report_cannot_mark_a_build_checked() {
    let world = World::new().await;
    let people = cast(&world).await;
    keep(&world, &channel_build()).await;
    let machine = system_host(&world).await;
    let node = node_with_case(&world, machine, &[], Some("omit_cases"));
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    let found = checked(&world, &people.root).await;
    assert_eq!(found.status, HarnessBuildStatus::Failed);
    let cases = found.check.unwrap().cases;
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].name, "runner");
    assert!(!cases[0].passed);
    assert!(cases[0].error.as_ref().unwrap().contains("omitted"));
}

#[tokio::test]
async fn native_forms_and_child_failures_disable_exactly_their_declared_feature() {
    for (case, feature) in [
        ("elicitation", "approvals.mcp_elicitation"),
        ("nested_subagent", "subagents"),
    ] {
        let world = World::new().await;
        let people = cast(&world).await;
        keep(&world, &channel_build()).await;
        let machine = system_host(&world).await;
        let node = node_with_case(&world, machine, &[], Some(case));
        world
            .app
            .call(AdminHarnessCheck, &[&"codex", &VERSION])
            .token(&people.root.token)
            .send()
            .await
            .ok();
        node.await.unwrap();
        let found = checked(&world, &people.root).await;
        assert_eq!(found.status, HarnessBuildStatus::Failed);
        let check = found.check.unwrap();
        assert_eq!(check.off, [feature]);
        assert_eq!(
            check
                .cases
                .iter()
                .filter(|result| !result.passed)
                .map(|result| result.name.as_str())
                .collect::<Vec<_>>(),
            [case]
        );
        assert!(
            !check
                .cases
                .iter()
                .any(|result| result.name == "dynamic_tools")
        );
        let before = json!({"kind":"codex", "harness":{"version":VERSION}, "capabilities":{
            "subagents":true, "steer":true, "orch_tools":"mcp", "plan_mode":true,
            "approvals":["command","file_change","permissions","user_input","mcp_elicitation"]
        }});
        let mut expected = before.clone();
        if case == "elicitation" {
            expected["capabilities"]["approvals"] =
                json!(["command", "file_change", "permissions", "user_input"]);
        } else {
            expected["capabilities"]["subagents"] = json!(false);
        }
        let mut conn = world.conn().await;
        assert_eq!(
            orch_engine::hosting::with_build_limits(&mut conn, before.clone())
                .await
                .unwrap(),
            expected
        );
        let mut another = before;
        another["harness"]["version"] = json!("0.171.0");
        assert_eq!(
            orch_engine::hosting::with_build_limits(&mut conn, another.clone())
                .await
                .unwrap(),
            another
        );
    }
}

#[tokio::test]
async fn an_unsupported_native_import_disables_only_that_builds_import_and_keeps_neutral_carry() {
    let world = World::new().await;
    let people = cast(&world).await;
    keep(&world, &channel_build()).await;
    let machine = system_host(&world).await;
    let node = node_with_case(&world, machine, &[], Some("native_import"));
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    let build = checked(&world, &people.root).await;
    assert_eq!(build.status, HarnessBuildStatus::Failed);
    let check = build.check.unwrap();
    assert_eq!(check.off, ["import_native"]);
    assert!(
        check
            .cases
            .iter()
            .any(|case| case.name == "native_import" && !case.passed)
    );
    assert!(
        check
            .cases
            .iter()
            .filter(|case| case.name != "native_import")
            .all(|case| case.passed)
    );
    let mut conn = world.conn().await;
    let limited = orch_engine::hosting::with_build_limits(
        &mut conn,
        json!({
            "kind":"codex", "harness":{"version":VERSION},
            "capabilities":{"import_native":true,"import_neutral":true,"steer":true}
        }),
    )
    .await
    .unwrap();
    assert_eq!(limited["capabilities"]["import_native"], false);
    assert_eq!(limited["capabilities"]["import_neutral"], true);
    assert_eq!(limited["capabilities"]["steer"], true);
}

#[tokio::test]
async fn a_native_failure_is_kept_with_its_case_and_switches_off_only_its_own_feature() {
    let world = World::new().await;
    let people = cast(&world).await;
    keep(&world, &channel_build()).await;
    let machine = system_host(&world).await;
    let node = node_with_case(&world, machine, &[], Some("user_input"));
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert_eq!(
        node.await.unwrap(),
        [
            "node.tool.ensure",
            "node.tool.check",
            "node.tool.conformance"
        ]
    );
    let found = checked(&world, &people.root).await;
    assert_eq!(found.status, HarnessBuildStatus::Failed);
    let check = found.check.unwrap();
    assert_eq!(
        check.off,
        ["approvals.user_input"],
        "the server owns feature attribution, regardless of the node's supplied feature"
    );
    let failed: Vec<_> = check.cases.iter().filter(|case| !case.passed).collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].name, "user_input");
    assert_eq!(
        failed[0].error.as_deref(),
        Some("the native question was not observed")
    );
    for (kind, version, expected) in [
        ("codex", VERSION, json!(["command"])),
        ("codex", "0.171.0", json!(["command", "user_input"])),
        ("claude-code", VERSION, json!(["command", "user_input"])),
    ] {
        let mut conn = world.conn().await;
        let descriptor = orch_engine::hosting::with_build_limits(&mut conn, json!({
            "kind":kind,"harness":{"version":version},"capabilities":{"steer":true,"approvals":["command","user_input"]}
        })).await.unwrap();
        assert_eq!(descriptor["capabilities"]["approvals"], expected);
        assert_eq!(descriptor["capabilities"]["steer"], true);
    }
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM commands WHERE machine_id = $1")
        .bind(machine)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    orch_api_core::harness_checks::check_waiting(&world.app.app).await;
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM commands WHERE machine_id = $1")
        .bind(machine)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        before, after,
        "the background schedule checks each build only once"
    );
}

#[tokio::test]
async fn a_build_claim_prevents_an_overlapping_check_during_either_stage() {
    let world = World::new().await;
    let build = channel_build();
    keep(&world, &build).await;
    let key = orch_api_core::harness_channels::channel_key(&build);
    assert!(
        orch_api_core::harness_checks::claim(&world.app.app, &key)
            .await
            .unwrap()
    );
    assert!(
        !orch_api_core::harness_checks::claim(&world.app.app, &key)
            .await
            .unwrap()
    );
    let mut conn = world.conn().await;
    let check = json!({"at":world.now(),"machine":"check-host","missing":[],"off":[],"error":null,"stage":"conformance","cases":[]});
    orch_db::queries::resources::management::set_harness_build_status(
        &mut conn,
        &key,
        "checking",
        Some(&check),
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    assert!(
        !orch_api_core::harness_checks::claim(&world.app.app, &key)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn both_command_deadlines_and_each_case_follow_the_system_settings() {
    let world = World::new().await;
    let people = cast(&world).await;
    keep(&world, &channel_build()).await;
    let machine = system_host(&world).await;
    let mut conn = world.conn().await;
    for (key, seconds) in [
        ("agents.harness_interface_timeout_secs", 13),
        ("agents.harness_case_timeout_secs", 7),
        ("agents.harness_conformance_timeout_secs", 33),
    ] {
        sqlx::query("INSERT INTO settings (node_id, key, value, locked, revision, updated_by, updated_at) VALUES (NULL, $1, $2, false, 1, NULL, $3)")
            .bind(key).bind(json!(seconds)).bind(world.now()).execute(&mut *conn).await.unwrap();
    }
    drop(conn);
    let node = node_answering(&world, machine, &[]);
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    assert_eq!(
        checked(&world, &people.root).await.status,
        HarnessBuildStatus::Checked
    );
    let requests: Vec<(String, serde_json::Value, chrono::DateTime<chrono::Utc>)> = sqlx::query_as("SELECT kind, body, deadline_at FROM commands WHERE machine_id = $1 AND kind IN ('node.tool.check', 'node.tool.conformance')")
        .bind(machine).fetch_all(&world.db.pool).await.unwrap();
    assert_eq!(requests.len(), 2);
    for (kind, mut body, deadline) in requests {
        if body.get("body").is_some() {
            body = body["body"].clone();
        }
        let seconds = if kind == "node.tool.check" { 13 } else { 33 };
        assert_eq!(deadline, world.now() + chrono::Duration::seconds(seconds));
        assert_eq!(body["timeout_secs"], seconds);
        if kind == "node.tool.conformance" {
            assert_eq!(body["case_timeout_secs"], 7);
        }
    }
}

#[tokio::test]
async fn a_recheck_keeps_a_known_failed_feature_off_until_its_own_cases_pass() {
    let world = World::new().await;
    let people = cast(&world).await;
    keep(&world, &channel_build()).await;
    let machine = system_host(&world).await;
    let node = node_with_case(&world, machine, &[], Some("user_input"));
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    assert_eq!(
        checked(&world, &people.root).await.check.unwrap().off,
        ["approvals.user_input"]
    );
    let node = node_with_case(&world, machine, &[], Some("verify_previous_off"));
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    let found = checked(&world, &people.root).await;
    assert_eq!(found.status, HarnessBuildStatus::Checked);
    assert!(found.check.unwrap().off.is_empty());
}

#[tokio::test]
async fn an_older_interface_only_certification_is_untested_until_its_native_cases_pass_in_the_background()
 {
    let world = World::new().await;
    let people = cast(&world).await;
    let build = channel_build();
    keep(&world, &build).await;
    let key = orch_api_core::harness_channels::channel_key(&build);
    let mut conn = world.conn().await;
    let previous =
        json!({"at":world.now(),"machine":"old-host","missing":[],"off":[],"error":null});
    orch_db::queries::resources::management::set_harness_build_status(
        &mut conn,
        &key,
        "checked",
        Some(&previous),
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(
        listed(&world, &people.root).await.status,
        HarnessBuildStatus::Untested
    );
    let machine = system_host(&world).await;
    let node = node_answering(&world, machine, &[]);
    orch_api_core::harness_checks::check_waiting(&world.app.app).await;
    assert_eq!(
        node.await.unwrap(),
        [
            "node.tool.ensure",
            "node.tool.check",
            "node.tool.conformance"
        ]
    );
    let found = listed(&world, &people.root).await;
    assert_eq!(found.status, HarnessBuildStatus::Checked);
    assert!(found.check.unwrap().cases.iter().all(|case| case.passed));
}

#[tokio::test]
async fn native_import_is_off_without_proof_and_returns_only_after_that_exact_build_passes() {
    let world = World::new().await;
    let people = cast(&world).await;
    keep(&world, &channel_build()).await;
    let machine = system_host(&world).await;
    let declared = json!({"kind":"codex", "harness":{"version":VERSION},
        "capabilities":{"import_native":true,"import_neutral":true,"steer":true}});
    let mut expected = declared.clone();
    expected["capabilities"]["import_native"] = json!(false);
    let mut conn = world.conn().await;
    assert_eq!(
        orch_engine::hosting::with_build_limits(&mut conn, declared.clone())
            .await
            .unwrap(),
        expected
    );
    drop(conn);

    let node = node_with_case(&world, machine, &[], Some("unavailable_import"));
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    let unavailable = checked(&world, &people.root).await;
    assert_eq!(unavailable.status, HarnessBuildStatus::Checked);
    let check = unavailable.check.unwrap();
    assert_eq!(check.off, ["import_native"]);
    assert!(
        check
            .cases
            .iter()
            .all(|case| case.passed && case.error.is_none())
    );
    assert!(
        check
            .cases
            .iter()
            .any(|case| case.name == "history.native_import" && case.unsupported.is_some())
    );
    let mut conn = world.conn().await;
    assert_eq!(
        orch_engine::hosting::with_build_limits(&mut conn, declared.clone())
            .await
            .unwrap(),
        expected
    );
    drop(conn);

    let node = node_with_case(&world, machine, &[], None);
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    let checked_build = checked(&world, &people.root).await;
    assert_eq!(checked_build.status, HarnessBuildStatus::Checked);
    assert!(
        checked_build
            .check
            .unwrap()
            .cases
            .iter()
            .any(|case| case.name == "history.native_import" && case.passed)
    );
    let mut conn = world.conn().await;
    assert_eq!(
        orch_engine::hosting::with_build_limits(&mut conn, expected.clone())
            .await
            .unwrap(),
        declared
    );
    let mut later = declared.clone();
    later["harness"]["version"] = json!("0.171.0");
    let mut unproved = later.clone();
    unproved["capabilities"]["import_native"] = json!(false);
    assert_eq!(
        orch_engine::hosting::with_build_limits(&mut conn, later)
            .await
            .unwrap(),
        unproved
    );
    drop(conn);

    let node = node_with_case(&world, machine, &[], Some("nested_subagent"));
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    assert_eq!(
        checked(&world, &people.root).await.check.unwrap().off,
        ["subagents"]
    );
    let mut conn = world.conn().await;
    assert_eq!(
        orch_engine::hosting::with_build_limits(&mut conn, expected.clone())
            .await
            .unwrap(),
        declared,
        "an unrelated native failure cannot remove the successful import proof"
    );
    drop(conn);

    let node = node_with_case(&world, machine, &[], Some("history.native_import"));
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    let failed = checked(&world, &people.root).await;
    assert_eq!(failed.status, HarnessBuildStatus::Failed);
    assert_eq!(failed.check.unwrap().off, ["import_native"]);
    let mut conn = world.conn().await;
    assert_eq!(
        orch_engine::hosting::with_build_limits(&mut conn, declared)
            .await
            .unwrap(),
        expected
    );
    drop(conn);
    let node = node_with_case(&world, machine, &[], None);
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    let restored = checked(&world, &people.root).await;
    assert_eq!(restored.status, HarnessBuildStatus::Checked);
    let check = restored.check.unwrap();
    assert!(check.off.is_empty());
    assert!(
        check
            .cases
            .iter()
            .any(|case| case.name == "history.native_import"
                && case.passed
                && case.error.is_none()
                && case.unsupported.is_none())
    );
    let mut conn = world.conn().await;
    assert_eq!(
        orch_engine::hosting::with_build_limits(&mut conn, expected)
            .await
            .unwrap()["capabilities"]["import_native"],
        true,
        "a successful new check restores only the capability it proved"
    );
}

#[tokio::test]
async fn an_unavailable_label_cannot_turn_a_missing_native_question_into_a_passing_check() {
    let world = World::new().await;
    let people = cast(&world).await;
    keep(&world, &channel_build()).await;
    let machine = system_host(&world).await;
    let node = node_with_case(&world, machine, &[], Some("unavailable_question"));
    world
        .app
        .call(AdminHarnessCheck, &[&"codex", &VERSION])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    node.await.unwrap();
    let found = checked(&world, &people.root).await;
    assert_eq!(found.status, HarnessBuildStatus::Failed);
    let check = found.check.unwrap();
    assert!(
        check
            .cases
            .iter()
            .any(|case| case.name == "runner" && !case.passed)
    );
    assert!(
        check
            .off
            .iter()
            .any(|feature| feature == "approvals.user_input")
    );
}

/// Answers the machine's tool commands for a program whose manifest lists no native case: the build is in its cache, and
/// lacks `missing`; a command of the second stage is an error, since none should be sent.
fn node_without_native_cases(
    world: &World,
    machine: Id,
    missing: &'static [&'static str],
) -> tokio::task::JoinHandle<Vec<String>> {
    let pool = world.db.pool.clone();
    tokio::spawn(async move {
        let mut answered = Vec::new();
        for _ in 0..800 {
            let waiting: Vec<(Id, String)> = sqlx::query_as(
                "SELECT id, kind FROM commands WHERE machine_id = $1 AND state IN ('queued', 'sent', 'accepted')",
            )
            .bind(machine)
            .fetch_all(&pool)
            .await
            .unwrap();
            for (id, kind) in waiting {
                let result = match kind.as_str() {
                    "node.tool.ensure" => json!({ "path": "/tools/fake-third/1.0.0" }),
                    "node.tool.check" => json!({ "missing": missing }),
                    _ => continue,
                };
                sqlx::query("UPDATE commands SET state = 'succeeded', result = $2 WHERE id = $1")
                    .bind(id)
                    .bind(&result)
                    .execute(&pool)
                    .await
                    .unwrap();
                answered.push(kind);
            }
            if answered.len() == 2 {
                return answered;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        answered
    })
}

#[tokio::test]
async fn a_program_that_is_nothing_but_a_manifest_has_its_builds_checked_by_its_interface_alone() {
    let world = World::new().await;
    let people = cast(&world).await;
    let machine = system_host(&world).await;
    // The connector of the machine reports the program: an adapter that is not part of Orch, and its manifest.
    sqlx::query("UPDATE machines SET facts = $2 WHERE id = $1")
        .bind(machine)
        .bind(json!({ "adapters": [ {
            "kind": orch_testkit::programs::THIRD_KIND, "adapter_version": 1, "builtin": false,
            "manifest": orch_testkit::programs::third_manifest_json()
        } ], "harnesses": [] }))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let build = HarnessBuild {
        kind: orch_testkit::programs::THIRD_KIND.into(),
        version: "1.0.0".into(),
        url: "https://example.com/fake-third-1.0.0.tar.gz".into(),
        ..channel_build()
    };
    keep(&world, &build).await;
    let node = node_without_native_cases(&world, machine, &["--steer"]);
    world
        .app
        .call(
            AdminHarnessCheck,
            &[&orch_testkit::programs::THIRD_KIND, &"1.0.0"],
        )
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert_eq!(
        node.await.unwrap(),
        ["node.tool.ensure", "node.tool.check"],
        "its manifest lists no native case, so no second stage is asked of the host"
    );
    let mut found = None;
    for _ in 0..400 {
        let list = world
            .app
            .call(AdminHarnessList, &[])
            .token(&people.root.token)
            .send()
            .await
            .ok();
        let here = list
            .into_iter()
            .find(|build| build.kind == orch_testkit::programs::THIRD_KIND)
            .unwrap();
        if here.status != HarnessBuildStatus::Checking {
            found = Some(here);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let found = found.expect("the check did not end");
    assert_eq!(found.status, HarnessBuildStatus::Checked);
    let check = found.check.unwrap();
    assert_eq!(
        check.off,
        ["steer"],
        "a name only one capability needs switches that capability off"
    );
    assert!(check.cases.is_empty());
    assert_eq!(
        check.stage,
        orch_api_contract::admin::HarnessCheckStage::Complete
    );
    assert_eq!(check.error, None);
}
