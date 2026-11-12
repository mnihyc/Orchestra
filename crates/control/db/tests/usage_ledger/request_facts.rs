use super::*;
use orch_api_contract::usage::{RequestTiming, UsageMeasurement};

#[tokio::test]
async fn lease_sources_make_one_counted_request_in_either_delivery_order_and_rebuild_identically() {
    for harness_first in [false, true] {
        let (_db, pool) = world().await;
        let mut tx = pool.pool().begin().await.unwrap();
        let place = chain(&mut tx, true).await;
        let attribution = attribution_of_lease(&mut tx, place.lease)
            .await
            .unwrap()
            .unwrap();
        let forwarded = a_request(new_id(), &attribution, used(100, 20, 10), true);
        let mut harness = a_request(new_id(), &attribution, used(500, 0, 40), false);
        harness.origin = ledger::origin::HARNESS;
        harness.request_ref = Some("adapter-request-1");
        let upstream = RequestTiming {
            provider_request_id: Some("resp_1".into()),
            upstream_status: Some(403),
            error_reason: Some("cyber".into()),
            attempt: Some(2),
            outcome: UsageMeasurement::Told {
                value: "model_refused".into(),
            },
            ..Default::default()
        };
        let adapter = RequestTiming {
            provider_request_id: Some("resp_1".into()),
            first_token_ms: UsageMeasurement::Told { value: 150 },
            outcome: UsageMeasurement::Told {
                value: "harness_error".into(),
            },
            ..Default::default()
        };
        let sources = if harness_first {
            [(&harness, &adapter), (&forwarded, &upstream)]
        } else {
            [(&forwarded, &upstream), (&harness, &adapter)]
        };
        for _ in 0..2 {
            // Replay both source reports as well as their original deliveries.
            for (event, timing) in sources {
                ledger::record_lease_program(&mut tx, event, Some(timing), None, place.lease)
                    .await
                    .unwrap();
            }
        }
        let rows = events_of_turn(&mut tx, place.turn).await.unwrap();
        assert_eq!(rows.len(), 1, "delivery order {harness_first}: {rows:?}");
        let row = &rows[0];
        assert!(row.counted);
        assert_eq!(row.origin, "forwarder");
        assert_eq!(
            (row.input_tokens, row.cached_input_tokens, row.output_tokens),
            (100, 20, 10)
        );
        assert_eq!(row.outcome.as_deref(), Some("model_refused"));
        assert_eq!(row.error_reason.as_deref(), Some("cyber"));
        assert_eq!(row.upstream_status, Some(403));
        assert_eq!(row.attempt, 2);
        assert_eq!(row.first_token_ms, Some(150));
        assert_eq!(row.thread_owner_id, Some(place.person));
        assert_eq!(
            row.measurement_states["_adapter_request_ref"],
            "adapter-request-1"
        );
        let live: Vec<Json> = sqlx::query_scalar("SELECT to_jsonb(d) FROM usage_daily d")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            live.iter()
                .map(|row| row["requests"].as_i64().unwrap())
                .sum::<i64>(),
            1
        );
        let months: Vec<Json> = sqlx::query_scalar("SELECT to_jsonb(d) FROM usage_monthly d")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        orch_db::usage::rollup::rebuild(&mut tx, None)
            .await
            .unwrap();
        let rebuilt: Vec<Json> = sqlx::query_scalar("SELECT to_jsonb(d) FROM usage_daily d")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        let rebuilt_months: Vec<Json> =
            sqlx::query_scalar("SELECT to_jsonb(d) FROM usage_monthly d")
                .fetch_all(&mut *tx)
                .await
                .unwrap();
        assert_eq!(live, rebuilt);
        assert_eq!(months, rebuilt_months);
    }
}

#[tokio::test]
async fn a_failed_request_without_usage_keeps_status_attempt_and_captured_owner() {
    let (_db, pool) = world().await;
    let mut tx = pool.pool().begin().await.unwrap();
    let place = chain(&mut tx, true).await;
    let attribution = attribution_of_lease(&mut tx, place.lease)
        .await
        .unwrap()
        .unwrap();
    let event = a_request(new_id(), &attribution, Tokens::default(), true);
    let timing = RequestTiming {
        outcome: UsageMeasurement::Told {
            value: "rate_limited".into(),
        },
        upstream_status: Some(429),
        attempt: Some(3),
        ..Default::default()
    };
    ledger::record_lease_program(&mut tx, &event, Some(&timing), None, place.lease)
        .await
        .unwrap();
    let rows = events_of_turn(&mut tx, place.turn).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].total_tokens, 0);
    assert!(
        rows[0].timing().reported_tokens.is_none(),
        "zero ledger storage must not invent a vendor count"
    );
    assert_eq!(rows[0].upstream_status, Some(429));
    assert_eq!(rows[0].attempt, 3);
    assert_eq!(
        ledger::attempt_after(&mut tx, place.turn, place.provider, "resp_1")
            .await
            .unwrap(),
        Some(4)
    );
    assert_eq!(rows[0].thread_owner_id, Some(place.person));
}

