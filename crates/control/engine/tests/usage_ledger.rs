//! What a model request of a lease used becomes one row of the usage ledger, with what it ran on, the person and place it is put down to, the rate it was counted at and the credits it comes to; a refused request fails the turn with its reason; and the provider's status and facts are what the machine that keeps it says.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{MODEL, World, meanwhile, queued};
use orch_authz::Caller;
use orch_link::streams::DeliveredEvent;
use orch_link_contract::commands::{CommandKind, CommandResult, LeaseCloseResult};
use orch_link_contract::events::{Event, LeaseDenied, LeaseUsage, ProviderStatus};
use orch_link_contract::streams::Stream;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::time::Duration;

const KEY: &str = "sk-orch-test-7a21c9e04d3b";

/// What the forwarder says one request used.
fn request(
    lease: Id,
    model: &str,
    (input, cached, output): (i64, i64, i64),
    named: Option<&str>,
) -> Event {
    Event::LeaseUsage(LeaseUsage {
        lease_id: lease,
        model: model.to_owned(),
        usage: serde_json::from_value(json!({
            "input_tokens": input, "cached_input_tokens": cached, "output_tokens": output,
            "reasoning_tokens": 0, "total_tokens": input + output
        }))
        .unwrap(),
        request_ref: named.map(str::to_owned),
    })
}

fn refused(lease: Id, model: &str, reason: &str) -> Event {
    Event::LeaseDenied(LeaseDenied {
        lease_id: lease,
        model: model.to_owned(),
        reason: reason.to_owned(),
    })
}

fn a_rate(input: &str, cached: &str, output: &str) -> Json {
    json!({ "input_per_mtok": input, "cached_input_per_mtok": cached, "output_per_mtok": output })
}

