//! An environment is made from a template on a host by a job whose phases are worked through in order, with a percentage that does not go down, and whatever goes wrong is undone with the reason kept; a job in the middle of its work when the API restarts is finished by the next process; a host that is full warns or refuses by its setting.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use common::eventually;
use hosting::{
    ScriptedHost, enroll, make, phases, serve, status, template, template_of, wait_over,
};
use orch_link_contract::commands::{CommandError, CommandKind as K, ErrorCode};
use orch_types::Id;

#[tokio::test]
async fn an_environment_is_made_by_asking_the_host_for_its_image_volumes_program_and_unit_in_that_order_and_ends_running()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let made = make(&world, template, "Main").await;
    // The host is told to start the unit; the connector in it comes online.
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    let unit = enroll(&world, runtime).await;
    let (state, progress, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications
        WHERE user_id = $1 AND kind = 'job.finished' AND data ->> 'job_id' = $2",
    )
    .bind(world.ada)
    .bind(made.job.id.to_string())
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        notices, 1,
        "the ordinary runner tells its requesting person when setup finishes"
    );
    assert_eq!(
        host.kinds(),
        [
            K::NodeImageEnsure,
            K::NodeVolumeEnsure,
            K::NodeVolumeEnsure,
            K::NodeVolumeEnsure,
            K::NodeToolEnsure,
            K::NodeRuntimeCreate,
            K::NodeRuntimeStart,
        ]
    );
    let phases = phases(&progress);
    assert_eq!(
        phases
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        [
            "schedule", "image", "volumes", "tools", "create", "enroll", "attach", "files",
            "after_up", "verify", "commit"
        ]
    );
    assert!(
        phases.iter().all(|(name, state)| state
            == if name == "after_up" {
                "skipped"
            } else {
                "done"
            }),
        "{phases:?}"
    );
    assert_eq!(progress["percent"], 100);
    assert_eq!(
        unit.kinds(),
        [K::EnvAttach, K::ExecStart],
        "the environment was attached once, and the executor its template offers was started"
    );
    let attaches = unit.calls_of(K::EnvAttach);
    assert!(
        attaches[0].body["workspace_root"].is_null(),
        "the driver supplies a unit's root"
    );
    assert_eq!(
        attaches[0].body["data_sources"],
        serde_json::json!({
            "workspace":{"kind":"made_by_orch"},"home":{"kind":"made_by_orch"}
        }),
        "a managed attachment retains its ordinary made-data selection"
    );
    let (status, detail) = status(&world, made.id).await;
    assert_eq!((status.as_str(), detail), ("running", None));
    let (generation, machine, root): (i32, Option<Id>, String) = sqlx::query_as(
        "SELECT generation, machine_id, workspace_root FROM environments WHERE id = $1",
    )
    .bind(made.id)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        (generation, machine, root.as_str()),
        (1, Some(unit.id), "/workspace")
    );
    let (runtime_state, secrets): (String, i64) = sqlx::query_as(
        "SELECT state, (SELECT count(*) FROM secrets WHERE owner_id = $1 AND name = 'enroll_token')
           FROM runtimes WHERE id = $1",
    )
    .bind(runtime)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(runtime_state, "running");
    assert_eq!(secrets, 0, "the token was single use and is gone");
}

#[tokio::test]
async fn a_unit_is_made_with_the_builds_of_the_harnesses_its_template_lets_run_beside_its_tools() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template_of(
        &world,
        &["codex-exec-server", "claude-code", "plain"],
        &["claude-code", "codex"],
    )
    .await;
    let made = make(&world, template, "Main").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    let _unit = enroll(&world, runtime).await;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let calls = host.calls();
    let named = |kind: K| -> Vec<String> {
        calls
            .iter()
            .filter(|call| call.kind == kind)
            .map(|call| call.body["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        named(K::NodeToolEnsure),
        ["orch-connector", "claude-code", "codex"],
        "the host is asked for the connector first, and then for each harness once"
    );
    let create = calls
        .iter()
        .find(|call| call.kind == K::NodeRuntimeCreate)
        .unwrap();
    assert_eq!(
        create.body["tools"],
        serde_json::json!([
            { "name": "claude-code", "version": "1.0.0" },
            { "name": "codex", "version": "1.0.0" }
        ]),
        "the unit is to have the harnesses and not the connector, which it runs from its own place"
    );
    assert_eq!(
        create.body["roles"],
        serde_json::json!(["environment", "agent"]),
        "and its connector hosts harnesses"
    );
}

#[tokio::test]
async fn a_unit_whose_template_lets_no_harness_run_there_is_made_with_the_connector_alone() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let made = make(&world, template, "Main").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    let _unit = enroll(&world, runtime).await;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let create = host
        .calls()
        .into_iter()
        .find(|call| call.kind == K::NodeRuntimeCreate)
        .unwrap();
    assert_eq!(create.body["tools"], serde_json::json!([]));
    assert_eq!(create.body["roles"], serde_json::json!(["environment"]));
    assert_eq!(
        host.count(K::NodeToolEnsure),
        1,
        "the connector, and nothing else"
    );
}

