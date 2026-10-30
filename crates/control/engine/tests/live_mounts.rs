//! Approved live mount changes reach the owned node, refresh environment facts and retain an honest recreate fallback.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use async_trait::async_trait;
use common::World;
use hosting::{ScriptedHost, running, serve, template};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_authz::ownership::{NewAccess, start_resource};
use orch_db::queries::environments::hosting as envs;
use orch_db::queries::link::{self as journal, Ending, NewCommand};
use orch_db::queries::resources::mounts::{self as rows, LocationRow, MountRow};
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing};
use orch_engine::{Engine, EngineParts};
use orch_link_contract::commands::{Command, CommandKind, CommandResult, NodeMountsApplyResult};
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::sync::{Arc, Mutex};

// These cases read the durable command journal, while common's scripted link retains only its own
// in-memory history. Keep that shared fixture unchanged and add the promised SQL rows for these cases.
struct JournaledCommands {
    db: orch_db::Db,
    clock: Arc<dyn orch_types::Clock>,
    link: Arc<dyn Commands>,
}

fn command_error(error: impl std::fmt::Display) -> CommandsError {
    CommandsError::Other(error.to_string())
}

impl JournaledCommands {
    async fn ended(&self, id: Id, outcome: &Outcome) -> Result<(), CommandsError> {
        let failed = match outcome {
            Outcome::Failed(error) => Some(serde_json::to_value(error).map_err(command_error)?),
            _ => None,
        };
        let ending = match outcome {
            Outcome::Succeeded(result) => Ending::Succeeded(result),
            Outcome::Failed(_) => Ending::Failed(failed.as_ref().unwrap()),
            Outcome::Expired => Ending::Expired,
            Outcome::Cancelled => Ending::Cancelled,
        };
        journal::finish_command(
            &mut *self.db.acquire().await.map_err(command_error)?,
            id,
            &ending,
            self.clock.now(),
        )
        .await
        .map_err(command_error)?;
        Ok(())
    }
}

#[async_trait]
impl Commands for JournaledCommands {
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        let now = self.clock.now();
        let deadline = command.deadline.map(Time::get).unwrap_or_else(|| {
            now + chrono::Duration::seconds(i64::from(command.kind.deadline_secs()))
        });
        let key = command.key.wire();
        journal::insert_command(
            &mut *self.db.acquire().await.map_err(command_error)?,
            &NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: key.as_deref(),
                deadline_at: deadline,
            },
            now,
        )
        .await
        .map_err(command_error)?;
        self.link.issue(command).await
    }

    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        let info = self.link.status(id).await?;
        if let Some(outcome) = info.as_ref().and_then(|info| info.outcome.as_ref()) {
            self.ended(id, outcome).await?;
        }
        Ok(info)
    }

    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        let outcome = self.link.wait(id).await?;
        self.ended(id, &outcome).await?;
        Ok(outcome)
    }

    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        let cancelled = self.link.cancel(id).await?;
        if cancelled {
            self.ended(id, &Outcome::Cancelled).await?;
        }
        Ok(cancelled)
    }

    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        Ok(
            journal::command(&mut *self.db.acquire().await.map_err(command_error)?, id)
                .await
                .map_err(command_error)?
                .map(|row| row.body),
        )
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::commands::RetainedCommand>, CommandsError> {
        Ok(
            journal::command(&mut *self.db.acquire().await.map_err(command_error)?, id)
                .await
                .map_err(command_error)?
                .map(|row| orch_engine::commands::RetainedCommand {
                    machine: row.machine_id,
                    body: row.body,
                }),
        )
    }

    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        self.link.observe(command).await
    }
}

fn journaled_engine(world: &World) -> Engine {
    // No second supervisor is started: direct mount calls use the world's real link and event handlers.
    Engine::new(EngineParts {
        db: world.pool.clone(),
        clock: Arc::clone(&world.clock),
        config: common::config(),
        commands: Arc::new(JournaledCommands {
            db: world.pool.clone(),
            clock: Arc::clone(&world.clock),
            link: world.commands.as_engine(),
        }),
        access: world.access.clone(),
        live: world.live.clone(),
    })
}

