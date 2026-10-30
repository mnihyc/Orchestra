//! Stored reference projections preserve an existing database and keep native edits, anchors and paging exact.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "document_reference_storage/chunks.rs"]
mod chunks;

use orch_db::queries::links::{self, OutPosition};
use orch_testkit::database::TestDb;
use orch_types::Id;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn native_projection_writes_keep_counts_and_earliest_anchors_exact() -> TestResult {
    let db = TestDb::fresh().await;
    let mut tx = db.pool.begin().await?;
    sqlx::raw_sql(
        "SET LOCAL session_replication_role=replica;
         INSERT INTO users(id,username,display_name,avatar_color) VALUES(md5('owner')::uuid,'owner','Owner',0);
         INSERT INTO nodes(id,kind,parent_id,org_id,path,name,owner_id,created_by) VALUES
           (md5('org')::uuid,'org',NULL,md5('org')::uuid,ARRAY[md5('org')::uuid],'Org',md5('owner')::uuid,md5('owner')::uuid),
           (md5('topic')::uuid,'topic',md5('org')::uuid,md5('org')::uuid,ARRAY[md5('org')::uuid,md5('topic')::uuid],'Topic',md5('owner')::uuid,md5('owner')::uuid),
           (md5('thread')::uuid,'thread',md5('topic')::uuid,md5('org')::uuid,ARRAY[md5('org')::uuid,md5('topic')::uuid,md5('thread')::uuid],'Thread',md5('owner')::uuid,md5('owner')::uuid);
         INSERT INTO threads(node_id,topic_id,active_continuation_id) VALUES(md5('thread')::uuid,md5('topic')::uuid,md5('continuation')::uuid);
         INSERT INTO thread_continuations(id,thread_id,status,created_at,created_seq)
           VALUES(md5('continuation')::uuid,md5('thread')::uuid,'ready',now(),0);
         INSERT INTO segments(id,thread_id,ordinal,env_generation,arrangement,credential_path,seq_updated,continuation_id,import_fidelity,ended_at)
           VALUES(md5('segment')::uuid,md5('thread')::uuid,1,0,'separated','native',1,md5('continuation')::uuid,'structured',now());
         INSERT INTO documents(id,node_id,org_id,owner_id,title,basename)
           VALUES(md5('document')::uuid,md5('topic')::uuid,md5('org')::uuid,md5('owner')::uuid,'Document','Document.md');
         INSERT INTO document_discussions(id,document_id,created_by,seq_updated)
           VALUES(md5('discussion')::uuid,md5('document')::uuid,md5('owner')::uuid,1);
         SET LOCAL session_replication_role=origin;
         INSERT INTO items(id,thread_id,segment_id,kind,status,payload,search_text,continuation_id,seq_created,seq_updated)
           SELECT md5('item-'||n)::uuid,md5('thread')::uuid,md5('segment')::uuid,'user_message','completed',
             CASE WHEN n IN (1,1003) THEN '{\"text\":\"<orch>private</orch>\"}'::jsonb ELSE '{\"text\":\"ordinary\"}'::jsonb END,
             CASE WHEN n=2 THEN '<orch>private</orch>' ELSE NULL END,md5('continuation')::uuid,n,n
           FROM generate_series(1,1003) n;
         INSERT INTO document_blocks(id,document_id,position,type,content,seq_created,seq_updated,updated_at)
           SELECT md5('block-'||n)::uuid,md5('document')::uuid,n,'paragraph',
             jsonb_build_array(jsonb_build_object('type','ref','kind','document','id',
               ('00000004-0000-7000-8000-'||lpad(to_hex(CASE WHEN n%10=0 THEN 1 ELSE n+1 END),12,'0'))::uuid)),
             n,n,'2021-01-01T00:00:00Z' FROM generate_series(1,1003) n;
         INSERT INTO document_comments(id,discussion_id,document_id,author_id,content,seq_updated,created_at)
           VALUES(md5('comment')::uuid,md5('discussion')::uuid,md5('document')::uuid,md5('owner')::uuid,
             '[{\"type\":\"ref\",\"kind\":\"document\",\"id\":\"00000004-0000-7000-8000-000000000001\"}]',1,'2020-01-01T00:00:00Z');",
    )
    .execute(&mut *tx)
    .await?;
    let projected: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM items WHERE contains_internal_marker),
           (SELECT count(*) FROM related_references WHERE source_moment IS NOT NULL),
           (SELECT count(*) FROM thread_item_revisions)",
    )
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(projected, (3, 1004, 0));
    let document: Id = sqlx::query_scalar("SELECT md5('document')::uuid")
        .fetch_one(&mut *tx)
        .await?;
    let first = links::out_edges(&mut tx, document, None, 21).await?;
    assert_eq!(first.len(), 21);
    assert_eq!(first[0].mentions, 101);
    assert_eq!(first[0].first_kind, "comment");
    let last = first.last().unwrap();
    let cursor = OutPosition {
        target_id: last.target_id,
        target_kind: last.target_kind.clone(),
        path_key: last.target_path_key.clone(),
    };
    let next = links::out_edges(&mut tx, document, Some(&cursor), 21).await?;
    assert_eq!(next.len(), 21);
    assert!(
        first
            .iter()
            .all(|a| next.iter().all(|b| a.target_id != b.target_id))
    );
    let mut deep_cursor = OutPosition {
        target_id: "00000004-0000-7000-8000-0000000001f4".parse()?,
        target_kind: "document".to_owned(),
        path_key: Vec::new(),
    };
    let deep = links::out_edges(&mut tx, document, Some(&deep_cursor), 21).await?;
    assert_eq!(deep.len(), 21);
    // The next numerical target belongs to a repeated mention of target one.
    assert_eq!(
        deep[0].target_id,
        "00000004-0000-7000-8000-0000000001f6".parse::<Id>()?
    );
    assert!(
        deep.iter()
            .all(|edge| edge.target_id > deep_cursor.target_id)
    );
    deep_cursor.target_id = "00000004-0000-7000-8000-0000000003ec".parse()?;
    assert!(
        links::out_edges(&mut tx, document, Some(&deep_cursor), 21)
            .await?
            .is_empty()
    );
    sqlx::raw_sql(
        "UPDATE items SET payload='{\"text\":\"ordinary\"}',seq_updated=2000 WHERE id=md5('item-1')::uuid;
         UPDATE items SET contains_internal_marker=false WHERE id=md5('item-2')::uuid;
         UPDATE document_blocks SET updated_at='2019-01-01T00:00:00Z' WHERE id=md5('block-10')::uuid;",
    )
    .execute(&mut *tx)
    .await?;
    let markers: (bool, bool) = sqlx::query_as(
        "SELECT (SELECT contains_internal_marker FROM items WHERE id=md5('item-1')::uuid),
                (SELECT contains_internal_marker FROM items WHERE id=md5('item-2')::uuid)",
    )
    .fetch_one(&mut *tx)
    .await?;
    assert_eq!(markers, (false, true));
    let earlier = links::out_edges(&mut tx, document, None, 1).await?;
    assert_eq!(earlier[0].mentions, 101);
    assert_eq!(earlier[0].first_kind, "block");
    sqlx::query("UPDATE document_blocks SET removed_at=now() WHERE id=md5('block-10')::uuid")
        .execute(&mut *tx)
        .await?;
    let after_delete = links::out_edges(&mut tx, document, None, 1).await?;
    assert_eq!(after_delete[0].mentions, 100);
    assert_eq!(after_delete[0].first_kind, "comment");
    tx.rollback().await?;
    db.close().await?;
    Ok(())
}
