//! Exact vendor credit history follows accepted observations, never cached or administered values.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{DateTime, Duration, Timelike, Utc};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::provider_credits::{self, Read, SeriesRow};
use orch_db::queries::provider_facts::set_sources;
use orch_db::queries::resources::keeper::set_facts;
use orch_db::queries::resources::routes::{
    NewProvider, ProviderChange, change_provider, insert_provider,
};
use orch_db::{Conn, Db};
use orch_testkit::database::TestDb;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::BTreeSet;

struct World {
    _fixture: TestDb,
    db: Db,
    provider: Id,
    at: DateTime<Utc>,
}

impl World {
    async fn new() -> Self {
        let fixture = TestDb::fresh().await;
        let db = Db::from_pool(fixture.pool.clone());
        let at = Utc::now()
            .with_minute(0)
            .unwrap()
            .with_second(10)
            .unwrap()
            .with_nanosecond(0)
            .unwrap()
            - Duration::days(2);
        let (owner, org, provider) = (new_id(), new_id(), new_id());
        let mut conn = db.acquire().await.unwrap();
        insert_user(&mut conn, owner, "credit-observer")
            .await
            .unwrap();
        insert_node(&mut conn, org, "org", None, "Observed provider", owner)
            .await
            .unwrap();
        assert!(
            insert_provider(
                &mut conn,
                &NewProvider {
                    id: provider,
                    name: "Vendor observations",
                    owner_id: owner,
                    home_id: org,
                    upstreams: &json!([{"protocol":"openai-responses","base_url":"https://example.invalid/v1","options":{}}]),
                    protocols: &["openai-responses".into()],
                    machine_id: None,
                    key_machine_id: None,
                    arrangements: &["separated".into()],
                    runtime_config: &json!({}),
                    status: "ready",
                },
                at,
            )
            .await
            .unwrap()
        );
        drop(conn);
        Self {
            _fixture: fixture,
            db,
            provider,
            at,
        }
    }

    async fn write(&self, facts: &Json, at: DateTime<Utc>) {
        set_facts(
            &mut *self.db.acquire().await.unwrap(),
            self.provider,
            facts,
            at,
        )
        .await
        .unwrap();
    }

    async fn rows(&self) -> Vec<SeriesRow> {
        read(
            &mut *self.db.acquire().await.unwrap(),
            self.provider,
            self.at,
            None,
            20,
        )
        .await
    }

    async fn count(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM provider_credit_observations WHERE provider_id=$1")
            .bind(self.provider)
            .fetch_one(self.db.pool())
            .await
            .unwrap()
    }

    async fn revision(&self) -> i64 {
        sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
            .bind(self.provider)
            .fetch_one(self.db.pool())
            .await
            .unwrap()
    }

    async fn source_revision(&self) -> i64 {
        sqlx::query_scalar("SELECT credit_source_revision FROM providers WHERE id=$1")
            .bind(self.provider)
            .fetch_one(self.db.pool())
            .await
            .unwrap()
    }
}

async fn read(
    conn: &mut Conn,
    provider: Id,
    at: DateTime<Utc>,
    after: Option<&str>,
    limit: i64,
) -> Vec<SeriesRow> {
    provider_credits::read(
        conn,
        &Read {
            provider,
            from: at - Duration::minutes(1),
            to: at + Duration::days(1),
            as_of: at + Duration::days(1),
            bucket: "hour",
            after,
            limit,
        },
    )
    .await
    .unwrap()
}

fn balance(at: DateTime<Utc>, amount: Option<&str>, unit: &str, unlimited: bool) -> Json {
    json!({
        "balances":[{"id":"plan","label":"Vendor credits","unit":unit,
            "remaining":amount,"total":null,"used":null,"unlimited":unlimited}],
        "field_times":{"balances":Time::new(at),"balances.plan":Time::new(at)}
    })
}