fn refuse(host: &ScriptedHost, kind: K, code: ErrorCode, why: &str) {
    host.refuse(kind, CommandError::new(code, why));
}

/// An environment is made while `ready` sets up what goes wrong in `phase`; the job fails there, and
/// what it had made is taken away again: the unit, and since this was the first time the volumes, with
/// the reason on the job and on the environment.
async fn a_failure_is_undone(phase: &str, ready: impl FnOnce(&ScriptedHost)) {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    ready(&host);
    let made = make(&world, template, "Main").await;
    let (state, progress, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "failed");
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications
        WHERE user_id = $1 AND kind = 'job.finished' AND data ->> 'job_id' = $2",
    )
    .bind(world.ada)
    .bind(made.job.id.to_string())
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        notices, 1,
        "a failed ordinary job also tells its requesting person once"
    );
    let error = error.unwrap();
    assert_eq!(error["phase"], phase, "{error}");
    assert_eq!(
        error["undone"], true,
        "everything it made was removed again: {error}"
    );
    let (environment, detail) = status(&world, made.id).await;
    assert_eq!(environment, "failed");
    assert!(
        detail.as_deref().unwrap_or_default().starts_with(phase),
        "the environment says in which phase and why: {detail:?}"
    );
    assert_ne!(progress["percent"], 100);
    let made_a_unit = host.count(K::NodeRuntimeCreate) > 0 || phase == "create";
    if made_a_unit || phase != "image" {
        eventually("the host to be told to take it all away", || async {
            host.count(K::NodeVolumeRemove) == 3
        })
        .await;
    }
    assert!(host.count(K::NodeRuntimeRemove) >= 1);
    let runtimes: Vec<String> =
        sqlx::query_scalar("SELECT state FROM runtimes WHERE environment_id = $1")
            .bind(made.id)
            .fetch_all(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(runtimes, ["removed"], "{phase}: no unit is left standing");
}

#[tokio::test]
async fn a_host_that_cannot_make_the_image_fails_the_job_in_that_phase_and_it_is_undone() {
    a_failure_is_undone("image", |host| {
        refuse(
            host,
            K::NodeImageEnsure,
            ErrorCode::NotFound,
            "no such image",
        );
    })
    .await;
}

#[tokio::test]
async fn a_volume_that_cannot_be_made_fails_the_job_and_the_ones_made_before_it_are_taken_away() {
    a_failure_is_undone("volumes", |host| {
        host.refuse(
            K::NodeVolumeEnsure,
            CommandError::new(ErrorCode::Unsupported, "the storage is full"),
        );
    })
    .await;
}

#[tokio::test]
async fn a_program_that_cannot_be_put_in_the_cache_fails_the_job_in_that_phase_and_it_is_undone() {
    a_failure_is_undone("tools", |host| {
        refuse(
            host,
            K::NodeToolEnsure,
            ErrorCode::Unsupported,
            "the download failed",
        );
    })
    .await;
}

#[tokio::test]
async fn a_unit_that_cannot_be_made_fails_the_job_in_that_phase_and_it_is_undone() {
    a_failure_is_undone("create", |host| {
        refuse(
            host,
            K::NodeRuntimeCreate,
            ErrorCode::Unsupported,
            "no way to make it",
        );
    })
    .await;
}

#[tokio::test]
async fn a_unit_that_its_node_finds_failed_before_its_connector_came_online_fails_the_job_at_once()
{
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let made = make(&world, template, "Main").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    host.tell(
        host.started().unwrap(),
        "failed",
        Some("exited with status 1"),
    )
    .await;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["phase"], "enroll");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("exited with status 1"),
        "{error}"
    );
    assert_eq!(error["undone"], true);
    assert_eq!(status(&world, made.id).await.0, "failed");
}

