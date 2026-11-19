//! Context estimates retain exact rounding while rereading only sources that changed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use super::*;
use orch_testkit::database::TestDb;
use serde_json::json;
use sqlx::Connection;

async fn scene(conn: &mut Conn) -> (Id, Id) {
    let person = new_id();
    orch_db::queries::identity::testing::insert_user(conn, person, "context-reader")
        .await
        .unwrap();
    let mut parent = None;
    let mut thread = person;
    for kind in ["org", "project", "topic", "thread"] {
        thread = new_id();
        orch_db::queries::hierarchy::testing::insert_node(conn, thread, kind, parent, kind, person)
            .await
            .unwrap();
        parent = Some(thread);
    }
    let segment = new_id();
    sqlx::query("INSERT INTO segments(id,thread_id,ordinal,env_generation,arrangement,credential_path,ended_at,import_fidelity,seq_updated,continuation_id) SELECT $1,node_id,1,1,'direct','native',now(),'structured',1,active_continuation_id FROM threads WHERE node_id=$2")
        .bind(segment).bind(thread).execute(&mut *conn).await.unwrap();
    (thread, segment)
}

async fn equal_to_recorded_scan(conn: &mut Conn, segment: Id, family: Family) {
    let expected = scan_seen(conn, segment, family).await.unwrap();
    let actual = seen_of(conn, segment, family).await.unwrap();
    assert_eq!(
        serde_json::to_value(estimates_of(&actual, family)).unwrap(),
        serde_json::to_value(estimates_of(&expected, family)).unwrap()
    );
    let fill = compose(
        100_000,
        None,
        None,
        vec![
            orch_row_part(ContextPartKind::Harness),
            orch_row_part(ContextPartKind::PeopleMessages),
        ],
        Time::new(chrono::Utc::now()),
    );
    assert_eq!(
        serde_json::to_value(with_orch_share(fill.clone(), &actual, family)).unwrap(),
        serde_json::to_value(with_orch_share(fill, &expected, family)).unwrap()
    );
}

fn orch_row_part(kind: ContextPartKind) -> ContextPart {
    ContextPart {
        kind,
        tokens: 10_000,
        count: ContextCount::Estimated,
        rows: Vec::new(),
    }
}

async fn source_reads(conn: &mut Conn) -> i64 {
    sqlx::query_scalar("SELECT COALESCE(sum(seq_tup_read+idx_tup_fetch),0)::bigint FROM pg_stat_xact_user_tables WHERE relid IN ('items'::regclass,'model_inputs'::regclass)")
        .fetch_one(conn).await.unwrap()
}

#[tokio::test]
async fn an_unchanged_long_conversation_keeps_each_block_rounding_without_rereading_its_history() {
    let database = TestDb::fresh().await;
    let mut conn = database.pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let (thread, segment) = scene(&mut tx).await;
    // A thousand one-character blocks distinguish per-block rounding from one concatenated estimate.
    sqlx::query("INSERT INTO model_inputs(id,node_id,segment_id,purpose,reference,blocks,state,seq_created,seq_updated) SELECT md5('context-input-'||n)::uuid,$1,$2,'input','input-'||n,'[{\"key\":\"input\",\"source\":\"person\",\"text\":\"x\"},{\"key\":\"where\",\"source\":\"orch\",\"text\":\"界\"}]'::jsonb,'sent',n,n FROM generate_series(1,1000) AS n")
        .bind(thread).bind(segment).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO items(id,thread_id,segment_id,continuation_id,kind,status,payload,seq_created,seq_updated) SELECT md5('context-item-'||n)::uuid,$1,$2,(SELECT active_continuation_id FROM threads WHERE node_id=$1),'agent_message','completed','{\"text\":\"x\"}'::jsonb,n+2000,n+2000 FROM generate_series(1,1000) AS n")
        .bind(thread).bind(segment).execute(&mut *tx).await.unwrap();
    let waiting: (i64, Option<i64>) = sqlx::query_as(
        "SELECT count(*),max(revision) FROM context_estimate_dirty WHERE segment_id=$1",
    )
    .bind(segment)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        waiting,
        (1, Some(2)),
        "a segment with no context report records each bulk write once in its one refresh record"
    );
    let family = crate::token_density::of("gpt-6-sol");
    equal_to_recorded_scan(&mut tx, segment, family).await;
    let before = source_reads(&mut tx).await;
    let seen = seen_of(&mut tx, segment, family).await.unwrap();
    let after = source_reads(&mut tx).await;
    assert!(
        after - before <= 2,
        "an unchanged estimate reread {} historical rows",
        after - before
    );
    let parts = estimates_of(&seen, family);
    assert_eq!(
        parts
            .iter()
            .find(|part| part.kind == ContextPartKind::PeopleMessages)
            .unwrap()
            .tokens,
        1000
    );
    tx.commit().await.unwrap();
    drop(conn);
    let mut conn = database.pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let before = source_reads(&mut tx).await;
    seen_of(&mut tx, segment, family).await.unwrap();
    assert!(
        source_reads(&mut tx).await - before <= 2,
        "a new transaction retains the durable estimate"
    );
    tx.commit().await.unwrap();
    drop(conn);
    database.close().await.unwrap();
}

