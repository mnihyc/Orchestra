//! An environment is stopped by a person or when it has been idle for as long as its setting allows, never with a turn running unless that is asked for; what is unused is removed after a notice that can be answered by keeping it; removal is a job of its own that is tried again with growing pauses; and what a host reports of a unit is applied.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use chrono::{Duration, Utc};
use common::{World, eventually, meanwhile, queued};
use hosting::{ScriptedHost, running, serve, status, template};
use orch_authz::Caller;
use orch_engine::environments::{StopError, StopReason};
use orch_link_contract::commands::{CommandError, CommandKind as K, ErrorCode};
use serde_json::json;

#[tokio::test]
async fn a_person_stops_an_environment_and_its_unit_and_volumes_are_kept() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Main").await;
    world
        .engine()
        .stop_environment(environment, false, StopReason::Person(world.ada))
        .await
        .unwrap();
    assert_eq!(
        host.kinds(),
        [K::NodeRuntimeStop],
        "the unit is stopped and nothing is removed"
    );
    assert_eq!(status(&world, environment).await.0, "stopped");
    let state: String = sqlx::query_scalar("SELECT state FROM runtimes WHERE id = $1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(state, "stopped");
    // Stopping what is stopped is the same stop.
    world
        .engine()
        .stop_environment(environment, false, StopReason::Person(world.ada))
        .await
        .unwrap();
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
}

#[tokio::test]
async fn an_environment_with_a_turn_running_is_not_stopped_unless_the_turns_are_interrupted() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Main").await;
    let thread = world.thread("work").await;
    hosting::bind(&world, thread, environment).await;
    let (start, _) = world.running_turn(thread, "go").await;
    let refused = world
        .engine()
        .stop_environment(environment, false, StopReason::Person(world.ada))
        .await
        .unwrap_err();
    assert!(matches!(refused, StopError::Busy(1)), "{refused:?}");
    assert_eq!(status(&world, environment).await.0, "running");
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
    // With the interrupt asked for, the turn is stopped first.
    let engine = world.engine();
    let stopping = tokio::spawn(async move {
        engine
            .stop_environment(environment, true, StopReason::Person(world.ada))
            .await
    });
    eventually("the harness to be asked to interrupt", || async {
        world.ran(K::TurnInterrupt) == 1
    })
    .await;
    world
        .say(
            thread,
            orch_link_contract::events::Event::TurnCompleted(
                orch_link_contract::events::TurnCompleted {
                    turn_id: start.turn_id,
                    status: orch_link_contract::events::TurnOutcome::Interrupted,
                    error: None,
                },
            ),
        )
        .await;
    stopping.await.unwrap().unwrap();
    assert_eq!(status(&world, environment).await.0, "stopped");
}

#[tokio::test]
async fn an_environment_idle_for_as_long_as_its_setting_allows_is_stopped_and_activity_starts_the_clock_over()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (idle, _, _u1) = running(&world, &host, template, "Idle").await;
    let (kept, _, _u2) = running(&world, &host, template, "Kept").await;
    // Nothing stops while the two are made ready: never, until the hour is set.
    world
        .set(world.root, "env.idle_stop_minutes", json!(0))
        .await;
    // Both were last active two hours ago; one is then kept awake.
    sqlx::query("UPDATE environments SET last_activity_at = $2 WHERE id = ANY($1)")
        .bind([idle, kept])
        .bind(Utc::now() - Duration::hours(2))
        .execute(world.pool.pool())
        .await
        .unwrap();
    assert!(
        world.engine().keep_awake(kept).await.unwrap(),
        "Keep awake counts as activity now"
    );
    world
        .set(world.root, "env.idle_stop_minutes", json!(60))
        .await;
    eventually("the idle environment to be stopped", || async {
        status(&world, idle).await.0 == "stopped"
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        status(&world, kept).await.0,
        "running",
        "the clock started over"
    );
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
}

#[tokio::test]
async fn an_idle_environment_whose_stop_the_host_has_not_answered_is_not_asked_to_stop_a_second_time()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Slow").await;
    // The host takes the stop and is slow to answer it.
    host.added.machine.handler.hold(K::NodeRuntimeStop);
    sqlx::query("UPDATE environments SET last_activity_at = $2 WHERE id = $1")
        .bind(environment)
        .bind(Utc::now() - Duration::hours(2))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(world.root, "env.idle_stop_minutes", json!(60))
        .await;
    host.wait_began(K::NodeRuntimeStop, 1).await;
    // The schedule looks again every 150 ms in this world, several times over this wait: while the host
    // has not answered, the stop is not asked for a second time.
    meanwhile(std::time::Duration::from_millis(900)).await;
    assert_eq!(host.began(K::NodeRuntimeStop), 1);
    host.added.machine.handler.release();
    eventually("the environment to be stopped", || async {
        status(&world, environment).await.0 == "stopped"
    })
    .await;
    assert_eq!(host.count(K::NodeRuntimeStop), 1);
}

