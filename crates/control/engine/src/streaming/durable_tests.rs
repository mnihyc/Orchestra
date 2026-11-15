//! A streamed stretch survives losing the engine process before its coalesced item write.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a regression case fails by panicking"
)]
use super::*;
use crate::EngineParts;
use crate::test_common::{World, eventually, message_began, message_grew};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn text_already_shown_to_readers_can_be_restored_by_a_new_engine_before_item_completion() {
    let world = World::tuned(|config| config.stream_flush = Duration::from_secs(3_600)).await;
    let thread = world.thread("Durable progress").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    world
        .say(thread, message_began(start.turn_id, "restart-buffer"))
        .await;
    world
        .say(
            thread,
            message_grew("restart-buffer", 0, "Visible and durable"),
        )
        .await;
    eventually("the stretch to be displayed", || async {
        world
            .live
            .deltas()
            .iter()
            .any(|delta| delta.thread_id == thread && delta.data == "Visible and durable")
    })
    .await;
    let original = world.engine();
    // A new engine has none of the old process's growing-text memory. Only its shared database survives.
    let restarted = Engine::new(EngineParts {
        db: original.db().clone(),
        clock: Arc::clone(&original.inner.clock),
        config: original.config().clone(),
        commands: Arc::clone(&original.inner.commands),
        access: Arc::clone(&original.inner.access),
        live: Arc::clone(&original.inner.live),
    });
    let mut tx = restarted.db().begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    threads::lock(&mut tx, thread).await.unwrap();
    let mut unit = Unit::new(&mut tx, original.inner.clock.now());
    restarted.write_held_of(&mut unit, thread).await.unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    let restored: String = world
        .scalar(
            "SELECT payload->>'text' FROM items WHERE thread_id=$1 AND native_ref='restart-buffer'",
            thread,
        )
        .await;
    assert_eq!(restored, "Visible and durable");
    drop(restarted);
    drop(original);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn coalesced_command_progress_keeps_the_smaller_accepted_inline_byte_allowance() {
    use orch_link_contract::events::Event;
    use serde_json::json;
    let world = World::tuned(|config| config.stream_flush = Duration::from_secs(3_600)).await;
    let thread = world.thread("A small native output allowance").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    world.say(thread,Event::decode("item.started",json!({"turn_id":start.turn_id,"native_ref":"small-output","stream":"main","parent_native_ref":null,"kind":"tool_call","payload":{
        "title":"Build","started_at":"2026-11-14T12:00:00Z","duration_ms":null,
        "tool":{"kind":"command","command":"build","cwd":"/work","exit_code":null,"process_id":null,"background":false,"actions":[],"output":{"inline":"","total_len":0,"truncated":false,"blob":null,"inline_limit":1000}},
        "raw":{"name":"command","input":{"inline":"build","total_len":5,"truncated":false,"blob":null},"output":null,"is_error":false}
    }})).unwrap()).await;
    world.say(thread,Event::decode("item.delta",json!({"native_ref":"small-output","field":"output","offset":0,"len":2000,"data":"x".repeat(2000)})).unwrap()).await;
    eventually("the accepted output to become live", || async {
        world
            .live
            .deltas()
            .iter()
            .any(|delta| delta.thread_id == thread && delta.field == DeltaField::Output)
    })
    .await;
    let engine = world.engine();
    let mut tx = engine.db().begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    threads::lock(&mut tx, thread).await.unwrap();
    let mut unit = Unit::new(&mut tx, engine.inner.clock.now());
    engine.write_held_of(&mut unit, thread).await.unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    let payload: Json = world
        .scalar(
            "SELECT payload FROM items WHERE thread_id=$1 AND native_ref='small-output'",
            thread,
        )
        .await;
    assert_eq!(
        payload["tool"]["output"]["inline"].as_str().unwrap().len(),
        1000
    );
    assert_eq!(payload["tool"]["output"]["total_len"], 2000);
    drop(engine);
    world.shutdown().await;
}
