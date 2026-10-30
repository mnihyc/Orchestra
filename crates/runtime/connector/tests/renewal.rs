//! A connector that runs for most of a year: its certificate comes due at two thirds of its life, is renewed over a new key, and the connector goes on with the new pair and a session of its own.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::TimeZone;
use common::{PATIENCE, registry};
use orch_connector_core::connector::{ConnectorBuilder, ConnectorParams, Supervision, run_until};
use orch_connector_core::enrollment::{
    DataDir, EnrollError, EnrollTransport, endpoints_of, roles_of,
};
use orch_link_contract::enrollment::{
    Endpoint, EnrollReply, EnrollRequest, EnrollmentToken, RenewReply, RenewRequest,
};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_pki::ca::Holder;
use orch_pki::certificates::CertificateRequest;
use orch_pki::enrollment::MachineIdentity;
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Hub, TestPki};
use orch_types::{Clock, ManualClock, new_id};
use tokio_util::sync::CancellationToken;

/// The system's side of a renewal: its authority signs the new key for the machine that asks.
struct Renewer<'a> {
    pki: &'a TestPki,
    clock: ManualClock,
    renewals: AtomicUsize,
}

#[async_trait]
impl EnrollTransport for Renewer<'_> {
    async fn enroll(
        &self,
        _address: &str,
        _token: &EnrollmentToken,
        _request: &EnrollRequest,
    ) -> Result<EnrollReply, EnrollError> {
        Err(EnrollError::Unreachable("not part of this test".to_owned()))
    }

    async fn renew(
        &self,
        _endpoint: &Endpoint,
        identity: &MachineIdentity,
        request: &RenewRequest,
    ) -> Result<RenewReply, EnrollError> {
        self.renewals.fetch_add(1, Ordering::SeqCst);
        let csr = CertificateRequest::from_pem(&request.csr_pem).unwrap();
        let certificate = self
            .pki
            .authority()
            .issue(
                &csr,
                &Holder::Machine(identity.machine_id),
                365,
                &self.clock,
            )
            .unwrap();
        Ok(RenewReply {
            certificate_pem: certificate.pem(),
            ca_pem: self.pki.authority().certificate_pem(),
        })
    }
}

