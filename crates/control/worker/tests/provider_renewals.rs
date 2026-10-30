//! A runtime that has been stopped for as long as its setting allows is started to renew the sign-in it holds, once; one stopped for less, one whose provider has the renewal off, one that has a job under way and one that is up are left alone.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_db::queries::resources::provider_runtime::{self as rows, NewRuntime, NewSubscription};
use orch_db::queries::resources::testing::insert_machine;
use orch_types::{Id, new_id};
use orch_worker_core::channels::Notifiers;
use orch_worker_core::provider_renewals::{self, Renewed};
use orch_worker_core::schedule::{self, Job};
use serde_json::{Value, json};
use support::world::World;

/// A managed subscription whose runtime has been in `state` for `days` days.
async fn provider_in(world: &World, org: Id, state: &str, days: i64, config: Value) -> Id {
    let now = world.now();
    let mut conn = world.test.pool.acquire().await.unwrap();
    let (agent, provider, runtime, host, template) =
        (new_id(), new_id(), new_id(), new_id(), new_id());
    sqlx::query(
        "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
         VALUES ($1, 'Fake agent', $2, $3, $3, 'fake', '{}', '{}')",
    )
    .bind(agent)
    .bind(world.admin)
    .bind(org)
    .execute(&mut *conn)
    .await
    .unwrap();
    let made = rows::insert_subscription(
        &mut conn,
        &NewSubscription {
            id: provider,
            name: &format!("Team {provider}"),
            owner_id: world.admin,
            home_id: org,
            harness_kind: "fake",
            agent_id: agent,
            config: &json!({}),
            arrangements: &["separated".to_owned()],
            runtime_config: &config,
        },
        now,
    )
    .await
    .unwrap();
    assert!(made);
    let machine = insert_machine(&mut conn, &format!("host-{provider}"), "connected")
        .await
        .unwrap();
    sqlx::query("INSERT INTO hosts (id, machine_id, name) VALUES ($1, $2, 'host')")
        .bind(host)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO env_templates (id, name, driver, spec, kind) VALUES ($1, 'unit', 'docker', '{}', 'provider')")
        .bind(template)
        .execute(&mut *conn)
        .await
        .unwrap();
    rows::insert_runtime(
        &mut conn,
        &NewRuntime {
            id: runtime,
            host_id: host,
            unit_name: &format!("orch-provider-{}", &runtime.simple().to_string()[20..]),
            provider_id: provider,
            driver: "docker",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &json!({}),
        },
        now,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE runtimes SET state = $2, updated_at = $3 WHERE id = $1")
        .bind(runtime)
        .bind(state)
        .bind(now - Duration::days(days))
        .execute(&mut *conn)
        .await
        .unwrap();
    // With no known expiry or successful renewal, the provider's creation is the cadence anchor.
    sqlx::query("UPDATE providers SET created_at = $2 WHERE id = $1")
        .bind(provider)
        .bind(now - Duration::days(days))
        .execute(&mut *conn)
        .await
        .unwrap();
    provider
}

async fn jobs_of(world: &World, provider: Id) -> Vec<(String, String, Value)> {
    sqlx::query_as("SELECT kind, state, plan FROM jobs WHERE provider_id = $1 ORDER BY created_at")
        .bind(provider)
        .fetch_all(&world.test.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_directory_login_renews_at_the_same_fake_clock_deadline_only_when_its_home_may_be_changed()
 {
    let world = World::new(Notifiers::new()).await;
    let org = world.org("Acme").await;
    let mut expected = Vec::new();
    for (owned, allowed) in [(true, false), (false, false), (false, true)] {
        let home = if owned {
            json!({"kind":"made_by_orch"})
        } else {
            json!({"kind":"existing_directory","path":"/allowed/home"})
        };
        let provider = provider_in(&world, org, "running", 8, json!({"runtime.home":home})).await;
        let machine:Id=sqlx::query_scalar("SELECT h.machine_id FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE r.provider_id=$1")
            .bind(provider).fetch_one(&world.test.pool).await.unwrap();
        sqlx::query("UPDATE providers SET machine_id=$2 WHERE id=$1")
            .bind(provider)
            .bind(machine)
            .execute(&world.test.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM runtimes WHERE provider_id=$1")
            .bind(provider)
            .execute(&world.test.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET online=true,roles=ARRAY['agent','provider'],facts=$2 WHERE id=$1").bind(machine)
            .bind(json!({"environment_descriptor":{"lifecycle":"attach"},"policy":{"provider.signin":allowed}}))
            .execute(&world.test.pool).await.unwrap();
        expected.push((provider, owned || allowed));
    }
    let mut conn = world.test.pool.acquire().await.unwrap();
    let done = provider_renewals::renew_due(&mut conn, world.now())
        .await
        .unwrap();
    assert_eq!(done.looked_at, 3);
    assert_eq!(done.queued, 2);
    drop(conn);
    for (provider, allowed) in expected {
        let jobs = jobs_of(&world, provider).await;
        assert_eq!(!jobs.is_empty(), allowed);
        if let Some((_, _, plan)) = jobs.first() {
            assert_eq!(plan["mode"], "renew");
        }
        let progress: Option<Value> =
            sqlx::query_scalar("SELECT progress FROM jobs WHERE provider_id=$1")
                .bind(provider)
                .fetch_optional(&world.test.pool)
                .await
                .unwrap();
        if let Some(progress) = progress {
            assert_eq!(
                progress["phases"].as_array().unwrap().len(),
                1,
                "a directory renewal never creates, starts or stops a unit"
            );
            assert_eq!(progress["phases"][0]["name"], "refresh");
        }
    }
}

#[tokio::test]
async fn a_runtime_stopped_for_as_many_days_as_the_setting_allows_is_started_once_to_renew_its_sign_in()
 {
    let world = World::new(Notifiers::new()).await;
    let org = world.org("Acme").await;
    let due = provider_in(&world, org, "stopped", 8, json!({})).await;
    let db = world.db();
    let done = provider_renewals::scheduled(&db, &world.clock)
        .await
        .unwrap()
        .expect("the run held the lock");
    assert_eq!(
        done,
        Renewed {
            looked_at: 1,
            queued: 1
        }
    );
    let jobs = jobs_of(&world, due).await;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].0, "provider_provision");
    assert_eq!(jobs[0].1, "queued");
    assert_eq!(
        jobs[0].2["mode"], "renew",
        "the engine starts the unit in the mode that renews"
    );
    // The next run finds the job under way and queues no other.
    let again = provider_renewals::scheduled(&db, &world.clock)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.queued, 0);
    assert_eq!(jobs_of(&world, due).await.len(), 1);
}

#[tokio::test]
async fn renewal_honors_its_own_interval_and_off_choice_and_includes_running_runtimes() {
    let world = World::new(Notifiers::new()).await;
    let org = world.org("Acme").await;
    let recent = provider_in(&world, org, "stopped", 3, json!({})).await;
    let off = provider_in(
        &world,
        org,
        "stopped",
        30,
        json!({ "runtime.renew_days": 0 }),
    )
    .await;
    let own = provider_in(
        &world,
        org,
        "stopped",
        10,
        json!({ "runtime.renew_days": 14 }),
    )
    .await;
    let up = provider_in(&world, org, "running", 30, json!({})).await;
    let short = provider_in(
        &world,
        org,
        "stopped",
        2,
        json!({ "runtime.renew_days": 2 }),
    )
    .await;
    let db = world.db();
    let done = provider_renewals::scheduled(&db, &world.clock)
        .await
        .unwrap()
        .unwrap();
    for (name, provider, queued) in [
        ("less than the days", recent, false),
        ("the renewal off", off, false),
        ("its own, longer number of days", own, false),
        ("up with an old unknown-expiry sign-in", up, true),
        ("its own, shorter number of days", short, true),
    ] {
        assert_eq!(
            !jobs_of(&world, provider).await.is_empty(),
            queued,
            "{name}"
        );
    }
    assert_eq!(done.queued, 2);
}

#[tokio::test]
async fn known_expiry_renews_inside_five_days_even_after_a_recent_stop_but_tokens_and_the_boundary_do_not()
 {
    let world = World::new(Notifiers::new()).await;
    let org = world.org("Acme").await;
    let due = provider_in(&world, org, "stopped", 0, json!({})).await;
    let boundary = provider_in(&world, org, "running", 0, json!({})).await;
    let token = provider_in(&world, org, "running", 30, json!({})).await;
    for (provider, days, yields) in [
        (due, 4, "login"),
        (boundary, 5, "login"),
        (token, 1, "token"),
    ] {
        sqlx::query("UPDATE providers SET facts = $2 WHERE id = $1")
            .bind(provider)
            .bind(json!({"credential":{"method":"native", "yields":yields,
                "expires_at":orch_types::Time::new(world.now() + Duration::days(days))}}))
            .execute(&world.test.pool)
            .await
            .unwrap();
    }
    let done = provider_renewals::scheduled(&world.db(), &world.clock)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.queued, 1);
    assert_eq!(jobs_of(&world, due).await.len(), 1);
    assert!(jobs_of(&world, boundary).await.is_empty());
    assert!(jobs_of(&world, token).await.is_empty());
}

#[tokio::test]
async fn two_workers_do_not_queue_the_same_renewal_at_once() {
    let world = World::new(Notifiers::new()).await;
    let org = world.org("Acme").await;
    let due = provider_in(&world, org, "stopped", 9, json!({})).await;
    let db = world.db();
    let held = schedule::take(&db, Job::ProviderRenew)
        .await
        .unwrap()
        .expect("the first run takes the lock");
    let other = provider_renewals::scheduled(&db, &world.clock)
        .await
        .unwrap();
    assert_eq!(
        other, None,
        "the second finds the lock taken and does nothing"
    );
    assert!(jobs_of(&world, due).await.is_empty());
    drop(held);
}
