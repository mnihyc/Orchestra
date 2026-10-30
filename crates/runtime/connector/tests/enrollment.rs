//! A connector that starts with nothing: it is given a token and an address, enrolls, keeps what it was given, and from then on holds its session and says what it offers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use chrono::TimeZone;
use common::{PATIENCE, registry};
use orch_connector_core::connector::{ConnectorBuilder, ConnectorParams};
use orch_connector_core::enrollment::{
    DataDir, EnrollError, EnrollInputs, EnrollTransport, enroll, roles_of,
};
use orch_link::machine::LinkStatus;
use orch_link_contract::enrollment::{
    Endpoint, EnrollReply, EnrollReplySystem, EnrollRequest, EnrollmentToken, RenewReply,
    RenewRequest,
};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_pki::enrollment::{MachineIdentity, MemoryLedger, TokenGrant, TokenPurpose, issue_token};
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Hub, TestPki};
use orch_types::{Clock, ManualClock, new_id};
use std::sync::Arc;
use std::time::Duration;

/// The system's side of an enrollment: its authority signs what a machine asks for, and the answer
/// names the hub as the gateway.
struct System<'a> {
    pki: &'a TestPki,
    ledger: MemoryLedger,
    clock: ManualClock,
}

impl System<'_> {
    fn token(&self) -> String {
        let issued = issue_token(self.pki.authority(), Duration::from_secs(900), &self.clock);
        self.ledger.record(
            &issued,
            TokenGrant {
                purpose: TokenPurpose::Machine,
                created_by: new_id(),
            },
        );
        issued.token.to_string()
    }
}

#[async_trait]
impl EnrollTransport for System<'_> {
    async fn enroll(
        &self,
        _address: &str,
        _token: &EnrollmentToken,
        request: &EnrollRequest,
    ) -> Result<EnrollReply, EnrollError> {
        let enrolled = orch_pki::enrollment::enroll(
            self.pki.authority(),
            &self.ledger,
            request,
            365,
            &self.clock,
        )
        .map_err(|error| EnrollError::Unreachable(error.to_string()))?;
        Ok(EnrollReply {
            machine_id: enrolled.machine_id,
            certificate_pem: enrolled.certificate.pem(),
            ca_pem: self.pki.authority().certificate_pem(),
            endpoints: vec![Hub::endpoint()],
            system: EnrollReplySystem {
                name: "Orch".to_owned(),
                public_url: "https://orch.example.com".to_owned(),
            },
            protocol_version: enrolled.protocol_version,
        })
    }

    async fn renew(
        &self,
        _endpoint: &Endpoint,
        _identity: &MachineIdentity,
        _request: &RenewRequest,
    ) -> Result<RenewReply, EnrollError> {
        Err(EnrollError::Unreachable("not part of this test".to_owned()))
    }
}

