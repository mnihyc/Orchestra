//! A harness that is signed in on the machine that has its tools: a turn starts it there on its own sign-in, sends no key and opens no lease on any machine, and is counted from the totals its harness reports when the turn ends.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{MODEL, World, meanwhile, queued, turn_ended};
use orch_agent_contract::descriptor::ArrangementType;
use orch_api_contract::threads::TokenUsage;
use orch_authz::Caller;
use orch_engine::binding::NewThread;
use orch_link_contract::commands::{Command, CommandKind, SessionOpenProvider};
use orch_link_contract::events::{Event, TurnOutcome, UsageUpdated};
use orch_link_contract::streams::Stream;
use orch_types::Id;
use std::time::Duration;

const KEY: &str = "sk-orch-together-3c58e1d7a9b4";

fn totals(input: i64, output: i64) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        cached_input_tokens: 0,
        cache_write_tokens: 0,
        output_tokens: output,
        reasoning_tokens: 0,
        total_tokens: input + output,
    }
}

/// What the harness says this turn and its thread have used in this fixture's first turn.
fn used(turn: Id, input: i64, output: i64) -> Event {
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: totals(input, output),
        thread: totals(input, output),
        context_window: None,
        context_used: None,
        report: None,
    })
}

/// Queues `words` in the thread, which makes the turn that the machine is then asked for.
async fn say(world: &World, thread: Id, words: &str) {
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued(words))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_on_the_machine_that_holds_the_sign_in_starts_the_harness_there_and_sends_no_key_and_opens_no_lease()
 {
    let world = World::new().await;
    let thread = world.thread("Together").await;
    assert_eq!(world.arrangement_of(thread).await, "together");
    say(&world, thread, "Go").await;
    world.run_turn(thread, 0, "Done").await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.ran(CommandKind::AgentStart), 1);
    assert_eq!(world.ran(CommandKind::SessionOpen), 1);
    assert_eq!(world.ran(CommandKind::TurnStart), 1);
    for kind in [
        CommandKind::ProviderStart,
        CommandKind::LeaseOpen,
        CommandKind::LeaseClose,
        CommandKind::ProviderCredentialIssue,
    ] {
        assert_eq!(
            world.ran(kind),
            0,
            "{kind} is for a harness that does not hold its own sign-in"
        );
    }
    // The session is opened on the harness's own sign-in: the provider is named and nothing else.
    let opens = world.seen.of_kind(CommandKind::SessionOpen);
    let Command::SessionOpen(open) = &opens[0] else {
        panic!("a session was opened");
    };
    assert!(
        matches!(&open.provider, SessionOpenProvider::Native { provider_id } if *provider_id == world.provider),
        "{:?}",
        open.provider
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM leases WHERE thread_id = $1", thread)
            .await,
        0,
        "a lease is the leave to use a key that is kept elsewhere"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_use_of_a_turn_on_one_machine_is_put_down_once_from_the_totals_the_harness_reported() {
    let world = World::new().await;
    let thread = world.thread("Counted").await;
    say(&world, thread, "Go").await;
    let start = world.turn_start(thread, 0).await;
    world.answer(thread, &start, "m0", "Done").await;
    world.say(thread, used(start.turn_id, 1_000, 100)).await;
    world.say(thread, used(start.turn_id, 1_500, 200)).await;
    // Nothing is put down while the turn goes on: a turn's total is one line, and it grows.
    meanwhile(Duration::from_millis(200)).await;
    assert!(world.ledger_of(start.turn_id).await.is_empty());
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let rows = world.wait_ledger(start.turn_id, 1).await;
    meanwhile(Duration::from_millis(300)).await;
    let rows_later = world.ledger_of(start.turn_id).await;
    assert_eq!(
        rows_later.len(),
        1,
        "the turn is counted once: {rows_later:?}"
    );
    let row = &rows[0];
    assert_eq!(
        (row.origin.as_str(), row.scope.as_str()),
        ("harness", "turn")
    );
    assert!(row.counted, "with no forwarder the harness is the meter");
    assert_eq!(
        (row.input_tokens, row.output_tokens, row.total_tokens),
        (1_500, 200, 1_700),
        "the last totals it reported"
    );
    assert_eq!(row.model, MODEL);
    assert_eq!(row.provider_id, world.provider);
    assert_eq!(
        row.user_id,
        Some(world.ada),
        "put down to whoever sent the input"
    );
    assert_eq!(row.harness_kind.as_deref(), Some("fake"));
    assert_eq!(row.arrangement.as_deref(), Some("together"));
    assert!(
        row.entry_id.is_none(),
        "no lease, so no entry the use went through"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_failed_after_the_harness_reported_what_it_used_is_counted_and_one_that_used_nothing_is_not()
 {
    let world = World::new().await;
    // This accounting case starts an independent next input rather than waiting for a recovery decision.
    world
        .set(world.topic, "calls.rules", serde_json::json!([]))
        .await;
    let thread = world.thread("Failing").await;
    say(&world, thread, "Go").await;
    let start = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(start.turn_id)).await;
    world.say(thread, used(start.turn_id, 400, 40)).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Failed))
        .await;
    world.wait_turn(start.turn_id, "failed").await;
    let rows = world.wait_ledger(start.turn_id, 1).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].origin.as_str(),
            rows[0].scope.as_str(),
            rows[0].counted
        ),
        ("harness", "turn", true)
    );
    assert_eq!((rows[0].input_tokens, rows[0].output_tokens), (400, 40));
    assert_eq!(rows[0].total_tokens, 440);
    // A harness that never reported a figure leaves the ledger as it is, even with a known rate.
    sqlx::query("UPDATE provider_models SET parameters=parameters || jsonb_build_object('rate',$2::jsonb) WHERE provider_id=$1 AND model_name=$3")
        .bind(world.provider)
        .bind(serde_json::json!({"input_per_mtok":"1","cached_input_per_mtok":"1","output_per_mtok":"1"}))
        .bind(MODEL).execute(world.pool.pool()).await.unwrap();
    world.wait_state(thread, "idle").await;
    say(&world, thread, "Again").await;
    let silent = world.turn_start(thread, 1).await;
    let (turn_usage, thread_usage): (Option<serde_json::Value>, Option<serde_json::Value>) = sqlx::query_as(
        "SELECT u.usage, t.usage FROM turns u JOIN threads t ON t.node_id=u.thread_id WHERE u.id=$1",
    ).bind(silent.turn_id).fetch_one(world.pool.pool()).await.unwrap();
    assert!(
        turn_usage.is_none(),
        "a new turn cannot inherit the previous turn's report: {turn_usage:?}"
    );
    assert_eq!(
        thread_usage.unwrap()["total_tokens"],
        440,
        "the thread's cumulative figure remains separate"
    );
    world.say(thread, common::turn_began(silent.turn_id)).await;
    world
        .say(thread, turn_ended(silent.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(silent.turn_id, "completed").await;
    meanwhile(Duration::from_millis(300)).await;
    let silent_rows = world.ledger_of(silent.turn_id).await;
    assert!(
        silent_rows.is_empty(),
        "an unreported turn invented ledger rows: {silent_rows:?}"
    );
    let previous = world.ledger_of(start.turn_id).await;
    assert_eq!(previous.len(), 1);
    assert_eq!(
        (previous[0].id, previous[0].total_tokens),
        (rows[0].id, 440)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_harness_that_explicitly_reports_zero_keeps_one_known_zero_instead_of_missing_usage() {
    let world = World::new().await;
    // A nonzero synthetic rate makes any invented consumption visible in credits too.
    sqlx::query("UPDATE provider_models SET parameters=parameters || jsonb_build_object('rate',$2::jsonb) WHERE provider_id=$1 AND model_name=$3")
        .bind(world.provider)
        .bind(serde_json::json!({ "input_per_mtok": "1", "cached_input_per_mtok": "1", "output_per_mtok": "1" }))
        .bind(MODEL).execute(world.pool.pool()).await.unwrap();
    let thread = world.thread("Known zero").await;
    say(&world, thread, "Go").await;
    let start = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(start.turn_id)).await;
    world.say(thread, used(start.turn_id, 0, 0)).await;
    world.say(thread, used(start.turn_id, 0, 0)).await;
    common::eventually("the explicit zero report to be stored", || async {
        world
            .scalar::<bool>(
                "SELECT usage IS NOT NULL FROM turns WHERE id=$1",
                start.turn_id,
            )
            .await
    })
    .await;
    assert!(
        world.ledger_of(start.turn_id).await.is_empty(),
        "running totals are not ledger entries"
    );
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let rows = world.wait_ledger(start.turn_id, 1).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(
        (row.origin.as_str(), row.scope.as_str(), row.counted),
        ("harness", "turn", true)
    );
    assert_eq!(
        (
            row.input_tokens,
            row.cached_input_tokens,
            row.cache_write_tokens,
            row.output_tokens,
            row.reasoning_tokens,
            row.total_tokens
        ),
        (0, 0, 0, 0, 0, 0)
    );
    assert_eq!(row.context_tokens, Some(0));
    assert_eq!(row.rate_source.as_deref(), Some("provider"));
    assert_eq!(row.credits.as_deref(), Some("0.000000"));
    let stored: Option<serde_json::Value> = world
        .scalar("SELECT usage FROM turns WHERE id=$1", start.turn_id)
        .await;
    assert_eq!(stored, Some(serde_json::to_value(totals(0, 0)).unwrap()));
    let uncharged: bool = world
        .scalar(
            "SELECT COALESCE(sum(credits),0)=0 FROM usage_events WHERE turn_id=$1 AND counted",
            start.turn_id,
        )
        .await;
    assert!(
        uncharged,
        "an explicitly zero report charges no credits: {rows:?}"
    );
    // The terminal turn has committed all earlier session events; wait for the next cursor,
    // so checking the repeated result cannot race an unapplied completion replay.
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: world.segment_of(thread).await,
    }
    .to_string();
    let cursor = {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_db::queries::link::cursors(&mut conn, world.machine_id)
            .await
            .unwrap()
            .get(&stream)
            .copied()
            .unwrap_or(0)
    };
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    common::eventually("the completion replay to be committed", || async {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_db::queries::link::cursors(&mut conn, world.machine_id)
            .await
            .unwrap()
            .get(&stream)
            .copied()
            .unwrap_or(0)
            > cursor
    })
    .await;
    let repeated = world.ledger_of(start.turn_id).await;
    assert_eq!(repeated.len(), 1);
    assert_eq!(repeated[0].id, row.id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_observed_request_keeps_its_performance_when_its_token_counts_are_unknown() {
    use orch_agent_contract::requests::UsageRequest;
    use orch_types::{Clock, ManualClock};
    use std::sync::Arc;
    let clock = Arc::new(ManualClock::new("2026-10-16T01:00:00Z".parse().unwrap()));
    let world = World::with_clock(clock.clone()).await;
    let thread = world.thread("Observed request").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let mut request = UsageRequest {
        id: "request-without-counts".to_owned(),
        model: Some(MODEL.to_owned()),
        asked: Some(clock.time()),
        ..UsageRequest::default()
    };
    world
        .say(
            thread,
            Event::ModelRequest(orch_link_contract::events::ModelRequest {
                turn_id: start.turn_id,
                request: request.clone(),
            }),
        )
        .await;
    clock.advance(chrono::Duration::seconds(1));
    request.first_output = Some(clock.time());
    request.output_streamed = Some(true);
    clock.advance(chrono::Duration::seconds(2));
    request.end = Some(clock.time());
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
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let rows = world.wait_ledger(start.turn_id, 1).await;
    assert_eq!(
        rows.len(),
        1,
        "no additional cumulative fallback or model request was invented: {rows:?}"
    );
    let row = &rows[0];
    assert_eq!(
        (row.origin.as_str(), row.scope.as_str(), row.counted),
        ("harness", "request", true)
    );
    assert_eq!(row.request_ref.as_deref(), Some("request-without-counts"));
    assert_eq!(
        (row.input_tokens, row.output_tokens, row.total_tokens),
        (0, 0, 0)
    );
    let timing = row.timing();
    let reported = timing.reported_tokens.as_ref().unwrap();
    assert_eq!(
        (
            reported.input_tokens,
            reported.cached_input_tokens,
            reported.cache_write_tokens,
            reported.output_tokens,
            reported.reasoning_tokens,
            reported.total_tokens
        ),
        (None, None, None, None, None, None)
    );
    assert_eq!(timing.first_token_ms.value(), Some(&1_000));
    assert_eq!(timing.request_ms.value(), Some(&3_000));
    assert!(timing.speed_milli.value().is_none());
    assert_eq!(row.outcome.as_deref(), Some("done"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn where_a_forwarder_counts_the_requests_the_totals_of_the_harness_make_no_second_count() {
    let world = World::on_a_key(KEY).await;
    // The creator asks for the arrangement in which the key stays with its provider.
    let thread = world
        .engine()
        .create_thread(
            &Caller::session(world.ada),
            NewThread {
                topic: world.topic,
                name: "Forwarded".to_owned(),
                agent: world.agent,
                provider: world.provider,
                environment: world.environment,
                directory: "web".to_owned(),
                arrangement: Some(ArrangementType::KeySeparated),
                model: Some(MODEL.to_owned()),
                effort: None,
            },
        )
        .await
        .unwrap()
        .thread;
    assert_eq!(world.arrangement_of(thread).await, "key_separated");
    say(&world, thread, "Go").await;
    let start = world.turn_start(thread, 0).await;
    world.answer(thread, &start, "m0", "Done").await;
    world.say(thread, used(start.turn_id, 900, 90)).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    meanwhile(Duration::from_millis(300)).await;
    assert!(
        world.ledger_of(start.turn_id).await.is_empty(),
        "the lease's rows are the counted ones, and this turn's lease reported none"
    );
}
