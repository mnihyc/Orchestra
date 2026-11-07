#![recursion_limit = "256"]
//! Recorded background work reaches ordinary thread reads and both feeds with current End authority.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
mod realtime_support;
use orch_link_contract::background::{
    BackgroundTask, BackgroundTaskKind, BackgroundTaskState, BackgroundTasks,
};
use orch_link_contract::events::Event;
use orch_types::Id;
use realtime_support::{Client, Rig};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

async fn owned_work(rig: &Rig) -> (Id, BackgroundTask, Id) {
    let thread = rig.thread("Named background work").await;
    let turn = rig.send(thread, "Begin").await;
    rig.begin_message(thread, &turn, "foreground").await;
    sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{capabilities,background_commands}','true'::jsonb) WHERE id=$1")
        .bind(rig.agent).execute(rig.pool.pool()).await.unwrap();
    rig.say(thread,Event::decode("item.started",json!({
        "turn_id":turn.turn_id,"native_ref":"owned-task","stream":"main","parent_native_ref":null,"kind":"tool_call",
        "payload":{"tool":{"kind":"command","command":"sleep 30","cwd":"/workspace/web","exit_code":null,"background":true,"process_id":"task","actions":[],
            "output":{"inline":"","total_len":0,"truncated":false,"blob":null}},"title":"Build web","started_at":"2026-11-06T00:00:00Z","duration_ms":null,
            "raw":{"name":"exec","input":{"inline":"sleep 30","total_len":8,"truncated":false,"blob":null},"output":null,"is_error":false}}
    })).unwrap()).await;
    let task = BackgroundTask {
        id: "task".into(),
        run_id: Some("run".into()),
        native_item_ref: "owned-task".into(),
        origin_turn: turn.turn_id,
        kind: BackgroundTaskKind::Command,
        label: "Build web".into(),
        state: BackgroundTaskState::Running,
        exit_code: None,
        can_end: true,
        name: None,
        nickname: None,
        role: None,
    };
    update(rig, thread, task.clone()).await;
    rig.finish_turn(thread, &turn, "foreground", "Waiting for Build.")
        .await;
    let input: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id=$1")
        .bind(turn.turn_id)
        .fetch_one(rig.pool.pool())
        .await
        .unwrap();
    (thread, task, input)
}
async fn update(rig: &Rig, thread: Id, task: BackgroundTask) {
    rig.say(
        thread,
        Event::BackgroundTasks(BackgroundTasks {
            self_waking: false,
            native_idle: false,
            tasks: vec![task],
            native_wake: None,
        }),
    )
    .await;
}
async fn sync(rig: &Rig, thread: Id, token: &str) -> Value {
    reqwest::Client::new()
        .get(format!(
            "http://{}/api/v1/threads/{thread}/sync",
            rig.server.addr
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_recorded_roster_reaches_thread_sync_and_later_realtime_state_frames() {
    let rig = Rig::new().await;
    let (thread, mut task, input) = owned_work(&rig).await;
    let read = sync(&rig, thread, &rig.token).await;
    let work = &read["thread"]["background"]["tasks"][0];
    assert_eq!(work["label"], "Build web");
    assert_eq!(work["input_id"], json!(input));
    assert_eq!(work["author"]["id"], json!(rig.ada));
    assert_eq!(work["state"], "running");
    assert_eq!(work["can_end"], true);
    assert!(work["end_id"].is_null());
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &format!("thread:{thread}"), None).await;
    let first = client.until("sync").await.pop().unwrap();
    assert_eq!(
        first["data"]["thread"]["background"]["tasks"][0]["id"],
        work["id"]
    );
    task.label = "Build web progress".into();
    update(&rig, thread, task).await;
    let began = Instant::now();
    loop {
        assert!(
            began.elapsed() < realtime_support::PATIENCE,
            "a task-only update must advance the thread state projection"
        );
        if let Some(realtime_support::Heard::Frame(frame)) =
            client.hear_within(Duration::from_millis(200)).await
            && frame["data"]["thread"]["background"]["tasks"][0]["label"] == "Build web progress"
        {
            break;
        }
    }
    drop(client);
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_native_feed_keeps_recorded_names_and_a_viewer_cannot_end_the_work() {
    let rig = Rig::new().await;
    let (thread, _, input) = owned_work(&rig).await;
    let (viewer, token) = rig.person("observer").await;
    rig.give(viewer, "viewer").await;
    let read = sync(&rig, thread, &token).await;
    assert_eq!(read["thread"]["background"]["tasks"][0]["can_end"], false);
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client
        .sub(1, &format!("thread:{thread}:background"), None)
        .await;
    client.expect("subscribed").await;
    let frame = client.expect("event").await;
    assert_eq!(frame["data"]["wake"]["tasks"][0]["label"], "Build web");
    assert_eq!(frame["data"]["wake"]["tasks"][0]["input_id"], json!(input));
    assert_eq!(frame["data"]["wake"]["tasks"][0]["can_end"], false);
    assert!(frame["data"]["list_ref"].is_null());
    drop(client);
    rig.server.stop().await;
}
