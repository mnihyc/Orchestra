//! Directory measurement cannot hold the ordinary watched facts publisher.
use super::*;
use orch_api_contract::storage::MachineDataInventory;
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Hub, TestPki};
use orch_types::{ManualClock, Time, new_id};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

const WITHIN: Duration = Duration::from_secs(20);

fn partial(at: Time) -> MachineDataInventory {
    MachineDataInventory {
        observed_at: at,
        complete: false,
        parts: Vec::new(),
        unlisted_parts: 0,
        problem: None,
    }
}

fn clock() -> Arc<dyn Clock> {
    let at: Time = "2026-11-08T06:00:00Z".parse().unwrap();
    Arc::new(ManualClock::new(at.get()))
}

#[tokio::test]
async fn watched_policy_facts_publish_while_directory_measurement_is_held() {
    let data = tempfile::tempdir().unwrap();
    let clock = clock();
    let pki = TestPki::new(Arc::clone(&clock));
    let machine = new_id();
    let issued = pki.machine(machine);
    let identity = MachineIdentity {
        machine_id: machine,
        key: issued.key,
        certificate: issued.certificate.clone(),
        authority: pki.certificate(),
    };
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let controller = ScriptedController::attach(&hub, Arc::clone(&clock), machine, Vec::new());
    let config: ConnectorConfig =
        orch_config::load_text("[mcp]\nimport = false\n", "connector.toml", |_| None).unwrap();
    let link = MachineLink::start(
        machine_config(LinkSettings {
            gateways: Vec::new(),
            version: "0.1.0",
            platform: crate::session::detect_platform(),
            roles: Vec::new(),
            identity: &identity,
            endpoints: vec![Hub::endpoint()],
            data_dir: data.path(),
            config: &config,
            follow_system_endpoints: true,
            hosted_endpoints: None,
            connected: true,
        })
        .unwrap(),
        hub.dialer(Some(issued.certificate)),
        Arc::clone(&clock),
    )
    .unwrap();
    let path = data.path().join("connector.toml");
    std::fs::write(&path, "[mcp]\nimport = false\n").unwrap();
    let policy = Arc::new(PolicyStore::connected(config, Some(path.clone())).unwrap());
    assert!(!policy.policy().unwrap().mcp_import);
    let directory = Arc::new(DataDirectoryFacts::new(data.path().to_owned(), clock));
    let discovery = Arc::new(crate::local_harnesses::Discovery::new(Arc::new(
        AdapterRegistry::new(),
    )));
    let host = Host::detect();
    let cached = Arc::clone(&directory);
    let current_policy = Arc::clone(&policy);
    let facts: Arc<crate::dispatch::FactsNow> = Arc::new(move || {
        let mut facts = machine_facts(
            &host,
            FactsDraft::default(),
            current_policy.policy(),
            Default::default(),
            0,
        );
        facts.data_directory = cached.observed();
        facts
    });
    let stop = CancellationToken::new();
    let entered = Arc::new(Notify::new());
    let started = Arc::clone(&entered);
    let observer = tokio::spawn(Arc::clone(&directory).observe_with(
        stop.clone(),
        move |state, at, cancelled| {
            let started = Arc::clone(&started);
            async move {
                state.remember(partial(at));
                started.notify_one();
                cancelled.cancelled().await;
            }
        },
    ));
    tokio::time::timeout(WITHIN, entered.notified())
        .await
        .unwrap();
    let reporter = tokio::spawn(report_facts(
        link.clone(),
        facts,
        policy.changes(),
        Arc::clone(&policy),
        discovery,
        Arc::clone(&directory),
        stop.clone(),
    ));
    assert!(
        controller
            .wait_until(WITHIN, |c| c.applied("machine").iter().any(|event| {
                event.kind == "machine.facts" && event.body["data_directory"]["complete"] == false
            }))
            .await
    );
    std::fs::write(&path, "[mcp]\nimport = true\n").unwrap();
    assert!(policy.reload_from_file().unwrap());
    assert!(
        controller
            .wait_until(WITHIN, |c| c.applied("machine").iter().any(|event| {
                event.kind == "machine.facts" && event.body["policy"]["mcp.import"] == true
            }))
            .await,
        "the actual watched policy must be published before the measurement ends"
    );
    assert!(!directory.observed().unwrap().complete);
    stop.cancel();
    tokio::time::timeout(WITHIN, observer)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(WITHIN, reporter)
        .await
        .unwrap()
        .unwrap();
    link.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn directory_observation_never_overlaps_and_stop_ends_a_held_measurement() {
    let data = tempfile::tempdir().unwrap();
    let directory = Arc::new(DataDirectoryFacts::new(data.path().to_owned(), clock()));
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let entered = Arc::new(Notify::new());
    let started = Arc::clone(&entered);
    let stop = CancellationToken::new();
    let observer = tokio::spawn(Arc::clone(&directory).observe_with(
        stop.clone(),
        move |state, at, cancelled| {
            let counted = Arc::clone(&counted);
            let started = Arc::clone(&started);
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                state.remember(partial(at));
                started.notify_one();
                cancelled.cancelled().await;
            }
        },
    ));
    entered.notified().await;
    tokio::time::advance(FACTS_EVERY * 3).await;
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    stop.cancel();
    tokio::time::timeout(WITHIN, observer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!directory.observed().unwrap().complete);
}
