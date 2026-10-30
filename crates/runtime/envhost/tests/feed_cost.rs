//! Processor time of an idle environment host before and during its five-second measurement watches.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[cfg(target_os = "linux")]
use orch_envhost::blobs::MemoryBlobs;
#[cfg(target_os = "linux")]
use orch_envhost::environment::{EnvHost, Gates, HostSettings, Reports};
#[cfg(target_os = "linux")]
use orch_link_contract::commands::{EnvAttach, IndexMode, WatchSet};
#[cfg(target_os = "linux")]
use orch_types::{SystemClock, new_id};
#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

/// Four reporting periods reduce the effect of scheduler startup and the system's CPU tick granularity.
#[cfg(target_os = "linux")]
const WINDOW: Duration = Duration::from_secs(20);
/// The browser's measurement interval, shared by statistics and process snapshots.
#[cfg(target_os = "linux")]
const INTERVAL_MS: u32 = 5_000;

#[cfg(target_os = "linux")]
fn processor_ticks() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    // After the command's closing parenthesis, fields 14 and 15 are the user's and kernel's ticks.
    fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
}

#[cfg(target_os = "linux")]
async fn measure() -> f64 {
    let hz = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .unwrap();
    let hz: f64 = std::str::from_utf8(&hz.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let before = processor_ticks();
    let began = Instant::now();
    tokio::time::sleep(WINDOW).await;
    (processor_ticks() - before) as f64 / hz / began.elapsed().as_secs_f64() * 100.0
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_the_incremental_processor_cost_of_idle_measurement_watches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let mut settings =
        HostSettings::connected(dir.path().join("data"), vec![dir.path().to_path_buf()]);
    settings.gates = Gates::OPEN;
    let (reports, mut receiver) = Reports::channel(64);
    let host = EnvHost::new(
        settings,
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        vec![],
    );
    let environment = new_id();
    host.env_attach(
        &EnvAttach {
            resources: None,
            data_generation: None,
            storage: None,
            data_sources: None,
            environment_id: environment,
            workspace_root: Some(root.to_string_lossy().into_owned()),
            ignore: vec![],
            index: IndexMode::Full,
            mounts: None,
            variables: BTreeMap::new(),
            secrets: BTreeMap::new(),
        },
        &BTreeMap::new(),
    )
    .await
    .unwrap();
    let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    // Let the initial indexing finish before measuring the idle baseline.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let idle = measure().await;
    host.watch_set(&WatchSet {
        environment_id: environment,
        stats: true,
        procs: true,
        ports: false,
        terminals: false,
        interval_ms: INTERVAL_MS,
    })
    .await
    .unwrap();
    let watching = measure().await;
    host.watch_set(&WatchSet {
        environment_id: environment,
        stats: false,
        procs: false,
        ports: false,
        terminals: false,
        interval_ms: INTERVAL_MS,
    })
    .await
    .unwrap();
    println!(
        "idle_cpu_percent={idle:.4} watched_cpu_percent={watching:.4} incremental_cpu_percent={:.4} window_secs={}",
        (watching - idle).max(0.0),
        WINDOW.as_secs()
    );
    host.env_detach(&orch_link_contract::commands::EnvDetach {
        environment_id: environment,
    })
    .await
    .unwrap();
    drain.abort();
}
