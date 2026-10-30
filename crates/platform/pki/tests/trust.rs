//! Who trusts whom: the pinned path under the system's own authority, the public path through an edge, and the
//! proof a machine gives inside a connection that carried no certificate.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use chrono::Duration;
use common::{authority, clock, handshake, machine, service};
use orch_pki::ca::Holder;
use orch_pki::certificates::{Certificate, LeafKey, ServiceName};
use orch_pki::verification::{
    ProofFault, ServerTrust, TlsIdentity, pinned_client_config, pinned_server_config,
    plain_server_config, public_client_config, root_store, verify_machine_proof,
};
use orch_types::{Clock, ManualClock};
use std::sync::Arc;

const GRACE_DAYS: u64 = 90;
const HOST: &str = "gateway.test";

fn arc_clock(clock: &ManualClock) -> Arc<dyn Clock> {
    Arc::new(clock.clone())
}

/// A gateway whose certificate outlives the tests that move the clock by years.
fn long_lived_gateway(ca: &orch_pki::ca::CertificateAuthority, clock: &ManualClock) -> TlsIdentity {
    let holder = Holder::Service {
        service: ServiceName::Gateway,
        names: vec![HOST.to_owned()],
    };
    let (key, certificate) = ca.issue_with_new_key(&holder, 4_000, clock).unwrap();
    TlsIdentity {
        chain: vec![certificate, ca.certificate().clone()],
        key,
    }
}

#[tokio::test]
async fn a_machine_and_the_gateway_of_one_system_know_each_other_on_the_pinned_path() {
    let clock = clock();
    let ca = authority(&clock);
    let gateway = service(&ca, &clock, ServiceName::Gateway, &[HOST]);
    let (id, mine) = machine(&ca, &clock);
    let server =
        pinned_server_config(&gateway, ca.certificate(), GRACE_DAYS, arc_clock(&clock)).unwrap();
    let client = pinned_client_config(
        ServerTrust::Authority(Box::new(ca.certificate().clone())),
        Some(ServiceName::Gateway),
        Some(&mine),
        arc_clock(&clock),
    )
    .unwrap();
    let outcome = handshake(client, server, HOST).await.unwrap();
    assert_eq!(outcome.client_certificate.unwrap().machine_id(), Some(id));
}

#[tokio::test]
async fn a_client_certificate_of_another_authority_is_refused_by_the_gateway() {
    let clock = clock();
    let ca = authority(&clock);
    let intruder = authority(&clock);
    let gateway = service(&ca, &clock, ServiceName::Gateway, &[HOST]);
    let (_, forged) = machine(&intruder, &clock);
    let server =
        pinned_server_config(&gateway, ca.certificate(), GRACE_DAYS, arc_clock(&clock)).unwrap();
    let client = pinned_client_config(
        ServerTrust::Authority(Box::new(ca.certificate().clone())),
        None,
        Some(&forged),
        arc_clock(&clock),
    )
    .unwrap();
    assert!(handshake(client, server, HOST).await.is_err());
}

#[tokio::test]
async fn a_gateway_of_another_authority_is_refused_by_the_machine() {
    let clock = clock();
    let ca = authority(&clock);
    let intruder = authority(&clock);
    let false_gateway = service(&intruder, &clock, ServiceName::Gateway, &[HOST]);
    let (_, mine) = machine(&ca, &clock);
    let server = pinned_server_config(
        &false_gateway,
        intruder.certificate(),
        GRACE_DAYS,
        arc_clock(&clock),
    )
    .unwrap();
    let client = pinned_client_config(
        ServerTrust::Authority(Box::new(ca.certificate().clone())),
        Some(ServiceName::Gateway),
        Some(&mine),
        arc_clock(&clock),
    )
    .unwrap();
    assert!(handshake(client, server, HOST).await.is_err());
}

