//! Million-row history admission stays an indexed existence check with one boolean result.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use orch_testkit::database::TestDb;
use orch_types::Id;
use serde_json::Value;

use super::{
    PAGE, PAGE_BODIES, copy_members_page, has_before_items, has_before_sql, missing_revision_sql,
    unconfirmed_inputs, unconfirmed_sql,
};

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
    // A text export asks for thirty-two turns at a time, including late
    // windows. The permanent ordinal index must skip all preceding turns.
    let window = crate::queries::thread_history::turns_between_sql("SELECT id FROM turns");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "SET plan_cache_mode=force_generic_plan; PREPARE history_window(uuid,uuid,bigint,bigint) AS {window}"
    )))
    .execute(&mut *conn)
    .await
    .unwrap();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) EXECUTE history_window('{thread}','{view}',501,532)"
    )))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(
        inspected_rows(&plan[0]["Plan"]) <= 256,
        "a thirty-two-turn window must not inspect the preceding history: {plan}"
    );
    sqlx::raw_sql("DEALLOCATE history_window; RESET plan_cache_mode")
        .execute(&mut *conn)
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
    // The same million-row world proves that a fresh cut does not enumerate
    // unchanged history while checking whether an older revision is missing.
    sqlx::raw_sql("ALTER TABLE items ADD COLUMN seq_updated bigint NOT NULL DEFAULT 1;
        CREATE INDEX items_continuation_updated_idx ON items(thread_id,continuation_id,seq_updated);
        CREATE TEMP TABLE thread_history_cut_spans(cut_id uuid,thread_id uuid,owner_continuation_id uuid,
            first_seq bigint,last_seq bigint,frozen_at bigint,loose_before_orch boolean,loose_low bigint,loose_high bigint,
            PRIMARY KEY(cut_id,thread_id,owner_continuation_id));
        CREATE TEMP TABLE thread_history_cut_rows(cut_id uuid,kind text,row_id uuid,PRIMARY KEY(cut_id,kind,row_id));
        CREATE TEMP TABLE thread_history_cut_items(cut_id uuid,item_id uuid,body jsonb,PRIMARY KEY(cut_id,item_id));
        CREATE TEMP TABLE thread_item_revisions(item_id uuid,seq_updated bigint,PRIMARY KEY(item_id,seq_updated));
        INSERT INTO thread_history_cut_spans VALUES(md5('cut')::uuid,md5('thread1')::uuid,md5('view1')::uuid,1,1000,1000,true,0,1000);
        INSERT INTO thread_history_cut_rows SELECT md5('cut')::uuid,'turn',id FROM turns WHERE thread_id=md5('thread1')::uuid;
        ANALYZE items; ANALYZE thread_history_cut_spans; ANALYZE thread_history_cut_rows;")
        .execute(&mut *conn).await.unwrap();
    let cut: Id = sqlx::query_scalar("SELECT md5('cut')::uuid")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) {}",
        missing_revision_sql()
    )))
    .bind(cut)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(
        inspected_rows(&plan[0]["Plan"]) <= 32,
        "an unchanged cut checks the update index instead of the entire history: {plan}"
    );
    sqlx::query("UPDATE items SET seq_updated=1001 WHERE id=md5('item999')::uuid")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>(missing_revision_sql())
            .bind(cut)
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        "damage near the end is found without depending on the first item page"
    );
    sqlx::query("INSERT INTO thread_item_revisions VALUES(md5('item999')::uuid,1000)")
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>(missing_revision_sql())
            .bind(cut)
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE items SET seq_updated=1001 WHERE id=md5('item1000')::uuid")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO thread_history_cut_items VALUES($1,md5('item1000')::uuid,'{}')")
        .bind(cut)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>(missing_revision_sql())
            .bind(cut)
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        "a retained frozen body does not need a live revision"
    );
    // A generic plan still decodes only its selected item page. Joining a page
    // to the body table must not choose a whole-history hash scan instead.
    sqlx::raw_sql("ALTER TABLE thread_history_cut_spans ADD COLUMN seq_offset bigint NOT NULL DEFAULT 0;
        ALTER TABLE thread_history_cut_items ADD COLUMN seq_created bigint NOT NULL DEFAULT 1000,
            ADD COLUMN turn_id uuid, ADD COLUMN stream text NOT NULL DEFAULT 'main',
            ADD COLUMN kind text NOT NULL DEFAULT 'agent_message', ADD COLUMN parent_cut_id uuid,
            ADD COLUMN revision_seq bigint, ADD COLUMN native_alias text;
        ALTER TABLE thread_item_revisions ADD COLUMN body jsonb;
        CREATE TEMP TABLE thread_native_item_aliases(item_id uuid PRIMARY KEY,native_ref text);
        CREATE INDEX ON items(thread_id,continuation_id,seq_created);
        ANALYZE thread_history_cut_items; ANALYZE thread_item_revisions; ANALYZE thread_native_item_aliases;
        SET plan_cache_mode=force_generic_plan;")
        .execute(&mut *conn).await.unwrap();
    let page = format!(
        "WITH {},{PAGE_BODIES} SELECT body FROM bodies",
        copy_members_page()
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "PREPARE history_page(uuid,bigint,bigint,text,bigint) AS {page}"
    )))
    .execute(&mut *conn)
    .await
    .unwrap();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) EXECUTE history_page('{cut}',500,9223372036854775807,NULL,{PAGE})"
    )))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(
        inspected_rows(&plan[0]["Plan"]) <= (PAGE * 32) as u64,
        "a decoded page must use point body lookups even under a generic plan: {plan}"
    );
    sqlx::raw_sql("DEALLOCATE history_page; RESET plan_cache_mode")
        .execute(&mut *conn)
        .await
        .unwrap();
}
