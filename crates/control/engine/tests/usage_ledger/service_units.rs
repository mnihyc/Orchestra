//! Real normalized ledger/rollup/cap consumers; transport and current authority remain the executor's.
use super::*;
use orch_api_contract::usage::{RequestTiming, UsageDimension, UsageMeasurement};
use orch_engine::caps::{self, ServiceCapAsk};
use orch_engine::rates::Source;
use orch_engine::service_usage::{ServiceSource, ServiceUse};
use orch_provider_contract::capabilities::Price;
use orch_types::new_id;

fn quote(amount: &str) -> Price {
    Price {
        amount: amount.into(),
        currency: "credits".into(),
        unit: "call".into(),
        ..Default::default()
    }
}
fn done() -> RequestTiming {
    RequestTiming {
        outcome: UsageMeasurement::Told {
            value: "done".into(),
        },
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_service_units_keep_native_turn_sources_a_copied_quote_and_one_ledger_identity() {
    let (world, thread, turn, _) = raw_tier_world(None).await;
    let mut tx = world.pool.begin().await.unwrap();
    let original = orch_db::usage::ledger::attribution_of_turn(&mut tx, turn)
        .await
        .unwrap()
        .unwrap();
    let unit_price = quote("0.00075");
    let timing = done();
    let groups = vec!["lookup".into()];
    let used = ServiceUse {
        id: new_id(),
        at: world.clock.now(),
        source: ServiceSource::Turn(&original),
        provider: world.provider,
        entry: None,
        person: Some(world.ada),
        purpose: "web_search",
        offering: r"lookup\exact",
        groups: &groups,
        unit: "call",
        quantity: UsageMeasurement::Told { value: 2 },
        quote: Some(&unit_price),
        rate_source: Source::Provider,
        timing: &timing,
        request_ref: Some("native-service-request"),
    };
    let cap_id = new_id();
    sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,services,purposes,created_by) VALUES($1,$2,'members',$3,'each','credits','week',0.0015,ARRAY['@lookup'],ARRAY['web_search'],$4)")
        .bind(cap_id).bind(world.project).bind(world.root).bind(world.ada).execute(&mut *tx).await.unwrap();
    let ask = ServiceCapAsk {
        place: thread,
        provider: world.provider,
        offering: used.offering,
        groups: &groups,
        purpose: used.purpose,
        person: Some(world.ada),
        priced: true,
        harness_kind: original.harness_kind.as_deref(),
    };
    let engine = world.engine();
    assert!(engine.room_for_service_use(&mut tx, &ask).await.unwrap());
    assert!(engine.count_service_use(&mut tx, &used).await.unwrap());
    assert!(!engine.room_for_service_use(&mut tx, &ask).await.unwrap());
    let edited = quote("999");
    assert!(
        !engine
            .count_service_use(
                &mut tx,
                &ServiceUse {
                    quote: Some(&edited),
                    ..used
                }
            )
            .await
            .unwrap()
    );
    let row = orch_db::usage::ledger::events_of_turn(&mut tx, turn)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == used.id)
        .unwrap();
    assert_eq!(row.origin, "orch");
    assert_eq!(row.purpose, "web_search");
    assert_eq!(row.model, "");
    assert_eq!(row.thread_id, Some(thread));
    assert_eq!(row.segment_id, Some(original.segment_id));
    assert_eq!(row.turn_id, Some(turn));
    assert_eq!(row.provider_id, world.provider);
    assert_eq!(row.user_id, Some(world.ada));
    assert_eq!(row.credits.as_deref(), Some("0.001500"));
    assert_eq!(row.total_tokens, 0);
    let measured = row.timing();
    let service = measured.service_usage.unwrap();
    assert_eq!(service.offering, r"lookup\exact");
    assert_eq!(service.unit, "call");
    assert_eq!(service.quantity.value(), Some(&2));
    assert_eq!(service.price.unwrap().amount, "0.00075");
    assert_eq!(service.credits_per_unit.as_deref(), Some("0.00075"));
    assert_eq!(row.measurement_states["_service_groups"], json!(["lookup"]));
    assert!(
        serde_json::to_value(row.timing()).unwrap()["service_usage"]
            .get("groups")
            .is_none(),
        "private group facts never enter the public timing contract"
    );
    let daily:(String,i64,Json)=sqlx::query_as("SELECT credits::text,unrated_services,service_units FROM usage_daily WHERE service->>'name'=$1")
        .bind(used.offering).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(
        daily,
        (
            "0.001500".into(),
            0,
            json!({"call":{"quantity":2,"known_requests":1,"unknown_requests":0}})
        )
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
        "0.0015"
    );
    assert!(
        !caps::covers_model(&cap, MODEL, &groups),
        "an explicit service selection grants no model budget"
    );
    let read = orch_db::usage::rollup::Read {
        from: world.clock.now().date_naive(),
        to: world.clock.now().date_naive(),
        bucket: None,
        by: Some(UsageDimension::Offering),
        under: None,
        filter: &orch_db::usage::rollup::Filter {
            purpose: Some("web_search".into()),
            ..Default::default()
        },
    };
    let cells = orch_db::usage::rollup::read(&mut tx, &read).await.unwrap();
    assert_eq!(cells.len(), 1);
    let key: Json = serde_json::from_str(cells[0].key.as_deref().unwrap()).unwrap();
    assert_eq!(key, json!({"kind":"service","name":used.offering}));
    assert_eq!(cells[0].figures.turns, 1);
    let before: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(d) FROM usage_daily d ORDER BY day,model,service_key")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    let months: Vec<Json> = sqlx::query_scalar(
        "SELECT to_jsonb(d) FROM usage_monthly d ORDER BY month,model,service_key",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    orch_db::usage::rollup::rebuild(&mut tx, None)
        .await
        .unwrap();
    assert_eq!(
        before,
        sqlx::query_scalar::<_, Json>(
            "SELECT to_jsonb(d) FROM usage_daily d ORDER BY day,model,service_key"
        )
        .fetch_all(&mut *tx)
        .await
        .unwrap()
    );
    assert_eq!(
        months,
        sqlx::query_scalar::<_, Json>(
            "SELECT to_jsonb(d) FROM usage_monthly d ORDER BY month,model,service_key"
        )
        .fetch_all(&mut *tx)
        .await
        .unwrap()
    );
    tx.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_service_quantities_prices_and_currencies_remain_separate_from_measured_zero() {
    let (world, thread, turn, _) = raw_tier_world(None).await;
    let mut tx = world.pool.begin().await.unwrap();
    let original = orch_db::usage::ledger::attribution_of_turn(&mut tx, turn)
        .await
        .unwrap()
        .unwrap();
    let timing = done();
    let quote = quote("0.00075");
    let foreign = Price {
        currency: "USD".into(),
        ..quote.clone()
    };
    let groups = vec!["lookup".into()];
    let base = ServiceUse {
        id: new_id(),
        at: world.clock.now(),
        source: ServiceSource::Turn(&original),
        provider: world.provider,
        entry: None,
        person: Some(world.ada),
        purpose: "web_search",
        offering: "known-source",
        groups: &groups,
        unit: "call",
        quantity: UsageMeasurement::Told { value: 0 },
        quote: Some(&quote),
        rate_source: Source::Provider,
        timing: &timing,
        request_ref: None,
    };
    let engine = world.engine();
    assert!(engine.count_service_use(&mut tx, &base).await.unwrap());
    for (quantity, price) in [
        (UsageMeasurement::Empty, Some(&quote)),
        (UsageMeasurement::Told { value: 2 }, None),
        (UsageMeasurement::Told { value: 2 }, Some(&foreign)),
        (UsageMeasurement::Told { value: -1 }, Some(&quote)),
    ] {
        assert!(
            engine
                .count_service_use(
                    &mut tx,
                    &ServiceUse {
                        id: new_id(),
                        quantity,
                        quote: price,
                        ..base
                    }
                )
                .await
                .unwrap()
        );
    }
    let totals:(String,i64,Json)=sqlx::query_as("SELECT sum(credits)::text,sum(unrated_services)::bigint,usage_merge_unit_counts(service_units) FROM usage_daily WHERE service->>'name'='known-source'")
        .fetch_one(&mut *tx).await.unwrap();
    assert_eq!(
        totals,
        (
            "0.000000".into(),
            4,
            json!({"call":{"quantity":4,"known_requests":3,"unknown_requests":2}})
        )
    );
    let quote_units:Vec<Option<String>>=sqlx::query_scalar("SELECT measurement_states#>>'{service_usage,price,currency}' FROM usage_events WHERE purpose='web_search' AND credits IS NULL")
        .fetch_all(&mut *tx).await.unwrap();
    assert!(
        quote_units
            .iter()
            .any(|unit| unit.as_deref() == Some("USD")),
        "a foreign currency is retained, never silently converted"
    );
    let id = new_id();
    sqlx::query("INSERT INTO usage_caps(id,node_id,meter,period,amount,services,purposes,uncounted,created_by) VALUES($1,$2,'credits','week',1,ARRAY['known-source'],ARRAY['web_search'],'block',$3)")
        .bind(id).bind(world.project).bind(world.ada).execute(&mut *tx).await.unwrap();
    let ask = ServiceCapAsk {
        place: thread,
        provider: world.provider,
        offering: base.offering,
        groups: &groups,
        purpose: base.purpose,
        person: Some(world.ada),
        priced: false,
        harness_kind: original.harness_kind.as_deref(),
    };
    assert!(
        !engine.room_for_service_use(&mut tx, &ask).await.unwrap(),
        "the original uncounted:block refuses an unpriced new unit"
    );
    let cap = orch_db::usage::caps::get(&mut tx, id)
        .await
        .unwrap()
        .unwrap();
    let standing = caps::standing(&mut tx, &cap, None, world.clock.now())
        .await
        .unwrap();
    assert_eq!(standing.unrated_services, 4);
    assert_eq!(standing.unrated_tokens, 0);
    assert_eq!(
        standing.state(),
        caps::State::Ok,
        "unknown spend is no fabricated reached amount"
    );
    let bad = RequestTiming {
        outcome: UsageMeasurement::Unread,
        ..Default::default()
    };
    assert!(
        engine
            .count_service_use(
                &mut tx,
                &ServiceUse {
                    id: new_id(),
                    timing: &bad,
                    ..base
                }
            )
            .await
            .is_err()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn service_identity_keys_preserve_backslashes_unicode_and_wide_private_group_facts() {
    let world = World::new().await;
    let mut conn = world.pool.acquire().await.unwrap();
    let groups = (0..100)
        .map(|i| format!("scope-{i:03}-a-long-recorded-group-name"))
        .collect::<Vec<_>>();
    let value = json!({"name":r"搜索\exact","groups":groups});
    let a: Vec<u8> = sqlx::query_scalar("SELECT usage_service_key($1)")
        .bind(&value)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let b: Vec<u8> = sqlx::query_scalar(
        "SELECT usage_service_key(jsonb_build_object('groups',$1::jsonb,'name',$2::text))",
    )
    .bind(&value["groups"])
    .bind(value["name"].as_str().unwrap())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(a, b);
    assert_eq!(
        a.len(),
        32,
        "the canonical SHA-256 key has fixed width, independently of the recorded facts"
    );
    let mut changed = value.clone();
    changed["name"] = json!(r"搜索\other");
    let other: Vec<u8> = sqlx::query_scalar("SELECT usage_service_key($1)")
        .bind(changed)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_ne!(a, other);
}

#[tokio::test]
async fn standalone_service_units_keep_no_turn_and_round_only_the_complete_row() {
    let world = World::new().await;
    let engine = world.engine();
    let mut tx = world.pool.begin().await.unwrap();
    let source = orch_db::usage::ledger::SummaryAttribution {
        org_id: world.root,
        project_id: world.project,
        topic_id: world.topic,
        thread_id: None,
        provider_id: world.provider,
        entry_id: None,
        user_id: None,
    };
    let price = Price {
        amount: "0.0000005".into(),
        currency: "credits".into(),
        unit: "image".into(),
        ..Default::default()
    };
    let timing = done();
    let used = ServiceUse {
        id: new_id(),
        at: world.clock.now(),
        source: ServiceSource::Standalone(&source),
        provider: world.provider,
        entry: None,
        person: None,
        purpose: "image_generation",
        offering: "image-service",
        groups: &[],
        unit: "image",
        quantity: UsageMeasurement::Told { value: 3 },
        quote: Some(&price),
        rate_source: Source::Provider,
        timing: &timing,
        request_ref: None,
    };
    assert!(engine.count_service_use(&mut tx, &used).await.unwrap());
    assert!(!engine.count_service_use(&mut tx, &used).await.unwrap());
    let row:(Option<Id>,Option<Id>,Option<Id>,String,Json)=sqlx::query_as("SELECT turn_id,segment_id,user_id,credits::text,measurement_states FROM usage_events WHERE id=$1")
        .bind(used.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!((row.0, row.1, row.2), (None, None, None));
    assert_eq!(row.3, "0.000002");
    assert_eq!(row.4["service_usage"]["credits_per_unit"], "0.0000005");
    let units: Json = sqlx::query_scalar(
        "SELECT service_units FROM usage_daily WHERE service->>'name'='image-service'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        units,
        json!({"image":{"quantity":3,"known_requests":1,"unknown_requests":0}})
    );
    let zero: Json = sqlx::query_scalar("SELECT usage_unit_counts_adjust($1,$2,-1)")
        .bind(json!({"image":{"quantity":3,"known_requests":2,"unknown_requests":0}}))
        .bind(json!({"image":{"quantity":3,"known_requests":1,"unknown_requests":0}}))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        zero,
        json!({"image":{"quantity":0,"known_requests":1,"unknown_requests":0}}),
        "a remaining explicit zero is still a known observation"
    );
    let empty: Json = sqlx::query_scalar("SELECT usage_unit_counts_adjust($1,$1,-1)")
        .bind(&zero)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        empty,
        json!({}),
        "removing the last real observation is absence, not a measured zero"
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn service_catalogue_quotes_use_only_the_existing_matching_unit_switch_and_scale() {
    let world = World::new().await;
    world.set_system("usage.catalogue_scale", json!("2")).await;
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO rate_catalogue(id,doc,as_of,origin) VALUES(true,$1,'2026-11-08','shipped') ON CONFLICT(id) DO UPDATE SET doc=EXCLUDED.doc")
        .bind(json!({"format":1,"unit":"USD","families":[]})).execute(&mut *tx).await.unwrap();
    let source = orch_db::usage::ledger::SummaryAttribution {
        org_id: world.root,
        project_id: world.project,
        topic_id: world.topic,
        thread_id: None,
        provider_id: world.provider,
        entry_id: None,
        user_id: None,
    };
    let price = Price {
        amount: "0.00075".into(),
        currency: "USD".into(),
        unit: "call".into(),
        ..Default::default()
    };
    let timing = done();
    let used = ServiceUse {
        id: new_id(),
        at: world.clock.now(),
        source: ServiceSource::Standalone(&source),
        provider: world.provider,
        entry: None,
        person: None,
        purpose: "web_search",
        offering: "catalogue-search",
        groups: &[],
        unit: "call",
        quantity: UsageMeasurement::Told { value: 2 },
        quote: Some(&price),
        rate_source: Source::Catalogue,
        timing: &timing,
        request_ref: None,
    };
    assert!(
        world
            .engine()
            .count_service_use(&mut tx, &used)
            .await
            .unwrap()
    );
    let priced: (String, String, Json) = sqlx::query_as(
        "SELECT credits::text,rate_source,measurement_states FROM usage_events WHERE id=$1",
    )
    .bind(used.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(priced.0, "0.003000");
    assert_eq!(priced.1, "catalogue");
    assert_eq!(priced.2["service_usage"]["price"]["currency"], "USD");
    assert_eq!(priced.2["service_usage"]["credits_per_unit"], "0.00150");
    let foreign = Price {
        currency: "OTHER".into(),
        ..price.clone()
    };
    let next = new_id();
    assert!(
        world
            .engine()
            .count_service_use(
                &mut tx,
                &ServiceUse {
                    id: next,
                    quote: Some(&foreign),
                    ..used
                }
            )
            .await
            .unwrap()
    );
    let credits: Option<String> =
        sqlx::query_scalar("SELECT credits::text FROM usage_events WHERE id=$1")
            .bind(next)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(credits.is_none());
    tx.commit().await.unwrap();
    world.set(world.root, "usage.catalogue", json!(false)).await;
    let mut tx = world.pool.begin().await.unwrap();
    let next = new_id();
    assert!(
        world
            .engine()
            .count_service_use(
                &mut tx,
                &ServiceUse {
                    id: next,
                    quote: Some(&price),
                    quantity: UsageMeasurement::Told { value: 2 },
                    ..used
                }
            )
            .await
            .unwrap()
    );
    let credits: Option<String> =
        sqlx::query_scalar("SELECT credits::text FROM usage_events WHERE id=$1")
            .bind(next)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(
        credits.is_none(),
        "disabled catalogue estimates stay disabled"
    );
    tx.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn historical_service_groups_and_quotes_stay_frozen_across_new_calls_replay_and_rebuild() {
    let (world, thread, turn, _) = raw_tier_world(None).await;
    let mut tx = world.pool.begin().await.unwrap();
    let original = orch_db::usage::ledger::attribution_of_turn(&mut tx, turn)
        .await
        .unwrap()
        .unwrap();
    let old_groups = vec!["before".to_owned()];
    let current_groups = vec!["current".to_owned()];
    let old_price = quote("0.1");
    let current_price = quote("0.3");
    let old_cap = new_id();
    let current_cap = new_id();
    for (id, group, amount) in [(old_cap, "before", "0.2"), (current_cap, "current", "0.3")] {
        sqlx::query("INSERT INTO usage_caps(id,node_id,subject_kind,subject_id,split,meter,period,amount,services,purposes,created_by) VALUES($1,$2,'members',$3,'each','credits','week',$4::text::numeric,$5,ARRAY['web_search'],$6)")
            .bind(id).bind(world.project).bind(world.root).bind(amount).bind(vec![format!("@{group}")]).bind(world.ada).execute(&mut *tx).await.unwrap();
    }
    let timing = done();
    let first = ServiceUse {
        id: new_id(),
        at: world.clock.now(),
        source: ServiceSource::Turn(&original),
        provider: world.provider,
        entry: None,
        person: Some(world.ada),
        purpose: "web_search",
        offering: "stable-search",
        groups: &old_groups,
        unit: "call",
        quantity: UsageMeasurement::Told { value: 2 },
        quote: Some(&old_price),
        rate_source: Source::Provider,
        timing: &timing,
        request_ref: Some("first-native-search"),
    };
    let first_ask = ServiceCapAsk {
        place: thread,
        provider: world.provider,
        offering: first.offering,
        groups: &old_groups,
        purpose: first.purpose,
        person: Some(world.ada),
        priced: true,
        harness_kind: original.harness_kind.as_deref(),
    };
    let engine = world.engine();
    assert!(
        engine
            .room_for_service_use(&mut tx, &first_ask)
            .await
            .unwrap()
    );
    assert!(engine.count_service_use(&mut tx, &first).await.unwrap());
    assert!(
        !engine
            .room_for_service_use(&mut tx, &first_ask)
            .await
            .unwrap()
    );
    let first_before = orch_db::usage::ledger::events_of_turn(&mut tx, turn)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == first.id)
        .unwrap();
    let second = ServiceUse {
        id: new_id(),
        at: world.clock.now(),
        source: ServiceSource::Turn(&original),
        provider: world.provider,
        entry: None,
        person: Some(world.ada),
        purpose: "web_search",
        offering: first.offering,
        groups: &current_groups,
        unit: "call",
        quantity: UsageMeasurement::Told { value: 1 },
        quote: Some(&current_price),
        rate_source: Source::Provider,
        timing: &timing,
        request_ref: Some("next-native-search"),
    };
    let second_ask = ServiceCapAsk {
        groups: &current_groups,
        ..first_ask
    };
    assert!(
        engine
            .room_for_service_use(&mut tx, &second_ask)
            .await
            .unwrap(),
        "the old group's exhausted cap does not widen to another group"
    );
    assert!(engine.count_service_use(&mut tx, &second).await.unwrap());
    assert!(
        !engine
            .room_for_service_use(&mut tx, &second_ask)
            .await
            .unwrap()
    );
    assert!(
        !engine
            .count_service_use(
                &mut tx,
                &ServiceUse {
                    id: first.id,
                    ..second
                }
            )
            .await
            .unwrap(),
        "a repeated native id never changes its recorded group, quantity or price"
    );
    let rows = orch_db::usage::ledger::events_of_turn(&mut tx, turn)
        .await
        .unwrap();
    let retained = rows.iter().find(|row| row.id == first.id).unwrap();
    assert_eq!(retained, &first_before);
    assert_eq!(
        retained.measurement_states["_service_groups"],
        json!(["before"])
    );
    assert_eq!(retained.credits.as_deref(), Some("0.200000"));
    let later = rows.iter().find(|row| row.id == second.id).unwrap();
    assert_eq!(
        later.measurement_states["_service_groups"],
        json!(["current"])
    );
    assert_eq!(later.credits.as_deref(), Some("0.300000"));
    assert_eq!(
        retained
            .timing()
            .service_usage
            .unwrap()
            .price
            .unwrap()
            .amount,
        "0.1"
    );
    assert_eq!(
        later.timing().service_usage.unwrap().price.unwrap().amount,
        "0.3"
    );
    for (id, amount) in [(old_cap, "0.2"), (current_cap, "0.3")] {
        let cap = orch_db::usage::caps::get(&mut tx, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            caps::standing(&mut tx, &cap, Some(world.ada), world.clock.now())
                .await
                .unwrap()
                .used
                .text(),
            amount,
            "each group's cap reads the recorded group facts rather than today's selection"
        );
    }
    let identities: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT service_key) FROM usage_daily WHERE service->>'name'=$1",
    )
    .bind(first.offering)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        identities, 2,
        "private historical membership remains in the derived identity"
    );
    let filter = orch_db::usage::rollup::Filter {
        service: Some(first.offering.into()),
        purpose: Some(first.purpose.into()),
        ..Default::default()
    };
    let read = orch_db::usage::rollup::Read {
        from: world.clock.now().date_naive(),
        to: world.clock.now().date_naive(),
        bucket: None,
        by: Some(UsageDimension::Offering),
        under: None,
        filter: &filter,
    };
    let live = orch_db::usage::rollup::read(&mut tx, &read).await.unwrap();
    assert_eq!(
        live.len(),
        1,
        "public offering totals do not expose group changes as separate offerings"
    );
    assert_eq!(
        serde_json::from_str::<Json>(live[0].key.as_deref().unwrap()).unwrap(),
        json!({"kind":"service","name":first.offering})
    );
    assert_eq!(live[0].figures.credits, "0.500000");
    assert_eq!(
        live[0].figures.turns, 1,
        "two calls of one real turn still count one turn"
    );
    assert_eq!(
        live[0].figures.service_units,
        json!({"call":{"quantity":3,"known_requests":2,"unknown_requests":0}})
    );
    let monthly: Vec<Json> = sqlx::query_scalar(
        "SELECT to_jsonb(m) FROM usage_monthly m WHERE service->>'name'=$1 ORDER BY service_key",
    )
    .bind(first.offering)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(monthly.len(), 2);
    orch_db::usage::rollup::rebuild(&mut tx, None)
        .await
        .unwrap();
    assert_eq!(
        orch_db::usage::rollup::read(&mut tx, &read).await.unwrap(),
        live
    );
    assert_eq!(
        sqlx::query_scalar::<_, Json>(
            "SELECT to_jsonb(m) FROM usage_monthly m WHERE service->>'name'=$1 ORDER BY service_key"
        )
        .bind(first.offering)
        .fetch_all(&mut *tx)
        .await
        .unwrap(),
        monthly
    );
    for (id, amount) in [(old_cap, "0.2"), (current_cap, "0.3")] {
        let cap = orch_db::usage::caps::get(&mut tx, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            caps::standing(&mut tx, &cap, Some(world.ada), world.clock.now())
                .await
                .unwrap()
                .used
                .text(),
            amount
        );
    }
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn reported_service_totals_replace_only_matching_catalogue_estimates_and_keep_units_independent()
 {
    use orch_api_contract::usage::ReportedCost;
    let world = World::new().await;
    world.set_system("usage.catalogue_scale", json!("2")).await;
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO rate_catalogue(id,doc,as_of,origin) VALUES(true,$1,'2026-11-08','shipped') ON CONFLICT(id) DO UPDATE SET doc=EXCLUDED.doc")
        .bind(json!({"format":1,"unit":"USD","families":[]})).execute(&mut *tx).await.unwrap();
    let original = orch_db::usage::ledger::SummaryAttribution {
        org_id: world.root,
        project_id: world.project,
        topic_id: world.topic,
        thread_id: None,
        provider_id: world.provider,
        entry_id: None,
        user_id: None,
    };
    let catalogue = Price {
        amount: "0.00075".into(),
        currency: "USD".into(),
        unit: "call".into(),
        ..Default::default()
    };
    let provider = quote("0.3");
    let card = quote("0.2");
    let foreign = Price {
        currency: "EUR".into(),
        ..catalogue.clone()
    };
    let wrong_charge_unit = Price {
        unit: "page".into(),
        ..catalogue.clone()
    };
    let seed = done();
    let base = ServiceUse {
        id: new_id(),
        at: world.clock.now(),
        source: ServiceSource::Standalone(&original),
        provider: world.provider,
        entry: None,
        person: None,
        purpose: "web_search",
        offering: "reported-service",
        groups: &[],
        unit: "call",
        quantity: UsageMeasurement::Told { value: 2 },
        quote: Some(&catalogue),
        rate_source: Source::Catalogue,
        timing: &seed,
        request_ref: None,
    };
    for (label, quote, source, quantity, unit, amount, credits, provenance, applied) in [
        (
            "actual",
            Some(&catalogue),
            Source::Catalogue,
            Some(2),
            Some("USD"),
            "0.0071",
            Some("0.014200"),
            Some("reported"),
            None,
        ),
        (
            "unknown-quantity",
            Some(&catalogue),
            Source::Catalogue,
            None,
            Some("USD"),
            "0.0071",
            Some("0.014200"),
            Some("reported"),
            None,
        ),
        (
            "zero",
            Some(&catalogue),
            Source::Catalogue,
            Some(2),
            Some("USD"),
            "0",
            Some("0.000000"),
            Some("reported"),
            None,
        ),
        (
            "provider",
            Some(&provider),
            Source::Provider,
            Some(2),
            Some("USD"),
            "0.0071",
            Some("0.600000"),
            Some("provider"),
            Some("0.3"),
        ),
        (
            "card",
            Some(&card),
            Source::RateCard,
            Some(2),
            Some("USD"),
            "0.0071",
            Some("0.400000"),
            Some("rate_card"),
            Some("0.2"),
        ),
        (
            "actual-foreign",
            Some(&catalogue),
            Source::Catalogue,
            Some(2),
            Some("EUR"),
            "0.0071",
            Some("0.003000"),
            Some("catalogue"),
            Some("0.00150"),
        ),
        (
            "actual-unit-missing",
            Some(&catalogue),
            Source::Catalogue,
            Some(2),
            None,
            "0.0071",
            Some("0.003000"),
            Some("catalogue"),
            Some("0.00150"),
        ),
        (
            "negative",
            Some(&catalogue),
            Source::Catalogue,
            Some(2),
            Some("USD"),
            "-1",
            Some("0.003000"),
            Some("catalogue"),
            Some("0.00150"),
        ),
        (
            "quote-foreign",
            Some(&foreign),
            Source::Catalogue,
            Some(2),
            Some("USD"),
            "0.0071",
            None,
            Some("catalogue"),
            None,
        ),
        (
            "charge-unit-mismatch",
            Some(&wrong_charge_unit),
            Source::Catalogue,
            Some(2),
            Some("USD"),
            "0.0071",
            None,
            Some("catalogue"),
            None,
        ),
        (
            "no-quote",
            None,
            Source::Catalogue,
            Some(2),
            Some("USD"),
            "0.0071",
            None,
            None,
            None,
        ),
    ] {
        let id = new_id();
        let timing = RequestTiming {
            reported_cost: UsageMeasurement::Told {
                value: ReportedCost {
                    amount: amount.into(),
                    unit: unit.map(str::to_owned),
                },
            },
            ..done()
        };
        let used = ServiceUse {
            id,
            offering: label,
            quote,
            rate_source: source,
            timing: &timing,
            quantity: quantity.map_or(UsageMeasurement::Empty, |value| UsageMeasurement::Told {
                value,
            }),
            ..base
        };
        assert!(
            world
                .engine()
                .count_service_use(&mut tx, &used)
                .await
                .unwrap(),
            "{label}"
        );
        assert!(
            !world
                .engine()
                .count_service_use(&mut tx, &used)
                .await
                .unwrap(),
            "native replay stays one observation"
        );
        let found: (Option<String>, Option<String>, Json) = sqlx::query_as(
            "SELECT credits::text,rate_source,measurement_states FROM usage_events WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(found.0.as_deref(), credits, "{label}");
        assert_eq!(found.1.as_deref(), provenance, "{label}");
        assert_eq!(
            found.2["reported_cost"],
            serde_json::to_value(&timing.reported_cost).unwrap(),
            "the original total and currency are retained"
        );
        assert_eq!(
            found.2["service_usage"]["credits_per_unit"].as_str(),
            applied,
            "a reported invoice never becomes an invented applied unit rate"
        );
        assert_eq!(
            found.2["service_usage"]["quantity"],
            serde_json::to_value(&used.quantity).unwrap(),
            "cost cannot invent a native count"
        );
        if let Some(quote) = quote {
            assert_eq!(
                found.2["service_usage"]["price"],
                json!({"amount":quote.amount,"currency":quote.currency,"unit":quote.unit})
            );
        }
    }
    tx.commit().await.unwrap();
    world.set(world.root, "usage.catalogue", json!(false)).await;
    let mut tx = world.pool.begin().await.unwrap();
    let timing = RequestTiming {
        reported_cost: UsageMeasurement::Told {
            value: ReportedCost {
                amount: "0.0071".into(),
                unit: Some("USD".into()),
            },
        },
        ..done()
    };
    let used = ServiceUse {
        id: new_id(),
        quantity: UsageMeasurement::Told { value: 2 },
        timing: &timing,
        ..base
    };
    assert!(
        world
            .engine()
            .count_service_use(&mut tx, &used)
            .await
            .unwrap()
    );
    let credits: Option<String> =
        sqlx::query_scalar("SELECT credits::text FROM usage_events WHERE id=$1")
            .bind(used.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(
        credits.is_none(),
        "a report cannot enable catalogue conversion"
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn selected_native_terms_keep_actual_vendor_totals_beside_authoritative_explicit_credits() {
    use orch_db::usage::ledger::Tokens;
    use orch_engine::turns::{OwnPurpose, OwnUse};
    let world = World::new().await;
    world.set_system("usage.catalogue_scale", json!("2")).await;
    world.set(world.root,"usage.rates",json!({"native-card":{"input_per_mtok":"0","cached_input_per_mtok":"0","output_per_mtok":"0","web_search_per_call":"4"}})).await;
    let mut tx = world.pool.begin().await.unwrap();
    sqlx::query("INSERT INTO rate_catalogue(id,doc,as_of,origin) VALUES(true,$1,'2026-11-08','shipped') ON CONFLICT(id) DO UPDATE SET doc=EXCLUDED.doc")
        .bind(json!({"format":1,"unit":"USD","families":[{"match":"native-*","rate":{"input_per_mtok":"0","cached_input_per_mtok":"0","output_per_mtok":"0","web_search_per_call":"99"}}]}))
        .execute(&mut *tx).await.unwrap();
    for (model, parameters) in [
        ("native-catalogue", json!({})),
        (
            "native-own",
            json!({"rate":{"input_per_mtok":"0","cached_input_per_mtok":"0","output_per_mtok":"0","web_search_per_call":"3"}}),
        ),
        ("native-card", json!({})),
        ("native-other-currency", json!({})),
    ] {
        sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) VALUES($1,$2,$3,'Native quote fixture '||$3,$4,true,'manual',$5)")
            .bind(new_id()).bind(world.provider).bind(model).bind(parameters).bind(world.ada).execute(&mut *tx).await.unwrap();
        let other_currency = model == "native-other-currency";
        let mut timing = done();
        timing.web_searches = UsageMeasurement::Told { value: 1 };
        timing.native_prices =
            serde_json::from_value(json!({"web_search":{"currency":"USD","terms":[
                {"amount":"0.005","counter":"web_searches"},
                {"amount":"0.0125","counter":"web_search_results"}
            ]}}))
            .unwrap();
        timing.reported_cost = UsageMeasurement::Told {
            value: orch_api_contract::usage::ReportedCost {
                amount: "0.0071".into(),
                unit: Some(if other_currency { "EUR" } else { "USD" }.into()),
            },
        };
        let own = OwnUse {
            purpose: OwnPurpose::WebSearch,
            timing: Some(&timing),
            id: new_id(),
            at: world.clock.now(),
            topic: world.topic,
            thread: None,
            provider: world.provider,
            entry: None,
            person: Some(world.ada),
            model,
            protocol: None,
            tokens: Tokens::default(),
            request_ref: Some("actual-native-priced-request"),
        };
        assert!(world.engine().count_own_use(&mut tx, &own).await.unwrap());
        assert!(!world.engine().count_own_use(&mut tx, &own).await.unwrap());
        let (credits, source, states): (Option<String>, Option<String>, Json) = sqlx::query_as(
            "SELECT credits::text,rate_source,measurement_states FROM usage_events WHERE id=$1",
        )
        .bind(own.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        let (wanted, authority) = match model {
            "native-own" => (Some("3.000000"), "provider"),
            "native-card" => (Some("4.000000"), "rate_card"),
            "native-other-currency" => (None, "catalogue"),
            _ => (Some("0.014200"), "reported"),
        };
        assert_eq!(credits.as_deref(), wanted, "{model}");
        assert_eq!(source.as_deref(), Some(authority), "{model}");
        assert_eq!(
            states["native_prices"],
            serde_json::to_value(&timing.native_prices).unwrap()
        );
        assert_eq!(
            states["reported_cost"],
            serde_json::to_value(&timing.reported_cost).unwrap()
        );
        assert!(
            !states["declared_counters"]
                .as_object()
                .is_some_and(|counters| counters.contains_key("web_search_results")),
            "no result quantity was invented"
        );
    }
}
