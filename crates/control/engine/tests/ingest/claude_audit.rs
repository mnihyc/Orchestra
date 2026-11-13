//! Actual adapter output crosses the production link decoder and transactional native observation reader.
use super::*;
use orch_adapter_claude_code::events::{EventMapper, MapperSettings};
use orch_adapter_claude_code::stream::Frame;
use orch_agent_contract::events::SessionEvent;
use std::sync::Arc;

async fn native_events(
    world: &World,
    thread: Id,
    segment: Id,
    seq: &mut i64,
    events: &[SessionEvent],
) {
    let mut frames = Vec::new();
    for event in events {
        let wire = serde_json::to_value(event).unwrap();
        let converted = Event::decode(event.kind(), wire["body"].clone()).unwrap();
        *seq += 1;
        frames.push(delivered(world, *seq, &converted));
    }
    if !frames.is_empty() {
        apply(world, thread, segment, &frames).await;
    }
}
fn native(kind: &str, extra: Json) -> Frame {
    let mut frame = json!({"type":"system","subtype":kind,"session_id":"claude-audit"});
    frame
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    Frame::from_value(frame)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claude_model_and_live_hook_states_audit_once_through_the_shared_native_observer() {
    let world = World::new().await;
    let thread = world.thread("Claude native audit").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(
                &world,
                1,
                &Event::SessionOpened(SessionOpened {
                    native_session_ref: "claude-audit".into(),
                    resumed: false,
                }),
            ),
            delivered(&world, 2, &turn_began(start.turn_id)),
        ],
    )
    .await;
    let mut seq = 2;
    let mut mapper = EventMapper::new(
        MapperSettings {
            directory: "/work".into(),
            cwd: "/work".into(),
            raw_events: false,
        },
        Arc::clone(&world.clock),
    );
    let _ = mapper.begin_turn_with_model(start.turn_id, &start.model, None);
    let model = mapper.on_frame(&native("init", json!({"model":"actual-native-model"})));
    native_events(&world, thread, segment, &mut seq, &model).await;
    let count = world
        .count(
            "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
            thread,
        )
        .await;
    assert_eq!(
        count, 1,
        "the actual init-model producer supplies structured audit/context facts"
    );
    let turn: Json = world
        .scalar(
            "SELECT usage_detail FROM turns WHERE thread_id=$1 ORDER BY ordinal LIMIT 1",
            thread,
        )
        .await;
    assert_eq!(turn["effective_model"], "actual-native-model");
    let began=mapper.on_frame(&native("hook_started",json!({"hook_id":"live-hook","hook_name":"Check files","hook_event":"PreToolUse","uuid":"begin"})));
    native_events(&world, thread, segment, &mut seq, &began).await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        2,
        "an actual committed running hook is audited before it finishes"
    );
    let changed=mapper.on_frame(&native("hook_progress",json!({"hook_id":"live-hook","hook_name":"Check files again","hook_event":"PreToolUse","uuid":"progress","stdout":"PRIVATE-HOOK-OUTPUT"})));
    native_events(&world, thread, segment, &mut seq, &changed).await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        3,
        "a changed live marker goes through the same audit observer"
    );
    let ended=mapper.on_frame(&native("hook_response",json!({"hook_id":"live-hook","hook_name":"Check files again","hook_event":"PreToolUse","uuid":"end","outcome":"error","exit_code":1,"stderr":"PRIVATE-HOOK-OUTPUT"})));
    native_events(&world, thread, segment, &mut seq, &ended).await;
    let before = world
        .count(
            "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
            thread,
        )
        .await;
    assert_eq!(
        before, 4,
        "a failed native hook is still an auditable reported change"
    );
    assert_eq!(world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND payload->'data'->>'change'='hook'",thread).await,1,
        "start/update/failure remain one native item");
    native_events(&world, thread, segment, &mut seq, &began).await;
    native_events(&world, thread, segment, &mut seq, &ended).await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        before,
        "durable native event replay does not append a second audit fact"
    );
    let mut idle = EventMapper::new(
        MapperSettings {
            directory: "/work".into(),
            cwd: "/work".into(),
            raw_events: false,
        },
        Arc::clone(&world.clock),
    );
    let observed=idle.on_frame(&native("hook_response",json!({"hook_id":"idle-hook","hook_name":"Idle check","hook_event":"Stop","outcome":"cancelled"})));
    native_events(&world, thread, segment, &mut seq, &observed).await;
    let idle_row:Json=world.scalar("SELECT jsonb_build_object('actor',actor_kind,'for_user',for_user_id,'turn',details->'turn_id','state',details->'state') FROM audit_log WHERE node_id=$1 AND action='harness.changed' AND details->>'subject'='Idle check'",thread).await;
    assert_eq!(
        idle_row,
        json!({"actor":"system","for_user":null,"turn":null,"state":"stopped"}),
        "an idle cancelled hook does not borrow the controller's already-active turn or person"
    );
    let rows: Vec<Json> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
    )
    .bind(thread)
    .fetch_all(world.pool.pool())
    .await
    .unwrap();
    assert!(
        rows.iter().all(|row| row["program"] != "Claude Code"),
        "the real opened profile supplies the audit program"
    );
    assert!(
        rows.iter()
            .any(|row| row["change"] == "hook" && row["state"] == "failed")
    );
    assert!(!serde_json::to_string(&rows).unwrap().contains("PRIVATE-"));
}