async fn give_model_a_rate(world: &World, rate: &Json) {
    sqlx::query("UPDATE provider_models SET parameters = parameters || jsonb_build_object('rate', $1::jsonb)")
        .bind(rate)
        .execute(world.pool.pool())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_reported_twice_is_one_row_with_the_providers_rate_the_credits_and_what_it_ran_on()
 {
    let world = World::on_a_key(KEY).await;
    give_model_a_rate(&world, &a_rate("2.50", "0.25", "15")).await;
    let thread = world.thread("Usage").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;

    let first = request(lease, MODEL, (1_000, 200, 100), Some("resp_1"));
    world.say_of_provider(first.clone()).await;
    world.say_of_provider(first).await;
    world
        .say_of_provider(request(lease, MODEL, (500, 0, 50), Some("resp_2")))
        .await;
    let rows = world.wait_ledger(start.turn_id, 2).await;
    meanwhile(Duration::from_millis(300)).await;
    let rows_later = world.ledger_of(start.turn_id).await;
    assert_eq!(
        rows_later.len(),
        2,
        "the request reported twice is one row: {rows_later:?}"
    );
    assert_eq!(rows.len(), 2);

    let one = rows_later
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("resp_1"))
        .unwrap();
    assert!(one.counted);
    assert_eq!(
        (one.origin.as_str(), one.scope.as_str()),
        ("forwarder", "request")
    );
    assert_eq!(one.model, MODEL);
    assert_eq!(
        (
            one.input_tokens,
            one.cached_input_tokens,
            one.cache_write_tokens,
            one.output_tokens,
            one.reasoning_tokens,
            one.total_tokens
        ),
        (1_000, 200, 0, 100, 0, 1_100)
    );
    assert_eq!(one.rate_source.as_deref(), Some("provider"));
    assert_eq!(one.rate, Some(a_rate("2.50", "0.25", "15")));
    // 800 fresh at 2.5, 200 cached at 0.25 and 100 out at 15, over a million.
    assert_eq!(one.credits.as_deref(), Some("0.003550"));
    assert_eq!(one.context_tokens, Some(1_000));
    let other = rows_later
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("resp_2"))
        .unwrap();
    assert_eq!(other.credits.as_deref(), Some("0.002000"));

    // What it ran on and what it is put down to, as they were.
    assert_eq!(one.org_id, world.root);
    assert_eq!(one.project_id, world.project);
    assert_eq!(one.topic_id, world.topic);
    assert_eq!(one.thread_id, Some(thread));
    assert_eq!(one.turn_id, Some(start.turn_id));
    assert_eq!(one.segment_id, Some(world.segment_of(thread).await));
    assert_eq!(one.provider_id, world.provider);
    assert_eq!(one.user_id, Some(world.ada));
    assert_eq!(one.agent_id, Some(world.agent));
    assert_eq!(one.harness_kind.as_deref(), Some("fake"));
    assert_eq!(one.environment_id, Some(world.environment));
    assert_eq!(one.environment_kind.as_deref(), Some("managed"));
    assert_eq!(one.arrangement.as_deref(), Some("together"));

    // The turn is known to have used what the forwarder counted, once.
    let usage: Json = world
        .scalar("SELECT usage FROM turns WHERE id = $1", start.turn_id)
        .await;
    assert_eq!(usage["input_tokens"], 1_500);
    assert_eq!(usage["cached_input_tokens"], 200);
    assert_eq!(usage["output_tokens"], 150);
    assert_eq!(usage["total_tokens"], 1_650);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivery_that_is_applied_again_makes_no_second_row_even_when_the_upstream_named_nothing()
{
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Usage").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;
    let event = request(lease, MODEL, (10, 0, 5), None);
    let delivered = DeliveredEvent {
        seq: 90,
        at: Time::new(world.clock.now()),
        kind: event.kind().as_str().to_owned(),
        body: event.body().unwrap(),
    };
    for _ in 0..2 {
        let mut tx = world.pool.begin().await.unwrap();
        world
            .engine()
            .apply_stream_events(
                &mut tx,
                world.machine_id,
                &Stream::Provider {
                    provider_id: world.provider,
                },
                std::slice::from_ref(&delivered),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let rows = world.ledger_of(start.turn_id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].total_tokens, 15);
    // Another delivery of the same size is another request.
    let again = DeliveredEvent {
        seq: 91,
        ..delivered
    };
    let mut tx = world.pool.begin().await.unwrap();
    world
        .engine()
        .apply_stream_events(
            &mut tx,
            world.machine_id,
            &Stream::Provider {
                provider_id: world.provider,
            },
            &[again],
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(world.ledger_of(start.turn_id).await.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rate_is_the_providers_own_else_the_rate_card_else_the_catalogues_estimate_and_else_none()
 {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Rates").await;
    // The provider's own entry for one model; the rate card for a family; the catalogue for another.
    sqlx::query(
        "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
         VALUES ($1, $2, 'own-model', '', $3, true, 'manual', $4)",
    )
    .bind(orch_types::new_id())
    .bind(world.provider)
    .bind(json!({ "rate": a_rate("7", "7", "7") }))
    .bind(world.ada)
    .execute(world.pool.pool())
    .await
    .unwrap();
    world
        .set(
            world.root,
            keys::USAGE_RATES,
            json!({ "card-*": a_rate("3", "3", "3") }),
        )
        .await;
    world
        .set(world.root, keys::USAGE_CATALOGUE_SCALE, json!("0.5"))
        .await;
    sqlx::query(
        "INSERT INTO rate_catalogue (id, doc, as_of, origin) VALUES (true, $1, '2026-10-10', 'shipped')",
    )
    .bind(json!({
        "format": 1, "as_of": "2026-10-10", "unit": "USD",
        "families": [
            { "match": "cat-*", "vendor": "Test", "source": "https://example.com", "as_of": "2026-10-10",
              "rate": a_rate("4", "0.4", "8") },
            { "match": "card-*", "vendor": "Test", "source": "https://example.com", "as_of": "2026-10-10",
              "rate": a_rate("99", "99", "99") }
        ]
    }))
    .execute(world.pool.pool())
    .await
    .unwrap();

    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;
    for (model, named) in [
        ("own-model", "r1"),
        ("openai/card-two", "r2"),
        ("cat-large", "r3"),
        ("nobody-knows", "r4"),
    ] {
        world
            .say_of_provider(request(lease, model, (1_000_000, 0, 0), Some(named)))
            .await;
    }
    let rows = world.wait_ledger(start.turn_id, 4).await;
    let by = |named: &str| {
        rows.iter()
            .find(|row| row.request_ref.as_deref() == Some(named))
            .unwrap()
    };
    assert_eq!(
        (by("r1").rate_source.as_deref(), by("r1").credits.as_deref()),
        (Some("provider"), Some("7.000000"))
    );
    assert_eq!(
        (by("r2").rate_source.as_deref(), by("r2").credits.as_deref()),
        (Some("rate_card"), Some("3.000000")),
        "the card, though the catalogue has a family for the model: the card comes first, and a gateway's name is matched by its last part"
    );
    assert_eq!(
        (by("r3").rate_source.as_deref(), by("r3").credits.as_deref()),
        (Some("catalogue"), Some("2.000000")),
        "4 a million, times the scale of a half"
    );
    assert_eq!(
        by("r3").rate,
        Some(a_rate("2", "0.2", "4")),
        "the rate in force, scaled"
    );

    // A model nothing names is counted in tokens and has no credits: nobody said it cost nothing.
    let none = by("r4");
    assert_eq!(none.total_tokens, 1_000_000);
    assert!(none.rate.is_none() && none.rate_source.is_none() && none.credits.is_none());

    // The catalogue is used only where the place switches it on.
    world
        .set(world.root, keys::USAGE_CATALOGUE, json!(false))
        .await;
    world
        .say_of_provider(request(lease, "cat-small", (1_000_000, 0, 0), Some("r5")))
        .await;
    let rows = world.wait_ledger(start.turn_id, 5).await;
    let off = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("r5"))
        .unwrap();
    assert!(off.rate.is_none() && off.credits.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_service_tier_the_turn_ran_with_is_on_the_row_and_counts_at_its_multiple() {
    let world = World::on_a_key(KEY).await;
    give_model_a_rate(
        &world,
        &json!({
            "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10",
            "tiers": { "fast": "2" }
        }),
    )
    .await;
    let thread = world.thread("Tiers").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    // The adapter has a parameter whose options are the model's service tiers (said after the harness
    // started, which declares what it declares), and the turn ran with one.
    sqlx::query("UPDATE agents SET descriptor = jsonb_set(descriptor, '{parameters}', $1)")
        .bind(json!([{
            "key": "service_tier", "title": "Speed", "description": "", "group": "model", "type": "enum",
            "options": { "from_model": "service_tiers" }, "default": null, "scopes": ["thread"],
            "apply": "next_turn"
        }]))
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE turns SET params = $2 WHERE id = $1")
        .bind(start.turn_id)
        .bind(json!({ "service_tier": "fast" }))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let lease = world.open_lease_of(thread).await;
    world
        .say_of_provider(request(lease, MODEL, (1_000_000, 0, 0), Some("t1")))
        .await;
    let rows = world.wait_ledger(start.turn_id, 1).await;
    assert_eq!(rows[0].tier.as_deref(), Some("fast"));
    assert_eq!(rows[0].credits.as_deref(), Some("4.000000"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_a_lease_repairs_a_missing_report_once_and_late_reports_cannot_count_it_again() {
    let world = World::on_a_key(KEY).await;
    give_model_a_rate(&world, &a_rate("2", "0.2", "10")).await;
    let thread = world.thread("Closing totals").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;
    world
        .say_of_provider_taken_in(request(lease, MODEL, (100, 20, 10), Some("delivered")))
        .await;
    world.machine.handler.on(CommandKind::LeaseClose, |_| {
        Ok(CommandResult::LeaseClose(LeaseCloseResult {
            requests: 2,
            usage: serde_json::from_value(
                serde_json::to_value(Some(json!({
                    "input_tokens": 300, "cached_input_tokens": 20, "cache_write_tokens": 0,
                    "output_tokens": 30, "reasoning_tokens": 0, "total_tokens": 330
                })))
                .unwrap(),
            )
            .unwrap(),
        }))
    });
    world
        .say(
            thread,
            common::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    let rows = world.wait_ledger(start.turn_id, 2).await;
    let remainder = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("lease.close"))
        .unwrap();
    assert_eq!(
        (
            remainder.input_tokens,
            remainder.output_tokens,
            remainder.total_tokens
        ),
        (200, 20, 220)
    );
    assert_eq!(remainder.credits.as_deref(), Some("0.000600"));
    world.restart_api().await;
    world
        .say_of_provider_taken_in(request(lease, MODEL, (200, 0, 20), Some("late")))
        .await;
    let after = world.ledger_of(start.turn_id).await;
    assert_eq!(
        after.len(),
        2,
        "replaying close or its missing report never counts it twice"
    );
    assert_eq!(after.iter().map(|row| row.total_tokens).sum::<i64>(), 330);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_with_no_shortfall_still_ends_the_meter_so_a_late_duplicate_is_ignored() {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Complete totals").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;
    world
        .say_of_provider_taken_in(request(lease, MODEL, (100, 0, 10), None))
        .await;
    world.machine.handler.on(CommandKind::LeaseClose, |_| {
        Ok(CommandResult::LeaseClose(LeaseCloseResult {
            requests: 1,
            usage: serde_json::from_value(
                serde_json::to_value(Some(json!({ "input_tokens": 100, "cached_input_tokens": 0,
                "output_tokens": 10, "reasoning_tokens": 0, "total_tokens": 110 })))
                .unwrap(),
            )
            .unwrap(),
        }))
    });
    world
        .say(
            thread,
            common::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    world.wait_ledger(start.turn_id, 2).await;
    world
        .say_of_provider_taken_in(request(lease, MODEL, (100, 0, 10), None))
        .await;
    let after = world.ledger_of(start.turn_id).await;
    assert_eq!(after.len(), 2);
    assert_eq!(after.iter().map(|row| row.total_tokens).sum::<i64>(), 110);
}

// ---------------------------------------------------------------- a refused request

async fn a_refused_request_fails_the_turn_and_the_thread_goes_on(reason: &'static str) {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Denied").await;
    let ada = Caller::session(world.ada);
    let (start, _) = world.running_turn(thread, "First").await;
    let lease = world.open_lease_of(thread).await;
    world
        .engine()
        .submit_input(&ada, thread, queued("Second"))
        .await
        .unwrap();

    world
        .say_of_provider(refused(lease, "gpt-other", reason))
        .await;
    world.wait_turn(start.turn_id, "failed").await;

    // The turn failed with the reason, the timeline says what was refused, and the harness is asked to stop.
    let error: Json = world
        .scalar("SELECT error FROM turns WHERE id = $1", start.turn_id)
        .await;
    assert_eq!(error["code"], reason);
    assert!(error["message"].as_str().unwrap().contains("gpt-other"));
    let errors: i64 = world
        .count(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'error'",
            thread,
        )
        .await;
    assert_eq!(errors, 1);
    world.wait_ran(CommandKind::TurnInterrupt, 1).await;
    let closed: Option<String> = world
        .scalar("SELECT close_reason FROM leases WHERE id = $1", lease)
        .await;
    assert_eq!(closed.as_deref(), Some("error"));

    // The thread is not blocked: the next input has its turn, on a lease of its own.
    let second = world.turn_start(thread, 1).await;
    assert_ne!(second.turn_id, start.turn_id);
    assert_ne!(world.state(thread).await.0, "blocked");
    assert_ne!(world.open_lease_of(thread).await, lease);

    // A refusal that is told again changes nothing.
    world
        .say_of_provider(refused(lease, "gpt-other", reason))
        .await;
    common::eventually(
        "the refused model report to write its one diagnostic",
        || async {
            world
                .count(
                    "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'error'",
                    thread,
                )
                .await
                == 1
        },
    )
    .await;
    let errors: i64 = world
        .count(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'error'",
            thread,
        )
        .await;
    assert_eq!(errors, 1);
    assert_eq!(world.turn_state(second.turn_id).await, "starting");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lease_that_refused_a_model_fails_its_turn_with_that_reason_and_the_thread_goes_on() {
    a_refused_request_fails_the_turn_and_the_thread_goes_on("model_not_allowed").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lease_that_refused_for_a_cap_fails_its_turn_with_that_reason_and_the_thread_goes_on() {
    a_refused_request_fails_the_turn_and_the_thread_goes_on("cap_reached").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refusal_that_comes_after_the_turn_was_over_is_still_told_once() {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Late").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;
    world
        .say(
            thread,
            common::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    // Each is in the database when it has been taken in: the second is no different for being told later.
    world
        .say_of_provider_taken_in(refused(lease, "gpt-other", "model_not_allowed"))
        .await;
    world
        .say_of_provider_taken_in(refused(lease, "gpt-other", "model_not_allowed"))
        .await;
    assert_eq!(
        world.turn_state(start.turn_id).await,
        "completed",
        "a turn that ended well stays so"
    );
    let errors: i64 = world
        .count(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'error'",
            thread,
        )
        .await;
    assert_eq!(errors, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reason_the_design_does_not_name_is_a_diagnostic_and_fails_nothing() {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Quiet").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;
    world
        .say_of_provider(refused(lease, MODEL, "path_not_allowed"))
        .await;
    meanwhile(Duration::from_millis(400)).await;
    assert_eq!(world.turn_state(start.turn_id).await, "running");
}

// ---------------------------------------------------------------- what the machine says of the provider

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_the_upstream_refuses_fails_the_turns_under_the_provider_and_blocks_its_threads_until_it_is_taken_again()
 {
    let world = World::on_a_key(KEY).await;
    let thread = world.thread("Refused").await;
    let ada = Caller::session(world.ada);
    let (start, _) = world.running_turn(thread, "First").await;
    world
        .engine()
        .submit_input(&ada, thread, queued("Second"))
        .await
        .unwrap();

    world
        .say_of_provider(Event::ProviderStatus(ProviderStatus {
            status: "error".to_owned(),
            detail: Some("the upstream answered 401".to_owned()),
        }))
        .await;
    world.wait_turn(start.turn_id, "failed").await;
    let error: Json = world
        .scalar("SELECT error FROM turns WHERE id = $1", start.turn_id)
        .await;
    assert_eq!(error["code"], "provider_unauthorized");
    assert_eq!(error["message"], "the upstream answered 401");
    let (status, detail): (String, Option<String>) =
        sqlx::query_as("SELECT status, status_detail FROM providers WHERE id = $1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        (status.as_str(), detail.as_deref()),
        ("error", Some("the upstream answered 401"))
    );

    // The next input waits, for the provider.
    world.wait_blocked(thread, "provider_unavailable").await;
    assert_eq!(world.turn_starts(thread).len(), 1);

    // The upstream takes the key again: the thread is looked at again and the input has its turn.
    world
        .say_of_provider(Event::ProviderStatus(ProviderStatus {
            status: "ready".to_owned(),
            detail: None,
        }))
        .await;
    let second = world.turn_start(thread, 1).await;
    assert_ne!(second.turn_id, start.turn_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_facts_a_machine_reports_of_its_provider_are_stored() {
    let world = World::on_a_key(KEY).await;
    let facts = json!({ "plan": "api", "credential": {
        "method": null,"yields":"key","issued_at":null,"expires_at":null
    } });
    world
        .say_of_provider(Event::ProviderFacts(
            serde_json::from_value(facts.clone()).unwrap(),
        ))
        .await;
    common::eventually("the facts are stored", || async {
        let stored: Json = world
            .scalar("SELECT facts FROM providers WHERE id = $1", world.provider)
            .await;
        stored["plan"] == facts["plan"] && stored["credential"] == facts["credential"]
    })
    .await;
    let at: Option<chrono::DateTime<chrono::Utc>> = world
        .scalar(
            "SELECT facts_at FROM providers WHERE id = $1",
            world.provider,
        )
        .await;
    assert!(at.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_that_does_not_keep_the_provider_cannot_speak_for_it() {
    let world = World::on_a_key(KEY).await;
    let stranger = world.add_machine("stranger", &["provider"]).await;
    let thread = world.thread("Strangers").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;
    let stream = Stream::Provider {
        provider_id: world.provider,
    };
    stranger
        .machine
        .publish(
            &stream,
            &Event::ProviderStatus(ProviderStatus {
                status: "error".to_owned(),
                detail: Some("lies".to_owned()),
            }),
        )
        .await;
    stranger
        .machine
        .publish(&stream, &request(lease, MODEL, (100, 0, 10), Some("fake")))
        .await;
    // What the machine that keeps it says after, is heard.
    world
        .say_of_provider(request(lease, MODEL, (7, 0, 3), Some("real")))
        .await;
    let rows = world.wait_ledger(start.turn_id, 1).await;
    meanwhile(Duration::from_millis(300)).await;
    let rows_later = world.ledger_of(start.turn_id).await;
    assert_eq!(rows_later.len(), 1, "{rows:?}");
    assert_eq!(rows_later[0].request_ref.as_deref(), Some("real"));
    let status: String = world
        .scalar("SELECT status FROM providers WHERE id = $1", world.provider)
        .await;
    assert_eq!(status, "ready");
    assert_eq!(world.turn_state(start.turn_id).await, "running");
}