async fn data(world: &World, host: &ScriptedHost) -> Id {
    let mount = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    rows::insert(
        &mut conn,
        &MountRow {
            id: mount,
            name: "dataset".into(),
            description: String::new(),
            org_id: Some(world.root),
            home_id: Some(world.root),
            owner_id: Some(world.ada),
            target: "/data/project".into(),
            read_only: true,
            enabled: true,
            default_fate: "as_is".into(),
            revision: 1,
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Mount.into(),
            id: mount,
        },
        world.ada,
        NewAccess::OnlyMe,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    rows::set_location(
        &mut conn,
        &LocationRow {
            mount_id: mount,
            machine_id: host.machine,
            host_id: Some(host.id),
            host_name: Some("node".into()),
            kind: "path".into(),
            source: "/srv/private-dataset".into(),
            storage: None,
            data_kind: None,
            created_by: world.ada,
            created_at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    mount
}

async fn declare(world: &World, host: &ScriptedHost, live: bool) {
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(jsonb_set(drivers,'{docker,mounts}','\"placed\"'::jsonb),'{docker,live_changes}',$2) WHERE id=$1")
        .bind(host.id).bind(json!({"mounts":live})).execute(world.pool.pool()).await.unwrap();
}

#[tokio::test]
async fn a_live_setting_change_refreshes_the_mount_view_without_recreating_or_resetting_index_choices()
 {
    let world = World::new().await;
    let mounts_engine = journaled_engine(&world);
    serve(&world);
    let host = ScriptedHost::on(&world, "live-node", 16_384).await;
    declare(&world, &host, true).await;
    let (environment, runtime, unit) = running(&world, &host, template(&world).await, "Main").await;
    let mount = data(&world, &host).await;
    let actual = Arc::new(Mutex::new(json!([])));
    let seen = Arc::clone(&actual);
    host.added.machine.handler.on(CommandKind::NodeMountsApply, move |command| {
        let Command::NodeMountsApply(body) = command else { panic!("wrong live command") };
        assert_eq!(body.runtime_id, runtime);
        let requested = body.mounts.as_array().unwrap();
        for mount in requested {
            assert_eq!(mount["source"], "/srv/private-dataset");
            assert_eq!(mount["mode"], "read_only");
        }
        let mounted = Json::Array(requested.iter().map(|mount| json!({"name":mount["name"],"path":mount["target"],"mode":mount["mode"]})).collect());
        *seen.lock().unwrap() = mounted.clone();
        Ok(CommandResult::NodeMountsApply(NodeMountsApplyResult { applied: true, mounts: mounted, reason: None }))
    });
    let attaches = Arc::new(Mutex::new(Vec::<Json>::new()));
    let captured = Arc::clone(&attaches);
    let observed = Arc::clone(&actual);
    unit.added.machine.handler.on(CommandKind::EnvAttach, move |command| {
        let Command::EnvAttach(body) = command else { panic!("wrong attach command") };
        captured.lock().unwrap().push(serde_json::to_value(body).unwrap());
        let mut facts = hosting::facts();
        facts["index"] = json!("live");
        facts["mounts"] = Json::Array(observed.lock().unwrap().as_array().unwrap().iter().map(|mounted| json!({"mount_id":mount,"name":mounted["name"],"state":"attached","reason":null,"path":mounted["path"],"mode":mounted["mode"],"asked":"read_only","enforced":true})).collect());
        Ok(CommandResult::EnvAttach(serde_json::from_value(facts).unwrap()))
    });
    // Retain a person's live-index choice and explicit ignores from the previous accepted attach.
    sqlx::query("INSERT INTO commands(id,machine_id,kind,body,state,result,deadline_at) VALUES($1,$2,'env.attach',$3,'succeeded',$4,$5)")
        .bind(new_id()).bind(unit.id).bind(json!({"environment_id":environment,"workspace_root":null,"ignore":["cache/**"],"index":"live","mounts":null,"variables":{},"secrets":{}})).bind(hosting::facts()).bind(world.clock.now()+chrono::Duration::minutes(2)).execute(world.pool.pool()).await.unwrap();
    world
        .set(world.topic, "env.mounts", json!({mount.to_string():true}))
        .await;
    mounts_engine
        .mount_scope_changed(world.topic)
        .await
        .unwrap();
    assert_eq!(world.commands.issued_of("node.mounts.apply"), 1);
    let mut conn = world.pool.acquire().await.unwrap();
    let view = orch_engine::mounts::views(&mut conn, environment, world.clock.now())
        .await
        .unwrap();
    assert_eq!(
        view[0].state,
        orch_api_contract::mounts::EnvMountState::Attached
    );
    assert_eq!(view[0].path.as_deref(), Some("/data/project"));
    assert!(view[0].enforced);
    assert!(rows::applied(&mut conn, runtime).await.unwrap().is_some());
    drop(conn);
    let attach = attaches.lock().unwrap()[0].clone();
    assert_eq!(attach["index"], "live");
    assert_eq!(attach["ignore"], json!(["cache/**"]));
    assert!(attach["mounts"].is_null() && attach["workspace_root"].is_null());
    mounts_engine
        .apply_environment_mounts(environment)
        .await
        .unwrap();
    assert_eq!(
        world.commands.issued_of("node.mounts.apply"),
        1,
        "the acknowledged set is already current"
    );
    world
        .set(world.topic, "env.mounts", json!({mount.to_string():false}))
        .await;
    mounts_engine
        .mount_scope_changed(world.topic)
        .await
        .unwrap();
    assert_eq!(*actual.lock().unwrap(), json!([]));
    assert_eq!(world.commands.issued_of("node.mounts.apply"), 2);
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        orch_engine::mounts::views(&mut conn, environment, world.clock.now())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        envs::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        runtime
    );
    assert_eq!(world.commands.issued_of("node.runtime.create"), 0);
    assert_eq!(world.commands.issued_of("node.runtime.stop"), 0);
    assert_eq!(world.commands.issued_of("node.runtime.start"), 0);
    world.engine().stop();
}

#[tokio::test]
async fn an_unsupported_driver_or_helper_keeps_a_visible_recreate_reason_and_the_same_running_runtime()
 {
    for live in [false, true] {
        let world = World::new().await;
        serve(&world);
        let host = ScriptedHost::on(&world, "unsupported-mounts", 16_384).await;
        declare(&world, &host, live).await;
        let (environment, runtime, unit) =
            running(&world, &host, template(&world).await, "Main").await;
        let mount = data(&world, &host).await;
        host.added
            .machine
            .handler
            .on(CommandKind::NodeMountsApply, |_| {
                Ok(CommandResult::NodeMountsApply(NodeMountsApplyResult {
                    applied: false,
                    mounts: Json::Null,
                    reason: Some("the kernel cannot attach this mount safely".into()),
                }))
            });
        world
            .set(world.topic, "env.mounts", json!({mount.to_string():true}))
            .await;
        let attaches_before = unit.count(CommandKind::EnvAttach);
        world
            .engine()
            .apply_environment_mounts(environment)
            .await
            .unwrap();
        let mut conn = world.pool.acquire().await.unwrap();
        let kept = envs::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(kept.id, runtime);
        assert_eq!(kept.state, "running");
        let detail: Option<String> =
            sqlx::query_scalar("SELECT state_detail FROM runtimes WHERE id=$1")
                .bind(runtime)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert!(
            detail
                .as_deref()
                .unwrap()
                .contains("mount changes need recreate")
        );
        assert!(!detail.as_deref().unwrap().contains("/srv/private-dataset"));
        assert!(rows::applied(&mut conn, runtime).await.unwrap().is_none());
        assert_eq!(
            world.commands.issued_of("node.mounts.apply"),
            usize::from(live)
        );
        assert_eq!(unit.count(CommandKind::EnvAttach), attaches_before);
        assert_eq!(world.commands.issued_of("node.runtime.create"), 0);
        assert_eq!(world.commands.issued_of("node.runtime.stop"), 0);
        world.engine().stop();
    }
}

#[tokio::test]
async fn reverting_a_failed_live_edit_replaces_retained_node_intent_even_when_the_old_set_is_already_attached()
 {
    let world = World::new().await;
    let mounts_engine = journaled_engine(&world);
    serve(&world);
    let host = ScriptedHost::on(&world, "reverted-live-node", 16_384).await;
    declare(&world, &host, false).await; // The helper has since become unavailable.
    let (environment, runtime, _unit) =
        running(&world, &host, template(&world).await, "Main").await;
    let at = world.clock.now();
    for (wanted, applied) in [
        (json!([]), true),
        (
            json!([{"name":"abandoned","kind":"path","source":"/srv/data","target":"/data/project","mode":"read_only","layer":null}]),
            false,
        ),
    ] {
        sqlx::query("INSERT INTO commands(id,machine_id,kind,body,state,result,deadline_at,created_at,updated_at) VALUES($1,$2,'node.mounts.apply',$3,'succeeded',$4,$5,$6,$6)")
            .bind(new_id()).bind(host.machine).bind(json!({"runtime_id":runtime,"mounts":wanted})).bind(json!({"applied":applied,"mounts":null,"reason":null})).bind(at+chrono::Duration::minutes(2)).bind(at).execute(world.pool.pool()).await.unwrap();
    }
    host.added
        .machine
        .handler
        .on(CommandKind::NodeMountsApply, move |command| {
            let Command::NodeMountsApply(body) = command else {
                panic!("wrong command")
            };
            assert_eq!(body.runtime_id, runtime);
            assert_eq!(
                body.mounts,
                json!([]),
                "the approved old set replaces the abandoned desired set"
            );
            Ok(CommandResult::NodeMountsApply(NodeMountsApplyResult {
                applied: true,
                mounts: json!([]),
                reason: None,
            }))
        });
    mounts_engine
        .apply_environment_mounts(environment)
        .await
        .unwrap();
    assert_eq!(
        world.commands.issued_of("node.mounts.apply"),
        1,
        "an unchanged attachment still cancels the failed edit's retained intent"
    );
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        rows::requested(&mut conn, runtime).await.unwrap(),
        Some(json!([]))
    );
    assert_eq!(
        envs::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        runtime
    );
    assert_eq!(world.commands.issued_of("node.runtime.stop"), 0);
    world.engine().stop();
}

#[tokio::test]
async fn a_failed_start_replay_stays_visible_on_the_same_running_runtime_and_only_its_own_node_can_clear_it()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "replay-report", 16_384).await;
    let (environment, runtime, _unit) =
        running(&world, &host, template(&world).await, "Main").await;
    let reason = "mount changes pending: the helper cannot apply this mount; recreate is needed";
    host.tell(runtime, "running", Some(reason)).await;
    common::eventually(
        "the running runtime retains its mount replay reason",
        || async {
            envs::runtime(&mut world.pool.acquire().await.unwrap(), runtime)
                .await
                .unwrap()
                .unwrap()
                .state_detail
                .as_deref()
                == Some(reason)
        },
    )
    .await;
    let foreign = ScriptedHost::on(&world, "foreign-replay-report", 16_384).await;
    foreign.tell(runtime, "running", None).await;
    common::eventually("the foreign replay report is acknowledged", || async {
        world
            .commands
            .acknowledged(foreign.machine, &orch_link_contract::streams::Stream::Node)
            >= 1
    })
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        envs::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .state,
        "running"
    );
    assert_eq!(
        envs::runtime(&mut conn, runtime)
            .await
            .unwrap()
            .unwrap()
            .state_detail
            .as_deref(),
        Some(reason)
    );
    drop(conn);
    host.tell(runtime, "running", None).await;
    common::eventually(
        "its own node clears the completed replay reason",
        || async {
            envs::runtime(&mut world.pool.acquire().await.unwrap(), runtime)
                .await
                .unwrap()
                .unwrap()
                .state_detail
                .is_none()
        },
    )
    .await;
    world.engine().stop();
}

