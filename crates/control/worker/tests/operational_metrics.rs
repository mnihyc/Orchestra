//! The exporter distinguishes an absent worker or backup from a fresh record and ages the last success.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::queries::installation;
use orch_testkit::{clock, database::TestDb};
use orch_types::new_id;
use serde_json::json;

fn gauge(readings: &orch_telemetry::Readings, name: &str, label: Option<&str>) -> f64 {
    readings
        .render()
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(' ')?;
            (key.starts_with(name) && label.is_none_or(|label| key.contains(label)))
                .then(|| value.parse::<f64>().unwrap())
        })
        .expect("the operational series is exported")
}

#[tokio::test]
async fn missing_records_are_explicit_and_a_later_failed_dump_does_not_refresh_the_backup_age() {
    let readings = orch_telemetry::install_metrics().unwrap();
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    let at = clock::start();
    orch_worker_core::gauges::operational(&mut conn, at)
        .await
        .unwrap();
    assert_eq!(
        gauge(
            &readings,
            "orch_component_heartbeat_present",
            Some("worker")
        ),
        0.0
    );
    assert_eq!(gauge(&readings, "orch_backup_present", None), 0.0);
    orch_db::heartbeat::beat(&mut conn, "worker", new_id(), "test", at, at)
        .await
        .unwrap();
    let mut publication = json!({"success":{"system_id":"disposable-test-system",
        "manifest_file":"fixture.manifest.json","manifest_hash":"0".repeat(64),
        "record":{"completed_at":at,"database_at":at,"release":"test","schema":"1","dump_name":"fixture.dump",
            "bytes":1,"hash":"0".repeat(64),"cause":"manual","database_only":true,"last_failure":null}},"last_failure":null});
    installation::put(&mut conn, "database_backup", &publication, at)
        .await
        .unwrap();
    let later = at + chrono::Duration::seconds(46);
    orch_worker_core::gauges::operational(&mut conn, later)
        .await
        .unwrap();
    assert_eq!(
        gauge(
            &readings,
            "orch_component_heartbeat_present",
            Some("worker")
        ),
        1.0
    );
    assert_eq!(
        gauge(
            &readings,
            "orch_component_heartbeat_age_seconds",
            Some("worker")
        ),
        46.0
    );
    assert_eq!(gauge(&readings, "orch_backup_present", None), 1.0);
    assert_eq!(gauge(&readings, "orch_backup_age_seconds", None), 46.0);
    publication["last_failure"] = json!({"at":later,"reason":"a later dump failed"});
    installation::put(&mut conn, "database_backup", &publication, later)
        .await
        .unwrap();
    orch_worker_core::gauges::operational(&mut conn, later + chrono::Duration::seconds(14))
        .await
        .unwrap();
    assert_eq!(
        gauge(&readings, "orch_backup_age_seconds", None),
        60.0,
        "the last successful pair is still the old one"
    );
}
