//! A live reader repairs a gap larger than a sync frame through paged HTTP without missing or repeated text.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use orch_api_contract::threads::{
    ItemPayload, THREAD_SYNC_MAX_CHANGES, THREAD_SYNC_MAX_LIMIT, ThreadItemList,
    ThreadItemListQuery, ThreadSyncGet, ThreadSyncGetQuery,
};
use orch_api_core::{registry::Registry, testing::TestApp};
use orch_db::sequences;
use orch_types::Id;
use realtime_support::{Client, Rig};
use serde_json::{Value, json};
use std::collections::BTreeMap;

async fn next(client: &mut Client, op: &str) -> Value {
    loop {
        let frame = client.frame().await;
        if frame["op"] == op {
            return frame;
        }
    }
}

async fn write_messages(rig: &Rig, thread: Id, count: i64) -> i64 {
    let segment = rig.segment_of(thread).await;
    let mut tx = rig.pool.begin().await.unwrap();
    let after: i64 = sqlx::query_scalar("UPDATE threads SET next_seq = next_seq + $2 WHERE node_id = $1 RETURNING next_seq - $2 - 1")
        .bind(thread).bind(count).fetch_one(&mut *tx).await.unwrap();
    // One batch sits just beyond the contract's changed-row boundary; this is a boundary fixture.
    sqlx::query("INSERT INTO items(id,thread_id,segment_id,kind,status,payload,seq_created,seq_updated) SELECT gen_random_uuid(),$1,$2,'agent_message','completed',jsonb_build_object('text', 'message-' || g::text, 'text_len', length('message-' || g::text), 'phase', NULL),$3+g,$3+g FROM generate_series(1,$4::bigint) g")
        .bind(thread).bind(segment).bind(after).bind(count).execute(&mut *tx).await.unwrap();
    let seq = after + count;
    sequences::notify(&mut tx, thread, seq).await.unwrap();
    tx.commit().await.unwrap();
    seq
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_live_gap_is_repaired_over_http_and_the_next_socket_row_follows_exactly() {
    let rig = Rig::with_time_held().await;
    let thread = rig.thread("Long gap").await;
    rig.server.engine.stop();
    let before = rig.rows(thread).await.seq;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    let feed = format!("thread:{thread}");
    client.sub(1, &feed, Some(before)).await;
    client.expect("subscribed").await;
    let count = THREAD_SYNC_MAX_CHANGES + 1;
    write_messages(&rig, thread, count).await;
    let resync = next(&mut client, "resync").await;
    assert_eq!(resync["feed"], feed);
    assert!(
        resync.to_string().len()
            < usize::try_from(orch_realtime_contract::version::BUFFER_MAX_BYTES).unwrap()
    );
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(
        rig.server.app.clone(),
        orch_testkit::clock::manual(),
        registry,
    );
    let gap = app
        .call(ThreadSyncGet, &[&thread])
        .token(&rig.token)
        .query(&ThreadSyncGetQuery {
            since: Some(before),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(!gap.complete);
    let whole = app
        .call(ThreadSyncGet, &[&thread])
        .token(&rig.token)
        .query(&ThreadSyncGetQuery {
            limit: Some(THREAD_SYNC_MAX_LIMIT),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let repaired_seq = whole.seq;
    let mut seen = BTreeMap::new();
    let mut older = whole.has_older;
    let mut oldest = whole.items.first().unwrap().seq_created;
    for item in whole.items {
        assert!(seen.insert(item.id, item).is_none());
    }
    while older {
        let page = app
            .call(ThreadItemList, &[&thread])
            .token(&rig.token)
            .query(&ThreadItemListQuery {
                before: Some(oldest),
                limit: Some(THREAD_SYNC_MAX_LIMIT),
                ..Default::default()
            })
            .send()
            .await
            .ok();
        older = page.has_older;
        if let Some(first) = page.items.first() {
            oldest = first.seq_created;
        }
        for item in page.items {
            assert!(
                seen.insert(item.id, item).is_none(),
                "HTTP repeated a timeline row"
            );
        }
    }
    let mut text: Vec<_> = seen
        .values()
        .filter_map(|item| match &item.payload {
            ItemPayload::AgentMessage(message) => Some((item.seq_created, message.text.clone())),
            _ => None,
        })
        .collect();
    text.sort();
    assert_eq!(
        text.into_iter().map(|(_, text)| text).collect::<Vec<_>>(),
        (1..=count)
            .map(|number| format!("message-{number}"))
            .collect::<Vec<_>>()
    );
    client.sub(1, &feed, Some(repaired_seq)).await;
    let subscribed = next(&mut client, "subscribed").await;
    assert_eq!(subscribed["mode"], "delta");
    write_messages(&rig, thread, 1).await;
    let frame = next(&mut client, "sync").await;
    assert_eq!(frame["prev"], repaired_seq);
    let rows = frame["data"]["items"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "only the new row follows the repair");
    let id: Id = rows[0]["id"].as_str().unwrap().parse().unwrap();
    assert!(!seen.contains_key(&id));
    assert_eq!(rows[0]["payload"]["text"], "message-1");
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_single_large_row_requests_resync_instead_of_exceeding_the_sockets_frame_budget() {
    const FRAME_BUDGET: i64 = 2_048;
    let rig = Rig::tuned(|tuning| tuning.buffer_max_bytes = FRAME_BUDGET).await;
    let thread = rig.thread("Large row").await;
    rig.server.engine.stop();
    let before = rig.rows(thread).await.seq;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .sub(1, &format!("thread:{thread}"), Some(before))
        .await;
    client.expect("subscribed").await;
    let segment = rig.segment_of(thread).await;
    let mut tx = rig.pool.begin().await.unwrap();
    let seq = sequences::next(&mut tx, thread).await.unwrap().unwrap();
    let text = "x".repeat(usize::try_from(FRAME_BUDGET * 2).unwrap());
    sqlx::query("INSERT INTO items(id,thread_id,segment_id,kind,status,payload,seq_created,seq_updated) VALUES($1,$2,$3,'agent_message','completed',$4,$5,$5)")
        .bind(orch_types::new_id()).bind(thread).bind(segment).bind(json!({"text": text, "text_len": text.len(), "phase": null})).bind(seq).execute(&mut *tx).await.unwrap();
    sequences::notify(&mut tx, thread, seq).await.unwrap();
    tx.commit().await.unwrap();
    let frame = next(&mut client, "resync").await;
    assert!(frame.to_string().len() <= usize::try_from(FRAME_BUDGET).unwrap());
    client.ping().await;
    rig.server.stop().await;
}
