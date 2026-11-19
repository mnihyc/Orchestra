//! Real socket projection resets, scoped read notices and frozen-prefix append exclusion.

mod continuation_read_support;
mod realtime_support;

use continuation_read_support::{Proof, Retained, retained, select};
use orch_api_contract::threads::continuations::ThreadReadSelector;
use orch_types::Id;
use realtime_support::{Client, PATIENCE, Rig};
use serde_json::{Value, json};
use std::time::Duration;

async fn history() -> Proof<(Rig, Id, Retained)> {
    let rig = Rig::new().await;
    let thread = rig.thread("Retained native read projections").await;
    for (words, reference, answer) in [
        ("First", "first-answer", "First answer"),
        ("Second", "second-answer", "Second answer"),
    ] {
        let turn = rig.send(thread, words).await;
        rig.begin_message(thread, &turn, reference).await;
        rig.finish_turn(thread, &turn, reference, answer).await;
    }
    realtime_support::eventually(
        "all native terminal/source work finishes before the projection fixture",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1
            AND (end_work IS NOT NULL OR files_end_due OR files_work_pending))",
            )
            .bind(thread)
            .fetch_one(rig.pool.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    let view = retained(&rig.pool, thread, rig.ada, rig.clock.time()).await?;
    Ok((rig, thread, view))
}

async fn until(client: &mut Client, mut accepts: impl FnMut(&Value) -> bool) -> Proof<Value> {
    Ok(tokio::time::timeout(PATIENCE, async {
        loop {
            let frame = client.frame().await;
            if accepts(&frame) {
                return frame;
            }
        }
    })
    .await?)
}

