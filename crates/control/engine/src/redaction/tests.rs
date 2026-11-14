//! Native timeline capture obeys the same masking policy as other stored text.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "regression assertions"
)]
use crate::test_common::{World, eventually, message_began, message_done, message_grew};
use orch_link_contract::events::Event;
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn configured_patterns_mask_native_started_updated_and_completed_payloads_before_storage() {
    let world = World::new().await;
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"fixture", "regex":"PRIVATE-CAPTURE-[0-9]{4}"}]),
        )
        .await;
    let thread = world.thread("Masked native content").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    world.say(thread, Event::decode("item.started", json!({
        "turn_id":start.turn_id, "native_ref":"masked-item", "stream":"main", "parent_native_ref":null, "kind":"agent_message",
        "payload":{"text":"PRIVATE-CAPTURE-1234", "text_len":20, "phase":null}
    })).unwrap()).await;
    eventually("the masked start to commit", || async {
        let text: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'text' FROM items WHERE thread_id=$1 AND native_ref='masked-item'",
        )
        .bind(thread)
        .fetch_optional(world.pool.pool())
        .await
        .unwrap();
        text.is_some_and(|text| !text.contains("PRIVATE-CAPTURE-1234") && text.contains('*'))
    })
    .await;
    world.say(thread, Event::decode("item.updated", json!({"native_ref":"masked-item", "payload":{"text":"second PRIVATE-CAPTURE-1234", "text_len":27}})).unwrap()).await;
    eventually("the masked update to commit", || async {
        let text:String=world.scalar("SELECT payload->>'text' FROM items WHERE thread_id=$1 AND native_ref='masked-item'",thread).await;
        text.starts_with("second ") && !text.contains("PRIVATE-CAPTURE-1234")
    }).await;
    world
        .say(
            thread,
            message_done("masked-item", "last PRIVATE-CAPTURE-1234"),
        )
        .await;
    eventually("the masked completion to commit", || async {
        let text:String=world.scalar("SELECT payload->>'text' FROM items WHERE thread_id=$1 AND native_ref='masked-item'",thread).await;
        text.starts_with("last ") && !text.contains("PRIVATE-CAPTURE-1234")
    }).await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pattern_split_between_native_deltas_never_reaches_the_live_feed_or_durable_text() {
    let world = World::tuned(|config| config.stream_flush = Duration::from_secs(3_600)).await;
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"fixture", "regex":"PRIVATE-CAPTURE-[0-9]{4}"}]),
        )
        .await;
    let thread = world.thread("Masked live text").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    world
        .say(thread, message_began(start.turn_id, "masked-stream"))
        .await;
    world
        .say(thread, message_grew("masked-stream", 0, "prefix PRIVATE-"))
        .await;
    world
        .say(
            thread,
            message_grew(
                "masked-stream",
                15,
                "CAPTURE-1234 and enough ordinary progress to release a safe prefix",
            ),
        )
        .await;
    eventually("ordinary progress to remain live", || async {
        world
            .live
            .deltas()
            .iter()
            .filter(|delta| delta.thread_id == thread)
            .map(|delta| delta.data.as_str())
            .collect::<String>()
            .contains("ordinary progress")
    })
    .await;
    let shown = world
        .live
        .deltas()
        .iter()
        .filter(|delta| delta.thread_id == thread)
        .map(|delta| delta.data.as_str())
        .collect::<String>();
    assert!(
        !shown.contains("PRIVATE-CAPTURE-1234"),
        "native fragments must be masked together"
    );
    world
        .say(
            thread,
            message_done(
                "masked-stream",
                "prefix PRIVATE-CAPTURE-1234 and enough ordinary progress to release a safe prefix",
            ),
        )
        .await;
    eventually("masked durable text to be complete",||async {
        let text:String=world.scalar("SELECT payload->>'text' FROM items WHERE thread_id=$1 AND native_ref='masked-stream'",thread).await;
        text.contains("safe prefix") && !text.contains("PRIVATE-CAPTURE-1234")
    }).await;
    world.shutdown().await;
}
