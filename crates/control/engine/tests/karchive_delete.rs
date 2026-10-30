//! Home deletion replays real Docker cleanup and preserves volumes until their frozen deadline.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::{World, eventually};
use futures::FutureExt;
use hosting::ScriptedHost;
use orch_driver_contract::{driver::EnvironmentDriver, runtime::RuntimeSpec, units::UnitRef};
use orch_driver_docker::{DockerDriver, config::DockerConfig};
use orch_link_contract::commands::{
    Command as LinkCommand, CommandError, CommandKind as K, CommandResult, Empty, ErrorCode,
};
use orch_types::{Hash, Id, Json, new_id};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::process::Command;

/// Daemon calls have headroom on the shared machine, but cannot wait indefinitely.
const DOCKER_WAIT: Duration = Duration::from_secs(120);
/// The disposable connector only sleeps and needs no graceful shutdown work.
const STOP_GRACE: Duration = Duration::from_secs(1);
/// The process lasts beyond the bounded fixture and is always removed afterward.
const UNIT_LIFETIME: u32 = 3_600;

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

fn journal(value: &Json) {
    let path = std::env::var_os("ORCH_ARCHIVE_DELETE_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("archive-delete-resources.json"));
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

#[derive(Clone)]
struct Managed {
    environment: Id,
    runtime: Id,
    name: String,
    volumes: [String; 3],
    refs: Json,
}

async fn seed(
    world: &World,
    host: &ScriptedHost,
    home: Id,
    template: Id,
    token: &str,
    part: &str,
    days: i64,
) -> Result<Managed, String> {
    let environment = new_id();
    let runtime = new_id();
    let name = format!("archive-delete-{token}-{part}");
    let volumes = [
        format!("{name}-workspace"),
        format!("{name}-home"),
        format!("{name}-state"),
    ];
    let refs = json!({"workspace":{"name":volumes[0],"storage":null},"home":{"name":volumes[1],"storage":null},"state":{"name":volumes[2],"storage":null}});
    sqlx::query(
        "INSERT INTO environments(id,name,owner_id,org_id,placement,home_node_id,template_id,
        generation,status,config) VALUES($1,$2,$3,$4,'managed',$5,$6,1,'running',$7)",
    )
    .bind(environment)
    .bind(&name)
    .bind(world.ada)
    .bind(world.root)
    .bind(home)
    .bind(template)
    .bind(json!({"env.removed_keep_days":days,"env.idle_stop_minutes":0}))
    .execute(world.pool.pool())
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query("INSERT INTO runtimes(id,kind,host_id,unit_name,environment_id,driver,template_id,spec_hash,generation,state,volumes)
        VALUES($1,'environment',$2,$3,$4,'docker',$5,$6,1,'running',$7)")
        .bind(runtime).bind(host.id).bind(&name).bind(environment).bind(template).bind(Hash::of(b"archive-delete-fixture").as_bytes().as_slice())
        .bind(&refs).execute(world.pool.pool()).await.map_err(|error|error.to_string())?;
    Ok(Managed {
        environment,
        runtime,
        name,
        volumes,
        refs,
    })
}

fn spec(managed: &Managed, system: &str) -> RuntimeSpec {
    serde_json::from_value(json!({"runtime_id":managed.runtime,"kind":"environment","name":managed.name,
        "driver":"docker","spec":{"image":"alpine:latest","runtime":"runc","user":{"uid":0,"gid":0},"resources":{}},
        "resources":{},"roles":["environment"],"gateway_url":"wss://gateway.orch.internal:41400",
        "enroll_token":"disposable-archive-delete-token","ca_cert":"disposable-test-ca","tools":[],
        "volumes":managed.refs,"mounts":[],"env":{},
        "labels":{"orch.system":system,"orch.runtime_id":managed.runtime,"orch.environment_id":managed.environment,"orch.kind":"environment"}})).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleting_a_home_restarts_midway_without_repeating_unit_removal_or_erasing_kept_volumes() {
    // This fixture requires the real daemon and the already-installed small image; absence fails.
    docker(&["image", "inspect", "alpine:latest"])
        .await
        .expect("the real Docker fixture needs the local alpine image");
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let host = ScriptedHost::on(&world, "archive-delete-host", 16_384).await;
    let template = hosting::template(&world).await;
    let token = new_id().simple().to_string();
    let system = format!("archive-delete-{token}");
    let root = std::env::temp_dir().join(format!("archive-delete-{token}"));
    std::fs::create_dir_all(&root).unwrap();
    let program = root.join("orch-connector");
    std::fs::write(&program, format!("#!/bin/sh\nexec sleep {UNIT_LIFETIME}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut config = DockerConfig::default();
    config.tool_cache.connector_dir = Some(root);
    config.networks.env = format!("{system}-env");
    config.networks.ctl = format!("{system}-ctl");
    config
        .labels
        .insert("orch.archive-delete-test".to_owned(), token.clone());
    let networks = [config.networks.env.clone(), config.networks.ctl.clone()];
    let driver = Arc::new(DockerDriver::connect(config).await.unwrap());
    let immediate = seed(&world, &host, world.topic, template, &token, "immediate", 0)
        .await
        .unwrap();
    let kept = seed(&world, &host, world.topic, template, &token, "kept", 7)
        .await
        .unwrap();
    let outside = seed(&world, &host, world.root, template, &token, "outside", 0)
        .await
        .unwrap();
    let managed = [immediate.clone(), kept.clone(), outside.clone()];
    let mut resources = json!({"system":system,"containers":managed.iter().flat_map(|item|[item.name.clone(),format!("{}-prep",item.name)]).collect::<Vec<_>>(),
        "volumes":managed.iter().flat_map(|item|item.volumes.iter().cloned().chain([format!("{}-run",item.name)])).collect::<Vec<_>>(),
        "networks":networks.iter().cloned().chain(managed.iter().map(|item|format!("{}-net",item.name))).collect::<Vec<_>>(),"cleaned":false});
    let reader = format!("{system}-reader");
    resources["containers"]
        .as_array_mut()
        .unwrap()
        .push(json!(reader));
    journal(&resources);
    let handles = Arc::new(Mutex::new(BTreeMap::<Id, UnitRef>::new()));
    let effects = Arc::new(Mutex::new(Vec::<(K, String)>::new()));
    let hold_purge = Arc::new(AtomicBool::new(true));
    let purge_started = Arc::new(AtomicBool::new(false));
    let purge_gate = Arc::new(tokio::sync::Semaphore::new(0));
    let outcome = std::panic::AssertUnwindSafe(async {
        for item in &managed {
            let unit = driver
                .create(&spec(item, &system))
                .await
                .map_err(|error| error.to_string())?;
            driver
                .start(&unit)
                .await
                .map_err(|error| error.to_string())?;
            docker(&["exec",&item.name,"sh","-c",
                "printf kept > /workspace/proof; printf kept > /home/agent/proof; printf kept > /var/lib/orch/proof"])
            .await?;
            handles.lock().unwrap().insert(item.runtime, unit);
        }
        for kind in [
            K::NodeRuntimeStop,
            K::NodeRuntimeRemove,
            K::NodeVolumeRemove,
        ] {
            let driver = driver.clone();
            let handles = handles.clone();
            let effects = effects.clone();
            let hold_purge = hold_purge.clone();
            let purge_started = purge_started.clone();
            let purge_gate = purge_gate.clone();
            let runtime = tokio::runtime::Handle::current();
            host.added.machine.handler.on(kind, move |command| {
                let command = command.clone();
                let driver = driver.clone();
                let handles = handles.clone();
                let effects = effects.clone();
                let hold_purge = hold_purge.clone();
                let purge_started = purge_started.clone();
                let purge_gate = purge_gate.clone();
                let runtime = runtime.clone();
                // The scripted callback is synchronous; hand its worker back while Docker waits.
                tokio::task::block_in_place(|| {
                    runtime.block_on(async move {
                                let operation = async {
                                    let (result, name) = match command {
                                        LinkCommand::NodeRuntimeStop(body) => {
                                            let unit = handles
                                                .lock()
                                                .unwrap()
                                                .get(&body.runtime_id)
                                                .cloned()
                                                .unwrap();
                                            driver.stop(&unit, STOP_GRACE).await?;
                                            (
                                                CommandResult::NodeRuntimeStop(Empty {}),
                                                body.runtime_id.to_string(),
                                            )
                                        }
                                        LinkCommand::NodeRuntimeRemove(body) => {
                                            if !body.keep_volumes && !body.volume_refs.is_empty() {
                                                purge_started.store(true, Ordering::SeqCst);
                                                if hold_purge.load(Ordering::SeqCst) {
                                                    purge_gate.acquire().await.unwrap().forget();
                                                }
                                                for reference in &body.volume_refs {
                                                    let name = reference["name"].as_str().expect("a confirmed purge carries a frozen owned volume name");
                                                    driver.remove_volume(name).await?;
                                                    effects.lock().unwrap().push((K::NodeVolumeRemove, name.to_owned()));
                                                }
                                                return Ok(CommandResult::NodeRuntimeRemove(Empty {}));
                                            }
                                            let unit = handles
                                                .lock()
                                                .unwrap()
                                                .get(&body.runtime_id)
                                                .cloned()
                                                .unwrap();
                                            driver.remove(&unit).await?;
                                            (
                                                CommandResult::NodeRuntimeRemove(Empty {}),
                                                body.runtime_id.to_string(),
                                            )
                                        }
                                        LinkCommand::NodeVolumeRemove(body) => {
                                            driver.remove_volume(&body.name).await?;
                                            (CommandResult::NodeVolumeRemove(Empty {}), body.name)
                                        }
                                        _ => unreachable!(
                                            "only native cleanup commands use this adapter"
                                        ),
                                    };
                                    effects.lock().unwrap().push((kind, name));
                            Ok::<_, orch_driver_contract::driver::DriverError>(result)
                                };
                                match tokio::time::timeout(DOCKER_WAIT, operation).await {
                                    Ok(Ok(result)) => Ok(result),
                                    Ok(Err(error)) => Err(CommandError::new(
                                        ErrorCode::Internal,
                                        error.to_string(),
                                    )),
                                    Err(error) => Err(CommandError::new(
                                        ErrorCode::Timeout,
                                        error.to_string(),
                                    )),
                                }
                    })
                })
            });
        }
        let engine = world.engine();
        let mut tx = world
            .pool
            .begin()
            .await
            .map_err(|error| error.to_string())?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx)
            .await
            .map_err(|error| error.to_string())?;
        engine
            .lock_home_in(&mut tx, world.topic)
            .await
            .map_err(|error| error.to_string())?;
        let now = world.clock.time();
        orch_db::queries::hierarchy::archive(&mut tx, world.topic, world.ada, now.get())
            .await
            .map_err(|error| error.to_string())?;
        engine
            .archive_in(&mut tx, world.topic, world.ada, now)
            .await
            .map_err(|error| error.to_string())?;
        engine
            .remove_home_in(&mut tx, world.topic, world.ada, now, None)
            .await
            .map_err(|error| error.to_string())?;
        orch_db::queries::hierarchy::delete_subtree(&mut tx, world.topic, world.ada, now.get())
            .await
            .map_err(|error| error.to_string())?;
        orch_authz::epoch::bump(&mut tx)
            .await
            .map_err(|error| error.to_string())?;
        tx.commit().await.map_err(|error| error.to_string())?;
        engine.kick();
        eventually("confirmed purge begun before any owned volume is erased", || async {
            purge_started.load(Ordering::SeqCst)
        }).await;
        if docker(&["container", "inspect", &immediate.name])
            .await
            .is_ok()
        {
            return Err("the unit was not removed before its volumes".to_owned());
        }
        for name in &immediate.volumes {
            docker(&["volume", "inspect", name]).await?;
        }
        world.restart_api().await;
        clock.advance(
            chrono::Duration::from_std(world.engine().config().hosting.job_lease).unwrap(),
        );
        hold_purge.store(false, Ordering::SeqCst);
        purge_gate.add_permits(1);
        eventually("immediate owned volumes removed after restart", || async {
            let state: String = sqlx::query_scalar("SELECT COALESCE(removal_phase,'') FROM environments WHERE id=$1")
                .bind(immediate.environment)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
            state == "purged"
        })
        .await;
        for name in &immediate.volumes {
            if docker(&["volume", "inspect", name]).await.is_ok() {
                return Err(format!("eligible volume remains: {name}"));
            }
        }
        eventually(
            "positive-window unit removed and its volumes retained",
            || async {
                let state: String = sqlx::query_scalar("SELECT COALESCE(removal_phase,'') FROM environments WHERE id=$1")
                    .bind(kept.environment)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
                state == "kept"
            },
        )
        .await;
        if docker(&["container", "inspect", &kept.name]).await.is_ok() {
            return Err("the kept environment still has a unit".to_owned());
        }
        for name in &kept.volumes {
            docker(&["volume", "inspect", name]).await?;
            let mount=format!("type=volume,source={name},target=/data,readonly");
            let content=docker(&["run","--rm","--name",&reader,"--network","none","--mount",&mount,
                "alpine:latest","cat","/data/proof"]).await?;
            if content!="kept" {return Err("retained volume content changed".to_owned());}
        }
        docker(&["exec", &outside.name, "test", "-e", "/workspace/proof"]).await?;
        let remove_count = |runtime: Id| {
            effects
                .lock()
                .unwrap()
                .iter()
                .filter(|(kind, name)| {
                    *kind == K::NodeRuntimeRemove && *name == runtime.to_string()
                })
                .count()
        };
        if remove_count(immediate.runtime) != 1
            || remove_count(kept.runtime) != 1
            || remove_count(outside.runtime) != 0
        {
            return Err("unit cleanup repeated or reached a different home".to_owned());
        }
        // A later policy edit does not shorten the already-frozen seven-day promise.
        world
            .set(world.root, "env.removed_keep_days", json!(0))
            .await;
        clock.advance(chrono::Duration::days(7));
        world.engine().kick();
        eventually("kept volumes removed only once eligible", || async {
            let state: String = sqlx::query_scalar("SELECT COALESCE(removal_phase,'') FROM environments WHERE id=$1")
                .bind(kept.environment)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
            state == "purged"
        })
        .await;
        for name in &kept.volumes {
            if docker(&["volume", "inspect", name]).await.is_ok() {
                return Err("a due kept volume remains".to_owned());
            }
        }
        if remove_count(kept.runtime) != 1 {
            return Err("purging retained volumes removed the unit again".to_owned());
        }
        docker(&["exec", &outside.name, "test", "-e", "/workspace/proof"]).await?;
        Ok::<(), String>(())
    })
    .catch_unwind()
    .await;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(panic) => Err(panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                panic
                    .downcast_ref::<&str>()
                    .map(|message| (*message).to_owned())
            })
            .unwrap_or_else(|| "the native cleanup proof panicked".to_owned())),
    };
    world.engine().stop();
    hold_purge.store(false, Ordering::SeqCst);
    purge_gate.add_permits(1);
    host.added.machine.handler.release();
    if docker(&["container", "inspect", &reader]).await.is_ok() {
        docker(&["container", "rm", "-f", &reader]).await.unwrap();
    }
    // Every native object belongs to this fixture's recorded system and exact names.
    for item in &managed {
        for name in [&item.name, &format!("{}-prep", item.name)] {
            if docker(&["container", "inspect", name]).await.is_ok() {
                docker(&["container", "rm", "-f", name]).await.unwrap();
            }
        }
        for name in item
            .volumes
            .iter()
            .chain(std::iter::once(&format!("{}-run", item.name)))
        {
            if docker(&["volume", "inspect", name]).await.is_ok() {
                docker(&["volume", "rm", name]).await.unwrap();
            }
        }
    }
    // Each unit stands on a network of its own, removed with the two common bridges' names.
    let own: Vec<String> = managed
        .iter()
        .map(|item| format!("{}-net", item.name))
        .collect();
    for name in networks.iter().chain(own.iter()) {
        if docker(&["network", "inspect", name]).await.is_ok() {
            docker(&["network", "rm", name]).await.unwrap();
        }
    }
    resources["cleaned"] = json!(true);
    journal(&resources);
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[tokio::test]
async fn archive_closes_a_real_provider_lease_and_cancels_undelivered_inputs_without_starting_another_turn()
 {
    let world = World::on_a_key("disposable-archive-key").await;
    let thread = world.thread("work").await;
    let (_turn, _start) = world.running_turn(thread, "work").await;
    world
        .engine()
        .submit_input(
            &orch_authz::Caller::session(world.ada),
            thread,
            common::queued("waiting"),
        )
        .await
        .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    world
        .engine()
        .lock_archive_in(&mut tx, thread)
        .await
        .unwrap();
    let now = world.clock.time();
    orch_db::queries::hierarchy::archive(&mut tx, thread, world.ada, now.get())
        .await
        .unwrap();
    world
        .engine()
        .archive_in(&mut tx, thread, world.ada, now)
        .await
        .unwrap();
    orch_authz::epoch::bump(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    let leases: (i64, i64) = sqlx::query_as(
        "SELECT count(*),count(*) FILTER(WHERE closed_at IS NULL) FROM leases WHERE thread_id=$1",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        leases,
        (1, 0),
        "the fixture opened a real database lease and archive closed it"
    );
    eventually("provider told of the closed lease", || async {
        world.ran(K::LeaseClose) == 1
    })
    .await;
    assert_eq!(world.ran(K::TurnStart), 1);
    world.engine().stop();
}