#[tokio::test]
async fn a_certificate_due_after_two_thirds_of_its_life_is_renewed_and_the_connector_goes_on_with_the_new_pair()
 {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
    let manual = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 11, 12, 0, 0)
            .unwrap(),
    );
    let clock: Arc<dyn Clock> = Arc::new(manual.clone());
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let machine_id = new_id();

    // A data directory as enrollment leaves it: the identity, the address of the gateway, the roles.
    let dir = tempfile::tempdir().unwrap();
    let data = DataDir::open(dir.path()).unwrap();
    let issued = pki.machine(machine_id);
    let first = MachineIdentity {
        machine_id,
        key: issued.key,
        certificate: issued.certificate,
        authority: pki.certificate(),
    };
    first.save(&data.identity_dir()).unwrap();
    data.update_config(|config| {
        config.connector.roles = vec!["agent".to_owned()];
        config.endpoints = vec![orch_config::connector::ConnectorEndpoint {
            url: Hub::endpoint().url,
            trust: "pinned".to_owned(),
        }];
        config.environment.roots = vec![std::env::temp_dir().to_string_lossy().into_owned()];
    })
    .unwrap();
    let old_fingerprint = first.certificate.fingerprint();
    let controller =
        ScriptedController::attach(&hub, Arc::clone(&clock), machine_id, vec![Role::Agent]);
    let renewer = Renewer {
        pki: &pki,
        clock: manual.clone(),
        renewals: AtomicUsize::new(0),
    };

    let presented = std::sync::Mutex::new(Vec::new());
    let make = |identity: MachineIdentity| {
        presented
            .lock()
            .unwrap()
            .push(identity.certificate.fingerprint());
        let config = data
            .config(|_| None)
            .map_err(orch_connector_core::connector::StartError::from)?;
        Ok(ConnectorBuilder::new(ConnectorParams {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0".to_owned(),
            data_dir: dir.path().to_owned(),
            dialer: hub.dialer(Some(identity.certificate.clone())),
            endpoints: endpoints_of(&config),
            roles: roles_of(&config),
            config,
            identity,
            managed: false,
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

    let shutdown = CancellationToken::new();
    let running = run_until(
        &data,
        &make,
        &renewer,
        Arc::clone(&clock),
        Supervision {
            look_every: Duration::from_millis(20),
            idle_every: Duration::from_millis(20),
        },
        &|_| None,
        shutdown.cancelled(),
    );
    let watching = async {
        let first_session = loop {
            if let Some(session) = hub.session_of(machine_id) {
                break session;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        let first_facts = controller
            .wait_until(PATIENCE, |c| {
                c.applied("machine")
                    .iter()
                    .any(|event| event.kind == "machine.facts")
            })
            .await;
        assert!(
            first_facts,
            "the first identity reports its facts before its lifetime is advanced"
        );
        // Early in its life nothing is due, however long the connector looks.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(renewer.renewals.load(Ordering::SeqCst), 0);
        assert_eq!(hub.session_of(machine_id), Some(first_session));

        // Two thirds of a year later it is.
        manual.advance(chrono::Duration::days(250));
        let renewed = controller
            .wait_until(PATIENCE, |_| {
                renewer.renewals.load(Ordering::SeqCst) == 1
                    && presented.lock().unwrap().len() == 2
                    && hub
                        .session_of(machine_id)
                        .is_some_and(|session| session != first_session)
            })
            .await;
        assert!(
            renewed,
            "the connector never went on with a session of the new pair"
        );
        let now = data.identity().unwrap();
        assert_eq!(now.machine_id, machine_id);
        assert_ne!(
            now.certificate.fingerprint(),
            old_fingerprint,
            "a new certificate is on disk"
        );
        assert!(now.certificate.holds(&now.key), "over a key of its own");
        let starts = presented.lock().unwrap().clone();
        assert_eq!(
            starts.len(),
            2,
            "the supervisor rebuilt the connector exactly once"
        );
        assert_eq!(starts[0], old_fingerprint);
        assert_eq!(
            starts[1],
            now.certificate.fingerprint(),
            "the new connector uses the renewed pair"
        );
        assert_ne!(
            now.certificate.public_point(),
            first.certificate.public_point(),
            "and the key is a new one"
        );
        // It said what it offers again to the system, which it had been told once before.
        let said = controller
            .wait_until(PATIENCE, |c| {
                c.applied("machine")
                    .iter()
                    .filter(|e| e.kind == "machine.facts")
                    .count()
                    >= 2
            })
            .await;
        assert!(
            said,
            "{:?} {:?}",
            controller
                .applied("machine")
                .iter()
                .map(|e| (e.seq, e.kind.clone()))
                .collect::<Vec<_>>(),
            controller.violations()
        );
        assert!(
            controller.violations().is_empty(),
            "{:?}",
            controller.violations()
        );
        // And nothing more is renewed: the new certificate has all its life ahead.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(renewer.renewals.load(Ordering::SeqCst), 1);
        shutdown.cancel();
    };
    let (result, ()) = tokio::join!(running, watching);
    result.unwrap();
}

#[tokio::test]
async fn a_certificate_that_came_due_while_the_connector_was_off_is_renewed_before_it_starts() {
    let manual = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 11, 12, 0, 0)
            .unwrap(),
    );
    let clock: Arc<dyn Clock> = Arc::new(manual.clone());
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let machine_id = new_id();
    let dir = tempfile::tempdir().unwrap();
    let data = DataDir::open(dir.path()).unwrap();
    let issued = pki.machine(machine_id);
    let first = MachineIdentity {
        machine_id,
        key: issued.key,
        certificate: issued.certificate,
        authority: pki.certificate(),
    };
    first.save(&data.identity_dir()).unwrap();
    data.update_config(|config| {
        config.connector.roles = vec!["agent".to_owned()];
        config.endpoints = vec![orch_config::connector::ConnectorEndpoint {
            url: Hub::endpoint().url,
            trust: "pinned".to_owned(),
        }];
    })
    .unwrap();
    // The connector was switched off for most of a year.
    manual.advance(chrono::Duration::days(300));
    let controller =
        ScriptedController::attach(&hub, Arc::clone(&clock), machine_id, vec![Role::Agent]);
    let renewer = Renewer {
        pki: &pki,
        clock: manual.clone(),
        renewals: AtomicUsize::new(0),
    };
    let presented = Arc::new(std::sync::Mutex::new(Vec::new()));
    let make = |identity: MachineIdentity| {
        presented
            .lock()
            .unwrap()
            .push(identity.certificate.fingerprint());
        let config = data
            .config(|_| None)
            .map_err(orch_connector_core::connector::StartError::from)?;
        Ok(ConnectorBuilder::new(ConnectorParams {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0".to_owned(),
            data_dir: dir.path().to_owned(),
            dialer: hub.dialer(Some(identity.certificate.clone())),
            endpoints: endpoints_of(&config),
            roles: roles_of(&config),
            config,
            identity,
            managed: false,
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
    let shutdown = CancellationToken::new();
    let running = run_until(
        &data,
        &make,
        &renewer,
        Arc::clone(&clock),
        Supervision::default(),
        &|_| None,
        shutdown.cancelled(),
    );
    let watching = async {
        let up = controller
            .wait_until(PATIENCE, |_| hub.session_of(machine_id).is_some())
            .await;
        assert!(up);
        shutdown.cancel();
    };
    let (result, ()) = tokio::join!(running, watching);
    result.unwrap();
    assert_eq!(
        renewer.renewals.load(Ordering::SeqCst),
        1,
        "renewed once, before the first start"
    );
    let presented = presented.lock().unwrap();
    assert_eq!(presented.len(), 1, "started once");
    assert_ne!(
        presented[0],
        first.certificate.fingerprint(),
        "with the new certificate"
    );
}
