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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn masking_opaque_native_keys_preserves_the_typed_raw_call_shape() {
    let world = World::new().await;
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"fixture","regex":"input"}]),
        )
        .await;
    let thread = world.thread("Masked native keys").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    world.say(thread,Event::decode("item.started",json!({"turn_id":start.turn_id,"native_ref":"native-keys","stream":"main","parent_native_ref":null,"kind":"tool_call","payload":{
        "title":"Operation","started_at":"2026-11-14T12:00:00Z","duration_ms":null,
        "tool":{"kind":"other","name":"operation","arguments":{"input":"content"},"result":null},
        "raw":{"name":"operation","input":{"inline":"request","total_len":7,"truncated":false,"blob":null},"output":null,"is_error":false}
    }})).unwrap()).await;
    eventually("the native keyed payload to commit", || async {
        let count: i64 = world
            .scalar(
                "SELECT count(*) FROM items WHERE thread_id=$1 AND native_ref='native-keys'",
                thread,
            )
            .await;
        count == 1
    })
    .await;
    let payload: orch_types::Json = world
        .scalar(
            "SELECT payload FROM items WHERE thread_id=$1 AND native_ref='native-keys'",
            thread,
        )
        .await;
    assert!(
        payload.pointer("/raw/input/inline").is_some(),
        "structural raw-call keys stay typed"
    );
    assert!(
        payload.pointer("/tool/arguments/input").is_none(),
        "opaque presentation keys are masked too"
    );
    let _: orch_api_contract::threads::ToolCallPayload = serde_json::from_value(payload).unwrap();
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forced_controller_stop_does_not_release_private_context_before_the_native_field_ends() {
    let world = World::tuned(|config| {
        config.stream_flush = Duration::from_secs(3_600);
        config.interrupt_force = Duration::ZERO;
    })
    .await;
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"fixture","regex":"BEGIN.*END"}]),
        )
        .await;
    let thread = world.thread("Native end still unknown").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    world
        .say(
            thread,
            message_began(start.turn_id, "private-until-native-end"),
        )
        .await;
    world
        .say(
            thread,
            message_grew(
                "private-until-native-end",
                0,
                "ordinary BEGIN private continuation",
            ),
        )
        .await;
    eventually("the possible match to be privately retained",||async {
        let count:i64=world.scalar("SELECT count(*) FROM item_redaction_holds h JOIN items i ON i.id=h.item_id WHERE i.thread_id=$1",thread).await;count>0
    }).await;
    world
        .engine()
        .interrupt(
            &orch_authz::Caller::session(world.ada),
            thread,
            start.turn_id,
        )
        .await
        .unwrap();
    world.wait_turn(start.turn_id, "interrupted").await;
    let text:String=world.scalar("SELECT payload->>'text' FROM items WHERE thread_id=$1 AND native_ref='private-until-native-end'",thread).await;
    assert!(!text.contains("private continuation"));
    let held:i64=world.scalar("SELECT count(*) FROM item_redaction_holds h JOIN items i ON i.id=h.item_id WHERE i.thread_id=$1",thread).await;
    assert!(
        held > 0,
        "a forced controller end is not a native field boundary"
    );
    world
        .say(
            thread,
            Event::decode(
                "turn.completed",
                json!({"turn_id":start.turn_id,"status":"interrupted","error":null}),
            )
            .unwrap(),
        )
        .await;
    eventually("the confirmed native boundary to release the safe final suffix",||async {
        let count:i64=world.scalar("SELECT count(*) FROM item_redaction_holds h JOIN items i ON i.id=h.item_id WHERE i.thread_id=$1",thread).await;count==0
    }).await;
    world.shutdown().await;
}