#[tokio::test]
async fn a_unit_whose_connector_never_comes_online_fails_the_job_when_the_time_allowed_runs_out() {
    // Deliberately expire enrollment in this case; ordinary worlds keep its deployment bound.
    const ENROLLMENT_EXPIRY: std::time::Duration = std::time::Duration::from_secs(10);
    let world = World::tuned(|config| {
        config.hosting.enroll_timeout = ENROLLMENT_EXPIRY;
    })
    .await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    // The tests allow ten seconds; a connector that is not heard of by then is not coming.
    let made = make(&world, template, "Main").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["phase"], "enroll");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("did not come online within 10 seconds"),
        "{error}"
    );
    assert!(
        host.count(K::NodeRuntimeRemove) >= 1,
        "the unit was taken away"
    );
}

#[tokio::test]
async fn an_environment_the_connector_cannot_attach_fails_the_job_and_the_unit_is_taken_away() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let made = make(&world, template, "Main").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let unit = enroll(&world, host.started().unwrap()).await;
    unit.added.machine.handler.on(K::EnvAttach, |_| {
        Err(CommandError::new(
            ErrorCode::PolicyRootNotAllowed,
            "the machine's owner does not offer that directory",
        ))
    });
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["phase"], "attach", "{error}");
    assert_eq!(error["code"], "policy.root_not_allowed");
    assert_eq!(error["undone"], true);
    assert_eq!(host.count(K::NodeVolumeRemove), 3);
}

#[tokio::test]
async fn a_new_side_that_reports_no_workspace_is_not_healthy_and_the_job_is_undone() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let made = make(&world, template, "Main").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let unit = enroll(&world, host.started().unwrap()).await;
    unit.added.machine.handler.on(K::EnvAttach, |_| {
        let mut facts = hosting::facts();
        facts["workspace_root"] = serde_json::json!("");
        Ok(orch_link_contract::commands::CommandResult::EnvAttach(
            serde_json::from_value(facts).unwrap(),
        ))
    });
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["phase"], "verify", "{error}");
    assert_eq!(error["code"], "unhealthy");
    assert_eq!(status(&world, made.id).await.0, "failed");
}

#[tokio::test]
async fn a_template_switched_off_before_the_unit_is_scheduled_fails_the_job_there_and_nothing_is_sent_to_the_host()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let made = hosting::make_after(
        &world,
        template,
        "Main",
        "UPDATE env_templates SET enabled = false",
    )
    .await;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["phase"], "schedule");
    assert_eq!(error["code"], "template_unavailable");
    assert_eq!(error["undone"], true);
    assert!(
        host.kinds().is_empty(),
        "nothing was sent: {:?}",
        host.kinds()
    );
    let (environment, detail) = status(&world, made.id).await;
    assert_eq!(environment, "failed");
    assert!(detail.unwrap().starts_with("schedule"));
}

#[tokio::test]
async fn a_job_that_is_cancelled_while_the_image_is_made_is_undone_and_the_environment_is_as_it_was()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    host.added.machine.handler.hold(K::NodeImageEnsure);
    let made = make(&world, template, "Main").await;
    host.wait_began(K::NodeImageEnsure, 1).await;
    assert!(
        world
            .engine()
            .cancel_job(made.job.id, world.ada)
            .await
            .unwrap()
    );
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "cancelled");
    let error = error.unwrap();
    assert_eq!(
        (error["code"].as_str(), error["phase"].as_str()),
        (Some("cancelled"), Some("image"))
    );
    assert_eq!(error["undone"], true);
    host.added.machine.handler.release();
    let (environment, _) = status(&world, made.id).await;
    assert_eq!(
        environment, "none",
        "an environment whose first making was cancelled was never made"
    );
    assert_eq!(
        host.count(K::NodeVolumeEnsure),
        0,
        "the phases after it never began"
    );
    assert_eq!(host.count(K::NodeRuntimeCreate), 0);
}

