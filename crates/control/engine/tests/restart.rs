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
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let job = world
        .engine()
        .queue_restart(&mut tx, &principal, environment, mode)
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
                            CommandResult::NodeRuntimeStart(Empty{})
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
