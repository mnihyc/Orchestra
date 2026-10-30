//! Key rotation and renewal mechanisms.
//!
//! A certificate is renewed before it runs out, with a new key: a machine
//! after two thirds of its life, a service when a third of it is left. The
//! new certificate names the same holder; what changes is the key, so that a
//! key that leaked once is worth nothing a year later.
//!
//! One case needs care: a machine that was off for a season comes back with a
//! certificate that has run out. For ninety days that is forgiven, for one
//! purpose only. The machine cannot hold a session, but it may ask for a new
//! certificate, and it is given one if the system still knows it as active
//! and still knows that certificate as its own.
//!
//! The request is authenticated by the connection when the connection
//! carries the certificate. Through the public edge it does not, and the
//! machine signs the request itself with the old key.

use crate::PkiError;
use crate::ca::{CertificateAuthority, Holder};
use crate::certificates::{Certificate, CertificateRequest, Identity, LeafKey, verify_signature};
use crate::verification::{ProofFault, verify_proof_with_grace};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_link_contract::enrollment::{renewable_after_expiry, renewal_due};
use orch_link_contract::session::clock_agrees;
use orch_types::{Clock, Id, Time};
use sha2::{Digest, Sha256};
use std::time::Duration;

/// The first line of what a machine signs to ask for a new certificate
/// through an edge that does not show its old one. It names the purpose and
/// the version, so that the signature is good for nothing else.
pub use orch_link_contract::enrollment::RENEWAL_CONTEXT;
/// Seconds in a day.
const DAY_SECS: u64 = 86_400;

/// What the system knows about a machine when it is asked to renew its
/// certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineRecord {
    /// Whether the machine may hold a session: it was not revoked or
    /// switched off.
    pub active: bool,
    /// The fingerprints the system accepts as the machine's: the current
    /// certificate and, until a session with it was seen, the one that
    /// is about to replace it.
    pub fingerprints: Vec<String>,
}

/// Why a renewal is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RenewalRefusal {
    /// The system does not know the machine, or has revoked it.
    #[error("the machine is unknown or revoked")]
    Revoked,
    /// The certificate presented is not the one the system knows the machine
    /// by.
    #[error("the certificate is not the machine's current one")]
    NotCurrent,
    /// The certificate ran out longer ago than a renewal forgives.
    #[error("the certificate expired beyond the renewal grace")]
    ExpiredBeyondGrace,
    /// The request asks for the key the certificate already has.
    #[error("a renewal has to use a new key")]
    SameKey,
    /// The certificate names nobody who can renew.
    #[error("the certificate names nobody who can renew")]
    NobodyToRenew,
}

/// Whether `certificate` is due for renewal at `now`: two thirds of its
/// time have passed.
#[must_use]
pub fn due(certificate: &Certificate, now: Time) -> bool {
    renewal_due(certificate.not_before(), certificate.not_after(), now)
}

/// Decides whether the holder of `presented` may be given a new certificate
/// for the key of `request`. For a machine that takes what the system
/// knows of it in `record`, and forgives a certificate that ran out less
/// than `grace_days` ago. A service renews while its certificate is good.
pub fn authorize(
    presented: &Certificate,
    record: Option<&MachineRecord>,
    request: &CertificateRequest,
    now: Time,
    grace_days: u64,
) -> Result<Identity, RenewalRefusal> {
    if request.public_point() == presented.public_point() {
        return Err(RenewalRefusal::SameKey);
    }
    match presented.identities() {
        [Identity::Machine(id)] => {
            let record = record.ok_or(RenewalRefusal::Revoked)?;
            if !record.active {
                return Err(RenewalRefusal::Revoked);
            }
            if !record.fingerprints.contains(&presented.fingerprint()) {
                return Err(RenewalRefusal::NotCurrent);
            }
            let grace = i64::try_from(grace_days).unwrap_or(i64::MAX);
            if !presented.valid_at(now)
                && !renewable_after_expiry(presented.not_after(), now, grace)
            {
                return Err(RenewalRefusal::ExpiredBeyondGrace);
            }
            Ok(Identity::Machine(*id))
        }
        [Identity::Service(service)] => {
            if !presented.valid_at(now) {
                return Err(RenewalRefusal::ExpiredBeyondGrace);
            }
            Ok(Identity::Service(*service))
        }
        _ => Err(RenewalRefusal::NobodyToRenew),
    }
}

