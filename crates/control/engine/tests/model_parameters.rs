//! What a thread runs with of a context, a speed and an access program: the thread's own choice within what its model's entry leaves open, what the entry says a thread starts with where it chose nothing, the model's own window given beside them, a choice that is no longer open left out, a change of model resolving all three again, and each sent to the harness and kept on the turn, the speed in the ledger.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{MODEL, World, queued, turn_ended};
use orch_authz::Caller;
use orch_link_contract::commands::TurnStart;
use orch_link_contract::events::{Event, TurnOutcome, UsageUpdated};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde_json::json;

/// The parameters of an adapter that publishes a speed, an access program and a context, each reading the
/// entry of the thread's model: the three that the fake adapter declares.
fn declared() -> Json {
    let scopes = json!(["org", "project", "topic", "thread"]);
    json!([
        {
            "key": "service_tier", "title": "Speed", "description": "A faster way of running a turn.",
            "group": "model", "type": "enum", "options": { "from_model": "service_tiers" },
            "default": { "from_model": "default_service_tier" }, "scopes": scopes, "apply": "next_turn",
            "eligible_when": { "model_has": "service_tiers" }
        },
        {
            "key": "access_program", "title": "Access program", "description": "A program.",
            "group": "safety", "type": "enum", "options": { "from_model": "access_programs" },
            "default": { "from_model": "default_access_program" }, "scopes": scopes, "apply": "next_turn",
            "eligible_when": { "model_has": "access_programs" }
        },
        {
            "key": "context", "title": "Context", "description": "How much of a conversation is kept.",
            "group": "model", "type": "integer",
            "range": { "min": 1000, "max": { "from_model": "context_max" } },
            "default": { "from_model": "default_context" }, "scopes": scopes, "apply": "next_turn",
            "eligible_when": { "model_has": "context_max" }
        }
    ])
}

/// Says that the agent publishes the three parameters: its harness declares them at every start, as an adapter
/// does, since whatever starts the harness (a turn, a read of the provider's facts) keeps what it declares.
async fn declare(world: &World) {
    let mut descriptor = common::descriptor();
    descriptor["parameters"] = declared();
    world.declares(descriptor).await;
}

/// A world whose agent publishes the three parameters and whose model has the entry `model`.
async fn world_with(model: Json) -> World {
    let world = World::observed().await;
    declare(&world).await;
    entry(&world, MODEL, model).await;
    world
}

/// Writes what the entry of a model says it can be run with, and adds the entry where there is none.
async fn entry(world: &World, model: &str, parameters: Json) {
    sqlx::query(
        "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
         VALUES ($1, $2, $3, '', $4, true, 'manual', $5)
         ON CONFLICT (provider_id, model_name) DO UPDATE SET parameters = EXCLUDED.parameters",
    )
    .bind(orch_types::new_id())
    .bind(world.provider)
    .bind(model)
    .bind(parameters)
    .bind(world.ada)
    .execute(world.pool.pool())
    .await
    .unwrap();
}

/// Sends an input and returns the turn the machine was asked to start for it.
async fn turn(world: &World, thread: Id, nth: usize) -> TurnStart {
    declare(world).await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    world.turn_start(thread, nth).await
}

/// Ends the turn that was started.
async fn finish(world: &World, thread: Id, start: &TurnStart, nth: usize) {
    world
        .answer(thread, start, &format!("m{nth}"), "Done.")
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    world.wait_state(thread, "idle").await;
}