#[tokio::test]
async fn a_server_that_is_another_service_than_the_one_expected_is_refused() {
    let clock = clock();
    let ca = authority(&clock);
    let worker = service(&ca, &clock, ServiceName::Worker, &[HOST]);
    let (_, mine) = machine(&ca, &clock);
    let server =
        pinned_server_config(&worker, ca.certificate(), GRACE_DAYS, arc_clock(&clock)).unwrap();
    let client = pinned_client_config(
        ServerTrust::Authority(Box::new(ca.certificate().clone())),
        Some(ServiceName::Gateway),
        Some(&mine),
        arc_clock(&clock),
    )
    .unwrap();
    assert!(handshake(client, server, HOST).await.is_err());
}

#[tokio::test]
async fn a_client_that_shows_no_certificate_is_refused_on_the_pinned_listener() {
    let clock = clock();
    let ca = authority(&clock);
    let gateway = service(&ca, &clock, ServiceName::Gateway, &[HOST]);
    let server =
        pinned_server_config(&gateway, ca.certificate(), GRACE_DAYS, arc_clock(&clock)).unwrap();
    let client = pinned_client_config(
        ServerTrust::Authority(Box::new(ca.certificate().clone())),
        None,
        None,
        arc_clock(&clock),
    )
    .unwrap();
    assert!(handshake(client, server, HOST).await.is_err());
}

#[tokio::test]
async fn at_first_contact_a_machine_that_knows_only_the_pin_accepts_the_authority_that_has_it() {
    let clock = clock();
    let ca = authority(&clock);
    let enrollment_endpoint = service(&ca, &clock, ServiceName::Gateway, &[HOST]);
    let server = plain_server_config(&enrollment_endpoint, arc_clock(&clock)).unwrap();
    let client =
        pinned_client_config(ServerTrust::Pin(ca.pin()), None, None, arc_clock(&clock)).unwrap();
    let outcome = handshake(client, server, HOST).await.unwrap();
    assert!(
        outcome.client_certificate.is_none(),
        "an enrolling machine has no certificate yet"
    );
}

#[tokio::test]
async fn at_first_contact_a_machine_refuses_a_server_whose_authority_has_another_pin() {
    let clock = clock();
    let ca = authority(&clock);
    let intruder = authority(&clock);
    let enrollment_endpoint = service(&intruder, &clock, ServiceName::Gateway, &[HOST]);
    let server = plain_server_config(&enrollment_endpoint, arc_clock(&clock)).unwrap();
    // The token that was copied from the screen pins the real authority.
    let client =
        pinned_client_config(ServerTrust::Pin(ca.pin()), None, None, arc_clock(&clock)).unwrap();
    let refusal = handshake(client, server, HOST).await.err().unwrap();
    assert!(
        refusal.to_lowercase().contains("unknownissuer"),
        "refused for its authority: {refusal}"
    );
}

#[tokio::test]
async fn a_server_that_does_not_send_its_authority_cannot_be_known_by_a_pin() {
    let clock = clock();
    let ca = authority(&clock);
    let mut leaf_only = service(&ca, &clock, ServiceName::Gateway, &[HOST]);
    leaf_only.chain.truncate(1);
    let server = plain_server_config(&leaf_only, arc_clock(&clock)).unwrap();
    let client =
        pinned_client_config(ServerTrust::Pin(ca.pin()), None, None, arc_clock(&clock)).unwrap();
    assert!(handshake(client, server, HOST).await.is_err());
}

