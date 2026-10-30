//! A unit stays unavailable until its ordered commands finish, replays once, and keeps stopped files on failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod runtimes;

use common::{World, eventually};
use hosting::{ScriptedHost, serve, wait_over};
use orch_link_contract::commands::{Command, CommandKind as K, CommandResult, UnitAfterUpResult};
use orch_types::Json;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[test]
fn provider_jobs_run_on_default_stacks_even_if_the_suite_stack_is_larger() {
    common::on_default_stack(
        "a_provider_uses_its_named_state_storage_and_runs_commands_before_its_machine_is_published",
    );
}

#[tokio::test]
async fn commands_hold_ready_run_template_then_scoped_names_and_resume_the_frozen_list_once_after_api_restart()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "after-start", 16384).await;
    let template = hosting::template(&world).await;
    sqlx::query("UPDATE env_templates SET spec=spec || $2 WHERE id=$1").bind(template)
        .bind(json!({"after_up":[{"name":"10-template","command":["template-tool"]}],"init":{"command":["legacy-init"]}}))
        .execute(world.pool.pool()).await.unwrap();
    world.set(world.root,"env.after_up",json!({"00-removed":{"command":["must-not-run"]},"10-local":{"command":["inherited-a"]},"20-local":{"command":["inherited-b"]}})).await;
    let made=hosting::make_with(&world,template,"Wait for tools",Some(host.id),
        &json!({"env.after_up":{"00-removed":null,"20-local":{"command":["local-b"]},"10-local":{"command":["local-a"]}}})).await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let unit = hosting::enroll_holding(&world, host.started().unwrap(), K::EnvAttach).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let handler = Arc::downgrade(&unit.added.machine.handler);
    unit.added
        .machine
        .handler
        .on(K::UnitAfterUp, move |command| {
            let Command::UnitAfterUp(body) = command else {
                unreachable!()
            };
            log.lock().unwrap().push(body.clone());
            if body.position == 0 {
                handler.upgrade().unwrap().hold(K::UnitAfterUp);
            }
            Ok(CommandResult::UnitAfterUp(UnitAfterUpResult {
                name: body.name.clone(),
                exit: Some(0),
                duration_ms: 2,
                timed_out: false,
                first_lines: "not a history field".into(),
            }))
        });
    unit.let_go();
    eventually("the second command is held before ready", || async {
        unit.began(K::UnitAfterUp) >= 2
    })
    .await;
    assert_eq!(hosting::status(&world, made.id).await.0, "provisioning");
    assert_eq!(
        hosting::job(&world, made.job.id).await.1["phase"],
        "after_up"
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
    let thread = world.thread("Wait for prepared tools").await;
    hosting::bind(&world, thread, made.id).await;
    let pending = world
        .engine()
        .submit_input(
            &orch_authz::Caller::session(world.ada),
            thread,
            common::queued("use the prepared tool"),
        )
        .await
        .unwrap();
    assert_eq!(
        world
            .machine
            .handler
            .executions()
            .iter()
            .filter(|run| run.kind == K::TurnStart)
            .count(),
        0
    );
    // A later edit cannot change the argv of a run whose effects have already begun.
    sqlx::query("UPDATE environments SET config=config || $2 WHERE id=$1")
        .bind(made.id)
        .bind(json!({"env.after_up":{"99-later":{"command":["changed-after-start"]}}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.restart_api().await;
    serve(&world);
    unit.let_go();
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen.iter()
            .map(|body| body.name.as_str())
            .collect::<Vec<_>>(),
        ["10-template", "init", "10-local", "20-local"]
    );
    assert!(seen.iter().all(|body| body.run_id == seen[0].run_id));
    assert_eq!(seen[2].run.command, ["local-a"]);
    let lines:Vec<Json>=sqlx::query_scalar("SELECT details FROM audit_log WHERE object_id=$1 AND action='unit.command_ran' ORDER BY at,id")
        .bind(made.id).fetch_all(world.pool.pool()).await.unwrap();
    assert_eq!(
        lines.len(),
        4,
        "replayed outcomes do not duplicate a history line"
    );
    for line in lines {
        let names = line.as_object().unwrap();
        assert!(
            names.contains_key("name")
                && names.contains_key("exit")
                && names.contains_key("duration_ms")
                && names.contains_key("timed_out")
        );
        assert!(!line.to_string().contains("not a history field"));
        assert!(!names.contains_key("command") && !names.contains_key("first_lines"));
    }
    assert_eq!(hosting::status(&world, made.id).await.0, "running");
    let start = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(start.turn_id)).await;
    eventually(
        "queued work is delivered only after commands make the environment ready",
        || async { world.input_state(pending.input.id).await.0 == "delivered" },
    )
    .await;
    world.engine().stop();
}

#[tokio::test]
async fn an_empty_after_start_list_sends_no_command_and_the_default_connector_answers_a_named_command()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "default-unit-commands", 16384).await;
    let template = hosting::template(&world).await;
    for (name, config, expected) in [
        ("No setup", json!({}), 0),
        (
            "One setup",
            json!({"env.after_up":{"10-setup":{"command":["setup-tool"]}}}),
            1,
        ),
    ] {
        let made = hosting::make_with(&world, template, name, Some(host.id), &config).await;
        host.wait_for(K::NodeRuntimeStart, if expected == 0 { 1 } else { 2 })
            .await;
        let unit = hosting::enroll(&world, host.started().unwrap()).await;
        let (state, _, error) = wait_over(&world, made.job.id).await;
        assert_eq!(state, "succeeded", "{error:?}");
        assert_eq!(hosting::status(&world, made.id).await.0, "running");
        assert_eq!(unit.began(K::UnitAfterUp), expected);
        if expected != 0 {
            assert_eq!(unit.calls_of(K::UnitAfterUp)[0].body["name"], "10-setup");
        }
    }
    world.engine().stop();
}