#[tokio::test]
async fn a_completed_daemon_start_replay_reopens_the_connectors_mount_handles_and_refreshes_its_facts()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "replay-facts", 16_384).await;
    let (environment, runtime, unit) = running(&world, &host, template(&world).await, "Main").await;
    unit.added.machine.handler.on(CommandKind::EnvAttach, |command| {
        let Command::EnvAttach(body) = command else { panic!("wrong refresh command") };
        assert!(body.workspace_root.is_none() && body.mounts.is_none());
        let mut facts = hosting::facts();
        facts["mounts"] = json!([{"mount_id":Id::nil(),"name":"replayed","state":"attached","reason":null,"path":"/data/replayed","mode":"read_only","asked":"read_only","enforced":true}]);
        Ok(CommandResult::EnvAttach(serde_json::from_value(facts).unwrap()))
    });
    host.tell(runtime, "running", Some("mount changes applied"))
        .await;
    common::eventually(
        "the connector is refreshed after the completed namespace replay",
        || async {
            envs::environment(&mut world.pool.acquire().await.unwrap(), environment)
                .await
                .unwrap()
                .unwrap()
                .facts["mounts"][0]["name"]
                == "replayed"
        },
    )
    .await;
    assert_eq!(world.commands.issued_of("node.runtime.create"), 0);
    assert_eq!(world.commands.issued_of("node.runtime.stop"), 0);
    world.engine().stop();
}