#[tokio::test]
async fn a_certificate_that_ran_out_is_heard_within_the_grace_and_refused_after_it() {
    let clock = clock();
    let ca = authority(&clock);
    let gateway = long_lived_gateway(&ca, &clock);
    let (_, mine) = machine(&ca, &clock);
    let make = |clock: &ManualClock| {
        let server =
            pinned_server_config(&gateway, ca.certificate(), GRACE_DAYS, arc_clock(clock)).unwrap();
        let client = pinned_client_config(
            ServerTrust::Authority(Box::new(ca.certificate().clone())),
            Some(ServiceName::Gateway),
            Some(&mine),
            arc_clock(clock),
        )
        .unwrap();
        (client, server)
    };
    // A month after it ran out: the connection is made, and the certificate is seen to be out of date.
    clock.advance(Duration::days(365 + 30));
    let (client, server) = make(&clock);
    let seen = handshake(client, server, HOST)
        .await
        .unwrap()
        .client_certificate
        .unwrap();
    assert!(
        !seen.valid_at(clock.time()),
        "whoever acts on the connection sees that it ran out"
    );
    // Four months after: not even that.
    clock.advance(Duration::days(90));
    let (client, server) = make(&clock);
    let refusal = handshake(client, server, HOST).await.err().unwrap();
    assert!(
        refusal.to_lowercase().contains("expired"),
        "refused for its age: {refusal}"
    );
}

#[tokio::test]
async fn on_the_public_path_a_machine_believes_a_publicly_vouched_edge_and_shows_no_certificate() {
    let clock = clock();
    let public_authority = authority(&clock);
    let edge = service(
        &public_authority,
        &clock,
        ServiceName::Gateway,
        &["orch.example.com"],
    );
    let server = plain_server_config(&edge, arc_clock(&clock)).unwrap();
    let client = public_client_config(
        root_store(&[public_authority.certificate().clone()]).unwrap(),
        arc_clock(&clock),
    )
    .unwrap();
    let outcome = handshake(client, server, "orch.example.com").await.unwrap();
    assert!(outcome.client_certificate.is_none());
}

#[tokio::test]
async fn on_the_public_path_an_edge_that_no_public_authority_vouches_for_is_refused() {
    let clock = clock();
    let public_authority = authority(&clock);
    let impostor = authority(&clock);
    let edge = service(
        &impostor,
        &clock,
        ServiceName::Gateway,
        &["orch.example.com"],
    );
    let server = plain_server_config(&edge, arc_clock(&clock)).unwrap();
    let client = public_client_config(
        root_store(&[public_authority.certificate().clone()]).unwrap(),
        arc_clock(&clock),
    )
    .unwrap();
    assert!(handshake(client, server, "orch.example.com").await.is_err());
}

#[tokio::test]
async fn the_name_of_the_host_must_be_one_the_certificate_is_good_for() {
    let clock = clock();
    let ca = authority(&clock);
    let gateway = service(&ca, &clock, ServiceName::Gateway, &[HOST]);
    let (_, mine) = machine(&ca, &clock);
    let server =
        pinned_server_config(&gateway, ca.certificate(), GRACE_DAYS, arc_clock(&clock)).unwrap();
    let client = pinned_client_config(
        ServerTrust::Authority(Box::new(ca.certificate().clone())),
        None,
        Some(&mine),
        arc_clock(&clock),
    )
    .unwrap();
    assert!(handshake(client, server, "elsewhere.test").await.is_err());
}

fn proof(key: &LeafKey, message: &[u8]) -> Vec<u8> {
    key.sign(message).to_vec()
}

const MESSAGE: &[u8] = b"orch-link-auth-v1\nnonce\nmachine\nhost\nnow";

#[test]
fn a_proof_signed_with_the_key_of_a_certificate_of_the_system_holds() {
    let clock = clock();
    let ca = authority(&clock);
    let (id, mine) = machine(&ca, &clock);
    let certificate = verify_machine_proof(
        ca.certificate(),
        &mine.chain[0].pem(),
        &proof(&mine.key, MESSAGE),
        MESSAGE,
        clock.time(),
    )
    .unwrap();
    assert_eq!(certificate.machine_id(), Some(id));
}

