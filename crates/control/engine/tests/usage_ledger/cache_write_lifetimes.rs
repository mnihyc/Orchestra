//! Actual cache-write lifetimes retain their catalogue quote and exact residual charge.
use super::*;

fn catalogue() -> Json {
    serde_json::from_str(include_str!("../../../../../deploy/model-rates.json")).unwrap()
}
fn cache_report(lease: Id, name: &str, counts: Json, twelve_hour: Option<i64>) -> Event {
    let mut timing = json!({"wire_protocol":"openai-responses",
        "reported_service_tier":{"state":"told","value":"default"}});
    if let Some(value) = twelve_hour {
        timing["cache_write_12h_tokens"] = json!({"state":"told","value":value});
    }
    Event::decode(
        "lease.usage",
        json!({"lease_id":lease,"model":MODEL,"request_ref":name,
        "usage":counts,"timing":timing}),
    )
    .unwrap()
}

#[test]
fn shipped_cache_quotes_cover_default_extended_and_long_context_writes() {
    let catalogue = catalogue();
    assert_eq!(catalogue["as_of"], "2026-11-08");
    for (family, short, long, above_short, above_long) in [
        ("gpt-6-astra*", "12.5", "25", "25", "50"),
        ("gpt-6.1-sol*", "2.5", "5", "5", "10"),
        ("gpt-6.1-terra*", "1.25", "2.5", "2.5", "5"),
        ("gpt-6-luna*", "0.125", "0.25", "0.25", "0.5"),
        ("gpt-6-sol*", "2.5", "5", "5", "10"),
        ("gpt-5.6-sol*", "5", "10", "10", "20"),
        ("gpt-5.6-terra*", "2.5", "5", "5", "10"),
        ("gpt-5.6-luna*", "0.25", "0.5", "0.5", "1"),
    ] {
        let row = catalogue["families"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["match"] == family)
            .unwrap();
        assert_eq!(row["as_of"], "2026-11-08");
        assert_eq!(row["rate"]["cache_write_per_mtok"], short);
        assert_eq!(row["rate"]["cache_write_12h_per_mtok"], long);
        assert_eq!(row["rate"]["above"][0]["cache_write_per_mtok"], above_short);
        assert_eq!(
            row["rate"]["above"][0]["cache_write_12h_per_mtok"],
            above_long
        );
    }
    let rows = catalogue["families"].as_array().unwrap();
    for (family, cached) in [("gpt-6.1-sol*", "0.1"), ("gpt-6.1-terra*", "0.05")] {
        assert_eq!(
            rows.iter().find(|row| row["match"] == family).unwrap()["rate"]["cached_input_per_mtok"],
            cached
        );
    }
    assert!(
        rows.iter().find(|row| row["match"] == "gpt-5.5*").unwrap()["rate"]
            .get("cache_write_per_mtok")
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_cache_write_report_uses_the_shipped_default_premium_and_freezes_its_quote() {
    let (world, _, turn, lease) = raw_tier_world(None).await;
    let mut catalogue = catalogue();
    let mut family = catalogue["families"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["match"] == "gpt-6-sol*")
        .unwrap()
        .clone();
    // The native fake model uses the shipped quote without contacting its real vendor.
    family["match"] = json!(MODEL);
    catalogue["families"] = json!([family]);
    world.set(world.root, keys::USAGE_RATES, json!({})).await;
    world
        .set(world.root, keys::USAGE_CATALOGUE, json!(true))
        .await;
    world
        .set(world.root, keys::USAGE_CATALOGUE_SCALE, json!("1"))
        .await;
    sqlx::query("UPDATE provider_models SET parameters=parameters-'rate'")
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO rate_catalogue(id,doc,as_of,origin) VALUES(true,$1,'2026-11-08','shipped') ON CONFLICT(id) DO UPDATE SET doc=EXCLUDED.doc,as_of=EXCLUDED.as_of,origin=EXCLUDED.origin")
        .bind(catalogue).execute(world.pool.pool()).await.unwrap();
    let report = cache_report(
        lease,
        "default-cache-write",
        raw_tier_tokens(1000, 0, 1000, 0),
        Some(0),
    );
    world.say_of_provider_taken_in(report.clone()).await;
    let rows = world.wait_ledger(turn, 1).await;
    assert_eq!(rows[0].cache_write_tokens, 1000);
    assert_eq!(rows[0].credits.as_deref(), Some("0.002500"));
    assert_eq!(rows[0].rate_source.as_deref(), Some("catalogue"));
    assert_eq!(
        rows[0].rate.as_ref().unwrap()["cache_write_per_mtok"],
        "2.5"
    );
    assert_eq!(
        rows[0].rate.as_ref().unwrap()["cache_write_12h_per_mtok"],
        "5"
    );
    give_model_a_rate(&world, &a_rate("99", "99", "99")).await;
    world.say_of_provider_taken_in(report).await;
    assert_eq!(world.ledger_of(turn).await, rows);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mixed_cache_write_lifetimes_close_only_the_unreported_request() {
    let (world, thread, turn, lease) = raw_tier_world(None).await;
    give_model_a_rate(
        &world,
        &json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1",
        "output_per_mtok":"2","cache_write_per_mtok":"1.25","cache_write_12h_per_mtok":"2.5"}),
    )
    .await;
    let report = cache_report(
        lease,
        "cache-before-close",
        raw_tier_tokens(1000, 0, 300, 0),
        Some(100),
    );
    world.say_of_provider_taken_in(report.clone()).await;
    assert_eq!(
        world.wait_ledger(turn, 1).await[0].credits.as_deref(),
        Some("0.001200")
    );
    let snapshot = json!({"requests":2,"usage":raw_tier_tokens(2000,0,600,0),"usage_groups":[{
        "model":MODEL,"usage":raw_tier_tokens(2000,0,600,0),"requests":2,
        "min_context_tokens":1000,"max_context_tokens":1000,
        "cache_write_usage":{"one_hour_tokens":0,"known_requests":0,"twelve_hour_tokens":300,"twelve_hour_known_requests":2},
        "timing":{"wire_protocol":"openai-responses","reported_service_tier":{"state":"told","value":"default"},
            "cache_write_12h_tokens":{"state":"told","value":100}}
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
    assert_eq!(residual.cache_write_tokens, 300);
    assert_eq!(
        residual.measurement_states["cache_write_12h_tokens"],
        json!({"state":"told","value":200})
    );
    assert_eq!(residual.credits.as_deref(), Some("0.001325"));
    let total: String = sqlx::query_scalar(
        "SELECT sum(credits)::text FROM usage_events WHERE turn_id=$1 AND counted",
    )
    .bind(turn)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(total, "0.002525");
    world.say_of_provider_taken_in(report).await;
    assert_eq!(world.ledger_of(turn).await, rows);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_cache_lifetime_measurements_are_not_reconstructed_from_close_totals() {
    let (world, thread, turn, lease) = raw_tier_world(None).await;
    give_model_a_rate(
        &world,
        &json!({"input_per_mtok":"1","cached_input_per_mtok":"0.1",
        "output_per_mtok":"2","cache_write_per_mtok":"1.25","cache_write_12h_per_mtok":"2.5"}),
    )
    .await;
    world
        .say_of_provider_taken_in(cache_report(
            lease,
            "unknown-cache-before-close",
            raw_tier_tokens(1000, 0, 300, 0),
            None,
        ))
        .await;
    assert!(world.wait_ledger(turn, 1).await[0].credits.is_none());
    let snapshot = json!({"requests":2,"usage":raw_tier_tokens(2000,0,600,0),"usage_groups":[{
        "model":MODEL,"usage":raw_tier_tokens(2000,0,600,0),"requests":2,
        "min_context_tokens":1000,"max_context_tokens":1000,
        "cache_write_usage":{"one_hour_tokens":0,"known_requests":0,"twelve_hour_tokens":300,"twelve_hour_known_requests":2},
        "timing":{"wire_protocol":"openai-responses","reported_service_tier":{"state":"told","value":"default"}}
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
    assert_eq!(residual.cache_write_tokens, 300);
    assert_eq!(
        residual.measurement_states["cache_write_12h_tokens"],
        json!({"state":"unread"})
    );
    assert!(residual.credits.is_none());
}
