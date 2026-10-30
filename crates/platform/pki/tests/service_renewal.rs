//! A service's own certificate renewed while it runs: due once two thirds of its life have passed, adopted only
//! when it holds the new key and names the same service, and shown from the next handshake on.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use chrono::Duration;
use common::{authority, clock, handshake, machine, service};
use orch_pki::certificates::{CertificateRequest, ServiceName};
use orch_pki::renewal::{RenewalPolicy, renew};
use orch_pki::service_renew::{LiveIdentity, RenewFault, adopt, renew_if_due};
use orch_pki::verification::{ServerTrust, pinned_client_config, pinned_server_config_live};
use orch_types::{Clock, ManualClock};
use std::sync::Arc;

const HOST: &str = "gateway.test";
const GRACE_DAYS: u64 = 90;

fn arc_clock(clock: &ManualClock) -> Arc<dyn Clock> {
    Arc::new(clock.clone())
}

#[tokio::test]
async fn a_service_renews_once_two_thirds_have_passed_and_shows_the_new_certificate_from_the_next_handshake()
 {
    let clock = clock();
    let ca = authority(&clock);
    let gateway = service(&ca, &clock, ServiceName::Gateway, &[HOST]);
    let first = gateway.chain[0].clone();
    let live = LiveIdentity::new(&gateway).unwrap();
    let server = pinned_server_config_live(
        Arc::clone(&live),
        ca.certificate(),
        GRACE_DAYS,
        arc_clock(&clock),
    )
    .unwrap();
    let (_, mine) = machine(&ca, &clock);
    let client = pinned_client_config(
        ServerTrust::Authority(Box::new(ca.certificate().clone())),
        Some(ServiceName::Gateway),
        Some(&mine),
        arc_clock(&clock),
    )
    .unwrap();
    handshake(Arc::clone(&client), Arc::clone(&server), HOST)
        .await
        .unwrap();

    // Not due yet: nobody is asked.
    let asked = renew_if_due(&live, clock.time(), |_| async {
        Err::<String, String>("asked too early".to_owned())
    })
    .await
    .unwrap();
    assert!(!asked);

    clock.advance(Duration::days(250));
    let presented = live.certificate().unwrap();
    let policy = RenewalPolicy {
        validity_days: 365,
        grace_days: 0,
        service_names: vec![HOST.to_owned()],
    };
    let renewed = renew_if_due(&live, clock.time(), |csr_pem| {
        let request = CertificateRequest::from_pem(&csr_pem).unwrap();
        let answer = renew(&ca, &presented, None, &request, &policy, &clock)
            .map(|certificate| certificate.pem())
            .map_err(|error| error.to_string());
        async move { answer }
    })
    .await
    .unwrap();
    assert!(renewed);
    let shown = live.certificate().unwrap();
    assert_ne!(shown, first, "a new certificate is shown");
    assert_eq!(
        shown.identities(),
        first.identities(),
        "for the same service"
    );
    assert!(shown.not_after() > first.not_after());
    handshake(Arc::clone(&client), Arc::clone(&server), HOST)
        .await
        .unwrap();

    // Past the first certificate's end the listener, never started again, is still believed: it shows the
    // renewed one.
    clock.advance(Duration::days(120));
    handshake(client, server, HOST).await.unwrap();
}

#[test]
fn a_certificate_for_another_key_or_of_another_service_is_not_adopted() {
    let clock = clock();
    let ca = authority(&clock);
    let gateway = service(&ca, &clock, ServiceName::Gateway, &[HOST]);
    let live = LiveIdentity::new(&gateway).unwrap();
    let current = live.certificate().unwrap();
    let worker = service(&ca, &clock, ServiceName::Worker, &[HOST]);

    let (other_key, _) = orch_pki::renewal::start().unwrap();
    let refused = adopt(&live, &current, other_key, &worker.chain[0].pem());
    assert!(matches!(refused, Err(RenewFault::Wrong(_))), "{refused:?}");
    let refused = adopt(&live, &current, worker.key, &worker.chain[0].pem());
    assert!(matches!(refused, Err(RenewFault::Wrong(_))), "{refused:?}");
    assert_eq!(
        live.certificate().unwrap(),
        current,
        "the gateway shows what it showed"
    );
}