#[tokio::test]
async fn an_idle_stop_that_was_decided_on_an_older_look_leaves_an_environment_that_has_been_active_since()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Busy").await;
    world
        .set(world.root, "env.idle_stop_minutes", json!(60))
        .await;
    // It was active a moment ago: whoever found it idle looked before that.
    sqlx::query("UPDATE environments SET last_activity_at = $2 WHERE id = $1")
        .bind(environment)
        .bind(Utc::now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .engine()
        .stop_environment(environment, false, StopReason::Idle)
        .await
        .unwrap();
    assert_eq!(status(&world, environment).await.0, "running");
    assert!(host.kinds().is_empty());
}

#[tokio::test]
async fn an_environment_that_is_never_to_stop_is_left_up_however_idle() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Main").await;
    world
        .set(world.root, "env.idle_stop_minutes", json!(0))
        .await;
    sqlx::query("UPDATE environments SET last_activity_at = $2 WHERE id = $1")
        .bind(environment)
        .bind(Utc::now() - Duration::days(30))
        .execute(world.pool.pool())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert_eq!(status(&world, environment).await.0, "running");
    assert!(host.kinds().is_empty());
}

#[tokio::test]
async fn removal_is_a_job_of_its_own_that_stops_and_removes_the_unit_and_keeps_its_volumes() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Main").await;
    world
        .set(world.root, "env.removed_keep_days", json!(1))
        .await;
    sqlx::query("UPDATE environments SET remove_at = now() WHERE id = $1")
        .bind(environment)
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("the environment to be removed", || async {
        let gone: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT deleted_at FROM environments WHERE id = $1")
                .bind(environment)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        gone.is_some()
    })
    .await;
    assert_eq!(host.kinds(), [K::NodeRuntimeStop, K::NodeRuntimeRemove]);
    let state: String = sqlx::query_scalar("SELECT state FROM runtimes WHERE id = $1")
        .bind(runtime)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(state, "removed");
    // The environment's other job is the record of how its unit was made, which the fixture keeps as
    // provisioning does.
    eventually("the job to end", || async {
        let state: String = sqlx::query_scalar(
            "SELECT state FROM jobs WHERE environment_id = $1 AND kind <> 'environment_provision'",
        )
        .bind(environment)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
        state == "succeeded"
    })
    .await;
    let kind: String = sqlx::query_scalar(
        "SELECT kind FROM jobs WHERE environment_id = $1 AND kind <> 'environment_provision'",
    )
    .bind(environment)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(kind, "environment_remove");
    let kept: bool = sqlx::query_scalar(
        "SELECT volumes_purge_at >= deleted_at + interval '1 day' FROM environments WHERE id = $1",
    )
    .bind(environment)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert!(
        kept,
        "the minimum positive retention day keeps the volumes instead of immediately purging them"
    );
}

#[tokio::test]
async fn a_removal_that_fails_is_tried_again_with_pauses_that_grow_and_the_reason_is_shown() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Main").await;
    for _ in 0..2 {
        host.refuse(
            K::NodeRuntimeRemove,
            CommandError::new(ErrorCode::Unsupported, "the daemon is busy"),
        );
    }
    sqlx::query("UPDATE environments SET remove_at = now() WHERE id = $1")
        .bind(environment)
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("the first removal to fail", || async {
        host.count(K::NodeRuntimeRemove) == 1
    })
    .await;
    eventually("the reason to be shown on the environment", || async {
        status(&world, environment).await.1.is_some_and(|detail| {
            detail.contains("the daemon is busy") && detail.contains("tried again")
        })
    })
    .await;
    eventually(
        "the environment to be removed after two failures",
        || async {
            let gone: Option<chrono::DateTime<Utc>> =
                sqlx::query_scalar("SELECT deleted_at FROM environments WHERE id = $1")
                    .bind(environment)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
            gone.is_some()
        },
    )
    .await;
    assert_eq!(
        host.count(K::NodeRuntimeRemove),
        3,
        "two failures, then it went"
    );
    let failed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE environment_id = $1 AND kind = 'environment_remove' AND state = 'failed'",
    )
    .bind(environment)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(failed, 2);
}