#[tokio::test]
async fn a_job_that_has_passed_its_commit_cannot_be_cancelled() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let made = make(&world, template, "Main").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let _unit = enroll(&world, host.started().unwrap()).await;
    let (state, _, _) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded");
    assert!(
        !world
            .engine()
            .cancel_job(made.job.id, world.ada)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_job_in_the_middle_of_its_work_when_the_api_restarts_is_finished_by_the_next_process() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    host.added.machine.handler.hold(K::NodeImageEnsure);
    let made = make(&world, template, "Main").await;
    host.wait_began(K::NodeImageEnsure, 1).await;
    let (_, before, _) = hosting::job(&world, made.job.id).await;
    world.restart_api().await;
    serve(&world);
    host.added.machine.handler.release();
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let _unit = enroll(&world, host.started().unwrap()).await;
    let (state, progress, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(progress["percent"], 100);
    assert!(
        phases(&progress).iter().all(|(name, state)| state
            == if name == "after_up" {
                "skipped"
            } else {
                "done"
            }),
        "{:?}",
        phases(&progress)
    );
    assert!(
        progress["percent"].as_u64() >= before["percent"].as_u64(),
        "the bar did not go down across the restart"
    );
    assert_eq!(
        host.count(K::NodeImageEnsure),
        1,
        "the image was asked for once, not again"
    );
    assert_eq!(
        host.count(K::NodeRuntimeCreate),
        1,
        "the unit was made once"
    );
    assert_eq!(status(&world, made.id).await.0, "running");
}

#[tokio::test]
async fn a_job_whose_runner_died_is_taken_over_when_its_lease_has_run_out_and_not_before() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    // A job whose runner holds the lease for a moment and then is heard of no more.
    let made = hosting::make_after(
        &world,
        template,
        "Main",
        "UPDATE jobs SET plan = jsonb_set(plan, '{runner}', jsonb_build_object('id', gen_random_uuid()::text, 'until', to_jsonb(now() + interval '1500 milliseconds')))
          WHERE environment_id = (SELECT id FROM environments ORDER BY created_at DESC LIMIT 1)",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert!(
        host.kinds().is_empty(),
        "the lease of a runner that may still be alive is respected"
    );
    host.wait_for(K::NodeImageEnsure, 1).await;
    let (state, _, _) = hosting::job(&world, made.job.id).await;
    assert_eq!(
        state, "running",
        "taken over by this process once the lease ran out"
    );
}

#[tokio::test]
async fn the_hosts_progress_in_making_the_image_is_the_jobs_and_the_bar_never_goes_down() {
    use orch_link_contract::events::{Event, JobProgress};
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    host.added.machine.handler.hold(K::NodeImageEnsure);
    let made = make(&world, template, "Main").await;
    host.wait_began(K::NodeImageEnsure, 1).await;
    let said = |percent: u32| {
        Event::JobProgress(JobProgress { command_id:None,
            job_id: made.job.id,
            progress: serde_json::from_value(serde_json::json!({
                "phase": "image", "phases": [], "percent": percent, "files_done": 0, "files_total": null,
                "bytes_done": u64::from(percent) * 1_000, "bytes_total": 100_000, "rate_bps": null,
                "eta_seconds": null, "message": format!("pulled {percent} percent")
            })).unwrap(),
            log: Some("Step 1/2 : FROM busybox".to_owned()),
        })
    };
    let percent = || async {
        let (_, progress, _) = hosting::job(&world, made.job.id).await;
        progress["percent"].as_u64().unwrap()
    };
    host.added
        .machine
        .publish(&orch_link_contract::streams::Stream::Node, &said(50))
        .await;
    eventually("the bar to follow the pull", || async {
        percent().await >= 20
    })
    .await;
    let halfway = percent().await;
    host.added
        .machine
        .publish(&orch_link_contract::streams::Stream::Node, &said(10))
        .await;
    host.added
        .machine
        .publish(&orch_link_contract::streams::Stream::Node, &said(80))
        .await;
    eventually("the bar to go on", || async { percent().await > halfway }).await;
    assert!(
        percent().await > halfway,
        "a report that is behind does not take it back"
    );
    let (_, progress, _) = hosting::job(&world, made.job.id).await;
    assert_eq!(
        progress["log"], "Step 1/2 : FROM busybox",
        "the end of what the build printed is kept"
    );
    assert_eq!(progress["message"], "pulled 80 percent");
}

fn no_capacity(why: &str) -> CommandError {
    CommandError {
        code: "no_capacity".to_owned(),
        detail: why.to_owned(),
        retryable: false,
        details: None,
    }
}