async fn subscribe(client: &mut Client, thread: Id, view: Option<Id>, seq: i64) -> Proof<Value> {
    let mut request = json!({"op":"sub","id":1,"feed":format!("thread:{thread}"),"since":seq,"projection_revision":1});
    if let Some(view) = view {
        request["continuation_id"] = json!(view);
    }
    client.send(request).await;
    let subscribed = client.expect("subscribed").await;
    assert_eq!(subscribed["mode"], "delta");
    client.expect("presence").await;
    Ok(subscribed)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_away_and_back_execution_change_resets_only_the_follow_active_socket() -> Proof {
    let (rig, thread, view) = history().await?;
    let head = rig.rows(thread).await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(rig.pool.pool())
        .await?;
    let mut following = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut old = Client::signed_in(rig.server.addr, &rig.token).await;
    let first = subscribe(&mut following, thread, None, head.seq).await?;
    let explicit = subscribe(&mut old, thread, Some(view.root), head.seq).await?;
    assert_eq!(first["projection"]["follows_active"], true);
    assert_eq!(explicit["projection"]["follows_active"], false);
    select(&rig.pool, thread, view.child, rig.clock.time()).await?;
    let switched = select(&rig.pool, thread, view.root, rig.clock.time()).await?;
    let reset = until(&mut following, |frame| frame["op"] == "resync").await?;
    assert_eq!(reset["id"], 1);
    let update = until(&mut old, |frame| {
        assert_ne!(
            frame["op"], "resync",
            "an explicit retained scope remains the same projection"
        );
        frame["op"] == "sync" && frame["seq"].as_i64().is_some_and(|seq| seq >= switched)
    })
    .await?;
    assert_eq!(
        update["data"]["projection"]["viewed_continuation_id"],
        json!(view.root)
    );
    assert_eq!(update["data"]["projection"]["projection_revision"], 1);
    assert_eq!(update["data"]["projection"]["execution_revision"], 3);
    assert_eq!(update["data"]["projection"]["follows_active"], false);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(rig.pool.pool())
        .await?;
    assert_eq!(
        after, before,
        "view subscriptions and resets cannot author autonomous input"
    );
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scoped_read_notice_names_the_retained_history_without_advancing_the_active_one() -> Proof
{
    let (rig, thread, view) = history().await?;
    let seq = select(&rig.pool, thread, view.child, rig.clock.time()).await?;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(2, "user", None).await;
    client.expect("subscribed").await;
    let response = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/threads/{thread}/read",
            rig.server.addr
        ))
        .bearer_auth(&rig.token)
        .json(&json!({"seq":seq,"continuation_id":view.root,"projection_revision":1}))
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let acknowledged: orch_api_contract::personal::ThreadReadReply = response.json().await?;
    assert_eq!(acknowledged.last_read_seq, seq);
    let event = until(&mut client, |frame| {
        frame["op"] == "event" && frame["data"]["what"] == "continuation_read_state"
    })
    .await?;
    assert_eq!(
        event["data"],
        json!({"what":"continuation_read_state","thread_id":thread,
        "continuation_id":view.root,"last_read_seq":seq,
        "thread_display":acknowledged.thread_display.as_ref().expect("the reader still views this thread")})
    );
    let active: Option<i64> = sqlx::query_scalar(
        "SELECT last_read_seq FROM thread_reads
        WHERE thread_id=$1 AND user_id=$2 AND continuation_id=$3",
    )
    .bind(thread)
    .bind(rig.ada)
    .bind(view.child)
    .fetch_optional(rig.pool.pool())
    .await?;
    assert_eq!(active, None);
    // The actually included prefix IDs remain discoverable without a delivered-input collection.
    let prefix = rig
        .server
        .engine
        .sync_view_in(
            &mut *rig.pool.acquire().await?,
            thread,
            Some(rig.ada),
            None,
            100,
            ThreadReadSelector {
                continuation_id: Some(view.child),
                projection_revision: None,
                ..Default::default()
            },
        )
        .await?;
    assert!(prefix.inputs.is_empty());
    assert!(prefix.items.iter().any(|item| item.id == view.prefix_item));
    assert!(prefix.items.iter().any(
        |item| matches!(&item.payload,orch_api_contract::threads::ItemPayload::UserMessage(message)
        if message.input_id==Some(view.first_input))
    ));
    assert!(!prefix.items.iter().any(
        |item| matches!(&item.payload,orch_api_contract::threads::ItemPayload::UserMessage(message)
        if message.input_id==Some(view.before_input))
    ));
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_controlled_live_append_reaches_its_owner_but_not_a_frozen_inherited_prefix() -> Proof {
    let (rig, thread, view) = history().await?;
    let head = rig.rows(thread).await;
    let mut source = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut frozen = Client::signed_in(rig.server.addr, &rig.token).await;
    subscribe(&mut source, thread, Some(view.root), head.seq).await?;
    subscribe(&mut frozen, thread, Some(view.child), head.seq).await?;
    let (turn, offset): (Option<Id>, i64) =
        sqlx::query_as("SELECT turn_id,(payload->>'text_len')::bigint FROM items WHERE id=$1")
            .bind(view.prefix_item)
            .fetch_one(rig.pool.pool())
            .await?;
    let extra = "CONTROLLED-LIVE-APPEND";
    // This tests the live-sink projection boundary, not native event admission or persistence.
    rig.server
        .realtime
        .sink()
        .delta(&orch_engine::live::LiveDelta {
            thread_id: thread,
            item_id: view.prefix_item,
            turn_id: turn,
            stream: "main".into(),
            field: orch_link_contract::events::DeltaField::Text,
            offset,
            len: i64::try_from(extra.len())?,
            data: extra.into(),
        });
    let appended = until(&mut source, |frame| frame["op"] == "delta").await?;
    assert_eq!(appended["item_id"], json!(view.prefix_item));
    assert_eq!(appended["data"], extra);
    assert!(
        frozen.is_quiet_for(Duration::from_millis(500)).await,
        "frozen prefix bytes cannot accept live suffix appends"
    );
    rig.server.stop().await;
    Ok(())
}
