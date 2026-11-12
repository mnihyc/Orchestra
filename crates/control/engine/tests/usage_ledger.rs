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
        timing: None,
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
            usage_groups: None,
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
            usage_groups: None,
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

/// An actual wire observer's per-request facts use the existing event decoder so
/// this control compiles against the old carrier, which drops the unknown fields.
fn observed_raw_request(
    lease: Id,
    named: &str,
    usage: Json,
    requested: Option<&str>,
    reported: Json,
) -> Event {
    Event::decode(
        "lease.usage",
        json!({
            "lease_id":lease,"model":MODEL,"usage":usage,"request_ref":named,
            "timing":{
                "wire_protocol":"openai-responses",
                "requested_service_tier":requested,
                "reported_service_tier":reported
            }
        }),
    )
    .unwrap()
}
fn raw_tier_tokens(input: i64, cached: i64, written: i64, output: i64) -> Json {
    json!({"input_tokens":input,"cached_input_tokens":cached,"cache_write_tokens":written,
        "output_tokens":output,"reasoning_tokens":0,"total_tokens":input+output})
}
async fn raw_tier_world(tier: Option<&str>) -> (World, Id, Id, Id) {
    let world = World::on_a_key(KEY).await;
    give_model_a_rate(
        &world,
        &json!({
            "input_per_mtok":"2","cached_input_per_mtok":"0.5","cache_write_per_mtok":"2.5",
            "output_per_mtok":"8","tiers":{"fast":"2","priority":"3"}
        }),
    )
    .await;
    let thread = world.thread("Raw actual tier accounting").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    // Same actual retained turn/declared tier fixture as the existing tier test.
    sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{parameters}',$1)")
        .bind(
            json!([{"key":"service_tier","title":"Speed","description":"","group":"model",
            "type":"enum","options":{"from_model":"service_tiers"},"default":null,
            "scopes":["thread"],"apply":"next_turn"}]),
        )
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE turns SET params=$2 WHERE id=$1")
        .bind(start.turn_id)
        .bind(tier.map_or_else(|| json!({}), |tier| json!({"service_tier":tier})))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let lease = world.open_lease_of(thread).await;
    (world, thread, start.turn_id, lease)
}
async fn raw_tier_legacy_prime(
    world: &World,
    turn: Id,
    lease: Id,
    requested_tier: Option<&str>,
    expected_credits: &str,
) {
    let retained_tier: Option<String> =
        sqlx::query_scalar("SELECT params->>'service_tier' FROM turns WHERE id=$1")
            .bind(turn)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        retained_tier.as_deref(),
        requested_tier,
        "the legacy control uses this turn's actual requested-tier fixture"
    );
    world
        .say_of_provider_taken_in(request(
            lease,
            MODEL,
            (10, 0, 0),
            Some("raw-tier-native-prime"),
        ))
        .await;
    let rows = world.wait_ledger(turn, 1).await;
    let prime = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("raw-tier-native-prime"))
        .unwrap();
    assert_eq!(
        prime.tier.as_deref(),
        requested_tier,
        "old None facts keep native requested-tier compatibility"
    );
    assert_eq!(prime.credits.as_deref(), Some(expected_credits));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_forwarded_reported_downgrade_prices_actual_tier_once_and_cache_tokens_once() {
    let (world, _, turn, lease) = raw_tier_world(Some("fast")).await;
    raw_tier_legacy_prime(&world, turn, lease, Some("fast"), "0.000040").await;
    let event = observed_raw_request(
        lease,
        "actual-default",
        raw_tier_tokens(1000, 200, 100, 50),
        Some("fast"),
        json!({"state":"told","value":"default"}),
    );
    world.say_of_provider_taken_in(event.clone()).await;
    world.say_of_provider_taken_in(event).await;
    let rows = world.wait_ledger(turn, 2).await;
    let row = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("actual-default"))
        .unwrap();
    assert_eq!(
        row.tier, None,
        "actual default is ordinary even when requested fast"
    );
    // 700*2 fresh +200*.5 cached +100*2.5 write +50*8 output, per million.
    assert_eq!(
        row.credits.as_deref(),
        Some("0.002150"),
        "no requested-tier or double-cache charge"
    );
    assert_eq!(
        (
            row.input_tokens,
            row.cached_input_tokens,
            row.cache_write_tokens,
            row.output_tokens,
            row.total_tokens
        ),
        (1000, 200, 100, 50, 1050)
    );
    assert_eq!(row.measurement_states["requested_service_tier"], "fast");
    assert_eq!(
        row.measurement_states["reported_service_tier"],
        json!({"state":"told","value":"default"})
    );
    assert_eq!(
        world.ledger_of(turn).await.len(),
        2,
        "same request reference is counted once"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_forwarded_aware_unreported_tier_retains_wire_choice_and_known_rate_without_guessing_credits()
 {
    let (world, _, turn, lease) = raw_tier_world(Some("fast")).await;
    raw_tier_legacy_prime(&world, turn, lease, Some("fast"), "0.000040").await;
    world
        .say_of_provider_taken_in(observed_raw_request(
            lease,
            "actual-not-reported",
            raw_tier_tokens(1000, 200, 100, 50),
            Some("auto"),
            json!({"state":"empty"}),
        ))
        .await;
    let rows = world.wait_ledger(turn, 2).await;
    let row = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("actual-not-reported"))
        .unwrap();
    assert_eq!(
        row.credits, None,
        "explicit wire auto plus omitted actual is unrated, not the turn's requested fast or ordinary"
    );
    assert!(row.rate.is_some() && row.rate_source.as_deref() == Some("provider"));
    assert_eq!(row.tier, None);
    assert_eq!(
        row.measurement_states["requested_service_tier"], "auto",
        "wire fact survives independently of retained turn fast"
    );
    assert_eq!(
        row.measurement_states["reported_service_tier"],
        json!({"state":"empty"})
    );
    assert_eq!(row.total_tokens, 1050);
}

#[tokio::test]
async fn missing_actual_tier_stays_unrated_even_for_an_ordinary_requested_tier() {
    let (world, _, turn, lease) = raw_tier_world(None).await;
    raw_tier_legacy_prime(&world, turn, lease, None, "0.000020").await;
    for (index, requested) in [None, Some("default"), Some("standard")]
        .into_iter()
        .enumerate()
    {
        let named = format!("unreported-ordinary-{index}");
        world
            .say_of_provider_taken_in(observed_raw_request(
                lease,
                &named,
                raw_tier_tokens(1_000_000, 0, 0, 0),
                requested,
                json!({"state":"empty"}),
            ))
            .await;
        let rows = world.wait_ledger(turn, index + 2).await;
        let row = rows
            .iter()
            .find(|row| row.request_ref.as_deref() == Some(named.as_str()))
            .unwrap();
        assert_eq!(
            row.credits, None,
            "requested defaults do not prove the actual processing tier"
        );
        assert!(row.rate.is_some(), "known rate facts remain available");
    }
}