fn extra(at: DateTime<Utc>, used: Option<&str>, limit: Option<&str>, enabled: bool) -> Json {
    json!({
        "extra_usage":{"enabled":enabled,"used":used,"limit":limit,"unit":"USD",
            "decimal_places":2,"used_percent":null,"disabled_reason":null,"limit_reached":false},
        "field_times":{"extra_usage":Time::new(at)}
    })
}

fn points(row: &SeriesRow) -> &[Json] {
    row.points
        .as_array()
        .expect("the actual query returns a point array")
}

fn at(point: &Json) -> DateTime<Utc> {
    point["at"].as_str().unwrap().parse().unwrap()
}

fn quantity<'a>(rows: &'a [SeriesRow], member: &str, quantity: &str) -> &'a SeriesRow {
    rows.iter()
        .find(|row| row.member == member && row.quantity == quantity)
        .expect("the actual observed quantity is returned")
}

#[tokio::test]
async fn exact_decimal_readings_keep_new_source_times_and_latest_actual_reading_in_the_minute() {
    let world = World::new().await;
    // Forty decimal characters, deliberately beyond a binary-float round trip.
    let amount = "12345678901234567890.1234567890123456789";
    let later_same_minute = world.at + Duration::seconds(20);
    let next_hour = world.at + Duration::hours(1);
    world
        .write(&balance(world.at, Some(amount), "credits", false), world.at)
        .await;
    world
        .write(
            &balance(later_same_minute, Some(amount), "credits", false),
            later_same_minute,
        )
        .await;
    world
        .write(
            &balance(next_hour, Some(amount), "credits", false),
            next_hour,
        )
        .await;
    assert_eq!(
        world.count().await,
        2,
        "same-minute replacement is not a second sample"
    );
    let rows = world.rows().await;
    assert_eq!(
        rows.len(),
        1,
        "unsupported total and used create no invented series"
    );
    let row = quantity(&rows, "balances", "remaining");
    assert_eq!(row.unit.as_deref(), Some("credits"));
    assert!(!row.overflow);
    assert_eq!(points(row).len(), 2);
    assert_eq!(
        [at(&points(row)[0]), at(&points(row)[1])],
        [later_same_minute, next_hour]
    );
    for point in points(row) {
        assert_eq!(point["amount"].as_str(), Some(amount));
        assert_eq!(point["unlimited"], false);
    }
    let stored: Vec<(DateTime<Utc>, String)> = sqlx::query_as(
        "SELECT observed_at,amount FROM provider_credit_observations WHERE provider_id=$1 ORDER BY observed_at",
    ).bind(world.provider).fetch_all(world.db.pool()).await.unwrap();
    assert_eq!(
        stored,
        vec![
            (later_same_minute, amount.into()),
            (next_hour, amount.into())
        ]
    );
}

#[tokio::test]
async fn cached_values_and_manual_overrides_never_become_new_vendor_observations() {
    let world = World::new().await;
    let mut cached = balance(world.at, Some("12.50000000000000000001"), "credits", false);
    cached["field_times"] = json!({});
    world.write(&cached, world.at).await;
    assert_eq!(
        world.count().await,
        0,
        "existing untimestamped facts are not backfilled"
    );
    let override_facts = json!({"balances":[{"id":"plan","unit":"credits","remaining":"999999"}]});
    assert!(
        change_provider(
            &mut *world.db.acquire().await.unwrap(),
            world.provider,
            world.revision().await,
            &ProviderChange {
                facts_overrides: Some(&override_facts),
                ..Default::default()
            },
            world.at + Duration::minutes(1),
        )
        .await
        .unwrap()
    );
    let refreshed = world.at + Duration::hours(1);
    cached["identity"] = json!("read elsewhere");
    cached["windows"] = json!([{"id":"weekly","label":"Weekly","used_percent":42.0}]);
    cached["field_times"] = json!({"identity":Time::new(refreshed),"windows":Time::new(refreshed)});
    world.write(&cached, refreshed).await;
    assert_eq!(
        world.count().await,
        0,
        "a current window/identity does not date a cached balance"
    );
    let actual = world.at + Duration::hours(2);
    world
        .write(
            &balance(actual, Some("12.50000000000000000001"), "credits", false),
            actual,
        )
        .await;
    let rows = world.rows().await;
    let row = quantity(&rows, "balances", "remaining");
    assert_eq!(points(row).len(), 1);
    assert_eq!(points(row)[0]["amount"], "12.50000000000000000001");
    assert_ne!(
        points(row)[0]["amount"],
        "999999",
        "administered display value is not vendor history"
    );
    let window_refresh = actual + Duration::hours(1);
    let mut retained = balance(actual, Some("12.50000000000000000001"), "credits", false);
    retained["windows"] = json!([{"id":"weekly","used_percent":50.0}]);
    retained["field_times"]["windows"] = json!(Time::new(window_refresh));
    world.write(&retained, window_refresh).await;
    assert_eq!(
        world.count().await,
        1,
        "unchanged source timestamp is not sampled again"
    );
}

