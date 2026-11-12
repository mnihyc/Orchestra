//! Owned task facts for lifecycle consumers; native event production has separate real-harness controls.
use super::World;
use orch_link_contract::background::{
    BackgroundTask, BackgroundTaskKind, BackgroundTaskState, BackgroundTasks,
};
use orch_link_contract::events::Event;
use orch_types::Id;
use serde_json::json;

pub async fn command(world: &World, thread: Id, turn: Id) -> BackgroundTask {
    world.say(thread,Event::decode("item.started",json!({
        "turn_id":turn,"native_ref":"retained-background","stream":"main","parent_native_ref":null,"kind":"tool_call",
        "payload":{"tool":{"kind":"command","command":"fixture command","cwd":"/workspace","exit_code":null,"background":true,"process_id":"task","actions":[],
          "output":{"inline":"","total_len":0,"truncated":false,"blob":null}},"title":"Retained task","started_at":world.clock.time(),"duration_ms":null,
          "raw":{"name":"exec","input":{"inline":"","total_len":0,"truncated":false,"blob":null},"output":null,"is_error":false}}
    })).unwrap()).await;
    let task = BackgroundTask {
        id: "task".into(),
        run_id: Some("run".into()),
        native_item_ref: "retained-background".into(),
        origin_turn: turn,
        kind: BackgroundTaskKind::Command,
        label: "Retained task".into(),
        state: BackgroundTaskState::Running,
        exit_code: None,
        can_end: true,
        name: None,
        nickname: None,
        role: None,
    };
    update(world, thread, task.clone()).await;
    task
}

pub async fn update(world: &World, thread: Id, task: BackgroundTask) {
    let reference = task.native_item_ref.clone();
    let key = json!([task.id, task.run_id]).to_string();
    let mut expected = json!({"background_tasks":{}});
    expected["background_tasks"][key] = json!({"task":{"state":task.state}});
    world
        .say(
            thread,
            Event::BackgroundTasks(BackgroundTasks {
                self_waking: false,
                native_idle: false,
                tasks: vec![task],
                native_wake: None,
            }),
        )
        .await;
    super::eventually("the owned task fact to commit",||async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND native_ref=$2 AND payload @> $3)")
            .bind(thread).bind(&reference).bind(&expected).fetch_one(world.pool.pool()).await.unwrap()
    }).await;
}
