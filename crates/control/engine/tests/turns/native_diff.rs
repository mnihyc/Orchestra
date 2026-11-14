//! A native turn diff is a replaceable summary, independent of the actual file-tool history.
use super::*;
use orch_link_contract::events::{EmittedItemKind, Event, ItemCompleted, ItemStarted};
use orch_link_contract::foreign::ItemPayload;
use orch_types::{Id, Json};
use serde_json::json;

async fn snapshot(world: &World, thread: Id, turn: Id, changes: Json) {
    let payload = ItemPayload::Marker(serde_json::from_value(json!({"marker":"harness_changed","data":{
        "program":"Codex","change":"turn_diff","subject":null,"previous":{"state":"unsupported"},
        "expected":{"state":"unsupported"},"value":{"changes":changes},"changed":true,"diverged":false,
        "next_turn":false,"turn_id":turn,"reported_at":world.clock.now()
    }})).unwrap());
    world
        .say(
            thread,
            Event::ItemStarted(ItemStarted {
                turn_id: turn,
                native_ref: "turn-diff".into(),
                stream: "main".into(),
                parent_native_ref: None,
                kind: EmittedItemKind::Marker,
                payload: payload.clone(),
            }),
        )
        .await;
    world
        .say(
            thread,
            Event::ItemCompleted(ItemCompleted {
                native_ref: "turn-diff".into(),
                status: orch_agent_contract::events::ItemStatus::Completed,
                payload,
                output_blob: None,
            }),
        )
        .await;
}

#[tokio::test]
async fn the_changed_files_follow_the_latest_turn_diff_snapshot_and_keep_exact_tool_history() {
    let world = World::new().await;
    let thread = world.thread("Native diff").await;
    let (turn, _) = world.running_turn(thread, "Edit files").await;
    let environment: Id = sqlx::query_scalar("SELECT environment_id FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    let change = |path: &str| json!({"path":path,"change":"modify","old_path":null,"diff":"@@ -1 +1 @@\n-old\n+new\n","added":1,"removed":1,"binary":false,"truncated":false});
    let payload = ItemPayload::ToolCall(serde_json::from_value(json!({
        "title":"Edit file","started_at":world.clock.now(),"duration_ms":1,
        "raw":{"name":"fileChange","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},"output":null,"is_error":false},
        "tool":{"kind":"file_change","changes":[change("earlier.txt")]}
    })).unwrap());
    world
        .say(
            thread,
            Event::ItemStarted(ItemStarted {
                turn_id: turn.turn_id,
                native_ref: "file-tool".into(),
                stream: "main".into(),
                parent_native_ref: None,
                kind: EmittedItemKind::ToolCall,
                payload: payload.clone(),
            }),
        )
        .await;
    world
        .say(
            thread,
            Event::ItemCompleted(ItemCompleted {
                native_ref: "file-tool".into(),
                status: orch_agent_contract::events::ItemStatus::Completed,
                payload,
                output_blob: None,
            }),
        )
        .await;
    for paths in [
        vec!["latest.txt"],
        vec!["replaced.txt", "other.txt"],
        vec![],
    ] {
        snapshot(
            &world,
            thread,
            turn.turn_id,
            json!(paths.iter().map(|path| change(path)).collect::<Vec<_>>()),
        )
        .await;
        eventually("the native diff replaces the changed files", || async {
            let mut conn = world.pool.acquire().await.unwrap();
            let rows = orch_db::queries::environments::workspace::latest_changes(
                &mut conn,
                environment,
                Some(thread),
                Some(turn.turn_id),
                None,
            )
            .await
            .unwrap();
            let found: std::collections::BTreeSet<_> = rows
                .iter()
                .filter_map(|row| row.change["path"].as_str())
                .collect();
            found == paths.iter().copied().collect()
        })
        .await;
    }
    let mut conn = world.pool.acquire().await.unwrap();
    let original: Id =
        sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND native_ref='file-tool'")
            .bind(thread)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let history = orch_db::queries::environments::workspace::latest_changes(
        &mut conn,
        environment,
        Some(thread),
        Some(turn.turn_id),
        Some(original),
    )
    .await
    .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].change["path"], "earlier.txt");
}
