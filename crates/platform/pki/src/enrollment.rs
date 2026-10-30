//! Enrollment token and trust-pin mechanisms.
//!
//! A machine joins a system with a token that somebody made for it. The
//! token carries two things: a secret that the system knows it by, good
//! once and for a quarter of an hour, and the pin of the system's authority.
//! The pin is why a token copied from a screen is enough: the machine trusts
//! nothing at first contact but a server whose authority has that pin, so a
//! token that reaches the wrong system gets the wrong system nothing.
//!
//! This module has both halves. The system's half makes tokens, remembers
//! them, and turns a request into a certificate for a new machine, or into
//! a refusal that leaves nothing behind. The machine's half makes a key that
//! never leaves it, a request over it, and, when the answer comes, checks
//! it against the pin before it keeps anything.

use crate::PkiError;
use crate::ca::{CertificateAuthority, Holder};
use crate::certificates::{Certificate, CertificateRequest, LeafKey, write_private_file};
use crate::verification::{OrchClientVerifier, TlsIdentity, crypto};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_link_contract::enrollment::{EnrollRefusal, EnrollReply, EnrollRequest, EnrollmentToken};
use orch_link_contract::version::ProtocolRange;
use orch_types::{Clock, Id, Time, new_id};
use rustls::pki_types::UnixTime;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// The bytes of random in a token's secret: 256 bits, so that nobody finds
/// a live token by trying.
pub const TOKEN_SECRET_BYTES: usize = 32;
/// The file a machine keeps its private key in, inside its identity
/// directory.
pub const KEY_FILE: &str = "key.pem";
/// The file a machine keeps its certificate in.
pub const CERT_FILE: &str = "cert.pem";
/// The file a machine keeps its authority's certificate in.
pub const CA_FILE: &str = "ca.pem";
/// The suffix of the file a renewed file waits in until it is moved into
/// place.
const NEXT_SUFFIX: &str = ".next";

/// What a token was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenPurpose {
    /// A person's machine, or a managed runtime's connector.
    Machine,
    /// The node of a host.
    Host,
}

/// What a token that was redeemed grants: the one who made it owns what it
/// enrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenGrant {
    /// What the token is for.
    pub purpose: TokenPurpose,
    /// Whoever made it, who owns the machine that comes of it.
    pub created_by: Id,
}

/// A token as it was just made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedToken {
    /// The token, which is shown once.
    pub token: EnrollmentToken,
    /// What the system keeps in its place: the SHA-256 of the secret, in
    /// lowercase hexadecimal.
    pub digest: String,
    /// When it stops being good.
    pub expires_at: Time,
}

/// What the system stores of a token instead of the token: the hash of
/// its secret.
#[must_use]
pub fn token_digest(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// Makes a token for `authority`, good for `valid_for` from now.
#[must_use]
pub fn issue_token(
    authority: &CertificateAuthority,
    valid_for: Duration,
    clock: &dyn Clock,
) -> IssuedToken {
    let mut random = [0u8; TOKEN_SECRET_BYTES];
    rand::fill(&mut random[..]);
    let secret = URL_SAFE_NO_PAD.encode(random);
    let token = EnrollmentToken::new(&secret, authority.pin());
    let expires_at =
        Time::new(clock.time().get() + chrono::Duration::from_std(valid_for).unwrap_or_default());
    IssuedToken {
        digest: token_digest(token.secret()),
        token,
        expires_at,
    }
}

/// Where the system keeps its tokens: wherever it keeps its machines.
pub trait TokenLedger: Send + Sync {
    /// Spends the token whose secret hashes to `digest`: once, and only
    /// while it is good at `now`. A token that is unknown, past its time or
    /// spent already is gone.
    fn redeem(&self, digest: &str, now: Time) -> Result<TokenGrant, EnrollRefusal>;
}

#[derive(Debug)]
struct LedgerEntry {
    expires_at: Time,
    grant: TokenGrant,
    spent: bool,
}

/// A ledger in memory, for tests and for a system that starts empty.
#[derive(Debug, Default)]
pub struct MemoryLedger {
    tokens: Mutex<HashMap<String, LedgerEntry>>,
}

impl MemoryLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Remembers a token that was just made.
    pub fn record(&self, issued: &IssuedToken, grant: TokenGrant) {
        self.tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                issued.digest.clone(),
                LedgerEntry {
                    expires_at: issued.expires_at,
                    grant,
                    spent: false,
                },
            );
    }
}

impl TokenLedger for MemoryLedger {
    fn redeem(&self, digest: &str, now: Time) -> Result<TokenGrant, EnrollRefusal> {
        let mut tokens = self
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = tokens.get_mut(digest).ok_or(EnrollRefusal::Gone)?;
        if entry.spent || now >= entry.expires_at {
            return Err(EnrollRefusal::Gone);
        }
        entry.spent = true;
        Ok(entry.grant)
    }
}