#[test]
fn a_proof_signed_by_another_key_does_not_hold() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, mine) = machine(&ca, &clock);
    let thief = LeafKey::generate().unwrap();
    let fault = verify_machine_proof(
        ca.certificate(),
        &mine.chain[0].pem(),
        &proof(&thief, MESSAGE),
        MESSAGE,
        clock.time(),
    )
    .unwrap_err();
    assert_eq!(fault, ProofFault::BadSignature);
}

#[test]
fn a_proof_made_for_other_words_does_not_hold_for_these() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, mine) = machine(&ca, &clock);
    let recorded = proof(
        &mine.key,
        b"orch-link-auth-v1\nan old nonce\nmachine\nhost\nthen",
    );
    let fault = verify_machine_proof(
        ca.certificate(),
        &mine.chain[0].pem(),
        &recorded,
        MESSAGE,
        clock.time(),
    )
    .unwrap_err();
    assert_eq!(fault, ProofFault::BadSignature);
}

#[test]
fn a_certificate_of_another_authority_proves_nothing() {
    let clock = clock();
    let ca = authority(&clock);
    let intruder = authority(&clock);
    let (_, forged) = machine(&intruder, &clock);
    let fault = verify_machine_proof(
        ca.certificate(),
        &forged.chain[0].pem(),
        &proof(&forged.key, MESSAGE),
        MESSAGE,
        clock.time(),
    )
    .unwrap_err();
    assert_eq!(fault, ProofFault::Untrusted);
}

#[test]
fn the_certificate_of_a_service_is_not_a_machines_proof_but_a_machine_certificate_cannot_serve() {
    let clock = clock();
    let ca = authority(&clock);
    let gateway = service(&ca, &clock, ServiceName::Gateway, &[HOST]);
    // A service certificate allows client use, so it may prove itself; it names a service, not a machine, which
    // whoever compares the proof with the machine it claims to be will find out.
    let certificate = verify_machine_proof(
        ca.certificate(),
        &gateway.chain[0].pem(),
        &proof(&gateway.key, MESSAGE),
        MESSAGE,
        clock.time(),
    )
    .unwrap();
    assert_eq!(certificate.machine_id(), None);
}

#[test]
fn a_proof_with_a_certificate_that_ran_out_is_told_so_even_a_day_late() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, mine) = machine(&ca, &clock);
    clock.advance(Duration::days(366));
    let fault = verify_machine_proof(
        ca.certificate(),
        &mine.chain[0].pem(),
        &proof(&mine.key, MESSAGE),
        MESSAGE,
        clock.time(),
    )
    .unwrap_err();
    assert_eq!(fault, ProofFault::Expired);
}

#[test]
fn a_certificate_that_is_not_yet_good_is_told_so() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, mine) = machine(&ca, &clock);
    let before = clock.time().get() - Duration::days(2);
    let fault = verify_machine_proof(
        ca.certificate(),
        &mine.chain[0].pem(),
        &proof(&mine.key, MESSAGE),
        MESSAGE,
        before.into(),
    )
    .unwrap_err();
    assert_eq!(fault, ProofFault::NotYetValid);
}

#[test]
fn text_that_is_no_certificate_is_malformed() {
    let clock = clock();
    let ca = authority(&clock);
    let fault = verify_machine_proof(
        ca.certificate(),
        "not a certificate",
        &[0; 64],
        MESSAGE,
        clock.time(),
    )
    .unwrap_err();
    assert_eq!(fault, ProofFault::Malformed);
    let _ = Certificate::from_pem("nothing").unwrap_err();
}