#[tokio::test]
async fn an_environment_unused_for_as_long_as_its_setting_says_is_scheduled_for_removal_after_a_notice_and_can_be_kept()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (unused, _, _u1) = running(&world, &host, template, "Unused").await;
    let (never, _, _u2) = running(&world, &host, template, "Never").await;
    world
        .set(world.root, "env.unused_remove_days", json!(3))
        .await;
    // `Never` has the setting off of its own; `Unused` has had no thread for four days.
    sqlx::query("UPDATE environments SET config = jsonb_build_object('env.unused_remove_days', 0) WHERE id = $1")
        .bind(never)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET created_at = $2, unused_since = $2 WHERE id = ANY($1)")
        .bind([unused, never])
        .bind(Utc::now() - Duration::days(4))
        .execute(world.pool.pool())
        .await
        .unwrap();
    eventually("the removal to be scheduled", || async {
        let at: Option<chrono::DateTime<Utc>> =
            sqlx::query_scalar("SELECT remove_at FROM environments WHERE id = $1")
                .bind(unused)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        at.is_some()
    })
    .await;
    let at: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT remove_at FROM environments WHERE id = $1")
            .bind(unused)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(
        at > Utc::now() + Duration::minutes(50),
        "ahead by the notice, an hour in the tests: {at}"
    );
    let none: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT remove_at FROM environments WHERE id = $1")
            .bind(never)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(none.is_none(), "its own setting says never");
    let told: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'environment.removal_scheduled' AND object_id = $1",
    )
    .bind(unused)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(told, 1, "a line in its history says it");
    // Keep it: the schedule is withdrawn and the count starts over.
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        orch_db::queries::environments::hosting::keep(&mut conn, unused, Utc::now())
            .await
            .unwrap()
    );
    drop(conn);
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    let again: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT remove_at FROM environments WHERE id = $1")
            .bind(unused)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(again.is_none(), "it was kept, and the days start from now");
}

#[tokio::test]
async fn a_removal_is_scheduled_together_with_the_line_in_its_history_or_not_at_all() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (unused, _, _unit) = running(&world, &host, template, "Unused").await;
    // The history cannot be written.
    world
        .exec(
            "CREATE FUNCTION refuse_history() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'the history is not written'; END $$",
        )
        .await;
    world
        .exec(
            "CREATE TRIGGER refuse_history BEFORE INSERT ON audit_log FOR EACH ROW
             WHEN (NEW.action = 'environment.removal_scheduled') EXECUTE FUNCTION refuse_history()",
        )
        .await;
    world
        .set(world.root, "env.unused_remove_days", json!(3))
        .await;
    sqlx::query("UPDATE environments SET created_at = $2, unused_since = $2 WHERE id = $1")
        .bind(unused)
        .bind(Utc::now() - Duration::days(4))
        .execute(world.pool.pool())
        .await
        .unwrap();
    // The schedule looks every 150 ms in this world, and tries each time.
    meanwhile(std::time::Duration::from_millis(900)).await;
    let at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT remove_at FROM environments WHERE id = $1")
            .bind(unused)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(
        at.is_none(),
        "no removal stands that its history does not say"
    );
}

#[tokio::test]
async fn a_unit_the_node_finds_gone_makes_its_environment_failed_with_the_reason() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, runtime, _unit) = running(&world, &host, template, "Main").await;
    let thread = world.thread("work").await;
    hosting::bind(&world, thread, environment).await;
    host.tell(runtime, "failed", Some("exit status 137")).await;
    eventually("the environment to fail", || async {
        status(&world, environment).await.0 == "failed"
    })
    .await;
    assert_eq!(
        status(&world, environment).await.1.as_deref(),
        Some("exit status 137")
    );
    // A thread that needs it is blocked with that reason, and keeps its message.
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("hello"))
        .await
        .unwrap();
    world.wait_blocked(thread, "environment_unavailable").await;
}

#[tokio::test]
async fn an_environment_near_its_idle_stop_warns_its_people_once_and_is_not_stopped_before_its_time()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "host-1", 16_384).await;
    let template = template(&world).await;
    let (environment, _, _unit) = running(&world, &host, template, "Quiet").await;
    world
        .set(world.root, "env.idle_stop_minutes", json!(0))
        .await;
    // Half an hour without activity, an hour allowed, a warning 45 minutes ahead: it is due now.
    sqlx::query("UPDATE environments SET last_activity_at = $2 WHERE id = $1")
        .bind(environment)
        .bind(Utc::now() - Duration::minutes(30))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(world.root, "env.idle_warning_minutes", json!(45))
        .await;
    world
        .set(world.root, "env.idle_stop_minutes", json!(60))
        .await;
    let warnings = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications WHERE kind = 'environment.idle_stop'
              AND data ->> 'environment_id' = $1::text AND (data ->> 'warning')::boolean",
        )
        .bind(environment)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
    };
    eventually("the idle warning", || async { warnings().await == 1 }).await;
    meanwhile(std::time::Duration::from_millis(600)).await;
    assert_eq!(
        warnings().await,
        1,
        "told once for this stretch without activity"
    );
    assert_eq!(
        status(&world, environment).await.0,
        "running",
        "its time has not come"
    );
    assert_eq!(host.count(K::NodeRuntimeStop), 0);
}
