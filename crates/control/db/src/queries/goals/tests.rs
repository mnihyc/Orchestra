//! Regression for repeated evaluation of the live/ended goal probes by an inlined IN subquery.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use super::*;
use crate::queries::{
    hierarchy::testing::insert_node, identity::testing::insert_user, threads::insert_thread,
};
use orch_testkit::database::TestDb;
use orch_types::new_id;
use serde_json::Value;

#[tokio::test]
async fn a_batch_materializes_each_goal_probe_once_even_when_a_thread_is_requested_again() {
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let (ada, acme, project, topic, thread) = (new_id(), new_id(), new_id(), new_id(), new_id());
    insert_user(&mut conn, ada, "ada").await.unwrap();
    insert_node(&mut conn, acme, "org", None, "Acme", ada)
        .await
        .unwrap();
    insert_node(&mut conn, project, "project", Some(acme), "Web", ada)
        .await
        .unwrap();
    insert_node(&mut conn, topic, "topic", Some(project), "Work", ada)
        .await
        .unwrap();
    assert!(
        insert_thread(&mut conn, thread, topic, "Tests", ada, Utc::now())
            .await
            .unwrap()
    );
    sqlx::query(
        "INSERT INTO thread_goals (id,thread_id,objective,condition,set_at,status)
                 SELECT gen_random_uuid(),$1,'Work','Done',now(),'met'
                 FROM generate_series(1,100000)",
    )
    .bind(thread)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("ANALYZE thread_goals")
        .execute(&mut *conn)
        .await
        .unwrap();
    let requested = vec![thread; 1_000];
    let expected = current(&mut conn, thread).await.unwrap().unwrap().id;
    let selected = current_of(&mut conn, &requested).await.unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, expected);
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {}",
        current_of_sql()
    )))
    .bind(&requested)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let mut pending = vec![&plan[0]["Plan"]];
    let mut saw_ended = false;
    while let Some(node) = pending.pop() {
        let kind = node["Node Type"].as_str().unwrap();
        assert!(!kind.contains("Sort"), "{plan}");
        if node["Relation Name"] == "thread_goals" {
            assert!(!kind.contains("Seq Scan"), "{plan}");
            let loops = node["Actual Loops"].as_f64().unwrap();
            let rows = node["Actual Rows"].as_f64().unwrap()
                + node
                    .get("Rows Removed by Filter")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
            // All 1000 requests name the same thread: each index must be probed at most once.
            assert!(loops <= 1.0, "{plan}");
            assert!(rows * loops <= 1.0, "{plan}");
            saw_ended |= node["Index Name"] == "thread_goals_ended_idx";
        }
        if let Some(children) = node.get("Plans").and_then(Value::as_array) {
            pending.extend(children);
        }
    }
    assert!(saw_ended, "the ended-goal index must be exercised: {plan}");
}
