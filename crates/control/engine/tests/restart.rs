//! A restart waits for real turn settlement, keeps a Docker container's outside-volume file and records its requester.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::{World, eventually, queued, turn_began, turn_ended};
use futures::FutureExt;
use orch_api_contract::environment::EnvironmentRestartMode;
use orch_authz::{Caller, Principal};
use orch_db::queries::{environments::hosting as envs, jobs};
use orch_link_contract::commands::{
    Command as LinkCommand, CommandError, CommandKind as K, CommandResult, Empty, ErrorCode,
};
use orch_link_contract::events::TurnOutcome;
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Command;

/// The shared daemon may be busy, but the isolated proof never waits indefinitely.
const DOCKER_WAIT: Duration = Duration::from_secs(120);
async fn docker(args: &[&str]) -> Result<String, String> {
    let output = tokio::time::timeout(
        DOCKER_WAIT,
        Command::new("docker")
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn request(world: &World, environment: Id, mode: EnvironmentRestartMode) -> jobs::JobRow {
    let mut tx = world.pool.begin().await.unwrap();
    let token = orch_types::new_id();
    let hash = orch_types::Hash::of(token.as_bytes());
    let expires = Time::new(world.clock.now() + chrono::Duration::hours(1));
    let policy: orch_access_contract::key_scope::KeyScope =
        serde_json::from_value(json!({"rules":[{
            "target":{"kind":"environment","id":environment,"reach":"self"},
            "allow":[{"operation":"EnvironmentRestart","actions":["manage"]}]
        }]}))
        .unwrap();
    assert!(policy.validate().is_empty());
    orch_db::queries::identity::create_api_token_scoped(
        &mut tx,
        &orch_db::queries::identity::NewApiToken {
            id: token,
            user_id: world.ada,
            name: "Native restart origin",
            token_hash: hash.as_bytes(),
            scopes: &[],
            node_id: None,
            at: world.clock.now(),
            expires_at: Some(expires.get()),
        },
        Some(&json!(policy.clone())),
    )
    .await
    .unwrap();
    let caller = Caller::token(
        world.ada,
        orch_authz::TokenGrant::from_stored_scope(&[], None, Some(policy), Some(expires))
            .unwrap()
            .for_token(token),
    );
    let caller = orch_engine::jobs::restart::operation::admit(&mut tx, &caller, environment)
        .await
        .unwrap();
    let principal = Principal::load(&mut tx, caller.clone(), Time::new(world.clock.now()))
        .await
        .unwrap();
    let http = orch_engine::operation_scope::http_origin::HttpOrigin::capture(
        orch_authz::CapturedOperation::capture(&caller).unwrap(),
        orch_engine::operation_scope::request_origin::Credential::Token {
            id: token,
            hash: hash.as_bytes().to_vec(),
        },
        false,
        principal.has_system_admin_standing(),
        Some(expires),
    )
    .unwrap();
    let engine = world.engine();
    let job = http
        .scope(engine.queue_restart(&mut tx, &principal, environment, mode))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    job
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn waiting_restart_keeps_the_same_container_and_every_file_and_names_the_requester_in_the_timeline()
 {
    docker(&["image", "inspect", "alpine:latest"])
        .await
        .expect("this proof needs the real daemon and its installed alpine image");
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock).await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "same-unit-restart", 16384).await;
    let template = hosting::template(&world).await;
    let (environment, runtime, unit) =
        hosting::running(&world, &host, template, "Keep installed files").await;
    let before = envs::runtime(&mut world.pool.acquire().await.unwrap(), runtime)
        .await
        .unwrap()
        .unwrap();
    let name = before.unit_name.clone();
    let volumes: Vec<_> = before
        .volumes
        .as_object()
        .unwrap()
        .values()
        .map(|volume| volume["name"].as_str().unwrap().to_owned())
        .collect();
    let ws = format!(
        "{}:/workspace",
        before.volumes["workspace"]["name"].as_str().unwrap()
    );
    let home = format!(
        "{}:/home/agent",
        before.volumes["home"]["name"].as_str().unwrap()
    );
    let state = format!(
        "{}:/var/lib/orch",
        before.volumes["state"]["name"].as_str().unwrap()
    );
    let effects = Arc::new(Mutex::new(Vec::<K>::new()));
    let outcome=std::panic::AssertUnwindSafe(async {
        let container=docker(&["create","--name",&name,"--network","none","--label","orch.restart-test=true",
            "--volume",&ws,"--volume",&home,"--volume",&state,"alpine:latest","sh","-c","exec sleep 3600"]).await?;
        docker(&["start",&name]).await?;
        docker(&["exec",&name,"sh","-c","printf kept-outside-volumes > /tmp/restart-proof; printf workspace-kept > /workspace/restart-proof"]).await?;
        for kind in [K::NodeRuntimeStop,K::NodeRuntimeStart] {
            let name=name.clone();let effects=effects.clone();let handle=tokio::runtime::Handle::current();
            host.added.machine.handler.on(kind,move |command| {
                let command=command.clone();let name=name.clone();let effects=effects.clone();let handle=handle.clone();
                tokio::task::block_in_place(||handle.block_on(async move {
                    let result=match command {
                        LinkCommand::NodeRuntimeStop(body)=>{
                            assert_eq!(body.runtime_id,runtime);docker(&["stop","-t","1",&name]).await?;
                            CommandResult::NodeRuntimeStop(Empty{})
                        },
                        LinkCommand::NodeRuntimeStart(body)=>{
                            assert_eq!(body.runtime_id,runtime);docker(&["start",&name]).await?;
                            CommandResult::NodeRuntimeStart(orch_link_contract::commands::NodeRuntimeStartResult {source:orch_link_contract::native_target::RuntimeSource::untracked(),reused:false})
                        },
                        _=>return Err("unexpected node operation".into()),
                    };
                    effects.lock().unwrap().push(kind);Ok::<_,String>(result)
                })).map_err(|error|CommandError::new(ErrorCode::Internal,error))
            });
        }
        unit.added.machine.handler.on(K::EnvDetach,|_|Ok(CommandResult::EnvDetach(Empty{})));
        let thread=world.thread("Wait for this turn").await;hosting::bind(&world,thread,environment).await;
        let (turn,_)=world.running_turn(thread,"working before restart").await;
        let restart=request(&world,environment,EnvironmentRestartMode::Wait).await;
        let waiting=world.engine().submit_input(&Caller::session(world.ada),thread,queued("keep this next input")).await.unwrap();
        eventually("the restart is visibly waiting for the running turn",||async {
            let row=hosting::job(&world,restart.id).await;row.0=="running" && row.1["phase"]=="waiting"
        }).await;
        assert!(effects.lock().unwrap().is_empty(),"a running turn holds the physical stop");
        assert_eq!(world.turn_state(turn.turn_id).await,"running");
        assert_eq!(world.input_state(waiting.input.id).await.0,"pending");
        world.say(thread,turn_ended(turn.turn_id,TurnOutcome::Completed)).await;
        world.wait_turn(turn.turn_id,"completed").await;
        let result=hosting::wait_over(&world,restart.id).await;assert_eq!(result.0,"succeeded","{:?}",result.2);
        assert_eq!(*effects.lock().unwrap(),[K::NodeRuntimeStop,K::NodeRuntimeStart]);
        assert_eq!(docker(&["inspect","--format","{{.Id}}",&name]).await?.trim(),container.trim());
        assert_eq!(docker(&["exec",&name,"cat","/tmp/restart-proof"]).await?,"kept-outside-volumes");
        assert_eq!(docker(&["exec",&name,"cat","/workspace/restart-proof"]).await?,"workspace-kept");
        let mut conn=world.pool.acquire().await.unwrap();
        let after=envs::live_runtime(&mut conn,environment).await.unwrap().unwrap();
        assert_eq!((after.id,after.generation,after.volumes),(runtime,before.generation,before.volumes.clone()));
        let lines:Vec<Json>=sqlx::query_scalar("SELECT payload FROM items WHERE thread_id=$1 AND kind='marker' AND payload->>'marker'='environment_restarted'")
            .bind(thread).fetch_all(&mut *conn).await.unwrap();
        assert_eq!(lines.len(),1);assert_eq!(lines[0]["data"]["by"]["id"],json!(world.ada));
        assert_eq!(lines[0]["data"]["by"]["username"],"ada");
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='environment.restarted' AND actor_id=$2")
            .bind(environment).bind(world.ada).fetch_one(&mut *conn).await.unwrap();assert_eq!(count,1);
        drop(conn);
        // The queued input's turn starts once the restart is verified; the input is delivered when the harness
        // says that turn began.
        let next=world.turn_start(thread,1).await;
        assert_eq!(common::words_after_where(&next.text),"keep this next input");
        world.say(thread,turn_began(next.turn_id)).await;
        eventually("the queued input is delivered after verification",||async {world.input_state(waiting.input.id).await.0=="delivered"}).await;
        assert_eq!(host.count(K::NodeRuntimeCreate),0);assert_eq!(host.count(K::NodeRuntimeRemove),0);assert_eq!(host.count(K::NodeVolumeRemove),0);
        Ok::<_,String>(())
    }).catch_unwind().await;
    world.engine().stop();
    let _ = docker(&["rm", "-f", &name]).await;
    for volume in volumes {
        let _ = docker(&["volume", "rm", &volume]).await;
    }
    match outcome {
        Ok(result) => result.unwrap(),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
async fn cancellation_while_waiting_keeps_the_running_unit_and_queued_input_order() {
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "cancel-restart", 16384).await;
    let template = hosting::template(&world).await;
    let (environment, runtime, _unit) =
        hosting::running(&world, &host, template, "Do not stop").await;
    let thread = world.thread("Still working").await;
    hosting::bind(&world, thread, environment).await;
    let (turn, _) = world.running_turn(thread, "keep running").await;
    let restart = request(&world, environment, EnvironmentRestartMode::Wait).await;
    eventually("waiting restart owns the thread", || async {
        hosting::job(&world, restart.id).await.1["phase"] == "waiting"
    })
    .await;
    world
        .engine()
        .cancel_job(restart.id, world.ada)
        .await
        .unwrap();
    assert_eq!(hosting::wait_over(&world, restart.id).await.0, "cancelled");
    assert_eq!(world.turn_state(turn.turn_id).await, "running");
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
    let current = envs::live_runtime(&mut world.pool.acquire().await.unwrap(), environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.id, runtime);
    world.engine().stop();
}

#[tokio::test]
async fn a_newer_control_ends_a_waiting_restart_without_interrupting_its_turn_or_unit() {
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "superseded-wait", 16384).await;
    let template = hosting::template(&world).await;
    let (environment, runtime, _unit) =
        hosting::running(&world, &host, template, "Keep current work").await;
    let thread = world.thread("Current work").await;
    hosting::bind(&world, thread, environment).await;
    let (turn, _) = world.running_turn(thread, "keep running").await;
    let restart = request(&world, environment, EnvironmentRestartMode::Wait).await;
    eventually("the original restart to wait for its turn", || async {
        hosting::job(&world, restart.id).await.1["phase"] == "waiting"
    })
    .await;
    let mut tx = world.pool.begin().await.unwrap();
    envs::lock_environment(&mut tx, environment).await.unwrap();
    let later = orch_db::queries::runtime_controls::accept(&mut tx, runtime, orch_types::new_id())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    let (outcome, _, error) = hosting::wait_over(&world, restart.id).await;
    assert_eq!(outcome, "failed", "{error:?}");
    assert_eq!(world.turn_state(turn.turn_id).await, "running");
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
    assert_eq!(host.count(K::NodeRuntimeStart), 0);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    assert!(
        orch_db::queries::runtime_controls::is_current(
            &mut world.pool.acquire().await.unwrap(),
            &later
        )
        .await
        .unwrap()
    );
    world.engine().stop();
}

#[tokio::test]
async fn a_newer_control_during_restart_stop_prevents_the_old_start_and_cleanup_stop() {
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "superseded-stopping", 16384).await;
    let template = hosting::template(&world).await;
    let (environment, runtime, _unit) =
        hosting::running(&world, &host, template, "Keep the later control").await;
    let before = envs::runtime(&mut world.pool.acquire().await.unwrap(), runtime)
        .await
        .unwrap()
        .unwrap();
    host.added.machine.handler.hold(K::NodeRuntimeStop);
    let restart = request(&world, environment, EnvironmentRestartMode::Wait).await;
    host.wait_began(K::NodeRuntimeStop, 1).await;
    let forward = orch_engine::ids::derived("restart.unit.stop", &[restart.id.as_bytes()]);
    assert!(
        world
            .commands
            .issued_commands("node.runtime.stop")
            .iter()
            .any(|(_, command)| command.id == forward),
        "the held native effect must be the forward Stop, rather than cleanup after an earlier failure"
    );
    assert_eq!(
        envs::runtime(&mut world.pool.acquire().await.unwrap(), runtime)
            .await
            .unwrap()
            .unwrap()
            .state,
        "stopping",
        "the actual forward phase must have committed before accepting another control"
    );
    let mut tx = world.pool.begin().await.unwrap();
    envs::lock_environment(&mut tx, environment).await.unwrap();
    let later = orch_db::queries::runtime_controls::accept(&mut tx, runtime, orch_types::new_id())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    host.added.machine.handler.release();
    let (outcome, _, error) = hosting::wait_over(&world, restart.id).await;
    assert_eq!(outcome, "failed", "{error:?}");
    assert_eq!(
        host.count(K::NodeRuntimeStop),
        1,
        "the old admitted effect is joined but no cleanup retargets the later control"
    );
    assert_eq!(host.count(K::NodeRuntimeStart), 0);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        orch_db::queries::runtime_controls::is_current(&mut conn, &later)
            .await
            .unwrap()
    );
    let current = envs::runtime(&mut conn, runtime).await.unwrap().unwrap();
    assert_eq!(current.volumes, before.volumes);
    assert_eq!(
        current.state, "stopping",
        "the old reply cannot publish another control's state"
    );
    assert_eq!(hosting::status(&world, environment).await.0, "running");
    world.engine().stop();
}