#[tokio::test]
async fn a_fresh_collection_or_item_never_redates_another_items_finer_amount_timestamp() {
    for (member, quantity_name) in [("balances", "remaining"), ("spend", "used")] {
        let world = World::new().await;
        let item = |id: &str, amount: &str| {
            let mut value = json!({"id":id,"unit":"vendor credits"});
            value[quantity_name] = json!(amount);
            if member == "spend" {
                value["period"] = json!("month");
            }
            value
        };
        let mut initial = json!({"field_times":{}});
        initial[member] = json!([item("a", "10.0001"), item("b", "30.0003")]);
        initial["field_times"][member] = json!(Time::new(world.at));
        initial["field_times"][format!("{member}.a")] = json!(Time::new(world.at));
        initial["field_times"][format!("{member}.b")] = json!(Time::new(world.at));
        world.write(&initial, world.at).await;
        let fresh_a = world.at + Duration::hours(1);
        let mut next = initial.clone();
        next[member][0][quantity_name] = json!("11.0002");
        next["field_times"][member] = json!(Time::new(fresh_a));
        next["field_times"][format!("{member}.a")] = json!(Time::new(fresh_a));
        // B's explicit item stamp remains old while the collection and A are fresh.
        world.write(&next, fresh_a).await;
        let label_only = world.at + Duration::hours(2);
        next[member][1]["label"] = json!("A fresh label, with the same old amount");
        next["field_times"][member] = json!(Time::new(label_only));
        next["field_times"][format!("{member}.b")] = json!(Time::new(label_only));
        next["field_times"][format!("/{member}/b/{quantity_name}")] = json!(Time::new(world.at));
        world.write(&next, label_only).await;
        let rows = world.rows().await;
        let a = rows
            .iter()
            .find(|row| row.member == member && row.item_id == "a" && row.quantity == quantity_name)
            .unwrap();
        let b = rows
            .iter()
            .find(|row| row.member == member && row.item_id == "b" && row.quantity == quantity_name)
            .unwrap();
        assert_eq!(points(a).len(), 2, "the actual fresh A amount is retained");
        assert_eq!(points(a)[1]["amount"], "11.0002");
        assert_eq!(at(&points(a)[1]), fresh_a);
        assert_eq!(
            points(b).len(),
            1,
            "B keeps its finer old timestamp, not either fresh container stamp"
        );
        assert_eq!(points(b)[0]["amount"], "30.0003");
        assert_eq!(at(&points(b)[0]), world.at);
        assert_eq!(world.count().await, 3);

        let removed_a = world.at + Duration::hours(3);
        let kept_b = next[member][1].clone();
        next[member] = json!([kept_b]);
        next["field_times"][member] = json!(Time::new(removed_a));
        world.write(&next, removed_a).await;
        let fresh_b = world.at + Duration::hours(4);
        next[member][0][quantity_name] = json!("31.0004");
        next["field_times"][member] = json!(Time::new(fresh_b));
        next["field_times"][format!("{member}.b")] = json!(Time::new(fresh_b));
        next["field_times"][format!("/{member}/b/{quantity_name}")] = json!(Time::new(fresh_b));
        world.write(&next, fresh_b).await;
        let rows = world.rows().await;
        let a = rows
            .iter()
            .find(|row| row.member == member && row.item_id == "a" && row.quantity == quantity_name)
            .unwrap();
        let b = rows
            .iter()
            .find(|row| row.member == member && row.item_id == "b" && row.quantity == quantity_name)
            .unwrap();
        assert_eq!(
            points(a).len(),
            3,
            "the retired ID retains its first clear, without repeating it at B's refresh"
        );
        assert_eq!(at(points(a).last().unwrap()), removed_a);
        assert!(points(a).last().unwrap()["amount"].is_null());
        assert_eq!(points(b).len(), 2);
        assert_eq!(at(points(b).last().unwrap()), fresh_b);
        assert_eq!(points(b).last().unwrap()["amount"], "31.0004");
        assert_eq!(world.count().await, 5);

        let reintroduced = world.at + Duration::hours(5);
        let kept_b = next[member][0].clone();
        next[member] = json!([item("a", "2.0005"), kept_b]);
        next["field_times"][member] = json!(Time::new(reintroduced));
        next["field_times"][format!("{member}.a")] = json!(Time::new(reintroduced));
        world.write(&next, reintroduced).await;
        let rows = world.rows().await;
        let a = rows
            .iter()
            .find(|row| row.member == member && row.item_id == "a" && row.quantity == quantity_name)
            .unwrap();
        assert_eq!(points(a).len(), 4);
        assert_eq!(at(points(a).last().unwrap()), reintroduced);
        assert_eq!(points(a).last().unwrap()["amount"], "2.0005");
        assert_eq!(
            world.count().await,
            6,
            "a real new reading may reintroduce a cleared item"
        );
    }
}

