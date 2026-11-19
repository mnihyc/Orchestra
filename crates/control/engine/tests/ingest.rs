//! What a harness says is applied exactly once, in order, to the thread it is about and to no other.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{
    World, eventually, meanwhile, message_began, message_done, message_grew, queued, said,
    turn_began, turn_ended,
};
use orch_authz::Caller;
use orch_link::streams::DeliveredEvent;
use orch_link_contract::events::{
    DeltaField, EmittedItemKind, Event, ItemCompleted, ItemDelta, ItemStarted, SessionOpened,
    SessionTitle, TurnOutcome, TurnPhase, TurnPhaseKind, UsageUpdated,
};
use orch_link_contract::streams::Stream;
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::time::Duration;

fn delivered(world: &World, seq: i64, event: &Event) -> DeliveredEvent {
    DeliveredEvent {
        native_opening_id: None,
        seq,
        at: Time::new(world.clock.now()),
        kind: event.kind().as_str().to_owned(),
        body: event.body().unwrap(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_literal_storage_escape_prefix_keeps_the_next_native_text_offset() {
    use orch_db::queries::threads;
    let world = World::tuned(|config| config.stream_flush = Duration::ZERO).await;
    let thread = world.thread("Literal source prefix").await;
    let (turn, _) = world.running_turn(thread, "Begin").await;
    let initial = "~orch.exact:literal";
    world.say(thread,Event::decode("item.started",json!({"turn_id":turn.turn_id,"native_ref":"literal-prefix","stream":"main","kind":"agent_message","payload":{"text":initial,"text_len":initial.chars().count()}})).unwrap()).await;
    world
        .say(
            thread,
            message_grew("literal-prefix", initial.chars().count() as i64, " tail"),
        )
        .await;
    let segment = world.segment_of(thread).await;
    eventually(
        "literal source text and its next stretch to be stored",
        || async {
            let mut conn = world.pool.acquire().await.unwrap();
            threads::item_by_native_ref(&mut conn, segment, "literal-prefix")
                .await
                .unwrap()
                .is_some_and(|item| item.payload["text"] == format!("{initial} tail"))
        },
    )
    .await;
    world
        .say(thread, turn_ended(turn.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_sessions_keep_their_original_discovery_identity_and_frozen_operands() {
    use orch_db::queries::{thread_history_cuts as cuts, threads};
    let world = World::new().await;
    let thread = world.thread("Exact native discovery").await;
    let (turn, _) = world.running_turn(thread, "Begin").await;
    let segment = world.segment_of(thread).await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE segments SET native_session_ref=NULL WHERE id=$1")
        .bind(segment)
        .execute(&mut *conn)
        .await
        .unwrap();
    let row = threads::thread(&mut conn, thread).await.unwrap().unwrap();
    assert!(
        threads::set_native_session_ref(&mut conn, segment, "session\0id", row.next_seq)
            .await
            .unwrap()
    );
    assert_eq!(
        threads::binding::adopting_threads(
            &mut conn,
            &["session\0id".to_owned(), "session�id".to_owned()]
        )
        .await
        .unwrap(),
        vec![("session\0id".to_owned(), thread)]
    );
    let cut = cuts::capture(
        &mut conn,
        &cuts::Capture {
            id: segment,
            thread,
            view: row.active_continuation_id,
            job: None,
            owner_thread: Some(thread),
            segment: Some(segment),
            execution_revision: row.execution_revision,
            range: None,
            at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    assert_eq!(cut.source_native_ref.as_deref(), Some("session\0id"));
    let operands = json!({"native_session_ref":"returned\0id","field\0":"first","field�":"second"});
    assert!(
        cuts::keep_artifact(&mut conn, cut.id, "native_result", &operands, &[])
            .await
            .unwrap()
    );
    assert_eq!(
        cuts::cut(&mut conn, cut.id)
            .await
            .unwrap()
            .unwrap()
            .artifacts["native_result"],
        operands
    );
    drop(conn);
    world
        .say(thread, turn_ended(turn.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_reference_zeroes_remain_distinct_from_literal_replacement_characters() {
    let world = World::new().await;
    let thread = world.thread("Distinct native identities").await;
    let (turn, _) = world.running_turn(thread, "Begin").await;
    let segment = world.segment_of(thread).await;
    let events=[
        Event::decode("item.started",json!({"turn_id":turn.turn_id,"native_ref":"source\0id","stream":"main","kind":"agent_message","payload":{"text":"First","text_len":5}})).unwrap(),
        Event::decode("item.started",json!({"turn_id":turn.turn_id,"native_ref":"source�id","stream":"main","kind":"agent_message","payload":{"text":"Second","text_len":6}})).unwrap(),
        Event::decode("item.completed",json!({"native_ref":"source\0id","status":"completed","payload":{"text":"First\0answer","text_len":12}})).unwrap(),
        Event::decode("item.completed",json!({"native_ref":"source�id","status":"completed","payload":{"text":"Second answer","text_len":13}})).unwrap(),
        turn_ended(turn.turn_id,TurnOutcome::Completed),
    ];
    let delivered: Vec<_> = events
        .iter()
        .enumerate()
        .map(|(i, event)| delivered(&world, 100 + i as i64, event))
        .collect();
    apply(&world, thread, segment, &delivered).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let first = orch_db::queries::threads::item_by_native_ref(&mut conn, segment, "source\0id")
        .await
        .unwrap()
        .unwrap();
    let second = orch_db::queries::threads::item_by_native_ref(&mut conn, segment, "source�id")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first.id, second.id);
    assert!(
        first.storage_gap.is_none(),
        "a repaired completion has no refused projection: {:?}",
        first.storage_gap
    );
    assert_eq!(first.status, "completed");
    assert_eq!(first.search_text.as_deref(), Some("First�answer"));
    assert_eq!(first.native_ref.as_deref(), Some("source\0id"));
    assert_eq!(first.payload["text"], "First�answer");
    assert_eq!(second.payload["text"], "Second answer");
    assert_eq!(first.text_replacements, 1);
    drop(conn);
    world.running_turn(thread, "Continue").await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_complete_native_snapshot_recovers_the_kind_of_a_gap_only_placeholder() {
    let world = World::new().await;
    let thread = world.thread("Recovered missing start").await;
    let (turn, _) = world.running_turn(thread, "Begin").await;
    let segment = world.segment_of(thread).await;
    sqlx::raw_sql("CREATE FUNCTION refuse_initial_value() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      IF NEW.payload->>'text'='refused initial value' THEN RAISE EXCEPTION 'synthetic refusal' USING ERRCODE='22P05'; END IF;
      RETURN NEW; END $$;
      CREATE TRIGGER refuse_initial_value BEFORE INSERT OR UPDATE ON items FOR EACH ROW EXECUTE FUNCTION refuse_initial_value();")
        .execute(world.pool.pool()).await.unwrap();
    let first=Event::decode("item.started",json!({"turn_id":turn.turn_id,"native_ref":"recover-kind","stream":"main","kind":"agent_message","payload":{"text":"refused initial value","text_len":21}})).unwrap();
    let last = message_done("recover-kind", "Recovered answer");
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(&world, 100, &first),
            delivered(&world, 101, &last),
            delivered(
                &world,
                102,
                &turn_ended(turn.turn_id, TurnOutcome::Completed),
            ),
        ],
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let item = orch_db::queries::threads::item_by_native_ref(&mut conn, segment, "recover-kind")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.kind, "agent_message");
    assert_eq!(item.payload["text"], "Recovered answer");
    assert!(item.storage_gap.is_none());
    drop(conn);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_terminal_event_refused_by_storage_keeps_a_gap_and_releases_the_next_turn() {
    let world = World::new().await;
    let thread = world.thread("An unstorable terminal explanation").await;
    let (start, _) = world.running_turn(thread, "Begin").await;
    let segment = world.segment_of(thread).await;
    sqlx::raw_sql("CREATE FUNCTION reject_terminal_data() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        IF NEW.error->>'message' = 'refused terminal value' THEN
          RAISE EXCEPTION 'synthetic terminal refusal' USING ERRCODE='22P05';
        END IF;
        RETURN NEW;
      END $$;
      CREATE TRIGGER reject_terminal_data BEFORE UPDATE ON turns FOR EACH ROW EXECUTE FUNCTION reject_terminal_data();")
        .execute(world.pool.pool()).await.unwrap();
    let terminal=Event::decode("turn.completed",json!({"turn_id":start.turn_id,"status":"failed","error":{"code":"harness_error","message":"refused terminal value","retryable":false}})).unwrap();
    apply(&world, thread, segment, &[delivered(&world, 77, &terminal)]).await;
    world.wait_turn(start.turn_id, "failed").await;
    let count: i64 = world
        .count(
            "SELECT count(*) FROM items WHERE thread_id=$1 AND storage_gap->>'sqlstate'='22P05'",
            thread,
        )
        .await;
    assert_eq!(count, 1);
    world
        .running_turn(thread, "The next turn still starts")
        .await;
    world.shutdown().await;
}

/// Applies events as the link would hand them over, in a transaction of their own.
async fn apply(world: &World, thread: Id, segment: Id, events: &[DeliveredEvent]) {
    world.open_session_for_reports(thread, segment).await;
    // These are current-producer fixture reports; delayed cases supply their original source explicitly.
    let opening = world.opening_of(thread).await;
    let events: Vec<_> = events
        .iter()
        .cloned()
        .map(|mut event| {
            if event.native_opening_id.is_none() {
                event.native_opening_id = Some(opening);
            }
            event
        })
        .collect();
    let mut tx = world.pool.begin().await.unwrap();
    world
        .engine()
        .apply_stream_events(
            &mut tx,
            world.machine_id,
            &Stream::Session {
                thread_id: thread,
                segment_id: segment,
            },
            &events,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// Everything the database holds of a thread, as one document.
async fn picture(world: &World, thread: Id) -> Json {
    let (whole,): (Json,) = sqlx::query_as(
        "SELECT jsonb_build_object(
            'thread', (SELECT to_jsonb(t) FROM threads t WHERE t.node_id = $1),
            'items', (SELECT coalesce(jsonb_agg(to_jsonb(i) ORDER BY i.seq_created, i.id), '[]'::jsonb)
                        FROM items i WHERE i.thread_id = $1),
            'turns', (SELECT coalesce(jsonb_agg(to_jsonb(u) ORDER BY u.ordinal), '[]'::jsonb)
                        FROM turns u WHERE u.thread_id = $1),
            'inputs', (SELECT coalesce(jsonb_agg(to_jsonb(n) ORDER BY n.position), '[]'::jsonb)
                         FROM thread_inputs n WHERE n.thread_id = $1),
            'segments', (SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY s.ordinal), '[]'::jsonb)
                           FROM segments s WHERE s.thread_id = $1),
            'model_inputs', (SELECT coalesce(jsonb_agg(to_jsonb(m) ORDER BY m.seq_created, m.id), '[]'::jsonb)
                              FROM model_inputs m WHERE m.node_id = $1),
            'approvals', (SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY a.id), '[]'::jsonb)
                            FROM approvals a WHERE a.thread_id = $1))",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    whole
}

/// Command receipt precedes the engine's durable record of what reached the model.
async fn model_inputs_settled(world: &World, thread: Id) {
    eventually("the model input command results are recorded", || async {
        world
            .count(
                "SELECT count(*) FROM model_inputs WHERE node_id=$1 AND state='prepared'",
                thread,
            )
            .await
            == 0
    })
    .await;
}

fn usage(turn: Id) -> Event {
    let figures = json!({
        "input_tokens": 100, "cached_input_tokens": 10, "output_tokens": 20,
        "reasoning_tokens": 5, "total_tokens": 120
    });
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: serde_json::from_value(figures.clone()).unwrap(),
        thread: serde_json::from_value(figures).unwrap(),
        context_window: Some(200_000),
        context_used: Some(5_000),
        report: None,
    })
}

/// A turn's worth of everything a session says.
async fn script(world: &World, segment: Id, turn: Id) -> Vec<DeliveredEvent> {
    let at = world.clock.now();
    let opening_id = orch_db::queries::session_dependencies::current_opening(
        &mut world.pool.acquire().await.unwrap(),
        segment,
    )
    .await
    .unwrap()
    .unwrap();
    let events = vec![
        Event::SessionOpened(SessionOpened {
            opening_id,
            native_session_ref: format!("native-{segment}"),
            resumed: false,
        }),
        turn_began(turn),
        Event::SessionTitle(SessionTitle {
            title: "Fix the login page".to_owned(),
        }),
        message_began(turn, "m1"),
        message_grew("m1", 0, "I will "),
        message_grew("m1", 7, "look at it."),
        Event::ItemStarted(ItemStarted {
            turn_id: turn,
            native_ref: "r1".to_owned(),
            stream: "main".to_owned(),
            parent_native_ref: None,
            kind: EmittedItemKind::Reasoning,
            payload: orch_link_contract::foreign::ItemPayload::Reasoning(
                orch_agent_contract::events::ReasoningPayload {
                    summary: String::new(),
                    text_len: 0,
                    hidden: false,
                },
            ),
        }),
        Event::ItemDelta(ItemDelta {
            native_ref: "r1".to_owned(),
            field: DeltaField::Summary,
            offset: 0,
            len: 5,
            data: "Think".to_owned(),
        }),
        Event::TurnPhase(TurnPhase {
            turn_id: turn,
            kind: TurnPhaseKind::Compacting,
            retry_attempt: None,
            at: Time::new(at),
        }),
        Event::TurnPhase(TurnPhase {
            turn_id: turn,
            kind: TurnPhaseKind::Working,
            retry_attempt: None,
            at: Time::new(at + chrono::Duration::seconds(2)),
        }),
        usage(turn),
        message_done("m1", "I will look at it."),
        Event::ItemCompleted(ItemCompleted {
            native_ref: "r1".to_owned(),
            status: orch_agent_contract::events::ItemStatus::Completed,
            payload: orch_link_contract::foreign::ItemPayload::Reasoning(
                orch_agent_contract::events::ReasoningPayload {
                    summary: "Think".to_owned(),
                    text_len: 5,
                    hidden: false,
                },
            ),
            output_blob: None,
        }),
        turn_ended(turn, TurnOutcome::Completed),
    ];
    events
        .iter()
        .zip(1_i64..)
        .map(|(event, seq)| delivered(world, seq, event))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_earlier_openings_reports_preserve_its_output_without_changing_the_current_phase() {
    let world = World::new().await;
    let thread = world.thread("Replacement phase").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Begin"))
        .await
        .unwrap();
    let turn = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    world.open_session_for_reports(thread, segment).await;
    let original = world.opening_of(thread).await;
    let from = |opening, seq, event: &Event| {
        let mut report = delivered(&world, seq, event);
        report.native_opening_id = Some(opening);
        report
    };
    apply(
        &world,
        thread,
        segment,
        &[
            from(original, 1, &turn_began(turn.turn_id)),
            from(original, 2, &message_began(turn.turn_id, "original-output")),
            from(
                original,
                3,
                &message_grew("original-output", 0, "Original output"),
            ),
        ],
    )
    .await;
    let (_, opening) = world
        .commands
        .issued_commands("session.open")
        .into_iter()
        .find(|(_, command)| command.body["thread_id"] == json!(thread))
        .unwrap();
    let replacement = orch_types::new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::link::insert_command(
        &mut conn,
        &orch_db::queries::link::NewCommand {
            native_target: None,
            id: replacement,
            machine_id: world.machine_id,
            kind: "session.open",
            body: &opening.body,
            serial_key: None,
            deadline_at: world.clock.now() + chrono::Duration::minutes(2),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    let turn_body = serde_json::to_value(&turn).unwrap();
    orch_db::queries::link::insert_command(
        &mut conn,
        &orch_db::queries::link::NewCommand {
            native_target: None,
            id: orch_types::new_id(),
            machine_id: world.machine_id,
            kind: "turn.start",
            body: &turn_body,
            serial_key: None,
            deadline_at: world.clock.now() + chrono::Duration::minutes(2),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(world.opening_of(thread).await, replacement);
    let phase = |kind| {
        Event::TurnPhase(TurnPhase {
            turn_id: turn.turn_id,
            kind,
            retry_attempt: None,
            at: world.clock.time(),
        })
    };
    apply(
        &world,
        thread,
        segment,
        &[from(replacement, 4, &phase(TurnPhaseKind::Retrying))],
    )
    .await;
    let complete = |reference: &str, payload: Json| {
        Event::decode(
            "item.completed",
            json!({"native_ref":reference,"status":"completed","payload":payload}),
        )
        .unwrap()
    };
    let old_review =
        orch_agent_contract::review::reported(orch_agent_contract::review::ReportedReview {
            native_ref: "original-review".into(),
            turn: Some(turn.turn_id),
            at: world.clock.time(),
            model: "original-reviewer",
            model_reported: true,
            outcome: "approved",
            reason: None,
            action: json!({"command":"original review"}),
            usage: None,
            raw: &json!({}),
        });
    let mut reports=vec![
        from(original,5,&phase(TurnPhaseKind::Compacting)),
        from(original,6,&message_done("original-output","Original output")),
        from(original,7,&turn_ended(turn.turn_id,TurnOutcome::Completed)),
        from(original,8,&message_began(turn.turn_id,"original-late-start")),
        from(original,9,&complete("original-compaction",json!({"marker":"compacted","data":{"duration_ms":null}}))),
        from(original,10,&complete("original-reroute",json!({"code":"model_rerouted","message":"An earlier native observation","retryable":false,
            "details":{"turn_id":turn.turn_id,"from_model":"original-model","to_model":"earlier-effective"}}))),
        from(original,11,&Event::ModelRequest(orch_link_contract::events::ModelRequest {
            turn_id:turn.turn_id,request:serde_json::from_value(json!({"id":"original-late-request","model":common::MODEL,
                "asked":world.clock.time(),"end":world.clock.time(),"outcome":{"status":"done"},
                "tokens":{"input_tokens":10,"cached_input_tokens":0,"cache_write_tokens":0,"output_tokens":2,"reasoning_tokens":0,"total_tokens":12}})).unwrap(),
        })),
    ];
    for (number, event) in old_review.iter().enumerate() {
        let value = serde_json::to_value(event).unwrap();
        let linked = Event::decode(event.kind(), value["body"].clone()).unwrap();
        reports.push(from(original, 12 + i64::try_from(number).unwrap(), &linked));
    }
    // Hold the real thread row while seeding the replacement and applying the deterministic late batch.
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::threads::lock(&mut tx, thread)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE turns SET state='starting',usage_detail=$2 WHERE id=$1")
        .bind(turn.turn_id)
        .bind(json!({"effective_model":"current-model"}))
        .execute(&mut *tx)
        .await
        .unwrap();
    let context = json!({"given":"replacement"});
    let reviewer = json!({"model":"current-reviewer","reported":true,"usage_apart":false});
    sqlx::query("UPDATE segments SET context_sent=$2,approval_reviewer=$3 WHERE id=$1")
        .bind(segment)
        .bind(&context)
        .bind(&reviewer)
        .execute(&mut *tx)
        .await
        .unwrap();
    world
        .engine()
        .apply_stream_events(
            &mut tx,
            world.machine_id,
            &Stream::Session {
                thread_id: thread,
                segment_id: segment,
            },
            &reports,
        )
        .await
        .unwrap();
    let state: (String, Json) = sqlx::query_as("SELECT state,usage_detail FROM turns WHERE id=$1")
        .bind(turn.turn_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        state.0, "starting",
        "old output cannot begin the replacement's retried turn"
    );
    assert_eq!(state.1["effective_model"], "current-model");
    let kept: (Json, Json) =
        sqlx::query_as("SELECT context_sent,approval_reviewer FROM segments WHERE id=$1")
            .bind(segment)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(kept, (context, reviewer));
    let ledger = orch_db::usage::ledger::events_of_turn(&mut tx, turn.turn_id)
        .await
        .unwrap();
    assert!(
        ledger
            .iter()
            .any(|entry| entry.request_ref.as_deref() == Some("original-late-request")),
        "actual earlier request use remains recorded"
    );
    tx.commit().await.unwrap();
    assert_eq!(
        world
            .scalar::<Option<String>>("SELECT phase FROM turns WHERE id=$1", turn.turn_id)
            .await
            .as_deref(),
        Some("retrying")
    );
    assert!(
        !world
            .scalar::<bool>(
                "SELECT native_end_confirmed FROM turns WHERE id=$1",
                turn.turn_id
            )
            .await
    );
    let output: (String, Json) = sqlx::query_as(
        "SELECT status,payload FROM items WHERE thread_id=$1 AND native_ref='original-output'",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(output.0, "completed");
    assert!(output.1.to_string().contains("Original output"));
    world.shutdown().await;
}

#[tokio::test]
async fn a_result_from_before_a_keeper_change_cannot_rewrite_the_current_providers_health() {
    for success in [false, true] {
        let world = World::new().await;
        let thread = world.thread("Keeper change").await;
        let (turn, _) = world
            .running_turn(thread, "An admitted model request")
            .await;
        let segment = world.segment_of(thread).await;
        let opening = world.opening_of(thread).await;
        let keeper = world.add_machine("Replacement keeper", &["provider"]).await;
        let mut tx = world.pool.begin().await.unwrap();
        orch_db::queries::threads::lock(&mut tx, thread)
            .await
            .unwrap()
            .unwrap();
        let present:bool=sqlx::query_scalar("SELECT (delivered_resources#>'{tool_admission,members}')?'llm' FROM segments WHERE id=$1")
            .bind(segment).fetch_one(&mut *tx).await.unwrap();
        assert!(
            present,
            "the deciding control starts with the real admitted model member"
        );
        sqlx::query("UPDATE providers SET key_machine_id=$2 WHERE id=$1")
            .bind(world.provider)
            .bind(keeper.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        let expected = if success { "error" } else { "ready" };
        sqlx::query("UPDATE providers SET status=$2 WHERE id=$1")
            .bind(world.provider)
            .bind(expected)
            .execute(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            orch_db::queries::session_dependencies::current_opening(&mut tx, segment)
                .await
                .unwrap(),
            Some(opening)
        );
        let event=Event::decode("turn.completed",json!({"turn_id":turn.turn_id,"status":if success {"completed"}else{"failed"},
            "error":if success {Json::Null}else{json!({"code":"provider_unauthorized","message":"The earlier keeper rejected its credential","retryable":false})}})).unwrap();
        let report = DeliveredEvent {
            native_opening_id: Some(opening),
            ..delivered(&world, 90, &event)
        };
        world
            .engine()
            .apply_stream_events(
                &mut tx,
                world.machine_id,
                &Stream::Session {
                    thread_id: thread,
                    segment_id: segment,
                },
                &[report],
            )
            .await
            .unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM providers WHERE id=$1")
            .bind(world.provider)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            status, expected,
            "a result from the old member cannot change the new keeper"
        );
        let state: String = sqlx::query_scalar("SELECT state FROM turns WHERE id=$1")
            .bind(turn.turn_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            state,
            if success { "completed" } else { "failed" },
            "the original turn still keeps its actual outcome"
        );
        tx.commit().await.unwrap();
        world.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_events_delivered_twice_change_nothing_the_second_time() {
    let world = World::new().await;
    let thread = world.thread("Twice delivered").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("Please do it"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let events = script(&world, segment, start.turn_id).await;

    apply(&world, thread, segment, &events).await;
    let forwarded = world.live.deltas().len();
    let sync = world.sync(thread).await;
    assert_eq!(
        said(&sync),
        ["user: Please do it", "agent: I will look at it."]
    );
    assert_eq!(
        sync.turns[0].state,
        orch_api_contract::threads::TurnState::Completed
    );
    assert!(sync.thread.usage.is_some(), "the thread's use is told");
    assert_eq!(
        forwarded, 2,
        "each stretch of text was forwarded once, the two that continue each other in one delivery as one"
    );
    world.wait_state(thread, "idle").await;
    // What the end of the turn writes after it (its use counted, who is told) is part of the picture.
    world.wait_followed_up(thread).await;
    model_inputs_settled(&world, thread).await;
    let once = picture(&world, thread).await;

    // The whole of it again, and then the stretches a reconnecting link would resend.
    for (name, window) in [
        ("all of it", &events[..]),
        ("the second half", &events[events.len() / 2..]),
        ("the first half", &events[..events.len() / 2]),
        ("the middle", &events[3..9]),
        ("the last", &events[events.len() - 1..]),
        ("the first", &events[..1]),
    ] {
        apply(&world, thread, segment, window).await;
        assert_eq!(
            picture(&world, thread).await,
            once,
            "delivered again: {name}"
        );
    }
    assert_eq!(
        world.live.deltas().len(),
        forwarded,
        "nothing already forwarded is forwarded again"
    );

    // A second turn begins; the first turn's words, delivered late, leave it as it is.
    world
        .engine()
        .submit_input(&ada, thread, queued("And another thing"))
        .await
        .unwrap();
    let second = world.turn_start(thread, 1).await;
    eventually("the second turn's accepted command is waiting for its native start", || async {
        world.scalar::<bool>("SELECT start_wait->>'stage' IS NOT DISTINCT FROM 'start' FROM turns WHERE id=$1", second.turn_id).await
    }).await;
    model_inputs_settled(&world, thread).await;
    let before = picture(&world, thread).await;
    apply(&world, thread, segment, &events).await;
    assert_eq!(
        picture(&world, thread).await,
        before,
        "words of a turn that is over leave the next alone"
    );
    assert_eq!(world.turn_state(second.turn_id).await, "starting");
}

#[tokio::test]
async fn a_recorded_sandbox_decision_keeps_its_applied_policy_after_later_machine_facts_change() {
    let world = World::new().await;
    let thread = world.thread("Recorded sandbox").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Please do it"))
        .await
        .unwrap();
    let turn = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let before = world.sync(thread).await;
    assert!(before.turns[0].sandbox_resolution.is_none());
    let expected = json!({
        "requested":"open", "applied":"workspace", "governing_policy":"machine_policy",
        "command_id":orch_types::new_id(), "at":world.clock.time(),
    });
    let mut body = expected.clone();
    body["turn_id"] = json!(turn.turn_id);
    let event = DeliveredEvent {
        native_opening_id: Some(world.opening_of(thread).await),
        seq: 1,
        at: world.clock.time(),
        kind: "session.sandbox".into(),
        body,
    };
    apply(&world, thread, segment, std::slice::from_ref(&event)).await;
    sqlx::query("UPDATE machines SET facts=facts || $2 WHERE id=$1")
        .bind(world.machine_id)
        .bind(json!({"policy":{"agent.max_sandbox":"read_only"}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    apply(&world, thread, segment, std::slice::from_ref(&event)).await;
    let after = world.sync(thread).await;
    assert_eq!(
        after.turns[0].sandbox_resolution,
        Some(serde_json::from_value(expected).unwrap())
    );
    world.shutdown().await;
}

#[tokio::test]
async fn embedded_zeroes_in_native_text_and_tool_keys_do_not_stall_the_next_turn() {
    let world = World::new().await;
    let thread = world.thread("Storable native output").await;
    let (turn, _) = world.running_turn(thread, "Read the output").await;
    world
        .say(thread, message_began(turn.turn_id, "zero-message"))
        .await;
    world
        .say(thread, message_done("zero-message", "Hello\0world"))
        .await;
    for (reference, tool, expected) in [
        (
            "zero-command",
            json!({"kind":"command","command":"cat binary","cwd":"/work","exit_code":0,"process_id":null,"background":false,"actions":[],
            "output":{"inline":"one\0two","total_len":7,"truncated":false,"blob":null}}),
            1,
        ),
        (
            "zero-terminal",
            json!({"kind":"other","name":"use_terminal","arguments":null,
            "result":{"field\0":"left\0right","field\u{fffd}":"unchanged"}}),
            2,
        ),
    ] {
        let payload = json!({"tool":tool,"title":"Read output","started_at":world.clock.time(),"duration_ms":null,"raw":{"name":"fixture","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},"output":null,"is_error":false}});
        world.say(thread, Event::decode("item.started",json!({"turn_id":turn.turn_id,"native_ref":reference,"stream":"main","parent_native_ref":null,"kind":"tool_call","payload":{"tool":{"kind":"other","name":"starting","arguments":null,"result":null},"title":"Starting","started_at":world.clock.time(),"duration_ms":null,"raw":{"name":"fixture","input":{"inline":"{}","total_len":2,"truncated":false,"blob":null},"output":null,"is_error":false}}})).unwrap()).await;
        world
            .say(
                thread,
                Event::decode(
                    "item.completed",
                    json!({"native_ref":reference,"status":"completed","payload":payload.clone()}),
                )
                .unwrap(),
            )
            .await;
        eventually("the repaired native item to complete",||async {
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND native_ref=$2 AND status='completed')")
                .bind(thread).bind(reference).fetch_one(world.pool.pool()).await.unwrap()
        }).await;
        let row: Json = sqlx::query_scalar(
            "SELECT to_jsonb(i) FROM items i WHERE thread_id=$1 AND native_ref=$2",
        )
        .bind(thread)
        .bind(reference)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
        assert_eq!(row["text_replacements"], expected);
        let completed = Event::decode(
            "item.completed",
            json!({"native_ref":reference,"status":"completed","payload":payload}),
        )
        .unwrap();
        apply(
            &world,
            thread,
            world.segment_of(thread).await,
            &[delivered(&world, 880, &completed)],
        )
        .await;
        let replayed: Json = sqlx::query_scalar(
            "SELECT to_jsonb(i) FROM items i WHERE thread_id=$1 AND native_ref=$2",
        )
        .bind(thread)
        .bind(reference)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
        assert_eq!(
            replayed, row,
            "a repeated native completion preserves the repair receipt and item sequence"
        );
    }
    world
        .say(thread, turn_ended(turn.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(turn.turn_id, "completed").await;
    let snapshot = world.sync(thread).await;
    let items = serde_json::to_value(&snapshot.items).unwrap();
    let message = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["payload"]["text"] == "Hello\u{fffd}world")
        .unwrap();
    assert_eq!(message["text_replacements"], 1);
    let command = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["payload"]["tool"]["kind"] == "command")
        .unwrap();
    assert_eq!(
        command["payload"]["tool"]["output"]["inline"],
        "one\u{fffd}two"
    );
    assert_eq!(command["payload"]["tool"]["output"]["total_len"], 7);
    let terminal = items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["payload"]["tool"]["name"] == "use_terminal")
        .unwrap();
    assert_eq!(
        terminal["payload"]["tool"]["result"]["field\u{fffd}"],
        "unchanged"
    );
    assert_eq!(
        terminal["payload"]["tool"]["result"]["field\u{fffd} (2)"],
        "left\u{fffd}right"
    );
    let (next, _) = world
        .running_turn(thread, "Continue after the output")
        .await;
    assert_ne!(next.turn_id, turn.turn_id);
    world.shutdown().await;
}

#[tokio::test]
async fn one_database_data_refusal_keeps_the_other_native_events_and_an_explicit_item_gap() {
    let world = World::new().await;
    let thread = world.thread("Continue past rejected data").await;
    let (turn, _) = world.running_turn(thread, "Keep the good events").await;
    let segment = world.segment_of(thread).await;
    sqlx::raw_sql("CREATE FUNCTION reject_native_fixture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.payload->>'text' = 'rejected native value' THEN RAISE EXCEPTION 'fixture data refusal' USING ERRCODE='22P05'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_native_fixture BEFORE INSERT OR UPDATE OF payload ON items FOR EACH ROW EXECUTE FUNCTION reject_native_fixture();")
        .execute(world.pool.pool()).await.unwrap();
    let events = [
        message_began(turn.turn_id, "before-refusal"),
        message_done("before-refusal", "Before"),
        message_began(turn.turn_id, "refused-item"),
        message_done("refused-item", "rejected native value"),
        message_began(turn.turn_id, "after-refusal"),
        message_done("after-refusal", "After"),
        turn_ended(turn.turn_id, TurnOutcome::Completed),
    ];
    let events: Vec<_> = events
        .iter()
        .enumerate()
        .map(|(at, event)| delivered(&world, i64::try_from(at + 1).unwrap(), event))
        .collect();
    apply(&world, thread, segment, &events).await;
    world.wait_turn(turn.turn_id, "completed").await;
    let snapshot = world.sync(thread).await;
    let items = serde_json::to_value(&snapshot.items).unwrap();
    let items = items.as_array().unwrap();
    assert!(items.iter().any(|item| item["payload"]["text"] == "Before"));
    assert!(items.iter().any(|item| item["payload"]["text"] == "After"));
    assert!(
        items
            .iter()
            .any(|item| item["storage_gap"]["sqlstate"] == "22P05")
    );
    let (next, _) = world
        .running_turn(thread, "Continue after refused data")
        .await;
    assert_ne!(next.turn_id, turn.turn_id);
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stretch_of_text_that_skips_ahead_is_left_for_the_completed_item() {
    let world = World::new().await;
    let thread = world.thread("Gap").await;
    let (start, _) = world.running_turn(thread, "Say something long").await;
    world.say(thread, message_began(start.turn_id, "m2")).await;
    world.say(thread, message_grew("m2", 0, "abc")).await;
    // A stretch that skips ahead, and one that was already applied.
    world.say(thread, message_grew("m2", 10, "xyz")).await;
    world.say(thread, message_grew("m2", 0, "abc")).await;
    common::eventually("the first stretch is applied", || async {
        world.sync(thread).await.items.iter().any(|item| {
            matches!(
                &item.payload,
                orch_api_contract::threads::ItemPayload::AgentMessage(m) if m.text == "abc"
            )
        })
    })
    .await;
    meanwhile(Duration::from_millis(200)).await;
    let texts: Vec<_> = world
        .live
        .deltas()
        .iter()
        .filter(|d| d.data != "Working")
        .map(|d| d.data.clone())
        .collect();
    assert_eq!(
        texts,
        ["abc"],
        "only text that continues what is held is forwarded"
    );

    world
        .say(thread, message_done("m2", "abcdefghijklxyz"))
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let said = said(&world.sync(thread).await);
    assert_eq!(
        said.last().map(String::as_str),
        Some("agent: abcdefghijklxyz"),
        "the completed item has the whole"
    );
}

/// The text of the thread's last message of the agent, as the thread reads back.
async fn message_text(world: &World, thread: Id) -> Option<String> {
    world
        .sync(thread)
        .await
        .items
        .iter()
        .rev()
        .find_map(|item| match &item.payload {
            orch_api_contract::threads::ItemPayload::AgentMessage(message) => {
                Some(message.text.clone())
            }
            _ => None,
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn growing_text_is_held_and_written_before_what_ends_its_turn() {
    // An interval no test reaches: only what the session says next writes what is held.
    let world = World::tuned(|config| config.stream_flush = Duration::from_secs(3_600)).await;
    let thread = world.thread("Held").await;
    let (start, _) = world.running_turn(thread, "Begin, then stop").await;
    world.say(thread, message_began(start.turn_id, "m4")).await;
    world.say(thread, message_grew("m4", 0, "Half of ")).await;
    world.say(thread, message_grew("m4", 8, "an answer")).await;
    let observed = std::sync::Mutex::new(None::<Json>);
    eventually("both stretches reach the live readers", || async {
        let item: Option<Id> =
            sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND native_ref='m4'")
                .bind(thread)
                .fetch_optional(world.pool.pool())
                .await
                .unwrap();
        let deltas = world.live.deltas();
        let snapshot = json!(deltas.iter().map(|delta| {
            json!({"thread_id":delta.thread_id,"item_id":delta.item_id,"turn_id":delta.turn_id,
                "stream":delta.stream,"field":delta.field,"offset":delta.offset,
                "len":delta.len,"data":delta.data})
        }).collect::<Vec<_>>());
        let mut previous = observed.lock().unwrap();
        if previous.as_ref() != Some(&snapshot) {
            eprintln!(
                "held-text-live thread={thread} turn={} deltas={snapshot}",
                start.turn_id
            );
            *previous = Some(snapshot);
        }
        drop(previous);
        deltas
            .iter()
            .filter(|delta| {
                delta.thread_id == thread
                    && Some(delta.item_id) == item
                    && delta.field == DeltaField::Text
            })
            .map(|delta| delta.data.as_str())
            .collect::<String>()
            == "Half of an answer"
    })
    .await;
    meanwhile(Duration::from_millis(200)).await;
    assert_eq!(
        message_text(&world, thread).await.as_deref(),
        Some(""),
        "the pieces are held, not written to the item"
    );
    let item: Id = world
        .scalar(
            "SELECT id FROM items WHERE thread_id=$1 AND native_ref='m4'",
            thread,
        )
        .await;
    let mut offset = 0;
    let mut streamed = String::new();
    for delta in world
        .live
        .deltas()
        .into_iter()
        .filter(|delta| delta.item_id == item)
    {
        assert_eq!(delta.thread_id, thread);
        assert_eq!(delta.item_id, item);
        assert_eq!(delta.turn_id, Some(start.turn_id));
        assert_eq!(delta.stream, "main");
        assert_eq!(delta.field, DeltaField::Text);
        assert_eq!(delta.offset, offset, "live text has no gaps or duplicates");
        assert_eq!(
            delta.len,
            i64::try_from(delta.data.chars().count()).unwrap()
        );
        offset += delta.len;
        streamed.push_str(&delta.data);
    }
    assert_eq!(streamed, "Half of an answer");
    assert_eq!(offset, i64::try_from(streamed.chars().count()).unwrap());

    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    eventually("the end of the turn wrote what was held", || async {
        message_text(&world, thread).await.as_deref() == Some("Half of an answer")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn events_of_two_threads_interleaved_stay_apart() {
    let world = World::new().await;
    let (first, second) = (world.thread("First").await, world.thread("Second").await);
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, first, queued("Question one"))
        .await
        .unwrap();
    world
        .engine()
        .submit_input(&ada, second, queued("Question two"))
        .await
        .unwrap();
    let (one, two) = (
        world.turn_start(first, 0).await,
        world.turn_start(second, 0).await,
    );
    let (segment_one, segment_two) = (
        world.segment_of(first).await,
        world.segment_of(second).await,
    );
    assert_ne!(segment_one, segment_two);

    // Both sessions use the same names for their items; the words are their own.
    let speak = |thread: Id, turn: Id, words: Vec<&'static str>| {
        let machine = &world;
        async move {
            machine.say(thread, turn_began(turn)).await;
            machine.say(thread, message_began(turn, "m1")).await;
            let mut before = 0_i64;
            let mut whole = String::new();
            for part in words {
                machine.say(thread, message_grew("m1", before, part)).await;
                before += i64::try_from(part.chars().count()).unwrap();
                whole.push_str(part);
                tokio::task::yield_now().await;
            }
            machine.say(thread, message_done("m1", &whole)).await;
            machine
                .say(thread, turn_ended(turn, TurnOutcome::Completed))
                .await;
        }
    };
    tokio::join!(
        speak(
            first,
            one.turn_id,
            vec!["Answer ", "to ", "the ", "first ", "question."]
        ),
        speak(
            second,
            two.turn_id,
            vec!["The ", "second ", "answer ", "is ", "different."]
        )
    );
    world.wait_state(first, "idle").await;
    world.wait_state(second, "idle").await;
    world.wait_followed_up(first).await;
    world.wait_followed_up(second).await;
    model_inputs_settled(&world, first).await;
    model_inputs_settled(&world, second).await;

    let (sync_one, sync_two) = (world.sync(first).await, world.sync(second).await);
    assert_eq!(
        said(&sync_one),
        ["user: Question one", "agent: Answer to the first question."]
    );
    assert_eq!(
        said(&sync_two),
        [
            "user: Question two",
            "agent: The second answer is different."
        ]
    );
    for (thread, sync) in [(first, &sync_one), (second, &sync_two)] {
        assert!(
            sync.items.iter().all(|item| item.thread_id == thread),
            "nothing of the other thread"
        );
        // Each thread's sequence is its own and has no gap in what readers see.
        let next: i64 = world
            .scalar("SELECT next_seq FROM threads WHERE node_id = $1", thread)
            .await;
        assert_eq!(sync.seq, next - 1);
        let mut seen: Vec<i64> = sync
            .items
            .iter()
            .flat_map(|item| [item.seq_created, item.seq_updated])
            .collect();
        seen.extend(sync.turns.iter().map(|turn| turn.seq_updated));
        assert!(
            seen.iter().all(|seq| (1..next).contains(seq)),
            "{seen:?} within 1..{next}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_native_settings_observation_stays_outside_an_already_reserved_turn() {
    let world = World::new().await;
    let thread = world.thread("Native settings").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let before: Json = world
        .scalar(
            "SELECT jsonb_build_object('model',model,'params',params) FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    let event=Event::decode("item.completed",json!({"native_ref":"native-settings-idle","status":"completed","payload":{"marker":"harness_changed","data":{"program":"Codex","change":"model","previous":{"state":"told","value":"chosen"},"value":"native-future","expected":{"state":"told","value":"chosen"},"changed":true,"diverged":true,"next_turn":true,"turn_id":null,"reported_at":world.clock.time()}}})).unwrap();
    world.say(thread, event.clone()).await;
    world.say(thread, event).await;
    eventually("one source-owned native settings marker",||async {world.scalar::<bool>("SELECT count(*)=1 FROM items WHERE thread_id=$1 AND native_ref='native-settings-idle'",thread).await}).await;
    let marker_turn: Option<Id> = world
        .scalar(
            "SELECT turn_id FROM items WHERE thread_id=$1 AND native_ref='native-settings-idle'",
            thread,
        )
        .await;
    assert!(
        marker_turn.is_none(),
        "a thread-level observation must not invent an originating turn"
    );
    let after: Json = world
        .scalar(
            "SELECT jsonb_build_object('model',model,'params',params) FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    assert_eq!(before, after);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn anonymous_native_failures_keep_source_event_identity_across_transport_replay() {
    let world = World::new().await;
    let thread = world.thread("Anonymous failure observations").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let running = orch_agent_contract::requests::UsageRequest {
        id: "native-session:1".into(),
        asked: Some(world.clock.time()),
        ..Default::default()
    };
    let mut first = running.clone();
    first.id = "native-session:native-turn:failure:1".into();
    first.previous_ref = Some(running.id.clone());
    first.upstream_status = Some(429);
    first.finish(
        "rate_limited",
        Some("Same native words"),
        Some("Same vendor words"),
    );
    let mut second = first.clone();
    second.id = "native-session:native-turn:failure:2".into();
    second.previous_ref = None;
    let reported = |request| {
        Event::ModelRequest(orch_link_contract::events::ModelRequest {
            turn_id: start.turn_id,
            request,
        })
    };
    let first_event = reported(first.clone());
    let events = [
        delivered(
            &world,
            1,
            &Event::SessionOpened(SessionOpened {
                opening_id: world.opening_of(thread).await,
                native_session_ref: "native-session".into(),
                resumed: false,
            }),
        ),
        delivered(&world, 2, &turn_began(start.turn_id)),
        delivered(&world, 3, &reported(running)),
        delivered(&world, 4, &first_event),
        delivered(&world, 5, &reported(second)),
    ];
    apply(&world, thread, segment, &events).await;
    let rows = world.ledger_of(start.turn_id).await;
    assert_eq!(
        rows.len(),
        2,
        "an exact provisional observation is replaced by its failure report: {rows:?}"
    );
    assert_ne!(rows[0].id, rows[1].id);
    for row in &rows {
        assert_eq!(row.total_tokens, 0);
        assert_eq!(row.upstream_status, Some(429));
        assert!(row.provider_request_id.is_none());
        assert_eq!(row.outcome.as_deref(), Some("rate_limited"));
    }
    assert_eq!(
        world
            .scalar::<i64>(
                "SELECT count(*) FROM usage_running WHERE turn_id=$1",
                start.turn_id
            )
            .await,
        0
    );
    apply(&world, thread, segment, &events).await;
    apply(
        &world,
        thread,
        segment,
        &[delivered(&world, 6, &first_event)],
    )
    .await;
    let replayed = world.ledger_of(start.turn_id).await;
    assert_eq!(
        replayed.len(),
        2,
        "the same reported failure event can be delivered again with its retained reference"
    );
    assert_eq!(rows, replayed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_marker_only_compaction_resends_context_once_without_borrowing_a_childs_context() {
    let world = World::new().await;
    let thread = world.thread("Marker-only compaction").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let marker = |native_ref: &str| {
        Event::decode("item.completed",json!({"native_ref":native_ref,"status":"completed","payload":{"marker":"compacted","data":{"duration_ms":null}}})).unwrap()
    };
    let began = |native_ref: &str, stream: &str| {
        Event::decode("item.started",json!({"native_ref":native_ref,"turn_id":start.turn_id,"stream":stream,"kind":"marker","payload":{"marker":"compacted","data":{"duration_ms":null}}})).unwrap()
    };
    let reset = || async {
        sqlx::query("UPDATE segments SET context_sent='{\"baseline\":true}' WHERE id=$1")
            .bind(segment)
            .execute(world.pool.pool())
            .await
            .unwrap();
    };
    let sent = || async {
        world
            .scalar::<Json>("SELECT context_sent FROM segments WHERE id=$1", segment)
            .await
    };
    reset().await;
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(
                &world,
                1,
                &Event::SessionOpened(SessionOpened {
                    opening_id: world.opening_of(thread).await,
                    native_session_ref: "native-compaction".into(),
                    resumed: false,
                }),
            ),
            delivered(&world, 2, &turn_began(start.turn_id)),
            delivered(&world, 3, &marker("only-end")),
        ],
    )
    .await;
    assert_eq!(sent().await, json!({"resend":true}), "no phase was needed");
    reset().await;
    apply(
        &world,
        thread,
        segment,
        &[delivered(&world, 4, &marker("only-end"))],
    )
    .await;
    assert_eq!(
        sent().await,
        json!({"baseline":true}),
        "replay does not re-arm a consumed resend"
    );
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(&world, 5, &began("child-compaction", "sub:child")),
            delivered(&world, 6, &marker("child-compaction")),
        ],
    )
    .await;
    assert_eq!(
        sent().await,
        json!({"baseline":true}),
        "a child did not compact the main conversation"
    );
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(&world, 7, &began("observed-compaction", "main")),
            delivered(&world, 8, &marker("observed-compaction")),
        ],
    )
    .await;
    assert_eq!(
        sent().await,
        json!({"resend":true}),
        "a started marker uses the same supplier"
    );
    reset().await;
    let data = json!({"program":"Codex","change":"context_compaction","subject":null,
        "previous":{"state":"unsupported"},"value":{"duration_ms":null},
        "expected":{"state":"unsupported"},"changed":true,"diverged":false,
        "next_turn":false,"turn_id":null,"reported_at":world.clock.time()});
    let report = Event::decode(
        "item.completed",
        json!({"native_ref":"idle-native-compaction","status":"completed",
        "payload":{"marker":"harness_changed","data":data}}),
    )
    .unwrap();
    apply(&world, thread, segment, &[delivered(&world, 9, &report)]).await;
    assert_eq!(
        sent().await,
        json!({"resend":true}),
        "an explicit unassigned native compaction uses the same supplier"
    );
    reset().await;
    apply(&world, thread, segment, &[delivered(&world, 10, &report)]).await;
    assert_eq!(
        sent().await,
        json!({"baseline":true}),
        "the same observed source does not re-arm a consumed resend"
    );
    let mut child_data = data;
    child_data["turn_id"] = json!(start.turn_id);
    let payload = json!({"marker":"harness_changed","data":child_data});
    let began = Event::decode(
        "item.started",
        json!({"native_ref":"native-child-compaction","turn_id":start.turn_id,
        "stream":"sub:child","kind":"marker","payload":payload}),
    )
    .unwrap();
    let ended = Event::decode(
        "item.completed",
        json!({"native_ref":"native-child-compaction","status":"completed","payload":payload}),
    )
    .unwrap();
    apply(
        &world,
        thread,
        segment,
        &[delivered(&world, 11, &began), delivered(&world, 12, &ended)],
    )
    .await;
    assert_eq!(
        sent().await,
        json!({"baseline":true}),
        "the new native marker preserves its child's separate context"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_runtime_changes_audit_their_source_once_without_copying_native_bodies() {
    let world = World::new().await;
    let thread = world.thread("Runtime audit").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let changes = [
        "model",
        "effort",
        "approval_policy",
        "sandbox_mode",
        "collaboration_mode",
        "collaboration_instructions",
        "session_status",
        "mcp_startup",
        "session_lifecycle",
        "hook",
    ];
    let marker = |change: &str, turn: Option<Id>| json!({"marker":"harness_changed","data":{"program":"Untrusted display","change":change,"subject":"Native label","previous":{"state":"unsupported"},"value":{"status":"completed","private":"never-copy-runtime-body"},"expected":{"state":"unsupported"},"changed":true,"diverged":false,"next_turn":false,"turn_id":turn,"reported_at":world.clock.time()}});
    let complete = |reference: &str, payload: Json| {
        Event::decode(
            "item.completed",
            json!({"native_ref":reference,"status":"completed","payload":payload}),
        )
        .unwrap()
    };
    let mut frames = vec![
        delivered(
            &world,
            1,
            &Event::SessionOpened(SessionOpened {
                opening_id: world.opening_of(thread).await,
                native_session_ref: "runtime-audit".into(),
                resumed: false,
            }),
        ),
        delivered(&world, 2, &turn_began(start.turn_id)),
    ];
    for (index, change) in changes.iter().enumerate() {
        frames.push(delivered(
            &world,
            3 + i64::try_from(index).unwrap(),
            &complete(change, marker(change, Some(start.turn_id))),
        ));
    }
    apply(&world, thread, segment, &frames).await;
    let rows:Vec<Json>=sqlx::query_scalar("SELECT jsonb_build_object('actor',actor_kind,'for_user',for_user_id,'details',details) FROM audit_log WHERE node_id=$1 AND action='harness.changed' ORDER BY at,id").bind(thread).fetch_all(world.pool.pool()).await.unwrap();
    assert_eq!(rows.len(), changes.len());
    for row in &rows {
        assert_eq!(row["actor"], "agent");
        assert_eq!(row["for_user"], json!(world.ada));
        assert_eq!(row["details"]["turn_id"], json!(start.turn_id));
        assert_ne!(row["details"]["program"], json!("Untrusted display"));
    }
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("never-copy-runtime-body")
    );
    let same = complete("model", marker("model", Some(start.turn_id)));
    apply(&world, thread, segment, &[delivered(&world, 20, &same)]).await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        10
    );
    let mut revised = marker("model", Some(start.turn_id));
    revised["data"]["value"] = json!("new-native-model");
    apply(
        &world,
        thread,
        segment,
        &[delivered(&world, 21, &complete("model", revised))],
    )
    .await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        11,
        "an actual update of one source item is audited"
    );
    let idle = complete("idle-observation", marker("session_status", None));
    apply(&world, thread, segment, &[delivered(&world, 22, &idle)]).await;
    let idle:Json=world.scalar("SELECT jsonb_build_object('actor',actor_kind,'for_user',for_user_id,'turn',details->'turn_id') FROM audit_log WHERE node_id=$1 AND action='harness.changed' AND details->>'native_ref'='idle-observation'",thread).await;
    assert_eq!(
        idle,
        json!({"actor":"system","for_user":null,"turn":null}),
        "idle facts borrow neither the newer turn nor its person"
    );
    let compacted = complete(
        "compaction-observation",
        json!({"marker":"compacted","data":{"duration_ms":null}}),
    );
    let rerouted = complete(
        "reroute-observation",
        json!({"code":"model_rerouted","message":"never-copy-runtime-body","retryable":false,"details":{"turn_id":start.turn_id,"from_model":"chosen","to_model":"native-effective"}}),
    );
    let review =
        orch_agent_contract::review::reported(orch_agent_contract::review::ReportedReview {
            native_ref: "review-observation".into(),
            turn: Some(start.turn_id),
            at: world.clock.time(),
            model: "codex-auto-review",
            model_reported: false,
            outcome: "approved",
            reason: Some("never-copy-runtime-body"),
            action: json!({"private":"never-copy-runtime-body"}),
            usage: None,
            raw: &Json::Null,
        });
    let review = review
        .into_iter()
        .find_map(|event| {
            if let orch_agent_contract::events::SessionEvent::ItemCompleted(item) = event {
                Some(complete("review-observation", item.payload))
            } else {
                None
            }
        })
        .unwrap();
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(&world, 23, &compacted),
            delivered(&world, 24, &rerouted),
            delivered(&world, 25, &review),
        ],
    )
    .await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        15
    );
    let audit:Json=world.scalar("SELECT jsonb_agg(details ORDER BY at,id) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",thread).await;
    assert!(!audit.to_string().contains("never-copy-runtime-body"));
    apply(
        &world,
        thread,
        segment,
        &[
            delivered(&world, 26, &compacted),
            delivered(&world, 27, &rerouted),
            delivered(&world, 28, &review),
        ],
    )
    .await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id=$1 AND action='harness.changed'",
                thread
            )
            .await,
        15
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unscoped_program_status_is_a_provider_audit_fact_without_native_diagnostics() {
    let world = World::new().await;
    let thread = world.thread("Program status audit").await;
    world.running_turn(thread, "Go").await;
    let count = || async {
        world.count("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='harness.changed' AND details->>'change'='program_status'",world.provider).await
    };
    let before = count().await;
    let event = Event::HarnessStatus(orch_link_contract::events::HarnessStatus {
        status: "online".into(),
        detail: Some("never-copy-native-startup-words".into()),
        harness_version: None,
        sessions_open: 1,
    });
    for _ in 0..2 {
        let mut tx = world.pool.begin().await.unwrap();
        world
            .engine()
            .apply_stream_events(
                &mut tx,
                world.machine_id,
                &Stream::Harness {
                    provider_id: world.provider,
                    agent_id: world.agent,
                },
                &[delivered(&world, 1, &event)],
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(count().await, before + 1);
    let row:Json=world.scalar("SELECT jsonb_build_object('actor',actor_kind,'for_user',for_user_id,'object_kind',object_kind,'details',details) FROM audit_log WHERE object_id=$1 AND action='harness.changed' AND details->>'change'='program_status' ORDER BY at DESC,id DESC LIMIT 1",world.provider).await;
    assert_eq!(row["object_kind"], "provider");
    assert_eq!(row["actor"], "system");
    assert!(row["for_user"].is_null());
    assert!(row["details"]["thread_id"].is_null());
    assert!(row["details"]["turn_id"].is_null());
    assert!(!row.to_string().contains("never-copy-native-startup-words"));
}

#[tokio::test]
async fn retries_publish_each_native_attempt_once_and_clear_on_work_or_completion() {
    let world = World::new().await;
    let thread = world.thread("Retry phases").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Please continue"),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let segment = world.segment_of(thread).await;
    let began = delivered(&world, 1, &turn_began(start.turn_id));
    apply(&world, thread, segment, &[began]).await;
    for (seq, attempt) in [(2, 1), (3, 2), (4, 4)] {
        let event = delivered(
            &world,
            seq,
            &Event::TurnPhase(TurnPhase {
                turn_id: start.turn_id,
                kind: TurnPhaseKind::Retrying,
                retry_attempt: Some(attempt),
                at: Time::new(world.clock.now()),
            }),
        );
        apply(&world, thread, segment, std::slice::from_ref(&event)).await;
        let sync = world.sync(thread).await;
        let phase = sync
            .turns
            .iter()
            .find(|t| t.id == start.turn_id)
            .unwrap()
            .phase
            .as_ref()
            .unwrap();
        assert_eq!(
            phase.kind,
            orch_api_contract::threads::TurnPhaseKind::Retrying
        );
        assert_eq!(phase.retry_attempt, Some(attempt));
        let once = picture(&world, thread).await;
        apply(&world, thread, segment, &[event]).await;
        assert_eq!(
            picture(&world, thread).await,
            once,
            "replay does not reset the status timer"
        );
    }
    let working = delivered(
        &world,
        5,
        &Event::TurnPhase(TurnPhase {
            turn_id: start.turn_id,
            kind: TurnPhaseKind::Working,
            retry_attempt: Some(99),
            at: Time::new(world.clock.now()),
        }),
    );
    apply(&world, thread, segment, &[working]).await;
    let sync = world.sync(thread).await;
    let phase = sync
        .turns
        .iter()
        .find(|t| t.id == start.turn_id)
        .unwrap()
        .phase
        .as_ref()
        .unwrap();
    assert_eq!(
        phase.kind,
        orch_api_contract::threads::TurnPhaseKind::Working
    );
    assert_eq!(
        phase.retry_attempt, None,
        "only native retry phases can carry retry numbers"
    );
    let completed = delivered(
        &world,
        6,
        &turn_ended(start.turn_id, TurnOutcome::Completed),
    );
    apply(&world, thread, segment, &[completed]).await;
    let late = delivered(
        &world,
        7,
        &Event::TurnPhase(TurnPhase {
            turn_id: start.turn_id,
            kind: TurnPhaseKind::Retrying,
            retry_attempt: Some(6),
            at: Time::new(world.clock.now()),
        }),
    );
    apply(&world, thread, segment, &[late]).await;
    assert!(
        world
            .sync(thread)
            .await
            .turns
            .iter()
            .find(|t| t.id == start.turn_id)
            .unwrap()
            .phase
            .is_none(),
        "late status cannot reopen a completed turn"
    );
}

#[path = "ingest/claude_audit.rs"]
mod claude_audit;

#[tokio::test]
async fn a_directory_machines_job_progress_names_its_own_current_command_and_cannot_follow_another_writer()
 {
    use orch_db::queries::{jobs, link};
    use orch_engine::{
        Engine, EngineParts,
        jobs::runner::{Phase, initial_progress, read_progress},
    };
    use orch_link_contract::commands::{MountData, NodeMountMeasure, WorkReport};
    let world = World::observed().await;
    world.engine().stop();
    let engine = Engine::new(EngineParts {
        db: world.pool.clone(),
        clock: world.clock.clone(),
        config: common::config(),
        commands: world.commands.as_engine(),
        access: world.access.clone(),
        live: world.live.clone(),
    });
    let command = orch_types::new_id();
    let mut initial = initial_progress(&[Phase {
        name: "measure",
        weight: 1,
    }]);
    initial.phase = "measure".into();
    initial.phases[0].state = orch_api_contract::workspace::PhaseState::Running;
    let job = jobs::insert(&mut world.pool.acquire().await.unwrap(),&jobs::NewJob {
        id:orch_types::new_id(),kind:"measurement",node_id:None,environment_id:None,provider_id:None,
        plan:&json!({"active_command":{"id":command,"machine":world.machine_id,"kind":"node.mount.measure","reported":true,"phase":"measure"}}),
        progress:&serde_json::to_value(&initial).unwrap(),created_by:Some(world.ada),
    },world.clock.now()).await.unwrap();
    let body = serde_json::to_value(NodeMountMeasure {
        driver: "directory".into(),
        mount_id: orch_types::new_id(),
        data: MountData::Path {
            path: "/srv/data".into(),
        },
        work: Some(WorkReport {
            timeout_policy: None,
            files_total: None,
            bytes_total: None,
            job_id: job.id,
            phase: "measure".into(),
            timeout_secs: 60,
        }),
    })
    .unwrap();
    link::insert_command(
        &mut world.pool.acquire().await.unwrap(),
        &link::NewCommand {
            native_target: None,
            id: command,
            machine_id: world.machine_id,
            kind: "node.mount.measure",
            body: &body,
            serial_key: None,
            deadline_at: world.clock.now() + chrono::Duration::minutes(1),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE commands SET state='accepted' WHERE id=$1")
        .bind(command)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let foreign = world
        .add_machine("Unrelated progress writer", &["environment"])
        .await;
    let mut progress = initial.clone();
    progress.files_done = 17;
    progress.bytes_done = 2048;
    let report = |command_id| {
        Event::JobProgress(orch_link_contract::events::JobProgress {
            command_id,
            job_id: job.id,
            progress: progress.clone(),
            log: None,
        })
    };
    for (machine, event, expected) in [
        (world.machine_id, report(None), 0),
        (foreign.id, report(Some(command)), 0),
        (world.machine_id, report(Some(orch_types::new_id())), 0),
        (world.machine_id, report(Some(command)), 17),
    ] {
        let mut tx = world.pool.begin().await.unwrap();
        engine
            .apply_stream_events(
                &mut tx,
                machine,
                &Stream::Machine,
                &[delivered(&world, 1, &event)],
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let row = jobs::job(&mut world.pool.acquire().await.unwrap(), job.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read_progress(&row).0.files_done, expected);
    }
    sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{active_command,id}',$2) WHERE id=$1")
        .bind(job.id)
        .bind(json!(orch_types::new_id()))
        .execute(world.pool.pool())
        .await
        .unwrap();
    progress.files_done = 99;
    let stale = Event::JobProgress(orch_link_contract::events::JobProgress {
        command_id: Some(command),
        job_id: job.id,
        progress,
        log: None,
    });
    let mut tx = world.pool.begin().await.unwrap();
    engine
        .apply_stream_events(
            &mut tx,
            world.machine_id,
            &Stream::Machine,
            &[delivered(&world, 2, &stale)],
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let row = jobs::job(&mut world.pool.acquire().await.unwrap(), job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        read_progress(&row).0.files_done,
        17,
        "the old command cannot update a replacement stage"
    );
    engine.stop();
}