#[tokio::test]
async fn a_host_that_is_full_warns_by_default_and_a_unit_it_cannot_make_says_what_it_had_declared()
{
    let world = World::new().await;
    serve(&world);
    // 1 100 MiB, and an environment declares 1 024: one fits, a second finds the host full.
    let host = ScriptedHost::on(&world, "host-1", 1_100).await;
    let template = template(&world).await;
    let (_first, _, _unit) = hosting::running(&world, &host, template, "First").await;
    let before = host.kinds().len();
    host.refuse(K::NodeRuntimeCreate, no_capacity("out of memory"));
    let made = make(&world, template, "Second").await;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["phase"], "create");
    assert_eq!(error["code"], "no_capacity");
    assert_eq!(
        error["message"],
        "host-1 could not start it: out of memory. It had declared 76 MiB free, and this environment needs 1024 MiB"
    );
    assert!(
        host.kinds().len() > before,
        "the host was tried: a warning does not refuse"
    );
    let (_, plan): (String, serde_json::Value) =
        sqlx::query_as("SELECT state, plan FROM jobs WHERE id = $1")
            .bind(made.job.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(plan["warnings"], serde_json::json!(["full", "no_room"]));
}

#[tokio::test]
async fn a_host_that_blocks_when_it_is_full_refuses_before_anything_is_made() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 1_100).await;
    let template = template(&world).await;
    let (_first, _, _unit) = hosting::running(&world, &host, template, "First").await;
    sqlx::query("UPDATE hosts SET when_full = 'block' WHERE id = $1")
        .bind(host.id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let before = host.kinds().len();
    // A host that is named and cannot take it is not replaced by another.
    let refused = hosting::try_make(
        &world,
        template,
        "Second",
        Some(host.id),
        &serde_json::json!({}),
    )
    .await
    .unwrap_err();
    let orch_engine::environments::CreateError::NoPlace(none) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(none.code, "no_capacity");
    assert!(
        none.message.contains("host-1") && none.message.contains("full"),
        "{}",
        none.message
    );
    // So is one that is left to the scheduler, when every host blocks.
    let refused = hosting::try_make(&world, template, "Third", None, &serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(refused, orch_engine::environments::CreateError::NoPlace(none) if none.code == "no_capacity")
    );
    assert_eq!(host.kinds().len(), before, "nothing was made or copied");
    let environments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM environments WHERE name IN ('Second', 'Third')")
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(environments, 0);
}