#[tokio::test]
async fn partial_enabled_events_are_gaps_and_never_retimestamp_cached_used_or_limit_amounts() {
    let world = World::new().await;
    let initial = extra(
        world.at,
        Some("12345678901234567890.123456789"),
        Some("99999999999999999999.99"),
        true,
    );
    world.write(&initial, world.at).await;
    let stopped = world.at + Duration::hours(1);
    let mut partial = initial.clone();
    partial["extra_usage"]["enabled"] = json!(false);
    partial["field_times"]["extra_usage.enabled"] = json!(Time::new(stopped));
    world.write(&partial, stopped).await;
    let rows = world.rows().await;
    assert_eq!(rows.len(), 2);
    for (name, original) in [
        ("used", "12345678901234567890.123456789"),
        ("limit", "99999999999999999999.99"),
    ] {
        let row = quantity(&rows, "extra_usage", name);
        assert_eq!(row.decimal_places, Some(2));
        assert_eq!(points(row).len(), 2);
        assert_eq!(points(row)[0]["amount"], original);
        let gap = &points(row)[1];
        assert_eq!(at(gap), stopped);
        assert!(
            gap["amount"].is_null(),
            "an enabled-only event is not a fresh amount"
        );
        assert_eq!(gap["enabled"], false);
        assert_eq!(gap["gap_before"], true);
    }
    let resumed = world.at + Duration::hours(2);
    world
        .write(
            &extra(resumed, Some("0"), Some("20000000000000000000.00"), true),
            resumed,
        )
        .await;
    let rows = world.rows().await;
    let used = quantity(&rows, "extra_usage", "used");
    let last = points(used).last().unwrap();
    assert_eq!(
        last["amount"], "0",
        "real zero is not an unknown or off state"
    );
    assert_eq!(last["enabled"], true);
    assert_eq!(
        points(used).len(),
        3,
        "the off gap remains between the actual amounts"
    );
}

