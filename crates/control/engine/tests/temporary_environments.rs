//! Temporary environments warn their owner, follow only their creating thread and remove owned data through leased jobs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
mod hosting;

use chrono::Duration;
use common::{World, eventually};
use hosting::{ScriptedHost, running, serve, template};
use orch_db::queries::environments::{retained, temporary};
use orch_link_contract::commands::CommandKind as K;
use orch_types::{Clock, Id, ManualClock};
use serde_json::json;
use std::sync::Arc;

async fn setup() -> (World, Arc<ManualClock>, ScriptedHost, Id) {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    serve(&world);
    let host = ScriptedHost::on(&world, "temporary-host", 32_768).await;
    let template = template(&world).await;
    (world, clock, host, template)
}

async fn opt_in(world: &World, id: Id, mode: &str) {
    sqlx::query("UPDATE environments SET config=$2,created_at=$3,last_activity_at=$3 WHERE id=$1")
        .bind(id)
        .bind(json!({"env.lifetime":mode,"env.idle_stop_minutes":0}))
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("a lifetime to be recorded", || async {
        temporary::read(&mut world.pool.acquire().await.unwrap(), id)
            .await
            .unwrap()
            .is_some()
    })
    .await;
}

async fn warnings(world: &World, id: Id) -> Vec<(Id, serde_json::Value)> {
    sqlx::query_as("SELECT user_id,data FROM notifications WHERE kind='environment.attention' AND data->>'reason'='temporary' AND data->>'environment_id'=$1")
        .bind(id.to_string()).fetch_all(world.pool.pool()).await.unwrap()
}

#[tokio::test]
async fn idle_cleanup_warns_only_the_owner_once_then_purges_the_exact_owned_volumes() {
    let (world, clock, host, template) = setup().await;
    let (id, _, _unit) = running(&world, &host, template, "Disposable").await;
    opt_in(&world, id, "temporary").await;
    clock.advance(Duration::hours(23));
    world.engine().kick();
    eventually("the owner's warning", || async {
        warnings(&world, id).await.len() == 1
    })
    .await;
    let told = warnings(&world, id).await;
    assert_eq!(told[0].0, world.ada);
    assert_eq!(
        told[0].1["expires_at"],
        json!(clock.now() + Duration::hours(1))
    );
    world.engine().kick();
    assert_eq!(
        host.count(K::NodeRuntimeRemove),
        0,
        "the warning is a real hour before deletion"
    );
    clock.advance(Duration::hours(1));
    world.engine().kick();
    eventually("the zero-window owned volumes to be purged", || async {
        let mut conn = world.pool.acquire().await.unwrap();
        retained::removal(&mut conn, id)
            .await
            .unwrap()
            .is_some_and(|row| row.purged_at.is_some())
    })
    .await;
    let row = retained::removal(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.removed_keep_days, Some(0));
    assert_eq!(warnings(&world, id).await.len(), 1);
    let purged: Vec<_> = host
        .calls()
        .into_iter()
        .filter(|call| call.kind == K::NodeRuntimeRemove && call.body["keep_volumes"] == false)
        .collect();
    assert!(!purged.is_empty());
    for call in purged {
        assert!(call.body.to_string().contains(&id.to_string()), "{call:?}");
    }
    assert!(
        host.calls()
            .iter()
            .any(|call| call.kind == K::NodeRuntimeRemove && call.body["keep_volumes"] == true),
        "removal keeps references until the purge lease takes them"
    );
}

