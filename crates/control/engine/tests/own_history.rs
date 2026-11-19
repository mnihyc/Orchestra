//! The agent is taught its exact conversation path and can read, search and download its earlier work after compaction.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;
use orch_link_contract::commands::OrchTool;
use orch_link_contract::events::{Event, TurnOutcome, TurnPhase, TurnPhaseKind};
use orch_types::Time;
use serde_json::{Value, json};
use shared::*;
use std::sync::Arc;

#[tokio::test]
async fn the_taught_path_reads_searches_and_downloads_own_history_before_and_after_compaction() {
    let world = a_world_whose_agent_takes_tools().await;
    let _older_twin = world.thread("Long work").await;
    let thread = world.thread("Long work").await;
    talk(
        &world,
        thread,
        "Remember amberneedle from the first turn.",
        "The first result is recorded.",
    )
    .await;
    let path = world
        .engine()
        .topic_material(&mut world.pool.acquire().await.unwrap(), thread)
        .await
        .unwrap()
        .conversation_path;
    let line = format!(
        "- Your own conversation is {path}: read_shared reads any of its turns, including those from before your context was shortened; search_shared with this path finds turns by words or a pattern; download_shared copies it whole (Markdown, or json records) into your workspace for your own tools."
    );
    assert!(world.turn_starts(thread)[0].text.contains(&line));
    let (active, _) = world
        .running_turn(thread, "Shorten the current context")
        .await;
    world
        .say(
            thread,
            Event::TurnPhase(TurnPhase {
                turn_id: active.turn_id,
                kind: TurnPhaseKind::Compacting,
                at: Time::new(world.clock.now()),
            }),
        )
        .await;
    common::eventually("the compacted context requires a refresh",||async {
        let sent:Value=sqlx::query_scalar("SELECT s.context_sent FROM segments s JOIN threads t ON t.current_segment_id=s.id WHERE t.node_id=$1").bind(thread).fetch_one(&mut *world.pool.acquire().await.unwrap()).await.unwrap();sent["resend"]==true
    }).await;
    world
        .say(
            thread,
            Event::TurnPhase(TurnPhase {
                turn_id: active.turn_id,
                kind: TurnPhaseKind::Working,
                at: Time::new(world.clock.now()),
            }),
        )
        .await;
    world
        .say(
            thread,
            common::turn_ended(active.turn_id, TurnOutcome::Completed),
        )
        .await;
    world.wait_turn(active.turn_id, "completed").await;
    talk(
        &world,
        thread,
        "Continue from the recorded first result",
        "Continuing after the context was shortened.",
    )
    .await;
    assert!(world.turn_starts(thread)[2].text.contains(&line));
    for detail in ["brief", "normal", "full"] {
        let result = ok(call(
            &world,
            thread,
            OrchTool::ReadShared,
            json!({"path":path,"from":1,"to":1,"detail":detail}),
        )
        .await);
        assert!(
            result["text"].as_str().unwrap().contains("amberneedle"),
            "{detail}: {result}"
        );
    }
    let found = ok(call(
        &world,
        thread,
        OrchTool::SearchShared,
        json!({"path":path,"query":"amberneedle","snippets":true}),
    )
    .await);
    let hits = found["hits"].as_array().unwrap();
    assert!(!hits.is_empty(), "{found}");
    assert!(
        hits.iter().all(|hit| hit["path"] == path),
        "the own path narrows search to one entry: {found}"
    );
    assert!(found.to_string().contains("amberneedle"));
    let parcels = Arc::new(MemoryParcels::default());
    world.engine().attach_parcels(parcels.clone());
    for format in ["markdown", "json"] {
        let downloaded = ok(call(
            &world,
            thread,
            OrchTool::DownloadShared,
            json!({"path":path,"format":format}),
        )
        .await);
        assert_eq!(downloaded["status"], "ready");
        let destination = downloaded["path"].as_str().unwrap();
        let bytes = parcels
            .files
            .lock()
            .unwrap()
            .get(&(thread, destination.to_owned()))
            .unwrap()
            .clone();
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            text.contains("amberneedle")
                && text.contains("Continuing after the context was shortened"),
            "{format}: {text}"
        );
        if format == "json" {
            let records: Value = serde_json::from_str(&text).unwrap();
            let total: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
                .bind(thread)
                .fetch_one(&mut *world.pool.acquire().await.unwrap())
                .await
                .unwrap();
            assert_eq!(
                i64::try_from(records["turns"].as_array().unwrap().len()).unwrap(),
                total,
                "the whole conversation, including pre-compaction turns"
            );
        }
    }
    let other = world.thread("Not shared").await;
    let denied = call(
        &world,
        thread,
        OrchTool::ReadShared,
        json!({"path":format!("id:{other}")}),
    )
    .await;
    assert_eq!(denied.error.unwrap().code, "not_found");
}
