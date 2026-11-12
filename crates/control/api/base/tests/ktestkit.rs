//! Test jobs reserve database capacity across processes and retain exact exit and cleanup evidence.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_testkit::database::{
    ADMISSION_WAIT_VARIABLE, CONNECTION_BUDGET_VARIABLE, CONNECTION_RESERVATION_VARIABLE,
    LEASE_DIRECTORY_VARIABLE, RUN_ID_VARIABLE, TestDb, URL_VARIABLE,
};
use orch_types::{Id, Json, new_id};
use sqlx::{Connection, PgConnection, postgres::PgPoolOptions};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Match the fixture's administrative bound, including a slow database drop.
const PROOF_WAIT: Duration = Duration::from_secs(300);
/// A short local filesystem poll; it never consumes a database connection.
const PROOF_POLL: Duration = Duration::from_millis(50);
/// Exercise the largest secondary pool that existing API test worlds open.
const SECONDARY_POOL: u32 = 24;
/// Two fixture reservations, so the third worker must wait without connecting.
const TWO_FIXTURES: &str = "64";
/// Four primary, twenty-four secondary, and four administrative connections.
const FIXTURE_RESERVATION: &str = "32";
const WORKER: &str = "a_fixture_database_is_removed_after_its_pool_is_closed";
// Each proof has its own budget configuration. Serialize the proof bodies so
// their intentionally separate lease directories do not add overlapping load.
static PROOFS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn artifact_directory() -> PathBuf {
    let base = std::env::var_os(LEASE_DIRECTORY_VARIABLE)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("orch-test-databases"));
    let directory = base.join("proofs").join(new_id().to_string());
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn runner() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/check-test-jobs/run.py")
}

async fn wait_for_file(path: &Path) {
    tokio::time::timeout(PROOF_WAIT, async {
        while !path.exists() {
            tokio::time::sleep(PROOF_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "waiting for worker signal {} exceeded {PROOF_WAIT:?}",
            path.display()
        )
    });
}

fn lease_records(directory: &Path) -> Vec<Json> {
    let mut records = Vec::new();
    for scope in std::fs::read_dir(directory).unwrap().filter_map(Result::ok) {
        let path = scope.path().join("records");
        if path.is_dir() {
            for file in std::fs::read_dir(path).unwrap().filter_map(Result::ok) {
                records.push(serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap());
            }
        }
    }
    records
}

/// Keep pids of these children; a failed assertion leaves no process running.
#[derive(Default)]
struct Workers(Vec<Child>);

impl Drop for Workers {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn worker(directory: &Path, signal: Id, budget: &str) -> Child {
    let log = File::create(directory.join(format!("{signal}.log"))).unwrap();
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", WORKER, "--nocapture"])
        .env(LEASE_DIRECTORY_VARIABLE, directory)
        .env(CONNECTION_BUDGET_VARIABLE, budget)
        .env(CONNECTION_RESERVATION_VARIABLE, FIXTURE_RESERVATION)
        .env(ADMISSION_WAIT_VARIABLE, PROOF_WAIT.as_secs().to_string())
        .env(
            RUN_ID_VARIABLE,
            std::env::var(RUN_ID_VARIABLE).unwrap_or_else(|_| new_id().to_string()),
        )
        .env("ORCH_KTESTKIT_SIGNAL", directory.join(signal.to_string()))
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap()
}

async fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    tokio::time::timeout(PROOF_WAIT, async {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            tokio::time::sleep(PROOF_POLL).await;
        }
    })
    .await
    .expect("the owned worker to exit after its database is removed")
}