#[tokio::test]
async fn zero_unlimited_cleared_and_resumed_balance_are_distinct_actual_observations() {
    let world = World::new().await;
    world
        .write(&balance(world.at, Some("0"), "credits", false), world.at)
        .await;
    let unknown = world.at + Duration::hours(1);
    world
        .write(&balance(unknown, None, "credits", false), unknown)
        .await;
    let unlimited = world.at + Duration::hours(2);
    world
        .write(&balance(unlimited, None, "credits", true), unlimited)
        .await;
    let cleared = world.at + Duration::hours(3);
    world
        .write(
            &json!({"balances":[],"field_times":{"balances":Time::new(cleared)}}),
            cleared,
        )
        .await;
    let resumed = world.at + Duration::hours(4);
    world
        .write(
            &balance(resumed, Some("25.0001"), "credits", false),
            resumed,
        )
        .await;
    let rows = world.rows().await;
    let row = quantity(&rows, "balances", "remaining");
    assert_eq!(points(row).len(), 5);
    assert_eq!(points(row)[0]["amount"], "0");
    assert_eq!(points(row)[0]["unlimited"], false);
    assert!(points(row)[1]["amount"].is_null());
    assert_eq!(
        points(row)[1]["unlimited"],
        false,
        "unknown is not an unlimited plan"
    );
    assert!(points(row)[2]["amount"].is_null());
    assert_eq!(points(row)[2]["unlimited"], true);
    assert!(points(row)[3]["amount"].is_null());
    for point in &points(row)[1..4] {
        assert_eq!(point["gap_before"], true);
    }
    assert_eq!(at(&points(row)[3]), cleared);
    let marker: bool = sqlx::query_scalar(
        "SELECT cleared FROM provider_credit_observations WHERE provider_id=$1 AND observed_at=$2 AND quantity='remaining'",
    ).bind(world.provider).bind(cleared).fetch_one(world.db.pool()).await.unwrap();
    assert!(marker, "the explicit collection clear is retained as a gap");
    assert_eq!(points(row)[4]["amount"], "25.0001");
    assert_eq!(points(row)[4]["unlimited"], false);
}

#[tokio::test]
async fn a_missing_amount_between_readings_in_one_minute_keeps_a_gap_without_inventing_a_point() {
    let world = World::new().await;
    world
        .write(
            &balance(world.at, Some("10.0001"), "credits", false),
            world.at,
        )
        .await;
    let missing = world.at + Duration::seconds(10);
    world
        .write(&balance(missing, None, "credits", false), missing)
        .await;
    let latest = world.at + Duration::seconds(20);
    world
        .write(&balance(latest, Some("12.0002"), "credits", false), latest)
        .await;
    assert_eq!(
        world.count().await,
        1,
        "the existing minute density stays bounded"
    );
    let rows = world.rows().await;
    let row = quantity(&rows, "balances", "remaining");
    assert_eq!(points(row).len(), 1);
    assert_eq!(at(&points(row)[0]), latest);
    assert_eq!(points(row)[0]["amount"], "12.0002");
    assert_eq!(
        points(row)[0]["gap_before"],
        true,
        "same-minute replacement must not erase an explicitly observed unknown interval"
    );
}

