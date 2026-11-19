//! Side threads that nobody used for as long as their place allows are closed by the worker, those still in use or younger are left, a run that finds another run at it does nothing, and the job is one the worker schedules.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_db::queries::threads::forking::{self, NewSide};
use orch_types::{Id, new_id};
use orch_worker_core::channels::Notifiers;
use orch_worker_core::schedule::{self, Job};
use orch_worker_core::side_threads;
use support::world::World;

/// The repeated interleavings exercise both possible orders under PostgreSQL's real row locks.
const ORDER_REPETITIONS: usize = 100;
/// Scheduling headroom for observing the intended blocker on a loaded test machine.
const LOCK_HEADROOM: std::time::Duration = std::time::Duration::from_secs(60);

async fn waits_for(world: &World, blocker: i32) {
    tokio::time::timeout(LOCK_HEADROOM, async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname = current_database() AND pg_blocking_pids(pid) @> ARRAY[$1]::integer[])")
                .bind(blocker).fetch_one(&world.test.pool).await.unwrap();
            if waiting { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
}

/// A thread in a topic of an org, and the topic's identifier.
async fn parent_thread(world: &World) -> (Id, Id) {
    let org = world.org("Acme").await;
    let project = world.node("project", org, "Web").await;
    let topic = world.node("topic", project, "Login").await;
    let parent = world.node("thread", topic, "Parent").await;
    (parent, topic)
}

/// A side thread of `parent` that was made `hours_ago` hours before the clock's now.
async fn side_made(world: &World, parent: Id, name: &str, hours_ago: i64) -> Id {
    let side = new_id();
    let mut conn = world.test.pool.acquire().await.unwrap();
    let made = forking::insert_side_thread(
        &mut conn,
        &NewSide {
            id: side,
            parent,
            name,
            owner: world.admin,
            from_turn: 0,
            at: world.now() - Duration::hours(hours_ago),
        },
    )
    .await
    .unwrap();
    assert!(made, "the parent is a thread");
    side
}

async fn closed(world: &World, thread: Id) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT deleted_at IS NOT NULL FROM nodes WHERE id = $1")
        .bind(thread)
        .fetch_one(&world.test.pool)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_side_thread_left_alone_for_a_day_is_closed_and_a_younger_one_is_not() {
    let world = World::new(Notifiers::new()).await;
    let (parent, _) = parent_thread(&world).await;
    let forgotten = side_made(&world, parent, "Forgotten", 30).await;
    let recent = side_made(&world, parent, "Recent", 3).await;
    let closed_now = side_threads::close_idle(&world.db(), &world.clock)
        .await
        .unwrap();
    assert_eq!(
        closed_now,
        Some(1),
        "the one that waited for more than a day"
    );
    assert!(closed(&world, forgotten).await);
    assert!(!closed(&world, recent).await);
    // Nothing is left to close until another day has gone by.
    assert_eq!(
        side_threads::close_idle(&world.db(), &world.clock)
            .await
            .unwrap(),
        Some(0)
    );
    world.clock.advance(Duration::hours(25));
    assert_eq!(
        side_threads::close_idle(&world.db(), &world.clock)
            .await
            .unwrap(),
        Some(1)
    );
    assert!(closed(&world, recent).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hours_a_side_thread_may_stand_idle_are_the_setting_of_its_parents_place() {
    let world = World::new(Notifiers::new()).await;
    let (parent, topic) = parent_thread(&world).await;
    let side = side_made(&world, parent, "Aside", 30).await;
    sqlx::query(
        "INSERT INTO settings (node_id, key, value) VALUES ($1, 'thread.side_idle_hours', '48')",
    )
    .bind(topic)
    .execute(&world.test.pool)
    .await
    .unwrap();
    assert_eq!(
        side_threads::close_idle(&world.db(), &world.clock)
            .await
            .unwrap(),
        Some(0),
        "thirty hours is less than the forty-eight this place allows"
    );
    assert!(!closed(&world, side).await);
    sqlx::query(
        "UPDATE settings SET value = '12' WHERE node_id = $1 AND key = 'thread.side_idle_hours'",
    )
    .bind(topic)
    .execute(&world.test.pool)
    .await
    .unwrap();
    assert_eq!(
        side_threads::close_idle(&world.db(), &world.clock)
            .await
            .unwrap(),
        Some(1)
    );
    assert!(closed(&world, side).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_side_thread_with_a_message_waiting_in_it_or_a_turn_under_way_is_not_idle() {
    let world = World::new(Notifiers::new()).await;
    let (parent, _) = parent_thread(&world).await;
    let waiting = side_made(&world, parent, "Waiting", 40).await;
    let working = side_made(&world, parent, "Working", 40).await;
    sqlx::query(
        "INSERT INTO thread_inputs (id, thread_id, author_id, source, content, delivery, state, position, client_id, seq_updated)
         VALUES ($1, $2, $3, 'user', '[]', 'queue', 'pending', 1, 'a', 1)",
    )
    .bind(new_id())
    .bind(waiting)
    .bind(world.admin)
    .execute(&world.test.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET state = 'running' WHERE node_id = $1")
        .bind(working)
        .execute(&world.test.pool)
        .await
        .unwrap();
    assert_eq!(
        side_threads::close_idle(&world.db(), &world.clock)
            .await
            .unwrap(),
        Some(0)
    );
    assert!(!closed(&world, waiting).await && !closed(&world, working).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_finds_another_run_at_the_job_does_nothing_and_the_job_is_one_the_worker_schedules()
 {
    let world = World::new(Notifiers::new()).await;
    let (parent, _) = parent_thread(&world).await;
    let forgotten = side_made(&world, parent, "Forgotten", 30).await;
    let holding = schedule::take(&world.db(), Job::SideThreads)
        .await
        .unwrap()
        .expect("the lock is free");
    assert_eq!(
        side_threads::close_idle(&world.db(), &world.clock)
            .await
            .unwrap(),
        None,
        "another run holds the lock"
    );
    assert!(!closed(&world, forgotten).await);
    drop(holding);
    assert_eq!(
        side_threads::close_idle(&world.db(), &world.clock)
            .await
            .unwrap(),
        Some(1)
    );
    assert!(Job::ALL.contains(&Job::SideThreads));
    assert_eq!(Job::SideThreads.key(), "side_threads");
    assert_eq!(Job::named("side_threads"), Some(Job::SideThreads));
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_closing_and_its_policy_change_in_both_orders_a_hundred_times_keep_the_later_decision()
{
    let world = World::new(Notifiers::new()).await;
    let (parent, topic) = parent_thread(&world).await;
    sqlx::query("INSERT INTO settings(node_id,key,value) VALUES($1,'thread.side_idle_hours','24')")
        .bind(topic)
        .execute(&world.test.pool)
        .await
        .unwrap();
    for round in 0..ORDER_REPETITIONS {
        let side = side_made(&world, parent, &format!("Aside {round}"), 30).await;
        sqlx::query(
            "UPDATE settings SET value='24' WHERE node_id=$1 AND key='thread.side_idle_hours'",
        )
        .bind(topic)
        .execute(&world.test.pool)
        .await
        .unwrap();
        let mut authority = world.test.pool.begin().await.unwrap();
        // An authority declaration no longer blocks reads. Hold the actual settings relation
        // so this policy commits before the closer can read its value.
        sqlx::query("LOCK TABLE settings IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *authority)
            .await
            .unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *authority)
            .await
            .unwrap();
        let db = world.db();
        let clock = world.clock.clone();
        let run = tokio::spawn(async move { side_threads::close_idle(&db, &clock).await.unwrap() });
        waits_for(&world, blocker).await;
        sqlx::query("UPDATE settings SET value = '48' WHERE node_id = $1 AND key = 'thread.side_idle_hours'")
            .bind(topic).execute(&mut *authority).await.unwrap();
        authority.commit().await.unwrap();
        assert_eq!(run.await.unwrap(), Some(0));
        assert!(
            !closed(&world, side).await,
            "the longer policy arrived first"
        );

        sqlx::query("UPDATE settings SET value = '12' WHERE node_id = $1 AND key = 'thread.side_idle_hours'")
            .bind(topic).execute(&world.test.pool).await.unwrap();
        let mut object = world.test.pool.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *object)
            .await
            .unwrap();
        // Hold the setting writer while a normal MVCC reader still sees twelve hours.
        // The closer's decision commits before the later forty-eight-hour policy.
        sqlx::query("SELECT node_id FROM settings WHERE node_id=$1 AND key='thread.side_idle_hours' FOR UPDATE")
            .bind(topic).execute(&mut *object).await.unwrap();
        let db = world.db();
        let change = tokio::spawn(async move {
            let mut tx = db.pool().begin().await.unwrap();
            sqlx::query("UPDATE settings SET value = '48' WHERE node_id = $1 AND key = 'thread.side_idle_hours'")
                .bind(topic).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        });
        waits_for(&world, blocker).await;
        let db = world.db();
        let clock = world.clock.clone();
        let run = tokio::spawn(async move { side_threads::close_idle(&db, &clock).await.unwrap() });
        assert_eq!(run.await.unwrap(), Some(1));
        object.rollback().await.unwrap();
        change.await.unwrap();
        assert!(
            closed(&world, side).await,
            "closing arrived first and is kept"
        );
    }
}
