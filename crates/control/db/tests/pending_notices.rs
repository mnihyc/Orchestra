//! Deferred notices preserve values and retries while reading and consuming only one indexed page.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::queries::{
    hierarchy, resources,
    threads::{self, notices},
};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, Json, new_id};
use serde_json::json;

async fn thread(db: &TestDb) -> Id {
    let mut conn = db.pool.acquire().await.unwrap();
    let owner = resources::testing::insert_user(&mut conn, "notice-reader")
        .await
        .unwrap();
    let org = new_id();
    let project = new_id();
    let topic = new_id();
    let thread = new_id();
    hierarchy::testing::insert_node(&mut conn, org, "org", None, "Office", owner)
        .await
        .unwrap();
    hierarchy::testing::insert_node(&mut conn, project, "project", Some(org), "Project", owner)
        .await
        .unwrap();
    hierarchy::testing::insert_node(&mut conn, topic, "topic", Some(project), "Topic", owner)
        .await
        .unwrap();
    assert!(
        threads::insert_thread(
            &mut conn,
            thread,
            topic,
            "Conversation",
            owner,
            clock::start()
        )
        .await
        .unwrap()
    );
    thread
}

#[tokio::test]
async fn a_queue_pages_without_losing_tail_values_or_new_arrivals_and_deduplicates_beyond_its_head()
{
    let db = TestDb::fresh().await;
    let thread = thread(&db).await;
    let mut conn = db.pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO thread_pending_notices(thread_id,notice) SELECT $1,jsonb_build_object('id','entry-'||n,'text','A complete notice '||n) FROM generate_series(1,130) n ORDER BY n")
        .bind(thread).execute(&mut *conn).await.unwrap();
    let first = notices::page(&mut conn, thread, None).await.unwrap();
    assert_eq!(first.len(), notices::PAGE_SIZE);
    assert!(
        notices::contains(&mut conn, thread, "entry-130")
            .await
            .unwrap()
    );
    assert!(
        !notices::append(&mut conn, thread, &json!({"id":"entry-130","text":"retry"}))
            .await
            .unwrap()
    );
    let second = notices::page(&mut conn, thread, Some(first.last().unwrap().position))
        .await
        .unwrap();
    let third = notices::page(&mut conn, thread, Some(second.last().unwrap().position))
        .await
        .unwrap();
    assert_eq!((second.len(), third.len()), (64, 2));
    assert_eq!(third[1].notice["text"], "A complete notice 130");
    let row = threads::thread(&mut conn, thread).await.unwrap().unwrap();
    assert_eq!(row.pending_notices.as_array().unwrap().len(), 64);
    assert!(
        notices::append(
            &mut conn,
            thread,
            &json!({"id":"later","text":"Arrived after the snapshot"})
        )
        .await
        .unwrap()
    );
    let said: Vec<String> = first
        .iter()
        .map(|row| row.notice["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(notices::forget(&mut conn, thread, &said).await.unwrap());
    assert!(notices::contains(&mut conn, thread, "later").await.unwrap());
    let next = notices::page(&mut conn, thread, None).await.unwrap();
    assert_eq!(next[0].notice["id"], "entry-65");
    assert_eq!(next.len(), 64);
    let preview: Json = sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        preview,
        json!(
            next.iter()
                .map(|row| row.notice.clone())
                .collect::<Vec<_>>()
        )
    );
    drop(conn);
    db.close().await.unwrap();
}

#[tokio::test]
async fn legacy_writes_keep_every_payload_duplicate_and_order_in_rows_with_a_bounded_preview() {
    let db = TestDb::fresh().await;
    let thread = thread(&db).await;
    let values: Vec<Json> = (0..513)
        .map(|n| {
            if n % 11 == 0 {
                Json::Null
            } else {
                json!({"id":format!("duplicate-{}",n%7),"value":[n,"你好",{"kept":true}]})
            }
        })
        .collect();
    let mut conn = db.pool.acquire().await.unwrap();
    sqlx::query("UPDATE threads SET pending_notices=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(json!(values))
        .execute(&mut *conn)
        .await
        .unwrap();
    let mut after = None;
    let mut actual = Vec::new();
    loop {
        let page = notices::page(&mut conn, thread, after).await.unwrap();
        assert!(page.len() <= notices::PAGE_SIZE);
        if page.is_empty() {
            break;
        }
        after = page.last().map(|row| row.position);
        actual.extend(page.into_iter().map(|row| row.notice));
    }
    assert_eq!(actual, values);
    let preview: Json = sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(preview, json!(&values[..64]));
    drop(conn);
    db.close().await.unwrap();
}

#[tokio::test]
async fn concurrent_retries_append_once_and_other_native_lanes_do_not_hide_general_notices() {
    let db = TestDb::fresh().await;
    let thread = thread(&db).await;
    let notice = json!({"id":"same-outcome","text":"Keep me once"});
    let mut first = db.pool.acquire().await.unwrap();
    let mut second = db.pool.acquire().await.unwrap();
    let (a, b) = tokio::join!(
        notices::append(&mut first, thread, &notice),
        notices::append(&mut second, thread, &notice)
    );
    assert_ne!(a.unwrap(), b.unwrap());
    sqlx::query("INSERT INTO thread_pending_notices(thread_id,notice) SELECT $1,jsonb_build_object('id','agent-history:'||md5(n::text)::uuid,'text','For a different lane') FROM generate_series(1,130) n")
        .bind(thread).execute(&mut *first).await.unwrap();
    assert!(
        notices::append(
            &mut first,
            thread,
            &json!({"id":"general-after-other-lanes","text":"Still reachable"})
        )
        .await
        .unwrap()
    );
    let page = notices::for_segment(&mut first, thread, new_id())
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!(page[1].notice["id"], "general-after-other-lanes");
    drop(first);
    drop(second);
    db.close().await.unwrap();
}

#[tokio::test]
async fn a_notice_queued_before_its_segment_keeps_following_that_segments_native_identity() {
    let db = TestDb::fresh().await;
    let thread = thread(&db).await;
    let segment = new_id();
    let notice =
        json!({"id":format!("agent-history:{segment}"),"text":"Keep the complete handoff"});
    let mut conn = db.pool.acquire().await.unwrap();
    assert!(notices::append(&mut conn, thread, &notice).await.unwrap());
    assert!(
        notices::for_segment(&mut conn, thread, segment)
            .await
            .unwrap()
            .is_empty()
    );
    // Imported history supplies a real segment identity without inventing local execution resources.
    sqlx::query("INSERT INTO segments(id,thread_id,continuation_id,ordinal,agent_id,provider_id,environment_id,env_generation,arrangement,credential_path,started_at,ended_at,import_fidelity,seq_updated)
      SELECT $2,node_id,active_continuation_id,1,NULL,NULL,NULL,0,'together','native',now(),now(),'structured',0 FROM threads WHERE node_id=$1")
      .bind(thread).bind(segment).execute(&mut *conn).await.unwrap();
    for native in [None, Some("measured-native-session"), None] {
        sqlx::query("UPDATE segments SET native_session_ref=$2 WHERE id=$1")
            .bind(segment)
            .bind(native)
            .execute(&mut *conn)
            .await
            .unwrap();
        let page = notices::for_segment(&mut conn, thread, segment)
            .await
            .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].notice, notice);
    }
    drop(conn);
    db.close().await.unwrap();
}

#[tokio::test]
async fn delivery_receipts_consume_idless_values_and_retries_preserve_the_next_page() {
    let db = TestDb::fresh().await;
    let thread = thread(&db).await;
    let turn = new_id();
    let mut conn = db.pool.acquire().await.unwrap();
    // Isolate receipt lookup from unrelated turn prerequisites while exercising the real suppliers.
    sqlx::query("CREATE TEMP TABLE turns(id uuid PRIMARY KEY,thread_id uuid NOT NULL,pending_notice_positions bigint[])")
        .execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO turns(id,thread_id) VALUES($1,$2)")
        .bind(turn)
        .bind(thread)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO thread_pending_notices(thread_id,notice) SELECT $1,CASE WHEN n%2=0 THEN 'null'::jsonb ELSE jsonb_build_object('text','No identifier','value',n) END FROM generate_series(1,138) n ORDER BY n")
        .bind(thread).execute(&mut *conn).await.unwrap();
    assert_eq!(
        notices::delivery_positions(&mut conn, thread, turn)
            .await
            .unwrap(),
        None
    );
    let first = notices::page(&mut conn, thread, None).await.unwrap();
    let positions: Vec<_> = first.iter().map(|row| row.position).collect();
    notices::pin_delivery(&mut conn, thread, turn, &positions)
        .await
        .unwrap();
    assert!(
        notices::append(
            &mut conn,
            thread,
            &json!({"id":"arrived-later","text":"Keep until delivered"})
        )
        .await
        .unwrap()
    );
    assert_eq!(
        notices::delivery_page(&mut conn, thread, &positions)
            .await
            .unwrap(),
        first
    );
    assert!(
        notices::forget_positions(&mut conn, thread, &positions)
            .await
            .unwrap()
    );
    let next = notices::page(&mut conn, thread, None).await.unwrap();
    let later_positions: Vec<_> = next.iter().map(|row| row.position).collect();
    notices::pin_delivery(&mut conn, thread, turn, &later_positions)
        .await
        .unwrap();
    assert_eq!(
        notices::delivery_positions(&mut conn, thread, turn)
            .await
            .unwrap(),
        Some(positions.clone())
    );
    assert!(
        notices::delivery_page(&mut conn, thread, &positions)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !notices::forget_positions(&mut conn, thread, &positions)
            .await
            .unwrap()
    );
    assert_eq!(notices::page(&mut conn, thread, None).await.unwrap(), next);
    assert!(
        notices::forget_positions(&mut conn, thread, &later_positions)
            .await
            .unwrap()
    );
    let tail = notices::page(&mut conn, thread, None).await.unwrap();
    assert_eq!(tail.len(), 11);
    assert_eq!(tail.last().unwrap().notice["id"], "arrived-later");
    let old_tail: Vec<_> = tail[..10].iter().map(|row| row.position).collect();
    assert!(
        notices::forget_positions(&mut conn, thread, &old_tail)
            .await
            .unwrap()
    );
    assert_eq!(
        notices::page(&mut conn, thread, None).await.unwrap().len(),
        1
    );
    assert!(
        notices::contains(&mut conn, thread, "arrived-later")
            .await
            .unwrap()
    );
}
