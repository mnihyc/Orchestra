//! Real database and job proofs of frozen windows, stopped restoration, expiry reservations and failed purge promises.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
mod hosting;
use common::World;
use orch_db::queries::{
    environments::{hosting as rows, retained},
    jobs,
};
use orch_link_contract::commands::{CommandError, CommandKind as K, ErrorCode};
use orch_types::{Id, Json, Time};
use serde_json::json;

async fn world() -> World {
    let world = World::with_clock(std::sync::Arc::new(orch_testkit::clock::manual())).await;
    hosting::serve(&world);
    world
}
async fn promise(world: &World, id: Id) -> retained::Removal {
    let mut conn = world.pool.acquire().await.unwrap();
    retained::removal(&mut conn, id).await.unwrap().unwrap()
}
async fn set(world: &World, id: Id, days: i64) {
    sqlx::query("UPDATE environments SET config=config || $2 WHERE id=$1")
        .bind(id)
        .bind(json!({"env.removed_keep_days":days}))
        .execute(world.pool.pool())
        .await
        .unwrap();
}
async fn schedule(world: &World, id: Id) -> retained::Removal {
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    assert!(
        world
            .engine()
            .schedule_environment_removal(
                &mut tx,
                id,
                Some(world.ada),
                world.clock.now(),
                world.clock.now()
            )
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    promise(world, id).await
}
async fn remove(world: &World, id: Id) -> Id {
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    let accepted = retained::removal(&mut tx, id).await.unwrap().unwrap();
    let row = match orch_engine::jobs::remove::queue(&mut tx, world.clock.now(), id, None).await {
        Ok(row) => row,
        // Acceptance/reaper can already have queued this exact removal. Join its actual job;
        // any different overlapping act still fails the fixture.
        Err(orch_engine::jobs::runner::MakeError::Running(row))
            if row.environment_id == Some(id)
                && row.kind == jobs::kind::ENVIRONMENT_REMOVE
                && row.plan["runtime"].is_null()
                && row.plan["removal_revision"] == json!(accepted.removal_revision) =>
        {
            *row
        }
        Err(error) => panic!("the requested environment removal was refused: {error:?}"),
    };
    tx.commit().await.unwrap();
    world.engine().kick();
    let (state, _, error) = hosting::wait_over(world, row.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    row.id
}
async fn claim(world: &World, id: Id, purge: bool) -> Id {
    let saved = promise(world, id).await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    rows::lock_environment(&mut tx, id).await.unwrap();
    let (by, identity) = if purge {
        (None, Json::Null)
    } else {
        let session = orch_types::new_id();
        orch_db::queries::identity::testing::insert_session(
            &mut tx,
            session,
            world.ada,
            orch_types::Hash::of(session.as_bytes()).as_bytes(),
            world.clock.now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
        (Some(world.ada), json!({"session_id":session}))
    };
    let row = match orch_engine::jobs::retained::queue(
        &mut tx,
        id,
        saved.removal_revision,
        purge,
        by,
        identity,
        world.clock.now(),
    )
    .await
    {
        Ok(row) => row,
        Err(orch_engine::jobs::runner::MakeError::Running(row))
            if purge
                && row.kind == jobs::retained::PURGE
                && row.environment_id == Some(id)
                && row.plan["removal_revision"] == json!(saved.removal_revision) =>
        {
            *row
        }
        Err(error) => panic!("the requested retained operation was refused: {error:?}"),
    };
    tx.commit().await.unwrap();
    world.engine().kick();
    row.id
}

/// Expired kept files are purged by their existing upkeep job, including when it already finished.
async fn automatic_purge(world: &World, id: Id, revision: i64) -> Id {
    common::eventually("the automatic purge is queued", || async {
        jobs::latest_of_environment(
            &mut world.pool.acquire().await.unwrap(),
            id,
            &[jobs::retained::PURGE],
        )
        .await
        .unwrap()
        .is_some_and(|job| job.plan["removal_revision"] == json!(revision))
    })
    .await;
    jobs::latest_of_environment(
        &mut world.pool.acquire().await.unwrap(),
        id,
        &[jobs::retained::PURGE],
    )
    .await
    .unwrap()
    .unwrap()
    .id
}

/// Admit the simulated expiry and its first upkeep job together, before a background reaper can claim it.
async fn expired_purge(world: &World, id: Id) -> Id {
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    rows::lock_environment(&mut tx, id).await.unwrap();
    let kept = retained::removal(&mut tx, id).await.unwrap().unwrap();
    sqlx::query("UPDATE environments SET volumes_purge_at=$2 WHERE id=$1")
        .bind(id)
        .bind(world.clock.now())
        .execute(&mut *tx)
        .await
        .unwrap();
    let job = orch_engine::jobs::retained::queue(
        &mut tx,
        id,
        kept.removal_revision,
        true,
        None,
        Json::Null,
        world.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    job.id
}

#[tokio::test]
async fn an_active_purge_cannot_be_queued_twice_before_its_first_command() {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "serialized-purge", 32_768).await;
    let template = hosting::template(&world).await;
    let (id, _, _unit) = hosting::running(&world, &host, template, "One purge").await;
    schedule(&world, id).await;
    remove(&world, id).await;
    let kept = promise(&world, id).await;
    let before = host.count(K::NodeRuntimeRemove);
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    rows::lock_environment(&mut tx, id).await.unwrap();
    sqlx::query("UPDATE environments SET volumes_purge_at=$2 WHERE id=$1")
        .bind(id)
        .bind(world.clock.now())
        .execute(&mut *tx)
        .await
        .unwrap();
    let first = orch_engine::jobs::retained::queue(
        &mut tx,
        id,
        kept.removal_revision,
        true,
        None,
        Json::Null,
        world.clock.now(),
    )
    .await
    .unwrap();
    let again = orch_engine::jobs::retained::queue(
        &mut tx,
        id,
        kept.removal_revision,
        true,
        None,
        Json::Null,
        world.clock.now(),
    )
    .await;
    match again {
        Err(orch_engine::jobs::runner::MakeError::Running(running)) => {
            assert_eq!(running.id, first.id)
        }
        other => panic!("the existing purge owns the kept files: {other:?}"),
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_purge'",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        host.count(K::NodeRuntimeRemove),
        before,
        "uncommitted work sends no command"
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn immediate_acceptance_commits_one_removal_job_and_a_retry_joins_it_without_losing_retention()
 {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "accepted-delete", 32_768).await;
    let template = hosting::template(&world).await;
    let (id, _, _unit) = hosting::running(&world, &host, template, "Accepted delete").await;
    host.added.machine.handler.hold(K::NodeRuntimeStop);
    let now = world.clock.now();
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    let origin = orch_db::audit::Origin {
        request_id: Some("accepted-delete-origin".into()),
        ..Default::default()
    };
    assert!(
        orch_db::audit::scope_origin(
            origin,
            world
                .engine()
                .schedule_removal_in(&mut tx, id, now, world.ada, Time::new(now))
        )
        .await
        .unwrap()
    );
    // Uncommitted work cannot be picked up by a reaper or another process.
    let queued = jobs::latest_of_environment(&mut tx, id, &[jobs::kind::ENVIRONMENT_REMOVE])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queued.state, "queued");
    assert_eq!(
        queued.plan["origin"]["request_id"],
        "accepted-delete-origin"
    );
    let accepted = retained::removal(&mut tx, id).await.unwrap().unwrap();
    assert_eq!(accepted.removal_phase.as_deref(), Some("removing"));
    assert!(accepted.deleted_at.is_none() && accepted.volumes_purge_at.is_none());
    tx.commit().await.unwrap();
    world.engine().kick();
    host.wait_began(K::NodeRuntimeStop, 1).await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    assert!(
        !world
            .engine()
            .schedule_removal_in(&mut tx, id, now, world.ada, Time::new(now))
            .await
            .unwrap()
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_remove'",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let repeated = retained::removal(&mut tx, id).await.unwrap().unwrap();
    assert_eq!(repeated.retained_volumes, accepted.retained_volumes);
    assert_eq!(repeated.removed_keep_days, accepted.removed_keep_days);
    assert_eq!(repeated.removal_revision, accepted.removal_revision);
    tx.commit().await.unwrap();
    host.added.machine.handler.release();
    let (state, _, error) = hosting::wait_over(&world, queued.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let kept = promise(&world, id).await;
    assert_eq!(kept.removal_phase.as_deref(), Some("kept"));
    assert_eq!(
        kept.volumes_purge_at.unwrap() - kept.deleted_at.unwrap(),
        chrono::Duration::days(7)
    );
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    assert!(
        host.calls()
            .iter()
            .any(|call| call.kind == K::NodeRuntimeRemove && call.body["keep_volumes"] == true)
    );
    world.engine().stop();
}

#[tokio::test]
async fn a_frozen_future_deadline_stays_withdrawable_and_due_discovery_is_independent_of_display_placement()
 {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "future-delete", 32_768).await;
    let template = hosting::template(&world).await;
    let (id, _, _unit) = hosting::running(&world, &host, template, "Future delete").await;
    let now = world.clock.now();
    let deadline = now + chrono::Duration::days(1);
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    assert!(
        world
            .engine()
            .schedule_removal_in(&mut tx, id, deadline, world.ada, Time::new(now))
            .await
            .unwrap()
    );
    // A second caller cannot bring a previously accepted date forward by changing its argument.
    assert!(
        !world
            .engine()
            .schedule_removal_in(&mut tx, id, now, world.ada, Time::new(now))
            .await
            .unwrap()
    );
    let accepted = retained::removal(&mut tx, id).await.unwrap().unwrap();
    assert_eq!(accepted.removal_phase.as_deref(), Some("scheduled"));
    assert!(
        jobs::latest_of_environment(&mut tx, id, &[jobs::kind::ENVIRONMENT_REMOVE])
            .await
            .unwrap()
            .is_none()
    );
    sqlx::query("UPDATE environments SET placement='connected' WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        !rows::environments_due(&mut tx, now, 100)
            .await
            .unwrap()
            .contains(&id)
    );
    assert!(
        rows::environments_due(&mut tx, deadline, 100)
            .await
            .unwrap()
            .contains(&id)
    );
    tx.rollback().await.unwrap();
    world.engine().stop();
}

#[tokio::test]
async fn archive_restoration_and_confirmed_purge_keep_then_remove_the_exact_installed_files_source()
{
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "kept-lifecycle", 32_768).await;
    let template = hosting::template(&world).await;
    let (environment, old, _unit) =
        hosting::running(&world, &host, template, "Installed programs").await;
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,recreate_preserves_files}','true') WHERE id=$1").bind(host.id).execute(world.pool.pool()).await.unwrap();
    schedule(&world, environment).await;
    let capture = remove(&world, environment).await;
    let saved = rows::runtime(&mut world.pool.acquire().await.unwrap(), old)
        .await
        .unwrap()
        .unwrap()
        .kept_image
        .unwrap();
    assert_eq!(saved["capture"], capture.to_string());
    let calls = host.calls();
    let keep = calls
        .iter()
        .position(|call| call.kind == K::NodeKeepFiles)
        .unwrap();
    let remove_position = calls
        .iter()
        .position(|call| call.kind == K::NodeRuntimeRemove)
        .unwrap();
    assert!(keep < remove_position);
    assert_eq!(
        host.count(K::NodeKeptRemove),
        0,
        "archiving retains the installed files"
    );
    let restored = claim(&world, environment, false).await;
    // The restored unit's connector comes online, as a new unit's does.
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let _restored_unit = hosting::enroll(&world, host.started().unwrap()).await;
    let (state, _, error) = hosting::wait_over(&world, restored).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let create = host
        .calls()
        .into_iter()
        .find(|call| call.kind == K::NodeRuntimeCreate)
        .unwrap();
    assert_eq!(create.body["kept_image"], saved);
    let current = rows::live_runtime(&mut world.pool.acquire().await.unwrap(), environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.kept_image, Some(saved));
    set(&world, environment, 0).await;
    let scheduled = schedule(&world, environment).await;
    remove(&world, environment).await;
    let purged = automatic_purge(&world, environment, scheduled.removal_revision).await;
    let (state, _, error) = hosting::wait_over(&world, purged).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert!(host.count(K::NodeKeptRemove) > 0);
    assert_eq!(
        promise(&world, environment).await.removal_phase.as_deref(),
        Some("purged")
    );
    world.engine().stop();
}

#[tokio::test]
async fn every_owned_role_and_the_accepted_seven_day_window_survive_later_policy_edits_and_unit_removal()
 {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-window", 32_768).await;
    let template = hosting::template(&world).await;
    let (id, runtime, _unit) = hosting::running(&world, &host, template, "Trace kept window").await;
    sqlx::query("UPDATE environments SET config=config || $2::jsonb WHERE id=$1")
        .bind(id)
        .bind(json!({"env.needs":{"cpu_millis":500,"memory_mb":1024,"disk_gb":17}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    // A private view is a real recorded owned role alongside the unit's ordinary files.
    sqlx::query("UPDATE runtimes SET volumes=volumes || jsonb_build_object('over:records',jsonb_build_object('name',$2::text,'storage',NULL)) WHERE id=$1").bind(runtime).bind(format!("orch-over-{id}-records")).execute(world.pool.pool()).await.unwrap();
    let accepted = schedule(&world, id).await;
    assert_eq!(accepted.removed_keep_days, Some(7));
    assert_eq!(accepted.retained_volumes.as_array().unwrap().len(), 4);
    set(&world, id, 0).await;
    remove(&world, id).await;
    let kept = promise(&world, id).await;
    assert_eq!(kept.removed_keep_days, Some(7));
    assert_eq!(kept.removal_phase.as_deref(), Some("kept"));
    assert_eq!(
        kept.volumes_purge_at.unwrap() - kept.deleted_at.unwrap(),
        chrono::Duration::days(7)
    );
    assert_eq!(kept.retained_volumes, accepted.retained_volumes);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    assert!(
        host.calls()
            .iter()
            .filter(|c| c.kind == K::NodeRuntimeRemove)
            .all(|c| c.body["keep_volumes"] == true)
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let host_use = retained::on_host(&mut conn, host.id).await.unwrap();
    assert_eq!((host_use.0, host_use.1, host_use.2), (1, 0, 1));
    assert_eq!(host_use.3, 17);
    let loads = orch_db::queries::resources::hosting::loads(&mut conn, &[host.id])
        .await
        .unwrap();
    let declared = orch_engine::places::declared_of(&loads);
    assert_eq!(
        (declared.cpu_millis, declared.memory_mb, declared.disk_gb),
        (0, 0, 17),
        "removed units release compute declarations while their kept files still count as disk"
    );
}

#[tokio::test]
async fn zero_and_maximum_windows_are_real_acceptance_choices_and_a_locked_ancestor_wins() {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-bounds", 32_768).await;
    let template = hosting::template(&world).await;
    for days in [0, 30] {
        let (id, _, _unit) =
            hosting::running(&world, &host, template, &format!("Trace {days} days")).await;
        set(&world, id, days).await;
        assert_eq!(
            schedule(&world, id).await.removed_keep_days,
            Some(i32::try_from(days).unwrap())
        );
    }
    let (id, _, _unit) = hosting::running(&world, &host, template, "Locked trace").await;
    set(&world, id, 0).await;
    let mut conn = world.pool.acquire().await.unwrap();
    orch_settings::write::write(
        &mut conn,
        &orch_settings::write::Write {
            actor: orch_db::audit::Actor::User(world.ada),
            target: orch_settings::Target::Node(world.root),
            key: orch_settings_contract::registry::keys::ENV_REMOVED_KEEP_DAYS,
            value: json!(30),
            locked: Some(true),
            expected_revision: None,
            may_lock: true,
            origin: &orch_db::audit::Origin::default(),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(schedule(&world, id).await.removed_keep_days, Some(30));
}

#[tokio::test]
async fn restoration_reserves_original_files_reuses_provisioning_and_survives_a_restart_stopped_once()
 {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-restore", 32_768).await;
    let template = hosting::template(&world).await;
    let made = hosting::make(&world, template, "Restored trace").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let old = host.started().unwrap();
    let _unit = hosting::enroll(&world, old).await;
    assert_eq!(hosting::wait_over(&world, made.job.id).await.0, "succeeded");
    let id = made.id;
    schedule(&world, id).await;
    remove(&world, id).await;
    let kept = promise(&world, id).await;
    let job = claim(&world, id, false).await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    rows::lock_environment(&mut tx, id).await.unwrap();
    assert!(
        orch_engine::jobs::retained::queue(
            &mut tx,
            id,
            kept.removal_revision,
            true,
            None,
            Json::Null,
            world.clock.now()
        )
        .await
        .is_err()
    );
    drop(tx);
    host.wait_for(K::NodeRuntimeStart, 2).await;
    let candidate = host.started().unwrap();
    assert_ne!(candidate, old);
    let _new_unit = hosting::enroll(&world, candidate).await;
    let (state, _, error) = hosting::wait_over(&world, job).await;
    assert_eq!(state, "succeeded", "{error:?}");
    world.restart_api().await;
    assert_eq!(hosting::status(&world, id).await.0, "stopped");
    let current = promise(&world, id).await;
    assert!(current.deleted_at.is_none() && current.volumes_purge_at.is_none());
    assert_eq!(current.removal_revision, kept.removal_revision + 1);
    let (volumes, generation, state): (Json, i32, String) =
        sqlx::query_as("SELECT volumes,generation,state FROM runtimes WHERE id=$1")
            .bind(candidate)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(generation, 2);
    assert_eq!(state, "stopped");
    let old_volumes: Json = world
        .scalar("SELECT volumes FROM runtimes WHERE id=$1", old)
        .await;
    assert_eq!(volumes, old_volumes);
    assert_eq!(
        host.count(K::NodeVolumeEnsure),
        3,
        "restore makes no new owned volume"
    );
    assert_eq!(world.ran(K::TurnStart), 0);
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='environment.restored' AND object_id=$1",
    )
    .bind(id)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(events, 1);
    let finished = jobs::job(&mut world.pool.acquire().await.unwrap(), job)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.plan["lifecycle_committed"], true);
}

#[tokio::test]
async fn restore_uses_the_acknowledged_private_layout_after_its_command_journal_has_expired() {
    use orch_engine::commands::{Outcome, Outgoing, run};
    use orch_link_contract::commands::{
        Command, CommandResult, NodeMountsApply, NodeMountsApplyResult, SerialKey,
    };
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-frozen-layout", 32_768).await;
    let template = hosting::template(&world).await;
    let made = hosting::make(&world, template, "Kept private layout").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let old = host.started().unwrap();
    let _unit = hosting::enroll(&world, old).await;
    assert_eq!(hosting::wait_over(&world, made.job.id).await.0, "succeeded");
    let layer = format!("orch-over-{}-records", made.id);
    let mounts = json!([{"name":"records","kind":"path","source":"/srv/records","target":"/data/records",
        "mode":"private","layer":{"name":layer,"storage":null}}]);
    host.added
        .machine
        .handler
        .on(K::NodeMountsApply, |command| {
            let Command::NodeMountsApply(_) = command else {
                panic!("unexpected command")
            };
            Ok(CommandResult::NodeMountsApply(NodeMountsApplyResult {
                applied: true,
                mounts: json!([{"name":"records","path":"/data/records","mode":"private"}]),
                reason: None,
            }))
        });
    let command = orch_types::new_id();
    let request = Outgoing::new(
        command,
        host.machine,
        SerialKey::Runtime(old),
        &NodeMountsApply {
            runtime_id: old,
            mounts: mounts.clone(),
        },
    )
    .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::link::insert_command(
        &mut conn,
        &orch_db::queries::link::NewCommand {
            id: command,
            machine_id: host.machine,
            kind: K::NodeMountsApply.as_str(),
            body: &request.body,
            serial_key: None,
            deadline_at: world.clock.now() + chrono::Duration::minutes(2),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let Outcome::Succeeded(result) = run(world.commands.as_engine().as_ref(), request)
        .await
        .unwrap()
    else {
        panic!("the node refused its private layout")
    };
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        orch_db::queries::link::finish_command(
            &mut conn,
            command,
            &orch_db::queries::link::Ending::Succeeded(&result),
            world.clock.now()
        )
        .await
        .unwrap()
    );
    orch_db::queries::resources::mounts::retain_layers(
        &mut conn,
        old,
        &json!({"over:records":{"name":layer,"storage":null}}),
    )
    .await
    .unwrap();
    drop(conn);
    schedule(&world, made.id).await;
    let removal = remove(&world, made.id).await;
    let row = jobs::job(&mut world.pool.acquire().await.unwrap(), removal)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.plan["retained_spec"]["mounts"], mounts);
    sqlx::query("DELETE FROM commands WHERE id=$1")
        .bind(command)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let restored = claim(&world, made.id, false).await;
    host.wait_for(K::NodeRuntimeStart, 2).await;
    let _new_unit = hosting::enroll(&world, host.started().unwrap()).await;
    let (state, _, error) = hosting::wait_over(&world, restored).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let spec = host
        .calls()
        .into_iter()
        .rev()
        .find(|call| call.kind == K::NodeRuntimeCreate)
        .unwrap()
        .body;
    assert_eq!(
        spec["mounts"], mounts,
        "the current empty mount configuration does not replace the kept layout"
    );
    assert_eq!(hosting::status(&world, made.id).await.0, "stopped");
    assert_eq!(world.ran(K::TurnStart), 0);
}

#[tokio::test]
async fn restore_never_clears_a_retained_restriction_and_requires_its_own_nodes_verified_policy() {
    use orch_api_contract::environment::{EgressMode, NetworkState, UnitNetworkMode};
    use orch_link_contract::{
        events::{Event, NodeNetwork},
        streams::Stream,
    };
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-restricted", 32_768).await;
    let template = hosting::template(&world).await;
    let made = hosting::make(&world, template, "Restricted kept trace").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let old = host.started().unwrap();
    let _old_unit = hosting::enroll(&world, old).await;
    assert_eq!(hosting::wait_over(&world, made.job.id).await.0, "succeeded");
    sqlx::query("UPDATE runtimes SET network_required='restricted' WHERE id=$1")
        .bind(old)
        .execute(world.pool.pool())
        .await
        .unwrap();
    schedule(&world, made.id).await;
    remove(&world, made.id).await;
    let kept = promise(&world, made.id).await;
    for (attempt, ready) in [false, true].into_iter().enumerate() {
        let job = claim(&world, made.id, false).await;
        host.wait_for(K::NodeRuntimeStart, attempt + 2).await;
        let candidate = host.started().unwrap();
        let create = host
            .calls()
            .into_iter()
            .rev()
            .find(|call| call.kind == K::NodeRuntimeCreate)
            .unwrap();
        assert_eq!(create.body["spec"]["network"]["egress"], "restricted");
        assert_eq!(create.body["network_policy"]["mode"], "restricted");
        let mut network = NetworkState::open(UnitNetworkMode::Bridge);
        network.mode = if ready {
            EgressMode::Restricted
        } else {
            EgressMode::Open
        };
        network.required_mode = EgressMode::Restricted;
        network.revision = create.body["network_policy"]["revision"]
            .as_str()
            .unwrap()
            .to_owned();
        network.always_revision = "verified-orch-transport".to_owned();
        host.added
            .machine
            .publish(
                &Stream::Node,
                &Event::NodeNetwork(NodeNetwork {
                    runtime_id: candidate,
                    network,
                }),
            )
            .await;
        common::eventually(
            "the restoring candidate's node report is stored",
            || async {
                world
                    .scalar::<Option<Json>>(
                        "SELECT network_state FROM runtimes WHERE id=$1",
                        candidate,
                    )
                    .await
                    .is_some()
            },
        )
        .await;
        let _unit = hosting::enroll(&world, candidate).await;
        let (state, _, error) = hosting::wait_over(&world, job).await;
        assert_eq!(
            state,
            if ready { "succeeded" } else { "failed" },
            "{error:?}"
        );
        let required: Option<String> = world
            .scalar(
                "SELECT network_required FROM runtimes WHERE id=$1",
                candidate,
            )
            .await;
        assert_eq!(required.as_deref(), Some("restricted"));
        let current = promise(&world, made.id).await;
        if ready {
            assert!(current.deleted_at.is_none());
            assert_eq!(hosting::status(&world, made.id).await.0, "stopped");
        } else {
            assert_eq!(current.retained_volumes, kept.retained_volumes);
            assert_eq!(current.volumes_purge_at, kept.volumes_purge_at);
            assert_eq!(current.removal_phase.as_deref(), Some("kept"));
        }
    }
    assert_eq!(host.count(K::NodeVolumeEnsure), 3);
    assert_eq!(world.ran(K::TurnStart), 0);
    let successes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='environment.restored'",
    )
    .bind(made.id)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(successes, 1);
}

#[tokio::test]
async fn a_sign_in_ending_after_candidate_creation_rolls_back_without_publishing_or_erasing_kept_files()
 {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-midway-revocation", 32_768).await;
    let template = hosting::template(&world).await;
    let made = hosting::make(&world, template, "Midway restore identity").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let _old_unit = hosting::enroll(&world, host.started().unwrap()).await;
    assert_eq!(hosting::wait_over(&world, made.job.id).await.0, "succeeded");
    schedule(&world, made.id).await;
    remove(&world, made.id).await;
    let kept = promise(&world, made.id).await;
    host.added.machine.handler.hold(K::NodeRuntimeStop);
    let restored = claim(&world, made.id, false).await;
    host.wait_for(K::NodeRuntimeStart, 2).await;
    let candidate = host.started().unwrap();
    let _candidate_unit = hosting::enroll(&world, candidate).await;
    host.wait_began(K::NodeRuntimeStop, 2).await;
    let row = jobs::job(&mut world.pool.acquire().await.unwrap(), restored)
        .await
        .unwrap()
        .unwrap();
    let session: Id = serde_json::from_value(row.plan["identity"]["session_id"].clone()).unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=$1")
        .bind(session)
        .bind(world.clock.now())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    host.added.machine.handler.release();
    let (state, _, error) = hosting::wait_over(&world, restored).await;
    assert_eq!(state, "failed", "{error:?}");
    let current = promise(&world, made.id).await;
    assert_eq!(current.removal_phase.as_deref(), Some("kept"));
    assert_eq!(current.retained_volumes, kept.retained_volumes);
    assert_eq!(current.volumes_purge_at, kept.volumes_purge_at);
    assert_eq!(host.count(K::NodeVolumeRemove), 0);
    assert!(
        host.calls()
            .iter()
            .filter(|call| call.kind == K::NodeRuntimeRemove)
            .all(|call| call.body["keep_volumes"] == true)
    );
    let successes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='environment.restored'",
    )
    .bind(made.id)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(successes, 0);
}

#[tokio::test]
async fn confirmed_purge_keeps_recorded_metadata_and_server_blob_pins_but_explains_missing_local_bytes()
 {
    use futures::TryStreamExt;
    use orch_blob_contract::store::{BlobStore, stream_of};
    use orch_blob_local::LocalBlobStore;
    use orch_db::queries::storage::{self, NewSnapshot};
    use orch_engine::manifest::{self as manifests, Entry, EntryKind, Header};
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-recorded-purge", 32_768).await;
    let template = hosting::template(&world).await;
    let (id, _, _unit) = hosting::running(&world, &host, template, "Recorded kept trace").await;
    let thread = world.thread("Recorded metadata").await;
    hosting::bind(&world, thread, id).await;
    let local = orch_types::new_id();
    let server = orch_types::new_id();
    let directory = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(
        LocalBlobStore::open(directory.path().join("content"))
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let content = bytes::Bytes::from_static(b"a retained manifest");
    let content_hash = orch_types::Hash::of(&content);
    let written = manifests::build(
        &Header {
            v: manifests::MANIFEST_VERSION,
            root: "/workspace".to_owned(),
            generated_at: orch_types::Time::new(world.clock.now()),
            generation: 1,
            case_sensitive: true,
        },
        vec![Entry {
            path: "proof".to_owned(),
            kind: EntryKind::File,
            size: 19,
            mtime_ns: 0,
            mode: 0o644,
            hash: Some(content_hash),
            link: None,
        }],
    )
    .unwrap();
    let manifest = written.hash;
    let manifest_size = i64::try_from(written.bytes.len()).unwrap();
    store
        .put(&content_hash, Some(19), stream_of(vec![content.clone()]))
        .await
        .unwrap();
    store
        .put(
            &manifest,
            Some(manifest_size),
            stream_of(vec![bytes::Bytes::from(written.bytes)]),
        )
        .await
        .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    storage::register_blob(&mut conn, &content_hash, 19, world.clock.now())
        .await
        .unwrap();
    storage::register_blob(&mut conn, &manifest, manifest_size, world.clock.now())
        .await
        .unwrap();
    for snapshot in [local, server] {
        storage::begin_snapshot(
            &mut conn,
            &NewSnapshot {
                id: snapshot,
                thread_id: Some(thread),
                environment_id: id,
                env_generation: 1,
                label: "Retained state",
                selection: &json!({}),
                created_by: Some(world.ada),
                at: world.clock.now(),
            },
        )
        .await
        .unwrap();
        sqlx::query("UPDATE snapshots SET state='complete',storage=$2,kind=$3,tree_ref=$4,manifest_blob=$5,total_bytes=19,file_count=1,completed_at=$6 WHERE id=$1")
            .bind(snapshot).bind(if snapshot == local {"environment"} else {"blobs"})
            .bind(if snapshot == local {"turn"} else {"manual"})
            .bind((snapshot == local).then_some("retained-tree"))
            .bind(manifest.as_bytes().as_slice()).bind(world.clock.now()).execute(&mut *conn).await.unwrap();
        storage::pin_snapshot(&mut conn, snapshot, &[manifest])
            .await
            .unwrap();
        if snapshot == server {
            storage::pin_snapshot(&mut conn, snapshot, &[content_hash])
                .await
                .unwrap();
        }
    }
    drop(conn);
    // A live thread is a current binding, which refuses removal; the deleted thread's link and its
    // recorded snapshots stay for inspection.
    sqlx::query("UPDATE nodes SET deleted_at=$2 WHERE id=$1")
        .bind(thread)
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    schedule(&world, id).await;
    remove(&world, id).await;
    let job = expired_purge(&world, id).await;
    assert_eq!(hosting::wait_over(&world, job).await.0, "succeeded");
    let mut conn = world.pool.acquire().await.unwrap();
    let local = storage::snapshot(&mut conn, local).await.unwrap().unwrap();
    assert_eq!(local.state, "unavailable");
    assert_eq!(
        local.unavailable_reason.as_deref(),
        Some("environment_purged")
    );
    assert_eq!(local.tree_ref.as_deref(), Some("retained-tree"));
    assert_eq!(local.manifest_blob, Some(manifest));
    assert_eq!((local.file_count, local.total_bytes), (1, 19));
    assert_eq!(
        storage::snapshot(&mut conn, server)
            .await
            .unwrap()
            .unwrap()
            .state,
        "complete"
    );
    let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(manifest.as_bytes().as_slice())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(pins, 2);
    let content_pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(content_hash.as_bytes().as_slice())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(content_pins, 1);
    let returned = store
        .get(&content_hash, None)
        .await
        .unwrap()
        .body
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let mut actual = Vec::new();
    for part in returned {
        actual.extend_from_slice(&part);
    }
    assert_eq!(
        actual.as_slice(),
        content.as_ref(),
        "the independent server snapshot still has its actual bytes"
    );
}

#[tokio::test]
async fn a_failed_purge_keeps_its_frozen_references_deadline_and_retry_phase_and_cannot_be_restored()
 {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-purge", 32_768).await;
    let template = hosting::template(&world).await;
    let (id, _, _unit) = hosting::running(&world, &host, template, "Failed purge trace").await;
    schedule(&world, id).await;
    remove(&world, id).await;
    let kept = promise(&world, id).await;
    host.refuse(
        K::NodeRuntimeRemove,
        CommandError::new(ErrorCode::Internal, "the retained host lost its disk"),
    );
    let job = expired_purge(&world, id).await;
    let (state, _, _) = hosting::wait_over(&world, job).await;
    assert_eq!(state, "failed");
    let failed = promise(&world, id).await;
    assert_eq!(failed.removal_phase.as_deref(), Some("purging"));
    assert_eq!(failed.retained_volumes, kept.retained_volumes);
    assert!(failed.purged_at.is_none());
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    rows::lock_environment(&mut tx, id).await.unwrap();
    assert!(
        orch_engine::jobs::retained::queue(
            &mut tx,
            id,
            failed.removal_revision,
            false,
            None,
            Json::Null,
            world.clock.now()
        )
        .await
        .is_err()
    );
    drop(tx);
    hosting::await_event(
        "current home admin receives the retained failure",
        std::time::Duration::from_secs(60),
        || async {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications WHERE kind='environment.attention'",
            )
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
                > 0
        },
    )
    .await;
    let new_admin = world.person("new_retained_admin").await;
    world.give(new_admin, "admin").await;
    host.refuse(
        K::NodeRuntimeRemove,
        CommandError::new(ErrorCode::Internal, "the retained host lost its disk"),
    );
    let retry = claim(&world, id, true).await;
    assert_eq!(hosting::wait_over(&world, retry).await.0, "failed");
    hosting::await_event("the same unresolved failure reaches the new current admin",std::time::Duration::from_secs(60),||async {
        sqlx::query_scalar::<_,i64>("SELECT count(*) FROM notifications WHERE kind='environment.attention' AND user_id=$1").bind(new_admin).fetch_one(world.pool.pool()).await.unwrap()==1
    }).await;
    let original_notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE kind='environment.attention' AND user_id=$1",
    )
    .bind(world.ada)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        original_notices, 1,
        "one unresolved lifecycle failure does not spam its existing recipient"
    );
}

#[tokio::test]
async fn the_default_window_fits_a_shorter_system_window_and_an_incapable_driver_refuses_before_effects()
 {
    let world = world().await;
    world.set_config(|config| config.soft_delete = std::time::Duration::from_secs(3 * 86_400));
    world.restart_api().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "trace-short-window", 32_768).await;
    let template = hosting::template(&world).await;
    let (id, _, _unit) = hosting::running(&world, &host, template, "Short system window").await;
    assert_eq!(schedule(&world, id).await.removed_keep_days, Some(3));
    let (unsupported, _, _unit) =
        hosting::running(&world, &host, template, "Unsupported retained files").await;
    sqlx::query(
        "UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,volumes}','false') WHERE id=$1",
    )
    .bind(host.id)
    .execute(world.pool.pool())
    .await
    .unwrap();
    let before = host.count(K::NodeRuntimeRemove);
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    assert!(
        world
            .engine()
            .schedule_environment_removal(
                &mut tx,
                unsupported,
                Some(world.ada),
                world.clock.now(),
                world.clock.now()
            )
            .await
            .is_err()
    );
    drop(tx);
    assert_eq!(host.count(K::NodeRuntimeRemove), before);
    assert!(promise(&world, unsupported).await.removal_phase.is_none());
    let remove_at: Option<chrono::DateTime<chrono::Utc>> = world
        .scalar(
            "SELECT remove_at FROM environments WHERE id=$1",
            unsupported,
        )
        .await;
    assert!(remove_at.is_none());
}

#[tokio::test]
async fn early_purge_rechecks_a_requester_who_lost_home_admin_and_keeps_the_unchanged_promise() {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-revoked-admin", 32_768).await;
    let template = hosting::template(&world).await;
    let (id, _, _unit) = hosting::running(&world, &host, template, "Revoked purge authority").await;
    schedule(&world, id).await;
    remove(&world, id).await;
    let saved = promise(&world, id).await;
    let person = world.person("former_admin").await;
    let role = world.give(person, "admin").await;
    let session = orch_types::new_id();
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    orch_db::queries::identity::testing::insert_session(
        &mut tx,
        session,
        person,
        &[9; 32],
        world.clock.now() + chrono::Duration::days(1),
    )
    .await
    .unwrap();
    // An early purge is an admin's act, asked in the session's As admin mode.
    let principal = orch_authz::Principal::load(
        &mut tx,
        orch_authz::Caller::session(person).with_admin_mode(true),
        orch_types::Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    assert!(
        orch_engine::jobs::retained::authority(&mut tx, &principal, id)
            .await
            .unwrap()
            .1
    );
    let job = orch_db::audit::scope_origin(
        orch_db::audit::Origin {
            as_admin: Some(person),
            ..Default::default()
        },
        orch_engine::jobs::retained::queue(
            &mut tx,
            id,
            saved.removal_revision,
            true,
            Some(person),
            json!({"session_id":session}),
            world.clock.now(),
        ),
    )
    .await
    .unwrap();
    orch_db::queries::access::testing::take_role_away(&mut tx, role, world.ada, world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let before = host.count(K::NodeRuntimeRemove);
    world.engine().kick();
    let (state, _, error) = hosting::wait_over(&world, job.id).await;
    assert_eq!(state, "failed", "{error:?}");
    assert_eq!(host.count(K::NodeRuntimeRemove), before);
    let after = promise(&world, id).await;
    assert_eq!(after.removal_phase.as_deref(), Some("kept"));
    assert_eq!(after.volumes_purge_at, saved.volumes_purge_at);
    assert_eq!(after.retained_volumes, saved.retained_volumes);
    let events: i64 = world
        .count(
            "SELECT count(*) FROM audit_log WHERE action='environment.purged' AND object_id=$1",
            id,
        )
        .await;
    assert_eq!(events, 0);
}

#[tokio::test]
async fn restore_rechecks_the_original_sign_in_and_home_authority_before_creating_any_unit() {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-revoked-restore", 32_768).await;
    let template = hosting::template(&world).await;
    let (id, _, _unit) =
        hosting::running(&world, &host, template, "Revoked restore authority").await;
    schedule(&world, id).await;
    remove(&world, id).await;
    let kept = promise(&world, id).await;
    let person = world.person("former_restore_admin").await;
    for revoke_role in [true, false] {
        let role = world.give(person, "admin").await;
        let session = orch_types::new_id();
        let mut tx = world.pool.begin().await.unwrap();
        orch_db::queries::transaction_guard::lock_authority(&mut tx)
            .await
            .unwrap();
        rows::lock_environment(&mut tx, id).await.unwrap();
        orch_db::queries::identity::testing::insert_session(
            &mut tx,
            session,
            person,
            orch_types::Hash::of(session.as_bytes()).as_bytes(),
            world.clock.now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();
        let job = orch_engine::jobs::retained::queue(
            &mut tx,
            id,
            kept.removal_revision,
            false,
            Some(person),
            json!({"session_id":session}),
            world.clock.now(),
        )
        .await
        .unwrap();
        if revoke_role {
            orch_db::queries::access::testing::take_role_away(
                &mut tx,
                role,
                world.ada,
                world.clock.now(),
            )
            .await
            .unwrap();
        } else {
            sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=$1")
                .bind(session)
                .bind(world.clock.now())
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
        let before = host.count(K::NodeRuntimeCreate);
        world.engine().kick();
        let (state, _, error) = hosting::wait_over(&world, job.id).await;
        assert_eq!(state, "failed", "{error:?}");
        assert_eq!(host.count(K::NodeRuntimeCreate), before);
        let current = promise(&world, id).await;
        assert_eq!(current.removal_phase.as_deref(), Some("kept"));
        assert_eq!(current.retained_volumes, kept.retained_volumes);
        assert_eq!(current.volumes_purge_at, kept.volumes_purge_at);
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='environment.restored'",
        )
        .bind(id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
        assert_eq!(events, 0);
    }
}

#[tokio::test]
async fn restore_and_early_purge_honor_the_tokens_current_scope_subtree_and_org_refusal() {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-token-boundary", 32_768).await;
    let template = hosting::template(&world).await;
    let made = hosting::make(&world, template, "Narrowed kept token").await;
    host.wait_for(K::NodeRuntimeStart, 1).await;
    let _unit = hosting::enroll(&world, host.started().unwrap()).await;
    assert_eq!(hosting::wait_over(&world, made.job.id).await.0, "succeeded");
    schedule(&world, made.id).await;
    remove(&world, made.id).await;
    let kept = promise(&world, made.id).await;
    let outside = world.topic("Outside the token's kept subtree").await;
    for (number, (purge, scope, node, refuse)) in [
        (false, "files.view", None, false),
        (true, "files.view", None, false),
        (false, "environment.manage", Some(outside), false),
        (true, "environment.manage", Some(outside), false),
        (false, "environment.manage", Some(world.root), true),
        (true, "environment.manage", Some(world.root), true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut tx = world.pool.begin().await.unwrap();
        orch_db::queries::transaction_guard::lock_authority(&mut tx)
            .await
            .unwrap();
        rows::lock_environment(&mut tx, made.id).await.unwrap();
        let token = orch_db::queries::identity::testing::insert_api_token(
            &mut tx,
            world.ada,
            &[u8::try_from(number).unwrap() + 40; 32],
        )
        .await
        .unwrap();
        sqlx::query("UPDATE api_tokens SET scopes=$2,node_id=$3 WHERE id=$1")
            .bind(token)
            .bind(vec![scope.to_owned()])
            .bind(node)
            .execute(&mut *tx)
            .await
            .unwrap();
        if refuse {
            orch_db::queries::tokens::refuse(
                &mut tx,
                token,
                world.root,
                world.ada,
                world.clock.now(),
            )
            .await
            .unwrap();
        }
        let row = orch_engine::jobs::retained::queue(
            &mut tx,
            made.id,
            kept.removal_revision,
            purge,
            Some(world.ada),
            json!({"token_id":token}),
            world.clock.now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let creates = host.count(K::NodeRuntimeCreate);
        let removes = host.count(K::NodeRuntimeRemove);
        world.engine().kick();
        let (state, _, error) = hosting::wait_over(&world, row.id).await;
        assert_eq!(state, "failed", "{error:?}");
        assert_eq!(host.count(K::NodeRuntimeCreate), creates);
        assert_eq!(host.count(K::NodeRuntimeRemove), removes);
        let current = promise(&world, made.id).await;
        assert_eq!(current.removal_phase.as_deref(), Some("kept"));
        assert_eq!(current.retained_volumes, kept.retained_volumes);
        assert_eq!(current.volumes_purge_at, kept.volumes_purge_at);
    }
}

#[tokio::test]
async fn a_connected_registration_returns_on_its_original_directory_only_under_current_machine_policy()
 {
    use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
    use orch_authz::ownership::{self, NewAccess};
    use orch_db::queries::environments::routes::{NewConnected, insert_connected};
    use orch_link_contract::policy::LocalPolicy;
    let world = world().await;
    let connector = hosting::connector(&world, "trace-personal", &["environment"]).await;
    let id = orch_types::new_id();
    let mut policy = LocalPolicy {
        environment_roots: vec!["/kept-work".to_owned()],
        environment_after_up: true,
        ..LocalPolicy::default()
    };
    let descriptor = serde_json::to_value(
        orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
            "1.0.0",
            orch_driver_contract::descriptor::EnvironmentDataStorage {
                made_by_orch: None,
                existing_directory: true,
                reason: None,
            },
        ),
    )
    .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    // The person's machine declares that it attaches directories, as a connector does in its report; what it
    // attaches is what its policy allows.
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb,'environment_descriptor',$3::jsonb) WHERE id=$1")
        .bind(connector.id)
        .bind(serde_json::to_value(&policy).unwrap())
        .bind(&descriptor)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        insert_connected(
            &mut tx,
            &NewConnected {
                id,
                name: "Kept personal directory",
                owner_id: world.ada,
                machine_id: connector.id,
                home_node_id: world.topic,
                workspace_root: "/kept-work/project",
                remote_write: Some(false),
                terminal: Some(false)
            },
            world.clock.now()
        )
        .await
        .unwrap()
    );
    ownership::start_resource(
        &mut tx,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id,
        },
        world.ada,
        NewAccess::OnlyMe,
        orch_types::Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE environments SET generation=1,config=$2 WHERE id=$1")
        .bind(id)
        .bind(
            json!({"env.after_up":{"10-check-restored":{"command":["verify-restored-directory"]}}}),
        )
        .execute(&mut *tx)
        .await
        .unwrap();
    world
        .engine()
        .schedule_environment_removal(
            &mut tx,
            id,
            Some(world.ada),
            world.clock.now(),
            world.clock.now(),
        )
        .await
        .unwrap();
    let frozen = retained::removal(&mut tx, id).await.unwrap().unwrap();
    assert!(
        retained::begin_remove(&mut tx, id, frozen.removal_revision)
            .await
            .unwrap()
    );
    assert!(
        retained::keep_removed(&mut tx, id, frozen.removal_revision, world.clock.now())
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let kept = promise(&world, id).await;
    policy.environment_roots.clear();
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb,'environment_descriptor',$3::jsonb) WHERE id=$1")
        .bind(connector.id)
        .bind(serde_json::to_value(&policy).unwrap())
        .bind(&descriptor)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let refused = claim(&world, id, false).await;
    assert_eq!(hosting::wait_over(&world, refused).await.0, "failed");
    assert_eq!(connector.count(K::EnvAttach), 0);
    assert_eq!(connector.count(K::UnitAfterUp), 0);
    assert_eq!(
        promise(&world, id).await.volumes_purge_at,
        kept.volumes_purge_at
    );
    policy.environment_roots.push("/kept-work".to_owned());
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('policy',$2::jsonb,'environment_descriptor',$3::jsonb) WHERE id=$1")
        .bind(connector.id)
        .bind(serde_json::to_value(&policy).unwrap())
        .bind(&descriptor)
        .execute(world.pool.pool())
        .await
        .unwrap();
    connector.hold(K::EnvAttach);
    let stale = claim(&world, id, false).await;
    common::eventually("the kept registration's attachment began", || {
        let began = connector.began(K::EnvAttach) == 1;
        async move { began }
    })
    .await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    let session = orch_db::queries::resources::machine(&mut tx, connector.id)
        .await
        .unwrap()
        .unwrap()
        .last_seen_at
        .unwrap();
    orch_db::queries::resources::set_presence(
        &mut tx,
        connector.id,
        true,
        None,
        session + chrono::Duration::seconds(1),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    connector.let_go();
    let (state, _, error) = hosting::wait_over(&world, stale).await;
    assert_eq!(state, "failed", "{error:?}");
    let unchanged = promise(&world, id).await;
    assert_eq!(unchanged.removal_phase.as_deref(), Some("kept"));
    assert_eq!(unchanged.retained_volumes, kept.retained_volumes);
    assert_eq!(unchanged.volumes_purge_at, kept.volumes_purge_at);
    assert_eq!(
        connector.began(K::EnvDetach),
        1,
        "an uncommitted registration is detached without deleting its files"
    );
    let successes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='environment.restored'",
    )
    .bind(id)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        successes, 0,
        "an answer from an earlier machine session cannot publish restored files"
    );
    assert_eq!(
        connector.count(K::UnitAfterUp),
        0,
        "an unverified attachment runs no after-start command"
    );
    connector.hold(K::EnvAttach);
    let restored = claim(&world, id, false).await;
    common::eventually("the fresh registration's attachment began", || {
        let began = connector.began(K::EnvAttach) == 2;
        async move { began }
    })
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let before = orch_db::queries::resources::machine(&mut conn, connector.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        orch_db::queries::resources::touch_seen(
            &mut conn,
            &[connector.id],
            before.last_seen_at.unwrap() + chrono::Duration::seconds(30),
            0
        )
        .await
        .unwrap(),
        1
    );
    let after = orch_db::queries::resources::machine(&mut conn, connector.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.session_marker(),
        before.session_marker(),
        "heartbeats cannot invalidate an in-flight verified directory"
    );
    drop(conn);
    connector.let_go();
    let (state, _, error) = hosting::wait_over(&world, restored).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let commands = connector.calls_of(K::UnitAfterUp);
    assert_eq!(
        commands.len(),
        1,
        "the verified directory runs its commands once"
    );
    assert_eq!(commands[0].body["environment_id"], json!(id));
    assert_eq!(commands[0].body["name"], "10-check-restored");
    assert_eq!(
        commands[0].body["run"]["command"],
        json!(["verify-restored-directory"])
    );
    let registration = rows::environment(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert!(registration.deleted_at.is_none());
    assert_eq!(registration.workspace_root, "/kept-work/project");
    assert_eq!(registration.machine_id, Some(connector.id));
    assert_eq!(registration.status, "stopped");
    assert_eq!(
        registration.generation, 1,
        "the unchanged connector generation remains truthful"
    );
    let site = orch_db::queries::browsers::site(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        site.status, "running",
        "a verified directory can accept an explicitly requested screen without creating a managed unit"
    );
    let attached = rows::connected(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        attached.attached(),
        "Files keeps the same verified attachment after a heartbeat"
    );
    assert_eq!(connector.count(K::EnvAttach), 2);
    assert!(
        connector
            .calls_of(K::EnvAttach)
            .iter()
            .all(|call| call.body["workspace_root"] == "/kept-work/project")
    );
    assert_eq!(connector.count(K::ExecStart), 0);
    assert_eq!(connector.count(K::FsMkdir), 0);
    assert_eq!(world.ran(K::TurnStart), 0);
    let managed: i64 = sqlx::query_scalar("SELECT count(*) FROM runtimes WHERE environment_id=$1")
        .bind(id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        managed, 0,
        "restoring a registration creates no managed unit"
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let session_id: orch_types::Id =
        sqlx::query_scalar("SELECT current_session_id FROM machines WHERE id=$1")
            .bind(connector.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let current = orch_db::queries::resources::machine(&mut conn, connector.id)
        .await
        .unwrap()
        .unwrap();
    let at = current.last_seen_at.unwrap();
    orch_db::queries::resources::set_session_presence(
        &mut conn,
        connector.id,
        true,
        None,
        Some(session_id),
        at,
    )
    .await
    .unwrap();
    let repeated = orch_db::queries::resources::machine(&mut conn, connector.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        repeated.session_marker(),
        current.session_marker(),
        "a duplicate gateway report keeps the attachment"
    );
    orch_db::queries::resources::set_session_presence(
        &mut conn,
        connector.id,
        true,
        None,
        Some(orch_types::new_id()),
        at,
    )
    .await
    .unwrap();
    let first = orch_db::queries::resources::machine(&mut conn, connector.id)
        .await
        .unwrap()
        .unwrap()
        .session_marker();
    orch_db::queries::resources::set_session_presence(
        &mut conn,
        connector.id,
        true,
        None,
        Some(orch_types::new_id()),
        at,
    )
    .await
    .unwrap();
    let second = orch_db::queries::resources::machine(&mut conn, connector.id)
        .await
        .unwrap()
        .unwrap()
        .session_marker();
    assert!(
        second > first,
        "two reconnects at the same held clock have different markers"
    );
    assert!(
        !rows::connected(&mut conn, id)
            .await
            .unwrap()
            .unwrap()
            .attached(),
        "a fresh session needs its own directory verification"
    );
}

#[tokio::test]
async fn an_irreversible_early_purge_retry_keeps_the_original_person_request_origin_and_actual_purge_day()
 {
    let world = world().await;
    let host = hosting::ScriptedHost::on(&world, "trace-requested-purge", 32_768).await;
    let template = hosting::template(&world).await;
    let (id, _, _unit) = hosting::running(&world, &host, template, "Requested purge retry").await;
    schedule(&world, id).await;
    remove(&world, id).await;
    let saved = promise(&world, id).await;
    let session = orch_types::new_id();
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    orch_db::queries::identity::testing::insert_session(
        &mut tx,
        session,
        world.ada,
        &[7; 32],
        world.clock.now() + chrono::Duration::days(1),
    )
    .await
    .unwrap();
    // An early purge is an admin's act, asked in the session's As admin mode.
    let origin = orch_db::audit::Origin {
        as_admin: Some(world.ada),
        ip: Some("203.0.113.8".parse().unwrap()),
        user_agent: Some("trace-admin".to_owned()),
        request_id: Some("requested-purge".to_owned()),
    };
    let job = orch_db::audit::scope_origin(
        origin,
        orch_engine::jobs::retained::queue(
            &mut tx,
            id,
            saved.removal_revision,
            true,
            Some(world.ada),
            json!({"session_id":session}),
            world.clock.now(),
        ),
    )
    .await
    .unwrap();
    host.refuse(
        K::NodeRuntimeRemove,
        CommandError::new(ErrorCode::Internal, "the disk is temporarily absent"),
    );
    tx.commit().await.unwrap();
    world.engine().kick();
    assert_eq!(hosting::wait_over(&world, job.id).await.0, "failed");
    assert_eq!(
        promise(&world, id).await.removal_phase.as_deref(),
        Some("purging")
    );
    world.restart_api().await;
    hosting::serve(&world);
    let retry = claim(&world, id, true).await;
    let (state, _, error) = hosting::wait_over(&world, retry).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let (person,request,details):(Id,String,Json)=sqlx::query_as("SELECT actor_id,request_id,details FROM audit_log WHERE action='environment.purged' AND object_id=$1").bind(id).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(person, world.ada);
    assert_eq!(request, "requested-purge");
    assert_eq!(details["reason"], "request");
    let after = promise(&world, id).await;
    assert_eq!(after.purged_at, Some(world.clock.now()));
    assert_eq!(after.removal_phase.as_deref(), Some("purged"));
    assert_eq!(after.retained_volumes, json!([]));
}
