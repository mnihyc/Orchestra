//! Any program's real per-model aggregates reach the same ledger without another request or token charge.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
use common::{MODEL, World, eventually, turn_ended};
use orch_agent_contract::requests::{RequestTokens, UsageRequest};
use orch_api_contract::{
    context::{ModelTokens, TurnUsageReport},
    threads::TokenUsage,
    usage::UsageMeasurement,
};
use orch_link_contract::{
    events::{Event, ModelRequest, TurnOutcome, UsageUpdated},
    streams::Stream,
};
use orch_types::Id;
use serde_json::json;
use std::collections::BTreeMap;

fn usage(turn: Id, models: Vec<ModelTokens>) -> Event {
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: TokenUsage::default(),
        thread: TokenUsage::default(),
        context_window: None,
        context_used: None,
        report: Some(TurnUsageReport {
            counters_only: true,
            models,
            ..TurnUsageReport::default()
        }),
    })
}
fn model(name: &str, counts: &[(&str, i64)]) -> ModelTokens {
    ModelTokens {
        model: name.into(),
        tokens: TokenUsage::default(),
        tokens_reported: false,
        counts: counts
            .iter()
            .map(|(key, count)| ((*key).into(), *count))
            .collect(),
    }
}
async fn wait_request(
    world: &World,
    turn: Id,
    reference: &str,
) -> Vec<orch_db::usage::ledger::EventRow> {
    eventually("the exact native request is committed", || async {
        world
            .ledger_of(turn)
            .await
            .iter()
            .any(|row| row.request_ref.as_deref() == Some(reference))
    })
    .await;
    world.ledger_of(turn).await
}
async fn wait_aggregate(world: &World, turn: Id) -> orch_db::usage::ledger::EventRow {
    eventually(
        "the actual native capability aggregate is committed",
        || async {
            world.ledger_of(turn).await.iter().any(|row| {
                row.scope == "turn" && row.measurement_states.get("declared_counters").is_some()
            })
        },
    )
    .await;
    world
        .ledger_of(turn)
        .await
        .into_iter()
        .find(|row| {
            row.scope == "turn" && row.measurement_states.get("declared_counters").is_some()
        })
        .unwrap()
}
async fn replay_completion(world: &World, thread: Id, turn: Id) {
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
        .say(thread, turn_ended(turn, TurnOutcome::Completed))
        .await;
    eventually("the completion replay is consumed", || async {
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_aggregate_counts_keep_models_and_only_add_the_unrecorded_units_once() {
    let world = World::new().await;
    sqlx::query("UPDATE provider_models SET parameters=parameters || jsonb_build_object('rate',$2::jsonb) WHERE provider_id=$1 AND model_name=$3")
        .bind(world.provider).bind(json!({"input_per_mtok":"0","cached_input_per_mtok":"0","output_per_mtok":"0","web_search_per_call":"0.01"})).bind(MODEL)
        .execute(world.pool.pool()).await.unwrap();
    let thread = world.thread("Native quantities").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let mut actual = UsageRequest {
        id: "native-request".into(),
        model: Some(MODEL.into()),
        web_searches: Some(1),
        tokens: RequestTokens {
            input_tokens: Some(100),
            output_tokens: Some(10),
            ..RequestTokens::default()
        },
        ..UsageRequest::default()
    };
    actual.finish("done", None, None);
    world
        .say(
            thread,
            Event::ModelRequest(ModelRequest {
                turn_id: start.turn_id,
                request: actual,
            }),
        )
        .await;
    let report = usage(
        start.turn_id,
        vec![
            model(MODEL, &[("web_search", 3)]),
            model(
                "other/native-model",
                &[("web_search", 2), ("future_capability", 1)],
            ),
            model("unused/old-model", &[("web_search", 0)]),
        ],
    );
    world.say(thread, report.clone()).await;
    world.say(thread, report).await;
    world
        .say(thread, usage(start.turn_id, vec![model(MODEL, &[])]))
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let rows = world.wait_ledger(start.turn_id, 4).await;
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().all(|row| row.model != "unused/old-model"));
    let counters: Vec<_> = rows
        .iter()
        .filter(|row| {
            row.scope == "turn" && row.measurement_states.get("declared_counters").is_some()
        })
        .collect();
    assert_eq!(counters.len(), 3);
    let mut quantities = BTreeMap::new();
    for row in &counters {
        assert_eq!(
            (row.origin.as_str(), row.scope.as_str(), row.counted),
            ("harness", "turn", true)
        );
        assert_eq!(row.total_tokens, 0);
        assert_eq!(row.context_tokens, None);
        assert_eq!(row.request_ref, None);
        assert_eq!(row.turn_id, Some(start.turn_id));
        assert_eq!(row.user_id, Some(world.ada));
        assert_eq!(row.provider_id, world.provider);
        assert!(row.outcome.is_none());
        let timing = row.timing();
        assert_eq!(timing.outcome, UsageMeasurement::Unsupported);
        assert_eq!(timing.request_ms, UsageMeasurement::Unsupported);
        assert_eq!(timing.first_token_ms, UsageMeasurement::Unsupported);
        assert!(
            timing.asked.is_none() && timing.end.is_none() && timing.provider_request_id.is_none()
        );
        assert_eq!(timing.attempt, None);
        assert!(row.measurement_states.get("attempt").is_none());
        for (capability, count) in timing.declared_counters {
            quantities.insert((row.model.clone(), capability), count);
        }
    }
    assert_eq!(
        quantities[&(MODEL.into(), "web_search".into())],
        UsageMeasurement::Told { value: 2 }
    );
    assert_eq!(
        quantities[&("other/native-model".into(), "web_search".into())],
        UsageMeasurement::Told { value: 2 }
    );
    assert_eq!(
        quantities[&("other/native-model".into(), "future_capability".into())],
        UsageMeasurement::Told { value: 1 }
    );
    let main = counters.iter().find(|row| row.model == MODEL).unwrap();
    assert_eq!(main.credits.as_deref(), Some("0.020000"));
    assert_eq!(main.rate_source.as_deref(), Some("provider"));
    assert!(
        counters
            .iter()
            .filter(|row| row.model != MODEL)
            .all(|row| row.credits.is_none()),
        "an unknown quote is not a free call"
    );
    assert_eq!(rows.iter().filter(|row| row.scope == "request").count(), 1);
    let (requests, tokens): (i64, i64) = sqlx::query_as(
        "SELECT count(outcome),sum(total_tokens)::bigint FROM usage_events WHERE turn_id=$1",
    )
    .bind(start.turn_id)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!((requests, tokens), (1, 110));
    replay_completion(&world, thread, start.turn_id).await;
    let replayed = world.ledger_of(start.turn_id).await;
    assert_eq!(replayed.len(), 4);
    assert_eq!(
        replayed.iter().map(|row| row.id).collect::<Vec<_>>(),
        rows.iter().map(|row| row.id).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwarded_aggregate_counts_are_not_a_second_meter() {
    let world = World::on_a_key("sk-orch-test-native-counts").await;
    let thread = world.thread("Forwarded quantities").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world
        .say(
            thread,
            usage(start.turn_id, vec![model(MODEL, &[("web_search", 9)])]),
        )
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world.wait_followed_up(thread).await;
    assert!(
        world
            .ledger_of(start.turn_id)
            .await
            .iter()
            .all(|row| row.origin != "harness"
                || row.scope != "turn"
                || row.measurement_states.get("declared_counters").is_none())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_native_zero_is_known_and_a_missing_counter_is_not_fabricated() {
    let world = World::new().await;
    let thread = world.thread("Reported zero").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world
        .say(
            thread,
            usage(
                start.turn_id,
                vec![model(MODEL, &[("web_search", 0), ("invalid", -1)])],
            ),
        )
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let _ = wait_aggregate(&world, start.turn_id).await;
    let rows = world.ledger_of(start.turn_id).await;
    let counters: Vec<_> = rows
        .iter()
        .filter(|row| row.measurement_states.get("declared_counters").is_some())
        .collect();
    assert_eq!(counters.len(), 1);
    assert!(
        !counters[0].counted,
        "a known zero remains a trace without another consumption fold"
    );
    assert_eq!(
        counters[0].timing().declared_counters["web_search"],
        UsageMeasurement::Told { value: 0 }
    );
    assert!(
        !counters[0]
            .timing()
            .declared_counters
            .contains_key("page_fetch")
    );
    assert!(
        !counters[0]
            .timing()
            .declared_counters
            .contains_key("invalid")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_native_request_cannot_add_units_already_counted_by_its_turn_aggregate() {
    let world = World::new().await;
    sqlx::query("UPDATE provider_models SET parameters=parameters || jsonb_build_object('rate',$2::jsonb) WHERE provider_id=$1 AND model_name=$3")
        .bind(world.provider).bind(json!({"input_per_mtok":"0","cached_input_per_mtok":"0","output_per_mtok":"0","web_search_per_call":"0.01"})).bind(MODEL)
        .execute(world.pool.pool()).await.unwrap();
    let thread = world.thread("Late native quantity detail").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world
        .say(
            thread,
            usage(start.turn_id, vec![model(MODEL, &[("web_search", 3)])]),
        )
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let original = wait_aggregate(&world, start.turn_id).await;
    // A later detail cannot reprice the already counted native source at today's edited unit price.
    sqlx::query("UPDATE provider_models SET parameters=jsonb_set(parameters,'{rate,web_search_per_call}', '\"99\"'::jsonb) WHERE provider_id=$1 AND model_name=$2")
        .bind(world.provider).bind(MODEL).execute(world.pool.pool()).await.unwrap();
    let mut request = UsageRequest {
        id: "late-native-request".into(),
        model: Some(MODEL.into()),
        web_searches: Some(1),
        tokens: RequestTokens {
            input_tokens: Some(100),
            output_tokens: Some(10),
            ..RequestTokens::default()
        },
        ..UsageRequest::default()
    };
    request.finish("done", None, None);
    world
        .say(
            thread,
            Event::ModelRequest(ModelRequest {
                turn_id: start.turn_id,
                request: request.clone(),
            }),
        )
        .await;
    let _ = wait_request(&world, start.turn_id, "late-native-request").await;
    let credits: bool = sqlx::query_scalar("SELECT coalesce(sum(credits) FILTER (WHERE counted),0)=0.03::numeric FROM usage_events WHERE turn_id=$1")
        .bind(start.turn_id).fetch_one(world.pool.pool()).await.unwrap();
    assert!(
        credits,
        "three native searches total, one later detailed: no fourth search"
    );
    let rows = world.ledger_of(start.turn_id).await;
    let kept = rows.iter().find(|row| row.id == original.id).unwrap();
    assert_eq!(kept.rate, original.rate);
    assert_eq!(kept.credits, original.credits);
    assert_eq!(kept.measurement_states, original.measurement_states);
    let detail = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("late-native-request"))
        .unwrap();
    assert_eq!(
        detail.timing().web_searches,
        UsageMeasurement::Told { value: 1 }
    );
    assert_eq!(detail.total_tokens, 110);
    assert_eq!(
        detail.measurement_states["_native_counter_covered"]["web_search"],
        1
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let billed = orch_db::usage::ledger::counted_capabilities(
        &mut conn,
        start.turn_id,
        world.provider,
        &BTreeMap::from([("web_search".into(), Some("web_searches".into()))]),
    )
    .await
    .unwrap();
    assert_eq!(billed[&(MODEL.into(), "web_search".into())], Some(3));
    drop(conn);
    world
        .say(
            thread,
            Event::ModelRequest(ModelRequest {
                turn_id: start.turn_id,
                request,
            }),
        )
        .await;
    replay_completion(&world, thread, start.turn_id).await;
    let replayed: bool = sqlx::query_scalar("SELECT coalesce(sum(credits) FILTER (WHERE counted),0)=0.03::numeric FROM usage_events WHERE turn_id=$1")
        .bind(start.turn_id).fetch_one(world.pool.pool()).await.unwrap();
    assert!(replayed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_request_coverage_is_exhausted_once_and_does_not_cover_another_model() {
    let world = World::new().await;
    sqlx::query("UPDATE provider_models SET parameters=parameters || jsonb_build_object('rate',$2::jsonb) WHERE provider_id=$1 AND model_name=$3")
        .bind(world.provider).bind(json!({"input_per_mtok":"0","cached_input_per_mtok":"0","output_per_mtok":"0","web_search_per_call":"0.01"})).bind(MODEL)
        .execute(world.pool.pool()).await.unwrap();
    let thread = world.thread("Bounded native coverage").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world
        .say(
            thread,
            usage(start.turn_id, vec![model(MODEL, &[("web_search", 3)])]),
        )
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let _ = wait_aggregate(&world, start.turn_id).await;
    for index in 0..4 {
        let mut request = UsageRequest {
            id: format!("late-{index}"),
            model: Some(MODEL.into()),
            web_searches: Some(1),
            tokens: RequestTokens {
                input_tokens: Some(100),
                output_tokens: Some(10),
                ..RequestTokens::default()
            },
            ..UsageRequest::default()
        };
        request.finish("done", None, None);
        world
            .say(
                thread,
                Event::ModelRequest(ModelRequest {
                    turn_id: start.turn_id,
                    request: request.clone(),
                }),
            )
            .await;
        let _ = wait_request(&world, start.turn_id, &format!("late-{index}")).await;
        world
            .say(
                thread,
                Event::ModelRequest(ModelRequest {
                    turn_id: start.turn_id,
                    request,
                }),
            )
            .await;
    }
    let mut request = UsageRequest {
        id: "other-model".into(),
        model: Some("other/native-model".into()),
        web_searches: Some(1),
        tokens: RequestTokens {
            input_tokens: Some(1),
            output_tokens: Some(1),
            ..RequestTokens::default()
        },
        ..UsageRequest::default()
    };
    request.finish("done", None, None);
    world
        .say(
            thread,
            Event::ModelRequest(ModelRequest {
                turn_id: start.turn_id,
                request,
            }),
        )
        .await;
    let rows = wait_request(&world, start.turn_id, "other-model").await;
    let other = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("other-model"))
        .unwrap();
    assert!(
        other
            .measurement_states
            .get("_native_counter_covered")
            .is_none()
    );
    assert!(
        other.credits.is_none(),
        "another model's unquoted unit is not priced from this model"
    );
    let covered: i64 = rows
        .iter()
        .filter_map(|row| row.measurement_states["_native_counter_covered"]["web_search"].as_i64())
        .sum();
    assert_eq!(
        covered, 3,
        "duplicate deliveries cannot allocate the same aggregate units again"
    );
    let fourth = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("late-3"))
        .unwrap();
    assert_eq!(fourth.credits.as_deref(), Some("0.010000"));
    let credits:bool=sqlx::query_scalar("SELECT coalesce(sum(credits) FILTER (WHERE counted),0)=0.04::numeric FROM usage_events WHERE turn_id=$1 AND model=$2")
        .bind(start.turn_id).bind(MODEL).fetch_one(world.pool.pool()).await.unwrap();
    assert!(credits);
    let mut conn = world.pool.acquire().await.unwrap();
    let counts = orch_db::usage::ledger::counted_capabilities(
        &mut conn,
        start.turn_id,
        world.provider,
        &BTreeMap::from([("web_search".into(), Some("web_searches".into()))]),
    )
    .await
    .unwrap();
    assert_eq!(counts[&(MODEL.into(), "web_search".into())], Some(4));
    assert_eq!(
        counts[&("other/native-model".into(), "web_search".into())],
        Some(1)
    );
    let foreign = orch_db::usage::ledger::native::covered(
        &mut conn,
        start.turn_id,
        orch_types::new_id(),
        MODEL,
        orch_types::new_id(),
        &BTreeMap::from([("web_search".into(), 1)]),
    )
    .await
    .unwrap();
    assert!(
        foreign.quantities.is_empty() && foreign.priced.is_empty(),
        "another provider cannot borrow this native aggregate"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unpriced_aggregate_covers_units_without_silently_waiving_a_later_known_fee() {
    let world = World::new().await;
    sqlx::query("UPDATE provider_models SET parameters=jsonb_set(parameters,'{rate}',$2::jsonb) WHERE provider_id=$1 AND model_name=$3")
        .bind(world.provider).bind(json!({"input_per_mtok":"0","cached_input_per_mtok":"0","output_per_mtok":"0"})).bind(MODEL)
        .execute(world.pool.pool()).await.unwrap();
    let thread = world.thread("Unquoted native unit").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world
        .say(
            thread,
            usage(start.turn_id, vec![model(MODEL, &[("web_search", 1)])]),
        )
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let initial = wait_aggregate(&world, start.turn_id).await;
    assert!(initial.credits.is_none());
    sqlx::query("UPDATE provider_models SET parameters=jsonb_set(parameters,'{rate,web_search_per_call}', '\"0.01\"'::jsonb) WHERE provider_id=$1 AND model_name=$2")
        .bind(world.provider).bind(MODEL).execute(world.pool.pool()).await.unwrap();
    let mut request = UsageRequest {
        id: "priced-detail".into(),
        model: Some(MODEL.into()),
        web_searches: Some(1),
        tokens: RequestTokens {
            input_tokens: Some(100),
            output_tokens: Some(10),
            ..RequestTokens::default()
        },
        ..UsageRequest::default()
    };
    request.finish("done", None, None);
    world
        .say(
            thread,
            Event::ModelRequest(ModelRequest {
                turn_id: start.turn_id,
                request,
            }),
        )
        .await;
    let rows = wait_request(&world, start.turn_id, "priced-detail").await;
    let detail = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("priced-detail"))
        .unwrap();
    assert_eq!(detail.credits.as_deref(), Some("0.010000"));
    assert_eq!(
        detail.measurement_states["_native_counter_covered"]["web_search"],
        1
    );
    assert!(
        rows.iter()
            .find(|row| row.id == initial.id)
            .unwrap()
            .credits
            .is_none(),
        "the original unknown quote is not rewritten"
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let counts = orch_db::usage::ledger::counted_capabilities(
        &mut conn,
        start.turn_id,
        world.provider,
        &BTreeMap::from([("web_search".into(), Some("web_searches".into()))]),
    )
    .await
    .unwrap();
    assert_eq!(counts[&(MODEL.into(), "web_search".into())], Some(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_rows_keep_first_counted_rounding_without_repricing_or_reconciliation_rows() {
    let world = World::new().await;
    sqlx::query("UPDATE provider_models SET parameters=jsonb_set(parameters,'{rate}',$2::jsonb) WHERE provider_id=$1 AND model_name=$3")
        .bind(world.provider).bind(json!({"input_per_mtok":"0","cached_input_per_mtok":"0","output_per_mtok":"0","web_search_per_call":"0.0000005"})).bind(MODEL)
        .execute(world.pool.pool()).await.unwrap();
    let mut turns = Vec::new();
    for aggregate_first in [true, false] {
        let thread = world
            .thread(if aggregate_first {
                "Aggregate rounding"
            } else {
                "Request row rounding"
            })
            .await;
        let (start, _) = world.running_turn(thread, "Go").await;
        let mut original = Vec::new();
        if aggregate_first {
            world
                .say(
                    thread,
                    usage(start.turn_id, vec![model(MODEL, &[("web_search", 3)])]),
                )
                .await;
            world
                .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
                .await;
            world.wait_turn(start.turn_id, "completed").await;
            let aggregate = wait_aggregate(&world, start.turn_id).await;
            assert_eq!(aggregate.credits.as_deref(), Some("0.000002"));
            original = world.ledger_of(start.turn_id).await;
        }
        let mut delivered = Vec::new();
        for index in 0..3 {
            let mut request = UsageRequest {
                id: format!("rounding-{index}"),
                model: Some(MODEL.into()),
                web_searches: Some(1),
                tokens: RequestTokens {
                    input_tokens: Some(100),
                    output_tokens: Some(10),
                    ..RequestTokens::default()
                },
                ..UsageRequest::default()
            };
            request.finish("done", None, None);
            world
                .say(
                    thread,
                    Event::ModelRequest(ModelRequest {
                        turn_id: start.turn_id,
                        request: request.clone(),
                    }),
                )
                .await;
            let rows = wait_request(&world, start.turn_id, &request.id).await;
            let row = rows
                .iter()
                .find(|row| row.request_ref.as_deref() == Some(request.id.as_str()))
                .unwrap();
            assert_eq!(
                row.credits.as_deref(),
                Some(if aggregate_first {
                    "0.000000"
                } else {
                    "0.000001"
                })
            );
            delivered.push(request);
        }
        if !aggregate_first {
            world
                .say(
                    thread,
                    usage(start.turn_id, vec![model(MODEL, &[("web_search", 3)])]),
                )
                .await;
            world
                .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
                .await;
            world.wait_turn(start.turn_id, "completed").await;
        }
        let rows = world.ledger_of(start.turn_id).await;
        assert_eq!(
            rows.len(),
            original.len() + 3,
            "only actual details may add rows; no rounding adjustment row"
        );
        for row in &original {
            assert_eq!(rows.iter().find(|kept| kept.id == row.id), Some(row));
        }
        let expected = if aggregate_first {
            "0.000002"
        } else {
            "0.000003"
        };
        let exact:bool=sqlx::query_scalar("SELECT coalesce(sum(credits) FILTER (WHERE counted),0)=$2::text::numeric FROM usage_events WHERE turn_id=$1")
            .bind(start.turn_id).bind(expected).fetch_one(world.pool.pool()).await.unwrap();
        assert!(exact, "the total is the sum of the rows as first counted");
        for request in delivered {
            world
                .say(
                    thread,
                    Event::ModelRequest(ModelRequest {
                        turn_id: start.turn_id,
                        request,
                    }),
                )
                .await;
        }
        replay_completion(&world, thread, start.turn_id).await;
        assert_eq!(
            world.ledger_of(start.turn_id).await,
            rows,
            "replay neither reprices a source nor reconciles its rounding"
        );
        turns.push(start.turn_id);
    }
    let bounded:bool=sqlx::query_scalar(
        "SELECT abs((SELECT sum(credits) FROM usage_events WHERE turn_id=$1 AND counted)
                   -(SELECT sum(credits) FROM usage_events WHERE turn_id=$2 AND counted))=0.000001::numeric
          AND abs((SELECT sum(credits) FROM usage_events WHERE turn_id=$1 AND counted)
                   -(SELECT sum(credits) FROM usage_events WHERE turn_id=$2 AND counted))
              <=0.0000005::numeric*(SELECT count(*) FROM usage_events WHERE turn_id=$2 AND counted)")
        .bind(turns[0]).bind(turns[1]).fetch_one(world.pool.pool()).await.unwrap();
    assert!(
        bounded,
        "per-row rounding stays within half a millionth of a credit per counted row"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn counter_only_observations_preserve_unknown_zero_and_known_aggregate_tokens() {
    let world = World::new().await;
    for observed in [None, Some(0_i64), Some(100_i64)] {
        let name = match observed {
            None => "Unknown aggregate web quantities",
            Some(0) => "Zero aggregate web quantities",
            Some(_) => "Known aggregate web quantities",
        };
        let thread = world.thread(name).await;
        let (start, _) = world.running_turn(thread, "Go").await;
        if let Some(input) = observed {
            let tokens = TokenUsage {
                input_tokens: input,
                output_tokens: input / 10,
                total_tokens: input + input / 10,
                ..TokenUsage::default()
            };
            // Known aggregate tokens need not have a known per-model allocation. An absent new marker
            // must keep this ordinary observation, including its genuine zero, working as before.
            world
                .say(
                    thread,
                    Event::UsageUpdated(UsageUpdated {
                        turn_id: start.turn_id,
                        turn: tokens,
                        thread: tokens,
                        context_window: None,
                        context_used: None,
                        report: Some(TurnUsageReport {
                            models: vec![model(MODEL, &[("web_search", 0)])],
                            ..TurnUsageReport::default()
                        }),
                    }),
                )
                .await;
            eventually("the ordinary aggregate report commits", || async {
                sqlx::query_scalar::<_, bool>("SELECT usage IS NOT NULL FROM turns WHERE id=$1")
                    .bind(start.turn_id)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap()
            })
            .await;
        }
        let before:(Option<orch_types::Json>,Option<orch_types::Json>)=sqlx::query_as(
            "SELECT t.usage,r.usage FROM turns t JOIN threads r ON r.node_id=t.thread_id WHERE t.id=$1")
            .bind(start.turn_id).fetch_one(world.pool.pool()).await.unwrap();
        if observed.is_none() {
            assert!(before.0.is_none(), "the fixture has no token observation");
        }
        world
            .say(
                thread,
                usage(start.turn_id, vec![model(MODEL, &[("web_search", 2)])]),
            )
            .await;
        eventually("the independent native quantities commit",||async {
            sqlx::query_scalar::<_,bool>("SELECT COALESCE(usage_detail->'models'->0->'counts'->>'web_search','')='2' FROM turns WHERE id=$1")
                .bind(start.turn_id).fetch_one(world.pool.pool()).await.unwrap()
        }).await;
        let after:(Option<orch_types::Json>,Option<orch_types::Json>)=sqlx::query_as(
            "SELECT t.usage,r.usage FROM turns t JOIN threads r ON r.node_id=t.thread_id WHERE t.id=$1")
            .bind(start.turn_id).fetch_one(world.pool.pool()).await.unwrap();
        assert_eq!(
            after, before,
            "a counter-only container neither supplies zero nor erases known token totals"
        );
        world
            .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
            .await;
        world.wait_turn(start.turn_id, "completed").await;
    }
}
