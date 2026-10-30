//! Joining a system with a token, and renewing the certificate that came of it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use chrono::Duration as Days;
use common::{authority, clock, machine, service};
use orch_link_contract::enrollment::{
    Endpoint, EnrollRefusal, EnrollReply, EnrollReplySystem, EnrollRequest, EnrollmentToken, Trust,
    renewal_due,
};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_pki::ca::CertificateAuthority;
use orch_pki::certificates::{CertificateRequest, LeafKey, ServiceName};
use orch_pki::enrollment::{
    EnrollError, MachineIdentity, MemoryLedger, PendingEnrollment, TokenGrant, TokenPurpose,
    TrustFault, enroll, issue_token,
};
use orch_pki::renewal::{
    MachineRecord, RenewalError, RenewalPolicy, RenewalProofFault, RenewalRefusal, authorize, due,
    renew, sign_request, verify_proof,
};
use orch_pki::verification::ProofFault;
use orch_types::{Clock, Id, ManualClock, new_id};
use std::time::Duration;

const QUARTER_HOUR: Duration = Duration::from_secs(900);
const GRACE_DAYS: u64 = 90;
const POLICY: RenewalPolicy = RenewalPolicy {
    validity_days: 365,
    grace_days: GRACE_DAYS,
    service_names: Vec::new(),
};

