//! A server advertisement cannot rewrite the local primary selection.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]

use orch_connector_core::enrollment::{DataDir, EnrollmentRecord, RECORD_FILE};
use orch_connector_core::{gateway_control::Requests, policy::PolicyStore};
use orch_link_contract::enrollment::{Endpoint, EnrollReplySystem, Trust};
use orch_link_contract::gateways::{GatewayChoice, GatewayInstance, GatewaySelection};
use orch_types::{Time, new_id};
use std::sync::{Arc, Mutex};

fn fixture() -> (tempfile::TempDir, DataDir, EnrollmentRecord) {
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
        protocol_version: 1,
        enrolled_at: "2026-11-06T08:00:00Z".parse::<Time>().unwrap(),
        managed: false,
    };
    std::fs::write(
        data.identity_dir().join(RECORD_FILE),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    (directory, data, record)
}

#[test]
fn selected_gateway_choice_survives_advertisements_removal_and_process_restart() {
    let (directory, data, record) = fixture();
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

#[test]
fn a_remote_gateway_change_is_persisted_without_losing_identity_and_reconnects_only_after_its_answer()
 {
    let (_directory, data, record) = fixture();
    data.update_config(|config| {
        config.connector.gateway = GatewayChoice {
            selection: GatewaySelection::Selected,
            gateway_id: Some("eu".into()),
        }
    })
    .unwrap();
    let config = data.config(|_| None).unwrap();
    let policy = Arc::new(PolicyStore::connected(config, Some(data.config_path())).unwrap());
    let reconnected = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&reconnected);
    let requests = Requests::new(
        data.root().to_owned(),
        Arc::clone(&policy),
        Arc::new(move |choice| observed.lock().unwrap().push(choice)),
    );
    let id = new_id();
    let automatic = GatewayChoice::default();
    assert!(requests.request(id, &automatic).unwrap().changed);
    assert_eq!(data.config(|_| None).unwrap().connector.gateway, automatic);
    assert!(reconnected.lock().unwrap().is_empty());
    requests.answered(new_id());
    assert!(reconnected.lock().unwrap().is_empty());
    requests.answered(id);
    requests.answered(id);
    assert_eq!(*reconnected.lock().unwrap(), vec![automatic.clone()]);
    policy
        .set_here(
            orch_link_contract::policy::PolicyKey::MachineRoles,
            &serde_json::json!(false),
        )
        .unwrap();
    assert_eq!(data.config(|_| None).unwrap().connector.gateway, automatic);
    assert_eq!(data.record().unwrap().machine_id, record.machine_id);
}

#[test]
fn a_gateway_change_never_selects_an_unadvertised_entrance_or_reconnects_an_overridden_choice() {
    let (_directory, data, _) = fixture();
    let policy = Arc::new(
        PolicyStore::connected(data.config(|_| None).unwrap(), Some(data.config_path())).unwrap(),
    );
    let reconnected = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&reconnected);
    let requests = Requests::new(
        data.root().to_owned(),
        Arc::clone(&policy),
        Arc::new(move |choice| observed.lock().unwrap().push(choice)),
    );
    let unknown = GatewayChoice {
        selection: GatewaySelection::Selected,
        gateway_id: Some("elsewhere".into()),
    };
    assert!(requests.request(new_id(), &unknown).is_err());
    assert_eq!(
        data.config(|_| None).unwrap().connector.gateway,
        GatewayChoice::default()
    );
    let id = new_id();
    let selected = GatewayChoice {
        selection: GatewaySelection::Selected,
        gateway_id: Some("eu".into()),
    };
    assert!(requests.request(id, &selected).unwrap().changed);
    policy.change_gateway(&GatewayChoice::default()).unwrap();
    requests.answered(id);
    assert!(
        reconnected.lock().unwrap().is_empty(),
        "the local owner's later choice stays authoritative"
    );
}
