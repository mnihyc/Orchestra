//! Goal retries keep their identities through cleanup, and catch-up accounting preserves exact totals.
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
use sqlx::Connection;

async fn world(conn: &mut Conn) -> (Id, [Id; 2]) {
    let (person, org, project, topic, thread) = (new_id(), new_id(), new_id(), new_id(), new_id());
    insert_user(conn, person, "ada").await.unwrap();
    insert_node(conn, org, "org", None, "Team", person)
        .await
        .unwrap();
    insert_node(conn, project, "project", Some(org), "Work", person)
        .await
        .unwrap();
    insert_node(conn, topic, "topic", Some(project), "Tests", person)
        .await
        .unwrap();
    assert!(
        insert_thread(conn, thread, topic, "Goal", person, Utc::now())
            .await
            .unwrap()
    );
    let ids = [new_id(), new_id()];
    for id in ids {
        sqlx::query(
            "INSERT INTO thread_goals(id, thread_id, objective, condition, set_at, status)
                     VALUES ($1, $2, 'Check the work', 'All tests pass', now(), 'met')",
        )
        .bind(id)
        .bind(thread)
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    (thread, ids)
}

async fn input(conn: &mut Conn, thread: Id, goal: Option<Id>, state: &str) -> Id {
    let id = new_id();
    sqlx::query("INSERT INTO thread_inputs(id, thread_id, goal_id, source, content, delivery, state, position, seq_updated)
                 VALUES ($1, $2, $3, 'goal', '[]', 'queue', $4, 1, 1)")
        .bind(id).bind(thread).bind(goal).bind(state).execute(conn).await.unwrap();
    id
}

#[tokio::test]
async fn retries_are_atomic_retained_generations_and_rollback_with_the_input() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let (thread, [goal, other]) = world(&mut conn).await;
    let pending = input(&mut conn, thread, Some(goal), "pending").await;
    input(&mut conn, thread, None, "failed").await;
    assert_eq!(goals::lost_inputs(&mut conn, goal).await.unwrap(), 0);
    sqlx::query("UPDATE thread_inputs SET state='failed' WHERE id=$1")
        .bind(pending)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(goals::lost_inputs(&mut conn, goal).await.unwrap(), 1);
    sqlx::query("UPDATE thread_inputs SET state='cancelled' WHERE id=$1")
        .bind(pending)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE thread_inputs SET state=state, goal_id=goal_id WHERE id=$1")
        .bind(pending)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(goals::lost_inputs(&mut conn, goal).await.unwrap(), 1);
    let mut transaction = conn.begin().await.unwrap();
    input(&mut transaction, thread, Some(goal), "failed").await;
    assert_eq!(goals::lost_inputs(&mut transaction, goal).await.unwrap(), 2);
    transaction.rollback().await.unwrap();
    assert_eq!(goals::lost_inputs(&mut conn, goal).await.unwrap(), 1);
    sqlx::query("DELETE FROM thread_inputs WHERE id=$1")
        .bind(pending)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        goals::lost_inputs(&mut conn, goal).await.unwrap(),
        1,
        "cleanup cannot reuse a continuation's idempotency identity"
    );
    let cancelled = input(&mut conn, thread, Some(goal), "cancelled").await;
    assert_eq!(goals::lost_inputs(&mut conn, goal).await.unwrap(), 2);
    sqlx::query("UPDATE thread_inputs SET goal_id=$2 WHERE id=$1")
        .bind(cancelled)
        .bind(other)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(goals::lost_inputs(&mut conn, goal).await.unwrap(), 2);
    assert_eq!(goals::lost_inputs(&mut conn, other).await.unwrap(), 1);
    sqlx::query("UPDATE thread_inputs SET state='pending' WHERE id=$1")
        .bind(cancelled)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE thread_inputs SET state='failed' WHERE id=$1")
        .bind(cancelled)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        goals::lost_inputs(&mut conn, other).await.unwrap(),
        2,
        "a subsequent failed delivery is a new attempt"
    );
}

#[tokio::test]
async fn concurrent_input_failures_do_not_lose_retry_generations() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let (thread, [goal, _]) = world(&mut conn).await;
    sqlx::query("INSERT INTO thread_inputs(id,thread_id,goal_id,source,content,delivery,state,position,seq_updated)
                 SELECT gen_random_uuid(),$1,$2,'goal','[]','queue','pending',n,1 FROM generate_series(1,32)n")
        .bind(thread).bind(goal).execute(&mut *conn).await.unwrap();
    drop(conn);
    let first = async {
        sqlx::query("UPDATE thread_inputs SET state='failed' WHERE goal_id=$1 AND position<=16")
            .bind(goal)
            .execute(&test.pool)
            .await
            .unwrap()
    };
    let second = async {
        sqlx::query("UPDATE thread_inputs SET state='cancelled' WHERE goal_id=$1 AND position>16")
            .bind(goal)
            .execute(&test.pool)
            .await
            .unwrap()
    };
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.rows_affected(), 16);
    assert_eq!(second.rows_affected(), 16);
    let mut conn = test.pool.acquire().await.unwrap();
    assert_eq!(goals::lost_inputs(&mut conn, goal).await.unwrap(), 32);
    sqlx::query("DELETE FROM thread_inputs WHERE goal_id=$1")
        .bind(goal)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(goals::lost_inputs(&mut conn, goal).await.unwrap(), 32);
}

fn figure(ordinal: i32, started: bool, end_pending: bool, credits: &str) -> goals::TurnFigures {
    goals::TurnFigures {
        id: new_id(),
        ordinal,
        state: "completed".into(),
        interrupt_cause: None,
        interrupted_by: None,
        started,
        end_pending,
        tokens: 11,
        credits: credits.into(),
    }
}

#[tokio::test]
async fn catch_up_counts_only_the_finished_prefix_and_keeps_decimal_credits_exact() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let (_, [goal, _]) = world(&mut conn).await;
    sqlx::query("UPDATE thread_goals SET turns_used=7,tokens_used=19,credits_used=0.000000000000000001,counted_turn=8 WHERE id=$1")
        .bind(goal).execute(&mut *conn).await.unwrap();
    let at = Utc::now();
    let turns = [
        figure(9, true, false, "0.123456789012345678"),
        figure(10, false, false, "99"),
        figure(11, true, false, "0.2"),
        figure(12, true, true, "10"),
        figure(13, true, false, "20"),
    ];
    goals::add_usage_batch(&mut conn, goal, &turns, at)
        .await
        .unwrap();
    let row = goals::goal(&mut conn, goal).await.unwrap().unwrap();
    assert_eq!(
        (row.turns_used, row.tokens_used, row.counted_turn),
        (9, 41, 11)
    );
    assert_eq!(row.credits_used, "0.323456789012345679");
    assert_eq!(row.updated_at.timestamp_millis(), at.timestamp_millis());
    goals::add_usage_batch(&mut conn, goal, &[], at)
        .await
        .unwrap();
    goals::add_usage_batch(&mut conn, goal, &turns[3..], at)
        .await
        .unwrap();
    assert_eq!(goals::goal(&mut conn, goal).await.unwrap().unwrap(), row);
    goals::add_usage_batch(&mut conn, goal, &[figure(12, false, false, "99")], at)
        .await
        .unwrap();
    let row = goals::goal(&mut conn, goal).await.unwrap().unwrap();
    assert_eq!(
        (row.turns_used, row.tokens_used, row.counted_turn),
        (9, 41, 12)
    );
    assert_eq!(row.credits_used, "0.323456789012345679");
}
