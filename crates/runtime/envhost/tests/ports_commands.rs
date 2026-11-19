//! Attached environments answer fresh port reads as live observations rather than command-journal measurements.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use orch_envhost::blobs::MemoryBlobs;
use orch_envhost::environment::{EnvHost, HostSettings, Report, Reported, Reports};
use orch_link_contract::commands::{EnvAttach, EnvDetach, IndexMode};
use orch_link_contract::ports::PortsLook;
use orch_types::{SystemClock, new_id};

#[tokio::test]
async fn a_port_read_returns_only_acceptance_and_its_fresh_generation_bound_snapshot_is_live() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings =
        HostSettings::connected(dir.path().join("data"), vec![dir.path().to_owned()]);
    settings.generation = 7;
    let (reports, mut receiver) = Reports::channel(16);
    let host = EnvHost::new(
        settings,
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        Vec::new(),
    );
    let environment_id = new_id();
    host.env_attach(
        &EnvAttach {
            resources: None,
            data_generation: None,
            storage: None,
            data_sources: None,
            environment_id,
            workspace_root: Some(dir.path().to_string_lossy().into_owned()),
            ignore: Vec::new(),
            index: IndexMode::Live,
            mounts: Some(Vec::new()),
            variables: BTreeMap::new(),
            secrets: BTreeMap::new(),
        },
        &BTreeMap::new(),
    )
    .await
    .unwrap();
    let request_ref = new_id();
    let accepted = host
        .ports_look(&PortsLook {
            environment_id,
            generation: 7,
            request_ref,
        })
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(accepted).unwrap(),
        serde_json::json!({"accepted":true})
    );
    let reported = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(Reported {
            environment_id: found,
            report,
        }) = receiver.recv().await
        {
            if matches!(&report, Report::Ports(_)) {
                assert_eq!(found, environment_id);
                assert!(!report.is_durable());
                if let Report::Ports(snapshot) = report {
                    return snapshot;
                }
            }
        }
        panic!("the live reader did not report");
    })
    .await
    .unwrap();
    assert_eq!(reported.request_ref, Some(request_ref));
    assert_eq!(reported.snapshot.environment_id, environment_id);
    assert_eq!(reported.snapshot.generation, 7);

    let stale = host
        .ports_look(&PortsLook {
            environment_id,
            generation: 6,
            request_ref: new_id(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        stale.code(),
        orch_link_contract::commands::ErrorCode::Changed
    );
    host.env_detach(&EnvDetach { environment_id })
        .await
        .unwrap();
    assert!(
        host.ports_look(&PortsLook {
            environment_id,
            generation: 7,
            request_ref: new_id()
        })
        .await
        .is_err()
    );
}