#[tokio::test]
async fn a_fixture_database_is_removed_after_its_pool_is_closed() {
    let _proof = PROOFS.lock().await;
    let signal = std::env::var_os("ORCH_KTESTKIT_SIGNAL").map(PathBuf::from);
    if let Some(signal) = &signal {
        std::fs::write(signal.with_extension("attempting"), "opening a fixture").unwrap();
    }
    let db = TestDb::fresh().await;
    let secondary = PgPoolOptions::new()
        .min_connections(SECONDARY_POOL)
        .max_connections(SECONDARY_POOL)
        .connect(&db.url)
        .await
        .unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let mut held = Vec::new();
    for _ in 0..db.pool.options().get_max_connections() {
        held.push(db.pool.acquire().await.unwrap());
    }
    if let Ok(exit) = std::env::var("ORCH_KTESTKIT_LEAK") {
        println!("test result: ok. 4 passed; 0 failed; 0 ignored;");
        // A stopped test process cannot run a Rust destructor. The runner must
        // retain the real exit and the exact database record instead of passing it.
        std::process::exit(exit.parse().unwrap());
    }
    if let Some(signal) = &signal {
        std::fs::write(signal.with_extension("ready"), &database).unwrap();
        wait_for_file(&signal.with_extension("release")).await;
    }
    drop(held);
    secondary.close().await;
    db.close().await.unwrap();
    let url = std::env::var(URL_VARIABLE)
        .unwrap_or_else(|_| "postgres://orch:orch@127.0.0.1:55432/postgres".to_owned());
    let mut admin = PgConnection::connect(&url).await.unwrap();
    let remains: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&database)
            .fetch_one(&mut admin)
            .await
            .unwrap();
    assert!(
        !remains,
        "the exact fixture database is gone before the worker exits"
    );
}

#[tokio::test]
async fn concurrent_test_processes_keep_owned_database_connections_within_the_configured_budget() {
    let _proof = PROOFS.lock().await;
    let directory = artifact_directory();
    let signals = [new_id(), new_id(), new_id()];
    let mut children = Workers::default();
    for signal in &signals[..2] {
        children.0.push(worker(&directory, *signal, TWO_FIXTURES));
        wait_for_file(&directory.join(signal.to_string()).with_extension("ready")).await;
    }
    children
        .0
        .push(worker(&directory, signals[2], TWO_FIXTURES));
    wait_for_file(
        &directory
            .join(signals[2].to_string())
            .with_extension("attempting"),
    )
    .await;
    let records = lease_records(&directory);
    assert_eq!(records.len(), 2, "a waiting process owns no database yet");
    let names: Vec<_> = records
        .iter()
        .map(|record| record["database"].as_str().unwrap().to_owned())
        .collect();
    let url = std::env::var(URL_VARIABLE)
        .unwrap_or_else(|_| "postgres://orch:orch@127.0.0.1:55432/postgres".to_owned());
    let mut admin = PgConnection::connect(&url).await.unwrap();
    let waiting_connections: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name = $1")
            .bind(format!("orch_testkit_{}", children.0[2].id()))
            .fetch_one(&mut admin)
            .await
            .unwrap();
    assert_eq!(
        waiting_connections, 0,
        "admission waits without a PostgreSQL connection"
    );
    let connections: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = ANY($1)")
            .bind(&names)
            .fetch_one(&mut admin)
            .await
            .unwrap();
    std::fs::write(
        directory.join("observed.json"),
        serde_json::to_vec(&serde_json::json!({
            "connection_budget": TWO_FIXTURES.parse::<u32>().unwrap(),
            "observed_owned_connections": connections,
            "waiting_worker_connections": waiting_connections,
            "owned_databases": names,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        connections >= i64::from(SECONDARY_POOL) * 2,
        "both actual secondary pools are open: {connections}"
    );
    assert!(
        connections <= TWO_FIXTURES.parse::<i64>().unwrap(),
        "owned connection count {connections} stays within its budget"
    );
    assert!(
        !directory
            .join(signals[2].to_string())
            .with_extension("ready")
            .exists()
    );
    std::fs::write(
        directory
            .join(signals[0].to_string())
            .with_extension("release"),
        "done",
    )
    .unwrap();
    wait_for_file(
        &directory
            .join(signals[2].to_string())
            .with_extension("ready"),
    )
    .await;
    for signal in &signals[1..] {
        std::fs::write(
            directory.join(signal.to_string()).with_extension("release"),
            "done",
        )
        .unwrap();
    }
    for child in &mut children.0 {
        assert!(wait_for_exit(child).await.success());
    }
    let records = lease_records(&directory);
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|record| record["state"] == "dropped"));
}

#[tokio::test]
async fn a_failed_process_keeps_its_exit_and_only_its_owned_orphan_is_cleaned() {
    exit_and_owned_cleanup("1", 1).await;
}

#[tokio::test]
async fn a_successful_process_with_an_owned_orphan_fails_the_job_until_exact_cleanup_succeeds() {
    exit_and_owned_cleanup("0", 2).await;
}

async fn exit_and_owned_cleanup(process_exit: &str, job_exit: i32) {
    let _proof = PROOFS.lock().await;
    let unrelated = TestDb::fresh().await;
    let directory = artifact_directory();
    let output = directory.join("failed-job");
    let result = tokio::process::Command::new("python3")
        .arg(runner())
        .arg("--output-dir")
        .arg(&output)
        .arg("--lease-dir")
        .arg(&directory)
        .arg("--")
        // A UUID's uppercase spelling identifies the same owned job. This is
        // accepted by TestDb too, so inspection must retain that ownership.
        .arg("python3")
        .arg("-c")
        .arg("import os, sys; os.environ['ORCH_TEST_DATABASE_RUN_ID'] = os.environ['ORCH_TEST_DATABASE_RUN_ID'].upper(); os.execv(sys.argv[1], sys.argv[1:])")
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", WORKER, "--nocapture"])
        .env("ORCH_KTESTKIT_LEAK", process_exit)
        .output()
        .await
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(job_exit),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Json =
        serde_json::from_slice(&std::fs::read(output.join("result.json")).unwrap()).unwrap();
    assert_eq!(report["process_exit"], process_exit.parse::<i32>().unwrap());
    assert_eq!(report["test_results"][0]["passed"], 4);
    assert_eq!(report["remaining_databases"].as_array().unwrap().len(), 1);
    let cleaned = tokio::process::Command::new("python3")
        .arg(runner())
        .arg("--output-dir")
        .arg(directory.join("cleanup"))
        .arg("--lease-dir")
        .arg(&directory)
        .arg("--cleanup-run")
        .arg(report["run_id"].as_str().unwrap())
        .output()
        .await
        .unwrap();
    assert!(
        cleaned.status.success(),
        "{}",
        String::from_utf8_lossy(&cleaned.stderr)
    );
    let cleanup: Json = serde_json::from_slice(&cleaned.stdout).unwrap();
    assert_eq!(cleanup["exact_cleanup"], report["remaining_databases"]);
    assert_eq!(cleanup["remaining_databases"], serde_json::json!([]));
    let still_here: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&unrelated.pool)
        .await
        .unwrap();
    assert_eq!(still_here, 1, "the unrelated fixture is untouched");
    unrelated.close().await.unwrap();
}