#[tokio::test]
async fn source_generation_and_unit_changes_keep_groups_separate_and_paginate_quantities_together()
{
    let world = World::new().await;
    world
        .write(&extra(world.at, Some("100"), Some("500"), true), world.at)
        .await;
    assert_eq!(world.source_revision().await, 0);
    let model_sources = json!({"models":{"by":"none"}});
    assert!(
        set_sources(
            &mut *world.db.acquire().await.unwrap(),
            world.provider,
            world.revision().await,
            &model_sources,
            false,
            false,
            world.at + Duration::minutes(1),
        )
        .await
        .unwrap()
    );
    assert_eq!(
        world.source_revision().await,
        0,
        "unrelated source selection is not a credit source change"
    );
    let changed_unit = world.at + Duration::hours(1);
    let mut pounds = extra(changed_unit, Some("125"), Some("750"), true);
    pounds["extra_usage"]["unit"] = json!("GBP");
    world.write(&pounds, changed_unit).await;
    let facts_sources =
        json!({"facts":{"by":"builtin","name":"openrouter"},"models":{"by":"none"}});
    let before_source_change = world.revision().await;
    let source_generation = before_source_change + 1;
    assert!(
        set_sources(
            &mut *world.db.acquire().await.unwrap(),
            world.provider,
            before_source_change,
            &facts_sources,
            true,
            true,
            world.at + Duration::hours(2),
        )
        .await
        .unwrap()
    );
    assert_eq!(world.source_revision().await, source_generation);
    let new_source = world.at + Duration::hours(3);
    let mut next = extra(new_source, Some("10"), Some("1000"), true);
    next["extra_usage"]["unit"] = json!("GBP");
    world.write(&next, new_source).await;
    let rows = world.rows().await;
    assert_eq!(
        rows.len(),
        6,
        "two amounts in each of three source/unit groups"
    );
    let groups: BTreeSet<_> = rows.iter().map(|row| row.group_key.clone()).collect();
    assert_eq!(groups.len(), 3);
    let identities: BTreeSet<_> = rows
        .iter()
        .map(|row| (row.source_revision, row.unit.as_deref()))
        .collect();
    assert_eq!(
        identities,
        BTreeSet::from([
            (0, Some("USD")),
            (0, Some("GBP")),
            (source_generation, Some("GBP"))
        ])
    );
    let first = read(
        &mut *world.db.acquire().await.unwrap(),
        world.provider,
        world.at,
        None,
        1,
    )
    .await;
    let first_groups: BTreeSet<_> = first.iter().map(|row| row.group_key.as_str()).collect();
    assert_eq!(
        first_groups.len(),
        2,
        "the query returns the selected group and one lookahead group"
    );
    for group in first_groups {
        assert_eq!(
            first
                .iter()
                .filter(|row| row.group_key == group)
                .map(|row| row.quantity.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["limit", "used"])
        );
    }
    let after = &first[0].group_key;
    let rest = read(
        &mut *world.db.acquire().await.unwrap(),
        world.provider,
        world.at,
        Some(after),
        1,
    )
    .await;
    assert!(rest.iter().all(|row| row.group_key != *after));
    assert_eq!(
        rest.iter()
            .map(|row| &row.group_key)
            .collect::<BTreeSet<_>>()
            .len(),
        2
    );
}

#[tokio::test]
async fn observations_rollback_with_facts_and_trim_uses_the_existing_time_cutoff() {
    let world = World::new().await;
    world
        .write(&balance(world.at, Some("10.1"), "credits", false), world.at)
        .await;
    let later = world.at + Duration::hours(1);
    let mut tx = world.db.begin().await.unwrap();
    set_facts(
        &mut tx,
        world.provider,
        &balance(later, Some("9.9"), "credits", false),
        later,
    )
    .await
    .unwrap();
    let visible = read(&mut tx, world.provider, world.at, None, 20).await;
    assert_eq!(points(quantity(&visible, "balances", "remaining")).len(), 2);
    tx.rollback().await.unwrap();
    assert_eq!(world.count().await, 1);
    let facts: Json = sqlx::query_scalar("SELECT facts FROM providers WHERE id=$1")
        .bind(world.provider)
        .fetch_one(world.db.pool())
        .await
        .unwrap();
    assert_eq!(facts["balances"][0]["remaining"], "10.1");
    world
        .write(&balance(later, Some("9.9"), "credits", false), later)
        .await;
    let cutoff = later.with_second(0).unwrap();
    let removed = provider_credits::trim(&mut *world.db.acquire().await.unwrap(), cutoff)
        .await
        .unwrap();
    assert_eq!(removed, 1);
    let rows = world.rows().await;
    let points = points(quantity(&rows, "balances", "remaining"));
    assert_eq!(points.len(), 1);
    assert_eq!(
        at(&points[0]),
        later,
        "a reading in the retained boundary minute survives"
    );
    assert_eq!(points[0]["amount"], "9.9");
}
