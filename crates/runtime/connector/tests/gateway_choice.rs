//! A server advertisement cannot rewrite the local primary selection.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]

use orch_connector_core::enrollment::{DataDir, EnrollmentRecord, RECORD_FILE};
use orch_link_contract::enrollment::{Endpoint, EnrollReplySystem, Trust};
use orch_link_contract::gateways::{GatewayChoice, GatewayInstance, GatewaySelection};
use orch_types::{Time, new_id};

#[test]
fn selected_gateway_choice_survives_advertisements_removal_and_process_restart() {
    let directory = tempfile::tempdir().unwrap();
    let data = DataDir::open(directory.path()).unwrap();
    let old = Endpoint {
        url: "wss://eu.test:9443".into(),
        trust: Trust::Pinned,
    };
    let record = EnrollmentRecord {
        gateway_measured_at: None,
        gateways: vec![GatewayInstance {
            id: "eu".into(),
            label: "Europe".into(),
            region: "Europe".into(),
            endpoints: vec![old.clone()],
        }],
        gateway_measurements: Vec::new(),
        server_endpoints: vec![old],
        machine_id: new_id(),
        name: "choice-fixture".into(),
        system: EnrollReplySystem {
            name: "Fixture".into(),
            public_url: "https://orch.test".into(),
        },
        pin: "fixture-pin".into(),
        protocol_version: orch_link_contract::version::PROTOCOL_VERSION,
        enrolled_at: "2026-11-06T08:00:00Z".parse::<Time>().unwrap(),
        managed: false,
    };
    std::fs::write(
        data.identity_dir().join(RECORD_FILE),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let selected = GatewayChoice {
        selection: GatewaySelection::Selected,
        gateway_id: Some("eu".into()),
    };
    data.update_config(|config| config.connector.gateway = selected.clone())
        .unwrap();
    let next = Endpoint {
        url: "wss://asia.test:9443".into(),
        trust: Trust::Pinned,
    };
    data.store_server_advertisement(
        Some(vec![next.clone()]),
        Some(vec![GatewayInstance {
            id: "asia".into(),
            label: "Asia tools".into(),
            region: "Asia".into(),
            endpoints: vec![next],
        }]),
        Some(Vec::new()),
        None,
    )
    .unwrap();
    let reopened = DataDir::inspect(directory.path()).unwrap();
    assert_eq!(
        reopened.config(|_| None).unwrap().connector.gateway,
        selected
    );
    assert_eq!(reopened.record().unwrap().gateways[0].id, "asia");
    assert_eq!(reopened.record().unwrap().machine_id, record.machine_id);
    assert_eq!(
        reopened
            .config(|_| None)
            .unwrap()
            .connector
            .gateway
            .selection,
        GatewaySelection::Selected
    );
}
