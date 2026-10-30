//! Provider directory attachment uses the common after-start phase, with the machine's actual facts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::World;
use orch_engine::provider_runtime::RuntimeStart;
use orch_link_contract::commands::{Command, CommandKind as K, CommandResult, UnitAfterUpResult};
use orch_types::new_id;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tokio::test]
async fn a_legacy_online_harness_reconciles_its_home_once_without_repeating_setup_on_an_unchanged_process()
 {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let world = World::new().await;
    let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
        "test",
        orch_driver_contract::descriptor::EnvironmentDataStorage {
            made_by_orch: None,
            existing_directory: true,
            reason: None,
        },
    );
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts || $2,'{policy}',COALESCE(facts->'policy','{}') || $3) WHERE id=$1")
        .bind(world.machine_id).bind(json!({"provider_runtime_descriptor":descriptor})).bind(json!({"environment.after_up":true}))
        .execute(world.pool.pool()).await.unwrap();
    sqlx::query("UPDATE providers SET runtime_config=$2 WHERE id=$1")
        .bind(world.provider)
        .bind(json!({"runtime.after_up":{"10-setup":{"command":["must-not-repeat-on-reuse"]}}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO harnesses(provider_id,agent_id,machine_id,status) VALUES($1,$2,$3,'online')",
    )
    .bind(world.provider)
    .bind(world.agent)
    .bind(world.machine_id)
    .execute(world.pool.pool())
    .await
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let starts = Arc::clone(&calls);
    world.machine.handler.on(K::AgentStart, move |command| {
        let Command::AgentStart(body) = command else {
            unreachable!()
        };
        assert!(
            body.runtime.as_ref().unwrap().home.is_none(),
            "an unconfigured runtime must not force a private home"
        );
        starts.fetch_add(1, Ordering::SeqCst);
        Ok(CommandResult::AgentStart(
            orch_link_contract::commands::AgentStartResult {
                reused: true,
                harness_version: "1.0.0".into(),
                descriptor: serde_json::from_value(common::descriptor()).unwrap(),
            },
        ))
    });
    let commands = Arc::new(AtomicUsize::new(0));
    let setups = Arc::clone(&commands);
    world.machine.handler.on(K::UnitAfterUp, move |_| {
        setups.fetch_add(1, Ordering::SeqCst);
        panic!("a reused ready process has no new after-start run")
    });
    assert!(matches!(
        world
            .engine()
            .start_provider_runtime(world.provider, Some(world.ada))
            .await
            .unwrap(),
        RuntimeStart::Up
    ));
    assert!(matches!(
        world
            .engine()
            .start_provider_runtime(world.provider, Some(world.ada))
            .await
            .unwrap(),
        RuntimeStart::Up
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(commands.load(Ordering::SeqCst), 0);
    let stored:Option<String>=sqlx::query_scalar("SELECT after_up_plan->>'configuration_hash' FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
        .bind(world.provider).bind(world.agent).bind(world.machine_id).fetch_one(world.pool.pool()).await.unwrap();
    assert!(
        stored.is_some(),
        "the existing journal records which home/configuration is ready without a schema addition"
    );
    let setups = Arc::clone(&commands);
    world.machine.handler.on(K::UnitAfterUp, move |command| {
        let Command::UnitAfterUp(body) = command else {
            unreachable!()
        };
        setups.fetch_add(1, Ordering::SeqCst);
        Ok(CommandResult::UnitAfterUp(UnitAfterUpResult {
            name: body.name.clone(),
            exit: Some(0),
            duration_ms: 1,
            timed_out: false,
            first_lines: String::new(),
        }))
    });
    for status in ["starting", "online"] {
        world
            .machine
            .publish(
                &orch_link_contract::streams::Stream::Harness {
                    provider_id: world.provider,
                    agent_id: world.agent,
                },
                &orch_link_contract::events::Event::HarnessStatus(
                    orch_link_contract::events::HarnessStatus {
                        status: status.into(),
                        detail: None,
                        harness_version: Some("1.0.0".into()),
                        sessions_open: 0,
                    },
                ),
            )
            .await;
    }
    common::eventually("a supervised restart to invalidate its completed setup",||async {
        sqlx::query_scalar::<_,bool>("SELECT status='starting' AND NOT(after_up_plan ? 'configuration_hash') FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
            .bind(world.provider).bind(world.agent).bind(world.machine_id).fetch_one(world.pool.pool()).await.unwrap()
    }).await;
    assert!(matches!(
        world
            .engine()
            .start_provider_runtime(world.provider, Some(world.ada))
            .await
            .unwrap(),
        RuntimeStart::Up
    ));
    assert_eq!(
        commands.load(Ordering::SeqCst),
        1,
        "a real supervised restart runs setup before becoming ready"
    );
}

#[tokio::test]
async fn directory_runtime_commands_use_scoped_values_and_the_shared_policy_support_and_failure_rules()
 {
    for case in ["allowed", "policy_off", "unsupported", "failed"] {
        let world = World::new().await;
        let template = new_id();
        sqlx::query("INSERT INTO env_templates(id,name,kind,driver,spec) VALUES($1,'Unit recipe','provider','test',$2)")
            .bind(template).bind(json!({"after_up":[{"name":"unit-only","command":["never-on-a-directory"]}]}))
            .execute(world.pool.pool()).await.unwrap();
        sqlx::query("UPDATE providers SET runtime_config=$2 WHERE id=$1").bind(world.provider).bind(json!({
            "runtime.template_id":template,"runtime.after_up":{"20-ready":{"command":["finish"]},"10-prepare":{"command":["prepare","a b"],"timeout_secs":7}}
        })).execute(world.pool.pool()).await.unwrap();
        let descriptor = orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
            "test",
            orch_driver_contract::descriptor::EnvironmentDataStorage {
                made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
                existing_directory: true,
                reason: None,
            },
        );
        let mut descriptor = serde_json::to_value(descriptor).unwrap();
        descriptor["commands_after_start"] = json!({"supported":case!="unsupported","reason":if case=="unsupported" {Some("process groups are unavailable")} else {None}});
        sqlx::query("UPDATE machines SET facts=jsonb_set(facts || $2,'{policy}',(facts->'policy') || $3) WHERE id=$1")
            .bind(world.machine_id).bind(json!({"provider_runtime_descriptor":descriptor}))
            .bind(json!({"environment.after_up":case!="policy_off"})).execute(world.pool.pool()).await.unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::clone(&seen);
        world.machine.handler.on(K::UnitAfterUp, move |command| {
            let Command::UnitAfterUp(body) = command else {
                unreachable!()
            };
            calls.lock().unwrap().push(body.clone());
            Ok(CommandResult::UnitAfterUp(UnitAfterUpResult {
                name: body.name.clone(),
                exit: Some(if case == "failed" { 7 } else { 0 }),
                duration_ms: 1,
                timed_out: false,
                first_lines: if case == "failed" {
                    "preparation failed".into()
                } else {
                    String::new()
                },
            }))
        });
        let result = if case == "allowed" {
            world.machine.handler.hold(K::UnitAfterUp);
            let engine = world.engine();
            let provider = world.provider;
            let owner = world.ada;
            let starting =
                tokio::spawn(
                    async move { engine.start_provider_runtime(provider, Some(owner)).await },
                );
            world.wait_began(K::UnitAfterUp, 1).await;
            world
                .machine
                .publish(
                    &orch_link_contract::streams::Stream::Harness {
                        provider_id: provider,
                        agent_id: world.agent,
                    },
                    &orch_link_contract::events::Event::HarnessStatus(
                        orch_link_contract::events::HarnessStatus {
                            status: "online".into(),
                            detail: None,
                            harness_version: Some("early-status".into()),
                            sessions_open: 0,
                        },
                    ),
                )
                .await;
            common::eventually("the early report to be applied without releasing readiness",||async {
                let row:Option<(String,Option<String>,Option<orch_types::Id>)>=sqlx::query_as("SELECT status,harness_version,start_command_id FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
                    .bind(provider).bind(world.agent).bind(world.machine_id).fetch_optional(world.pool.pool()).await.unwrap();
                row.is_some_and(|(status,version,pending)|status=="starting" && version.as_deref()==Some("early-status") && pending.is_some())
            }).await;
            assert!(
                !starting.is_finished(),
                "provider readiness waits for its after-start commands"
            );
            starting.abort();
            let _ = starting.await;
            world.restart_api().await;
            sqlx::query("UPDATE providers SET runtime_config=jsonb_set(runtime_config,'{runtime.after_up}',$2) WHERE id=$1")
                .bind(provider).bind(json!({"99-later":{"command":["must-not-replace-frozen-commands"]}})).execute(world.pool.pool()).await.unwrap();
            world.machine.handler.release();
            tokio::time::timeout(
                Duration::from_secs(5),
                world.engine().start_provider_runtime(provider, Some(owner)),
            )
            .await
            .unwrap()
        } else {
            tokio::time::timeout(
                Duration::from_secs(5),
                world
                    .engine()
                    .start_provider_runtime(world.provider, Some(world.ada)),
            )
            .await
            .unwrap()
        };
        let calls = seen.lock().unwrap().clone();
        if matches!(case, "allowed" | "policy_off") {
            assert!(matches!(result, Ok(RuntimeStart::Up)), "{case}: {result:?}");
        } else {
            assert!(result.is_err(), "{case}: {result:?}");
        }
        assert_eq!(
            calls.len(),
            match case {
                "allowed" => 2,
                "failed" => 1,
                _ => 0,
            },
            "{case}"
        );
        if !calls.is_empty() {
            assert_eq!(calls[0].name, "10-prepare");
            assert_eq!(calls[0].run.command, ["prepare", "a b"]);
            assert_eq!(calls[0].run.timeout_secs, 7);
            assert_eq!(calls[0].agent_id, Some(world.agent));
            assert_eq!(calls[0].provider_id, Some(world.provider));
            assert_eq!(calls[0].environment_id, None);
        }
        if case == "allowed" {
            assert_eq!(calls[1].name, "20-ready");
            assert_eq!(calls[0].run_id, calls[1].run_id);
        }
        let harness = orch_db::queries::resources::thread_engine::harness(
            &mut world.pool.acquire().await.unwrap(),
            world.provider,
            world.agent,
            world.machine_id,
        )
        .await
        .unwrap()
        .unwrap();
        if case == "policy_off" {
            assert!(
                harness
                    .status_detail
                    .as_deref()
                    .unwrap()
                    .contains("environment.after_up")
            );
        }
        if case == "failed" {
            assert!(
                harness
                    .status_detail
                    .as_deref()
                    .unwrap()
                    .contains("preparation failed")
            );
            assert_eq!(harness.status, "error");
        }
        if case == "allowed" {
            let history:Vec<serde_json::Value>=sqlx::query_scalar("SELECT details FROM audit_log WHERE object_id=$1 AND action='unit.command_ran' ORDER BY at,id")
                .bind(world.provider).fetch_all(world.pool.pool()).await.unwrap();
            assert_eq!(
                history.len(),
                2,
                "one output-free history line per frozen command, including recovery"
            );
            assert_eq!(history[0]["name"], "10-prepare");
            assert_eq!(history[1]["name"], "20-ready");
            assert!(
                history
                    .iter()
                    .all(|row| row.get("first_lines").is_none() && row.get("command").is_none())
            );
            world
                .engine()
                .start_provider_runtime(world.provider, Some(world.ada))
                .await
                .unwrap();
            assert_eq!(
                seen.lock().unwrap().len(),
                2,
                "an online attachment never repeats its commands"
            );
        }
    }
}
