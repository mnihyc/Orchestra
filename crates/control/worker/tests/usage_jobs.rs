//! The hours of metric samples are folded once they have ended, and the rate catalogue is fetched only from the address an admin set, when it is due and newer, and is checked whole.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use orch_config::ServerConfig;
use orch_db::audit::{Actor, Origin};
use orch_db::migrations;
use orch_db::seeds;
use orch_worker_core::catalogue::{self, CatalogueError, Outcome, Wanted};
use orch_worker_core::channels::Notifiers;
use orch_worker_core::schedule::{self, Job};
use orch_worker_core::{retention, rollup};
use serde_json::json;
use std::sync::{Arc, Mutex};
use support::world::World;

fn hour_of(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 7, hour, minute, 0).unwrap()
}

async fn sample(world: &World, name: &str, at: DateTime<Utc>, value: f64) {
    sqlx::query("INSERT INTO metric_samples (at, name, labels, value) VALUES ($1, $2, '{}', $3)")
        .bind(at)
        .bind(name)
        .bind(value)
        .execute(&world.test.pool)
        .await
        .unwrap();
}

/// `(hour, name, mean, peak, total, samples)` of every hourly figure, the oldest first.
async fn hours(world: &World) -> Vec<(i64, String, f64, f64, f64, i16)> {
    sqlx::query_as(
        "SELECT extract(hour FROM at AT TIME ZONE 'UTC')::bigint, name, mean, peak, total, samples
           FROM metric_hours ORDER BY at, name",
    )
    .fetch_all(&world.test.pool)
    .await
    .unwrap()
}

async fn world_with_the_days_of_the_samples() -> World {
    let world = World::new(Notifiers::new()).await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    for day in [hour_of(10, 0), world.now()] {
        migrations::ensure_partitions(&mut conn, day).await.unwrap();
    }
    world
}

