//! What the engine reads and writes to keep a provider on a key running: its start row and the names of its secrets without their values, where it stands, which providers a machine's absence moves, and which were switched off.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{DateTime, Duration, Utc};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::resources::keeper::{
    self, OFFLINE_DETAIL, PresenceMoves, move_status, rate_catalogue, secret_refs, set_facts,
    start_row, switched_off_since, sync_presence,
};
use orch_db::queries::resources::routes::delete_provider;
use orch_db::queries::resources::testing::insert_machine;
use orch_db::queries::settings::put_secret;
use orch_db::{Conn, Db};
use orch_testkit::database::TestDb;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

/// The time now as the database keeps it, to the microsecond, so that what is written reads back equal.
fn now() -> DateTime<Utc> {
    DateTime::from_timestamp_micros(Utc::now().timestamp_micros()).unwrap()
}

/// A pool for a database of its own.
async fn world() -> (TestDb, Db) {
    let db = TestDb::fresh().await;
    let pool = Db::from_pool(
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&db.url)
            .await
            .unwrap(),
    );
    (db, pool)
}

/// A person and the org the providers of a test belong to.
struct Home {
    owner: Id,
    org: Id,
}

async fn home(conn: &mut Conn) -> Home {
    let (owner, org) = (new_id(), new_id());
    insert_user(conn, owner, "ada").await.unwrap();
    insert_node(conn, org, "org", None, "Acme", owner)
        .await
        .unwrap();
    Home { owner, org }
}

/// A machine that is, or is not, there.
async fn machine(conn: &mut Conn, name: &str, online: bool) -> Id {
    let id = insert_machine(conn, name, "connected").await.unwrap();
    set_online(conn, id, online).await;
    id
}

async fn set_online(conn: &mut Conn, machine: Id, online: bool) {
    sqlx::query("UPDATE machines SET online = $2 WHERE id = $1")
        .bind(machine)
        .bind(online)
        .execute(conn)
        .await
        .unwrap();
}