#[tokio::test]
async fn estimates_replace_changed_and_deleted_sources_and_rebuild_only_for_a_new_context_or_density()
 {
    let database = TestDb::fresh().await;
    let mut conn = database.pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let (thread, segment) = scene(&mut tx).await;
    let family = crate::token_density::of("gpt-6-sol");
    let input = new_id();
    sqlx::query("INSERT INTO model_inputs(id,node_id,segment_id,purpose,reference,blocks,state,seq_created,seq_updated) VALUES($1,$2,$3,'session','opening',$4,'sent',0,0)")
        .bind(new_id()).bind(thread).bind(segment).bind(json!([{"key":"preamble","source":"orch","text":"session instructions"},{"key":"search","source":"tool","text":"{}","total_len":331}])).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO model_inputs(id,node_id,segment_id,purpose,reference,blocks,state,seq_created,seq_updated) VALUES($1,$2,$3,'input','changing',$4,'sent',1,1)")
        .bind(input).bind(thread).bind(segment).bind(json!([{"key":"input","source":"person","text":"字abc"},{"key":"memory","source":"orch","text":"x","total_len":1001}])).execute(&mut *tx).await.unwrap();
    let item = new_id();
    sqlx::query("INSERT INTO items(id,thread_id,segment_id,continuation_id,kind,status,payload,seq_created,seq_updated) SELECT $1,$2,$3,active_continuation_id,'tool_call','in_progress',$4,2,2 FROM threads WHERE node_id=$2")
        .bind(item).bind(thread).bind(segment).bind(json!({"tool":{"kind":"command"},"raw":{"name":"exec","input":{"total_len":999999},"output":{"total_len":77}}})).execute(&mut *tx).await.unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    for patch in [
        json!({"tool":{"kind":"command"},"raw":{"name":"exec","input":{"total_len":123},"output":{"total_len":999999}}}),
        json!({"tool":{"kind":"image"},"raw":{"name":"view"}}),
    ] {
        sqlx::query("UPDATE items SET payload=$2,seq_updated=seq_updated+1 WHERE id=$1")
            .bind(item)
            .bind(patch)
            .execute(&mut *tx)
            .await
            .unwrap();
        equal_to_recorded_scan(&mut tx, segment, family).await;
    }
    for (kind, payload) in [
        ("agent_message", json!({"text":"a streaming answer"})),
        ("agent_message", json!({"text":"a streaming answer字"})),
        (
            "user_message",
            json!({"parts":[{"type":"image"},{"type":"text","text":"hello"},{"type":"image"}]}),
        ),
    ] {
        sqlx::query("UPDATE items SET kind=$2,payload=$3 WHERE id=$1")
            .bind(item)
            .bind(kind)
            .bind(payload)
            .execute(&mut *tx)
            .await
            .unwrap();
        equal_to_recorded_scan(&mut tx, segment, family).await;
    }
    sqlx::query("UPDATE model_inputs SET blocks=$2 WHERE node_id=$1 AND purpose='session'")
        .bind(thread)
        .bind(json!([{"key":"preamble","source":"orch","text":"new session instructions"}]))
        .execute(&mut *tx)
        .await
        .unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    sqlx::query("UPDATE items SET before_orch=true WHERE id=$1")
        .bind(item)
        .execute(&mut *tx)
        .await
        .unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    sqlx::query("UPDATE items SET before_orch=false,stream='sub:helper' WHERE id=$1")
        .bind(item)
        .execute(&mut *tx)
        .await
        .unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    sqlx::query("UPDATE items SET stream='main' WHERE id=$1")
        .bind(item)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE model_inputs SET state='failed' WHERE id=$1")
        .bind(input)
        .execute(&mut *tx)
        .await
        .unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    sqlx::query("UPDATE model_inputs SET state='sent',blocks=$2 WHERE id=$1")
        .bind(input)
        .bind(json!([{"key":"input","source":"person","text":"changed界"}]))
        .execute(&mut *tx)
        .await
        .unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    equal_to_recorded_scan(
        &mut tx,
        segment,
        crate::token_density::of("claude-sonnet-5"),
    )
    .await;
    let compacted = new_id();
    sqlx::query("INSERT INTO items(id,thread_id,segment_id,continuation_id,kind,status,payload,seq_created,seq_updated) SELECT $1,$2,$3,active_continuation_id,'marker','completed','{\"marker\":\"compacted\"}'::jsonb,3,4 FROM threads WHERE node_id=$2")
        .bind(compacted).bind(thread).bind(segment).execute(&mut *tx).await.unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    sqlx::query("DELETE FROM items WHERE id=$1")
        .bind(compacted)
        .execute(&mut *tx)
        .await
        .unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    {
        let mut save = tx.begin().await.unwrap();
        sqlx::query("DELETE FROM model_inputs WHERE id=$1")
            .bind(input)
            .execute(&mut *save)
            .await
            .unwrap();
        equal_to_recorded_scan(&mut save, segment, family).await;
        save.rollback().await.unwrap();
    }
    equal_to_recorded_scan(&mut tx, segment, family).await;
    sqlx::query("DELETE FROM model_inputs WHERE id=$1")
        .bind(input)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM items WHERE id=$1")
        .bind(item)
        .execute(&mut *tx)
        .await
        .unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    tx.commit().await.unwrap();
    drop(conn);
    database.close().await.unwrap();
}