#[tokio::test(flavor = "multi_thread")]
async fn each_hour_that_has_ended_becomes_one_figure_for_each_series_and_the_running_hour_does_not()
{
    let world = world_with_the_days_of_the_samples().await;
    // The clock shows 14:00 on the eighth; the seventh has a gauge over two hours and a counter in one.
    for (minute, value) in [(5, 1.0), (6, 2.0), (7, 6.0)] {
        sample(&world, "outbox_pending", hour_of(10, minute), value).await;
    }
    sample(&world, "outbox_pending", hour_of(11, 0), 4.0).await;
    sample(&world, "outbox_sent", hour_of(10, 30), 3.0).await;
    sample(&world, "outbox_sent", hour_of(10, 31), 5.0).await;
    // The running hour, which is not over.
    sample(
        &world,
        "outbox_pending",
        world.now() + Duration::minutes(1),
        9.0,
    )
    .await;

    let done = rollup::metric_hours(&world.db(), &world.clock)
        .await
        .unwrap()
        .expect("the lock is free");

    assert_eq!(done.hours, 3);
    assert_eq!(
        hours(&world).await,
        [
            (10, "outbox_pending".to_owned(), 3.0, 6.0, 9.0, 3),
            (10, "outbox_sent".to_owned(), 4.0, 5.0, 8.0, 2),
            (11, "outbox_pending".to_owned(), 4.0, 4.0, 4.0, 1),
        ],
        "mean and largest of the minutes of a gauge, their sum for a counter, and how many minutes had a value"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_that_was_away_folds_every_hour_it_missed_and_a_late_minute_changes_the_newest_hour()
 {
    let world = world_with_the_days_of_the_samples().await;
    sample(&world, "outbox_pending", hour_of(10, 1), 2.0).await;
    sample(&world, "outbox_pending", hour_of(12, 1), 8.0).await;
    sample(&world, "outbox_pending", hour_of(12, 2), 4.0).await;
    // The hour between was missed altogether, and 13:00 is the last that has ended.
    sample(&world, "outbox_pending", hour_of(13, 59), 1.0).await;
    rollup::metric_hours(&world.db(), &world.clock)
        .await
        .unwrap()
        .unwrap();
    let folded = hours(&world).await;
    assert_eq!(
        folded.iter().map(|each| each.0).collect::<Vec<_>>(),
        [10, 12, 13]
    );

    // A minute of the newest hour that arrived after the hour was folded is in the figure at the next run.
    sample(&world, "outbox_pending", hour_of(13, 30), 7.0).await;
    // And a run that finds nothing new leaves what is there as it is.
    rollup::metric_hours(&world.db(), &world.clock)
        .await
        .unwrap()
        .unwrap();
    let after = hours(&world).await;
    assert_eq!(after.len(), 3, "no figure is made twice");
    assert_eq!(after[0], folded[0], "an older hour is not read again");
    assert_eq!(after[1], folded[1]);
    assert_eq!(
        after[2],
        (13, "outbox_pending".to_owned(), 4.0, 7.0, 8.0, 2)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_run_of_the_fold_finds_the_lock_of_the_first_taken() {
    let world = world_with_the_days_of_the_samples().await;
    let first = schedule::take(&world.db(), Job::MetricsRollup)
        .await
        .unwrap()
        .expect("the first run takes the lock");
    assert!(
        rollup::metric_hours(&world.db(), &world.clock)
            .await
            .unwrap()
            .is_none(),
        "the second has nothing to do"
    );
    first.commit().await.unwrap();
    assert!(
        rollup::metric_hours(&world.db(), &world.clock)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_minutes_of_a_day_are_dropped_only_after_the_fold_has_made_their_hours() {
    let world = world_with_the_days_of_the_samples().await;
    let long_ago = Utc.with_ymd_and_hms(2026, 9, 1, 10, 5, 0).unwrap();
    migrations::ensure_partitions(&mut world.test.pool.acquire().await.unwrap(), long_ago)
        .await
        .unwrap();
    sample(&world, "outbox_pending", long_ago, 3.0).await;
    let config = ServerConfig::default();
    let minutes = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM metric_samples")
            .fetch_one(&world.test.pool)
            .await
            .unwrap()
    };

    retention::metric_minutes(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        minutes().await,
        1,
        "its hour is not folded, so the minute stays"
    );

    rollup::metric_hours(&world.db(), &world.clock)
        .await
        .unwrap()
        .unwrap();
    retention::metric_minutes(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        minutes().await,
        0,
        "once the hour is there the minute is not needed"
    );
    assert_eq!(hours(&world).await.len(), 1, "and the hour is kept");
}

// ---- the rate catalogue ----

/// What the address answers with.
#[derive(Clone)]
enum Answer {
    Text(String),
    Status(StatusCode),
    Redirect(String),
}

/// An address on this machine that answers as it is told to.
struct Address {
    url: String,
    answer: Arc<Mutex<Answer>>,
    asked: Arc<Mutex<u32>>,
}

impl Address {
    async fn start() -> Self {
        let answer = Arc::new(Mutex::new(Answer::Status(StatusCode::NOT_FOUND)));
        let asked = Arc::new(Mutex::new(0_u32));
        let (served, counted) = (Arc::clone(&answer), Arc::clone(&asked));
        let app = Router::new().route(
            "/rates.json",
            get(move || {
                let (served, counted) = (Arc::clone(&served), Arc::clone(&counted));
                async move {
                    *counted.lock().unwrap() += 1;
                    let answer = served.lock().unwrap().clone();
                    match answer {
                        Answer::Text(text) => (StatusCode::OK, text).into_response(),
                        Answer::Status(status) => status.into_response(),
                        Answer::Redirect(to) => {
                            let mut response: Response = StatusCode::FOUND.into_response();
                            response
                                .headers_mut()
                                .insert(header::LOCATION, to.parse().unwrap());
                            response
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/rates.json", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, answer, asked }
    }

    fn says(&self, answer: Answer) {
        *self.answer.lock().unwrap() = answer;
    }

    fn times_asked(&self) -> u32 {
        *self.asked.lock().unwrap()
    }
}

/// A catalogue of one family whose rate for input is `input`, with figures as of `as_of`.
fn catalogue_of(as_of: &str, input: &str) -> String {
    json!({
        "format": 1, "as_of": as_of, "unit": "USD",
        "families": [ {
            "match": "gpt-x*", "vendor": "Example", "source": "https://example.test/pricing", "as_of": as_of,
            "rate": { "input_per_mtok": input, "cached_input_per_mtok": "0.1", "output_per_mtok": "10" }
        } ]
    })
    .to_string()
}

/// The same with a second family whose rate is not a number.
fn catalogue_with_a_broken_second_entry(as_of: &str) -> String {
    let mut doc: serde_json::Value = serde_json::from_str(&catalogue_of(as_of, "1")).unwrap();
    doc["families"].as_array_mut().unwrap().push(json!({
        "match": "other-model*", "vendor": "Example", "source": "https://example.test/other", "as_of": as_of,
        "rate": { "input_per_mtok": "two", "cached_input_per_mtok": "0.1", "output_per_mtok": "10" }
    }));
    doc.to_string()
}

async fn set_system(world: &World, key: &str, value: serde_json::Value) {
    sqlx::query("INSERT INTO settings (node_id, key, value) VALUES (NULL, $1, $2)")
        .bind(key)
        .bind(value)
        .execute(&world.test.pool)
        .await
        .unwrap();
}

/// A world whose release came with the catalogue of the tenth of October, and an address.
async fn world_with_a_catalogue_and_an_address() -> (World, Address) {
    let world = World::new(Notifiers::new()).await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    seeds::load_shipped_catalogue(&mut conn, &catalogue_of("2026-10-10", "1"), world.now())
        .await
        .unwrap();
    drop(conn);
    let address = Address::start().await;
    set_system(&world, "usage.catalogue_url", json!(address.url)).await;
    (world, address)
}

/// `(origin, as_of, source, input rate, loaded_by)` of the catalogue in force.
async fn in_force(
    world: &World,
) -> (
    String,
    NaiveDate,
    Option<String>,
    String,
    Option<orch_types::Id>,
) {
    sqlx::query_as(
        "SELECT origin, as_of, source, doc->'families'->0->'rate'->>'input_per_mtok', loaded_by FROM rate_catalogue",
    )
    .fetch_one(&world.test.pool)
    .await
    .unwrap()
}

fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_fetched_where_no_address_was_set() {
    let world = World::new(Notifiers::new()).await;
    let asked = catalogue::refresh(
        &world.db(),
        &world.clock,
        Actor::User(world.admin),
        &Origin::default(),
        Wanted::Always,
    )
    .await;
    assert!(matches!(asked, Err(CatalogueError::NoAddress)), "{asked:?}");
    assert!(
        catalogue::scheduled(&world.db(), &world.clock)
            .await
            .unwrap()
            .is_none(),
        "the schedule does nothing, and fails at nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_admin_who_asks_for_a_fetch_gets_what_the_address_gives_whatever_its_date() {
    let (world, address) = world_with_a_catalogue_and_an_address().await;
    address.says(Answer::Text(catalogue_of("2026-01-01", "2")));

    let outcome = catalogue::refresh(
        &world.db(),
        &world.clock,
        Actor::User(world.admin),
        &Origin::default(),
        Wanted::Always,
    )
    .await
    .unwrap();

    let Outcome::Loaded(loaded) = outcome else {
        panic!("expected the catalogue to be loaded, not {outcome:?}");
    };
    assert_eq!(loaded.as_of, day(2026, 1, 1));
    assert_eq!(loaded.replaced, Some(day(2026, 10, 10)));
    assert_eq!(
        in_force(&world).await,
        (
            "fetched".to_owned(),
            day(2026, 1, 1),
            Some(address.url.clone()),
            "2".to_owned(),
            Some(world.admin)
        ),
        "it says where it came from and who asked"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_schedule_fetches_only_when_the_days_have_passed_and_loads_only_what_is_newer() {
    let (world, address) = world_with_a_catalogue_and_an_address().await;
    set_system(&world, "usage.catalogue_refresh_days", json!(7)).await;
    address.says(Answer::Text(catalogue_of("2026-11-01", "3")));
    let run = || async {
        catalogue::scheduled(&world.db(), &world.clock)
            .await
            .unwrap()
            .expect("the lock is free")
    };

    world.clock.advance(Duration::days(3));
    assert_eq!(run().await, Outcome::NotDue, "seven days have not passed");
    assert_eq!(address.times_asked(), 0, "and nobody was asked");

    world.clock.advance(Duration::days(5));
    address.says(Answer::Text(catalogue_of("2026-10-10", "3")));
    assert_eq!(
        run().await,
        Outcome::NotNewer {
            fetched: day(2026, 10, 10),
            in_force: day(2026, 10, 10)
        },
        "the same date is not newer"
    );
    assert_eq!(in_force(&world).await.3, "1", "and changes nothing");

    address.says(Answer::Text(catalogue_of("2026-11-01", "3")));
    let Outcome::Loaded(loaded) = run().await else {
        panic!("a newer catalogue is loaded");
    };
    assert_eq!(loaded.replaced, Some(day(2026, 10, 10)));
    let (origin, as_of, _, input, by) = in_force(&world).await;
    assert_eq!(
        (origin.as_str(), as_of, input.as_str()),
        ("fetched", day(2026, 11, 1), "3")
    );
    assert_eq!(by, None, "the schedule is the system, not a person");

    assert_eq!(
        run().await,
        Outcome::NotDue,
        "and the seven days begin again from the day it was loaded"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn where_the_days_between_fetches_are_none_the_schedule_never_fetches() {
    let (world, address) = world_with_a_catalogue_and_an_address().await;
    address.says(Answer::Text(catalogue_of("2026-11-01", "3")));
    world.clock.advance(Duration::days(400));

    let outcome = catalogue::scheduled(&world.db(), &world.clock)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome, Outcome::NotDue);
    assert_eq!(address.times_asked(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_catalogue_with_one_wrong_entry_is_refused_naming_it_and_the_one_in_force_stays() {
    let (world, address) = world_with_a_catalogue_and_an_address().await;
    address.says(Answer::Text(catalogue_with_a_broken_second_entry(
        "2026-11-01",
    )));

    let refused = catalogue::refresh(
        &world.db(),
        &world.clock,
        Actor::User(world.admin),
        &Origin::default(),
        Wanted::Always,
    )
    .await;

    let Err(CatalogueError::Refused(reason)) = refused else {
        panic!("expected a refusal, not {refused:?}");
    };
    assert!(
        reason.contains("other-model*") || reason.contains("two"),
        "the entry that is wrong is named: {reason}"
    );
    let (origin, as_of, _, input, _) = in_force(&world).await;
    assert_eq!(
        (origin.as_str(), as_of, input.as_str()),
        ("shipped", day(2026, 10, 10), "1")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_address_that_redirects_or_fails_gives_no_catalogue_and_changes_nothing() {
    let (world, address) = world_with_a_catalogue_and_an_address().await;
    let ask = || async {
        catalogue::refresh(
            &world.db(),
            &world.clock,
            Actor::System,
            &Origin::default(),
            Wanted::Always,
        )
        .await
    };

    address.says(Answer::Redirect("http://127.0.0.1:9/elsewhere".to_owned()));
    assert!(
        matches!(ask().await, Err(CatalogueError::Fetch(_))),
        "the one address is asked, and no other place"
    );
    address.says(Answer::Status(StatusCode::INTERNAL_SERVER_ERROR));
    assert!(matches!(ask().await, Err(CatalogueError::Fetch(_))));
    address.says(Answer::Text("x".repeat(2 * 1024 * 1024 + 1)));
    assert!(
        matches!(ask().await, Err(CatalogueError::Fetch(_))),
        "an answer larger than an upload may be is not read"
    );
    address.says(Answer::Text("not a catalogue".to_owned()));
    assert!(matches!(ask().await, Err(CatalogueError::Refused(_))));

    assert_eq!(in_force(&world).await.0, "shipped");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_run_of_the_fetch_finds_the_lease_of_the_first_taken() {
    let (world, address) = world_with_a_catalogue_and_an_address().await;
    set_system(&world, "usage.catalogue_refresh_days", json!(1)).await;
    address.says(Answer::Text(catalogue_of("2026-11-01", "3")));
    world.clock.advance(Duration::days(2));
    let first = orch_db::queries::job_leases::Lease::new(
        Job::RateCatalogue.key(),
        "the first run",
        Duration::minutes(10),
    );
    assert!(
        first
            .hold(&world.db(), orch_types::Clock::now(&world.clock))
            .await
            .unwrap(),
        "the first run takes the lease"
    );

    assert!(
        catalogue::scheduled(&world.db(), &world.clock)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(address.times_asked(), 0);
    first.release(&world.db()).await.unwrap();
    assert!(
        catalogue::scheduled(&world.db(), &world.clock)
            .await
            .unwrap()
            .is_some(),
        "once the first run ended the next one fetches"
    );
    assert_eq!(address.times_asked(), 1);
}