#[tokio::test]
async fn admission_timeout_names_the_lease_it_waited_for_and_creates_no_orphan() {
    let _proof = PROOFS.lock().await;
    let directory = artifact_directory();
    let signal = new_id();
    let mut children = Workers(vec![worker(&directory, signal, FIXTURE_RESERVATION)]);
    wait_for_file(&directory.join(signal.to_string()).with_extension("ready")).await;
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", WORKER, "--nocapture"])
        .env(LEASE_DIRECTORY_VARIABLE, &directory)
        .env(CONNECTION_BUDGET_VARIABLE, FIXTURE_RESERVATION)
        .env(CONNECTION_RESERVATION_VARIABLE, FIXTURE_RESERVATION)
        // This test deliberately expires admission while its only slot is owned.
        .env(ADMISSION_WAIT_VARIABLE, "1")
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostic.contains("waiting for a test database lease exceeded"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("all 1 slots reserve 32 of 32 connections"),
        "{diagnostic}"
    );
    assert_eq!(
        lease_records(&directory).len(),
        1,
        "the timed-out waiter created no database"
    );
    std::fs::write(
        directory.join(signal.to_string()).with_extension("release"),
        "done",
    )
    .unwrap();
    assert!(wait_for_exit(&mut children.0[0]).await.success());
    assert_eq!(lease_records(&directory)[0]["state"], "dropped");
}