#[tokio::test]
async fn machines_trust_the_named_orch_service_at_an_ip_or_another_dns_name_but_never_another_role_or_authority()
 {
    use orch_pki::verification::machine_client_config;
    let clock = clock();
    let own = authority(&clock);
    let other = authority(&clock);
    for expected in [ServiceName::Gateway, ServiceName::Frontend] {
        for address in ["127.0.0.1", "second-door.test"] {
            let identity = service(&own, &clock, expected, &[HOST]);
            let server = plain_server_config(&identity, arc_clock(&clock)).unwrap();
            let client = machine_client_config(
                Some(ServerTrust::Authority(Box::new(own.certificate().clone()))),
                expected,
                None,
                arc_clock(&clock),
            )
            .unwrap();
            handshake(client, server, address).await.unwrap();
            let client = machine_client_config(
                Some(ServerTrust::Pin(own.pin())),
                expected,
                None,
                arc_clock(&clock),
            )
            .unwrap();
            let server = plain_server_config(&identity, arc_clock(&clock)).unwrap();
            handshake(client, server, address).await.unwrap();
        }
        for identity in [
            service(&own, &clock, ServiceName::Worker, &[HOST]),
            machine(&own, &clock).1,
            service(&other, &clock, expected, &[HOST]),
        ] {
            let server = plain_server_config(&identity, arc_clock(&clock)).unwrap();
            let client = machine_client_config(
                Some(ServerTrust::Authority(Box::new(own.certificate().clone()))),
                expected,
                None,
                arc_clock(&clock),
            )
            .unwrap();
            assert!(handshake(client, server, HOST).await.is_err());
        }
    }
}

#[test]
fn device_trust_in_a_company_authority_is_additive_and_keeps_name_verification() {
    let clock = clock();
    let company = authority(&clock);
    let directory = tempfile::tempdir().unwrap();
    let root_file = directory.path().join("company.pem");
    std::fs::write(&root_file, company.certificate_pem()).unwrap();
    let empty = directory.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    for trusted in [false, true] {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "company_store_child", "--nocapture"])
            .env("ORCH_TRUST_CHILD", "yes")
            .env("ORCH_TRUST_CA", &root_file)
            .env("ORCH_TRUST_KEY", company.key_pem())
            .env(
                "SSL_CERT_FILE",
                if trusted {
                    root_file.as_path()
                } else {
                    empty.as_path()
                },
            )
            .env("SSL_CERT_DIR", &empty)
            .env("ORCH_TRUST_ALLOWED", if trusted { "yes" } else { "no" })
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[tokio::test]
async fn company_store_child() {
    if std::env::var("ORCH_TRUST_CHILD").is_err() {
        return;
    }
    let clock = clock();
    let ca = std::fs::read_to_string(std::env::var("ORCH_TRUST_CA").unwrap()).unwrap();
    let key = std::env::var("ORCH_TRUST_KEY").unwrap();
    let company = orch_pki::ca::CertificateAuthority::from_pem(&ca, &key).unwrap();
    let trusted = std::env::var("ORCH_TRUST_ALLOWED").unwrap() == "yes";
    let roots = orch_pki::verification::trusted_roots();
    assert!(
        roots.len() > 10,
        "native roots supplement the compiled public roots"
    );
    let server = service(&company, &clock, ServiceName::Api, &[HOST]);
    // Device trust remains independent even if this authority also signs Orch services and this leaf names another one.
    for own in [
        None,
        Some(ServerTrust::Authority(Box::new(
            company.certificate().clone(),
        ))),
    ] {
        for (name, allowed) in [(HOST, trusted), ("wrong-name.test", false)] {
            let client = orch_pki::verification::machine_client_config(
                own.clone(),
                ServiceName::Frontend,
                None,
                arc_clock(&clock),
            )
            .unwrap();
            let result = handshake(
                client,
                plain_server_config(&server, arc_clock(&clock)).unwrap(),
                name,
            )
            .await;
            assert_eq!(
                result.is_ok(),
                allowed,
                "{name}: {}",
                result.as_ref().err().map_or("accepted", String::as_str)
            );
            if !trusted {
                let message = result.err().unwrap();
                assert!(message.contains("UnknownIssuer"), "{message}");
                assert!(
                    message.contains("Orch CA Test"),
                    "unknown issuer is named: {message}"
                );
            }
        }
    }
}