#[tokio::test]
async fn bulk_context_changes_keep_every_source_and_advance_each_observed_revision_once() {
    let database = TestDb::fresh().await;
    let mut conn = database.pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let (thread, segment) = scene(&mut tx).await;
    let family = crate::token_density::of("gpt-6-sol");
    equal_to_recorded_scan(&mut tx, segment, family).await;
    sqlx::query("INSERT INTO model_inputs(id,node_id,segment_id,purpose,reference,blocks,state,seq_created,seq_updated)
        SELECT md5('batched-input-'||n)::uuid,$1,$2,'input','batch-'||n,
        '[{\"key\":\"input\",\"source\":\"person\",\"text\":\"x\"}]'::jsonb,'sent',n,n FROM generate_series(1,8)n")
        .bind(thread).bind(segment).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO items(id,thread_id,segment_id,continuation_id,kind,status,payload,seq_created,seq_updated)
        SELECT md5('batched-item-'||n)::uuid,$1,$2,(SELECT active_continuation_id FROM threads WHERE node_id=$1),
        'agent_message','completed','{\"text\":\"x\"}'::jsonb,n+10,n+10 FROM generate_series(1,8)n")
        .bind(thread).bind(segment).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE model_inputs SET blocks='[{\"key\":\"input\",\"source\":\"person\",\"text\":\"changed\"}]'::jsonb WHERE segment_id=$1")
        .bind(segment).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE items SET payload='{\"text\":\"changed\"}'::jsonb WHERE segment_id=$1")
        .bind(segment)
        .execute(&mut *tx)
        .await
        .unwrap();
    let pending: Vec<(String, i64, i64, i64)> = sqlx::query_as(
        "SELECT source,count(*),min(revision),max(revision) FROM context_estimate_dirty
          WHERE segment_id=$1 GROUP BY source ORDER BY source",
    )
    .bind(segment)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        pending,
        vec![("input".into(), 8, 2, 2), ("item".into(), 8, 2, 2)]
    );
    equal_to_recorded_scan(&mut tx, segment, family).await;
    sqlx::query("UPDATE items SET status='in_progress' WHERE segment_id=$1")
        .bind(segment)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE model_inputs SET seq_updated=seq_updated+1 WHERE segment_id=$1")
        .bind(segment)
        .execute(&mut *tx)
        .await
        .unwrap();
    let untouched: i64 =
        sqlx::query_scalar("SELECT count(*) FROM context_estimate_dirty WHERE segment_id=$1")
            .bind(segment)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        untouched, 0,
        "metadata alone does not invalidate the context sources"
    );
    let next = new_id();
    sqlx::query("INSERT INTO segments(id,thread_id,ordinal,env_generation,arrangement,credential_path,ended_at,import_fidelity,seq_updated,continuation_id)
        SELECT $1,node_id,2,1,'direct','native',now(),'structured',1,active_continuation_id FROM threads WHERE node_id=$2")
        .bind(next).bind(thread).execute(&mut *tx).await.unwrap();
    equal_to_recorded_scan(&mut tx, next, family).await;
    sqlx::query("UPDATE items SET segment_id=$2 WHERE segment_id=$1")
        .bind(segment)
        .bind(next)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE model_inputs SET segment_id=$2 WHERE segment_id=$1")
        .bind(segment)
        .bind(next)
        .execute(&mut *tx)
        .await
        .unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    equal_to_recorded_scan(&mut tx, next, family).await;
    {
        let mut removed = tx.begin().await.unwrap();
        sqlx::query("DELETE FROM items WHERE segment_id=$1")
            .bind(next)
            .execute(&mut *removed)
            .await
            .unwrap();
        sqlx::query("DELETE FROM model_inputs WHERE segment_id=$1")
            .bind(next)
            .execute(&mut *removed)
            .await
            .unwrap();
        equal_to_recorded_scan(&mut removed, next, family).await;
        removed.rollback().await.unwrap();
    }
    equal_to_recorded_scan(&mut tx, next, family).await;
    tx.commit().await.unwrap();
    drop(conn);
    database.close().await.unwrap();
}

