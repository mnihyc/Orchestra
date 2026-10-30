//! Million-row history admission stays an indexed existence check with one boolean result.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use orch_testkit::database::TestDb;
use orch_types::Id;
use serde_json::Value;

use super::{has_before_items, has_before_sql, unconfirmed_inputs, unconfirmed_sql};

fn inspected_rows(node: &Value) -> u64 {
    if matches!(
        node["Relation Name"].as_str(),
        Some("turns" | "items" | "thread_inputs")
    ) {
        assert_ne!(
            node["Node Type"], "Seq Scan",
            "large relations must use an index: {node}"
        );
    }
    assert_ne!(
        node["CTE Name"], "chosen",
        "no materialized whole turn selection"
    );
    let here = if node["Relation Name"].is_string() {
        (
            // EXPLAIN rounds per-loop figures; allow one extra row per loop in this upper bound.
            (node["Actual Rows"].as_u64().unwrap_or(0) + 1)
                + node["Rows Removed by Filter"].as_u64().unwrap_or(0)
                + node["Rows Removed by Index Recheck"].as_u64().unwrap_or(0)
        ) * node["Actual Loops"].as_u64().unwrap_or(0)
    } else {
        0
    };
    here + node["Plans"]
        .as_array()
        .map_or(0, |plans| plans.iter().map(inspected_rows).sum())
}

#[tokio::test]
async fn a_million_completed_turns_do_not_become_the_uncertainty_working_set() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    sqlx::raw_sql(include_str!("scale.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("history_uncertainty_indexes.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::raw_sql("ANALYZE threads; ANALYZE thread_continuations; ANALYZE thread_continuation_spans; ANALYZE thread_inputs; ANALYZE turns; ANALYZE items;")
        .execute(&mut *conn).await.unwrap();
    let (thread, view, inherited): (Id, Id, Id) =
        sqlx::query_as("SELECT md5('thread1')::uuid,md5('view1')::uuid,md5('selected1')::uuid")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(
        !unconfirmed_inputs(&mut conn, thread, view, None)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE thread_inputs SET state='failed' WHERE id=md5('input1000')::uuid")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        unconfirmed_inputs(&mut conn, thread, view, None)
            .await
            .unwrap()
    );
    assert!(
        !unconfirmed_inputs(&mut conn, thread, view, Some((1, 999)))
            .await
            .unwrap()
    );
    assert!(
        !unconfirmed_inputs(&mut conn, thread, view, Some((1, 0)))
            .await
            .unwrap()
    );
    assert!(
        !unconfirmed_inputs(&mut conn, thread, inherited, None)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE items SET kind='user_message',payload=jsonb_build_object('input_id',md5('untouched')::uuid) WHERE id=md5('item500')::uuid")
        .execute(&mut *conn).await.unwrap();
    assert!(
        unconfirmed_inputs(&mut conn, thread, inherited, None)
            .await
            .unwrap()
    );
    assert!(
        !unconfirmed_inputs(&mut conn, thread, inherited, Some((501, 1000)))
            .await
            .unwrap()
    );
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) {}",
        unconfirmed_sql()
    )))
    .bind(thread)
    .bind(inherited)
    .bind(Option::<i32>::None)
    .bind(Option::<i32>::None)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(
        inspected_rows(&plan[0]["Plan"]) <= 32,
        "the guard inspects sparse references, not completed history: {plan}"
    );
    let ids: Vec<Id> = sqlx::query_scalar("SELECT node_id FROM threads")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    let totals = crate::queries::shares::turn_totals(&mut conn, &ids)
        .await
        .unwrap();
    assert_eq!(totals.len(), 1000);
    assert!(totals.values().all(|(turns, _)| *turns == 1000));
    assert!(
        !has_before_items(&mut conn, thread, inherited)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE items SET before_orch=true,turn_id=NULL WHERE id=md5('item500')::uuid")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        has_before_items(&mut conn, thread, inherited)
            .await
            .unwrap()
    );
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) {}",
        has_before_sql()
    )))
    .bind(thread)
    .bind(inherited)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(
        inspected_rows(&plan[0]["Plan"]) <= 32,
        "an existence check must not decode before-Orch history: {plan}"
    );
}