async fn plan_chart(
    conn: &mut Conn,
    provider: Id,
    window: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> orch_db::queries::provider_plan::ChartRow {
    use orch_db::queries::provider_plan::{self, PlanRead, WindowDefinition};
    let windows = [WindowDefinition {
        window_id: window.to_owned(),
        natural_start: Some(orch_types::Time::new(from)),
        lookback_seconds: 3600,
    }];
    provider_plan::chart(
        conn,
        &PlanRead {
            provider,
            windows: &windows,
            from,
            to: to + Duration::milliseconds(1),
            now: to,
            bucket: "hour",
        },
    )
    .await
    .unwrap()
    .remove(0)
}

#[tokio::test]
async fn plan_readings_are_atomic_gauges_and_old_hours_keep_their_peak() {
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let home = home(&mut conn).await;
    let provider = api(
        &mut conn,
        &home,
        Api {
            machine: None,
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    let at = now() - Duration::hours(3);
    for (offset, value) in [(0, 20), (0, 30), (5, 80), (10, 40)] {
        let facts = json!({"windows":[{"id":"five","label":"5h","used_percent":value,"resets_at":null},
            {"id":"unknown","used_percent":null},{"id":"invalid","used_percent":"NaN"}]});
        set_facts(&mut conn, provider, &facts, at + Duration::minutes(offset))
            .await
            .unwrap();
    }
    let samples: Vec<f64> = sqlx::query_scalar("SELECT value FROM metric_samples WHERE name='provider_plan_used_percent' AND labels->>'provider'=$1 ORDER BY at")
        .bind(provider.to_string()).fetch_all(&mut *conn).await.unwrap();
    assert_eq!(samples, [30.0, 80.0, 40.0]);
    orch_db::migrations::ensure_partitions(&mut conn, now())
        .await
        .unwrap();
    orch_db::queries::notifications::upkeep::fold_metric_hours(&mut conn, None, now())
        .await
        .unwrap();
    sqlx::query("DELETE FROM metric_samples WHERE name='provider_plan_used_percent' AND labels->>'provider'=$1")
        .bind(provider.to_string()).execute(&mut *conn).await.unwrap();
    let chart = plan_chart(&mut conn, provider, "five", at - Duration::hours(1), now()).await;
    let points: Vec<orch_provider_contract::forecast::ForecastPoint> =
        serde_json::from_value(chart.points).unwrap();
    assert_eq!(
        points
            .iter()
            .map(|point| point.used_percent)
            .fold(0.0_f64, f64::max),
        80.0
    );
    use sqlx::Connection;
    let mut tx = conn.begin().await.unwrap();
    set_facts(
        &mut tx,
        provider,
        &json!({"windows":[{"id":"five","used_percent":99}]}),
        now(),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let count: i64=sqlx::query_scalar("SELECT count(*) FROM metric_samples WHERE name='provider_plan_used_percent' AND labels->>'provider'=$1")
        .bind(provider.to_string()).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn early_resets_record_the_observed_cause_once_and_explicit_receipts_are_reused() {
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let home = home(&mut conn).await;
    let at = now();
    for cause in ["outside", "vendor", "orch"] {
        let provider = api(
            &mut conn,
            &home,
            Api {
                machine: None,
                key_machine: None,
                status: "ready",
            },
        )
        .await;
        let facts = |percent, count| json!({"windows":[{"id":"week","label":"Weekly","used_percent":percent,"resets_at":at+Duration::days(2)}],"available_resets":{"available_count":count}});
        set_facts(&mut conn, provider, &facts(80, 3), at)
            .await
            .unwrap();
        if cause == "orch" {
            let mut receipt = orch_db::audit::Entry::new(
                orch_db::audit::Actor::User(home.owner),
                "provider.reset_used",
                "provider",
                Some(provider),
                "A reset was confirmed",
            );
            receipt.details = json!({"windows_before":[{"id":"week","used_percent":80,"resets_at":at+Duration::days(2)}],"windows_after":[{"id":"week","used_percent":0,"resets_at":at+Duration::days(2)}]});
            orch_db::audit::append(
                &mut conn,
                &receipt,
                &Default::default(),
                at + Duration::minutes(4),
            )
            .await
            .unwrap();
        }
        let after = facts(0, if cause == "vendor" { 3 } else { 2 });
        set_facts(&mut conn, provider, &after, at + Duration::minutes(5))
            .await
            .unwrap();
        set_facts(&mut conn, provider, &after, at + Duration::minutes(6))
            .await
            .unwrap();
        let entries:Vec<Json>=sqlx::query_scalar("SELECT details FROM audit_log WHERE object_id=$1 AND action='provider.window_started_over'")
            .bind(provider).fetch_all(&mut *conn).await.unwrap();
        if cause == "orch" {
            assert!(entries.is_empty());
        } else {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0]["cause"], cause);
            assert_eq!(entries[0]["window_id"], "week");
            assert_eq!(entries[0]["before_percent"], 80.0);
        }
        let chart = plan_chart(
            &mut conn,
            provider,
            "week",
            at - Duration::hours(1),
            at + Duration::minutes(10),
        )
        .await;
        let markers = chart.resets.as_array().unwrap();
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0]["cause"], cause);
    }
}

#[tokio::test]
async fn first_plan_readings_and_partition_upkeep_can_arrive_together() {
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let home = home(&mut conn).await;
    let first = api(
        &mut conn,
        &home,
        Api {
            machine: None,
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    let second = api(
        &mut conn,
        &home,
        Api {
            machine: None,
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    drop(conn);
    let mut one = db.acquire().await.unwrap();
    let mut two = db.acquire().await.unwrap();
    let mut upkeep = db.acquire().await.unwrap();
    let facts = json!({"windows":[{"id":"week","used_percent":20}]});
    let at = now();
    let (one, two, upkeep) = tokio::join!(
        set_facts(&mut one, first, &facts, at),
        set_facts(&mut two, second, &facts, at),
        orch_db::migrations::ensure_partitions(&mut upkeep, at)
    );
    assert!(one.unwrap());
    assert!(two.unwrap());
    upkeep.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM metric_samples WHERE name='provider_plan_used_percent'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn an_orch_reset_receipt_in_the_readings_transaction_is_reused_when_the_clock_has_not_ticked()
{
    use sqlx::Connection;
    let (_fixture, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let home = home(&mut conn).await;
    let provider = api(
        &mut conn,
        &home,
        Api {
            machine: None,
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    let at = now();
    let facts = |used| json!({"windows":[{"id":"week","label":"Weekly","used_percent":used,"resets_at":at+Duration::days(2)}]});
    set_facts(&mut conn, provider, &facts(80), at)
        .await
        .unwrap();
    let mut tx = conn.begin().await.unwrap();
    let mut receipt = orch_db::audit::Entry::new(
        orch_db::audit::Actor::User(home.owner),
        "provider.reset_used",
        "provider",
        Some(provider),
        "A reset was confirmed",
    );
    receipt.details = json!({"windows_before":[{"id":"week","used_percent":80,"resets_at":at+Duration::days(2)}],"windows_after":[{"id":"week","used_percent":0,"resets_at":at+Duration::days(2)}]});
    orch_db::audit::append(&mut tx, &receipt, &Default::default(), at)
        .await
        .unwrap();
    set_facts(&mut tx, provider, &facts(0), at).await.unwrap();
    tx.commit().await.unwrap();
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='provider.window_started_over'").bind(provider).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(count, 0);
}

/// What an API provider is made with.
struct Api {
    machine: Option<Id>,
    key_machine: Option<Id>,
    status: &'static str,
}

async fn api(conn: &mut Conn, home: &Home, shape: Api) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, upstreams, protocols,
                                machine_id, key_machine_id, arrangements, status)
         VALUES ($1, 'Key', $2, $3, $3, 'api', $4, ARRAY['openai-responses'], $5, $6,
                 ARRAY['separated','key_separated'], $7)",
    )
    .bind(id)
    .bind(home.owner)
    .bind(home.org)
    .bind(json!([{ "protocol": "openai-responses", "base_url": "https://api.example.com/v1", "options": {} }]))
    .bind(shape.machine)
    .bind(shape.key_machine)
    .bind(shape.status)
    .execute(conn)
    .await
    .unwrap();
    id
}

async fn standing(conn: &mut Conn, provider: Id) -> (String, Option<String>, i64) {
    sqlx::query_as("SELECT status, status_detail, revision FROM providers WHERE id = $1")
        .bind(provider)
        .fetch_one(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_provider_is_read_for_its_start_also_when_deleted_and_its_secrets_come_without_values() {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    let home = home(&mut conn).await;
    let there = machine(&mut conn, "keeper", true).await;
    let provider = api(
        &mut conn,
        &home,
        Api {
            machine: None,
            key_machine: Some(there),
            status: "ready",
        },
    )
    .await;

    let row = start_row(&mut conn, provider).await.unwrap().unwrap();
    assert_eq!((row.id, row.kind.as_str()), (provider, "api"));
    assert_eq!(row.key_machine_id, Some(there));
    assert_eq!(row.machine_id, None);
    assert_eq!(row.protocols, ["openai-responses"]);
    assert_eq!(row.upstreams[0]["base_url"], "https://api.example.com/v1");
    assert_eq!(row.deleted_at, None);
    assert!(start_row(&mut conn, new_id()).await.unwrap().is_none());

    // The secrets are named, in order, and never given: a command refers to them by their identifiers.
    assert!(secret_refs(&mut conn, provider).await.unwrap().is_empty());
    let first = now() - Duration::hours(2);
    let key = put_secret(
        &mut conn,
        "provider",
        provider,
        "api_key",
        b"sealed-one",
        None,
        first,
    )
    .await
    .unwrap();
    put_secret(
        &mut conn,
        "provider",
        provider,
        "header:x-org",
        b"sealed-two",
        None,
        first,
    )
    .await
    .unwrap();
    // Another owner's secret of the same name is not the provider's.
    put_secret(
        &mut conn,
        "provider",
        new_id(),
        "api_key",
        b"x",
        None,
        first,
    )
    .await
    .unwrap();
    let refs = secret_refs(&mut conn, provider).await.unwrap();
    assert_eq!(
        refs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["api_key", "header:x-org"]
    );
    assert_eq!(refs[0].id, key.id);
    assert_eq!(refs[0].version, first);

    // A new value is the same secret under a later version, so that the command made from it is another one.
    let later = now();
    let rotated = put_secret(
        &mut conn,
        "provider",
        provider,
        "api_key",
        b"sealed-new",
        None,
        later,
    )
    .await
    .unwrap();
    assert_eq!(rotated.id, key.id);
    let refs = secret_refs(&mut conn, provider).await.unwrap();
    assert_eq!(refs[0].version, later);
    assert_eq!(refs[1].version, first);

    // A deleted provider is still read, so that the machine that holds it can be told to stop it.
    delete_provider(&mut conn, provider, later).await.unwrap();
    let gone = start_row(&mut conn, provider).await.unwrap().unwrap();
    assert_eq!(gone.deleted_at, Some(later));
    assert_eq!(gone.status, "disabled");
}

#[tokio::test]
async fn a_status_moves_only_from_the_states_named_and_only_when_it_changes() {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    let home = home(&mut conn).await;
    let there = machine(&mut conn, "keeper", true).await;
    let provider = api(
        &mut conn,
        &home,
        Api {
            machine: Some(there),
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    let now = now();
    let (status, detail, revision) = standing(&mut conn, provider).await;
    assert_eq!((status.as_str(), detail), ("ready", None));

    assert!(
        move_status(
            &mut conn,
            provider,
            &["ready", "error"],
            "error",
            Some("the key was refused"),
            now
        )
        .await
        .unwrap()
    );
    let (status, detail, after) = standing(&mut conn, provider).await;
    assert_eq!(
        (status.as_str(), detail.as_deref()),
        ("error", Some("the key was refused"))
    );
    assert_eq!(after, revision + 1);

    // Saying the same again changes nothing, not even the revision.
    assert!(
        !move_status(
            &mut conn,
            provider,
            &["ready", "error"],
            "error",
            Some("the key was refused"),
            now
        )
        .await
        .unwrap()
    );
    assert_eq!(standing(&mut conn, provider).await.2, after);
    // A new sentence is a change.
    assert!(
        move_status(
            &mut conn,
            provider,
            &["error"],
            "error",
            Some("still refused"),
            now
        )
        .await
        .unwrap()
    );

    // A provider in another state is left alone: here it was switched off by a person.
    sqlx::query("UPDATE providers SET status = 'disabled' WHERE id = $1")
        .bind(provider)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        !move_status(&mut conn, provider, &["ready", "error"], "ready", None, now)
            .await
            .unwrap()
    );
    assert_eq!(standing(&mut conn, provider).await.0, "disabled");

    // And so is a deleted one, whatever it stands as.
    sqlx::query("UPDATE providers SET status = 'ready', deleted_at = now() WHERE id = $1")
        .bind(provider)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        !move_status(&mut conn, provider, &["ready"], "error", None, now)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn facts_are_written_when_they_differ_and_not_otherwise() {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    let home = home(&mut conn).await;
    let there = machine(&mut conn, "keeper", true).await;
    let provider = api(
        &mut conn,
        &home,
        Api {
            machine: Some(there),
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    let now = now();
    let facts = json!({ "models": ["mock-large"], "protocols": ["openai-responses"] });
    let before = standing(&mut conn, provider).await.2;

    assert!(set_facts(&mut conn, provider, &facts, now).await.unwrap());
    assert!(!set_facts(&mut conn, provider, &facts, now).await.unwrap());
    let stored: (serde_json::Value, Option<DateTime<Utc>>, i64) =
        sqlx::query_as("SELECT facts, facts_at, revision FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(stored.0, facts);
    assert_eq!(stored.1, Some(now));
    assert_eq!(stored.2, before + 1, "the second report was not a change");
    assert!(
        set_facts(&mut conn, provider, &json!({ "models": [] }), now)
            .await
            .unwrap()
    );

    // A deleted provider takes no more reports.
    delete_provider(&mut conn, provider, now).await.unwrap();
    assert!(!set_facts(&mut conn, provider, &facts, now).await.unwrap());
}

#[tokio::test]
async fn a_machine_that_is_away_puts_its_api_providers_offline_and_their_return_brings_them_back() {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    let home = home(&mut conn).await;
    let keeper_machine = machine(&mut conn, "keeper", true).await;
    let runtime_machine = machine(&mut conn, "runtime", true).await;
    let unit_machine = insert_machine(&mut conn, "unit", "managed").await.unwrap();
    set_online(&mut conn, unit_machine, true).await;
    let now = now();

    // The key is on a machine of its own.
    let on_a_key = api(
        &mut conn,
        &home,
        Api {
            machine: None,
            key_machine: Some(keeper_machine),
            status: "ready",
        },
    )
    .await;
    // The runtime is a person's machine and the system holds the key.
    let on_a_runtime = api(
        &mut conn,
        &home,
        Api {
            machine: Some(runtime_machine),
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    // The runtime is a managed one: whether its machine is there is its own supervision's business.
    let managed = api(
        &mut conn,
        &home,
        Api {
            machine: Some(unit_machine),
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    // Somebody switched this one off, and another is in a state of its own.
    let switched_off = api(
        &mut conn,
        &home,
        Api {
            machine: Some(runtime_machine),
            key_machine: None,
            status: "disabled",
        },
    )
    .await;
    let exhausted = api(
        &mut conn,
        &home,
        Api {
            machine: Some(runtime_machine),
            key_machine: None,
            status: "exhausted",
        },
    )
    .await;
    // A subscription is a harness's login, whatever machine that is on.
    let agent = new_id();
    sqlx::query(
        "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
         VALUES ($1, 'Codex', $2, $3, $3, 'codex', '{}', '{}')",
    )
    .bind(agent)
    .bind(home.owner)
    .bind(home.org)
    .execute(&mut *conn)
    .await
    .unwrap();
    let subscription = new_id();
    sqlx::query(
        "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                machine_id, arrangements, status)
         VALUES ($1, 'Plan', $2, $3, $3, 'subscription', 'codex', $4, $5, ARRAY['separated'], 'ready')",
    )
    .bind(subscription)
    .bind(home.owner)
    .bind(home.org)
    .bind(agent)
    .bind(runtime_machine)
    .execute(&mut *conn)
    .await
    .unwrap();

    // Everything is there: nothing moves.
    assert_eq!(
        sync_presence(&mut conn, now).await.unwrap(),
        PresenceMoves::default()
    );

    // The key's machine and the runtime's machine go away.
    set_online(&mut conn, keeper_machine, false).await;
    set_online(&mut conn, runtime_machine, false).await;
    set_online(&mut conn, unit_machine, false).await;
    let moved = sync_presence(&mut conn, now).await.unwrap();
    let mut away = moved.away.clone();
    away.sort();
    let mut expected = vec![on_a_key, on_a_runtime];
    expected.sort();
    assert_eq!(away, expected);
    assert!(moved.back.is_empty());
    let (status, detail, _) = standing(&mut conn, on_a_key).await;
    assert_eq!(
        (status.as_str(), detail.as_deref()),
        ("offline", Some(OFFLINE_DETAIL))
    );
    for left_alone in [managed, subscription] {
        assert_eq!(standing(&mut conn, left_alone).await.0, "ready");
    }
    assert_eq!(standing(&mut conn, switched_off).await.0, "disabled");
    assert_eq!(standing(&mut conn, exhausted).await.0, "exhausted");
    // Looking again is not another move.
    let revision = standing(&mut conn, on_a_key).await.2;
    assert_eq!(
        sync_presence(&mut conn, now).await.unwrap(),
        PresenceMoves::default()
    );
    assert_eq!(standing(&mut conn, on_a_key).await.2, revision);

    // One machine returns: only what stands on it comes back, and the sentence goes.
    set_online(&mut conn, keeper_machine, true).await;
    let moved = sync_presence(&mut conn, now).await.unwrap();
    assert_eq!(moved.back, [on_a_key]);
    assert!(moved.away.is_empty());
    let (status, detail, _) = standing(&mut conn, on_a_key).await;
    assert_eq!((status.as_str(), detail), ("ready", None));
    assert_eq!(standing(&mut conn, on_a_runtime).await.0, "offline");

    // A machine that was revoked does not bring a provider back, however it reports itself.
    set_online(&mut conn, runtime_machine, true).await;
    sqlx::query("UPDATE machines SET status = 'revoked' WHERE id = $1")
        .bind(runtime_machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(sync_presence(&mut conn, now).await.unwrap().back.is_empty());
    assert_eq!(standing(&mut conn, on_a_runtime).await.0, "offline");
}

#[tokio::test]
async fn providers_switched_off_or_deleted_are_listed_oldest_first_with_the_machine_that_runs_them()
{
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    let home = home(&mut conn).await;
    let key_machine = machine(&mut conn, "keeper", true).await;
    let runtime_machine = machine(&mut conn, "runtime", true).await;
    let origin = now() - Duration::hours(1);
    let at = |seconds: i64| origin + Duration::seconds(seconds);

    let on_a_key = api(
        &mut conn,
        &home,
        Api {
            machine: Some(runtime_machine),
            key_machine: Some(key_machine),
            status: "ready",
        },
    )
    .await;
    let on_a_runtime = api(
        &mut conn,
        &home,
        Api {
            machine: Some(runtime_machine),
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    let nowhere = api(
        &mut conn,
        &home,
        Api {
            machine: None,
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    let running = api(
        &mut conn,
        &home,
        Api {
            machine: Some(runtime_machine),
            key_machine: None,
            status: "ready",
        },
    )
    .await;
    let long_ago = api(
        &mut conn,
        &home,
        Api {
            machine: Some(runtime_machine),
            key_machine: None,
            status: "ready",
        },
    )
    .await;

    for (provider, when) in [
        (long_ago, -50),
        (on_a_runtime, 20),
        (nowhere, 30),
        (on_a_key, 10),
    ] {
        sqlx::query("UPDATE providers SET status = 'disabled', updated_at = $2 WHERE id = $1")
            .bind(provider)
            .bind(at(when))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    // A deletion is a switching off, and is listed the same way.
    delete_provider(&mut conn, running, at(40)).await.unwrap();

    let listed = switched_off_since(&mut conn, origin, 100).await.unwrap();
    assert_eq!(
        listed.iter().map(|s| s.id).collect::<Vec<_>>(),
        [on_a_key, on_a_runtime, running],
        "oldest first; the one with no machine and the one switched off before the time are not there"
    );
    // The machine that keeps the key comes before the one the runtime is on.
    assert_eq!(listed[0].machine_id, Some(key_machine));
    assert_eq!(listed[1].machine_id, Some(runtime_machine));
    assert!(listed.iter().all(|s| s.revision >= 1));

    let page = switched_off_since(&mut conn, origin, 2).await.unwrap();
    assert_eq!(page.len(), 2);
    let later = switched_off_since(&mut conn, at(10), 100).await.unwrap();
    assert_eq!(
        later.iter().map(|s| s.id).collect::<Vec<_>>(),
        [on_a_runtime, running]
    );
    assert!(
        switched_off_since(&mut conn, at(40), 100)
            .await
            .unwrap()
            .is_empty()
    );
    // Nothing of a provider that is not switched off.
    assert!(
        keeper::open_leases_of(&mut conn, on_a_key)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_rate_catalogue_is_read_as_it_was_last_loaded_and_is_none_before() {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(rate_catalogue(&mut conn).await.unwrap(), None);
    let doc =
        json!({ "as_of": "2026-01-01", "models": [{ "family": "mock-*", "input_per_mtok": "1" }] });
    sqlx::query(
        "INSERT INTO rate_catalogue (doc, as_of, origin) VALUES ($1, '2026-01-01', 'shipped')",
    )
    .bind(&doc)
    .execute(&mut *conn)
    .await
    .unwrap();
    assert_eq!(rate_catalogue(&mut conn).await.unwrap(), Some(doc));
}
