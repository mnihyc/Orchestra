//! Actual recorded images survive ingestion and fold across the requested and later turns only.
use super::*;
use orch_api_contract::source::FileChangeImage;
use orch_link_contract::events::{EmittedItemKind, ItemCompleted, ItemStarted};
use orch_link_contract::foreign::ItemPayload;

fn image(text: Option<&str>) -> serde_json::Value {
    match text {
        Some(text) => json!({"state":"text","text":text}),
        None => json!({"state":"absent"}),
    }
}
fn change(path: &str, before: Option<&str>, after: Option<&str>) -> serde_json::Value {
    json!({"path":path,"change": if before.is_none() {"add"} else if after.is_none() {"delete"} else {"modify"},
        "old_path":null,"diff":"a bounded displayed diff","added":1,"removed":1,"binary":false,"truncated":true,
        "before":image(before),"after":image(after)})
}
#[tokio::test]
async fn actual_three_turn_file_records_keep_first_preimages_final_writes_and_no_public_images() {
    let world = World::new().await;
    world.machine.handler.on(CommandKind::FsRecord, |_| {
        Ok(CommandResult::FsRecord(fallback()))
    });
    let thread = world.thread("Touched files").await;
    let mut first = None;
    for (nth, changes) in [
        vec![change("code", Some("zero"), Some("one"))],
        vec![
            change("code", Some("one"), Some("two")),
            change("new", None, Some("added")),
        ],
        vec![change("gone", Some("deleted contents"), None)],
    ]
    .into_iter()
    .enumerate()
    {
        world
            .engine()
            .submit_input(&Caller::session(world.ada), thread, queued("Change files"))
            .await
            .unwrap();
        let started = world.turn_start(thread, nth).await;
        if first.is_none() {
            first = Some(started.turn_id);
        }
        world.say(thread, common::turn_began(started.turn_id)).await;
        let payload = ItemPayload::ToolCall(serde_json::from_value(json!({
            "title":"Changed files","started_at":world.clock.now(),"duration_ms":1,
            "raw":{"name":"file tool","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},"output":null,"is_error":false},
            "tool":{"kind":"file_change","changes":changes}
        })).unwrap());
        let reference = format!("files-{nth}");
        world
            .say(
                thread,
                Event::ItemStarted(ItemStarted {
                    turn_id: started.turn_id,
                    native_ref: reference.clone(),
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
                    native_ref: reference,
                    status: orch_agent_contract::events::ItemStatus::Completed,
                    payload,
                    output_blob: None,
                }),
            )
            .await;
        world
            .say(
                thread,
                common::turn_ended(started.turn_id, TurnOutcome::Completed),
            )
            .await;
        world.wait_turn(started.turn_id, "completed").await;
    }
    let mut conn = world.pool.acquire().await.unwrap();
    let row = threads::thread(&mut conn, thread).await.unwrap().unwrap();
    let plan = orch_engine::jobs::restore::touched::load(
        &mut conn,
        row.environment_id.unwrap(),
        thread,
        first.unwrap(),
        &row.workspace_dir,
        "live_tree".into(),
    )
    .await
    .unwrap();
    assert_eq!(plan.paths.len(), 3);
    let code = plan.paths.iter().find(|path| path.path == "code").unwrap();
    assert_eq!(
        code.before,
        Some(FileChangeImage::Text {
            text: "zero".into()
        })
    );
    assert_eq!(
        code.after,
        Some(FileChangeImage::Text { text: "two".into() })
    );
    assert_ne!(
        code.after,
        Some(FileChangeImage::Text {
            text: "changed since".into()
        })
    );
    assert_eq!(
        plan.paths
            .iter()
            .find(|path| path.path == "new")
            .unwrap()
            .before,
        Some(FileChangeImage::Absent)
    );
    assert_eq!(
        plan.paths
            .iter()
            .find(|path| path.path == "gone")
            .unwrap()
            .after,
        Some(FileChangeImage::Absent)
    );
    let shown = world.sync(thread).await.items;
    let mut changed_paths = 0;
    for item in &shown {
        if let orch_api_contract::threads::ItemPayload::ToolCall(call) = &item.payload
            && let orch_api_contract::threads::ToolPayload::FileChange { changes } = &call.tool
        {
            for change in changes {
                changed_paths += 1;
                assert!(change.before.is_none() && change.after.is_none());
                assert_eq!(change.diff.as_deref(), Some("a bounded displayed diff"));
            }
        }
    }
    assert_eq!(changed_paths, 4, "every original file change stays visible");
    let public = serde_json::to_value(shown).unwrap();
    let public = public.to_string();
    assert!(
        !public.contains("deleted contents"),
        "complete private images must not become public timeline diffs"
    );
    assert!(
        orch_engine::jobs::restore::touched::load(
            &mut conn,
            row.environment_id.unwrap(),
            world.root,
            first.unwrap(),
            &row.workspace_dir,
            "off".into()
        )
        .await
        .is_err()
    );
}