async fn waits_on(conn: &mut Conn, pid: i32, table: Option<&str>) {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT CASE WHEN $2::text IS NULL THEN cardinality(pg_blocking_pids($1))>0 ELSE EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND relation=to_regclass($2) AND NOT granted) END")
                .bind(pid).bind(table).fetch_one(&mut *conn).await.unwrap();
            if waiting { return; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("the native lock establishes the intended interleaving");
}

#[tokio::test]
async fn an_input_can_change_while_its_previous_context_estimate_is_paused_and_the_change_is_never_lost()
 {
    let database = TestDb::fresh().await;
    let mut conn = database.pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let (thread, segment) = scene(&mut tx).await;
    let input = new_id();
    let family = crate::token_density::of("gpt-6-sol");
    sqlx::query("INSERT INTO model_inputs(id,node_id,segment_id,purpose,reference,blocks,state,seq_created,seq_updated) VALUES($1,$2,$3,'input','racing',$4,'sent',1,1)")
        .bind(input).bind(thread).bind(segment).bind(json!([{"key":"input","source":"person","text":"original"}])).execute(&mut *tx).await.unwrap();
    seen_of(&mut tx, segment, family).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query("UPDATE model_inputs SET blocks=$2 WHERE id=$1")
        .bind(input)
        .bind(json!([{"key":"input","source":"person","text":"first"}]))
        .execute(&mut *conn)
        .await
        .unwrap();
    let mut blocker = conn.begin().await.unwrap();
    sqlx::query("LOCK TABLE context_estimate_contributions IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pool = database.pool.clone();
    let (reader_id, reader_pid) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        let mut conn = pool.acquire().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        reader_id.send(pid).unwrap();
        let mut tx = conn.begin().await.unwrap();
        let seen = seen_of(&mut tx, segment, family).await.unwrap();
        tx.commit().await.unwrap();
        estimates_of(&seen, family)
    });
    waits_on(
        &mut blocker,
        reader_pid.await.unwrap(),
        Some("context_estimate_contributions"),
    )
    .await;
    let pool = database.pool.clone();
    let writer = tokio::spawn(async move {
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("UPDATE model_inputs SET blocks=$2 WHERE id=$1")
            .bind(input)
            .bind(json!([{"key":"input","source":"person","text":"second".repeat(1000)}]))
            .execute(&mut *conn)
            .await
            .unwrap();
    });
    tokio::time::timeout(std::time::Duration::from_secs(20), writer)
        .await
        .expect("an estimate does not hold up a new source write")
        .unwrap();
    blocker.commit().await.unwrap();
    let first = tokio::time::timeout(std::time::Duration::from_secs(20), reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first
            .iter()
            .find(|part| part.kind == ContextPartKind::PeopleMessages)
            .unwrap()
            .tokens,
        estimate_text(&"second".repeat(1000), family, TextKind::Prose)
    );
    let mut tx = conn.begin().await.unwrap();
    equal_to_recorded_scan(&mut tx, segment, family).await;
    tx.commit().await.unwrap();
    drop(conn);
    database.close().await.unwrap();
}
