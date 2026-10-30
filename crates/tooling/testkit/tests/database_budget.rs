//! Database admission follows the real server's capacity without configuring a budget.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_testkit::database::{
    CONNECTION_BUDGET_VARIABLE, CONNECTION_RESERVATION_VARIABLE, LEASE_DIRECTORY_VARIABLE,
    RUN_ID_VARIABLE, TestDb, URL_VARIABLE,
};
use orch_types::new_id;
use rand::RngExt;
use sqlx::{Connection, PgConnection};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Matches the administrative cleanup guard; this prevents hanging, not a speed assertion.
const PATIENCE: Duration = Duration::from_secs(300);
/// Polling local readiness does not assert that a worker wins a scheduling race.
const POLL: Duration = Duration::from_millis(50);
/// Keep these disposable services within the development listener range.
const PORTS: std::ops::RangeInclusive<u16> = 41000..=41999;
/// This worker's ordinary test also verifies one real fixture on the configured server.
const WORKER: &str = "fixture_databases_keep_their_schema_until_they_are_closed";
const COUNT_VARIABLE: &str = "ORCH_TESTKIT_CAPACITY_COUNT";

struct Server {
    name: String,
    url: String,
}

impl Server {
    async fn start(connections: u32) -> Self {
        let name = format!("orch-testkit-bounds-{}", new_id());
        let port = loop {
            let port = rand::rng().random_range(PORTS);
            if TcpListener::bind(("127.0.0.1", port)).is_ok() {
                break port;
            }
        };
        let server = Self {
            name,
            url: format!("postgres://orch:orch@127.0.0.1:{port}/postgres"),
        };
        let status = Command::new("timeout")
            .args(["300s", "docker", "run", "--detach", "--pull=never"])
            .args([
                "--name",
                &server.name,
                "--label",
                "orch.package=testkit-bounds",
            ])
            .args(["--publish", &format!("127.0.0.1:{port}:5432")])
            // Thirty migrated copies fit in this bounded temporary data volume.
            .args(["--tmpfs", "/var/lib/postgresql/data:rw,size=1536m"])
            // PostgreSQL's connection table needs more than Docker's default shared memory.
            .args(["--shm-size", "256m"])
            .args([
                "--env",
                "POSTGRES_USER=orch",
                "--env",
                "POSTGRES_PASSWORD=orch",
            ])
            .args([
                "postgres:17-alpine",
                "-c",
                &format!("max_connections={connections}"),
            ])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "the owned PostgreSQL server starts");
        tokio::time::timeout(PATIENCE, async {
            loop {
                if let Ok(connection) = PgConnection::connect(&server.url).await {
                    connection.close().await.unwrap();
                    break;
                }
                tokio::time::sleep(POLL).await;
            }
        })
        .await
        .expect("the owned PostgreSQL server becomes ready");
        server
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let status = Command::new("timeout")
            .args(["300s", "docker", "rm", "--force", &self.name])
            .stdout(Stdio::null())
            .status();
        assert!(
            status.is_ok_and(|status| status.success()),
            "the owned PostgreSQL container is removed"
        );
    }
}

fn records(directory: &Path) -> Vec<serde_json::Value> {
    let mut records = Vec::new();
    for scope in std::fs::read_dir(directory).unwrap() {
        let record_directory = scope.unwrap().path().join("records");
        if record_directory.is_dir() {
            for file in std::fs::read_dir(record_directory).unwrap() {
                records.push(
                    serde_json::from_slice(&std::fs::read(file.unwrap().path()).unwrap()).unwrap(),
                );
            }
        }
    }
    records
}

async fn admits_fixtures(connections: u32, expected: usize) {
    let server = Server::start(connections).await;
    let directory = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", WORKER, "--nocapture"])
        .env(URL_VARIABLE, &server.url)
        .env_remove(CONNECTION_BUDGET_VARIABLE)
        .env_remove(CONNECTION_RESERVATION_VARIABLE)
        .env(LEASE_DIRECTORY_VARIABLE, directory.path())
        .env(RUN_ID_VARIABLE, new_id().to_string())
        .env(COUNT_VARIABLE, expected.to_string())
        .kill_on_drop(true);
    let output = tokio::time::timeout(PATIENCE, command.output())
        .await
        .expect("all fixture databases open and close without hanging")
        .unwrap();
    assert!(
        output.status.success(),
        "capacity worker: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let records = records(directory.path());
    assert_eq!(records.len(), expected);
    assert!(records.iter().all(|record| record["state"] == "dropped"));
    for record in records {
        let budget = record["budget"]["connections"].as_u64().unwrap();
        let reservation = record["budget"]["per_fixture"].as_u64().unwrap();
        assert_eq!(budget / reservation, expected as u64);
    }
}

#[tokio::test]
async fn a_server_with_twelve_hundred_connections_admits_thirty_unconfigured_fixtures() {
    admits_fixtures(1200, 30).await;
}

#[tokio::test]
async fn a_server_with_one_hundred_connections_admits_three_unconfigured_fixtures() {
    admits_fixtures(100, 3).await;
}

#[tokio::test]
async fn fixture_databases_keep_their_schema_until_they_are_closed() {
    let count: usize = std::env::var(COUNT_VARIABLE)
        .map(|value| value.parse().unwrap())
        .unwrap_or(1);
    // Start competing admissions together, then retain every fixture until all
    // have their schema. A four-slot default would deadlock the thirty-fixture proof.
    let fixtures = futures::future::join_all((0..count).map(|_| TestDb::fresh())).await;
    let mut names = Vec::new();
    for fixture in &fixtures {
        let name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        names.push(name);
        let has_schema: bool = sqlx::query_scalar("SELECT to_regclass('nodes') IS NOT NULL")
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        assert!(has_schema, "each concurrently held fixture has its schema");
    }
    for fixture in fixtures {
        fixture.close().await.unwrap();
    }
    let mut admin = PgConnection::connect(&std::env::var(URL_VARIABLE).unwrap())
        .await
        .unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_database WHERE datname = ANY($1)")
            .bind(names)
            .fetch_one(&mut admin)
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "all exact owned fixture databases are removed"
    );
    admin.close().await.unwrap();
}

#[tokio::test]
async fn independently_opened_fixture_connections_bound_statements_and_detect_disconnected_clients()
{
    let fixture = TestDb::fresh().await;
    let mut independent = PgConnection::connect(&fixture.url).await.unwrap();
    let mut pooled = fixture.pool.acquire().await.unwrap();
    for conn in [&mut *pooled, &mut independent] {
        let limits: (i64, i64) = sqlx::query_as(
            "SELECT (extract(epoch FROM current_setting('statement_timeout')::interval)*1000)::bigint,
                    (extract(epoch FROM current_setting('client_connection_check_interval')::interval)*1000)::bigint",
        )
        .fetch_one(conn)
        .await
        .unwrap();
        assert_eq!(limits, (600_000, 5_000));
    }
    drop(pooled);
    // Exercise cancellation with a shorter local limit instead of sleeping for ten minutes.
    sqlx::query("SET statement_timeout = '100ms'")
        .execute(&mut independent)
        .await
        .unwrap();
    let error = sqlx::query("SELECT pg_sleep(5)")
        .execute(&mut independent)
        .await
        .unwrap_err();
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("57014")
    );
    independent.close().await.unwrap();
    fixture.close().await.unwrap();
}
