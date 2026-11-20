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
        Some("turns" | "items" | "thread_inputs" | "thread_history_item_edges")
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

tokio::task_local! {static COUNTED_SQL:std::sync::Arc<std::sync::atomic::AtomicUsize>;}
struct SqlStatements;
impl tracing::Subscriber for SqlStatements {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == "sqlx::query" {
            let _ = COUNTED_SQL
                .try_with(|count| count.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}
async fn counted_sql<T>(work: impl std::future::Future<Output = T>) -> (T, usize) {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| tracing::subscriber::set_global_default(SqlStatements).unwrap());
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let result = COUNTED_SQL.scope(count.clone(), work).await;
    (result, count.load(std::sync::atomic::Ordering::SeqCst))
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
        CREATE TEMP TABLE thread_history_cut_prefixes(cut_id uuid PRIMARY KEY,prefix_cut_id uuid NOT NULL);
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
    // A retained side prefix has its own immutable owner. Seeking a late page
    // must not expand that owner's two hundred thousand saved bodies.
    sqlx::raw_sql(
        "CREATE UNIQUE INDEX ON thread_history_cut_items(cut_id,seq_created);
        CREATE INDEX ON thread_history_cut_items(cut_id,seq_created) WHERE body IS NOT NULL;
        INSERT INTO thread_history_cut_prefixes VALUES(md5('cut')::uuid,md5('frozen-prefix')::uuid);
        INSERT INTO thread_history_cut_items(cut_id,item_id,seq_created,kind,body)
          SELECT md5('frozen-prefix')::uuid,CASE WHEN n<=1000 THEN md5('item'||n::text)::uuid
            ELSE md5('frozen-item:'||n::text)::uuid END,n,'agent_message','{}'::jsonb
          FROM generate_series(1,200000) n;
        ANALYZE thread_history_cut_items; ANALYZE thread_history_cut_prefixes;",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    for (after, through) in [(0, PAGE), (199_500, 200_000)] {
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) EXECUTE history_page('{cut}',{after},{through},NULL,{PAGE})"
        )))
        .fetch_one(&mut *conn).await.unwrap();
        assert_eq!(
            plan[0]["Plan"]["Actual Rows"].as_i64(),
            Some(PAGE),
            "the plan must return a real full page of retained bodies: {plan}"
        );
        assert!(
            inspected_rows(&plan[0]["Plan"]) <= (PAGE * 32) as u64,
            "a saved-prefix page uses point overrides, not whole-body hash scans: {plan}"
        );
    }
    sqlx::raw_sql("DEALLOCATE history_page; RESET plan_cache_mode")
        .execute(&mut *conn)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_full_detail_boundary_reads_its_edge_without_walking_a_long_prefix() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    sqlx::raw_sql(include_str!("scale.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let (thread, view, inherited): (Id, Id, Id) =
        sqlx::query_as("SELECT md5('thread1')::uuid,md5('view1')::uuid,md5('selected1')::uuid")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    sqlx::raw_sql(include_str!("full_boundaries.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let high_sql = crate::queries::thread_history::turn_high_sql();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "SET plan_cache_mode=force_generic_plan; PREPARE full_turn_edge(uuid,uuid,bigint) AS {high_sql}"
    ))).execute(&mut *conn).await.unwrap();
    for edge in [32, 199_968, 200_000] {
        let plan: Value=sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) EXECUTE full_turn_edge('{thread}','{view}',{edge})"
        ))).fetch_one(&mut *conn).await.unwrap();
        assert!(
            inspected_rows(&plan[0]["Plan"]) <= 64,
            "a Full window edge reads indexed boundaries, not its preceding history: {plan}"
        );
        assert_eq!(
            crate::queries::thread_history::turn_high(&mut conn, thread, view, edge)
                .await
                .unwrap(),
            Some(edge),
            "normal sequence ordering still selects the exact edge"
        );
    }
    sqlx::raw_sql(include_str!("full_late_boundary.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    for edge in [32, 199_968, 200_000] {
        assert_eq!(
            crate::queries::thread_history::turn_high(&mut conn, thread, view, edge)
                .await
                .unwrap(),
            Some(200_001),
            "a later-created item of an earlier turn remains in the exact prefix maximum"
        );
    }
    let late_plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) EXECUTE full_turn_edge('{thread}','{view}',32)"
    )))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(
        inspected_rows(&late_plan[0]["Plan"]) <= 64,
        "a late earlier-turn item does not require expanding the prefix: {late_plan}"
    );
    assert_eq!(
        crate::queries::thread_history::turn_high(&mut conn, thread, inherited, 500)
            .await
            .unwrap(),
        Some(500),
        "a frozen owner's sequence edge excludes later source items"
    );
    // Large sequence gaps can give equal floating distances. Exact sequence
    // ordering must still win within the representation's distance bucket.
    sqlx::query(
        "INSERT INTO items(id,thread_id,continuation_id,kind,payload,seq_created)
        VALUES(md5('boundary-distance-origin')::uuid,$1,$2,'marker','{}',9223372036854775806)",
    )
    .bind(thread)
    .bind(view)
    .execute(&mut *conn)
    .await
    .unwrap();
    let plan: Value=sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) EXECUTE full_turn_edge('{thread}','{view}',199968)"
    ))).fetch_one(&mut *conn).await.unwrap();
    let distance_bucket = (1_u64 << (i64::BITS - f64::MANTISSA_DIGITS + 1)) + 64;
    assert!(
        inspected_rows(&plan[0]["Plan"]) <= distance_bucket,
        "a sequence-distance bucket stays bounded independently of history length: {plan}"
    );
    assert_eq!(
        crate::queries::thread_history::turn_high(&mut conn, thread, view, 199_968)
            .await
            .unwrap(),
        Some(200_001)
    );
    sqlx::raw_sql("DEALLOCATE full_turn_edge; RESET plan_cache_mode")
        .execute(&mut *conn)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_frozen_cut_boundary_reads_its_edge_without_walking_its_whole_prefix() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    sqlx::raw_sql(include_str!("scale.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("full_boundaries.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("full_late_boundary.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("full_cut_boundaries.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let cut: Id = sqlx::query_scalar("SELECT md5('full-edge-cut')::uuid")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let sql = super::turn_high_sql();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "SET plan_cache_mode=force_generic_plan; PREPARE frozen_turn_edge(uuid,bigint) AS {sql}"
    )))
    .execute(&mut *conn)
    .await
    .unwrap();
    for edge in [32, 199_968, 200_000] {
        let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) EXECUTE frozen_turn_edge('{cut}',{edge})"
        )))
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(
            inspected_rows(&plan[0]["Plan"]) <= 64,
            "a frozen edge must use its captured turn boundary keys: {plan}"
        );
        assert_eq!(
            super::turn_high(&mut conn, cut, edge).await.unwrap(),
            Some(200_001)
        );
    }
    sqlx::query("UPDATE thread_history_cuts SET captured_seq=9223372036854775807 WHERE id=$1")
        .bind(cut)
        .execute(&mut *conn)
        .await
        .unwrap();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) EXECUTE frozen_turn_edge('{cut}',199968)"
    )))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let distance_bucket = (1_u64 << (i64::BITS - f64::MANTISSA_DIGITS + 1)) + 64;
    assert!(
        inspected_rows(&plan[0]["Plan"]) <= distance_bucket,
        "a frozen distance bucket stays bounded independently of prefix length: {plan}"
    );
    assert_eq!(
        super::turn_high(&mut conn, cut, 199_968).await.unwrap(),
        Some(200_001)
    );
    sqlx::raw_sql("DEALLOCATE frozen_turn_edge; RESET plan_cache_mode")
        .execute(&mut *conn)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_single_turn_cut_reads_its_metadata_without_walking_the_unselected_source() {
    let db = TestDb::fresh().await;
    let mut conn = db.pool.acquire().await.unwrap();
    sqlx::raw_sql(include_str!("selected_metadata.sql"))
        .execute(&mut *conn)
        .await
        .unwrap();
    let (cut, thread, view, segment): (Id, Id, Id, Id) = sqlx::query_as(
        "SELECT md5('metadata-cut')::uuid,md5('metadata-thread')::uuid,
          md5('metadata-view')::uuid,md5('metadata-segment200000')::uuid",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let asked = super::Capture {
        id: cut,
        thread,
        view,
        job: None,
        owner_thread: None,
        segment: Some(segment),
        execution_revision: 1,
        range: Some((199999, 199999)),
        at: orch_testkit::clock::start(),
    };
    let (_, statements) = counted_sql(async {
        super::capture_source_metadata(&mut conn, &asked, true)
            .await
            .unwrap();
        super::capture_source_metadata(&mut conn, &asked, false)
            .await
            .unwrap();
    })
    .await;
    assert!(
        statements > 0,
        "the query observer must see the actual metadata work"
    );
    // One current-binding point read, one selected-turn page per metadata kind,
    // their inserts and end checks fit here; no whole-source cursor is allowed.
    assert!(
        statements <= 16,
        "a one-turn answer visited {statements} metadata statements"
    );
    let counts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind,count(*)::bigint FROM thread_history_cut_rows
      WHERE cut_id=$1 AND kind<>'turn' GROUP BY kind ORDER BY kind",
    )
    .bind(cut)
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        counts,
        vec![("input".to_owned(), 1), ("segment".to_owned(), 2)],
        "the selected segment and the separate current binding both remain, with exactly its input"
    );
    let selected: Id = sqlx::query_scalar("SELECT md5('metadata-turn199999')::uuid")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let sql = super::input_turn_metadata_sql();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "SET plan_cache_mode=force_generic_plan; PREPARE selected_input_page(uuid,uuid,uuid[],uuid,uuid,bigint) AS {sql}"
    ))).execute(&mut *conn).await.unwrap();
    let plan:Value=sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE,TIMING OFF,FORMAT JSON) EXECUTE selected_input_page('{cut}','{thread}',ARRAY['{selected}'::uuid],NULL,NULL,{PAGE})"
    ))).fetch_one(&mut *conn).await.unwrap();
    assert!(
        inspected_rows(&plan[0]["Plan"]) <= 32,
        "a selected input page must seek the selected turn, not filter the source: {plan}"
    );
    sqlx::raw_sql("DEALLOCATE selected_input_page; RESET plan_cache_mode")
        .execute(&mut *conn)
        .await
        .unwrap();
    let extra = 2 * PAGE + 1;
    sqlx::query("INSERT INTO thread_inputs(id,thread_id,turn_id,state)
      SELECT md5('related-selected-input'||n)::uuid,$1,$2,'delivered' FROM generate_series(1,$3::bigint)n")
        .bind(thread).bind(selected).bind(extra).execute(&mut *conn).await.unwrap();
    let (result, work) =
        counted_sql(super::capture_source_metadata(&mut conn, &asked, false)).await;
    result.unwrap();
    let expected_pages = usize::try_from(extra + 1)
        .unwrap()
        .div_ceil(usize::try_from(PAGE).unwrap());
    assert!(
        work <= 6 + expected_pages,
        "reverse input links grow by their answer pages, not unselected source pages: {work}"
    );
    let captured: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_history_cut_rows WHERE cut_id=$1 AND kind='input'",
    )
    .bind(cut)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        captured,
        extra + 1,
        "direct and reverse links neither lose nor duplicate an input"
    );
}