#[tokio::test]
async fn a_connector_given_a_token_enrolls_then_holds_its_session_and_says_what_it_offers() {
    let manual = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 11, 12, 0, 0)
            .unwrap(),
    );
    let clock: Arc<dyn Clock> = Arc::new(manual.clone());
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let system = System {
        pki: &pki,
        ledger: MemoryLedger::new(),
        clock: manual.clone(),
    };

    // First run: nothing but a directory, an address and a token.
    let dir = tempfile::tempdir().unwrap();
    let data = DataDir::open(dir.path()).unwrap();
    assert!(!data.is_enrolled());
    let inputs = EnrollInputs {
        address: "https://orch.example.com".to_owned(),
        token: system.token(),
        name: "alice-laptop".to_owned(),
        roles: vec![Role::Agent],
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        connector_version: "0.1.0".to_owned(),
        managed: false,
    };
    let enrolled = enroll(&inputs, &data, &system, &manual).await.unwrap();
    assert!(data.is_enrolled());
    for file in ["key.pem", "cert.pem", "ca.pem"] {
        assert!(data.identity_dir().join(file).is_file(), "{file} is kept");
    }

    // The next run needs only the directory: the identity, the endpoints and the roles come from it.
    let identity = data.identity().unwrap();
    let config = data.config(|_| None).unwrap();
    assert_eq!(identity.machine_id, enrolled.record.machine_id);
    assert_eq!(data.endpoints(&config).unwrap(), [Hub::endpoint()]);
    assert!(config.endpoints.is_empty());
    assert_eq!(roles_of(&config), [Role::Agent]);
    let mut config = config;
    // The owner offers /tmp, their own value as a file that names the key makes it.
    config.environment.roots = vec!["/tmp".to_owned()];
    config
        .policy
        .local_keys
        .get_or_insert_with(Vec::new)
        .push("environment.roots".into());

    let controller = ScriptedController::attach(
        &hub,
        Arc::clone(&clock),
        identity.machine_id,
        roles_of(&config),
    );
    let connector = ConnectorBuilder::new(ConnectorParams {
        follow_system_endpoints: true,
        hosted_endpoints: None,
        version: "0.1.0".to_owned(),
        data_dir: dir.path().to_owned(),
        config: config.clone(),
        dialer: hub.dialer(Some(identity.certificate.clone())),
        endpoints: data.endpoints(&config).unwrap(),
        roles: roles_of(&config),
        identity,
        managed: false,
        clock: Arc::clone(&clock),
        platform: inputs.platform,
        adapters: registry(),
        tools_dir: None,
        blobs: None,
    })
    .start()
    .await
    .unwrap();

    // It holds a session...
    tokio::time::timeout(PATIENCE, connector.link().connected())
        .await
        .expect("the connector never got a session");
    assert!(hub.session_of(enrolled.record.machine_id).is_some());
    let status = connector.link().status().borrow().clone();
    assert!(
        matches!(&status, LinkStatus::Connected { endpoint, .. } if endpoint == &Hub::endpoint().url),
        "{status:?}"
    );

    // ...and says what it offers: the harness kinds it can drive, its host, the policy of its owner.
    let said = controller
        .wait_until(PATIENCE, |c| {
            c.applied("machine")
                .iter()
                .any(|e| e.kind == "machine.facts")
        })
        .await;
    assert!(said, "the facts never arrived");
    let facts = controller
        .applied("machine")
        .into_iter()
        .find(|e| e.kind == "machine.facts")
        .unwrap()
        .body;
    assert_eq!(
        facts["adapters"],
        serde_json::json!([{ "kind": "fake", "adapter_version": 1, "builtin": true, "needs_binary": false,
                             "default_home_owned": true }])
    );
    assert!(!facts["hostname"].as_str().unwrap().is_empty());
    assert!(facts["cpus"].as_u64().unwrap() >= 1);
    assert_eq!(
        facts["policy"]["environment.roots"],
        serde_json::json!(["/tmp"]),
        "a machine a person connected reports the policy of its owner"
    );
    assert!(controller.violations().is_empty());
    connector.stop().await;
}

#[tokio::test]
async fn a_machine_the_system_made_reports_no_policy_and_a_connector_can_be_started_again_from_its_directory()
 {
    let manual = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 11, 12, 0, 0)
            .unwrap(),
    );
    let clock: Arc<dyn Clock> = Arc::new(manual.clone());
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let machine_id = new_id();
    let issued = pki.machine(machine_id);
    let identity = MachineIdentity {
        machine_id,
        key: issued.key,
        certificate: issued.certificate.clone(),
        authority: pki.certificate(),
    };
    let dir = tempfile::tempdir().unwrap();
    let controller =
        ScriptedController::attach(&hub, Arc::clone(&clock), machine_id, vec![Role::Agent]);
    for round in 0..2 {
        let connector = ConnectorBuilder::new(ConnectorParams {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0".to_owned(),
            data_dir: dir.path().to_owned(),
            config: orch_config::ConnectorConfig::default(),
            dialer: hub.dialer(Some(issued.certificate.clone())),
            endpoints: vec![Hub::endpoint()],
            roles: vec![Role::Agent],
            identity: MachineIdentity {
                machine_id,
                key: identity.key.duplicate().unwrap(),
                certificate: identity.certificate.clone(),
                authority: identity.authority.clone(),
            },
            managed: true,
            clock: Arc::clone(&clock),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            adapters: registry(),
            tools_dir: None,
            blobs: None,
        })
        .start()
        .await
        .unwrap();
        tokio::time::timeout(PATIENCE, connector.link().connected())
            .await
            .expect("no session");
        let wanted = round + 1;
        let said = controller
            .wait_until(PATIENCE, move |c| {
                c.applied("machine")
                    .iter()
                    .filter(|e| e.kind == "machine.facts")
                    .count()
                    >= wanted
            })
            .await;
        assert!(said, "round {round}: the facts never arrived");
        let facts = controller
            .applied("machine")
            .into_iter()
            .rfind(|e| e.kind == "machine.facts")
            .unwrap()
            .body;
        assert!(
            facts["policy"].is_null(),
            "no policy of its own on a machine the system made"
        );
        connector.stop().await;
    }
    assert!(
        controller.violations().is_empty(),
        "{:?}",
        controller.violations()
    );
}

