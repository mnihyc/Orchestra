//! History item batches keep reference projections exact and fit the database statement budget.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_testkit::database::TestDb;
use std::time::{Duration, Instant};

async fn history_fixture(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) {
    sqlx::raw_sql(
        "SET LOCAL session_replication_role=replica;
         INSERT INTO users(id,username,display_name,avatar_color)
           VALUES(md5('owner')::uuid,'owner','Owner',0);
         INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by) VALUES
           (md5('org')::uuid,'org',NULL,md5('org')::uuid,ARRAY[md5('org')::uuid],'Org',md5('owner')::uuid,md5('owner')::uuid),
           (md5('topic')::uuid,'topic',md5('org')::uuid,md5('org')::uuid,ARRAY[md5('org')::uuid,md5('topic')::uuid],'Topic',md5('owner')::uuid,md5('owner')::uuid),
           (md5('thread')::uuid,'thread',md5('topic')::uuid,md5('org')::uuid,ARRAY[md5('org')::uuid,md5('topic')::uuid,md5('thread')::uuid],'Thread',md5('owner')::uuid,md5('owner')::uuid);
         INSERT INTO threads(node_id,topic_id,active_continuation_id)
           VALUES(md5('thread')::uuid,md5('topic')::uuid,md5('continuation')::uuid);
         INSERT INTO thread_continuations(id,thread_id,status,created_at,created_seq)
           VALUES(md5('continuation')::uuid,md5('thread')::uuid,'ready',now(),0);
         INSERT INTO segments(id,thread_id,ordinal,env_generation,arrangement,credential_path,seq_updated,continuation_id,import_fidelity,ended_at)
           VALUES(md5('segment')::uuid,md5('thread')::uuid,1,0,'separated','native',1,md5('continuation')::uuid,'structured',now());
         SET LOCAL session_replication_role=origin;",
    )
    .execute(&mut **tx)
    .await
    .unwrap();
}

async fn target_counts(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> (i64, i64) {
    sqlx::query_as(
        "SELECT
          COALESCE((SELECT links FROM related_reference_targets WHERE source_kind='conversation'
            AND source_id=md5('thread')::uuid AND target_kind='document' AND target_id=md5('a')::uuid),0),
          COALESCE((SELECT links FROM related_reference_targets WHERE source_kind='conversation'
            AND source_id=md5('thread')::uuid AND target_kind='document' AND target_id=md5('b')::uuid),0)",
    )
    .fetch_one(&mut **tx)
    .await
    .unwrap()
}

#[tokio::test]
async fn item_batches_keep_references_exact_through_updates_and_deletes() {
    let db = TestDb::fresh().await;
    let mut tx = db.pool.begin().await.unwrap();
    history_fixture(&mut tx).await;
    sqlx::raw_sql(
        "INSERT INTO items(id,thread_id,segment_id,continuation_id,kind,status,payload,seq_created,seq_updated)
         SELECT md5('item-'||n)::uuid,md5('thread')::uuid,md5('segment')::uuid,md5('continuation')::uuid,
           'agent_message',CASE WHEN n=3 THEN 'in_progress' ELSE 'completed' END,
           CASE WHEN n=4 THEN '{}'::jsonb
                WHEN n=2 THEN jsonb_build_array(jsonb_build_object('type','ref','kind','document','id',md5('a')::uuid),
                                                    jsonb_build_object('type','ref','kind','document','id',md5('b')::uuid))
                ELSE jsonb_build_array(jsonb_build_object('type','ref','kind','document','id',md5('a')::uuid)) END,n,n
         FROM generate_series(1,4) n;",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(target_counts(&mut tx).await, (2, 1));
    sqlx::raw_sql(
        "UPDATE items SET payload=jsonb_build_array(jsonb_build_object('type','ref','kind','document','id',md5('b')::uuid))
           WHERE id IN (md5('item-1')::uuid,md5('item-2')::uuid);
         UPDATE items SET status='completed' WHERE id=md5('item-3')::uuid;",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(target_counts(&mut tx).await, (1, 2));
    sqlx::query(
        "UPDATE items SET status='in_progress',payload=jsonb_build_array(
           jsonb_build_object('type','ref','kind','document','id',md5('b')::uuid)) WHERE id=md5('item-3')::uuid",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(target_counts(&mut tx).await, (1, 2));
    sqlx::query("UPDATE items SET status='completed' WHERE id=md5('item-3')::uuid")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(target_counts(&mut tx).await, (0, 3));
    sqlx::query("DELETE FROM items WHERE id IN (md5('item-2')::uuid,md5('item-3')::uuid)")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(target_counts(&mut tx).await, (0, 1));
}

#[tokio::test]
#[ignore = "measures a 200,000-item history; run explicitly for the database statement budget"]
async fn long_history_reference_pages_fit_the_api_statement_budget() {
    // The fork copies at most 500 items per statement, and the API allows 15 seconds per statement.
    const PAGE: i64 = 500;
    const ITEMS: i64 = 200_000;
    let db = TestDb::fresh().await;
    let mut tx = db.pool.begin().await.unwrap();
    history_fixture(&mut tx).await;
    sqlx::query("SET LOCAL statement_timeout='15s'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let start = Instant::now();
    let mut slowest = Duration::ZERO;
    for page in 0..ITEMS / PAGE {
        let began = Instant::now();
        sqlx::query(
            "INSERT INTO items(id,thread_id,segment_id,continuation_id,kind,status,payload,seq_created,seq_updated)
             SELECT md5('item-'||n)::uuid,md5('thread')::uuid,md5('segment')::uuid,md5('continuation')::uuid,
               'agent_message','completed',jsonb_build_object('parts',jsonb_build_array(
                 jsonb_build_object('type','ref','kind','document','id',md5('a')::uuid))),n,n
             FROM generate_series($1::bigint,$2::bigint) n",
        )
        .bind(page * PAGE + 1)
        .bind((page + 1) * PAGE)
        .execute(&mut *tx)
        .await
        .unwrap_or_else(|error| panic!("page {} failed: {error}", page + 1));
        slowest = slowest.max(began.elapsed());
        if (page + 1) % 20 == 0 {
            eprintln!(
                "items={} elapsed={:?} slowest_page={:?}",
                (page + 1) * PAGE,
                start.elapsed(),
                slowest
            );
        }
    }
    assert_eq!(target_counts(&mut tx).await, (ITEMS, 0));
}
