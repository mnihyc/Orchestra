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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_terminal_error_details_are_masked_in_storage_and_every_public_turn_field() {
    let world = World::new().await;
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"fixture","regex":"PRIVATE-CAPTURE-[0-9]{4}"}]),
        )
        .await;
    let thread = world.thread("Masked native failure").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    world.say(thread, Event::decode("turn.completed", json!({
        "turn_id":start.turn_id,"status":"failed","error":{
            "code":"harness_error","message":"Failed PRIVATE-CAPTURE-1234","said":"Native PRIVATE-CAPTURE-1234",
            "reason":"Reason PRIVATE-CAPTURE-1234","retryable":false,
            "limit":{"url":"https://example.invalid/?key=PRIVATE-CAPTURE-1234","kind":"PRIVATE-CAPTURE-1234"}
        }
    })).unwrap()).await;
    eventually("the native failure to commit", || async {
        world
            .scalar::<String>("SELECT state FROM turns WHERE id=$1", start.turn_id)
            .await
            == "failed"
    })
    .await;
    let stored: orch_types::Json = world
        .scalar("SELECT error FROM turns WHERE id=$1", start.turn_id)
        .await;
    assert!(
        !stored.to_string().contains("PRIVATE-CAPTURE-1234"),
        "ordinary turn errors cross the masking boundary before storage"
    );
    let view = world.sync(thread).await;
    assert!(
        !serde_json::to_string(&view.turns)
            .unwrap()
            .contains("PRIVATE-CAPTURE-1234"),
        "all public error fields, including said, stay masked"
    );
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn public_native_urls_working_directories_and_nested_metadata_are_content_not_name_exemptions()
 {
    let world = World::new().await;
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"fixture","regex":"PRIVATE-CAPTURE-[0-9]{4}"}]),
        )
        .await;
    let thread = world.thread("Private native locations").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    let output = json!({"inline":"content","total_len":7,"truncated":false,"blob":null});
    for (native, tool) in [
        (
            "private-url",
            json!({"kind":"web_fetch","url":"https://example.invalid/?key=PRIVATE-CAPTURE-1234","title":null,"status":200,"content":output}),
        ),
        (
            "private-cwd",
            json!({"kind":"command","command":"build","cwd":"/work/PRIVATE-CAPTURE-1234","exit_code":null,"output":output,"process_id":null,"background":false,"actions":[]}),
        ),
    ] {
        world.say(thread,Event::decode("item.started",json!({"turn_id":start.turn_id,"native_ref":native,"stream":"main","parent_native_ref":null,"kind":"tool_call","payload":{
            "title":"A private operation","started_at":"2026-11-14T21:45:00Z","duration_ms":null,"tool":tool,
            "raw":{"name":"operation","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},"output":null,"is_error":false},
            "metadata":{"path":"PRIVATE-CAPTURE-1234","url":"PRIVATE-CAPTURE-1234","kind":"PRIVATE-CAPTURE-1234","id":"PRIVATE-CAPTURE-1234"}
        }})).unwrap()).await;
    }
    eventually("both native locations to commit", || async {
        world.scalar::<i64>("SELECT count(*) FROM items WHERE thread_id=$1 AND native_ref IN ('private-url','private-cwd')",thread).await == 2
    }).await;
    let payloads: Vec<orch_types::Json> = sqlx::query_scalar("SELECT payload FROM items WHERE thread_id=$1 AND native_ref IN ('private-url','private-cwd') ORDER BY native_ref").bind(thread).fetch_all(world.pool.pool()).await.unwrap();
    assert!(
        payloads
            .iter()
            .all(|payload| !payload.to_string().contains("PRIVATE-CAPTURE-1234")),
        "typed roles, not key spellings, decide what remains public"
    );
    for payload in payloads {
        let _: orch_api_contract::threads::ToolCallPayload =
            serde_json::from_value(payload).unwrap();
    }
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_approval_presentation_is_masked_while_original_operands_remain_private_and_decidable()
 {
    use orch_api_contract::approvals::{ApprovalDecision, HarnessOptionDecision};
    use orch_authz::Caller;
    use orch_link_contract::commands::CommandKind;
    let world = World::new().await;
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"fixture","regex":"PRIVATE-CAPTURE-[0-9]{4}"}]),
        )
        .await;
    let thread = world.thread("A private native approval").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    world.say(thread,Event::decode("approval.requested",json!({"turn_id":start.turn_id,"request_ref":"private-approval","title":"Run PRIVATE-CAPTURE-1234","request":{
        "request_kind":"command","detail":{"command":"printf PRIVATE-CAPTURE-1234","cwd":"/workspace/web","reason":"PRIVATE-CAPTURE-1234"},
        "options":[{"id":"allow_once","label":"Allow once","style":"allow","scope":"once"},{"id":"deny","label":"Deny","style":"deny","scope":null}]
    }})).unwrap()).await;
    world.wait_state(thread, "waiting_approval").await;
    let approval: orch_types::Id = world
        .scalar("SELECT id FROM approvals WHERE thread_id=$1", thread)
        .await;
    let (title, request): (String, orch_types::Json) =
        sqlx::query_as("SELECT title,request FROM approvals WHERE id=$1")
            .bind(approval)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(
        !title.contains("PRIVATE-CAPTURE-1234")
            && !request.to_string().contains("PRIVATE-CAPTURE-1234"),
        "ordinary approval data contains only its masked presentation"
    );
    let encrypted: Vec<u8> = world
        .scalar("SELECT native_request FROM approvals WHERE id=$1", approval)
        .await;
    let binding = format!("native-approval:{approval}");
    let original = crate::raw_events::master_key()
        .unwrap()
        .open(&encrypted, binding.as_bytes())
        .unwrap();
    let original: orch_types::Json = serde_json::from_slice(&original).unwrap();
    assert_eq!(
        original["detail"]["command"], "printf PRIVATE-CAPTURE-1234",
        "execution keeps the original under its private binding"
    );
    let view = world.sync(thread).await;
    assert!(
        !serde_json::to_string(&view.items)
            .unwrap()
            .contains("PRIVATE-CAPTURE-1234")
    );
    let decided = world
        .engine()
        .decide_approval(
            &Caller::session(world.ada),
            approval,
            &ApprovalDecision::Option(HarnessOptionDecision {
                option_id: "allow_once".into(),
                note: None,
            }),
        )
        .await
        .unwrap();
    assert_eq!(decided.state, "approved");
    world.wait_ran(CommandKind::ApprovalResolve, 1).await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_secret_split_between_diagnostic_frames_is_masked_at_its_join_before_public_storage() {
    let world = World::new().await;
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"fixture","regex":"PRIVATE-CAPTURE-[0-9]{4}"}]),
        )
        .await;
    let thread = world.thread("A private diagnostic stream").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    for (index, part) in ["PRIVATE-", "CAPTURE-1234 and safe trace text"]
        .into_iter()
        .enumerate()
    {
        world.say(thread,Event::decode("raw.frame",json!({"n":index+1,"direction":"in","text_complete":true,
            "text":[{"stream":"trace-message:text","pointer":"/params/delta","end":index==1}],
            "payload":{"method":"message.delta","params":{"delta":part}}
        })).unwrap()).await;
    }
    let segment = world.segment_of(thread).await;
    eventually("both diagnostic frames to commit", || async {
        world
            .scalar::<i64>(
                "SELECT count(*) FROM raw_events WHERE segment_id=$1",
                segment,
            )
            .await
            == 2
    })
    .await;
    let mut conn = world.pool.pool().acquire().await.unwrap();
    let row = orch_db::queries::threads::thread(&mut conn, thread)
        .await
        .unwrap()
        .unwrap();
    let page = crate::raw_events::page(&mut conn, &row, segment, None, 10)
        .await
        .unwrap();
    let mut frames = page.events;
    frames.sort_by_key(|frame| frame.n);
    let joined = frames
        .iter()
        .filter_map(|frame| {
            frame
                .payload
                .pointer("/params/delta")
                .and_then(orch_types::Json::as_str)
        })
        .collect::<String>();
    assert!(
        !joined.contains("PRIVATE-CAPTURE-1234") && !joined.contains("CAPTURE-1234"),
        "the diagnostic copy cannot reconstruct either joined match or trailing secret fragment"
    );
    assert!(
        joined.contains("safe trace text"),
        "joined masking retains the ordinary diagnostic content"
    );
    // The ordinary trace rows themselves, rather than only a read projection, contain the safe join.
    let rows = orch_db::queries::raw_events::newest(&mut conn, segment, None, 10)
        .await
        .unwrap();
    let mut stored = rows
        .into_iter()
        .map(|row| {
            let bytes = zstd::decode_all(row.payload.as_slice()).unwrap();
            let value: orch_types::Json = serde_json::from_slice(&bytes).unwrap();
            (row.n, value)
        })
        .collect::<Vec<_>>();
    stored.sort_by_key(|(n, _)| *n);
    let stored = stored
        .iter()
        .filter_map(|(_, frame)| {
            frame
                .pointer("/params/delta")
                .and_then(orch_types::Json::as_str)
        })
        .collect::<String>();
    assert!(!stored.contains("PRIVATE-CAPTURE-1234") && !stored.contains("CAPTURE-1234"));
    drop(conn);
    world
        .say(
            thread,
            crate::test_common::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_native_values_never_escape_into_decode_logs() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing::instrument::WithSubscriber;
    #[derive(Clone)]
    struct Log(Arc<Mutex<Vec<u8>>>);
    impl Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let world = World::new().await;
    world
        .set(
            world.topic,
            "redaction.patterns",
            json!([{"name":"fixture","regex":"PRIVATE-CAPTURE-[0-9]{4}"}]),
        )
        .await;
    let thread = world.thread("Safe native decode diagnostics").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    let segment = world.segment_of(thread).await;
    let log = Log(Arc::new(Mutex::new(Vec::new())));
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();
    let engine = world.engine();
    let mut tx = world.pool.begin().await.unwrap();
    let events = [orch_link::streams::DeliveredEvent {
        seq: 900,
        at: world.clock.time(),
        kind: "item.started".into(),
        body: json!({
            "turn_id":start.turn_id,"native_ref":"invalid-kind","stream":"main","parent_native_ref":null,
            "kind":"PRIVATE-CAPTURE-1234","payload":{"text":"ordinary","text_len":8,"phase":null}
        }),
    }];
    engine
        .apply_stream_events(
            &mut tx,
            world.machine_id,
            &orch_link_contract::streams::Stream::Session {
                thread_id: thread,
                segment_id: segment,
            },
            &events,
        )
        .with_subscriber(subscriber)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let recorded = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        recorded.contains("event"),
        "the malformed event still produces a diagnostic"
    );
    assert!(
        !recorded.contains("PRIVATE-CAPTURE-1234"),
        "the serde source is native text, not safe diagnostic metadata"
    );
    assert_eq!(
        world
            .scalar::<i64>(
                "SELECT count(*) FROM items WHERE thread_id=$1 AND native_ref='invalid-kind'",
                thread
            )
            .await,
        0
    );
    drop(engine);
    world.shutdown().await;
}
