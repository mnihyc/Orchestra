//! Each purge and trim removes what is due and nothing else; certificates about to end are told to their owners once; a worker that starts brings back what a dead one left; and the running worker does its jobs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::{DateTime, Duration, TimeZone, Utc};
use orch_config::ServerConfig;
use orch_db::queries::identity::queue_mail;
use orch_db::queries::identity::testing::insert_session;
use orch_db::queries::notifications::outbox as db_outbox;
use orch_db::queries::notifications::routes::{self, RouteChange};
use orch_db::queries::notifications::upkeep::SoftDeletePurged;
use orch_db::queries::resources::testing::insert_machine;
use orch_db::{heartbeat as db_heartbeat, migrations};
use orch_notifier_contract::descriptor::Reaches;
use orch_types::{Id, Json, new_id};
use orch_worker_core::channels::Notifiers;
use orch_worker_core::heartbeat::{self, Identity};
use orch_worker_core::schedule::{self, Services};
use orch_worker_core::{purge, reconciliation, reminders, retention};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use support::fakes::Scripted;
use support::world::{Of, World};
use tokio_util::sync::CancellationToken;

async fn count(world: &World, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(&world.test.pool)
        .await
        .unwrap()
}

/// What is in the Inbox of a person: `(kind, title, data)`, the oldest first.
async fn inbox(world: &World, user: Id) -> Vec<(String, String, Json)> {
    sqlx::query_as(
        "SELECT kind, title, data FROM notifications WHERE user_id = $1 ORDER BY inbox_seq",
    )
    .bind(user)
    .fetch_all(&world.test.pool)
    .await
    .unwrap()
}

fn serving(notifier: &Arc<Scripted>) -> Notifiers {
    let mut notifiers = Notifiers::new();
    notifiers.register(notifier.clone()).unwrap();
    notifiers
}