#[tokio::test]
async fn a_failed_command_stops_the_same_unit_keeps_its_files_and_shows_the_name_exit_and_first_lines()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "failed-tools", 16384).await;
    let template = hosting::template(&world).await;
    let made=hosting::make_with(&world,template,"Tool failed",Some(host.id),
        &json!({"env.after_up":{"10-install":{"command":["install-tool"]},"20-must-not-run":{"command":["later"]}}})).await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    let unit = hosting::enroll_holding(&world, runtime, K::EnvAttach).await;
    unit.added.machine.handler.on(K::UnitAfterUp, |command| {
        let Command::UnitAfterUp(body) = command else {
            unreachable!()
        };
        assert_eq!(body.name, "10-install");
        Ok(CommandResult::UnitAfterUp(UnitAfterUpResult {
            name: body.name.clone(),
            exit: Some(7),
            duration_ms: 3,
            timed_out: false,
            first_lines: "safe diagnostic\nsecond line".into(),
        }))
    });
    unit.let_go();
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "failed");
    let said = error.unwrap().to_string();
    assert!(said.contains("10-install") && said.contains('7') && said.contains("safe diagnostic"));
    assert_eq!(unit.began(K::UnitAfterUp), 1);
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    let current = orch_db::queries::environments::hosting::runtime(
        &mut world.pool.acquire().await.unwrap(),
        runtime,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(current.state, "failed");
    assert!(!current.volumes.as_object().unwrap().is_empty());
    let lines: Vec<Json> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE object_id=$1 AND action='unit.command_ran'",
    )
    .bind(made.id)
    .fetch_all(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["exit"], 7);
    assert!(!lines[0].to_string().contains("safe diagnostic"));
    world.engine().stop();
}