#[tokio::test]
async fn a_restart_only_key_keeps_its_original_source_across_its_own_stop_phase_progress() {
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "restart-source-phases", 16384).await;
    let template = hosting::template(&world).await;
    let (environment, runtime, unit) =
        hosting::running(&world, &host, template, "Restart the admitted unit").await;
    world.engine().stop();
    let accepted = request(&world, environment, EnvironmentRestartMode::Wait).await;
    let mut tx = world.pool.begin().await.unwrap();
    let row = jobs::job(&mut tx, accepted.id).await.unwrap().unwrap();
    let check = orch_engine::jobs::lifecycle_commands::environment::principal(
        &mut tx,
        &row,
        world.clock.as_ref(),
        world.access.as_ref(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        check.principal.caller().admitted_operation(),
        Some("EnvironmentRestart")
    );
    jobs::merge_plan(&mut tx, row.id, &json!({
        "restart_stop_requested":true,
        "active_command":{"id":orch_engine::ids::derived("restart.env.detach", &[row.id.as_bytes()]),
            "machine":unit.id,"kind":"env.detach","phase":"stopping"}
    }), world.clock.now()).await.unwrap();
    let changed = jobs::job(&mut tx, row.id).await.unwrap().unwrap();
    let check = orch_engine::jobs::lifecycle_commands::environment::principal(
        &mut tx,
        &changed,
        world.clock.as_ref(),
        world.access.as_ref(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        check.principal.caller().admitted_operation(),
        Some("EnvironmentRestart")
    );
    assert!(
        envs::set_runtime_state(
            &mut tx,
            runtime,
            &["running"],
            "stopping",
            None,
            world.clock.now()
        )
        .await
        .unwrap()
    );
    let changed = jobs::job(&mut tx, row.id).await.unwrap().unwrap();
    let check = orch_engine::jobs::lifecycle_commands::environment::principal(
        &mut tx,
        &changed,
        world.clock.as_ref(),
        world.access.as_ref(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        check.principal.caller().admitted_operation(),
        Some("EnvironmentRestart")
    );
    tx.commit().await.unwrap();
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
    assert_eq!(host.count(K::NodeRuntimeStart), 0);
}

#[tokio::test]
async fn a_restart_whose_original_source_disappears_never_falls_back_to_its_actors_broadened_key() {
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "closed-restart-source", 16384).await;
    let template = hosting::template(&world).await;
    let (environment, runtime, _unit) =
        hosting::running(&world, &host, template, "Keep the original source").await;
    let thread = world.thread("Work before source closure").await;
    hosting::bind(&world, thread, environment).await;
    let (turn, _) = world.running_turn(thread, "keep running").await;
    let restart = request(&world, environment, EnvironmentRestartMode::Wait).await;
    eventually("the exact original restart to wait", || async {
        hosting::job(&world, restart.id).await.1["phase"] == "waiting"
    })
    .await;
    let mut tx = world.pool.begin().await.unwrap();
    let row = jobs::job(&mut tx, restart.id).await.unwrap().unwrap();
    let original: orch_engine::operation_scope::http_origin::HttpOrigin =
        serde_json::from_value(row.plan["environment_lifecycle"]["accepted"].clone()).unwrap();
    assert_eq!(original.origin().operation(), "EnvironmentRestart");
    let orch_engine::operation_scope::request_origin::Credential::Token { id, .. } =
        original.credential()
    else {
        panic!("the fixture must keep its real original key");
    };
    sqlx::query("UPDATE api_tokens SET scopes=ARRAY['*'],operation_scope=NULL WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET plan=plan-'environment_lifecycle'-'environment_lifecycle_version'-'environment_lifecycle_required' WHERE id=$1")
        .bind(restart.id).execute(&mut *tx).await.unwrap();
    let stripped = jobs::job(&mut tx, restart.id).await.unwrap().unwrap();
    assert!(
        orch_engine::jobs::restart::Restarting::of(&stripped).is_err(),
        "a known source cannot become an ordinary restart when its marker is gone"
    );
    tx.commit().await.unwrap();
    world.engine().kick();
    let (outcome, _, error) = hosting::wait_over(&world, restart.id).await;
    assert_eq!(outcome, "failed", "{error:?}");
    assert_eq!(world.turn_state(turn.turn_id).await, "running");
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
    assert_eq!(host.count(K::NodeRuntimeStart), 0);
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    assert_eq!(
        envs::runtime(&mut world.pool.acquire().await.unwrap(), runtime)
            .await
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
    world.engine().stop();
}