fn sent(start: &TurnStart) -> Json {
    Json::Object(start.params.clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_that_chose_nothing_starts_with_all_that_is_open_and_the_model_s_own_window_is_given()
 {
    let world = world_with(json!({
        "efforts": ["low", "high"], "default_effort": "low",
        "context_window": 272_000, "context_max": 1_000_000
    }))
    .await;
    let thread = world.thread("Context").await;
    let start = turn(&world, thread, 0).await;
    assert_eq!(
        sent(&start),
        json!({ "context": 1_000_000, "context_window": 272_000 }),
        "all that is open, and the window the model has by itself"
    );
    // The session is opened with the same, and the turn keeps what it was started with.
    let open = world
        .seen
        .of_kind(orch_link_contract::commands::CommandKind::SessionOpen)
        .into_iter()
        .find_map(|command| match command {
            orch_link_contract::commands::Command::SessionOpen(open)
                if open.thread_id == thread =>
            {
                Some(open)
            }
            _ => None,
        })
        .expect("a session was opened");
    assert_eq!(Json::Object(open.params), sent(&start));
    let recorded: Json = world
        .scalar("SELECT params FROM turns WHERE id = $1", start.turn_id)
        .await;
    assert_eq!(recorded, sent(&start), "recorded on the turn");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_may_name_another_start_than_all_that_is_open() {
    let world = world_with(json!({
        "context_window": 272_000, "context_max": 1_000_000, "default_context": 272_000
    }))
    .await;
    let thread = world.thread("Start").await;
    let start = turn(&world, thread, 0).await;
    assert_eq!(start.params["context"], 272_000, "the entry's own start");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_choice_between_the_least_and_the_largest_is_sent_as_it_is_and_one_above_a_lowered_largest_is_not()
 {
    let world = world_with(json!({ "context_window": 272_000, "context_max": 1_000_000 })).await;
    let thread = world.thread("Choice").await;
    world
        .set(thread, keys::AGENT_PARAMS, json!({ "context": 600_000 }))
        .await;
    let first = turn(&world, thread, 0).await;
    assert_eq!(first.params["context"], 600_000);
    finish(&world, thread, &first, 0).await;

    // The managers lower what is open below the thread's choice: it is not sent, the turn runs on the
    // model's start, and nothing is blocked.
    entry(
        &world,
        MODEL,
        json!({ "context_window": 272_000, "context_max": 500_000 }),
    )
    .await;
    let second = turn(&world, thread, 1).await;
    assert_eq!(second.params["context"], 500_000);
    finish(&world, thread, &second, 1).await;

    // A choice below what the adapter's harness takes is not open either.
    world
        .set(thread, keys::AGENT_PARAMS, json!({ "context": 999 }))
        .await;
    let third = turn(&world, thread, 2).await;
    assert_eq!(third.params["context"], 500_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_chosen_or_shown_on_a_model_without_a_larger_context_and_its_window_is_still_given()
 {
    let world = world_with(json!({ "context_window": 272_000 })).await;
    let thread = world.thread("Plain").await;
    world
        .set(thread, keys::AGENT_PARAMS, json!({ "context": 100_000 }))
        .await;
    let start = turn(&world, thread, 0).await;
    assert_eq!(
        sent(&start),
        json!({ "context_window": 272_000 }),
        "no context is sent, whatever was chosen, and the window is given all the same"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_speed_and_a_program_start_where_the_entry_says_and_a_threads_own_choice_comes_before_them()
 {
    let world = world_with(json!({
        "service_tiers": [{ "id": "fast", "label": "Fast" }, { "id": "flex", "label": "Flex" }],
        "default_service_tier": "fast",
        "access_programs": ["blue", "red"], "default_access_program": "blue"
    }))
    .await;
    let thread = world.thread("Speed").await;
    let start = turn(&world, thread, 0).await;
    assert_eq!(
        sent(&start),
        json!({ "service_tier": "fast", "access_program": "blue" })
    );
    finish(&world, thread, &start, 0).await;

    world
        .set(
            thread,
            keys::AGENT_PARAMS,
            json!({ "service_tier": "flex", "access_program": "red" }),
        )
        .await;
    let chosen = turn(&world, thread, 1).await;
    assert_eq!(
        sent(&chosen),
        json!({ "service_tier": "flex", "access_program": "red" })
    );
    finish(&world, thread, &chosen, 1).await;

    // A model that enables a faster speed and names no start does not make any thread fast.
    entry(
        &world,
        MODEL,
        json!({ "service_tiers": [{ "id": "fast", "label": "Fast" }], "access_programs": ["blue"] }),
    )
    .await;
    world.unset(thread, keys::AGENT_PARAMS).await;
    let plain = turn(&world, thread, 2).await;
    assert_eq!(sent(&plain), json!({}), "the ordinary speed and no program");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_the_entry_offers_and_no_manager_opened_is_not_shown_and_a_start_that_is_no_longer_enabled_is_not_sent()
 {
    let world = world_with(json!({
        "offered": { "service_tiers": [{ "id": "fast", "label": "Fast" }], "access_programs": ["blue"],
                     "context_max": 1_000_000 },
        "context_window": 272_000
    }))
    .await;
    let thread = world.thread("Offered").await;
    world
        .set(
            thread,
            keys::AGENT_PARAMS,
            json!({ "service_tier": "fast", "access_program": "blue", "context": 900_000 }),
        )
        .await;
    let start = turn(&world, thread, 0).await;
    assert_eq!(
        sent(&start),
        json!({ "context_window": 272_000 }),
        "on offer is not open: nothing is sent whatever the thread chose"
    );
    finish(&world, thread, &start, 0).await;

    // A start stored on the entry that is not among what is enabled is not sent; a choice that is not
    // among what is enabled is flagged and the thread runs on the entry's start.
    entry(
        &world,
        MODEL,
        json!({
            "service_tiers": [{ "id": "flex", "label": "Flex" }], "default_service_tier": "fast",
            "access_programs": ["red"], "default_access_program": "blue"
        }),
    )
    .await;
    world
        .set(
            thread,
            keys::AGENT_PARAMS,
            json!({ "service_tier": "fast" }),
        )
        .await;
    let dangling = turn(&world, thread, 1).await;
    assert_eq!(
        sent(&dangling),
        json!({}),
        "neither the thread's stale choice nor the entry's dangling start"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_change_of_model_resolves_all_three_again_for_the_new_one() {
    let world = world_with(json!({
        "service_tiers": [{ "id": "fast", "label": "Fast" }], "default_service_tier": "fast",
        "context_window": 200_000, "context_max": 400_000
    }))
    .await;
    entry(
        &world,
        "fake-2",
        json!({
            "access_programs": ["blue"], "default_access_program": "blue",
            "context_window": 100_000, "context_max": 300_000, "default_context": 100_000
        }),
    )
    .await;
    let thread = world.thread("Switch").await;
    let first = turn(&world, thread, 0).await;
    assert_eq!(
        sent(&first),
        json!({ "service_tier": "fast", "context": 400_000, "context_window": 200_000 })
    );
    finish(&world, thread, &first, 0).await;

    world.set(thread, keys::AGENT_MODEL, json!("fake-2")).await;
    let second = turn(&world, thread, 1).await;
    assert_eq!(second.model, "fake-2");
    assert_eq!(
        sent(&second),
        json!({ "access_program": "blue", "context": 100_000, "context_window": 100_000 }),
        "the speed the first model had is gone with it, and the new one's own starts apply"
    );
}

fn usage(turn: Id, tokens: i64) -> Event {
    let counted = json!({
        "input_tokens": tokens, "cached_input_tokens": 0, "output_tokens": 0,
        "reasoning_tokens": 0, "total_tokens": tokens
    });
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: serde_json::from_value(counted.clone()).unwrap(),
        thread: serde_json::from_value(counted).unwrap(),
        context_window: None,
        context_used: None,
        report: None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_speed_of_a_turn_is_recorded_on_it_and_in_the_ledger_and_the_rollups_keep_it() {
    let world = world_with(json!({
        "service_tiers": [{ "id": "fast", "label": "Fast" }], "default_service_tier": "fast"
    }))
    .await;
    let thread = world.thread("Ledger").await;
    let fast = turn(&world, thread, 0).await;
    declare(&world).await;
    world.say(thread, usage(fast.turn_id, 120)).await;
    finish(&world, thread, &fast, 0).await;
    let rows = world.wait_ledger(fast.turn_id, 1).await;
    assert_eq!(
        rows[0].tier.as_deref(),
        Some("fast"),
        "the speed the turn ran at"
    );

    // The same thread, at the ordinary speed: the managers' start is a start and not a rule.
    world
        .exec("UPDATE provider_models SET parameters = '{\"service_tiers\":[{\"id\":\"fast\",\"label\":\"Fast\"}]}'")
        .await;
    let ordinary = turn(&world, thread, 1).await;
    world.say(thread, usage(ordinary.turn_id, 30)).await;
    finish(&world, thread, &ordinary, 1).await;
    let rows = world.wait_ledger(ordinary.turn_id, 1).await;
    assert_eq!(rows[0].tier, None);

    let by_tier: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT tier, sum(total_tokens)::bigint FROM usage_daily GROUP BY tier ORDER BY tier",
    )
    .fetch_all(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        by_tier,
        [(Some("fast".to_owned()), 120), (None, 30)],
        "both rollups keep the tier apart"
    );
    let monthly: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_monthly WHERE tier = 'fast' AND total_tokens = 120",
    )
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(monthly, 1);
}