#[tokio::test]
async fn explicit_tool_items_supply_counts_and_time_without_borrowing_a_concurrent_child() {
    let (_db, pool) = world().await;
    let mut tx = pool.pool().begin().await.unwrap();
    let place = chain(&mut tx, true).await;
    let attribution = attribution_of_lease(&mut tx, place.lease)
        .await
        .unwrap()
        .unwrap();
    let event = a_request(new_id(), &attribution, used(100, 0, 20), true);
    let timing = RequestTiming {
        outcome: UsageMeasurement::Told {
            value: "done".into(),
        },
        ..Default::default()
    };
    ledger::record_lease_program(&mut tx, &event, Some(&timing), None, place.lease)
        .await
        .unwrap();
    let unknown = events_of_turn(&mut tx, place.turn).await.unwrap();
    assert_eq!((unknown[0].tool_calls, unknown[0].tool_ms), (None, None));
    let refs = vec!["tool-first".to_owned(), "tool-second".to_owned()];
    ledger::tools::record(&mut tx, place.turn, place.provider, "resp_1", None, &refs)
        .await
        .unwrap();
    let item = |reference: &str, duration: Option<i64>| {
        (
            reference.to_owned(),
            json!({"title":"Probe","duration_ms":duration}),
        )
    };
    let mut ids = Vec::new();
    for (reference, payload) in [
        item("tool-first", Some(125)),
        item("tool-second", None),
        item("unrelated-child", Some(9000)),
    ] {
        let row = threads::insert_item(
            &mut tx,
            &threads::NewItem {
                id: new_id(),
                thread_id: place.thread,
                segment_id: place.segment,
                turn_id: Some(place.turn),
                stream: "main",
                parent_item_id: None,
                kind: "tool_call",
                status: if reference == "tool-second" {
                    "in_progress"
                } else {
                    "completed"
                },
                author_id: None,
                payload: &payload,
                native_ref: Some(&reference),
                search_text: None,
                at: Utc::now(),
            },
            10,
        )
        .await
        .unwrap()
        .unwrap();
        ids.push(row.id);
    }
    let rows = events_of_turn(&mut tx, place.turn).await.unwrap();
    assert_eq!((rows[0].tool_calls, rows[0].tool_ms), (Some(2), None));
    threads::change_item(
        &mut tx,
        ids[1],
        &threads::ItemChange {
            status: Some("completed"),
            payload: &json!({"title":"Probe","duration_ms":275}),
            search_text: None,
            at: Utc::now(),
        },
        11,
    )
    .await
    .unwrap();
    let rows = events_of_turn(&mut tx, place.turn).await.unwrap();
    assert_eq!((rows[0].tool_calls, rows[0].tool_ms), (Some(2), Some(400)));
    ledger::tools::record(&mut tx, place.turn, place.provider, "resp_1", None, &refs)
        .await
        .unwrap();
    let rows = events_of_turn(&mut tx, place.turn).await.unwrap();
    assert_eq!(
        (rows[0].tool_calls, rows[0].tool_ms),
        (Some(2), Some(400)),
        "replay never adds a tool twice"
    );
}

#[tokio::test]
async fn a_reported_failure_replaces_only_its_exact_turn_repaired_provisional_record() {
    let (_db, pool) = world().await;
    let mut tx = pool.pool().begin().await.unwrap();
    let place = chain(&mut tx, true).await;
    let attribution = attribution_of_lease(&mut tx, place.lease)
        .await
        .unwrap()
        .unwrap();
    let mut provisional = a_request(new_id(), &attribution, Tokens::default(), false);
    provisional.origin = ledger::origin::HARNESS;
    let running = RequestTiming {
        outcome: UsageMeasurement::Unread,
        ..Default::default()
    };
    ledger::record_lease_program(&mut tx, &provisional, Some(&running), None, place.lease)
        .await
        .unwrap();
    ledger::finish_running(&mut tx, place.turn, "harness_error", None, false)
        .await
        .unwrap();
    assert_eq!(events_of_turn(&mut tx, place.turn).await.unwrap().len(), 1);
    ledger::supersede_observation(&mut tx, place.turn, place.provider, "resp_1")
        .await
        .unwrap();
    let mut reported = provisional.clone();
    reported.id = new_id();
    reported.request_ref = Some("native-session:native-turn:failure:1");
    let timing = RequestTiming {
        outcome: UsageMeasurement::Told {
            value: "overloaded".into(),
        },
        upstream_status: Some(503),
        ..Default::default()
    };
    ledger::record_lease_program(&mut tx, &reported, Some(&timing), None, place.lease)
        .await
        .unwrap();
    let rows = events_of_turn(&mut tx, place.turn).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, reported.id);
    let count: i64 =
        sqlx::query_scalar("SELECT COALESCE(sum(requests),0)::bigint FROM usage_daily")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let live: Vec<Json> = sqlx::query_scalar("SELECT to_jsonb(d) FROM usage_daily d")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    orch_db::usage::rollup::rebuild(&mut tx, None)
        .await
        .unwrap();
    let rebuilt: Vec<Json> = sqlx::query_scalar("SELECT to_jsonb(d) FROM usage_daily d")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(live, rebuilt);
}
