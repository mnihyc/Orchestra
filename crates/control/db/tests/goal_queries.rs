//! Goal lookups and fair sweeps stay bounded as completed goals accumulate or active goals wait.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::Utc;
use orch_db::Conn;
use orch_db::queries::{
    goals, hierarchy::testing::insert_node, identity::testing::insert_user, threads::insert_thread,
};
use orch_testkit::database::TestDb;
use orch_types::{Id, new_id};

async fn threads(conn: &mut Conn) -> [Id; 3] {
    let (ada, acme, project, topic) = (new_id(), new_id(), new_id(), new_id());
    insert_user(conn, ada, "ada").await.unwrap();
    insert_node(conn, acme, "org", None, "Acme", ada)
        .await
        .unwrap();
    insert_node(conn, project, "project", Some(acme), "Web", ada)
        .await
        .unwrap();
    insert_node(conn, topic, "topic", Some(project), "Work", ada)
        .await
        .unwrap();
    let mut threads = [new_id(), new_id(), new_id()];
    threads.sort();
    for thread in threads {
        assert!(
            insert_thread(conn, thread, topic, "Tests", ada, Utc::now())
                .await
                .unwrap()
        );
    }
    threads
}

#[tokio::test]
async fn a_goal_sweep_advances_past_waiting_threads_one_candidate_at_a_time_and_wraps() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let ids = threads(&mut conn).await;
    sqlx::query("INSERT INTO thread_goals (id, thread_id, objective, condition, set_at, status, active_since)
                 SELECT gen_random_uuid(), node_id, 'Work', 'Done', now(), 'active', now() FROM threads")
        .execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE threads SET state = CASE WHEN node_id = $1 THEN 'idle' ELSE 'running' END")
        .bind(ids[2])
        .execute(&mut *conn)
        .await
        .unwrap();
    let now = Utc::now();
    let first = goals::threads_with_goal_work(&mut conn, now, None, 1)
        .await
        .unwrap();
    assert_eq!(first, (vec![], Some(ids[0])));
    let second = goals::threads_with_goal_work(&mut conn, now, first.1, 1)
        .await
        .unwrap();
    assert_eq!(second, (vec![], Some(ids[1])));
    let third = goals::threads_with_goal_work(&mut conn, now, second.1, 1)
        .await
        .unwrap();
    assert_eq!(third, (vec![ids[2]], Some(ids[2])));
    let end = goals::threads_with_goal_work(&mut conn, now, third.1, 1)
        .await
        .unwrap();
    assert_eq!(end, (vec![], None));
    assert_eq!(
        goals::threads_with_goal_work(&mut conn, now, end.1, 1)
            .await
            .unwrap(),
        first
    );
}

#[tokio::test]
async fn the_current_goal_uses_the_live_row_or_one_ended_row_without_sorting_its_history() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let requested = threads(&mut conn).await;
    let thread = requested[0];
    // Many completed and cleared goals on one thread, including equal timestamps.
    sqlx::query("INSERT INTO thread_goals (id, thread_id, objective, condition, set_at, status)
                 SELECT gen_random_uuid(), $1, 'Work', 'Done', now(),
                   CASE WHEN n % 2 = 0 THEN 'met' ELSE 'cleared' END FROM generate_series(1, 100000) n")
        .bind(thread).execute(&mut *conn).await.unwrap();
    // The newest ended goal is visible; older cleared goals do not prevent it being shown.
    sqlx::query("UPDATE thread_goals SET status='met' WHERE id=(SELECT id FROM thread_goals WHERE thread_id=$1 ORDER BY set_at DESC, id DESC LIMIT 1)")
        .bind(thread).execute(&mut *conn).await.unwrap();
    let live = new_id();
    sqlx::query("INSERT INTO thread_goals (id, thread_id, objective, condition, set_at, status, active_since)
                 VALUES ($1, $2, 'Work', 'Done', now() - interval '1 day', 'active', now())")
        .bind(live).bind(thread).execute(&mut *conn).await.unwrap();
    sqlx::query("ANALYZE thread_goals")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        goals::current(&mut conn, thread).await.unwrap().unwrap().id,
        live
    );
    let batch = goals::current_of(&mut conn, &requested).await.unwrap();
    assert_eq!(batch.len(), 1, "threads with no goal add no row");
    assert_eq!(batch[0].id, live, "the batch also prefers the live goal");
    assert!(goals::current_of(&mut conn, &[]).await.unwrap().is_empty());
    sqlx::query("UPDATE thread_goals SET status='met' WHERE id=$1")
        .bind(live)
        .execute(&mut *conn)
        .await
        .unwrap();
    let ended = goals::current(&mut conn, thread).await.unwrap().unwrap();
    assert_eq!(ended.status, "met");
    let batch = goals::current_of(&mut conn, &requested).await.unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(
        batch[0].id, ended.id,
        "the batch uses the same ended-goal order"
    );
    for query in [
        "SELECT id FROM thread_goals WHERE thread_id=$1 AND status IN ('active','paused')",
        "SELECT id FROM thread_goals WHERE thread_id=$1 AND status IN ('met','stopped','cleared') ORDER BY set_at DESC, id DESC LIMIT 1",
    ] {
        let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "EXPLAIN (FORMAT JSON) {query}"
        )))
        .bind(thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let plan = plan.to_string();
        assert!(!plan.contains("Seq Scan"), "{plan}");
        assert!(!plan.contains("Sort"), "{plan}");
    }
    let ended_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_goals WHERE thread_id=$1 AND status IN ('met','stopped')",
    )
    .bind(thread)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        goals::clear_ended(&mut conn, thread, Utc::now())
            .await
            .unwrap(),
        Some(ended.id)
    );
    assert!(
        goals::current(&mut conn, thread).await.unwrap().is_none(),
        "clearing the latest goal must not resurrect any older goal"
    );
    assert!(
        goals::current_of(&mut conn, &requested)
            .await
            .unwrap()
            .is_empty(),
        "the batch must also keep older goals hidden after Clear"
    );
    assert_eq!(
        goals::clear_ended(&mut conn, thread, Utc::now())
            .await
            .unwrap(),
        None,
        "repeated inputs must not clear progressively older goals"
    );
    let ended_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_goals WHERE thread_id=$1 AND status IN ('met','stopped')",
    )
    .bind(thread)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        ended_after,
        ended_before - 1,
        "clearing changes one row while retaining the historical outcomes"
    );
}