/// Why an enrollment was not carried out.
#[derive(Debug, thiserror::Error)]
pub enum EnrollError {
    /// The request is refused, and nothing was made.
    #[error("{0}")]
    Refused(#[from] EnrollRefusal),
    /// The system could not make the certificate.
    #[error("{0}")]
    Internal(#[from] PkiError),
}

/// A machine that was enrolled.
#[derive(Debug, Clone)]
pub struct Enrolled {
    /// The machine's identifier from now on.
    pub machine_id: Id,
    /// Its certificate.
    pub certificate: Certificate,
    /// The protocol version the two will speak.
    pub protocol_version: u32,
    /// What the token granted: who owns the machine.
    pub grant: TokenGrant,
}

/// Carries out an enrollment: a certificate for a new machine, or a refusal.
/// Everything that can be wrong with the request is looked at before the
/// token is spent, so that a request that is refused for being malformed
/// does not use the token up; the token is spent last, and nothing is made
/// unless it was good.
pub fn enroll(
    authority: &CertificateAuthority,
    ledger: &dyn TokenLedger,
    request: &EnrollRequest,
    validity_days: u64,
    clock: &dyn Clock,
) -> Result<Enrolled, EnrollError> {
    let token: EnrollmentToken = request.token.parse().map_err(|_| EnrollRefusal::Invalid)?;
    if !token.pins(&authority.pin()) {
        return Err(EnrollRefusal::Invalid.into());
    }
    let protocol_version = ProtocolRange::SUPPORTED
        .choose(&request.protocol_versions)
        .ok_or(EnrollRefusal::ProtocolUnsupported)?;
    if request.name.trim().is_empty() || request.roles.is_empty() {
        return Err(EnrollRefusal::Invalid.into());
    }
    let csr = CertificateRequest::from_pem(&request.csr_pem).map_err(|_| EnrollRefusal::Invalid)?;
    let now = clock.time();
    let grant = ledger.redeem(&token_digest(token.secret()), now)?;
    let machine_id = new_id();
    let certificate = authority.issue(&csr, &Holder::Machine(machine_id), validity_days, clock)?;
    Ok(Enrolled {
        machine_id,
        certificate,
        protocol_version,
        grant,
    })
}

/// Why a machine does not accept what a system answered to its enrollment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TrustFault {
    /// The authority in the answer is not the one the token pins.
    #[error("the answer's authority is not the one the token pins")]
    WrongPin,
    /// The answer holds a certificate or an authority that cannot be read, or
    /// that is not what an authority or a machine's certificate is.
    #[error("the answer's certificates are not what they should be")]
    Malformed,
    /// The certificate was not issued by that authority.
    #[error("the certificate was not issued by that authority")]
    NotIssued,
    /// The certificate is not for the key this machine made, or not for the
    /// machine the answer names.
    #[error("the certificate is not for this machine's key")]
    NotMine,
}

/// A machine's keys and certificates: what it keeps in its identity
/// directory.
#[derive(Debug)]
pub struct MachineIdentity {
    /// The machine's identifier.
    pub machine_id: Id,
    /// Its private key.
    pub key: LeafKey,
    /// Its certificate.
    pub certificate: Certificate,
    /// The system's authority.
    pub authority: Certificate,
}

impl MachineIdentity {
    /// What the machine shows in a TLS connection.
    pub fn tls(&self) -> Result<TlsIdentity, PkiError> {
        Ok(TlsIdentity {
            chain: vec![self.certificate.clone()],
            key: self.key.duplicate()?,
        })
    }

    /// Writes the identity into `directory`: `key.pem` and `cert.pem`,
    /// readable by the owner alone, and `ca.pem`.
    pub fn save(&self, directory: &Path) -> Result<(), PkiError> {
        write_private_file(&directory.join(KEY_FILE), &self.key.to_pem())?;
        write_private_file(&directory.join(CERT_FILE), &self.certificate.pem())?;
        write_private_file(&directory.join(CA_FILE), &self.authority.pem())
    }