fn request_for(token: &EnrollmentToken, csr_pem: &str) -> EnrollRequest {
    EnrollRequest {
        machine_id: None,
        token: token.to_string(),
        csr_pem: csr_pem.to_owned(),
        name: "alice-laptop".to_owned(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        connector_version: "0.1.0".to_owned(),
        protocol_versions: vec![1],
        roles: vec![Role::Agent, Role::Environment],
    }
}

struct System {
    clock: ManualClock,
    authority: CertificateAuthority,
    ledger: MemoryLedger,
}

fn system() -> System {
    let clock = clock();
    let authority = authority(&clock);
    System {
        clock,
        authority,
        ledger: MemoryLedger::new(),
    }
}

impl System {
    fn token(&self) -> EnrollmentToken {
        let issued = issue_token(&self.authority, QUARTER_HOUR, &self.clock);
        self.ledger.record(
            &issued,
            TokenGrant {
                purpose: TokenPurpose::Machine,
                created_by: new_id(),
            },
        );
        issued.token
    }

    fn enroll(
        &self,
        request: &EnrollRequest,
    ) -> Result<orch_pki::enrollment::Enrolled, EnrollError> {
        enroll(&self.authority, &self.ledger, request, 365, &self.clock)
    }
}

fn refusal(result: Result<orch_pki::enrollment::Enrolled, EnrollError>) -> EnrollRefusal {
    match result.unwrap_err() {
        EnrollError::Refused(refusal) => refusal,
        EnrollError::Internal(error) => panic!("an internal failure: {error}"),
    }
}

#[test]
fn a_token_carries_the_pin_of_the_authority_that_made_it() {
    let system = system();
    let token = system.token();
    assert!(token.pins(&system.authority.pin()));
    let text = token.to_string();
    assert!(text.starts_with("oenr_"));
    assert_eq!(text.parse::<EnrollmentToken>().unwrap(), token);
}

#[test]
fn a_token_enrolls_a_machine_with_a_certificate_for_its_own_key_and_the_id_the_system_gave() {
    let system = system();
    let token = system.token();
    let pending = PendingEnrollment::start().unwrap();
    let enrolled = system
        .enroll(&request_for(&token, pending.csr_pem()))
        .unwrap();
    assert_eq!(enrolled.certificate.machine_id(), Some(enrolled.machine_id));
    assert_eq!(enrolled.protocol_version, 1);
    let reply = EnrollReply {
        machine_id: enrolled.machine_id,
        certificate_pem: enrolled.certificate.pem(),
        ca_pem: system.authority.certificate_pem(),
        endpoints: vec![Endpoint {
            url: "wss://gw.orch.internal:9443".to_owned(),
            trust: Trust::Pinned,
        }],
        system: EnrollReplySystem {
            name: "Orch".to_owned(),
            public_url: "https://orch.example.com".to_owned(),
        },
        protocol_version: 1,
    };
    let identity = pending.complete(&token, &reply, &system.clock).unwrap();
    assert_eq!(identity.machine_id, enrolled.machine_id);
    assert!(identity.certificate.holds(&identity.key));
}

#[test]
fn a_token_is_good_once() {
    let system = system();
    let token = system.token();
    let first = PendingEnrollment::start().unwrap();
    system
        .enroll(&request_for(&token, first.csr_pem()))
        .unwrap();
    let second = PendingEnrollment::start().unwrap();
    assert_eq!(
        refusal(system.enroll(&request_for(&token, second.csr_pem()))),
        EnrollRefusal::Gone
    );
}

#[test]
fn a_token_is_not_good_after_a_quarter_of_an_hour() {
    let system = system();
    let token = system.token();
    system.clock.advance(Days::seconds(14 * 60 + 59));
    let pending = PendingEnrollment::start().unwrap();
    system
        .enroll(&request_for(&token, pending.csr_pem()))
        .unwrap();
    let late = system.token();
    system.clock.advance(Days::minutes(16));
    let again = PendingEnrollment::start().unwrap();
    assert_eq!(
        refusal(system.enroll(&request_for(&late, again.csr_pem()))),
        EnrollRefusal::Gone
    );
}

#[test]
fn a_token_the_system_never_made_is_gone() {
    let system = system();
    let stranger = EnrollmentToken::new("made-up-secret", system.authority.pin());
    let pending = PendingEnrollment::start().unwrap();
    assert_eq!(
        refusal(system.enroll(&request_for(&stranger, pending.csr_pem()))),
        EnrollRefusal::Gone
    );
}

#[test]
fn a_token_that_pins_another_authority_is_not_one_of_this_system() {
    let system = system();
    let other = authority(&system.clock);
    let issued = issue_token(&other, QUARTER_HOUR, &system.clock);
    system.ledger.record(
        &issued,
        TokenGrant {
            purpose: TokenPurpose::Machine,
            created_by: new_id(),
        },
    );
    let pending = PendingEnrollment::start().unwrap();
    assert_eq!(
        refusal(system.enroll(&request_for(&issued.token, pending.csr_pem()))),
        EnrollRefusal::Invalid
    );
}

#[test]
fn a_request_refused_for_what_it_says_does_not_use_the_token_up() {
    let system = system();
    let token = system.token();
    let mut broken = request_for(&token, "not a request");
    assert_eq!(refusal(system.enroll(&broken)), EnrollRefusal::Invalid);
    broken.csr_pem = PendingEnrollment::start().unwrap().csr_pem().to_owned();
    broken.protocol_versions = vec![7];
    assert_eq!(
        refusal(system.enroll(&broken)),
        EnrollRefusal::ProtocolUnsupported
    );
    let pending = PendingEnrollment::start().unwrap();
    system
        .enroll(&request_for(&token, pending.csr_pem()))
        .unwrap();
}

#[test]
fn a_machine_keeps_nothing_from_an_answer_whose_authority_is_not_the_one_its_token_pins() {
    let system = system();
    let token = system.token();
    let intruder = authority(&system.clock);
    let issued = issue_token(&intruder, QUARTER_HOUR, &system.clock);
    let intruder_ledger = MemoryLedger::new();
    intruder_ledger.record(
        &issued,
        TokenGrant {
            purpose: TokenPurpose::Machine,
            created_by: new_id(),
        },
    );
    let pending = PendingEnrollment::start().unwrap();
    let asked = request_for(&issued.token, pending.csr_pem());
    let enrolled = enroll(&intruder, &intruder_ledger, &asked, 365, &system.clock).unwrap();
    let reply = EnrollReply {
        machine_id: enrolled.machine_id,
        certificate_pem: enrolled.certificate.pem(),
        ca_pem: intruder.certificate_pem(),
        endpoints: Vec::new(),
        system: EnrollReplySystem {
            name: "Fake".to_owned(),
            public_url: "https://fake.example".to_owned(),
        },
        protocol_version: 1,
    };
    // The machine holds the real system's token, so the answer of the intruder fails the pin, and there is
    // no identity to keep.
    let fault = pending.complete(&token, &reply, &system.clock).unwrap_err();
    assert_eq!(fault, TrustFault::WrongPin);
}

#[test]
fn a_certificate_for_another_key_than_the_machines_is_not_accepted() {
    let system = system();
    let token = system.token();
    let pending = PendingEnrollment::start().unwrap();
    let someone_else = PendingEnrollment::start().unwrap();
    let enrolled = system
        .enroll(&request_for(&token, someone_else.csr_pem()))
        .unwrap();
    let reply = EnrollReply {
        machine_id: enrolled.machine_id,
        certificate_pem: enrolled.certificate.pem(),
        ca_pem: system.authority.certificate_pem(),
        endpoints: Vec::new(),
        system: EnrollReplySystem {
            name: "Orch".to_owned(),
            public_url: String::new(),
        },
        protocol_version: 1,
    };
    assert_eq!(
        pending.complete(&token, &reply, &system.clock).unwrap_err(),
        TrustFault::NotMine
    );
}

#[test]
fn a_machines_identity_is_kept_in_files_only_it_reads_and_comes_back_whole() {
    let system = system();
    let (id, identity) = machine(&system.authority, &system.clock);
    let mine = MachineIdentity {
        machine_id: id,
        key: identity.key.duplicate().unwrap(),
        certificate: identity.chain[0].clone(),
        authority: system.authority.certificate().clone(),
    };
    let directory = tempfile::tempdir().unwrap();
    mine.save(directory.path()).unwrap();
    #[cfg(unix)]
    for name in ["key.pem", "cert.pem", "ca.pem"] {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(directory.path().join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{name}");
    }
    let back = MachineIdentity::load(directory.path()).unwrap();
    assert_eq!(back.machine_id, id);
    assert_eq!(back.certificate, mine.certificate);
}

#[test]
fn a_renewal_cut_off_between_its_two_files_is_finished_on_the_next_start() {
    let system = system();
    let (id, identity) = machine(&system.authority, &system.clock);
    let mut mine = MachineIdentity {
        machine_id: id,
        key: identity.key.duplicate().unwrap(),
        certificate: identity.chain[0].clone(),
        authority: system.authority.certificate().clone(),
    };
    let directory = tempfile::tempdir().unwrap();
    mine.save(directory.path()).unwrap();
    let (new_key, csr) = orch_pki::renewal::start().unwrap();
    let new_certificate = renew(
        &system.authority,
        &mine.certificate,
        Some(&MachineRecord {
            active: true,
            fingerprints: vec![mine.certificate.fingerprint()],
        }),
        &CertificateRequest::from_pem(&csr).unwrap(),
        &POLICY,
        &system.clock,
    )
    .unwrap();
    // The cut: the key was moved into place, and the certificate still waits beside the old one.
    std::fs::write(directory.path().join("key.pem"), new_key.to_pem()).unwrap();
    std::fs::write(
        directory.path().join("cert.pem.next"),
        new_certificate.pem(),
    )
    .unwrap();
    let found = MachineIdentity::load(directory.path()).unwrap();
    assert_eq!(found.certificate, new_certificate);
    assert!(found.certificate.holds(&found.key));
    assert!(!directory.path().join("cert.pem.next").exists());
    // And a whole replacement leaves a pair that belongs together.
    let (another_key, another_csr) = orch_pki::renewal::start().unwrap();
    let another = renew(
        &system.authority,
        &new_certificate,
        Some(&MachineRecord {
            active: true,
            fingerprints: vec![new_certificate.fingerprint()],
        }),
        &CertificateRequest::from_pem(&another_csr).unwrap(),
        &POLICY,
        &system.clock,
    )
    .unwrap();
    mine.replace(directory.path(), another_key, another.clone())
        .unwrap();
    assert_eq!(
        MachineIdentity::load(directory.path()).unwrap().certificate,
        another
    );
}

fn active(certificate: &orch_pki::certificates::Certificate) -> MachineRecord {
    MachineRecord {
        active: true,
        fingerprints: vec![certificate.fingerprint()],
    }
}

fn new_request() -> (LeafKey, CertificateRequest) {
    let (key, csr) = orch_pki::renewal::start().unwrap();
    (key, CertificateRequest::from_pem(&csr).unwrap())
}

#[test]
fn a_machine_is_due_for_renewal_after_two_thirds_of_its_certificate() {
    let system = system();
    let (_, identity) = machine(&system.authority, &system.clock);
    let certificate = &identity.chain[0];
    assert!(!due(certificate, system.clock.time()));
    system.clock.advance(Days::days(243));
    assert!(!due(certificate, system.clock.time()));
    system.clock.advance(Days::days(2));
    assert!(due(certificate, system.clock.time()));
    assert_eq!(
        due(certificate, system.clock.time()),
        renewal_due(
            certificate.not_before(),
            certificate.not_after(),
            system.clock.time()
        )
    );
}

#[test]
fn a_renewal_gives_the_same_machine_a_certificate_over_a_new_key() {
    let system = system();
    let (id, identity) = machine(&system.authority, &system.clock);
    let (key, request) = new_request();
    let renewed = renew(
        &system.authority,
        &identity.chain[0],
        Some(&active(&identity.chain[0])),
        &request,
        &POLICY,
        &system.clock,
    )
    .unwrap();
    assert_eq!(renewed.machine_id(), Some(id));
    assert!(renewed.holds(&key));
    assert!(!renewed.holds(&identity.key));
    assert_ne!(renewed.fingerprint(), identity.chain[0].fingerprint());
}

#[test]
fn a_renewal_over_the_key_the_machine_already_has_is_refused() {
    let system = system();
    let (_, identity) = machine(&system.authority, &system.clock);
    let same = CertificateRequest::from_pem(&identity.key.request_pem().unwrap()).unwrap();
    let error = authorize(
        &identity.chain[0],
        Some(&active(&identity.chain[0])),
        &same,
        system.clock.time(),
        GRACE_DAYS,
    )
    .unwrap_err();
    assert_eq!(error, RenewalRefusal::SameKey);
}

#[test]
fn a_machine_the_system_has_revoked_or_does_not_know_is_not_renewed() {
    let system = system();
    let (_, identity) = machine(&system.authority, &system.clock);
    let (_, request) = new_request();
    let now = system.clock.time();
    let revoked = MachineRecord {
        active: false,
        ..active(&identity.chain[0])
    };
    assert_eq!(
        authorize(
            &identity.chain[0],
            Some(&revoked),
            &request,
            now,
            GRACE_DAYS
        ),
        Err(RenewalRefusal::Revoked)
    );
    assert_eq!(
        authorize(&identity.chain[0], None, &request, now, GRACE_DAYS),
        Err(RenewalRefusal::Revoked)
    );
}

#[test]
fn a_certificate_the_system_no_longer_knows_the_machine_by_is_not_renewed() {
    let system = system();
    let (_, identity) = machine(&system.authority, &system.clock);
    let (_, request) = new_request();
    let other = MachineRecord {
        active: true,
        fingerprints: vec!["0".repeat(64)],
    };
    assert_eq!(
        authorize(
            &identity.chain[0],
            Some(&other),
            &request,
            system.clock.time(),
            GRACE_DAYS
        ),
        Err(RenewalRefusal::NotCurrent)
    );
}

#[test]
fn both_the_old_and_the_new_certificate_renew_until_the_new_one_has_held_a_session() {
    let system = system();
    let (_, identity) = machine(&system.authority, &system.clock);
    let (_, first) = new_request();
    let (_, second) = new_request();
    let renewed = renew(
        &system.authority,
        &identity.chain[0],
        Some(&active(&identity.chain[0])),
        &first,
        &POLICY,
        &system.clock,
    )
    .unwrap();
    let both = MachineRecord {
        active: true,
        fingerprints: vec![identity.chain[0].fingerprint(), renewed.fingerprint()],
    };
    let now = system.clock.time();
    authorize(&identity.chain[0], Some(&both), &second, now, GRACE_DAYS).unwrap();
    authorize(&renewed, Some(&both), &second, now, GRACE_DAYS).unwrap();
}

#[test]
fn a_certificate_that_ran_out_is_renewed_within_ninety_days_and_not_after() {
    let system = system();
    let (_, identity) = machine(&system.authority, &system.clock);
    let record = active(&identity.chain[0]);
    let (_, request) = new_request();
    system.clock.advance(Days::days(365 + 89));
    authorize(
        &identity.chain[0],
        Some(&record),
        &request,
        system.clock.time(),
        GRACE_DAYS,
    )
    .unwrap();
    system.clock.advance(Days::days(3));
    assert_eq!(
        authorize(
            &identity.chain[0],
            Some(&record),
            &request,
            system.clock.time(),
            GRACE_DAYS
        ),
        Err(RenewalRefusal::ExpiredBeyondGrace)
    );
}

#[test]
fn a_service_renews_while_its_certificate_is_good_and_keeps_its_names() {
    let system = system();
    let gateway = service(
        &system.authority,
        &system.clock,
        ServiceName::Gateway,
        &["gateway", "gateway.orch.internal"],
    );
    let (_, request) = new_request();
    let policy = RenewalPolicy {
        service_names: vec!["gateway".to_owned()],
        ..POLICY
    };
    let renewed = renew(
        &system.authority,
        &gateway.chain[0],
        None,
        &request,
        &policy,
        &system.clock,
    )
    .unwrap();
    assert_eq!(renewed.service(), Some(ServiceName::Gateway));
    assert_eq!(renewed.dns_names(), ["gateway"]);
    system.clock.advance(Days::days(366));
    let late = renew(
        &system.authority,
        &gateway.chain[0],
        None,
        &request,
        &policy,
        &system.clock,
    );
    assert!(matches!(
        late,
        Err(RenewalError::Refused(RenewalRefusal::ExpiredBeyondGrace))
    ));
}

fn proof_for(
    system: &System,
    id: Id,
    identity: &orch_pki::verification::TlsIdentity,
    csr: &str,
    host: &str,
) -> orch_pki::renewal::RenewalProof {
    sign_request(
        &identity.key,
        &identity.chain[0],
        id,
        csr,
        host,
        &system.clock,
    )
}

#[test]
fn through_the_public_edge_a_machine_signs_its_renewal_request_with_its_old_key() {
    let system = system();
    let (id, identity) = machine(&system.authority, &system.clock);
    let (_, csr) = orch_pki::renewal::start().unwrap();
    let proof = proof_for(&system, id, &identity, &csr, "orch.example.com");
    let seen = verify_proof(
        system.authority.certificate(),
        &proof,
        id,
        &csr,
        "orch.example.com",
        GRACE_DAYS,
        &system.clock,
    )
    .unwrap();
    assert_eq!(seen.machine_id(), Some(id));
}

#[test]
fn a_renewal_signature_is_good_for_that_request_and_that_host_only() {
    let system = system();
    let (id, identity) = machine(&system.authority, &system.clock);
    let (_, csr) = orch_pki::renewal::start().unwrap();
    let (_, another_csr) = orch_pki::renewal::start().unwrap();
    let proof = proof_for(&system, id, &identity, &csr, "orch.example.com");
    let check = |csr: &str, host: &str| {
        verify_proof(
            system.authority.certificate(),
            &proof,
            id,
            csr,
            host,
            GRACE_DAYS,
            &system.clock,
        )
    };
    assert_eq!(
        check(&another_csr, "orch.example.com").unwrap_err(),
        RenewalProofFault::Proof(ProofFault::BadSignature)
    );
    assert_eq!(
        check(&csr, "evil.example.com").unwrap_err(),
        RenewalProofFault::Proof(ProofFault::BadSignature)
    );
}

#[test]
fn a_renewal_signature_that_is_old_is_stale() {
    let system = system();
    let (id, identity) = machine(&system.authority, &system.clock);
    let (_, csr) = orch_pki::renewal::start().unwrap();
    let proof = proof_for(&system, id, &identity, &csr, "orch.example.com");
    system.clock.advance(Days::seconds(121));
    let error = verify_proof(
        system.authority.certificate(),
        &proof,
        id,
        &csr,
        "orch.example.com",
        GRACE_DAYS,
        &system.clock,
    )
    .unwrap_err();
    assert_eq!(error, RenewalProofFault::Stale);
}

#[test]
fn an_expired_certificate_proves_a_renewal_within_the_grace_and_not_after() {
    let system = system();
    let (id, identity) = machine(&system.authority, &system.clock);
    let (_, csr) = orch_pki::renewal::start().unwrap();
    system.clock.advance(Days::days(365 + 30));
    let proof = proof_for(&system, id, &identity, &csr, "h");
    verify_proof(
        system.authority.certificate(),
        &proof,
        id,
        &csr,
        "h",
        GRACE_DAYS,
        &system.clock,
    )
    .unwrap();
    system.clock.advance(Days::days(100));
    let proof = proof_for(&system, id, &identity, &csr, "h");
    let error = verify_proof(
        system.authority.certificate(),
        &proof,
        id,
        &csr,
        "h",
        GRACE_DAYS,
        &system.clock,
    )
    .unwrap_err();
    assert_eq!(error, RenewalProofFault::Proof(ProofFault::Expired));
}
