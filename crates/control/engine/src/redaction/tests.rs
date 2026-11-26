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

#[test]
fn repaired_native_keys_keep_mask_marks_on_the_changed_member_without_counting_them_as_source_text()
{
    let masker = orch_chat::redaction::Redactor::new(&[], Vec::new());
    let source = json!({"title":"Output","started_at":"2026-11-17T00:00:00Z","duration_ms":null,
        "tool":{"kind":"other","name":"terminal","arguments":null,"result":{
            "field\0":{"url":"https://example.invalid/A\0B"},
            "field�":{"url":"https://safe.example.invalid/"}
        }},"raw":{"name":"terminal","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},"output":null,"is_error":false}});
    let presented = crate::redaction::presentation::item("tool_call", &source, &masker).unwrap();
    let mut payload = presented.value;
    payload["masked_fields"] = json!(presented.masked);
    let (public, count, _) = orch_db::queries::threads::storable_item_payload(&payload);
    assert_eq!(
        count, 2,
        "only the native key and value contribute source replacements"
    );
    let marks = public["masked_fields"].as_array().unwrap();
    assert!(
        marks
            .iter()
            .any(|path| path == "/tool/result/field� (2)/url")
    );
    assert!(
        !marks.iter().any(|path| path == "/tool/result/field�/url"),
        "the unchanged sibling keeps its own controls"
    );
    for path in marks.iter().filter_map(serde_json::Value::as_str) {
        assert!(
            public.pointer(path).is_some(),
            "every public mark still names its actual member: {path}"
        );
    }
    assert_eq!(
        public["tool"]["result"]["field� (2)"]["url"],
        "https://example.invalid/A�B"
    );
    assert_eq!(
        public["tool"]["result"]["field�"]["url"],
        "https://safe.example.invalid/"
    );
}

#[test]
fn repairing_a_native_address_does_not_make_its_changed_public_copy_an_executable_link() {
    let masker = orch_chat::redaction::Redactor::new(&[], Vec::new());
    let source = json!({"title":"Open","started_at":"2026-11-15T22:00:00Z","duration_ms":null,
        "tool":{"kind":"web_fetch","url":"https://example.invalid/A\0B","title":null,"status":200,
        "content":{"inline":"Captured page","total_len":13,"truncated":false,"blob":null}}});
    let presented = crate::redaction::presentation::item("tool_call", &source, &masker).unwrap();
    assert!(
        presented.masked.iter().any(|path| path == "/tool/url"),
        "a repaired address uses the same existing unavailable-open boundary"
    );
    let (public, count, _) = orch_db::queries::threads::storable_item_payload(&presented.value);
    assert_eq!(public["tool"]["url"], "https://example.invalid/A�B");
    assert_eq!(count, 1);
}

#[test]
fn native_limit_scopes_keep_their_typed_meaning_while_the_reported_model_family_is_masked() {
    let masker = orch_chat::redaction::Redactor::new(
        &[orch_chat::redaction::Pattern {
            name: "fixture".into(),
            regex: "plan|PRIVATE-CAPTURE-[0-9]{4}".into(),
        }],
        Vec::new(),
    );
    let public = crate::redaction::presentation::error(
        &json!({"code":"model_rate_limit","message":"Wait","retryable":true,
        "limit":{"scope":"plan","model_family":"PRIVATE-CAPTURE-1234","window_minutes":60}}),
        &masker,
    )
    .unwrap();
    assert_eq!(public["limit"]["scope"], "plan");
    assert!(!public.to_string().contains("PRIVATE-CAPTURE-1234"));
    serde_json::from_value::<orch_api_contract::threads::TurnLimit>(public["limit"].clone())
        .unwrap();
}

#[test]
fn typed_content_addresses_remain_references_while_identical_opaque_text_is_masked() {
    let hash = orch_types::Hash::of(b"public captured image").to_string();
    let masker = orch_chat::redaction::Redactor::new(
        &[orch_chat::redaction::Pattern {
            name: "fixture".into(),
            regex: hash.clone(),
        }],
        Vec::new(),
    );
    let source = json!({"title":"Captured image","started_at":"2026-11-14T12:00:00Z","duration_ms":null,
        "tool":{"kind":"image","blob":hash,"media_type":"image/png","name":null,"caption":null},
        "metadata":{"blob":hash}});
    let public = crate::redaction::presentation::item("tool_call", &source, &masker)
        .unwrap()
        .value;
    assert_eq!(
        public["tool"]["blob"], hash,
        "a typed address must remain a usable content reference"
    );
    assert_ne!(
        public["metadata"]["blob"], hash,
        "an opaque member with the same spelling has no reference role"
    );
    serde_json::from_value::<orch_api_contract::threads::ToolCallPayload>(public).unwrap();
}

#[test]
fn encoded_native_arguments_are_masked_as_people_decode_them_without_changing_captured_byte_positions()
 {
    let encoded = r#"{"value":"\u0050RIVATE-CAPTURE-1234","safe":"尾部"}"#;
    let masker = orch_chat::redaction::Redactor::new(
        &[orch_chat::redaction::Pattern {
            name: "fixture".into(),
            regex: "PRIVATE-CAPTURE-[0-9]{4}".into(),
        }],
        Vec::new(),
    );
    let source = json!({"title":"Native call","started_at":"2026-11-14T12:00:00Z","duration_ms":null,
        "tool":{"kind":"other","name":"operation","arguments":null,"result":null},
        "raw":{"name":"operation","input":{"inline":encoded,"total_len":encoded.len(),"truncated":false,"blob":null},"output":null,"is_error":false}});
    let public = crate::redaction::presentation::item("tool_call", &source, &masker)
        .unwrap()
        .value;
    let captured = public
        .pointer("/raw/input/inline")
        .and_then(orch_types::Json::as_str)
        .unwrap();
    assert_eq!(captured.len(), encoded.len());
    let decoded: orch_types::Json = serde_json::from_str(captured).unwrap();
    assert!(
        !decoded.to_string().contains("PRIVATE-CAPTURE-1234"),
        "the decoded value must cross the same native text boundary"
    );
    assert_eq!(decoded["safe"], "尾部");
}

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
    eventually("masked durable text to be complete", || async {
        let text: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'text' FROM items WHERE thread_id=$1 AND native_ref='masked-stream'",
        )
        .bind(thread)
        .fetch_optional(world.pool.pool())
        .await
        .unwrap();
        text.is_some_and(|text| {
            text.contains("safe prefix") && !text.contains("PRIVATE-CAPTURE-1234")
        })
    })
    .await;
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
    eventually("the actual native fragment to be privately retained",||async {
        let count:i64=world.scalar("SELECT count(*) FROM item_redaction_holds h JOIN items i ON i.id=h.item_id WHERE i.thread_id=$1 AND i.native_ref='private-until-native-end' AND h.part_len>0",thread).await;count>0
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
async fn stopping_during_a_native_text_query_cannot_leave_its_worker_asleep_after_shutdown() {
    let world = World::tuned(|config| config.stream_flush = Duration::from_secs(3_600)).await;
    let engine = world.engine();
    {
        // Force the stop between the worker's initial stop check and registration of its next wait.
        let mut gate = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE item_stream_parts IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *gate)
            .await
            .unwrap();
        let writer = engine.write_held_text();
        tokio::pin!(writer);
        assert!(futures::poll!(writer.as_mut()).is_pending());
        engine.stop();
        gate.rollback().await.unwrap();
        tokio::time::timeout(Duration::from_secs(20), writer.as_mut())
            .await
            .expect("the native text worker missed a stop that arrived during its query");
    }
    drop(engine);
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
async fn incomplete_trace_parts_keep_secrets_private_and_declared_new_fields_readable() {
    for missing in [true, false] {
        let world = World::new().await;
        world
            .set(
                world.topic,
                "redaction.patterns",
                json!([{"name":"fixture","regex":"PRIVATE-CAPTURE-[0-9]{4}"}]),
            )
            .await;
        let thread = world.thread("An incomplete private diagnostic field").await;
        let (turn, _) = world.running_turn(thread, "Begin").await;
        // The missing second frame contained CAPTURE-. Its surviving suffix still carries private data.
        for (n, stream, start, end, text) in [
            (1, "private", true, false, "PRIVATE-"),
            (3, "private", false, true, "1234"),
            (4, "safe", true, false, "ordinary safe "),
            (5, "safe", false, true, "trace text"),
        ] {
            if n == 3 && !missing {
                world.say(thread,Event::decode("raw.frame",json!({"n":2,"direction":"in","text_complete":false,"text":[],"payload":{"delta":"CAPTURE-"}})).unwrap()).await;
            }
            world.say(thread,Event::decode("raw.frame",json!({"n":n,"direction":"in","text_complete":true,
            "text":[{"stream":stream,"pointer":"/delta","start":start,"end":end}],"payload":{"delta":text}})).unwrap()).await;
        }
        let segment = world.segment_of(thread).await;
        eventually("all retained diagnostic fragments to commit", || async {
            world
                .scalar::<i64>(
                    "SELECT count(*) FROM raw_events WHERE segment_id=$1",
                    segment,
                )
                .await
                == if missing { 4 } else { 5 }
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
        for frame in page.events.iter().filter(|frame| frame.n <= 3) {
            assert!(
                !frame.payload.to_string().contains("1234"),
                "a missing match context cannot expose the surviving private suffix"
            );
            assert!(frame.masking.is_some(), "the withheld context says why");
        }
        let safe = page
            .events
            .iter()
            .filter(|frame| frame.n >= 4)
            .map(|frame| {
                frame
                    .payload
                    .get("delta")
                    .and_then(orch_types::Json::as_str)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(
            safe.contains(&"ordinary safe ") && safe.contains(&"trace text"),
            "a new native field has its own complete matching context"
        );
        drop(conn);
        world
            .say(
                thread,
                Event::decode(
                    "turn.completed",
                    json!({"turn_id":turn.turn_id,"status":"completed","error":null}),
                )
                .unwrap(),
            )
            .await;
        world.wait_turn(turn.turn_id, "completed").await;
        world.shutdown().await;
    }
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
    let opening = world.opening_of(thread).await;
    let mut tx = world.pool.begin().await.unwrap();
    let events = [orch_link::streams::DeliveredEvent {
        native_opening_id: Some(opening),
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
