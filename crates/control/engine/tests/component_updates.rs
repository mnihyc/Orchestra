//! A node drains every hosted turn and keeps new turns queued until its update fence ends.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
mod common;

use common::{World, queued, turn_ended};
use orch_authz::Caller;
use orch_db::queries::{component_updates as updates, resources::access_fixtures as fixtures};
use orch_link_contract::events::TurnOutcome;
use orch_types::Id;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_with_no_local_harness_drains_its_last_hosted_turn_and_holds_new_work() {
    let world = World::new().await;
    let first = world.thread("First hosted turn").await;
    let second = world.thread("Last hosted turn").await;
    let held = world.thread("Waits for node update").await;
    let caller = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&caller, first, queued("First"))
        .await
        .unwrap();
    world
        .engine()
        .submit_input(&caller, second, queued("Last"))
        .await
        .unwrap();
    let one = world.turn_start(first, 0).await;
    let two = world.turn_start(second, 0).await;

    let node = {
        let mut conn = world.pool.acquire().await.unwrap();
        let host = fixtures::insert_resource(
            &mut conn,
            "host",
            Some(world.ada),
            Some(world.root),
            "Physical node",
        )
        .await
        .unwrap();
        let node: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
            .bind(host)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        // These real harness turns run on a child machine, never on the node's own agent role.
        sqlx::query("UPDATE machines SET host_id=$2 WHERE id=$1")
            .bind(world.machine_id)
            .bind(host)
            .execute(&mut *conn)
            .await
            .unwrap();
        node
    };
    let mut tx = world.pool.begin().await.unwrap();
    updates::request(&mut tx, node, "0.2.0", Some(world.ada), world.clock.now())
        .await
        .unwrap();
    assert!(updates::busy(&mut tx, node).await.unwrap());
    tx.commit().await.unwrap();

    world
        .engine()
        .submit_input(&caller, held, queued("After the update"))
        .await
        .unwrap();
    world.wait_state(held, "blocked").await;
    assert!(world.turn_starts(held).is_empty());

    world
        .say(first, turn_ended(one.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(first, "idle").await;
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(
        updates::busy(&mut conn, node).await.unwrap(),
        "the last hosted turn still blocks replacement"
    );
    drop(conn);
    world
        .say(second, turn_ended(two.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(second, "idle").await;
    let mut tx = world.pool.begin().await.unwrap();
    // The turn is written under its thread's lock, in the engine's own order (the thread, then its turn): the
    // turn's terminal follow-up may still hold the thread and wait for this very turn.
    assert!(
        orch_db::queries::threads::lock(&mut tx, second)
            .await
            .unwrap()
            .is_some()
    );
    assert!(!updates::busy(&mut tx, node).await.unwrap());
    sqlx::query("UPDATE turns SET state='interrupted',error='{\"code\":\"forced\"}',native_end_confirmed=false WHERE id=$1")
        .bind(two.turn_id).execute(&mut *tx).await.unwrap();
    assert!(
        updates::busy(&mut tx, node).await.unwrap(),
        "a forced controller end is not a native idle grant"
    );
    sqlx::query("UPDATE turns SET native_end_confirmed=true WHERE id=$1")
        .bind(two.turn_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(!updates::busy(&mut tx, node).await.unwrap());
    assert!(
        updates::blocks_binding(&mut tx, world.environment, world.provider)
            .await
            .unwrap(),
        "idle alone does not release admission before reconnect"
    );
    updates::finish(&mut tx, node, "current", None, world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world
        .engine()
        .component_update_finished(node)
        .await
        .unwrap();
    let after = world.turn_start(held, 0).await;
    world
        .say(held, turn_ended(after.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(held, "idle").await;
}