    /// Reads an identity from `directory`. A renewal that was cut off
    /// between its two files is finished first: the new key is in place and
    /// the new certificate waits beside it.
    pub fn load(directory: &Path) -> Result<Self, PkiError> {
        let read = |name: &str| {
            let path = directory.join(name);
            std::fs::read_to_string(&path).map_err(|source| PkiError::File {
                path: path.display().to_string(),
                source,
            })
        };
        let key = LeafKey::from_pem(&read(KEY_FILE)?)?;
        let waiting = directory.join(format!("{CERT_FILE}{NEXT_SUFFIX}"));
        let mut certificate = Certificate::from_pem(&read(CERT_FILE)?)?;
        if !certificate.holds(&key) && waiting.exists() {
            let next = Certificate::from_pem(&read(&format!("{CERT_FILE}{NEXT_SUFFIX}"))?)?;
            if next.holds(&key) {
                std::fs::rename(&waiting, directory.join(CERT_FILE)).map_err(|source| {
                    PkiError::File {
                        path: waiting.display().to_string(),
                        source,
                    }
                })?;
                certificate = next;
            }
        }
        if !certificate.holds(&key) {
            return Err(PkiError::Certificate(
                "the key and the certificate of this machine do not belong together".to_owned(),
            ));
        }
        let machine_id = certificate
            .machine_id()
            .ok_or_else(|| PkiError::Certificate("the certificate names no machine".to_owned()))?;
        Ok(Self {
            machine_id,
            key,
            certificate,
            authority: Certificate::from_pem(&read(CA_FILE)?)?,
        })
    }

    /// Puts a renewed key and certificate in place of the old ones. The
    /// certificate is written beside the old one first and the key moved
    /// before it, so that whichever way this is cut off, [`MachineIdentity::load`]
    /// finds a key and a certificate that belong together.
    pub fn replace(
        &mut self,
        directory: &Path,
        key: LeafKey,
        certificate: Certificate,
    ) -> Result<(), PkiError> {
        let file_error = |path: &Path, source: std::io::Error| PkiError::File {
            path: path.display().to_string(),
            source,
        };
        let waiting_key = directory.join(format!("{KEY_FILE}{NEXT_SUFFIX}"));
        let waiting_cert = directory.join(format!("{CERT_FILE}{NEXT_SUFFIX}"));
        write_private_file(&waiting_cert, &certificate.pem())?;
        write_private_file(&waiting_key, &key.to_pem())?;
        std::fs::rename(&waiting_key, directory.join(KEY_FILE))
            .map_err(|e| file_error(&waiting_key, e))?;
        std::fs::rename(&waiting_cert, directory.join(CERT_FILE))
            .map_err(|e| file_error(&waiting_cert, e))?;
        self.key = key;
        self.certificate = certificate;
        Ok(())
    }
}

/// A machine's enrollment that has been asked for and not answered: the key
/// it made, and the request over it.
#[derive(Debug)]
pub struct PendingEnrollment {
    key: LeafKey,
    csr_pem: String,
}

impl PendingEnrollment {
    /// Makes a key and a request over it.
    pub fn start() -> Result<Self, PkiError> {
        let key = LeafKey::generate()?;
        let csr_pem = key.request_pem()?;
        Ok(Self { key, csr_pem })
    }

    /// The request to send.
    #[must_use]
    pub fn csr_pem(&self) -> &str {
        &self.csr_pem
    }

    /// Checks the system's answer against the token's pin and the machine's
    /// own key, and only then returns the identity to keep. Nothing is kept
    /// here: a refusal leaves no file and no state behind.
    ///
    /// On both trust paths the authority in the answer has to be the one the
    /// token pins; the certificate has to be issued by it, for this machine's
    /// key, naming the machine the answer names.
    pub fn complete(
        self,
        token: &EnrollmentToken,
        reply: &EnrollReply,
        clock: &dyn Clock,
    ) -> Result<MachineIdentity, TrustFault> {
        let authority = Certificate::from_pem(&reply.ca_pem).map_err(|_| TrustFault::Malformed)?;
        if !authority.is_ca() || !authority.signs_certificates() {
            return Err(TrustFault::Malformed);
        }
        if !token.pins(&authority.spki_sha256()) {
            return Err(TrustFault::WrongPin);
        }
        let certificate =
            Certificate::from_pem(&reply.certificate_pem).map_err(|_| TrustFault::Malformed)?;
        let provider = crypto();
        let verifier = OrchClientVerifier::new(&authority, Duration::ZERO, &provider)
            .map_err(|_| TrustFault::Malformed)?;
        let seconds = u64::try_from(clock.now().timestamp()).unwrap_or(0);
        verifier
            .check(
                &certificate.to_rustls(),
                &[],
                UnixTime::since_unix_epoch(Duration::from_secs(seconds)),
            )
            .map_err(|_| TrustFault::NotIssued)?;
        if !certificate.holds(&self.key) || certificate.machine_id() != Some(reply.machine_id) {
            return Err(TrustFault::NotMine);
        }
        Ok(MachineIdentity {
            machine_id: reply.machine_id,
            key: self.key,
            certificate,
            authority,
        })
    }
}
