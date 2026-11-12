//! Exactly one source counts a turn: the forwarder's requests where the turn went through one, else what the harness reported once; and what Orch asked of a model for itself is a row that always counts, against the provider and the caps that are shared and never against a person's own.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use chrono::Utc;
use common::{MODEL, World, meanwhile, turn_ended};
use orch_agent_contract::requests::{RequestTokens, UsageRequest};
use orch_api_contract::context::TurnUsageReport;
use orch_api_contract::threads::TokenUsage;
use orch_db::usage::ledger::Tokens;
use orch_engine::caps;
use orch_engine::turns::{OwnPurpose, OwnUse};
use orch_link_contract::events::{Event, LeaseUsage, ModelRequest, TurnOutcome, UsageUpdated};
use orch_types::{Id, new_id};
use serde_json::json;
use std::time::Duration;

const KEY: &str = "sk-orch-test-rows";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ending_a_turn_closes_a_missing_request_report_without_inventing_times_or_counts() {
    let world = World::new().await;
    let thread = world.thread("Abandoned request").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let request = orch_agent_contract::requests::UsageRequest {
        id: "still-running".to_owned(),
        asked: Some(world.clock.time()),
        ..Default::default()
    };
    world
        .say(
            thread,
            Event::ModelRequest(orch_link_contract::events::ModelRequest {
                turn_id: start.turn_id,
                request,
            }),
        )
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Interrupted))
        .await;
    world.wait_turn(start.turn_id, "interrupted").await;
    common::eventually("the terminal request/accounting transaction", || async {
        world
            .scalar::<bool>(
                "SELECT end_work IS NULL FROM turns WHERE id=$1",
                start.turn_id,
            )
            .await
    })
    .await;
    let rows = world.wait_ledger(start.turn_id, 1).await;
    let row = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("still-running"))
        .unwrap();
    assert_eq!(row.outcome.as_deref(), Some("forced"));
    assert!(row.timing().end.is_none());
    assert!(row.request_ms.is_none());
    assert!(
        row.timing()
            .reported_tokens
            .as_ref()
            .unwrap()
            .output_tokens
            .is_none()
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let running: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_running WHERE turn_id=$1")
        .bind(start.turn_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(running, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_adapter_request_has_identical_performance_on_a_plan_and_a_forwarded_key_without_double_counting_tokens()
 {
    for forwarded in [false, true] {
        let world = if forwarded {
            World::on_a_key(KEY).await
        } else {
            World::new().await
        };
        let thread = world.thread("Measured request").await;
        let (start, _) = world.running_turn(thread, "Go").await;
        let mut request = orch_agent_contract::requests::UsageRequest {
            id: "request-1".into(),
            model: Some(MODEL.to_owned()),
            asked: Some("2026-10-15T00:00:00Z".parse().unwrap()),
            ..Default::default()
        };
        let event = |request| {
            Event::ModelRequest(orch_link_contract::events::ModelRequest {
                turn_id: start.turn_id,
                request,
            })
        };
        world.say(thread, event(request.clone())).await;
        request.first_output = Some("2026-10-15T00:00:01Z".parse().unwrap());
        request.output_streamed = Some(true);
        world.say(thread, event(request.clone())).await;
        request.end = Some("2026-10-15T00:00:03Z".parse().unwrap());
        request.reasoning_hidden = true;
        request.tokens = orch_agent_contract::requests::RequestTokens {
            input_tokens: Some(400),
            cached_input_tokens: Some(0),
            cache_write_tokens: Some(0),
            output_tokens: Some(80),
            reasoning_tokens: Some(20),
            total_tokens: Some(480),
        };
        request.finish("done", None, None);
        if forwarded {
            let lease = world.open_lease_of(thread).await;
            world
                .say_of_provider(forwarder_says(lease, 400, 80, "upstream-1"))
                .await;
        }
        world.say(thread, event(request)).await;
        world
            .say(thread, harness_says(start.turn_id, 400, 80))
            .await;
        world
            .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
            .await;
        world.wait_turn(start.turn_id, "completed").await;
        let rows = world
            .wait_ledger(start.turn_id, if forwarded { 2 } else { 1 })
            .await;
        assert_eq!(
            rows.iter()
                .filter(|row| row.counted)
                .map(|row| row.total_tokens)
                .sum::<i64>(),
            480
        );
        let measured = rows
            .iter()
            .find(|row| row.outcome.as_deref() == Some("done"))
            .unwrap();
        assert_eq!(measured.timing().speed_milli.value(), Some(&30_000));
        assert_eq!(measured.timing().first_token_ms.value(), Some(&1000));
        assert_eq!(measured.counted, !forwarded);
        let mut conn = world.pool.acquire().await.unwrap();
        let (requests,speed,first):(i64,serde_json::Value,serde_json::Value)=sqlx::query_as("SELECT COALESCE(sum(requests),0)::bigint,usage_merge_counts(speed_buckets),usage_merge_counts(first_token_buckets) FROM usage_daily WHERE thread_id=$1")
            .bind(thread).fetch_one(&mut *conn).await.unwrap();
        assert_eq!(requests, 1);
        assert_eq!(
            orch_db::usage::performance::percentile(&speed, 50)
                .map(|n| (n * 1000.0).round() as i64),
            Some(30_643)
        );
        assert_eq!(
            orch_db::usage::performance::percentile(&first, 50).map(|n| n.ceil() as i64),
            Some(981)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_request_replayed_late_keeps_its_observed_time_and_turn_details_keep_timed_main_calls()
 {
    use orch_types::{Clock, ManualClock};
    use std::sync::Arc;
    let clock = Arc::new(ManualClock::new("2026-10-15T00:00:00Z".parse().unwrap()));
    let world = World::with_clock(clock.clone()).await;
    let thread = world.thread("Delayed request report").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let asked = clock.time();
    clock.advance(chrono::Duration::seconds(1));
    let first = clock.time();
    clock.advance(chrono::Duration::seconds(2));
    let end = clock.time();
    clock.advance(chrono::Duration::hours(1));
    let mut raw = orch_agent_contract::requests::UsageRequest {
        id: "replayed".into(),
        asked: Some(asked),
        first_output: Some(first),
        end: Some(end),
        reasoning_hidden: true,
        output_streamed: Some(true),
        tokens: orch_agent_contract::requests::RequestTokens {
            output_tokens: Some(100),
            reasoning_tokens: Some(20),
            ..Default::default()
        },
        ..Default::default()
    };
    raw.finish("done", None, None);
    world
        .say(
            thread,
            Event::ModelRequest(orch_link_contract::events::ModelRequest {
                turn_id: start.turn_id,
                request: raw,
            }),
        )
        .await;
    for (id, subagent) in [("child", true), ("compaction", false)] {
        let mut raw = orch_agent_contract::requests::UsageRequest {
            id: id.into(),
            subagent,
            ..Default::default()
        };
        raw.finish("done", None, None);
        world
            .say(
                thread,
                Event::ModelRequest(orch_link_contract::events::ModelRequest {
                    turn_id: start.turn_id,
                    request: raw,
                }),
            )
            .await;
    }
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let rows = world.wait_ledger(start.turn_id, 3).await;
    assert_eq!(
        rows.iter()
            .find(|row| row.request_ref.as_deref() == Some("replayed"))
            .unwrap()
            .at,
        end.get()
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let recent:i64=sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE turn_id=$1 AND request_ref='replayed' AND at >= $2").bind(start.turn_id).bind(clock.now()-chrono::Duration::minutes(15)).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        recent, 0,
        "replay does not turn an old request into current provider degradation"
    );
    sqlx::query("UPDATE provider_models SET display_name=$2 WHERE provider_id=$1")
        .bind(world.provider)
        .bind("Measured model")
        .execute(&mut *conn)
        .await
        .unwrap();
    let cost = orch_engine::context_fill::turn_cost(&mut conn, thread, start.turn_id)
        .await
        .unwrap()
        .unwrap();
    assert!(!cost.rows.is_empty());
    assert!(
        cost.rows
            .iter()
            .all(|row| row.model_display.as_deref() == Some("Measured model")),
        "a person reads the declared model name beside its price"
    );
    sqlx::query("UPDATE provider_models SET display_name='' WHERE provider_id=$1")
        .bind(world.provider)
        .execute(&mut *conn)
        .await
        .unwrap();
    let unnamed = orch_engine::context_fill::turn_cost(&mut conn, thread, start.turn_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        unnamed.rows.iter().all(|row| row.model_display.is_none()),
        "a route id is not fabricated into a missing display name"
    );
    let performance = cost.performance.unwrap();
    assert_eq!(performance.model_ms, Some(3000));
    assert!(performance.tool_ms.is_some());
    assert!(performance.tool_includes_subagents);
    assert!(performance.tool_includes_unmeasured);
    assert!(performance.reasoning_hidden);
}

fn harness_says(turn: Id, input: i64, output: i64) -> Event {
    let usage = json!({
        "input_tokens": input, "cached_input_tokens": 0, "output_tokens": output,
        "reasoning_tokens": 0, "total_tokens": input + output
    });
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: serde_json::from_value(usage.clone()).unwrap(),
        thread: serde_json::from_value(usage).unwrap(),
        context_window: None,
        context_used: None,
        report: None,
    })
}

fn forwarder_says(lease: Id, input: i64, output: i64, named: &str) -> Event {
    Event::LeaseUsage(LeaseUsage {
        lease_id: lease,
        model: MODEL.to_owned(),
        usage: serde_json::from_value(json!({
            "input_tokens": input, "cached_input_tokens": 0, "output_tokens": output,
            "reasoning_tokens": 0, "total_tokens": input + output
        }))
        .unwrap(),
        request_ref: Some(named.to_owned()),
        timing: None,
    })
}

async fn rolled_up(world: &World) -> (i64, i64) {
    sqlx::query_as(
        "SELECT COALESCE(sum(turns), 0)::bigint, COALESCE(sum(total_tokens), 0)::bigint FROM usage_daily",
    )
    .fetch_one(world.pool.pool())
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_a_harness_reports_of_a_turn_nobody_else_counted_is_one_counted_row_and_one_turn_of_the_day()
 {
    let world = World::new().await;
    let thread = world.thread("Native").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world
        .say(thread, harness_says(start.turn_id, 400, 20))
        .await;
    world
        .say(thread, harness_says(start.turn_id, 1_200, 80))
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let rows = world.wait_ledger(start.turn_id, 1).await;
    meanwhile(Duration::from_millis(300)).await;
    let rows_later = world.ledger_of(start.turn_id).await;
    assert_eq!(rows_later.len(), 1, "{rows_later:?}");
    let row = &rows[0];
    assert_eq!(
        (row.origin.as_str(), row.scope.as_str()),
        ("harness", "turn")
    );
    assert!(row.counted);
    assert_eq!(
        (row.input_tokens, row.output_tokens, row.total_tokens),
        (1_200, 80, 1_280)
    );
    assert_eq!(row.thread_id, Some(thread));
    assert_eq!(row.user_id, Some(world.ada));
    assert_eq!(rolled_up(&world).await, (1, 1_280), "the day knows it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_the_forwarder_counted_is_not_counted_again_from_what_its_harness_reported() {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Forwarded").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;
    world
        .say_of_provider(forwarder_says(lease, 1_000, 100, "resp_1"))
        .await;
    world.wait_ledger(start.turn_id, 1).await;
    world
        .say(thread, harness_says(start.turn_id, 1_200, 90))
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    meanwhile(Duration::from_millis(400)).await;
    let rows = world.ledger_of(start.turn_id).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].origin, "forwarder");
    assert_eq!(rolled_up(&world).await, (1, 1_100));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_orch_asks_of_a_model_for_itself_is_one_row_that_counts_for_the_shared_caps_and_never_a_persons_own()
 {
    let world = World::new().await;
    let thread = world.thread("Summed").await;
    let bob = world.person("bob").await;
    let call = new_id();
    let own = OwnUse {
        purpose: OwnPurpose::Summary,
        timing: None,
        id: call,
        at: Utc::now(),
        topic: world.topic,
        thread: Some(thread),
        provider: world.provider,
        entry: None,
        person: None,
        model: MODEL,
        protocol: None,
        tokens: Tokens {
            input: 400,
            cached_input: 0,
            cache_write: 0,
            output: 20,
            reasoning: 0,
            total: 420,
        },
        request_ref: Some("resp_sum"),
    };
    let mut conn = world.pool.acquire().await.unwrap();
    assert!(world.engine().count_own_use(&mut conn, &own).await.unwrap());
    assert!(
        !world.engine().count_own_use(&mut conn, &own).await.unwrap(),
        "a call counted twice is one row"
    );
    let row: (String, String, String, bool, Option<Id>, Option<Id>, Option<Id>) = sqlx::query_as(
        "SELECT origin, purpose, scope, counted, user_id, turn_id, thread_id FROM usage_events WHERE id = $1",
    )
    .bind(call)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "orch".to_owned(),
            "summary".to_owned(),
            "request".to_owned(),
            true,
            None,
            None,
            Some(thread)
        )
    );
    assert_eq!(rolled_up(&world).await, (0, 420), "a summary is no turn");

    // A shared cap covers it; the cap of a person's own does not.
    let shared: orch_db::usage::caps::CapRow = {
        let id = new_id();
        sqlx::query(
            "INSERT INTO usage_caps (id, node_id, meter, period, amount, created_by) VALUES ($1, $2, 'tokens', 'day', 1000, $3)",
        )
        .bind(id)
        .bind(world.project)
        .bind(world.ada)
        .execute(&mut *conn)
        .await
        .unwrap();
        orch_db::usage::caps::get(&mut conn, id)
            .await
            .unwrap()
            .unwrap()
    };
    let each: orch_db::usage::caps::CapRow = {
        let id = new_id();
        sqlx::query(
            "INSERT INTO usage_caps (id, node_id, split, meter, period, amount, created_by) VALUES ($1, $2, 'each', 'tokens', 'day', 1000, $3)",
        )
        .bind(id)
        .bind(world.project)
        .bind(world.ada)
        .execute(&mut *conn)
        .await
        .unwrap();
        orch_db::usage::caps::get(&mut conn, id)
            .await
            .unwrap()
            .unwrap()
    };
    let now = Utc::now();
    let shared_standing = caps::standing(&mut conn, &shared, None, now).await.unwrap();
    let own_standing = caps::standing(&mut conn, &each, Some(bob), now)
        .await
        .unwrap();
    assert_eq!(shared_standing.used.text(), "420");
    assert_eq!(own_standing.used.text(), "0");

    // When a shared cap has no room, summaries are skipped; a cap on turns at once does not hold them back.
    let engine = world.engine();
    assert!(
        engine
            .room_for_summary(&mut conn, world.topic, world.provider, MODEL)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE usage_caps SET amount = 420 WHERE id = $1")
        .bind(shared.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        !engine
            .room_for_summary(&mut conn, world.topic, world.provider, MODEL)
            .await
            .unwrap()
    );
    sqlx::query("DELETE FROM usage_caps WHERE id = $1")
        .bind(shared.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO usage_caps (id, node_id, meter, amount, created_by) VALUES ($1, $2, 'running', 0, $3)",
    )
    .bind(new_id())
    .bind(world.topic)
    .bind(world.ada)
    .execute(&mut *conn)
    .await
    .unwrap();
    assert!(
        engine
            .room_for_summary(&mut conn, world.topic, world.provider, MODEL)
            .await
            .unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_owned_reroute_keeps_the_choice_and_moves_request_and_context_facts_to_the_actual_model()
{
    let world = World::new().await;
    let thread = world.thread("Native effective model").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let actual = "native-effective";
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) VALUES($1,$2,$3,'Effective model','{}',true,'manual',$4)")
        .bind(new_id()).bind(world.provider).bind(actual).bind(world.ada).execute(world.pool.pool()).await.unwrap();
    let mut first = harness_says(start.turn_id, 10, 1);
    if let Event::UsageUpdated(usage) = &mut first {
        usage.context_window = Some(200_000);
        usage.context_used = Some(10);
    }
    world.say(thread, first).await;
    common::eventually("initial context window", || async {
        world
            .scalar::<bool>(
                "SELECT COALESCE(usage->>'context_window'='200000',false) FROM threads WHERE node_id=$1",
                thread,
            )
            .await
    })
    .await;
    let before: serde_json::Value = world
        .scalar(
            "SELECT jsonb_build_object('model',model,'params',params) FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    let started=Event::decode("item.started",json!({"turn_id":start.turn_id,"native_ref":"native-reroute","stream":"main","kind":"error","payload":{"code":"model_rerouted","message":"The native program chose its effective model.","retryable":false}})).unwrap();
    world.say(thread, started).await;
    world.say(thread,Event::decode("item.completed",json!({"native_ref":"native-reroute","status":"completed","payload":{"code":"model_rerouted","message":"The native program chose its effective model.","retryable":false,"details":{"turn_id":start.turn_id,"from_model":MODEL,"to_model":actual}}})).unwrap()).await;
    common::eventually("owned actual model",||async {world.scalar::<bool>("SELECT COALESCE(usage_detail->>'effective_model'='native-effective',false) FROM turns WHERE id=$1",start.turn_id).await}).await;
    let pending: serde_json::Value = world
        .scalar("SELECT usage FROM threads WHERE node_id=$1", thread)
        .await;
    assert!(
        pending["context_window"].is_null(),
        "the old model window is not the new model's window"
    );
    let after: serde_json::Value = world
        .scalar(
            "SELECT jsonb_build_object('model',model,'params',params) FROM turns WHERE id=$1",
            start.turn_id,
        )
        .await;
    assert_eq!(
        before, after,
        "native observation never replaces the person's chosen model/parameters"
    );
    let mut next = harness_says(start.turn_id, 100, 20);
    if let Event::UsageUpdated(usage) = &mut next {
        usage.context_window = Some(100_000);
        usage.context_used = Some(100);
    }
    world.say(thread, next).await;
    common::eventually("effective model context",||async {world.scalar::<bool>("SELECT COALESCE(usage->>'context_window'='100000' AND usage->>'context_model'='native-effective',false) FROM threads WHERE node_id=$1",thread).await}).await;
    let mut request = orch_agent_contract::requests::UsageRequest {
        id: "effective-request".into(),
        ..Default::default()
    };
    request.tokens.input_tokens = Some(100);
    request.tokens.output_tokens = Some(20);
    request.finish("done", None, None);
    world
        .say(
            thread,
            Event::ModelRequest(orch_link_contract::events::ModelRequest {
                turn_id: start.turn_id,
                request,
            }),
        )
        .await;
    common::eventually("effective model request",||async {world.scalar::<bool>("SELECT EXISTS(SELECT 1 FROM usage_events WHERE turn_id=$1 AND request_ref='effective-request' AND model='native-effective')",start.turn_id).await}).await;
    let sync = world.sync(thread).await;
    let turn = sync
        .turns
        .iter()
        .find(|turn| turn.id == start.turn_id)
        .unwrap();
    assert_eq!(
        turn.usage_detail
            .as_ref()
            .unwrap()
            .effective_model
            .as_deref(),
        Some(actual)
    );
    assert_eq!(
        turn.usage_detail
            .as_ref()
            .unwrap()
            .effective_model_display
            .as_deref(),
        Some("Effective model")
    );
    assert!(sync.items.iter().any(|item| {
        serde_json::to_value(&item.payload)
            .unwrap()
            .pointer("/payload/details/to_display")
            == Some(&json!("Effective model"))
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issued_summaries_use_the_issuers_summary_week_and_automatic_work_has_no_person() {
    use orch_engine::caps::OwnCapAsk;
    use orch_types::{Clock, ManualClock};
    use std::sync::Arc;
    let clock = Arc::new(ManualClock::new("2026-11-02T12:00:00Z".parse().unwrap()));
    let world = World::with_clock(clock.clone()).await;
    let thread = world.thread("Personal summary allowance").await;
    let bob = world.person("bob").await;
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE provider_models SET parameters=parameters || jsonb_build_object('rate',$1::jsonb) WHERE provider_id=$2 AND model_name=$3")
        .bind(json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1","output_per_mtok":"2"}))
        .bind(world.provider).bind(MODEL).execute(&mut *conn).await.unwrap();
    let cap_id = new_id();
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,purposes,warn_percent,created_by) VALUES($1,$2,'members',$3,'each','credits','week',0.001,ARRAY['summary'],50,$4)")
        .bind(cap_id).bind(world.project).bind(world.root).bind(world.ada).execute(&mut *conn).await.unwrap();
    let engine = world.engine();
    let request = OwnCapAsk {
        place: thread,
        provider: world.provider,
        model: MODEL,
        tier: None,
        purpose: "summary",
        person: Some(world.ada),
    };
    assert!(engine.room_for_own_use(&mut conn, &request).await.unwrap());
    let own = OwnUse {
        purpose: OwnPurpose::Summary,
        timing: None,
        id: new_id(),
        at: clock.now(),
        topic: world.topic,
        thread: Some(thread),
        provider: world.provider,
        entry: None,
        person: Some(world.ada),
        model: MODEL,
        protocol: None,
        tokens: Tokens {
            input: 1000,
            total: 1000,
            ..Default::default()
        },
        request_ref: Some("issued-summary"),
    };
    assert!(engine.count_own_use(&mut conn, &own).await.unwrap());
    assert!(
        !engine.count_own_use(&mut conn, &own).await.unwrap(),
        "replay is the same native ledger row"
    );
    assert!(!engine.room_for_own_use(&mut conn, &request).await.unwrap());
    assert!(
        engine
            .room_for_own_use(
                &mut conn,
                &OwnCapAsk {
                    person: Some(bob),
                    ..request
                }
            )
            .await
            .unwrap()
    );
    assert!(
        engine
            .room_for_own_use(
                &mut conn,
                &OwnCapAsk {
                    person: None,
                    ..request
                }
            )
            .await
            .unwrap()
    );
    assert!(
        engine
            .room_for_own_use(
                &mut conn,
                &OwnCapAsk {
                    purpose: "turn",
                    ..request
                }
            )
            .await
            .unwrap(),
        "a summary cap does not bound ordinary generation"
    );
    let cap = orch_db::usage::caps::get(&mut conn, cap_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        caps::standing(&mut conn, &cap, Some(world.ada), clock.now())
            .await
            .unwrap()
            .used
            .text(),
        "0.001"
    );
    assert_eq!(
        caps::standing(&mut conn, &cap, Some(bob), clock.now())
            .await
            .unwrap()
            .used
            .text(),
        "0"
    );
    let automatic = OwnUse {
        id: new_id(),
        person: None,
        request_ref: Some("automatic-summary"),
        ..own.clone()
    };
    assert!(engine.count_own_use(&mut conn, &automatic).await.unwrap());
    let row: (Option<Id>, String, Option<String>) =
        sqlx::query_as("SELECT user_id,purpose,credits::text FROM usage_events WHERE id=$1")
            .bind(automatic.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(row, (None, "summary".into(), Some("0.001000".into())));
    assert_eq!(
        caps::standing(&mut conn, &cap, Some(world.ada), clock.now())
            .await
            .unwrap()
            .used
            .text(),
        "0.001"
    );
    let reached: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='usage_cap.reached' AND object_id=$1",
    )
    .bind(cap_id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        reached, 1,
        "the original cap audit producer runs once for the real issuer"
    );
    clock.advance(chrono::Duration::days(7));
    assert!(
        engine.room_for_own_use(&mut conn, &request).await.unwrap(),
        "the existing UTC week starts a fresh allowance"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_owned_questions_cap_and_notice_use_its_governing_org_instead_of_thread_storage() {
    let world = World::new().await;
    let thread = world.thread("Stored elsewhere").await;
    let (start, _) = world.running_turn(thread, "An actual retained turn").await;
    let mut tx = world.pool.begin().await.unwrap();
    let native = orch_db::usage::ledger::attribution_of_turn(&mut tx, start.turn_id)
        .await
        .unwrap()
        .unwrap();
    let governing = new_id();
    orch_db::queries::hierarchy::testing::insert_node(
        &mut tx,
        governing,
        "org",
        None,
        "Knowledge boundary",
        world.ada,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE provider_models SET parameters=parameters || jsonb_build_object('rate',$1::jsonb) WHERE provider_id=$2 AND model_name=$3")
        .bind(json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1","output_per_mtok":"2"})).bind(world.provider).bind(MODEL).execute(&mut *tx).await.unwrap();
    let cap_id = new_id();
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,meter,period,amount,purposes,created_by) VALUES($1,$2,'user',$3,'credits','week',0.001,ARRAY['ask'],$3)")
        .bind(cap_id).bind(governing).bind(world.ada).execute(&mut *tx).await.unwrap();
    let own = OwnUse {
        purpose: OwnPurpose::Ask,
        timing: None,
        id: new_id(),
        at: world.clock.now(),
        topic: governing,
        thread: Some(thread),
        provider: world.provider,
        entry: None,
        person: Some(world.ada),
        model: MODEL,
        protocol: None,
        tokens: Tokens {
            input: 1000,
            total: 1000,
            ..Default::default()
        },
        request_ref: Some("governed-question"),
    };
    let engine = world.engine();
    assert!(
        engine
            .count_own_use_context(&mut tx, &own, Some((start.turn_id, native.segment_id)))
            .await
            .unwrap()
    );
    let cap = orch_db::usage::caps::get(&mut tx, cap_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        caps::standing(&mut tx, &cap, Some(world.ada), world.clock.now())
            .await
            .unwrap()
            .used
            .text(),
        "0.001"
    );
    let people = orch_db::usage::caps::users_of(&mut tx, &cap, None, world.clock.now())
        .await
        .unwrap();
    assert_eq!(people.len(), 1);
    assert_eq!(people[0].user_id, world.ada);
    let place = orch_db::usage::rollup::place_of(&mut tx, governing)
        .await
        .unwrap()
        .unwrap();
    let filter = orch_db::usage::rollup::Filter {
        node: Some(place),
        purpose: Some("ask".into()),
        ..Default::default()
    };
    let cells = orch_db::usage::rollup::read(
        &mut tx,
        &orch_db::usage::rollup::Read {
            from: world.clock.now().date_naive(),
            to: world.clock.now().date_naive(),
            filter: &filter,
            bucket: None,
            by: None,
            under: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0].figures.credits, "0.001000");
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='usage_cap.reached' AND object_id=$1",
    )
    .bind(cap_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        audits, 1,
        "notice coverage also uses the retained governing root"
    );
    tx.commit().await.unwrap();
}

// ---- a turn is closed as its use less what its rows carry, whenever the rows come

fn tokens(input: i64, output: i64) -> TokenUsage {
    serde_json::from_value(json!({
        "input_tokens": input, "cached_input_tokens": 0, "output_tokens": output,
        "reasoning_tokens": 0, "total_tokens": input + output
    }))
    .unwrap()
}

/// A use said with the statement of what its rows carry (`None`: nothing stated).
fn says_carried(turn: Id, used: (i64, i64), carried: Option<(i64, i64)>) -> Event {
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: tokens(used.0, used.1),
        thread: tokens(used.0, used.1),
        context_window: None,
        context_used: None,
        report: Some(TurnUsageReport {
            carried_by_rows: carried.map(|(input, output)| tokens(input, output)),
            ..TurnUsageReport::default()
        }),
    })
}

fn a_row(turn: Id, name: &str, input: i64, output: i64) -> Event {
    let mut request = UsageRequest {
        id: name.to_owned(),
        model: Some(MODEL.to_owned()),
        tokens: RequestTokens {
            input_tokens: Some(input),
            output_tokens: Some(output),
            ..RequestTokens::default()
        },
        ..UsageRequest::default()
    };
    request.finish("done", None, None);
    Event::ModelRequest(ModelRequest {
        turn_id: turn,
        request,
    })
}

/// What the ledger counts for the turn, by part, and how many rows hold it.
async fn counted_of(world: &World, turn: Id) -> (i64, i64, usize) {
    let rows = world.ledger_of(turn).await;
    let counted: Vec<_> = rows.iter().filter(|row| row.counted).collect();
    (
        counted.iter().map(|row| row.input_tokens).sum(),
        counted.iter().map(|row| row.output_tokens).sum(),
        counted.len(),
    )
}

/// Which way the rows come, against the completion of the turn and the follow-up that closes its use.
#[derive(Clone, Copy, Debug)]
enum Rows {
    BeforeTheCompletion,
    RightAfterTheCompletion,
    AfterTheFollowUp,
}

async fn a_turn_with_a_row(
    way: Rows,
    used: (i64, i64),
    carried: Option<(i64, i64)>,
    row: (i64, i64),
) -> (i64, i64, usize) {
    let world = World::new().await;
    let thread = world.thread("Carried").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let turn = start.turn_id;
    world.say(thread, says_carried(turn, used, None)).await;
    world.say(thread, says_carried(turn, used, carried)).await;
    match way {
        Rows::BeforeTheCompletion => {
            world.say(thread, a_row(turn, "child-1", row.0, row.1)).await;
            world
                .say(thread, turn_ended(turn, TurnOutcome::Completed))
                .await;
            world.wait_turn(turn, "completed").await;
        }
        Rows::RightAfterTheCompletion => {
            world
                .say(thread, turn_ended(turn, TurnOutcome::Completed))
                .await;
            world.say(thread, a_row(turn, "child-1", row.0, row.1)).await;
            world.wait_turn(turn, "completed").await;
        }
        Rows::AfterTheFollowUp => {
            world
                .say(thread, turn_ended(turn, TurnOutcome::Completed))
                .await;
            world.wait_turn(turn, "completed").await;
            world.wait_ledger(turn, 1).await;
            meanwhile(Duration::from_millis(200)).await;
            world.say(thread, a_row(turn, "child-1", row.0, row.1)).await;
        }
    }
    common::eventually("the rows of the turn are all in the ledger", || async {
        counted_of(&world, turn).await.2 >= 2
    })
    .await;
    meanwhile(Duration::from_millis(300)).await;
    counted_of(&world, turn).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_whose_use_is_stated_to_be_carried_in_part_by_its_rows_counts_the_same_whenever_the_rows_come()
 {
    // A use of 500 / 50 whose row of 300 / 30 is inside it: 200 / 20 are the turn's own, in whichever order.
    let mut outcomes = Vec::new();
    for way in [
        Rows::BeforeTheCompletion,
        Rows::RightAfterTheCompletion,
        Rows::AfterTheFollowUp,
    ] {
        let counted = a_turn_with_a_row(way, (500, 50), Some((300, 30)), (300, 30)).await;
        outcomes.push((way, counted));
    }
    for (way, counted) in &outcomes {
        assert_eq!(
            *counted,
            (500, 50, 2),
            "the rows came {way:?}: the turn's own 200 / 20 and the row's 300 / 30, once each"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_the_use_does_not_hold_is_added_to_it_and_not_taken_from_it_whenever_it_comes() {
    // A use of 200 / 30 that holds none of the row of 5 000 / 400 (stated: nothing carried): both count whole,
    // though the row is larger than the use, which the turn shows as the larger of the two.
    for way in [
        Rows::BeforeTheCompletion,
        Rows::RightAfterTheCompletion,
        Rows::AfterTheFollowUp,
    ] {
        let counted = a_turn_with_a_row(way, (200, 30), Some((0, 0)), (5_000, 400)).await;
        assert_eq!(counted, (5_200, 430, 2), "the rows came {way:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_statement_is_not_waited_for_its_rows_and_a_use_said_without_one_is_closed_by_the_rows_it_has() {
    // Stated, and the row never comes: the turn is closed as 500 / 50 less the 300 / 30 stated, and no more.
    let world = World::new().await;
    let thread = world.thread("Stated").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world
        .say(thread, says_carried(start.turn_id, (500, 50), Some((300, 30))))
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.wait_ledger(start.turn_id, 1).await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(counted_of(&world, start.turn_id).await, (200, 20, 1));
    // Nothing stated: the engine's own rule, the use less the rows it has when it closes the turn.
    let thread = world.thread("Not stated").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world
        .say(thread, says_carried(start.turn_id, (500, 50), None))
        .await;
    world
        .say(thread, a_row(start.turn_id, "child-1", 300, 30))
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.wait_ledger(start.turn_id, 2).await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(counted_of(&world, start.turn_id).await, (500, 50, 2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_statement_belongs_to_the_use_it_came_with() {
    // The use is said again, larger, without a statement: the earlier statement does not stand for it.
    let world = World::new().await;
    let thread = world.thread("Said again").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world
        .say(thread, says_carried(start.turn_id, (500, 50), Some((300, 30))))
        .await;
    world
        .say(thread, says_carried(start.turn_id, (900, 90), None))
        .await;
    world
        .say(thread, a_row(start.turn_id, "child-1", 300, 30))
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.wait_ledger(start.turn_id, 2).await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(counted_of(&world, start.turn_id).await, (900, 90, 2));
}
