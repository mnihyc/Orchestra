//! Exactly one source counts a turn: the forwarder's requests where the turn went through one, else what the harness reported once; and what Orch asked of a model for itself is a row that always counts, against the provider and the caps that are shared and never against a person's own.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use chrono::Utc;
use common::{MODEL, World, meanwhile, turn_ended};
use orch_db::usage::ledger::Tokens;
use orch_engine::caps;
use orch_engine::turns::{OwnPurpose, OwnUse};
use orch_link_contract::events::{Event, LeaseUsage, TurnOutcome, UsageUpdated};
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
        person: Some(bob),
        model: MODEL,
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
            Some(bob),
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
