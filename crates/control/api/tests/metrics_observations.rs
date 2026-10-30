//! A person reads stored observations of actual requests, live feeds and counted ledger work.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod realtime_support;
mod resources;
mod support;
mod usage_world;

use chrono::Duration;
use orch_api_contract::admin::AdminMetricList;
use orch_api_core::registry::Registry;
use orch_types::{Time, new_id};
use support::World;
use tower::ServiceExt as _;
use usage_world::{Use, spend, team};

#[tokio::test]
async fn real_work_reaches_exporter_and_management_without_recounting_trace_or_old_usage() {
    // One recorder per integration-test executable, as in a service process.
    let readings = orch_telemetry::install_metrics().unwrap();
    let world = World::new().await;
    let team = team(&world).await;
    {
        let mut conn = world.conn().await;
        orch_db::migrations::ensure_partitions(&mut conn, world.now())
            .await
            .unwrap();
    }
    spend(
        &world,
        &team,
        &Use::of(&world, team.boss.id(), 10).rated("0.25", "provider"),
    )
    .await;
    spend(
        &world,
        &team,
        &Use::of(&world, team.boss.id(), 999)
            .rated("9", "provider")
            .trace(),
    )
    .await;
    orch_worker_core::gauges::sample(&mut *world.conn().await, world.now())
        .await
        .unwrap();
    let _ = readings.take_samples(); // Existing usage is a baseline, not all charged to this minute.
    spend(
        &world,
        &team,
        &Use::of(&world, team.boss.id(), 100).rated("0.125", "provider"),
    )
    .await;
    spend(&world, &team, &Use::of(&world, team.boss.id(), 12)).await;
    {
        let mut conn = world.conn().await;
        sqlx::query("INSERT INTO usage_caps (id,meter,period,amount,created_by) VALUES ($1,'tokens','total',50,$2)")
            .bind(new_id()).bind(team.boss.id()).execute(&mut *conn).await.unwrap();
        orch_worker_core::gauges::sample(&mut conn, world.now())
            .await
            .unwrap();
    }
    world.app.app.engine.measure_caps().await.unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = orch_api_core::server::router(world.app.app.clone(), registry);
    let response = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/admin/system")
                .header("authorization", format!("Bearer {}", team.sysadmin.token))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    world.app.app.realtime.start(world.app.app.engine.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    let stopping = stop.clone();
    let server = tokio::spawn(orch_api_core::server::serve_plain(
        listener,
        router,
        async move {
            stopping.cancelled().await;
        },
    ));
    let mut browser = realtime_support::Client::signed_in(address, &team.boss.token).await;
    browser.sub(1, &format!("topic:{}", team.topic), None).await;
    // An admitted feed answers `subscribed` and sends what changes from there on; what it held
    // before is read over HTTP.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let frame = browser.frame().await;
            if frame["op"] == "subscribed" {
                assert_eq!(frame["mode"], "delta", "{frame}");
                break;
            }
            assert_ne!(frame["op"], "error", "{frame}");
        }
    })
    .await
    .expect("the feed is admitted");
    let samples = readings.take_samples();
    let one = |name: &str| {
        samples
            .iter()
            .find(|row| {
                row.name == name
                    && row
                        .labels
                        .get("purpose")
                        .is_none_or(|purpose| purpose == "turn")
                    && row
                        .labels
                        .get("source")
                        .is_none_or(|source| source == "provider")
            })
            .unwrap()
    };
    assert_eq!(one("orch_usage_tokens_total").value, 112.0);
    assert_eq!(one("orch_usage_unrated_tokens_total").value, 12.0);
    assert_eq!(one("orch_usage_credits_total").value, 0.125);
    assert_eq!(one("orch_http_requests_total").value, 1.0);
    assert_eq!(one("orch_ws_connections").value, 1.0);
    assert_eq!(one("orch_ws_subscriptions").value, 1.0);
    assert_eq!(
        samples
            .iter()
            .find(|row| row.name == "orch_usage_caps"
                && row.labels["meter"] == "tokens"
                && row.labels["state"] == "reached")
            .unwrap()
            .value,
        1.0
    );
    assert!(!samples.iter().any(|row| {
        row.labels
            .values()
            .any(|value| value.contains(&team.boss.token))
    }));
    let rendered = readings.render();
    assert!(rendered.contains("# TYPE orch_usage_credits_total counter"));
    assert!(rendered.contains("orch_ws_subscriptions"));
    let process = new_id();
    {
        let mut conn = world.conn().await;
        orch_db::queries::metrics::write_process_samples(
            &mut conn,
            world.now(),
            "api",
            process,
            &samples,
        )
        .await
        .unwrap();
    }
    let query = format!(
        "name=usage_tokens_total&name=usage_credits_total&step=1&from={}&to={}",
        Time::new(world.now() - Duration::minutes(1)),
        Time::new(world.now() + Duration::minutes(1))
    );
    let series = world
        .app
        .call(AdminMetricList, &[])
        .token(&team.sysadmin.token)
        .raw_query(&query)
        .send()
        .await
        .ok();
    assert_eq!(
        series
            .iter()
            .find(|row| row.name == "usage_tokens_total" && row.labels["purpose"] == "turn")
            .unwrap()
            .points[0]
            .1,
        112.0
    );
    assert_eq!(
        series
            .iter()
            .find(|row| row.name == "usage_credits_total"
                && row.labels["purpose"] == "turn"
                && row.labels["source"] == "provider")
            .unwrap()
            .points[0]
            .1,
        0.125
    );
    let next = readings.take_samples();
    assert!(
        next.iter()
            .filter(|row| row.counter)
            .all(|row| row.value == 0.0),
        "sampling itself repeats no work"
    );
    {
        let mut conn = world.conn().await;
        sqlx::query("DELETE FROM usage_caps")
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    world.app.app.engine.measure_caps().await.unwrap();
    assert!(
        readings
            .take_samples()
            .iter()
            .filter(|row| row.name == "orch_usage_caps")
            .all(|row| row.value == 0.0)
    );
    drop(browser);
    stop.cancel();
    server.await.unwrap().unwrap();
    world.app.app.realtime.stop().await;
}