#[tokio::test]
async fn only_the_creating_threads_archive_starts_the_warning_and_keep_withdraws_it() {
    let (world, clock, host, template) = setup().await;
    let (id, _, _unit) = running(&world, &host, template, "Thread experiment").await;
    opt_in(&world, id, "thread").await;
    let creator = world.thread("Creator").await;
    let other = world.thread("Other").await;
    {
        let mut conn = world.pool.acquire().await.unwrap();
        temporary::associate(&mut conn, id, creator, world.ada)
            .await
            .unwrap();
        temporary::associate(&mut conn, id, other, world.ada)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
        .bind(other)
        .bind(clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    clock.advance(Duration::hours(1));
    world.engine().kick();
    assert!(warnings(&world, id).await.is_empty());
    sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
        .bind(creator)
        .bind(clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().kick();
    eventually("archive to warn before removing", || async {
        warnings(&world, id).await.len() == 1
    })
    .await;
    let row = temporary::read(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.thread_id, Some(creator));
    assert_eq!(row.expires_at, clock.now() + Duration::hours(1));
    {
        let mut tx = world.pool.begin().await.unwrap();
        assert!(temporary::keep(&mut tx, id, clock.now()).await.unwrap());
        tx.commit().await.unwrap();
    }
    clock.advance(Duration::days(2));
    world.engine().kick();
    assert!(
        temporary::read(&mut world.pool.acquire().await.unwrap(), id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
}

#[tokio::test]
async fn an_open_export_job_defers_archive_cleanup_and_finishing_it_leaves_another_warning_hour() {
    let (world, clock, host, template) = setup().await;
    let (id, _, _unit) = running(&world, &host, template, "Export in progress").await;
    opt_in(&world, id, "thread").await;
    let creator = world.thread("Export creator").await;
    temporary::associate(
        &mut world.pool.acquire().await.unwrap(),
        id,
        creator,
        world.ada,
    )
    .await
    .unwrap();
    // A queued snapshot is enough: it owns the files before any machine command begins.
    let export = orch_types::new_id();
    sqlx::query(
        "INSERT INTO jobs(id,kind,environment_id,state,plan) VALUES($1,'snapshot',$2,'running',$3)",
    )
    .bind(export)
    .bind(id)
    .bind(json!({"runner":{"id":orch_types::new_id(),"until":clock.now()+Duration::days(7)}}))
    .execute(world.pool.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
        .bind(creator)
        .bind(clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    clock.advance(Duration::hours(40));
    world.engine().kick();
    eventually("the live export to hold the environment", || async {
        temporary::read(&mut world.pool.acquire().await.unwrap(), id)
            .await
            .unwrap()
            .is_some_and(|row| row.active && row.activity_at == clock.now())
    })
    .await;
    assert!(warnings(&world, id).await.is_empty());
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    sqlx::query("UPDATE jobs SET state='succeeded',finished_at=$2 WHERE id=$1")
        .bind(export)
        .bind(clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    // Archive is already due, but the owner still gets a fresh hour after capture completes.
    world.engine().kick();
    eventually("a fresh warning after export", || async {
        warnings(&world, id).await.len() == 1
    })
    .await;
    assert_eq!(host.count(K::NodeRuntimeRemove), 0);
    let row = temporary::read(&mut world.pool.acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.expires_at, clock.now() + Duration::hours(1));
}

#[tokio::test]
async fn a_directory_machine_detaches_then_removes_only_its_frozen_owned_data_generation() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let connector = hosting::primary(&world);
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{environment_descriptor}', $2) WHERE id=$1",
    )
    .bind(connector.id)
    .bind(json!({"lifecycle":"attach","confinement":"none"}))
    .execute(world.pool.pool())
    .await
    .unwrap();
    let id = hosting::directory(&world, connector.id, "Owned directory", "/workspace").await;
    sqlx::query("UPDATE environments SET data_generation=3 WHERE id=$1")
        .bind(id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    opt_in(&world, id, "temporary").await;
    clock.advance(Duration::hours(23));
    world.engine().kick();
    eventually("the directory owner to be warned", || async {
        warnings(&world, id).await.len() == 1
    })
    .await;
    clock.advance(Duration::hours(1));
    world.engine().kick();
    eventually("owned directory cleanup to finish", || async {
        connector.count(K::EnvDataRemove) == 1
    })
    .await;
    let removed = connector.calls_of(K::EnvDataRemove);
    assert_eq!(
        removed[0].body,
        json!({"environment_id":id,"data_generation":3})
    );
    let kinds = connector.kinds();
    let detach = kinds.iter().position(|kind| *kind == K::EnvDetach).unwrap();
    let remove = kinds
        .iter()
        .position(|kind| *kind == K::EnvDataRemove)
        .unwrap();
    assert!(detach < remove);
    assert!(
        !kinds.contains(&K::NodeRuntimeRemove),
        "a directory machine never receives a container command"
    );
}

#[tokio::test]
async fn owned_directory_cleanup_retries_busy_detach_with_a_new_id_same_body_and_original_deadline()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use orch_link_contract::commands::{CommandError, ErrorCode};
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let connector = hosting::primary(&world);
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{environment_descriptor}', $2) WHERE id=$1",
    )
    .bind(connector.id)
    .bind(json!({"lifecycle":"attach","confinement":"none"}))
    .execute(world.pool.pool())
    .await?;
    let id = hosting::directory(&world, connector.id, "Busy cleanup", "/workspace").await;
    opt_in(&world, id, "temporary").await;
    clock.advance(Duration::hours(23));
    world.engine().kick();
    eventually("cleanup warning", || async {
        warnings(&world, id).await.len() == 1
    })
    .await;
    connector.refuse_detaches(
        1,
        CommandError::new(ErrorCode::Busy, "recorded files are still publishing").retryable(),
    );
    clock.advance(Duration::hours(1));
    world.engine().kick();
    eventually("cleanup retries and removes data", || async {
        connector.count(K::EnvDataRemove) == 1
    })
    .await;
    assert_eq!(connector.count(K::EnvDetach), 2);
    // This world's command authority is ScriptedCommands plus the native journal, not SQL rows.
    let attempts: Vec<_> = world
        .commands
        .issued_commands("env.detach")
        .into_iter()
        .filter(|(_, cmd)| cmd.body["environment_id"] == id.to_string())
        .collect();
    assert_eq!(attempts.len(), 2);
    assert_ne!(attempts[0].1.id, attempts[1].1.id);
    assert_eq!(attempts[0].0, attempts[1].0);
    assert_eq!(attempts[0].1.body, attempts[1].1.body);
    assert_eq!(attempts[0].1.key, attempts[1].1.key);
    assert_eq!(attempts[0].1.deadline, attempts[1].1.deadline);
    let commands = world.commands.as_engine();
    let mut busy = 0;
    let mut succeeded = 0;
    for (_, cmd) in attempts {
        match commands.status(cmd.id).await?.and_then(|info| info.outcome) {
            Some(orch_engine::commands::Outcome::Failed(error))
                if error.code == "busy" && error.retryable =>
            {
                busy += 1
            }
            Some(orch_engine::commands::Outcome::Succeeded(_)) => succeeded += 1,
            other => return Err(format!("unexpected cleanup outcome: {other:?}").into()),
        }
    }
    assert_eq!((busy, succeeded), (1, 1));
    Ok(())
}

#[tokio::test]
async fn recovery_after_the_original_deadline_replays_original_and_busy_child_success_before_checkpoint()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use orch_api_contract::workspace::PhaseState;
    use orch_db::queries::jobs::{self, NewJob, kind};
    use orch_engine::commands::{Outcome, Outgoing};
    use orch_link::journal::{Entry, Step};
    use orch_link_contract::commands::{CommandError, EnvDetach, ErrorCode, SerialKey};
    for with_busy in [false, true] {
        let clock = Arc::new(orch_testkit::clock::manual());
        let world = World::with_clock(clock.clone()).await;
        let connector = hosting::primary(&world);
        let environment =
            hosting::directory(&world, connector.id, "Recovered cleanup", "/workspace").await;
        let job_id = orch_types::new_id();
        let original = orch_engine::ids::derived("env.detach", &[job_id.as_bytes(), b"removal"]);
        let deadline = orch_types::Time::new(
            clock.now() + Duration::seconds(i64::from(K::EnvDetach.deadline_secs())),
        );
        if with_busy {
            connector.refuse_detaches(
                1,
                CommandError::new(ErrorCode::Busy, "publication owns the store").retryable(),
            );
        }
        let command = Outgoing::new(
            original,
            connector.id,
            SerialKey::Environment(environment),
            &EnvDetach {
                environment_id: environment,
            },
        )?
        .by(deadline);
        let commands = world.commands.as_engine();
        commands.issue(command.clone()).await?;
        let first = tokio::time::timeout(common::PATIENCE, commands.wait(original)).await??;
        let successful = if with_busy {
            assert!(
                matches!(first, Outcome::Failed(error) if error.code == "busy" && error.retryable)
            );
            let child = orch_engine::ids::derived("command.busy-retry", &[original.as_bytes()]);
            commands
                .issue(Outgoing {
                    id: child,
                    ..command
                })
                .await?;
            assert!(matches!(
                tokio::time::timeout(common::PATIENCE, commands.wait(child)).await??,
                Outcome::Succeeded(_)
            ));
            child
        } else {
            assert!(matches!(first, Outcome::Succeeded(_)));
            original
        };
        let native: Entry = serde_json::from_slice(&std::fs::read(
            world
                .machine
                .data_dir()
                .join("journal")
                .join(format!("{successful}.json")),
        )?)?;
        assert!(
            matches!(native.step, Step::Done { result } if result.outcome.is_ok()),
            "success is durable before the recovered phase checkpoint"
        );

        // Reconstruct the durable crash point: the native answer is Done, while stop is still
        // Running in a claimed removal job. There is no transaction waiting on the machine.
        world.engine().stop();
        let mut progress =
            orch_engine::jobs::runner::initial_progress(&orch_engine::jobs::remove::phases());
        progress.phase = "stop".to_owned();
        for phase in &mut progress.phases {
            phase.state = if phase.name == "credential" {
                PhaseState::Done
            } else if phase.name == "stop" {
                PhaseState::Running
            } else {
                PhaseState::Pending
            };
        }
        let plan = json!({"native":{"machine":connector.id,"generation":3},"scratch":true,"temporary":true,
            "location_known":true,"runner":{"id":orch_types::new_id(),"until":deadline}});
        let progress = serde_json::to_value(progress)?;
        {
            let mut conn = world.pool.acquire().await?;
            sqlx::query(
                "UPDATE environments SET removal_phase='removing',data_generation=3 WHERE id=$1",
            )
            .bind(environment)
            .execute(&mut *conn)
            .await?;
            jobs::insert(
                &mut conn,
                &NewJob {
                    id: job_id,
                    kind: kind::ENVIRONMENT_REMOVE,
                    node_id: None,
                    environment_id: Some(environment),
                    provider_id: None,
                    plan: &plan,
                    progress: &progress,
                    created_by: Some(world.ada),
                },
                clock.now(),
            )
            .await?;
            sqlx::query("UPDATE jobs SET state='running' WHERE id=$1")
                .bind(job_id)
                .execute(&mut *conn)
                .await?;
        }
        clock.advance(Duration::seconds(
            i64::from(K::EnvDetach.deadline_secs()) + 1,
        ));
        world.restart_api().await;
        let (state, progress, error) = hosting::wait_over(&world, job_id).await;
        assert_eq!(state, "succeeded", "{progress}; {error:?}");
        assert!(
            hosting::phases(&progress)
                .iter()
                .any(|(name, state)| name == "stop" && state == "done")
        );
        let count = if with_busy { 2 } else { 1 };
        assert_eq!(
            connector.count(K::EnvDetach),
            count,
            "recovery replays, never executes another detach"
        );
        let attempts = world.commands.issued_commands("env.detach");
        assert_eq!(attempts.len(), count);
        assert!(
            attempts
                .iter()
                .all(|(machine, cmd)| *machine == connector.id
                    && cmd.deadline == deadline
                    && cmd.key == Some(environment.to_string())
                    && cmd.body == json!({"environment_id":environment}))
        );
        assert_eq!(
            connector.count(K::EnvDataRemove),
            1,
            "later cleanup phases still run once"
        );
        assert_eq!(
            world
                .scalar::<String>(
                    "SELECT removal_phase FROM environments WHERE id=$1",
                    environment
                )
                .await,
            "purged"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_cleanup_that_stays_busy_ends_with_a_visible_job_failure_without_deleting_its_data()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use orch_link_contract::commands::{CommandError, ErrorCode};
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let connector = hosting::primary(&world);
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{environment_descriptor}', $2) WHERE id=$1",
    )
    .bind(connector.id)
    .bind(json!({"lifecycle":"attach","confinement":"none"}))
    .execute(world.pool.pool())
    .await?;
    let id = hosting::directory(&world, connector.id, "Cleanup deadline", "/workspace").await;
    opt_in(&world, id, "temporary").await;
    clock.advance(Duration::hours(23));
    world.engine().kick();
    eventually("cleanup warning", || async {
        warnings(&world, id).await.len() == 1
    })
    .await;
    connector.refuse_detaches(
        usize::MAX,
        CommandError::new(ErrorCode::Busy, "recorded files are still publishing").retryable(),
    );
    clock.advance(Duration::hours(1));
    let original_deadline = orch_types::Time::new(
        clock.now() + Duration::seconds(i64::from(K::EnvDetach.deadline_secs())),
    );
    world.engine().kick();
    eventually("cleanup's first Busy refusal", || async {
        connector.count(K::EnvDetach) >= 1
    })
    .await;
    clock.advance(Duration::minutes(1));
    world.engine().kick();
    tokio::time::timeout(common::PATIENCE, async {
        loop {
            let failed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM jobs WHERE environment_id=$1 AND state='failed')",
            )
            .bind(id)
            .fetch_one(world.pool.pool())
            .await?;
            if failed {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await??;
    assert_eq!(connector.count(K::EnvDataRemove), 0);
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE environment_id=$1 AND state IN ('queued','running')",
    )
    .bind(id)
    .fetch_one(world.pool.pool())
    .await?;
    assert_eq!(pending, 0);
    let attempts = world.commands.issued_commands("env.detach");
    assert!(!attempts.is_empty());
    assert!(
        attempts
            .iter()
            .all(|(_, cmd)| cmd.deadline == original_deadline)
    );
    assert!(
        original_deadline.get() < clock.now(),
        "an unfinished cleanup obtains no new deadline"
    );
    Ok(())
}
