//! The authority and what it issues: its shape, its custody, and the certificates that come of a request.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{authority, clock, machine, service};
use orch_pki::ca::{CA_VALIDITY_DAYS, CaFiles, CertificateAuthority, Holder};
use orch_pki::certificates::{
    Certificate, CertificateRequest, Identity, LeafKey, ServiceName, pem_encode,
};
use orch_types::{Clock, new_id};
use rcgen::{CertificateParams, DnType, IsCa, KeyPair, SanType};

const DAY_SECS: i64 = 86_400;

fn days_between(certificate: &Certificate) -> f64 {
    let millis = certificate.not_after().unix_millis() - certificate.not_before().unix_millis();
    millis as f64 / 1_000.0 / DAY_SECS as f64
}

#[test]
fn the_authority_signs_leaves_and_no_authority_stands_below_it() {
    let clock = clock();
    let ca = authority(&clock);
    let certificate = ca.certificate();
    assert!(certificate.is_ca());
    assert_eq!(certificate.path_len(), Some(0));
    assert!(certificate.signs_certificates());
    assert_eq!(certificate.subject_common_name(), Some("Orch CA Test"));
    assert!(certificate.valid_at(clock.time()));
    let years = days_between(certificate);
    assert!(
        (CA_VALIDITY_DAYS as f64..CA_VALIDITY_DAYS as f64 + 1.0).contains(&years),
        "ten years, and no more than the day of backdating on top: {years}"
    );
}

#[test]
fn a_machine_certificate_names_the_machine_and_allows_only_a_client() {
    let clock = clock();
    let ca = authority(&clock);
    let (id, identity) = machine(&ca, &clock);
    let certificate = &identity.chain[0];
    assert_eq!(certificate.identities(), [Identity::Machine(id)]);
    assert_eq!(certificate.machine_id(), Some(id));
    assert!(certificate.allows_client_auth());
    assert!(!certificate.allows_server_auth());
    assert!(!certificate.is_ca());
    assert!(certificate.holds(&identity.key));
    let days = days_between(certificate);
    assert!(
        (365.0..366.0).contains(&days),
        "365 days, with the clock-skew allowance before: {days}"
    );
}

#[test]
fn a_service_certificate_names_the_service_and_the_names_it_is_reached_by() {
    let clock = clock();
    let ca = authority(&clock);
    let identity = service(
        &ca,
        &clock,
        ServiceName::Gateway,
        &["gateway", "gateway.orch.internal", "127.0.0.1"],
    );
    let certificate = &identity.chain[0];
    assert_eq!(certificate.service(), Some(ServiceName::Gateway));
    assert!(certificate.allows_client_auth() && certificate.allows_server_auth());
    assert_eq!(
        certificate.dns_names(),
        ["gateway", "gateway.orch.internal"]
    );
}

#[test]
fn what_a_request_asks_for_beyond_its_key_is_not_read() {
    let clock = clock();
    let ca = authority(&clock);
    let other = new_id();
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "I am the gateway");
    params.subject_alt_names.push(SanType::URI(
        format!("orch://machine/{other}").try_into().unwrap(),
    ));
    params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let pem = params.serialize_request(&key).unwrap().pem().unwrap();
    let request = CertificateRequest::from_pem(&pem).unwrap();
    let mine = new_id();
    let certificate = ca
        .issue(&request, &Holder::Machine(mine), 365, &clock)
        .unwrap();
    assert_eq!(certificate.identities(), [Identity::Machine(mine)]);
    assert!(!certificate.is_ca());
    assert_eq!(
        certificate.subject_common_name(),
        Some(Identity::Machine(mine).uri().as_str())
    );
}

#[test]
fn serial_numbers_are_drawn_at_random() {
    let clock = clock();
    let ca = authority(&clock);
    let serials: Vec<Vec<u8>> = (0..20)
        .map(|_| machine(&ca, &clock).1.chain[0].serial().to_vec())
        .collect();
    for serial in &serials {
        assert!(serial.len() >= 16, "128 bits, plus a sign byte at most");
    }
    let mut unique = serials.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), serials.len());
}

#[test]
fn a_request_over_a_key_that_is_not_p256_is_refused() {
    for algorithm in [&rcgen::PKCS_ED25519, &rcgen::PKCS_ECDSA_P384_SHA384] {
        let key = KeyPair::generate_for(algorithm).unwrap();
        let pem = CertificateParams::default()
            .serialize_request(&key)
            .unwrap()
            .pem()
            .unwrap();
        assert!(CertificateRequest::from_pem(&pem).is_err());
    }
}

#[test]
fn a_request_that_was_altered_after_it_was_signed_is_refused() {
    let key = LeafKey::generate().unwrap();
    let pem = key.request_pem().unwrap();
    let block = x509_parser::pem::parse_x509_pem(pem.as_bytes()).unwrap().1;
    let mut der = block.contents;
    // The subject is inside what is signed.
    let at = der.windows(4).position(|window| window == b"orch").unwrap();
    der[at] = b'x';
    assert!(CertificateRequest::from_pem(&pem_encode("CERTIFICATE REQUEST", &der)).is_err());
}

#[test]
fn the_authority_is_made_once_and_its_key_is_for_its_owner_alone() {
    let clock = clock();
    let directory = tempfile::tempdir().unwrap();
    let files = CaFiles::in_dir(directory.path());
    assert!(!files.exist());
    let (first, made) = files.create_if_missing("Test", 3_650, &clock).unwrap();
    assert!(made);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(files.key_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
    let (second, made_again) = files.create_if_missing("Another", 3_650, &clock).unwrap();
    assert!(
        !made_again,
        "a second authority would cut off every machine of the first"
    );
    assert_eq!(first.pin(), second.pin());
    assert_eq!(files.load().unwrap().pin(), first.pin());
}

#[test]
fn a_key_that_is_not_the_certificates_is_not_an_authority() {
    let clock = clock();
    let one = authority(&clock);
    let other = authority(&clock);
    let error =
        CertificateAuthority::from_pem(&one.certificate_pem(), &other.key_pem()).unwrap_err();
    assert!(
        error.to_string().contains("not the certificate's"),
        "{error}"
    );
}

#[test]
fn a_leaf_is_not_taken_for_an_authority() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, identity) = machine(&ca, &clock);
    let error = CertificateAuthority::from_pem(&identity.chain[0].pem(), &identity.key.to_pem())
        .unwrap_err();
    assert!(error.to_string().contains("no authority's"), "{error}");
}

#[test]
fn a_certificate_is_known_by_the_hash_of_its_bytes_and_an_authority_by_the_hash_of_its_key() {
    let clock = clock();
    let ca = authority(&clock);
    let (_, identity) = machine(&ca, &clock);
    let certificate = &identity.chain[0];
    assert_eq!(certificate.fingerprint().len(), 64);
    assert!(
        certificate
            .fingerprint()
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_ne!(certificate.fingerprint(), ca.certificate().fingerprint());
    assert_ne!(ca.pin(), certificate.spki_sha256());
}