#[tokio::test]
async fn each_reported_tier_prices_the_request_independently_of_the_requested_tier() {
    let (world, _, turn, lease) = raw_tier_world(Some("fast")).await;
    give_model_a_rate(
        &world,
        &json!({"input_per_mtok":"2","cached_input_per_mtok":"0.5",
        "cache_write_per_mtok":"2.5","output_per_mtok":"8",
        "tiers":{"fast":"2","priority":"3","ultrafast":"6"}}),
    )
    .await;
    raw_tier_legacy_prime(&world, turn, lease, Some("fast"), "0.000040").await;
    for (index, (tier, price)) in [
        ("default", "2.000000"),
        ("fast", "4.000000"),
        ("priority", "6.000000"),
        ("ultrafast", "12.000000"),
    ]
    .into_iter()
    .enumerate()
    {
        let named = format!("observed-price-{tier}");
        world
            .say_of_provider_taken_in(observed_raw_request(
                lease,
                &named,
                raw_tier_tokens(1_000_000, 0, 0, 0),
                Some("ultrafast"),
                json!({"state":"told","value":tier}),
            ))
            .await;
        let rows = world.wait_ledger(turn, index + 2).await;
        let row = rows
            .iter()
            .find(|row| row.request_ref.as_deref() == Some(named.as_str()))
            .unwrap();
        assert_eq!(row.credits.as_deref(), Some(price), "actual tier {tier}");
        assert_eq!(row.tier.as_deref(), (tier != "default").then_some(tier));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_close_group_snapshot_preserves_mixed_actual_tiers_when_it_overtakes_a_request_report()
{
    let (world, thread, turn, lease) = raw_tier_world(None).await;
    // Both old and new code price this ordinary first request at2; the deciding
    // change is the actual priority group in the later aggregate close snapshot.
    world
        .say_of_provider_taken_in(observed_raw_request(
            lease,
            "arrived-before-close",
            raw_tier_tokens(1_000_000, 0, 0, 0),
            Some("fast"),
            json!({"state":"told","value":"default"}),
        ))
        .await;
    let first = world.wait_ledger(turn, 1).await;
    assert_eq!(first[0].credits.as_deref(), Some("2.000000"));
    let snapshot = json!({
        "requests":2,"usage":raw_tier_tokens(3_000_000,0,0,0),
        "usage_groups":[
            {"model":MODEL,"usage":raw_tier_tokens(1_000_000,0,0,0),"requests":1,
             "min_context_tokens":1_000_000,"max_context_tokens":1_000_000,
             "timing":{"wire_protocol":"openai-responses","requested_service_tier":"fast",
                 "reported_service_tier":{"state":"told","value":"default"}}},
            {"model":MODEL,"usage":raw_tier_tokens(2_000_000,0,0,0),"requests":1,
             "min_context_tokens":2_000_000,"max_context_tokens":2_000_000,
             "timing":{"wire_protocol":"openai-responses","requested_service_tier":"fast",
                 "reported_service_tier":{"state":"told","value":"priority"}}}
        ]
    });
    world.machine.handler.on(CommandKind::LeaseClose, move |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(snapshot.clone()).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    let rows = world.wait_ledger(turn, 2).await;
    let priority: Vec<_> = rows
        .iter()
        .filter(|row| row.tier.as_deref() == Some("priority"))
        .collect();
    assert_eq!(
        priority.len(),
        1,
        "known close snapshot must retain its actual priority group, not fabricate an ordinary residual"
    );
    assert_eq!(priority[0].total_tokens, 2_000_000);
    assert_eq!(
        priority[0].credits.as_deref(),
        Some("12.000000"),
        "exact actual multiplier wins over fast alias"
    );
    assert_eq!(
        rows.iter().map(|row| row.total_tokens).sum::<i64>(),
        3_000_000,
        "already-counted ordinary group is subtracted once"
    );
    let total: String = sqlx::query_scalar(
        "SELECT COALESCE(sum(credits),0)::text FROM usage_events WHERE turn_id=$1 AND counted",
    )
    .bind(turn)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(total, "14.000000");
    world.restart_api().await;
    world
        .say_of_provider_taken_in(observed_raw_request(
            lease,
            "overtaken-priority-report",
            raw_tier_tokens(2_000_000, 0, 0, 0),
            Some("fast"),
            json!({"state":"told","value":"priority"}),
        ))
        .await;
    assert_eq!(
        world.ledger_of(turn).await,
        rows,
        "existing close cut/replay prevents later request double counting"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_aware_empty_close_with_unknown_usage_still_cuts_late_reports() {
    let (world, thread, turn, lease) = raw_tier_world(Some("fast")).await;
    world.machine.handler.on(CommandKind::LeaseClose, |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(json!({"requests":1,"usage":null,"usage_groups":[]})).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    let rows = world.wait_ledger(turn, 1).await;
    assert_eq!(rows[0].total_tokens, 0);
    assert_eq!(rows[0].tier, None);
    assert_eq!(rows[0].context_tokens, None);
    assert_eq!(rows[0].credits, None, "an unread cut is not measured zero");
    assert_eq!(
        rows[0].measurement_states["reported_service_tier"],
        json!({"state":"unread"})
    );
    assert_private_close_snapshot(&rows[0], &json!({"usage":null,"usage_groups":[]}));
    world
        .say_of_provider_taken_in(observed_raw_request(
            lease,
            "late-after-unread-cut",
            raw_tier_tokens(1_000_000, 0, 0, 0),
            Some("fast"),
            json!({"state":"told","value":"priority"}),
        ))
        .await;
    assert_eq!(world.ledger_of(turn).await, rows);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_close_partial_group_snapshot_keeps_known_tier_and_unrates_only_uncovered_tokens() {
    let (world, thread, turn, lease) = raw_tier_world(Some("fast")).await;
    let snapshot = json!({
        "requests":2,"usage":raw_tier_tokens(3_000_000,0,0,0),
        "usage_groups":[
            {"model":MODEL,"usage":raw_tier_tokens(2_000_000,0,0,0),"requests":1,
             "min_context_tokens":2_000_000,"max_context_tokens":2_000_000,
             "timing":{"wire_protocol":"openai-responses","requested_service_tier":"fast",
                 "reported_service_tier":{"state":"told","value":"priority"}}}
        ]
    });
    world.machine.handler.on(CommandKind::LeaseClose, move |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(snapshot.clone()).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    let rows = world.wait_ledger(turn, 2).await;
    let known = rows
        .iter()
        .find(|row| row.tier.as_deref() == Some("priority"))
        .unwrap();
    assert_eq!(known.total_tokens, 2_000_000);
    assert_eq!(known.credits.as_deref(), Some("12.000000"));
    let uncovered = rows.iter().find(|row| row.tier.is_none()).unwrap();
    assert_eq!(uncovered.total_tokens, 1_000_000);
    assert_eq!(uncovered.credits, None);
    assert!(uncovered.rate.is_some());
    assert_eq!(
        uncovered.measurement_states["requested_service_tier"],
        Json::Null
    );
    assert_eq!(
        uncovered.measurement_states["reported_service_tier"],
        json!({"state":"unread"})
    );
    assert_eq!(
        rows.iter().map(|row| row.total_tokens).sum::<i64>(),
        3_000_000
    );
    world
        .say_of_provider_taken_in(observed_raw_request(
            lease,
            "late-uncovered-report",
            raw_tier_tokens(1_000_000, 0, 0, 0),
            None,
            json!({"state":"told","value":"default"}),
        ))
        .await;
    assert_eq!(world.ledger_of(turn).await, rows);
}

/// Emulates a connector journal and ledger written before private lease linkage existed.
/// The sibling distinguishes proved single-forwarded-lease history from genuine ambiguity.
async fn raw_legacy_grouped_close_world(sibling: bool) -> (World, Id, Json) {
    let (world, thread, turn, lease) = raw_tier_world(None).await;
    world
        .say_of_provider_taken_in(request(
            lease,
            MODEL,
            (1_000_000, 0, 0),
            Some("pre-upgrade"),
        ))
        .await;
    world.wait_ledger(turn, 1).await;
    sqlx::query("UPDATE usage_events SET measurement_states=measurement_states-'_lease_id' WHERE turn_id=$1 AND request_ref='pre-upgrade'")
        .bind(turn).execute(world.pool.pool()).await.unwrap();
    if sibling {
        sqlx::query("INSERT INTO leases (id,provider_id,thread_id,turn_id,mode,allowed_models,opened_at,closed_at,close_reason)
                     SELECT $2,provider_id,thread_id,turn_id,'forwarded',allowed_models,opened_at,now(),'turn_ended'
                       FROM leases WHERE id=$1")
            .bind(lease).bind(orch_types::new_id()).execute(world.pool.pool()).await.unwrap();
    }
    let snapshot = json!({
        "requests":2,"usage":raw_tier_tokens(3_000_000,0,0,0),
        "usage_groups":[
            {"model":MODEL,"usage":raw_tier_tokens(2_000_000,0,0,0),"requests":1,
             "min_context_tokens":2_000_000,"max_context_tokens":2_000_000,
             "timing":{"wire_protocol":"openai-responses","requested_service_tier":"fast",
                 "reported_service_tier":{"state":"told","value":"priority"}}}
        ]
    });
    let reply = snapshot.clone();
    world.machine.handler.on(CommandKind::LeaseClose, move |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(reply.clone()).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    (world, turn, snapshot)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_close_sole_forwarded_legacy_usage_keeps_exact_aggregate_without_guessing_group() {
    let (world, turn, snapshot) = raw_legacy_grouped_close_world(false).await;
    let rows = world.wait_ledger(turn, 2).await;
    assert_eq!(
        rows.iter().map(|row| row.total_tokens).sum::<i64>(),
        3_000_000
    );
    let remainder = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("lease.close"))
        .unwrap();
    assert_eq!(remainder.total_tokens, 2_000_000);
    assert_eq!(remainder.tier, None);
    assert_eq!(remainder.credits, None);
    assert_eq!(
        remainder.measurement_states["reported_service_tier"],
        json!({"state":"unread"})
    );
    assert_private_close_snapshot(remainder, &snapshot);
    let old = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("pre-upgrade"))
        .unwrap();
    assert_eq!(old.total_tokens, 1_000_000);
    assert_eq!(
        old.measurement_states["_lease_id"],
        Json::Null,
        "migration proof never rewrites private linkage"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_close_ambiguous_legacy_history_uses_unrated_aggregate_cap_without_group_allocation() {
    let (world, turn, snapshot) = raw_legacy_grouped_close_world(true).await;
    let rows = world.wait_ledger(turn, 2).await;
    let unread = rows
        .iter()
        .find(|row| row.measurement_states["reported_service_tier"] == json!({"state":"unread"}))
        .unwrap();
    assert_eq!(
        unread.total_tokens, 2_000_000,
        "legacy aggregate cap applies before every group allocation"
    );
    assert_eq!(unread.credits, None);
    assert_eq!(unread.context_tokens, None);
    assert_private_close_snapshot(unread, &snapshot);
    assert_eq!(
        rows.iter().map(|row| row.total_tokens).sum::<i64>(),
        3_000_000
    );
}

fn raw_all_tokens(input: i64, cached: i64, written: i64, output: i64, reasoning: i64) -> Json {
    json!({"input_tokens":input,"cached_input_tokens":cached,"cache_write_tokens":written,
        "output_tokens":output,"reasoning_tokens":reasoning,"total_tokens":input+output})
}

fn raw_actual_group(usage: &Json, actual: &str) -> Json {
    json!({"model":MODEL,"usage":usage,"requests":1,
        "min_context_tokens":usage["input_tokens"],"max_context_tokens":usage["input_tokens"],
        "timing":{"wire_protocol":"openai-responses","requested_service_tier":"fast",
            "reported_service_tier":{"state":"told","value":actual}}})
}

/// Uses the exact pre-v1 carrier shape: serde accepts the additive wire timing,
/// drops it, and produces the old request body. The envelope update emulates
/// that old Host recording before private lease linkage was introduced.
async fn raw_old_decoder_overlap_world(
    old_usage: &Json,
    snapshot: &Json,
    old_host: bool,
) -> (World, Id, Id) {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct OldLeaseUsage {
        lease_id: Id,
        model: String,
        usage: orch_agent_contract::events::TokenUsage,
        request_ref: Option<String>,
    }
    let (world, thread, turn, lease) = raw_tier_world(None).await;
    let wire = observed_raw_request(
        lease,
        "decoded-by-old-host",
        old_usage.clone(),
        Some("fast"),
        json!({"state":"told","value":"default"}),
    )
    .body()
    .unwrap();
    assert!(wire.get("timing").is_some());
    let old: OldLeaseUsage = serde_json::from_value(wire).unwrap();
    let dropped = serde_json::to_value(old).unwrap();
    assert!(
        dropped.get("timing").is_none(),
        "the pre-v1 decoder actually drops additive facts"
    );
    world
        .say_of_provider_taken_in(Event::decode("lease.usage", dropped).unwrap())
        .await;
    world.wait_ledger(turn, 1).await;
    if old_host {
        sqlx::query("UPDATE usage_events SET measurement_states=measurement_states-'_lease_id' WHERE turn_id=$1 AND request_ref='decoded-by-old-host'")
            .bind(turn).execute(world.pool.pool()).await.unwrap();
    }
    let old = world.ledger_of(turn).await;
    assert_eq!(old[0].measurement_states["wire_protocol"], Json::Null);
    assert_eq!(
        old[0].measurement_states["requested_service_tier"],
        Json::Null
    );
    assert_eq!(
        old[0].measurement_states["reported_service_tier"],
        json!({"state":"unsupported"})
    );
    assert_eq!(old[0].measurement_states["_lease_id"].is_null(), old_host);
    let snapshot = snapshot.clone();
    world.machine.handler.on(CommandKind::LeaseClose, move |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(snapshot.clone()).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    (world, turn, lease)
}

fn assert_private_close_snapshot(marker: &orch_db::usage::ledger::EventRow, received: &Json) {
    let retained = &marker.measurement_states["_lease_close_snapshot"];
    let object = retained.as_object().unwrap();
    assert_eq!(object.len(), 2);
    assert!(object.contains_key("usage") && object.contains_key("usage_groups"));
    let actual: Option<orch_agent_contract::events::TokenUsage> =
        serde_json::from_value(retained["usage"].clone()).unwrap();
    let expected: Option<orch_agent_contract::events::TokenUsage> =
        serde_json::from_value(received["usage"].clone()).unwrap();
    assert_eq!(
        actual, expected,
        "the received inclusive aggregate is retained"
    );
    let actual: Vec<orch_link_contract::commands::LeaseUsageGroup> =
        serde_json::from_value(retained["usage_groups"].clone()).unwrap();
    let expected: Vec<orch_link_contract::commands::LeaseUsageGroup> =
        serde_json::from_value(received["usage_groups"].clone()).unwrap();
    assert_eq!(
        actual.len(),
        expected.len(),
        "only the actually received group subset is retained"
    );
    let keys: Vec<_> = actual
        .iter()
        .map(|group| serde_json::to_string(&group.key()).unwrap())
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(
        keys, sorted,
        "snapshot ordering uses the same canonical factual identity"
    );
    for group in &actual {
        let original = expected
            .iter()
            .find(|original| original.key() == group.key())
            .unwrap();
        assert_eq!(group.model, original.model);
        assert_eq!(group.usage, original.usage);
        assert_eq!(group.requests, original.requests);
        assert_eq!(
            (group.min_context_tokens, group.max_context_tokens),
            (original.min_context_tokens, original.max_context_tokens)
        );
        assert!(
            group.timing.is_unsupported(),
            "only tier facts are retained, never performance claims"
        );
        assert_eq!(group.timing.vendor_words, None);
        assert_eq!(group.timing.error_message, None);
    }
    for group in retained["usage_groups"].as_array().unwrap() {
        assert_eq!(
            group["timing"].as_object().unwrap().len(),
            3,
            "only the three factual timing members are private"
        );
    }
    assert!(
        serde_json::to_vec(retained).unwrap().len()
            <= orch_link_contract::version::MAX_FRAME_BYTES as usize
    );
    let public = serde_json::to_value(marker.timing()).unwrap();
    assert!(
        public.get("_lease_close_snapshot").is_none(),
        "public timing never exposes the private audit aggregate"
    );
}

async fn assert_old_decoder_overlap(
    old_usage: Json,
    total: Json,
    groups: Vec<Json>,
    remaining: Json,
    old_host: bool,
    requests: i64,
) {
    let snapshot = json!({"requests":requests,"usage":total,"usage_groups":groups,
        "unrelated_answer_metadata":{"content":"not retained"}});
    let (world, turn, lease) = raw_old_decoder_overlap_world(&old_usage, &snapshot, old_host).await;
    let rows = world.wait_ledger(turn, 2).await;
    let marker = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("lease.close"))
        .unwrap();
    assert_eq!(marker.tier, None);
    assert_eq!(marker.credits, None);
    assert!(marker.rate.is_some());
    assert_eq!(marker.context_tokens, None);
    assert_eq!(
        marker.measurement_states["reported_service_tier"],
        json!({"state":"unread"})
    );
    assert_private_close_snapshot(marker, &snapshot);
    assert_eq!(
        [
            marker.input_tokens,
            marker.cached_input_tokens,
            marker.cache_write_tokens,
            marker.output_tokens,
            marker.reasoning_tokens,
            marker.total_tokens
        ],
        [
            "input_tokens",
            "cached_input_tokens",
            "cache_write_tokens",
            "output_tokens",
            "reasoning_tokens",
            "total_tokens"
        ]
        .map(|field| remaining[field].as_i64().unwrap()),
    );
    assert_eq!(
        [
            rows.iter().map(|row| row.input_tokens).sum(),
            rows.iter().map(|row| row.cached_input_tokens).sum(),
            rows.iter().map(|row| row.cache_write_tokens).sum(),
            rows.iter().map(|row| row.output_tokens).sum(),
            rows.iter().map(|row| row.reasoning_tokens).sum(),
            rows.iter().map(|row| row.total_tokens).sum::<i64>()
        ],
        [
            "input_tokens",
            "cached_input_tokens",
            "cache_write_tokens",
            "output_tokens",
            "reasoning_tokens",
            "total_tokens"
        ]
        .map(|field| total[field].as_i64().unwrap()),
        "all six aggregate members are counted once",
    );
    // A second caller cannot replace the first canonical snapshot, even with
    // different group counts. The real close replay is also gated by its cut.
    let mut altered: Vec<orch_link_contract::commands::LeaseUsageGroup> =
        serde_json::from_value(snapshot["usage_groups"].clone()).unwrap();
    if let Some(group) = altered.first_mut() {
        group.requests += 1;
    }
    let usage: orch_agent_contract::events::TokenUsage =
        serde_json::from_value(total.clone()).unwrap();
    {
        let mut conn = world.pool.pool().acquire().await.unwrap();
        assert!(
            !orch_db::usage::ledger::retain_lease_close_snapshot(
                &mut conn,
                marker.id,
                lease,
                Some(&usage),
                &altered,
            )
            .await
            .unwrap()
        );
    }
    assert_eq!(world.ledger_of(turn).await, rows);
    world.restart_api().await;
    let late = observed_raw_request(
        lease,
        "late-overlap-report",
        old_usage,
        Some("fast"),
        json!({"state":"told","value":"priority"}),
    );
    world.say_of_provider_taken_in(late.clone()).await;
    world.say_of_provider_taken_in(late).await;
    assert_eq!(
        world.ledger_of(turn).await,
        rows,
        "restart and replay preserve the canonical cut"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_old_host_decoder_full_group_overlap_counts_once_and_cuts_replay() {
    let old = raw_all_tokens(1000, 200, 100, 50, 10);
    assert_old_decoder_overlap(
        old.clone(),
        old.clone(),
        vec![raw_actual_group(&old, "default")],
        raw_all_tokens(0, 0, 0, 0, 0),
        true,
        1,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_old_host_decoder_mixed_group_overlap_counts_exact_unassignable_remainder() {
    let old = raw_all_tokens(1000, 200, 100, 50, 10);
    let priority = raw_all_tokens(2000, 400, 200, 100, 20);
    let mut priority_group = raw_actual_group(&priority, "priority");
    priority_group["timing"]["vendor_words"] = json!("not retained");
    assert_old_decoder_overlap(
        old.clone(),
        raw_all_tokens(3000, 600, 300, 150, 30),
        vec![priority_group, raw_actual_group(&old, "default")],
        priority,
        true,
        2,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_old_host_decoder_zero_group_overlap_still_records_unread_cut() {
    let zero = raw_all_tokens(0, 0, 0, 0, 0);
    assert_old_decoder_overlap(
        zero.clone(),
        zero.clone(),
        vec![raw_actual_group(&zero, "default")],
        zero,
        true,
        1,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_linked_legacy_without_actual_facts_does_not_allocate_overlapping_close_groups() {
    let old = raw_all_tokens(1000, 200, 100, 50, 10);
    let priority = raw_all_tokens(2000, 400, 200, 100, 20);
    assert_old_decoder_overlap(
        old.clone(),
        raw_all_tokens(3000, 600, 300, 150, 30),
        vec![raw_actual_group(&priority, "priority")],
        priority,
        false,
        2,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_inconsistent_close_groups_keep_received_facts_and_count_only_exact_aggregate() {
    let (world, thread, turn, _) = raw_tier_world(Some("fast")).await;
    let total = raw_all_tokens(100, 20, 10, 5, 1);
    let conflicting = raw_all_tokens(1000, 200, 100, 50, 10);
    let snapshot = json!({"requests":1,"usage":total,
        "usage_groups":[raw_actual_group(&conflicting,"priority")]});
    let reply = snapshot.clone();
    world.machine.handler.on(CommandKind::LeaseClose, move |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(reply.clone()).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    let rows = world.wait_ledger(turn, 1).await;
    assert_eq!(rows[0].input_tokens, 100);
    assert_eq!(rows[0].cached_input_tokens, 20);
    assert_eq!(rows[0].cache_write_tokens, 10);
    assert_eq!(rows[0].output_tokens, 5);
    assert_eq!(rows[0].reasoning_tokens, 1);
    assert_eq!(rows[0].total_tokens, 105);
    assert_eq!(rows[0].tier, None);
    assert_eq!(rows[0].credits, None);
    assert_eq!(
        rows[0].measurement_states["reported_service_tier"],
        json!({"state":"unread"})
    );
    assert_private_close_snapshot(&rows[0], &snapshot);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_search_reports_record_actual_counts_and_price_once() {
    let (world, _, turn, lease) = raw_tier_world(None).await;
    give_model_a_rate(&world,&json!({"input_per_mtok":"2","cached_input_per_mtok":"0.5","output_per_mtok":"8","web_search_per_call":"0.01"})).await;
    let report = orch_link_contract::events::Event::decode("lease.usage", json!({
        "lease_id":lease,"model":MODEL,"usage":raw_tier_tokens(1000,0,0,0),"request_ref":"search-reply",
        "timing":{"wire_protocol":"openai-responses","reported_service_tier":{"state":"told","value":"default"},
            "web_searches":{"state":"told","value":2}}
    })).unwrap();
    world.say_of_provider_taken_in(report.clone()).await;
    world.say_of_provider_taken_in(report).await;
    let rows = world.wait_ledger(turn, 1).await;
    assert_eq!(
        rows[0].measurement_states["web_searches"],
        json!({"state":"told","value":2})
    );
    assert_eq!(rows[0].credits.as_deref(), Some("0.022000"));
    assert_eq!(world.ledger_of(turn).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn raw_search_close_subtracts_reported_searches_and_cuts_late_replay() {
    let (world, thread, turn, lease) = raw_tier_world(None).await;
    give_model_a_rate(&world,&json!({"input_per_mtok":"2","cached_input_per_mtok":"0.5","output_per_mtok":"8","web_search_per_call":"0.01"})).await;
    let timing = json!({"wire_protocol":"openai-responses","reported_service_tier":{"state":"told","value":"default"},"web_searches":{"state":"told","value":1}});
    let report = orch_link_contract::events::Event::decode("lease.usage",json!({"lease_id":lease,"model":MODEL,"usage":raw_tier_tokens(1000,0,0,0),"request_ref":"first-search","timing":timing})).unwrap();
    world.say_of_provider_taken_in(report.clone()).await;
    world.wait_ledger(turn, 1).await;
    let snapshot = json!({"requests":2,"usage":raw_tier_tokens(3000,0,0,0),"usage_groups":[{
        "model":MODEL,"usage":raw_tier_tokens(3000,0,0,0),"requests":2,"min_context_tokens":1000,"max_context_tokens":2000,
        "search_usage":{"searches":3,"known_requests":2},"timing":timing
    }]});
    world.machine.handler.on(CommandKind::LeaseClose, move |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(snapshot.clone()).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    let rows = world.wait_ledger(turn, 2).await;
    let residual = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("lease.close"))
        .unwrap();
    assert_eq!(
        residual.measurement_states["web_searches"],
        json!({"state":"told","value":2})
    );
    assert_eq!(residual.credits.as_deref(), Some("0.024000"));
    world.say_of_provider_taken_in(report).await;
    assert_eq!(world.ledger_of(turn).await, rows);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reported_vendor_totals_replace_only_catalogue_credits_and_remain_comparable_to_explicit_rates()
 {
    let world = World::on_a_key(KEY).await;
    sqlx::query("UPDATE provider_models SET parameters=parameters-'rate'")
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) VALUES($1,$2,'priced-own','',$3,true,'manual',$4)")
        .bind(orch_types::new_id()).bind(world.provider).bind(json!({"rate":{"input_per_mtok":"7","cached_input_per_mtok":"7","output_per_mtok":"7","web_search_per_call":"0"}})).bind(world.ada).execute(world.pool.pool()).await.unwrap();
    world
        .set(
            world.root,
            keys::USAGE_RATES,
            json!({"priced-card":{"input_per_mtok":"3","cached_input_per_mtok":"3","output_per_mtok":"3","web_search_per_call":"0"}}),
        )
        .await;
    world
        .set(world.root, keys::USAGE_CATALOGUE_SCALE, json!("2"))
        .await;
    sqlx::query("INSERT INTO rate_catalogue(id,doc,as_of,origin) VALUES(true,$1,'2026-10-10','shipped')")
        .bind(json!({"format":1,"as_of":"2026-10-10","unit":"USD","families":[{"match":"priced-*","vendor":"Fixture","source":"https://example.invalid","as_of":"2026-10-10","rate":{"input_per_mtok":"4","cached_input_per_mtok":"0.4","output_per_mtok":"8","web_search_per_call":"0.01"}}]}))
        .execute(world.pool.pool()).await.unwrap();
    let thread = world.thread("Reported cost policy").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    let lease = world.open_lease_of(thread).await;
    for (model, named, unit, amount) in [
        ("priced-own", "own", "USD", "0.0071"),
        ("priced-card", "card", "USD", "0.0071"),
        ("priced-catalogue", "actual", "USD", "0.0071"),
        ("priced-catalogue", "mismatch", "EUR", "0.0071"),
        ("priced-catalogue", "zero", "USD", "0"),
    ] {
        let event=Event::decode("lease.usage",json!({"lease_id":lease,"model":model,"usage":{"input_tokens":1000000,"cached_input_tokens":0,"output_tokens":0,"reasoning_tokens":0,"total_tokens":1000000},"request_ref":named,"timing":{"wire_protocol":"openai-responses","web_searches":{"state":"told","value":2},"reported_cost":{"state":"told","value":{"amount":amount,"unit":unit}}}})).unwrap();
        world.say_of_provider_taken_in(event.clone()).await;
        world.say_of_provider_taken_in(event).await;
    }
    let rows = world.wait_ledger(start.turn_id, 5).await;
    let row = |name: &str| {
        rows.iter()
            .find(|row| row.request_ref.as_deref() == Some(name))
            .unwrap()
    };
    for (name, source, credits) in [
        ("own", "provider", "7.000000"),
        ("card", "rate_card", "3.000000"),
        ("actual", "reported", "0.014200"),
        ("mismatch", "catalogue", "8.040000"),
        ("zero", "reported", "0.000000"),
    ] {
        assert_eq!(row(name).rate_source.as_deref(), Some(source));
        assert_eq!(row(name).credits.as_deref(), Some(credits));
    }
    assert_eq!(
        row("own").measurement_states["reported_cost"],
        json!({"state":"told","value":{"amount":"0.0071","unit":"USD"}})
    );
    assert_eq!(
        world.ledger_of(start.turn_id).await.len(),
        5,
        "replay never counts reported totals twice"
    );
}

async fn reported_cost_close_world() -> (World, Id, Id, Id) {
    let (world, thread, turn, lease) = raw_tier_world(None).await;
    sqlx::query("UPDATE provider_models SET parameters=parameters-'rate'")
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .set(world.root, keys::USAGE_CATALOGUE_SCALE, json!("2"))
        .await;
    sqlx::query("INSERT INTO rate_catalogue(id,doc,as_of,origin) VALUES(true,$1,'2026-10-10','shipped')")
        .bind(json!({"format":1,"as_of":"2026-10-10","unit":"USD","families":[{"match":MODEL,"vendor":"Fixture","source":"https://example.invalid","as_of":"2026-10-10","rate":{"input_per_mtok":"4","cached_input_per_mtok":"0.4","output_per_mtok":"8","web_search_per_call":"0.01"}}]}))
        .execute(world.pool.pool()).await.unwrap();
    (world, thread, turn, lease)
}
fn reported_cost_event(lease: Id, reference: &str, input: i64, cost: &str, searches: i64) -> Event {
    Event::decode("lease.usage",json!({"lease_id":lease,"model":MODEL,"usage":raw_tier_tokens(input,0,0,0),"request_ref":reference,"timing":{
        "wire_protocol":"openai-responses","outcome":{"state":"told","value":"done"},"reported_service_tier":{"state":"told","value":"default"},
        "web_searches":{"state":"told","value":searches},"reported_cost":{"state":"told","value":{"amount":cost,"unit":"USD"}}}})).unwrap()
}
fn reported_cost_close(known: i64, unit: &str) -> Json {
    json!({"requests":2,"usage":raw_tier_tokens(3000,0,0,0),"usage_groups":[{
        "model":MODEL,"usage":raw_tier_tokens(3000,0,0,0),"requests":2,"min_context_tokens":1000,"max_context_tokens":2000,
        "search_usage":{"searches":5,"known_requests":2},
        "reported_cost_usage":{"total":{"amount":"0.0194","unit":unit},"known_requests":known},
        "timing":{"wire_protocol":"openai-responses","reported_service_tier":{"state":"told","value":"default"}}}]})
}
async fn finish_reported_cost_close(world: &World, thread: Id, turn: Id, snapshot: Json) {
    world.machine.handler.on(CommandKind::LeaseClose, move |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(snapshot.clone()).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reported_close_totals_subtract_already_counted_costs_and_late_requests_remain_audit_only()
{
    let (world, thread, turn, lease) = reported_cost_close_world().await;
    world
        .say_of_provider_taken_in(reported_cost_event(lease, "cost-first", 1000, "0.0071", 2))
        .await;
    world.wait_ledger(turn, 1).await;
    finish_reported_cost_close(&world, thread, turn, reported_cost_close(2, "USD")).await;
    let rows = world.wait_ledger(turn, 2).await;
    let residual = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("lease.close"))
        .unwrap();
    assert_eq!(residual.input_tokens, 2000);
    assert_eq!(residual.rate_source.as_deref(), Some("reported"));
    assert_eq!(residual.credits.as_deref(), Some("0.024600"));
    assert_eq!(
        residual.measurement_states["reported_cost"],
        json!({"state":"told","value":{"amount":"0.0123","unit":"USD"}})
    );
    let late = reported_cost_event(lease, "cost-second", 2000, "0.0123", 3);
    world.say_of_provider_taken_in(late.clone()).await;
    world.say_of_provider_taken_in(late).await;
    let rows = world.wait_ledger(turn, 3).await;
    assert!(
        !rows
            .iter()
            .find(|row| row.request_ref.as_deref() == Some("cost-second"))
            .unwrap()
            .counted
    );
    let total: String = world
        .scalar(
            "SELECT sum(credits)::text FROM usage_events WHERE turn_id=$1 AND counted",
            turn,
        )
        .await;
    assert_eq!(total, "0.038800");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_complete_close_without_request_frames_keeps_the_actual_total_instead_of_search_estimates()
 {
    let (world, thread, turn, _) = reported_cost_close_world().await;
    finish_reported_cost_close(&world, thread, turn, reported_cost_close(2, "USD")).await;
    let rows = world.wait_ledger(turn, 1).await;
    assert_eq!(rows[0].rate_source.as_deref(), Some("reported"));
    assert_eq!(rows[0].credits.as_deref(), Some("0.038800"));
    assert_eq!(
        rows[0].measurement_states["reported_cost"],
        json!({"state":"told","value":{"amount":"0.0194","unit":"USD"}})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_partial_or_different_currency_close_total_keeps_the_catalogue_estimate() {
    for (known, unit) in [(1, "USD"), (2, "EUR")] {
        let (world, thread, turn, _) = reported_cost_close_world().await;
        finish_reported_cost_close(&world, thread, turn, reported_cost_close(known, unit)).await;
        let rows = world.wait_ledger(turn, 1).await;
        assert_eq!(rows[0].rate_source.as_deref(), Some("catalogue"));
        assert_eq!(rows[0].credits.as_deref(), Some("0.124000"));
        if known == 1 {
            assert_eq!(
                rows[0].measurement_states["reported_cost"],
                json!({"state":"unread"})
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capacity_close_groups_subtract_counted_capacity_and_price_only_the_missing_native_class() {
    let (world, thread, turn, lease) = raw_tier_world(None).await;
    give_model_a_rate(
        &world,
        &json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1","output_per_mtok":"2",
        "tiers":{"fast":"2"},"capacity_tiers":{"priority":"3"}}),
    )
    .await;
    let timing = |capacity: &str| {
        json!({"wire_protocol":"anthropic-messages",
        "requested_service_tier":"fast","reported_service_tier":{"state":"told","value":"fast"},
        "requested_capacity_tier":"auto","reported_capacity_tier":{"state":"told","value":capacity}})
    };
    let report=Event::decode("lease.usage",json!({"lease_id":lease,"model":MODEL,
        "usage":raw_tier_tokens(1_000_000,0,0,0),"request_ref":"capacity-before-close","timing":timing("standard")})).unwrap();
    world.say_of_provider_taken_in(report.clone()).await;
    let first = world.wait_ledger(turn, 1).await;
    assert_eq!(first[0].credits.as_deref(), Some("2.000000"));
    let snapshot = json!({"requests":2,"usage":raw_tier_tokens(3_000_000,0,0,0),"usage_groups":[
        {"model":MODEL,"usage":raw_tier_tokens(1_000_000,0,0,0),"requests":1,"min_context_tokens":1_000_000,"max_context_tokens":1_000_000,"timing":timing("standard")},
        {"model":MODEL,"usage":raw_tier_tokens(2_000_000,0,0,0),"requests":1,"min_context_tokens":2_000_000,"max_context_tokens":2_000_000,"timing":timing("priority")}
    ]});
    world.machine.handler.on(CommandKind::LeaseClose, move |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(snapshot.clone()).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    let rows = world.wait_ledger(turn, 2).await;
    let priority: Vec<_> = rows
        .iter()
        .filter(|row| {
            row.measurement_states["reported_capacity_tier"]
                == json!({"state":"told","value":"priority"})
        })
        .collect();
    assert_eq!(
        priority.len(),
        1,
        "the residual keeps its actual capacity class"
    );
    assert_eq!(priority[0].input_tokens, 2_000_000);
    assert_eq!(priority[0].credits.as_deref(), Some("12.000000"));
    assert_eq!(
        rows.iter().map(|row| row.input_tokens).sum::<i64>(),
        3_000_000
    );
    let total: String = sqlx::query_scalar(
        "SELECT COALESCE(sum(credits),0)::text FROM usage_events WHERE turn_id=$1 AND counted",
    )
    .bind(turn)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(total, "14.000000");
    world.say_of_provider_taken_in(report).await;
    assert_eq!(world.ledger_of(turn).await, rows);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hourly_cache_close_subtracts_the_already_priced_subset_without_a_second_charge() {
    let (world, thread, turn, lease) = raw_tier_world(None).await;
    give_model_a_rate(
        &world,
        &json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1","output_per_mtok":"2",
        "cache_write_per_mtok":"1.25","cache_write_1h_per_mtok":"2"}),
    )
    .await;
    let timing = |hourly: i64| {
        json!({"wire_protocol":"anthropic-messages",
        "reported_service_tier":{"state":"told","value":"standard"},
        "reported_capacity_tier":{"state":"told","value":"standard"},
        "cache_write_1h_tokens":{"state":"told","value":hourly}})
    };
    let report = Event::decode(
        "lease.usage",
        json!({"lease_id":lease,"model":MODEL,"request_ref":"hourly-before-close",
        "usage":raw_tier_tokens(1000,0,300,0),"timing":timing(100)}),
    )
    .unwrap();
    world.say_of_provider_taken_in(report.clone()).await;
    assert_eq!(
        world.wait_ledger(turn, 1).await[0].credits.as_deref(),
        Some("0.001150")
    );
    let snapshot = json!({"requests":2,"usage":raw_tier_tokens(2000,0,600,0),"usage_groups":[{
        "model":MODEL,"usage":raw_tier_tokens(2000,0,600,0),"requests":2,
        "min_context_tokens":1000,"max_context_tokens":1000,
        "cache_write_usage":{"one_hour_tokens":300,"known_requests":2},"timing":timing(100)
    }]});
    world.machine.handler.on(CommandKind::LeaseClose, move |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(snapshot.clone()).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    let rows = world.wait_ledger(turn, 2).await;
    let missing = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("lease.close"))
        .unwrap();
    assert_eq!(missing.cache_write_tokens, 300);
    assert_eq!(
        missing.measurement_states["cache_write_1h_tokens"],
        json!({"state":"told","value":200})
    );
    assert_eq!(missing.credits.as_deref(), Some("0.001225"));
    let total: String = sqlx::query_scalar(
        "SELECT COALESCE(sum(credits),0)::text FROM usage_events WHERE turn_id=$1 AND counted",
    )
    .bind(turn)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(total, "0.002375");
    world.say_of_provider_taken_in(report).await;
    assert_eq!(world.ledger_of(turn).await, rows);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreported_old_hourly_subset_cannot_acquire_a_guessed_close_allocation() {
    let (world, thread, turn, lease) = raw_tier_world(None).await;
    give_model_a_rate(
        &world,
        &json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1","output_per_mtok":"2",
        "cache_write_per_mtok":"1.25","cache_write_1h_per_mtok":"2"}),
    )
    .await;
    let timing = json!({"wire_protocol":"anthropic-messages","reported_service_tier":{"state":"told","value":"standard"},
        "reported_capacity_tier":{"state":"told","value":"standard"}});
    let report = Event::decode(
        "lease.usage",
        json!({"lease_id":lease,"model":MODEL,"request_ref":"unreported-hourly-subset",
        "usage":raw_tier_tokens(1000,0,300,0),"timing":timing}),
    )
    .unwrap();
    world.say_of_provider_taken_in(report).await;
    world.wait_ledger(turn, 1).await;
    let snapshot = json!({"requests":2,"usage":raw_tier_tokens(2000,0,600,0),"usage_groups":[{
        "model":MODEL,"usage":raw_tier_tokens(2000,0,600,0),"requests":2,
        "min_context_tokens":1000,"max_context_tokens":1000,
        "cache_write_usage":{"one_hour_tokens":300,"known_requests":2},"timing":timing
    }]});
    world.machine.handler.on(CommandKind::LeaseClose, move |_| {
        Ok(CommandResult::LeaseClose(
            serde_json::from_value(snapshot.clone()).unwrap(),
        ))
    });
    world
        .say(
            thread,
            common::turn_ended(turn, orch_link_contract::events::TurnOutcome::Completed),
        )
        .await;
    let rows = world.wait_ledger(turn, 2).await;
    let missing = rows
        .iter()
        .find(|row| row.request_ref.as_deref() == Some("lease.close"))
        .unwrap();
    assert_eq!(missing.cache_write_tokens, 300);
    assert_eq!(
        missing.measurement_states["cache_write_1h_tokens"],
        json!({"state":"unread"})
    );
    assert!(
        missing.credits.is_none(),
        "a known aggregate does not identify the earlier unreported subset"
    );
}

#[path = "usage_ledger/service_units.rs"]
mod service_units;

#[path = "usage_ledger/cache_write_lifetimes.rs"]
mod cache_write_lifetimes;

#[tokio::test]
async fn a_paid_model_keeps_its_frozen_org_and_project_after_the_topic_moves() {
    use orch_api_contract::usage::{RequestTiming, UsageMeasurement};
    use orch_db::queries::hierarchy::{self, testing::insert_node};
    use orch_db::usage::ledger::{SummaryAttribution, Tokens, summary_place};
    use orch_engine::turns::{OwnPurpose, OwnUse};
    let world = World::new().await;
    let mut conn = world.pool.acquire().await.unwrap();
    let frozen = SummaryAttribution {
        org_id: world.root,
        project_id: world.project,
        topic_id: world.topic,
        thread_id: None,
        provider_id: world.provider,
        entry_id: None,
        user_id: Some(world.ada),
    };
    assert_eq!(
        summary_place(&mut conn, world.topic).await.unwrap(),
        Some((frozen.org_id, frozen.project_id))
    );
    let other_project = orch_types::new_id();
    insert_node(
        &mut conn,
        other_project,
        "project",
        Some(world.root),
        "Other billing project",
        world.ada,
    )
    .await
    .unwrap();
    hierarchy::move_under(&mut conn, world.topic, other_project, world.clock.now())
        .await
        .unwrap();
    assert_eq!(
        summary_place(&mut conn, world.topic).await.unwrap(),
        Some((world.root, other_project)),
        "the native hierarchy really changed before completion accounting"
    );
    let timing = RequestTiming {
        outcome: UsageMeasurement::Told {
            value: "done".into(),
        },
        ..RequestTiming::default()
    };
    let own = OwnUse {
        purpose: OwnPurpose::Summary,
        timing: Some(&timing),
        id: orch_types::new_id(),
        at: world.clock.now(),
        topic: frozen.topic_id,
        thread: frozen.thread_id,
        provider: frozen.provider_id,
        entry: frozen.entry_id,
        person: frozen.user_id,
        model: MODEL,
        protocol: None,
        tokens: Tokens {
            input: 9,
            output: 3,
            total: 12,
            ..Tokens::default()
        },
        request_ref: None,
    };
    assert!(
        world
            .engine()
            .count_own_use_frozen_context(&mut conn, &own, None, &frozen)
            .await
            .unwrap()
    );
    assert!(
        !world
            .engine()
            .count_own_use_frozen_context(&mut conn, &own, None, &frozen)
            .await
            .unwrap(),
        "reconciliation of the same measured native command remains once-only"
    );
    let row:(Id,Id,Id,Id,Option<Id>,i64,i64)=sqlx::query_as("SELECT org_id,project_id,topic_id,provider_id,user_id,input_tokens,output_tokens FROM usage_events WHERE id=$1")
        .bind(own.id).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        row,
        (
            frozen.org_id,
            frozen.project_id,
            frozen.topic_id,
            frozen.provider_id,
            frozen.user_id,
            9,
            3
        )
    );
    assert_ne!(
        row.1, other_project,
        "a moved source does not transfer already measured costs to its new project"
    );
}