#[tokio::test]
async fn a_provider_uses_its_named_state_storage_and_runs_commands_before_its_machine_is_published()
{
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "runtime-tools", 16384).await;
    let template = runtimes::provider_template(&world).await;
    let provider = runtimes::managed_subscription(&world, "Private tools").await;
    sqlx::query("UPDATE providers SET runtime_config=$2 WHERE id=$1").bind(provider)
        .bind(json!({"runtime.storage":{"state":"fast"},"runtime.needs":{"memory_mb":512},"runtime.after_up":{"10-helper":{"command":["/opt/helper","install"]}}}))
        .execute(world.pool.pool()).await.unwrap();
    sqlx::query("UPDATE env_templates SET spec=spec || $2 WHERE id=$1").bind(template)
        .bind(json!({"plugins":[{"kind":"translator","name":"team-format","command":["/opt/team-format"]}]}))
        .execute(world.pool.pool()).await.unwrap();
    let job = runtimes::queue(
        &world,
        provider,
        orch_engine::jobs::provider_provision::Mode::Make,
    )
    .await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    let unit = hosting::connector(&world, "provider-unit", &["provider", "agent"]).await;
    unit.hold(K::UnitAfterUp);
    unit.added.machine.handler.on(K::UnitAfterUp, |command| {
        let Command::UnitAfterUp(body) = command else {
            unreachable!()
        };
        assert!(body.environment_id.is_none());
        assert_eq!(body.name, "10-helper");
        Ok(CommandResult::UnitAfterUp(UnitAfterUpResult {
            name: body.name.clone(),
            exit: Some(0),
            duration_ms: 1,
            timed_out: false,
            first_lines: String::new(),
        }))
    });
    orch_db::queries::resources::hosting::adopt_machine(
        &mut world.pool.acquire().await.unwrap(),
        unit.id,
        runtime,
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    eventually("the provider's command holds ready", || async {
        unit.began(K::UnitAfterUp) == 1
    })
    .await;
    let published: Option<orch_types::Id> =
        sqlx::query_scalar("SELECT machine_id FROM providers WHERE id=$1")
            .bind(provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(published.is_none());
    let create = host
        .calls()
        .into_iter()
        .find(|call| call.kind == K::NodeRuntimeCreate)
        .unwrap();
    assert_eq!(create.body["volumes"]["state"]["storage"], "fast");
    assert_eq!(create.body["spec"]["needs"]["memory_mb"], 512);
    let programs: Json =
        serde_json::from_str(create.body["env"]["ORCH_PLUGINS"].as_str().unwrap()).unwrap();
    assert_eq!(programs[0]["name"], "team-format");
    assert_eq!(programs[0]["kind"], "translator");
    unit.let_go();
    let (state, _, error) = wait_over(&world, job).await;
    assert_eq!(state, "succeeded", "{error:?}");
    world.engine().stop();
}

#[tokio::test]
async fn cancelling_an_unconfirmed_command_stops_its_unit_and_records_an_unknown_exit_without_output()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "cancel-tools", 16384).await;
    let template = hosting::template(&world).await;
    let made = hosting::make_with(
        &world,
        template,
        "Cancel setup",
        Some(host.id),
        &json!({"env.after_up":{"10-held":{"command":["held-tool"]}}}),
    )
    .await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let unit = hosting::enroll_holding(&world, host.started().unwrap(), K::UnitAfterUp).await;
    eventually("the command attempt is in progress", || async {
        unit.began(K::UnitAfterUp) == 1
    })
    .await;
    world
        .engine()
        .cancel_job(made.job.id, world.ada)
        .await
        .unwrap();
    assert_eq!(wait_over(&world, made.job.id).await.0, "cancelled");
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    let lines: Vec<Json> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE object_id=$1 AND action='unit.command_ran'",
    )
    .bind(made.id)
    .fetch_all(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["name"], "10-held");
    assert!(lines[0]["exit"].is_null());
    assert!(lines[0].get("first_lines").is_none());
    world.engine().stop();
}

#[tokio::test]
async fn provider_command_failure_keeps_its_state_volume_and_stops_the_runtime_with_a_visible_reason()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "failed-provider-tool", 16384).await;
    runtimes::provider_template(&world).await;
    let provider = runtimes::managed_subscription(&world, "Missing helper").await;
    sqlx::query("UPDATE providers SET runtime_config=$2 WHERE id=$1")
        .bind(provider)
        .bind(json!({"runtime.after_up":{"10-static-helper":{"command":["/opt/missing-helper"]}}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let job = runtimes::queue(
        &world,
        provider,
        orch_engine::jobs::provider_provision::Mode::Make,
    )
    .await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    let unit = hosting::connector(&world, "failing-provider-unit", &["provider", "agent"]).await;
    unit.added.machine.handler.on(K::UnitAfterUp, |command| {
        let Command::UnitAfterUp(body) = command else {
            unreachable!()
        };
        Ok(CommandResult::UnitAfterUp(UnitAfterUpResult {
            name: body.name.clone(),
            exit: None,
            duration_ms: 0,
            timed_out: false,
            first_lines: "the command could not run (entity not found)".into(),
        }))
    });
    orch_db::queries::resources::hosting::adopt_machine(
        &mut world.pool.acquire().await.unwrap(),
        unit.id,
        runtime,
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    let (state, _, error) = wait_over(&world, job).await;
    assert_eq!(state, "failed");
    assert!(error.unwrap().to_string().contains("10-static-helper"));
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    let state = orch_db::queries::resources::provider_runtime::state(
        &mut world.pool.acquire().await.unwrap(),
        provider,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(state.machine_id.is_none());
    assert!(state.status_detail.unwrap().contains("10-static-helper"));
    world.engine().stop();
}