/// Why a renewal could not be carried out.
#[derive(Debug, thiserror::Error)]
pub enum RenewalError {
    /// The renewal is refused.
    #[error("{0}")]
    Refused(#[from] RenewalRefusal),
    /// The certificate could not be made.
    #[error("{0}")]
    Internal(#[from] PkiError),
}

/// What a renewal is carried out under: the settings of the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewalPolicy {
    /// How long the new certificate is good for, in days.
    pub validity_days: u64,
    /// How long after it ran out a machine's certificate is still
    /// renewed, in days.
    pub grace_days: u64,
    /// The names a service's new certificate is good for; a machine's has
    /// none.
    pub service_names: Vec<String>,
}

/// Renews: authorizes, then issues a certificate for the same holder over
/// the request's key.
pub fn renew(
    authority: &CertificateAuthority,
    presented: &Certificate,
    record: Option<&MachineRecord>,
    request: &CertificateRequest,
    policy: &RenewalPolicy,
    clock: &dyn Clock,
) -> Result<Certificate, RenewalError> {
    let holder = match authorize(presented, record, request, clock.time(), policy.grace_days)? {
        Identity::Machine(id) => Holder::Machine(id),
        Identity::Service(service) => Holder::Service {
            service,
            names: policy.service_names.clone(),
        },
    };
    Ok(authority.issue(request, &holder, policy.validity_days, clock)?)
}

/// Makes the key and the request for a renewal: a new key, as the system
/// demands.
pub fn start() -> Result<(LeafKey, String), PkiError> {
    let key = LeafKey::generate()?;
    let request = key.request_pem()?;
    Ok((key, request))
}

/// The bytes a machine signs to ask for a renewal through an edge that sees
/// no client certificate: the purpose, the hash of the request, the machine,
/// the host it connected to and its clock, each on a line of its own.
/// Binding the request means that a signature taken off the wire can be
/// used for that request only, which asks for a certificate for a key that
/// the taker does not hold.
#[must_use]
pub fn proof_signing_input(csr_pem: &str, machine_id: Id, host: &str, sent_at: Time) -> String {
    let request = hex::encode(Sha256::digest(csr_pem.as_bytes()));
    format!("{RENEWAL_CONTEXT}\n{request}\n{machine_id}\n{host}\n{sent_at}")
}

/// A machine's proof, sent with a renewal request that arrives through an
/// edge that did not see its certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewalProof {
    /// The machine's current certificate, as PEM.
    pub certificate_pem: String,
    /// The signature, unpadded base64url of the 64 bytes `r ‖ s`.
    pub signature: String,
    /// The machine's clock when it signed.
    pub sent_at: Time,
}

/// Signs a renewal request with the machine's current key.
#[must_use]
pub fn sign_request(
    key: &LeafKey,
    certificate: &Certificate,
    machine_id: Id,
    csr_pem: &str,
    host: &str,
    clock: &dyn Clock,
) -> RenewalProof {
    let sent_at = clock.time();
    let input = proof_signing_input(csr_pem, machine_id, host, sent_at);
    RenewalProof {
        certificate_pem: certificate.pem(),
        signature: URL_SAFE_NO_PAD.encode(key.sign(input.as_bytes())),
        sent_at,
    }
}

/// Why a renewal proof is not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RenewalProofFault {
    /// The proof was signed too long ago or too far ahead for the system's
    /// clock.
    #[error("the proof is out of step with the system's clock")]
    Stale,
    /// The certificate or the signature does not hold.
    #[error("{0}")]
    Proof(#[from] ProofFault),
}

/// Checks a renewal proof and returns the certificate it proves: it has to be
/// good, or have run out less than `grace_days` ago, and its key has to have
/// signed this request for this host at a moment near `clock`'s.
pub fn verify_proof(
    authority: &Certificate,
    proof: &RenewalProof,
    machine_id: Id,
    csr_pem: &str,
    host: &str,
    grace_days: u64,
    clock: &dyn Clock,
) -> Result<Certificate, RenewalProofFault> {
    let now = clock.time();
    if !clock_agrees(proof.sent_at, now) {
        return Err(RenewalProofFault::Stale);
    }
    let signature = URL_SAFE_NO_PAD
        .decode(&proof.signature)
        .map_err(|_| ProofFault::Malformed)?;
    let input = proof_signing_input(csr_pem, machine_id, host, proof.sent_at);
    let certificate = verify_proof_with_grace(
        authority,
        &proof.certificate_pem,
        &signature,
        input.as_bytes(),
        now,
        Duration::from_secs(grace_days.saturating_mul(DAY_SECS)),
    )?;
    if certificate.machine_id() != Some(machine_id)
        || !verify_signature(certificate.public_point(), input.as_bytes(), &signature)
    {
        return Err(ProofFault::BadSignature.into());
    }
    Ok(certificate)
}