#[tokio::test]
async fn every_connection_refreshes_system_addresses_without_overwriting_an_owners_list() {
    use orch_link_contract::enrollment::Trust;
    let manual = ManualClock::new(chrono::Utc.with_ymd_and_hms(2026, 10, 25, 0, 0, 0).unwrap());
    let clock: Arc<dyn Clock> = Arc::new(manual.clone());
    let pki = TestPki::new(Arc::clone(&clock));
    for owns in [false, true] {
        let hub = Hub::new(Arc::clone(&clock), pki.certificate());
        let system = System {
            pki: &pki,
            ledger: MemoryLedger::new(),
            clock: manual.clone(),
        };
        let directory = tempfile::tempdir().unwrap();
        let data = DataDir::open(directory.path()).unwrap();
        let inputs = EnrollInputs {
            address: Hub::endpoint().url,
            token: system.token(),
            name: "Address test".into(),
            roles: vec![Role::Agent],
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            connector_version: "0.1.0".into(),
            managed: false,
        };
        let enrolled = enroll(&inputs, &data, &system, &manual).await.unwrap();
        if owns {
            data.update_config(|config| {
                config.endpoints = vec![orch_config::connector::ConnectorEndpoint {
                    url: "wss://owner.test:41702".into(),
                    trust: "pinned".into(),
                }]
            })
            .unwrap();
        }
        let config = data.config(|_| None).unwrap();
        let initial = data.endpoints(&config).unwrap();
        let first = vec![Endpoint {
            url: "wss://first.test:41703".into(),
            trust: Trust::Pinned,
        }];
        let second = vec![Endpoint {
            url: "wss://second.test/link".into(),
            trust: Trust::Public,
        }];
        hub.advertise_endpoints(first.clone());
        let _controller = ScriptedController::attach(
            &hub,
            Arc::clone(&clock),
            enrolled.record.machine_id,
            vec![Role::Agent],
        );
        let connector = ConnectorBuilder::new(ConnectorParams {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0".into(),
            data_dir: directory.path().to_owned(),
            endpoints: initial.clone(),
            config,
            identity: enrolled.identity,
            roles: vec![Role::Agent],
            managed: false,
            dialer: hub.dialer(Some(data.identity().unwrap().certificate)),
            clock: Arc::clone(&clock),
            platform: inputs.platform,
            adapters: registry(),
            tools_dir: None,
            blobs: None,
        })
        .start()
        .await
        .unwrap();
        tokio::time::timeout(PATIENCE, connector.link().connected())
            .await
            .unwrap();
        assert_eq!(
            connector.link().endpoints(),
            if owns { initial.clone() } else { first }
        );
        assert!(connector.link().session_token().is_some());
        let original = hub.session_of(enrolled.record.machine_id).unwrap();
        hub.advertise_endpoints(second.clone());
        hub.disconnect(enrolled.record.machine_id);
        tokio::time::timeout(PATIENCE, async {
            loop {
                if hub
                    .session_of(enrolled.record.machine_id)
                    .is_some_and(|session| session != original)
                    && data.record().unwrap().server_endpoints == second
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            connector.link().endpoints(),
            if owns { initial } else { second }
        );
        assert_eq!(data.config(|_| None).unwrap().endpoints.is_empty(), !owns);
        connector.stop().await;
    }
}

#[tokio::test]
async fn a_foreground_connector_or_node_adopts_a_saved_replacement_without_another_physical_runner()
{
    use orch_connector_core::connector::{Supervision, run_until};
    use orch_connector_core::enrollment::EnrollmentRecord;
    use orch_connector_core::local_socket::{Reply, Request, ask};
    use tokio_util::sync::CancellationToken;
    for role in [Role::Agent, Role::Node] {
        let manual = ManualClock::new(chrono::Utc.with_ymd_and_hms(2026, 10, 25, 0, 0, 0).unwrap());
        let clock: Arc<dyn Clock> = Arc::new(manual.clone());
        let pki = TestPki::new(Arc::clone(&clock));
        let hub = Hub::new(Arc::clone(&clock), pki.certificate());
        let id = new_id();
        let first = pki.machine(id);
        let directory = tempfile::tempdir().unwrap();
        let data = DataDir::open(directory.path()).unwrap();
        MachineIdentity {
            machine_id: id,
            key: first.key.duplicate().unwrap(),
            certificate: first.certificate.clone(),
            authority: pki.certificate(),
        }
        .save(&data.identity_dir())
        .unwrap();
        let record = EnrollmentRecord {
            server_endpoints: vec![Hub::endpoint()],
            machine_id: id,
            name: "Kept".into(),
            system: EnrollReplySystem {
                name: "Test".into(),
                public_url: "https://orch.test".into(),
            },
            pin: String::new(),
            protocol_version: 1,
            enrolled_at: manual.time(),
            managed: role == Role::Node,
        };
        std::fs::write(
            data.identity_dir().join("enrollment.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        data.update_config(|config| {
            config.connector.roles = vec![orch_connector_core::enrollment::role_name(&role)]
        })
        .unwrap();
        let before = std::fs::read(data.config_path()).unwrap();
        let _controller = ScriptedController::attach(&hub, Arc::clone(&clock), id, vec![role]);
        let system = System {
            pki: &pki,
            ledger: MemoryLedger::new(),
            clock: manual.clone(),
        };
        let make = |identity: MachineIdentity| {
            let config = data.config(|_| None)?;
            Ok(ConnectorBuilder::new(ConnectorParams {
                follow_system_endpoints: true,
                hosted_endpoints: None,
                version: "0.1.0".into(),
                data_dir: directory.path().to_owned(),
                endpoints: data.endpoints(&config)?,
                config,
                roles: vec![role],
                managed: role == Role::Node,
                dialer: hub.dialer(Some(identity.certificate.clone())),
                identity,
                clock: Arc::clone(&clock),
                platform: Platform {
                    os: Os::Linux,
                    arch: Arch::X86_64,
                },
                adapters: registry(),
                tools_dir: None,
                blobs: None,
            }))
        };
        let stop = CancellationToken::new();
        let exercise = async {
            tokio::time::timeout(PATIENCE, async {
                while ask(data.root(), &Request::Status).await.is_err()
                    || hub.session_of(id).is_none()
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            let initial = hub.session_of(id).unwrap();
            manual.advance(chrono::Duration::days(1));
            let replacement = pki.machine(id);
            MachineIdentity {
                machine_id: id,
                key: replacement.key.duplicate().unwrap(),
                certificate: replacement.certificate.clone(),
                authority: pki.certificate(),
            }
            .save(&data.identity_dir())
            .unwrap();
            assert!(matches!(
                ask(data.root(), &Request::ReloadIdentity).await.unwrap(),
                Reply::Done { .. }
            ));
            tokio::time::timeout(PATIENCE, async {
                while !hub.session_of(id).is_some_and(|session| session != initial) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(std::fs::read(data.config_path()).unwrap(), before);
            stop.cancel();
        };
        let run = run_until(
            &data,
            &make,
            &system,
            Arc::clone(&clock),
            Supervision::default(),
            &|_| None,
            stop.cancelled(),
        );
        let (result, ()) = tokio::join!(run, exercise);
        result.unwrap();
    }
}