fn rows_by_table(removed: &[SoftDeletePurged]) -> BTreeMap<&'static str, u64> {
    removed.iter().map(|each| (each.table, each.rows)).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn what_has_run_out_is_removed_and_what_has_not_is_left() {
    let world = World::new(Notifiers::new()).await;
    let now = world.now();
    let (ada, bob) = (world.person("ada").await, world.person("bob").await);
    let mut conn = world.test.pool.acquire().await.unwrap();

    // Sessions with an access token each: one ended an hour ago, one is good for another.
    insert_session(&mut conn, new_id(), ada, &[1; 32], now - Duration::hours(1))
        .await
        .unwrap();
    insert_session(&mut conn, new_id(), bob, &[2; 32], now + Duration::hours(1))
        .await
        .unwrap();
    // Mailed links: one ended, one is good.
    for (n, ends) in [
        (1_u8, now - Duration::minutes(1)),
        (2, now + Duration::hours(2)),
    ] {
        sqlx::query(
            "INSERT INTO email_tokens (id, user_id, purpose, token_hash, email, expires_at, created_at)
             VALUES ($1, $2, 'verify_email', $3, 'a@example.org', $4, $5)",
        )
        .bind(new_id())
        .bind(ada)
        .bind([n; 32].as_slice())
        .bind(ends)
        .bind(now - Duration::days(1))
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    // Mailed codes: ada's ended, bob's is good.
    for (user, ends) in [
        (ada, now - Duration::seconds(1)),
        (bob, now + Duration::minutes(5)),
    ] {
        sqlx::query(
            "INSERT INTO sign_in_codes (user_id, code_hash, email, expires_at, created_at)
             VALUES ($1, $2, 'a@example.org', $3, $4)",
        )
        .bind(user)
        .bind([9_u8; 32].as_slice())
        .bind(ends)
        .bind(now - Duration::minutes(5))
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    // Browsers: one nobody got in from for 91 days, one for 89.
    for (n, last) in [
        (1_u8, now - Duration::days(91)),
        (2, now - Duration::days(89)),
    ] {
        sqlx::query(
            "INSERT INTO known_devices (id, mark_hash, user_id, first_at, last_at) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(new_id())
        .bind([n; 32].as_slice())
        .bind(ada)
        .bind(now - Duration::days(200))
        .bind(last)
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    // Counts of wrong tries, the longest window being three hours: aged out and never told;
    // fresh; aged out but told an hour ago; aged out and told two days ago.
    for (bucket, tries, told) in [
        ("password|gone", vec![now - Duration::hours(4)], None),
        ("password|fresh", vec![now - Duration::minutes(10)], None),
        (
            "password|told-lately",
            vec![now - Duration::hours(5)],
            Some(now - Duration::hours(1)),
        ),
        (
            "password|told-long-ago",
            vec![now - Duration::hours(5)],
            Some(now - Duration::days(2)),
        ),
    ] {
        sqlx::query("INSERT INTO login_throttle (bucket, times, noticed_at) VALUES ($1, $2, $3)")
            .bind(bucket)
            .bind(tries)
            .bind(told)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    // Answers kept to repeat a request, for a day: 25 hours old, 23 hours old.
    for (key, at) in [
        ("old", now - Duration::hours(25)),
        ("new", now - Duration::hours(23)),
    ] {
        sqlx::query(
            "INSERT INTO idempotency_keys (user_id, key, method, path, status, response, created_at)
             VALUES ($1, $2, 'POST', '/x', 201, '{}', $3)",
        )
        .bind(ada)
        .bind(key)
        .bind(at)
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    // A single sign-on attempt that ended, one that has not.
    let provider = new_id();
    sqlx::query(
        "INSERT INTO identity_providers (id, key, name, issuer, client_id) VALUES ($1, 'sso', 'SSO', 'https://sso.example', 'orch')",
    )
    .bind(provider)
    .execute(&mut *conn)
    .await
    .unwrap();
    for (state, ends) in [
        ("ended", now - Duration::minutes(1)),
        ("going", now + Duration::minutes(5)),
    ] {
        sqlx::query(
            "INSERT INTO oidc_states (state, identity_provider_id, nonce, pkce_verifier, browser_hash, expires_at)
             VALUES ($1, $2, 'n', 'v', decode('00', 'hex'), $3)",
        )
        .bind(state)
        .bind(provider)
        .bind(ends)
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    // Enrollment tokens: unused and ended; used and ended; unused and good.
    let machine = insert_machine(&mut conn, "laptop", "connected")
        .await
        .unwrap();
    for (n, ends, used, machine_id) in [
        (1_u8, now - Duration::minutes(1), None, None),
        (
            2,
            now - Duration::minutes(1),
            Some(now - Duration::minutes(10)),
            Some(machine),
        ),
        (3, now + Duration::minutes(10), None, None),
    ] {
        sqlx::query(
            "INSERT INTO enrollment_tokens (token_hash, created_by, purpose, expires_at, used_at, machine_id)
             VALUES ($1, $2, 'connected', $3, $4, $5)",
        )
        .bind([n; 32].as_slice())
        .bind(ada)
        .bind(ends)
        .bind(used)
        .bind(machine_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    // A code for linking a bot that nobody followed in time, and one that is still good.
    let chat = world.channel(Of::System, "chat", json!({}), &[]).await;
    for (user, ends) in [
        (ada, now - Duration::minutes(1)),
        (bob, now + Duration::minutes(5)),
    ] {
        let route = world.route(user, &chat, RouteChange::default()).await;
        routes::start_link(
            &mut conn,
            route.id,
            &[5; 32],
            ends,
            now - Duration::minutes(10),
        )
        .await
        .unwrap();
    }
    drop(conn);

    let config = ServerConfig::default();
    let removed = purge::expired(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .expect("the lock is free");
    assert_eq!(removed.sign_in.access_tokens, 1);
    assert_eq!(removed.sign_in.sessions, 1);
    assert_eq!(removed.sign_in.email_tokens, 1);
    assert_eq!(removed.sign_in.sign_in_codes, 1);
    assert_eq!(removed.sign_in.known_devices, 1);
    assert_eq!(removed.sign_in.oidc_states, 1);
    assert_eq!(removed.sign_in.enrollment_tokens, 1);
    assert_eq!(removed.throttle, 2);
    assert_eq!(removed.idempotency_keys, 1);
    assert_eq!(removed.link_codes, 1);

    for (table, left) in [
        ("access_tokens", 1),
        ("sessions", 1),
        ("email_tokens", 1),
        ("sign_in_codes", 1),
        ("known_devices", 1),
        ("oidc_states", 1),
        ("enrollment_tokens", 2),
        ("idempotency_keys", 1),
        ("users", 3),
    ] {
        assert_eq!(count(&world, table).await, left, "{table}");
    }
    let kept: Vec<String> = sqlx::query_scalar("SELECT bucket FROM login_throttle ORDER BY bucket")
        .fetch_all(&world.test.pool)
        .await
        .unwrap();
    assert_eq!(kept, ["password|fresh", "password|told-lately"]);
    let waiting: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notification_routes WHERE link_hash IS NOT NULL")
            .fetch_one(&world.test.pool)
            .await
            .unwrap();
    assert_eq!(waiting, 1, "the code that was good stays");
    assert_eq!(
        count(&world, "notification_routes").await,
        2,
        "and no route went with a code"
    );

    let again = purge::expired(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            again.sign_in.access_tokens,
            again.throttle,
            again.idempotency_keys,
            again.link_codes
        ),
        (0, 0, 0, 0),
        "a second run finds nothing more"
    );

    // Time moves on: what was good has run out.
    world.clock.advance(Duration::days(2));
    let later = purge::expired(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            later.sign_in.sessions,
            later.sign_in.email_tokens,
            later.sign_in.sign_in_codes
        ),
        (1, 1, 1)
    );
    assert_eq!(
        (
            later.sign_in.oidc_states,
            later.sign_in.enrollment_tokens,
            later.link_codes
        ),
        (1, 1, 1)
    );
    assert_eq!(later.idempotency_keys, 1);
    assert_eq!(
        count(&world, "enrollment_tokens").await,
        1,
        "the one that was used is a record of the machine and stays"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rows_soft_deleted_long_enough_ago_are_removed_unless_something_still_refers_to_them() {
    let world = World::new(Notifiers::new()).await;
    let now = world.now();
    let (old, young) = (now - Duration::days(31), now - Duration::days(29));
    let mut conn = world.test.pool.acquire().await.unwrap();

    // Machines: one long gone; one deleted lately; one alive; one long gone that a host row still refers to.
    let gone = insert_machine(&mut conn, "gone", "connected")
        .await
        .unwrap();
    let recent = insert_machine(&mut conn, "recent", "connected")
        .await
        .unwrap();
    let _live = insert_machine(&mut conn, "live", "connected")
        .await
        .unwrap();
    let hosted = insert_machine(&mut conn, "hosted", "connected")
        .await
        .unwrap();
    for (machine, at) in [(gone, old), (recent, young), (hosted, old)] {
        sqlx::query("UPDATE machines SET deleted_at = $2 WHERE id = $1")
            .bind(machine)
            .bind(at)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO hosts (id, machine_id, name) VALUES ($1, $2, 'host')")
        .bind(new_id())
        .bind(hosted)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    // An org gone with its project; an org gone whose project was deleted later; an org that is alive.
    let acme = world.org("Acme").await;
    let acme_project = world.node("project", acme, "Web").await;
    let globex = world.org("Globex").await;
    let globex_project = world.node("project", globex, "Api").await;
    let initech = world.org("Initech").await;
    for (node, at) in [
        (acme, old),
        (acme_project, old),
        (globex, old),
        (globex_project, young),
    ] {
        sqlx::query("UPDATE nodes SET deleted_at = $2 WHERE id = $1")
            .bind(node)
            .bind(at)
            .execute(&world.test.pool)
            .await
            .unwrap();
    }

    let config = ServerConfig::default();
    let removed = purge::soft_deleted(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .expect("the lock is free");
    let by_table = rows_by_table(&removed);
    assert_eq!(by_table.get("machines"), Some(&1), "{by_table:?}");
    assert_eq!(
        by_table.get("nodes"),
        Some(&2),
        "the org and its project, the project first: {by_table:?}"
    );

    let machines: Vec<String> = sqlx::query_scalar("SELECT name FROM machines ORDER BY name")
        .fetch_all(&world.test.pool)
        .await
        .unwrap();
    assert_eq!(
        machines,
        ["hosted", "live", "recent"],
        "the one a host refers to stays, as does the one deleted lately"
    );
    let nodes: Vec<Id> = sqlx::query_scalar("SELECT id FROM nodes")
        .fetch_all(&world.test.pool)
        .await
        .unwrap();
    assert!(
        !nodes.contains(&acme) && !nodes.contains(&acme_project),
        "an org deleted long ago goes with its project"
    );
    assert!(
        nodes.contains(&globex) && nodes.contains(&globex_project),
        "an org stays while a project of it that was deleted later is still there"
    );
    assert!(nodes.contains(&initech), "and one that is alive stays");

    // The day the project is old enough, the org goes with it, and so does the machine deleted lately.
    world.clock.advance(Duration::days(2));
    let removed = purge::soft_deleted(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .unwrap();
    let by_table = rows_by_table(&removed);
    assert_eq!(by_table.get("nodes"), Some(&2), "{by_table:?}");
    assert_eq!(by_table.get("machines"), Some(&1), "{by_table:?}");
    let nodes: Vec<Id> = sqlx::query_scalar("SELECT id FROM nodes")
        .fetch_all(&world.test.pool)
        .await
        .unwrap();
    assert!(
        nodes.contains(&initech) && !nodes.contains(&globex) && !nodes.contains(&globex_project)
    );

    // And when the host is gone, so is the machine.
    sqlx::query("DELETE FROM hosts")
        .execute(&world.test.pool)
        .await
        .unwrap();
    let removed = purge::soft_deleted(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows_by_table(&removed).get("machines"), Some(&1));
    assert_eq!(
        count(&world, "machines").await,
        1,
        "only the live one is left"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn raw_events_past_their_days_go_with_their_partitions_and_the_rest_are_trimmed_row_by_row() {
    let world = World::new(Notifiers::new()).await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    // Partitions for July to November.
    for month in [7, 9] {
        migrations::ensure_partitions(
            &mut conn,
            Utc.with_ymd_and_hms(2026, month, 15, 0, 0, 0).unwrap(),
        )
        .await
        .unwrap();
    }
    let segment = new_id();
    let at = |month, day| Utc.with_ymd_and_hms(2026, month, day, 9, 0, 0).unwrap();
    for (n, when) in [
        (1_i64, at(7, 15)),
        (2, at(9, 10)),
        (3, at(9, 30)),
        (4, at(10, 5)),
    ] {
        sqlx::query(
            "INSERT INTO raw_events (segment_id, n, at, direction, payload) VALUES ($1, $2, $3, 'to_harness', '\\x00')",
        )
        .bind(segment)
        .bind(n)
        .bind(when)
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    drop(conn);

    // The clock shows 8 October; fourteen days are kept, so the cutoff is 24 September.
    let done = retention::raw_events(&world.db(), &world.clock)
        .await
        .unwrap()
        .expect("the lock is free");
    assert_eq!(
        done.partitions_dropped, 2,
        "July and August ended before the cutoff"
    );
    assert_eq!(
        done.rows_trimmed, 1,
        "the tenth of September is in a month that is only partly old"
    );
    let left: Vec<i64> = sqlx::query_scalar("SELECT n FROM raw_events ORDER BY n")
        .fetch_all(&world.test.pool)
        .await
        .unwrap();
    assert_eq!(left, [3, 4]);

    // A longer time is kept when the setting says so: 30 days reach back to the eighth of September.
    sqlx::query("INSERT INTO settings (node_id, key, value) VALUES (NULL, 'diagnostics.retention_days', '30')")
        .execute(&world.test.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO raw_events (segment_id, n, at, direction, payload) VALUES ($1, 5, $2, 'to_harness', '\\x00')",
    )
    .bind(segment)
    .bind(at(9, 10))
    .execute(&world.test.pool)
    .await
    .unwrap();
    let done = retention::raw_events(&world.db(), &world.clock)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((done.partitions_dropped, done.rows_trimmed), (0, 0));
    assert_eq!(count(&world, "raw_events").await, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_minutes_of_metric_samples_go_only_once_their_hours_are_folded_and_the_hours_when_they_are_old_enough()
 {
    let world = World::new(Notifiers::new()).await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    for day in [
        Utc.with_ymd_and_hms(2026, 8, 20, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
        world.now(),
    ] {
        migrations::ensure_partitions(&mut conn, day).await.unwrap();
    }
    let minute = |month, day, hour| Utc.with_ymd_and_hms(2026, month, day, hour, 5, 0).unwrap();
    for when in [minute(8, 20, 10), minute(9, 1, 10), minute(10, 8, 13)] {
        sqlx::query("INSERT INTO metric_samples (at, name, labels, value) VALUES ($1, 'outbox_pending', '{}', 3)")
            .bind(when)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    // The hour of the first day is folded; that of the second is not (the rollup is behind). An hour from May is there too.
    for hour in [
        Utc.with_ymd_and_hms(2026, 8, 20, 10, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 5, 1, 10, 0, 0).unwrap(),
    ] {
        sqlx::query(
            "INSERT INTO metric_hours (at, name, labels, mean, peak, total, samples) VALUES ($1, 'outbox_pending', '{}', 3, 3, 3, 1)",
        )
        .bind(hour)
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    drop(conn);
    let days = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT to_char(at AT TIME ZONE 'UTC', 'MM-DD') FROM metric_samples ORDER BY at",
        )
        .fetch_all(&world.test.pool)
        .await
        .unwrap()
    };

    // 0 keeps every minute for good.
    let mut every_minute = ServerConfig::default();
    every_minute.retention.metrics_fine_days = 0;
    let done = retention::metric_minutes(&world.db(), &world.clock, &every_minute)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((done.partitions_dropped, done.hours_removed), (0, 0));
    assert_eq!(days().await, ["08-20", "09-01", "10-08"]);

    let config = ServerConfig::default();
    let done = retention::metric_minutes(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .expect("the lock is free");
    assert_eq!(
        done.partitions_dropped, 3,
        "20, 21 and 22 August: folded and old"
    );
    assert_eq!(done.hours_removed, 0, "hours are kept for good as shipped");
    assert_eq!(
        days().await,
        ["09-01", "10-08"],
        "the first of September is old but its hour is not folded, so it stays, and so does everything after it"
    );

    let mut keeping_hours = ServerConfig::default();
    keeping_hours.retention.metrics_days = 100;
    let done = retention::metric_minutes(&world.db(), &world.clock, &keeping_hours)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        done.hours_removed, 1,
        "the hour of the first of May is past the hundred days; the one in August is not"
    );
    assert_eq!(count(&world, "metric_hours").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_person_keeps_their_newest_recent_visits_and_no_more() {
    let world = World::new(Notifiers::new()).await;
    let org = world.org("Acme").await;
    let (ada, bob) = (world.person("ada").await, world.person("bob").await);
    let mut places = Vec::new();
    for n in 0..8 {
        places.push(world.node("project", org, &format!("Project {n}")).await);
    }
    for (n, place) in places.iter().enumerate() {
        sqlx::query(
            "INSERT INTO recents (user_id, kind, id, visited_at) VALUES ($1, 'project', $2, $3)",
        )
        .bind(ada)
        .bind(place)
        .bind(world.now() - Duration::hours(i64::try_from(n).unwrap()))
        .execute(&world.test.pool)
        .await
        .unwrap();
    }
    for place in &places[..3] {
        sqlx::query("INSERT INTO recents (user_id, kind, id) VALUES ($1, 'project', $2)")
            .bind(bob)
            .bind(place)
            .execute(&world.test.pool)
            .await
            .unwrap();
    }
    let mut config = ServerConfig::default();
    config.limits.recents_per_user = 5;
    assert_eq!(
        retention::recents(&world.db(), &config).await.unwrap(),
        Some(3)
    );
    let ada_kept: Vec<Id> =
        sqlx::query_scalar("SELECT id FROM recents WHERE user_id = $1 ORDER BY visited_at DESC")
            .bind(ada)
            .fetch_all(&world.test.pool)
            .await
            .unwrap();
    assert_eq!(ada_kept, &places[..5], "the five newest");
    let bob_kept: i64 = sqlx::query_scalar("SELECT count(*) FROM recents WHERE user_id = $1")
        .bind(bob)
        .fetch_one(&world.test.pool)
        .await
        .unwrap();
    assert_eq!(bob_kept, 3, "a person under the share loses nothing");
    assert_eq!(
        retention::recents(&world.db(), &config).await.unwrap(),
        Some(0)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_of_a_machine_whose_certificate_is_about_to_end_is_told_once_for_each_certificate()
 {
    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(serving(&notifier)).await;
    let chat = world.channel(Of::System, "chat", json!({}), &[]).await;
    let ada = world.person("ada").await;
    world.route_on(ada, &chat).await;
    let now = world.now();
    let machine = |name: &'static str, ends: DateTime<Utc>, owner: Option<Id>| {
        let pool = world.test.pool.clone();
        async move {
            let mut conn = pool.acquire().await.unwrap();
            let id = insert_machine(&mut conn, name, "connected").await.unwrap();
            sqlx::query("UPDATE machines SET cert_not_after = $2, owner_id = $3 WHERE id = $1")
                .bind(id)
                .bind(ends)
                .bind(owner)
                .execute(&mut *conn)
                .await
                .unwrap();
            id
        }
    };
    let soon = machine("laptop", now + Duration::days(10), Some(ada)).await;
    machine("far", now + Duration::days(40), Some(ada)).await;
    machine("ended long ago", now - Duration::days(100), Some(ada)).await;
    machine("ended lately", now - Duration::days(10), Some(ada)).await;
    machine("managed", now + Duration::days(5), None).await;

    let config = ServerConfig::default();
    let done = reminders::certificates(&world.db(), &world.clock, &config, true)
        .await
        .unwrap()
        .expect("the lock is free");
    assert_eq!((done.machines, done.told), (2, 2));
    let rows = inbox(&world, ada).await;
    assert_eq!(rows.len(), 2);
    let titles: Vec<&str> = rows.iter().map(|(_, title, _)| title.as_str()).collect();
    assert!(
        titles
            .iter()
            .any(|title| title.contains("laptop") && title.contains("ends on")),
        "{titles:?}"
    );
    assert!(
        titles
            .iter()
            .any(|title| title.contains("ended lately") && title.contains("ended on")),
        "{titles:?}"
    );
    assert!(rows.iter().all(
        |(kind, _, data)| kind == "machine.offline" && data["reason"] == "certificate_expiring"
    ));
    assert!(
        rows.iter()
            .any(|(_, _, data)| data["path"] == format!("/resources/machines/{soon}"))
    );

    assert_eq!(
        world.outbox().run().await.unwrap().sent,
        2,
        "and by the routes they switched on"
    );
    assert_eq!(notifier.deliveries(), 2);

    world.clock.advance(Duration::days(1));
    let again = reminders::certificates(&world.db(), &world.clock, &config, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (again.machines, again.told),
        (2, 0),
        "a day later: still about to end, and not told again"
    );

    // A machine that renewed has another end, and is told if that comes near too.
    sqlx::query("UPDATE machines SET cert_not_after = $2 WHERE id = $1")
        .bind(soon)
        .bind(now + Duration::days(12))
        .execute(&world.test.pool)
        .await
        .unwrap();
    let renewed = reminders::certificates(&world.db(), &world.clock, &config, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renewed.told, 1);
    assert_eq!(inbox(&world, ada).await.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_starts_ends_the_sessions_a_dead_one_left_holding_claims_and_counts_what_waits()
 {
    let notifier = Scripted::new("email", Reaches::Email);
    let world = World::new(serving(&notifier)).await;
    let channel = world.channel(Of::System, "email", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let route = world.route_on(ada, &channel).await;
    let waiting = world
        .queue_for_route(&route, "email", &[world.snapshot("turn.completed", "One")])
        .await;

    // A session that claimed the row and went quiet.
    let db = world.db();
    let mut dead = db_outbox::begin_claim(&db, std::time::Duration::from_secs(900))
        .await
        .unwrap();
    assert!(
        db_outbox::claim_due(&mut dead, world.now())
            .await
            .unwrap()
            .is_some()
    );
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    assert_eq!(
        reconciliation::end_stale_claims(&db, std::time::Duration::from_secs(600))
            .await
            .unwrap(),
        0,
        "a claim that is not old is left"
    );
    assert_eq!(
        reconciliation::end_stale_claims(&db, std::time::Duration::from_secs(1))
            .await
            .unwrap(),
        1,
        "one that is old is ended"
    );
    assert_eq!(
        world.outbox().run().await.unwrap().sent,
        1,
        "and its row is taken up"
    );
    assert_eq!(world.row(waiting).await.state, "sent");
    drop(dead);

    // Rows that were due long before the start are counted.
    world
        .queue_for_route(&route, "email", &[world.snapshot("turn.completed", "Two")])
        .await;
    world.clock.advance(Duration::minutes(5));
    let recovered = reconciliation::at_start(&db, &world.clock).await.unwrap();
    assert_eq!((recovered.sessions_ended, recovered.overdue), (0, 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_worker_reports_itself_and_writes_what_it_measures_for_each_minute() {
    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(serving(&notifier)).await;
    let chat = world.channel(Of::System, "chat", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let route = world.route_on(ada, &chat).await;
    world
        .queue_for_route(&route, "chat", &[world.snapshot("turn.completed", "One")])
        .await;
    world
        .queue_for_route(&route, "chat", &[world.snapshot("turn.completed", "Two")])
        .await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    migrations::ensure_partitions(&mut conn, world.now())
        .await
        .unwrap();

    let who = Identity::begin("0.1.0-test", &world.clock);
    heartbeat::beat(&world.db(), &world.clock, &who)
        .await
        .unwrap();
    world.clock.advance(Duration::seconds(15));
    heartbeat::beat(&world.db(), &world.clock, &who)
        .await
        .unwrap();
    let beats = db_heartbeat::all(&mut conn).await.unwrap();
    assert_eq!(
        beats.len(),
        1,
        "one row for the worker, however often it reports"
    );
    assert_eq!(
        (
            beats[0].component.as_str(),
            beats[0].system,
            beats[0].version.as_str(),
            beats[0].seen_at
        ),
        ("worker", who.run, "0.1.0-test", world.now())
    );
    drop(conn);

    let samples = || async {
        sqlx::query_as::<_, (String, Json, f64)>(
            "SELECT name, labels, value FROM metric_samples ORDER BY at, name, labels::text",
        )
        .fetch_all(&world.test.pool)
        .await
        .unwrap()
    };
    let sample = |store: bool| {
        let (db, notifiers) = (world.db(), world.notifiers.clone());
        let (clock, metrics) = (world.clock.clone(), Arc::clone(&world.metrics));
        async move {
            heartbeat::sample(&db, &clock, &metrics, &notifiers, store)
                .await
                .unwrap()
        }
    };

    // The first minute: two rows wait, one route is on.
    assert_eq!(sample(true).await, 3, "the queue, the routes and the pool");
    let first = samples().await;
    assert!(
        first.contains(&(
            "outbox_pending".to_owned(),
            json!({ "channel": "chat" }),
            2.0
        )),
        "{first:?}"
    );
    assert!(
        first.contains(&(
            "routes".to_owned(),
            json!({ "channel": "chat", "state": "on" }),
            1.0
        )),
        "{first:?}"
    );
    assert!(first.iter().any(|row| row.0 == "db_pool_in_use"));

    // The second: what was queued is sent, so the queue stands at 0 and not at what it was, and what was sent is counted.
    assert_eq!(world.outbox().run().await.unwrap().sent, 2);
    world.clock.advance(Duration::minutes(1));
    assert_eq!(sample(true).await, 4);
    let all = samples().await;
    let second = &all[first.len()..];
    assert!(
        second.contains(&(
            "outbox_pending".to_owned(),
            json!({ "channel": "chat" }),
            0.0
        )),
        "{second:?}"
    );
    let sent = (
        "route_messages_total".to_owned(),
        json!({ "channel": "chat", "form": "single", "result": "sent" }),
    );
    assert!(
        second.contains(&(sent.0.clone(), sent.1.clone(), 2.0)),
        "{second:?}"
    );

    // A sample again in the same minute: a gauge is replaced by the later value, a counter adds what grew since.
    world
        .queue_for_route(&route, "chat", &[world.snapshot("turn.completed", "Three")])
        .await;
    assert_eq!(world.outbox().run().await.unwrap().sent, 1);
    assert_eq!(sample(true).await, 4);
    let later = samples().await;
    assert_eq!(
        later.len(),
        all.len(),
        "the minute's rows are the same rows"
    );
    assert!(later.contains(&(sent.0, sent.1, 3.0)), "{later:?}");

    // A system that stores nothing writes nothing.
    assert_eq!(sample(false).await, 0);
    assert_eq!(samples().await.len(), later.len());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_worker_delivers_what_is_queued_reports_itself_and_stops_when_it_is_told_to() {
    let notifier = Scripted::new("email", Reaches::Email);
    let world = World::new(serving(&notifier)).await;
    let channel = world.channel(Of::System, "email", json!({}), &[]).await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    queue_mail(
        &mut conn,
        "email",
        channel.id,
        &json!({
            "purpose": "sign_in_code", "to": "ada@example.org", "user_id": null, "language": "en",
            "data": { "code": "ABCD-1234" }, "subject": "Your code", "body": "ABCD-1234"
        }),
        world.now(),
    )
    .await
    .unwrap();
    migrations::ensure_partitions(&mut conn, world.now())
        .await
        .unwrap();
    drop(conn);

    let services = Services {
        db: world.db(),
        clock: Arc::new(world.clock.clone()),
        config: Arc::new(ServerConfig::default()),
        notifiers: world.notifiers.clone(),
        master: world.master.clone(),
        who: Identity::begin("0.1.0-test", &world.clock),
        metrics: Arc::clone(&world.metrics),
    };
    let stop = CancellationToken::new();
    let running = tokio::spawn(schedule::run(services, stop.clone()));
    for _ in 0..100 {
        if notifier.deliveries() > 0 && count(&world, "component_heartbeats").await == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    stop.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(20), running)
        .await
        .expect("it stops when told to")
        .unwrap()
        .unwrap();
    assert_eq!(notifier.deliveries(), 1);
    assert_eq!(world.rows().await[0].state, "sent");
    assert_eq!(count(&world, "component_heartbeats").await, 1);
    assert_eq!(
        notifier.delivered()[0].to,
        json!({ "email": "ada@example.org" })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_waits_for_the_schema_the_api_applies_and_stops_waiting_when_it_is_told_to() {
    let world = World::new(Notifiers::new()).await;
    let poll = std::time::Duration::from_millis(50);
    let stop = CancellationToken::new();
    // The database of a test is complete: a migration it has needs no waiting; one it has not is waited for.
    assert!(
        reconciliation::wait_for_schema(&world.db(), 1, poll, &stop)
            .await
            .unwrap()
    );
    let waiting = tokio::spawn({
        let (db, stop) = (world.db(), stop.clone());
        async move { reconciliation::wait_for_schema(&db, i64::MAX, poll, &stop).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert!(
        !waiting.is_finished(),
        "a schema that is not there yet is waited for"
    );
    stop.cancel();
    assert!(
        !waiting.await.unwrap().unwrap(),
        "and the wait ends, without the schema, when the worker is told to stop"
    );
}