#[tokio::test]
async fn the_scheduler_takes_the_host_with_the_most_free_memory_and_prefers_one_with_room_to_one_that_warns()
 {
    use orch_db::audit::{Actor, Origin};
    use orch_settings::{Target, write as settings_write};
    let world = World::new().await;
    serve(&world);
    let small = ScriptedHost::on(&world, "small", 2_000).await;
    let large = ScriptedHost::on(&world, "large", 16_000).await;
    let template = template(&world).await;
    let _first = make(&world, template, "First").await;
    large.wait_for(K::NodeRuntimeCreate, 1).await;
    assert!(
        small.kinds().is_empty(),
        "the host with the most free memory was chosen"
    );
    // The system counts a host as full from five percent of its memory declared: the large host is full
    // now, with more room than the small one, which has none declared. A host that warns comes last.
    let mut conn = world.pool.acquire().await.unwrap();
    settings_write::write(
        &mut conn,
        &settings_write::Write {
            actor: Actor::User(world.ada),
            target: Target::System,
            key: "hosts.full_percent",
            value: serde_json::json!(5),
            locked: None,
            expected_revision: None,
            may_lock: true,
            origin: &Origin::default(),
        },
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    drop(conn);
    let _second = make(&world, template, "Second").await;
    small.wait_for(K::NodeRuntimeCreate, 1).await;
    assert_eq!(
        large.count(K::NodeRuntimeCreate),
        1,
        "the second went to the host that has room"
    );
}

async fn first_provision_reuses_its_capacity_entry(named: bool) {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "bounded-host", 16_384).await;
    let template = template(&world).await;
    let entry = hosting::limited_physical_use(
        &world,
        orch_access_contract::permissions::ResourceKind::Host,
        host.id,
        1,
    )
    .await;
    let selected = named.then_some(host.id);
    let made = hosting::try_make(
        &world,
        template,
        "First capacity owner",
        selected,
        &serde_json::json!({}),
    )
    .await
    .expect("the first environment owns the one available registration slot");
    // Observe either meaningful progress or the actual terminal failure, using the fixture's
    // existing deadline. A quota failure must not masquerade as an enrollment timeout.
    eventually(
        "the first reserved environment starts its runtime or reports its failure",
        || async {
            if host.started().is_some() {
                return true;
            }
            let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
                .bind(made.job.id)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
            matches!(state.as_str(), "failed" | "cancelled")
        },
    )
    .await;
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
        .bind(made.job.id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_ne!(
        state, "failed",
        "provisioning must not count its own persisted entry as another environment"
    );
    assert_ne!(state, "cancelled");
    let runtime = host
        .started()
        .expect("the admitted environment reached native runtime start");
    let _unit = enroll(&world, runtime).await;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(status(&world, made.id).await.0, "running");
    let stored: Option<Id> =
        sqlx::query_scalar("SELECT host_entry_id FROM environments WHERE id=$1")
            .bind(made.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        stored,
        Some(entry),
        "automatic and named placement retain the actual shared Use entry"
    );
    let before = host.count(K::NodeRuntimeCreate);
    for (index, selected) in [None, Some(host.id)].into_iter().enumerate() {
        let refused = hosting::try_make(
            &world,
            template,
            &format!("Refused extra capacity {index}"),
            selected,
            &serde_json::json!({}),
        )
        .await
        .expect_err("a second distinct environment must not borrow the existing reservation");
        assert!(
            matches!(refused, orch_engine::environments::CreateError::NoPlace(ref none) if none.code == "no_capacity"),
            "{refused:?}"
        );
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM environments WHERE host_entry_id=$1 AND deleted_at IS NULL",
    )
    .bind(entry)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        host.count(K::NodeRuntimeCreate),
        before,
        "refused registrations start no extra native unit"
    );
    // A native host failure leaves the same environment and its stored volume host.
    // Its admitted reservation survives a later grant change on its own host or template.
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE permission_entries SET options=$2 WHERE id=$1")
        .bind(entry)
        .bind(serde_json::json!({"max_environments":0}))
        .execute(&mut *tx)
        .await
        .unwrap();
    orch_db::queries::access::bump_epoch(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    host.tell(runtime, "failed", Some("scripted original unit failure"))
        .await;
    eventually(
        "the actual host failure reaches the environment",
        || async { status(&world, made.id).await.0 == "failed" },
    )
    .await;
    let restarting = world
        .engine()
        .start_environment(made.id, Some(world.ada))
        .await
        .unwrap();
    let orch_engine::environments::Starting::Job(replacement) = restarting else {
        panic!("the failed environment must attempt its ordinary replacement job");
    };
    let successor = hosting::runtime_started_for_job(&world, &host, replacement.id).await;
    assert_ne!(
        successor, runtime,
        "replacement starts a distinct native unit"
    );
    let _replacement_unit = enroll(&world, successor).await;
    let (state, _, error) = wait_over(&world, replacement.id).await;
    assert_eq!(
        state, "succeeded",
        "existing placement keeps its admitted creation ceiling: {error:?}"
    );
    assert_eq!(status(&world, made.id).await.0, "running");
    assert_eq!(host.count(K::NodeRuntimeCreate), before + 1);
    let (stored, generation, runtimes): (Option<Id>, i32, i64) = sqlx::query_as(
        "SELECT host_entry_id,generation,(SELECT count(*) FROM runtimes WHERE environment_id=e.id) FROM environments e WHERE id=$1",
    ).bind(made.id).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(
        stored,
        Some(entry),
        "replacement preserves the original reservation identity"
    );
    assert_eq!(generation, 2);
    assert_eq!(
        runtimes, 2,
        "existing placement admits exactly its successor, not a new environment"
    );
    world.shutdown().await;
}

#[tokio::test]
async fn a_named_first_provision_consumes_only_its_existing_capacity_reservation() {
    Box::pin(first_provision_reuses_its_capacity_entry(true)).await;
}

#[tokio::test]
async fn an_automatic_first_provision_consumes_only_its_existing_capacity_reservation() {
    Box::pin(first_provision_reuses_its_capacity_entry(false)).await;
}
#[tokio::test]
async fn a_completed_environment_recreates_on_its_own_host_after_use_is_withdrawn() {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
    use orch_authz::{Caller, Policy, Principal};
    use orch_types::{Json, Time};
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "retained-host", 16_384).await;
    let template = template(&world).await;
    let entry = hosting::limited_physical_use(&world, ResourceKind::Host, host.id, 1).await;
    let own = serde_json::json!({"env.resources":{"cpu_millis":1000,"memory_mb":1024},
        "env.storage":{"workspace":"fast","home":"fast","state":"fast"}});
    let made = hosting::try_make(&world, template, "Retained placement", Some(host.id), &own)
        .await
        .unwrap();
    let runtime = hosting::runtime_started_for_job(&world, &host, made.job.id).await;
    let _unit = enroll(&world, runtime).await;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let admitted: Json =
        sqlx::query_scalar("SELECT plan #> '{created_spec,resources}' FROM jobs WHERE id=$1")
            .bind(made.job.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(admitted["cpu_millis"], 1000);
    assert_eq!(admitted["memory_mb"], 1024);
    let volumes: Json = sqlx::query_scalar("SELECT volumes FROM runtimes WHERE id=$1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(volumes["workspace"]["storage"], "fast");

    let mut tx = world.pool.begin().await.unwrap();
    let source_owner: Id = sqlx::query_scalar("SELECT owner_id FROM hosts WHERE id=$1")
        .bind(host.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(
        orch_db::queries::access::remove_entry(&mut tx, entry, source_owner, world.clock.now())
            .await
            .unwrap()
    );
    orch_db::queries::access::bump_epoch(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let person = Principal::load(
        &mut conn,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    assert!(
        !orch_authz::resources::check(
            &mut conn,
            &person,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host.id
            },
            Permission::HostUse,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    assert!(
        orch_authz::resources::check(
            &mut conn,
            &person,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: made.id
            },
            Permission::EnvironmentManage,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    drop(conn);
    let before = host.count(K::NodeRuntimeCreate);
    assert!(
        hosting::try_make(
            &world,
            template,
            "Refused new placement",
            Some(host.id),
            &own
        )
        .await
        .is_err()
    );
    assert_eq!(host.count(K::NodeRuntimeCreate), before);

    host.tell(runtime, "failed", Some("scripted original unit failure"))
        .await;
    eventually(
        "the original native failure reaches its environment",
        || async { status(&world, made.id).await.0 == "failed" },
    )
    .await;
    let starting = world
        .engine()
        .start_environment(made.id, Some(world.ada))
        .await
        .unwrap();
    let orch_engine::environments::Starting::Job(replacement) = starting else {
        panic!("the failed owned placement must create its replacement job");
    };
    let successor = hosting::runtime_started_for_job(&world, &host, replacement.id).await;
    assert_ne!(successor, runtime);
    let _replacement = enroll(&world, successor).await;
    let (state, _, error) = wait_over(&world, replacement.id).await;
    assert_eq!(
        state, "succeeded",
        "EnvironmentManage retains its admitted own placement: {error:?}"
    );
    let supplied: Json =
        sqlx::query_scalar("SELECT plan #> '{created_spec,resources}' FROM jobs WHERE id=$1")
            .bind(replacement.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        supplied, admitted,
        "the native replacement preserves the admitted resource values"
    );
    let kept_volumes: Json = sqlx::query_scalar("SELECT volumes FROM runtimes WHERE id=$1")
        .bind(successor)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        kept_volumes, volumes,
        "the same owned storage and volume identity remains"
    );
    let (kept_entry, generation): (Option<Id>, i32) =
        sqlx::query_as("SELECT host_entry_id,generation FROM environments WHERE id=$1")
            .bind(made.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(kept_entry, Some(entry));
    assert_eq!(generation, 2);
    assert_eq!(status(&world, made.id).await.0, "running");
    assert_eq!(host.count(K::NodeRuntimeCreate), before + 1);
    // Record a real failure before disabling the host: disable also stops its current unit.
    host.tell(successor, "failed", Some("scripted successor unit failure"))
        .await;
    eventually("the successor's native failure is recorded", || async {
        status(&world, made.id).await.0 == "failed"
    })
    .await;
    // EnvironmentManage does not lift a physical operator's hard host refusal.
    sqlx::query("UPDATE hosts SET disabled=true WHERE id=$1")
        .bind(host.id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let blocked = std::sync::Mutex::new(None);
    eventually("the disabled-host replacement reaches scheduling", || async {
        match world.engine().start_environment(made.id, Some(world.ada)).await {
            Ok(orch_engine::environments::Starting::Job(job)) => {
                *blocked.lock().unwrap() = Some(job);
                true
            }
            Err(orch_engine::environments::StartError::Unavailable(reason))
                if reason == "the host has not confirmed the previous stop" =>
            {
                // Only the real disabled-host stop may delay the schedule check; no report is invented.
                let requested: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM commands WHERE kind='node.runtime.stop' AND body->>'runtime_id'=$1::uuid::text)")
                    .bind(successor).fetch_one(world.pool.pool()).await.unwrap();
                assert!(requested, "the current successor must have an actual stop request");
                false
            }
            result => panic!("the failed placement must report its attempted replacement: {result:?}"),
        }
    }).await;
    let blocked = blocked.into_inner().unwrap().unwrap();
    let (state, _, error) = wait_over(&world, blocked.id).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["phase"], "schedule");
    assert_eq!(error["code"], "no_host");
    assert_eq!(
        host.count(K::NodeRuntimeCreate),
        before + 1,
        "disabled host starts no further native unit"
    );
    world.shutdown().await;
}
#[tokio::test]
async fn retained_placement_cannot_bypass_room_occupied_by_other_work() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "capacity-bound-host", 16_384).await;
    let template = template(&world).await;
    let own = serde_json::json!({"env.resources":{"cpu_millis":1000,"memory_mb":1024}});
    // A different real environment already consumes this host's declared room.
    let other = hosting::try_make(&world, template, "Other running work", Some(host.id), &own)
        .await
        .unwrap();
    let other_runtime = hosting::runtime_started_for_job(&world, &host, other.job.id).await;
    let _other_unit = enroll(&world, other_runtime).await;
    let (state, _, error) = wait_over(&world, other.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let entry = hosting::limited_physical_use(
        &world,
        orch_access_contract::permissions::ResourceKind::Host,
        host.id,
        1,
    )
    .await;
    let made = hosting::try_make(&world, template, "Retained work", Some(host.id), &own)
        .await
        .unwrap();
    let runtime = hosting::runtime_started_for_job(&world, &host, made.job.id).await;
    let _unit = enroll(&world, runtime).await;
    let (state, _, error) = wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");

    let mut tx = world.pool.begin().await.unwrap();
    let owner: Id = sqlx::query_scalar("SELECT owner_id FROM hosts WHERE id=$1")
        .bind(host.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(
        orch_db::queries::access::remove_entry(&mut tx, entry, owner, world.clock.now())
            .await
            .unwrap()
    );
    orch_db::queries::access::bump_epoch(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    host.tell(runtime, "failed", Some("scripted retained unit failure"))
        .await;
    eventually("the retained native unit is failed", || async {
        status(&world, made.id).await.0 == "failed"
    })
    .await;
    // Reported capacity now leaves 476 MiB after the other environment's1024MiB.
    // Failed units are excluded by declared_of, so this shortage is not its own old reservation.
    sqlx::query("UPDATE hosts SET when_full='block',capacity=jsonb_set(capacity,'{memory_mb}','1500') WHERE id=$1")
        .bind(host.id).execute(world.pool.pool()).await.unwrap();
    let before = host.count(K::NodeRuntimeCreate);
    let starting = world
        .engine()
        .start_environment(made.id, Some(world.ada))
        .await
        .unwrap();
    let orch_engine::environments::Starting::Job(blocked) = starting else {
        panic!("the failed placement must attempt its normal replacement job");
    };
    let (state, _, error) = wait_over(&world, blocked.id).await;
    assert_eq!(state, "failed");
    let error = error.unwrap();
    assert_eq!(error["phase"], "schedule");
    assert_eq!(
        error["code"], "no_capacity",
        "EnvironmentManage does not lift WhenFull::Block"
    );
    assert_eq!(
        host.count(K::NodeRuntimeCreate),
        before,
        "no replacement unit starts into occupied room"
    );
    assert_eq!(status(&world, other.id).await.0, "running");
    let (stored, generation): (Option<Id>, i32) =
        sqlx::query_as("SELECT host_entry_id,generation FROM environments WHERE id=$1")
            .bind(made.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(stored, Some(entry));
    assert_eq!(generation, 1);
    world.shutdown().await;
}
