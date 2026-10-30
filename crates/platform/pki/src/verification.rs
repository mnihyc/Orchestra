//! Pinned and public-trust verification.
//!
//! Machines believe the expected Orch service under their own authority, or a server the device
//! trusts for the name dialled. The same decision serves both gateway entrances and Orch's web edge.
//!
//! On the **pinned** path the connection is TLS under the system's own
//! authority: the gateway shows a certificate that authority issued, and wants one from the machine.
//! Its service identity is checked instead of the address dialled. At first contact the machine
//! does not even have the authority's certificate, only its pin from the
//! enrollment token, and accepts a server whose chain ends in an authority
//! with that pin.
//!
//! On the **public** path the connection ends at an edge that shows a
//! certificate any browser would take, and asks for none. The machine
//! accepts Orch's own web-edge identity or checks the edge against the public and machine roots,
//! and proves who it is inside the
//! connection: it signs what the gateway sent with the key of its
//! certificate.
//!
//! The verifiers here are the second half of each path; the configurations
//! put them together with the right identity and the right clock, so that
//! expiry is judged by the clock a caller gives and not by the machine's.

use crate::PkiError;
use crate::certificates::{Certificate, Identity, LeafKey, ServiceName, verify_signature};
use orch_types::{Clock, Time};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::time_provider::TimeProvider;
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, Error as TlsError,
    RootCertStore, ServerConfig, SignatureScheme,
};
use std::sync::Arc;
use std::time::Duration;
use subtle::ConstantTimeEq;

/// Seconds in a day.
const DAY_SECS: u64 = 86_400;

/// Public roots and the machine's installed authorities, including its certificate file/directory overrides.
#[must_use]
pub fn trusted_roots() -> Arc<RootCertStore> {
    let mut roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    Arc::new(roots)
}

/// A certificate and its key, with the chain that leads to the authority:
/// what a side of a connection shows.
#[derive(Debug)]
pub struct TlsIdentity {
    /// The certificate of the holder first, then the authority's if it is
    /// to be sent along.
    pub chain: Vec<Certificate>,
    /// The key of the first certificate.
    pub key: LeafKey,
}

impl TlsIdentity {
    fn chain_der(&self) -> Vec<CertificateDer<'static>> {
        self.chain.iter().map(Certificate::to_rustls).collect()
    }
}

/// How a client decides whether the server is the system's.
#[derive(Debug, Clone)]
pub enum ServerTrust {
    /// The server's chain has to end in this authority.
    Authority(Box<Certificate>),
    /// The server's chain has to carry an authority whose public key has
    /// this SHA-256: at first contact, when only the token's pin is known.
    Pin([u8; 32]),
}

/// The wall clock of the verifiers: the clock a caller gives, so that a
/// certificate is expired when that clock says so.
#[derive(Debug)]
pub struct ClockTime(Arc<dyn Clock>);

impl ClockTime {
    /// Takes the time from `clock`.
    #[must_use]
    pub fn provider(clock: Arc<dyn Clock>) -> Arc<dyn TimeProvider> {
        Arc::new(Self(clock))
    }
}

impl TimeProvider for ClockTime {
    fn current_time(&self) -> Option<UnixTime> {
        let seconds = u64::try_from(self.0.now().timestamp()).ok()?;
        Some(UnixTime::since_unix_epoch(Duration::from_secs(seconds)))
    }
}

/// The cryptography every verifier and configuration here uses.
#[must_use]
pub fn crypto() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn unix(time: Time) -> UnixTime {
    let seconds = u64::try_from(time.unix_millis().div_euclid(1_000)).unwrap_or(0);
    UnixTime::since_unix_epoch(Duration::from_secs(seconds))
}

fn roots_of(authority: &Certificate) -> Result<Arc<RootCertStore>, TlsError> {
    let mut roots = RootCertStore::empty();
    roots.add(authority.to_rustls())?;
    Ok(Arc::new(roots))
}

fn app_failure() -> TlsError {
    TlsError::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
}

fn bad_encoding() -> TlsError {
    TlsError::InvalidCertificate(CertificateError::BadEncoding)
}

/// Judges the certificate of a client against the system's authority: it has
/// to chain to it, allow client use, and name somebody of the system. A
/// certificate that ran out less than `grace` ago is let through, so that the
/// request to renew it can be heard; whoever acts on the connection decides
/// what an expired certificate may do.
#[derive(Debug)]
pub struct OrchClientVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    algorithms: WebPkiSupportedAlgorithms,
    grace: Duration,
}

impl OrchClientVerifier {
    /// A verifier for clients of `authority`, tolerating certificates that
    /// expired up to `grace` ago.
    pub fn new(
        authority: &Certificate,
        grace: Duration,
        provider: &Arc<CryptoProvider>,
    ) -> Result<Arc<Self>, PkiError> {
        let roots = roots_of(authority).map_err(|e| PkiError::Certificate(e.to_string()))?;
        let inner = WebPkiClientVerifier::builder_with_provider(roots, Arc::clone(provider))
            .build()
            .map_err(|e| PkiError::Certificate(e.to_string()))?;
        Ok(Arc::new(Self {
            inner,
            algorithms: provider.signature_verification_algorithms,
            grace,
        }))
    }

    /// Checks `end_entity` as the chain of a client at `now`. Used by the
    /// handshake, and by whoever has the certificate from somewhere else:
    /// a proof sent inside a connection that carried none.
    pub fn check(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<(), TlsError> {
        let named = Certificate::from_der(end_entity).map_err(|_| bad_encoding())?;
        if named.identities().is_empty() {
            return Err(app_failure());
        }
        match self
            .inner
            .verify_client_cert(end_entity, intermediates, now)
        {
            Ok(_) => Ok(()),
            Err(TlsError::InvalidCertificate(
                CertificateError::Expired | CertificateError::ExpiredContext { .. },
            )) => {
                let ran_out = named.not_after();
                let late = now.as_secs().saturating_sub(unix(ran_out).as_secs());
                if self.grace.is_zero() || late > self.grace.as_secs() {
                    return Err(TlsError::InvalidCertificate(CertificateError::Expired));
                }
                // Judge everything else about the certificate as of the last
                // moment it was good.
                self.inner
                    .verify_client_cert(end_entity, intermediates, unix(ran_out))
                    .map(|_| ())
            }
            Err(other) => Err(other),
        }
    }
}

impl ClientCertVerifier for OrchClientVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, TlsError> {
        self.check(end_entity, intermediates, now)
            .map(|()| ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Judges the certificate of a server against the system's authority, or
/// against a pin, and, where it is told which service it must be, that it
/// names that service.
#[derive(Debug)]
pub struct OrchServerVerifier {
    trust: ServerTrust,
    provider: Arc<CryptoProvider>,
    expected: Option<ServiceName>,
}

impl OrchServerVerifier {
    /// A verifier for servers that `trust` vouches for. With `expected`, the
    /// server must also be that service of the system.
    #[must_use]
    pub fn new(
        trust: ServerTrust,
        expected: Option<ServiceName>,
        provider: &Arc<CryptoProvider>,
    ) -> Arc<Self> {
        Arc::new(Self {
            trust,
            provider: Arc::clone(provider),
            expected,
        })
    }

    fn authority_for(&self, intermediates: &[CertificateDer<'_>]) -> Result<Certificate, TlsError> {
        match &self.trust {
            ServerTrust::Authority(authority) => Ok((**authority).clone()),
            ServerTrust::Pin(pin) => intermediates
                .iter()
                .filter_map(|der| Certificate::from_der(der).ok())
                .find(|candidate| {
                    candidate.is_ca() && bool::from(candidate.spki_sha256().ct_eq(pin))
                })
                .ok_or(TlsError::InvalidCertificate(
                    CertificateError::UnknownIssuer,
                )),
        }
    }
}

impl ServerCertVerifier for OrchServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let named = Certificate::from_der(end_entity).map_err(|_| bad_encoding())?;
        let authority = self.authority_for(intermediates)?;
        let roots = roots_of(&authority)?;
        if let Some(expected) = self.expected {
            // The system's own signed service identity binds the peer. Advertised hostnames and
            // addresses may change without changing that identity or the pinned authority.
            let parsed = rustls::server::ParsedCertificate::try_from(end_entity)?;
            rustls::client::verify_server_cert_signed_by_trust_anchor(
                &parsed,
                &roots,
                intermediates,
                now,
                self.provider.signature_verification_algorithms.all,
            )?;
            if !named.identities().contains(&Identity::Service(expected)) {
                return Err(app_failure());
            }
            Ok(ServerCertVerified::assertion())
        } else {
            let inner =
                WebPkiServerVerifier::builder_with_provider(roots, Arc::clone(&self.provider))
                    .build()
                    .map_err(|error| TlsError::General(error.to_string()))?;
            inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The configuration of the gateway's **pinned** listener: it shows
/// `identity`, and wants a certificate of the system from every client,
/// tolerating one that ran out less than `grace_days` ago.
pub fn pinned_server_config(
    identity: &TlsIdentity,
    authority: &Certificate,
    grace_days: u64,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ServerConfig>, PkiError> {
    let provider = crypto();
    let verifier = OrchClientVerifier::new(
        authority,
        Duration::from_secs(grace_days.saturating_mul(DAY_SECS)),
        &provider,
    )?;
    let config =
        ServerConfig::builder_with_details(Arc::clone(&provider), ClockTime::provider(clock))
            .with_safe_default_protocol_versions()
            .map_err(|e| PkiError::Certificate(e.to_string()))?
            .with_client_cert_verifier(verifier)
            .with_single_cert(identity.chain_der(), identity.key.to_rustls())
            .map_err(|e| PkiError::Certificate(e.to_string()))?;
    Ok(Arc::new(config))
}

/// The configuration of a listener that asks for no certificate: the gateway
/// behind the public edge, or an enrollment endpoint, where the client has
/// none yet.
pub fn plain_server_config(
    identity: &TlsIdentity,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ServerConfig>, PkiError> {
    let config = ServerConfig::builder_with_details(crypto(), ClockTime::provider(clock))
        .with_safe_default_protocol_versions()
        .map_err(|e| PkiError::Certificate(e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(identity.chain_der(), identity.key.to_rustls())
        .map_err(|e| PkiError::Certificate(e.to_string()))?;
    Ok(Arc::new(config))
}

/// The configuration of a client on the **pinned** path: it believes only
/// servers `trust` vouches for, and shows `identity` where it has one (it
/// has none at enrollment). With `expected`, the server has to be that
/// service.
pub fn pinned_client_config(
    trust: ServerTrust,
    expected: Option<ServiceName>,
    identity: Option<&TlsIdentity>,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ClientConfig>, PkiError> {
    let provider = crypto();
    let verifier = OrchServerVerifier::new(trust, expected, &provider);
    let builder =
        ClientConfig::builder_with_details(Arc::clone(&provider), ClockTime::provider(clock))
            .with_safe_default_protocol_versions()
            .map_err(|e| PkiError::Certificate(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(verifier);
    let config = match identity {
        Some(identity) => builder
            .with_client_auth_cert(identity.chain_der(), identity.key.to_rustls())
            .map_err(|e| PkiError::Certificate(e.to_string()))?,
        None => builder.with_no_client_auth(),
    };
    Ok(Arc::new(config))
}

/// The configuration of the pinned listener as [`pinned_server_config`]
/// makes it, showing what `identity` holds at each handshake: a service that
/// renews its certificate shows the new one without being started again.
pub fn pinned_server_config_live(
    identity: Arc<crate::service_renew::LiveIdentity>,
    authority: &Certificate,
    grace_days: u64,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ServerConfig>, PkiError> {
    let provider = crypto();
    let verifier = OrchClientVerifier::new(
        authority,
        Duration::from_secs(grace_days.saturating_mul(DAY_SECS)),
        &provider,
    )?;
    let config =
        ServerConfig::builder_with_details(Arc::clone(&provider), ClockTime::provider(clock))
            .with_safe_default_protocol_versions()
            .map_err(|e| PkiError::Certificate(e.to_string()))?
            .with_client_cert_verifier(verifier)
            .with_cert_resolver(identity);
    Ok(Arc::new(config))
}

/// The configuration of a listener that asks for no certificate as
/// [`plain_server_config`] makes it, showing what `identity` holds at each
/// handshake.
pub fn plain_server_config_live(
    identity: Arc<crate::service_renew::LiveIdentity>,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ServerConfig>, PkiError> {
    let config = ServerConfig::builder_with_details(crypto(), ClockTime::provider(clock))
        .with_safe_default_protocol_versions()
        .map_err(|e| PkiError::Certificate(e.to_string()))?
        .with_no_client_auth()
        .with_cert_resolver(identity);
    Ok(Arc::new(config))
}

/// The configuration of a client on the pinned path as
/// [`pinned_client_config`] makes it, showing what `identity` holds at each
/// handshake, so that a renewed certificate is shown from the next connection.
pub fn pinned_client_config_live(
    trust: ServerTrust,
    expected: Option<ServiceName>,
    identity: Arc<crate::service_renew::LiveIdentity>,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ClientConfig>, PkiError> {
    let provider = crypto();
    let verifier = OrchServerVerifier::new(trust, expected, &provider);
    let config =
        ClientConfig::builder_with_details(Arc::clone(&provider), ClockTime::provider(clock))
            .with_safe_default_protocol_versions()
            .map_err(|e| PkiError::Certificate(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_cert_resolver(identity);
    Ok(Arc::new(config))
}

/// The configuration of a client on the **public** path: it believes the
/// servers that `roots` vouch for, as a browser does, and shows no
/// certificate.
pub fn public_client_config(
    roots: Arc<RootCertStore>,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ClientConfig>, PkiError> {
    device_client_config(roots, None, None, clock)
}

/// A machine trusts the named Orch service under its own authority, or device roots for the name dialled.
pub fn machine_client_config(
    trust: Option<ServerTrust>,
    expected: ServiceName,
    identity: Option<&TlsIdentity>,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ClientConfig>, PkiError> {
    device_client_config(
        trusted_roots(),
        trust.map(|trust| (trust, expected)),
        identity,
        clock,
    )
}

fn device_client_config(
    roots: Arc<RootCertStore>,
    own: Option<(ServerTrust, ServiceName)>,
    identity: Option<&TlsIdentity>,
    clock: Arc<dyn Clock>,
) -> Result<Arc<ClientConfig>, PkiError> {
    let provider = crypto();
    let inner = WebPkiServerVerifier::builder_with_provider(roots, Arc::clone(&provider))
        .build()
        .map_err(|error| PkiError::Certificate(error.to_string()))?;
    let own =
        own.map(|(trust, expected)| OrchServerVerifier::new(trust, Some(expected), &provider));
    let builder = ClientConfig::builder_with_details(provider, ClockTime::provider(clock))
        .with_safe_default_protocol_versions()
        .map_err(|error| PkiError::Certificate(error.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PublicServerVerifier { inner, own }));
    let config = match identity {
        Some(identity) => builder
            .with_client_auth_cert(identity.chain_der(), identity.key.to_rustls())
            .map_err(|error| PkiError::Certificate(error.to_string()))?,
        None => builder.with_no_client_auth(),
    };
    Ok(Arc::new(config))
}

// Either trust decision may accept the server; an unknown device issuer names the repair.
#[derive(Debug)]
struct PublicServerVerifier {
    inner: Arc<WebPkiServerVerifier>,
    own: Option<Arc<OrchServerVerifier>>,
}

impl ServerCertVerifier for PublicServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        if let Some(own) = &self.own
            && let Ok(verified) =
                own.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        {
            return Ok(verified);
        }
        self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            .map_err(|error| {
                if !matches!(error, TlsError::InvalidCertificate(CertificateError::UnknownIssuer)) { return error; }
                let issuer = x509_parser::parse_x509_certificate(end_entity.as_ref())
                    .map(|(_, certificate)| certificate.issuer().to_string())
                    .unwrap_or_else(|_| "unreadable issuer".to_owned())
                    .chars().map(|character| if character.is_control() { ' ' } else { character }).collect::<String>();
                TlsError::General(format!("UnknownIssuer: {issuer}; install its authority in the machine's certificate store. For an Orch connection, the pinned gateway endpoint needs no public or company certificate"))
            })
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.inner
            .verify_tls12_signature(message, certificate, signature)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        self.inner
            .verify_tls13_signature(message, certificate, signature)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// A root store holding `authorities`, for the public path.
pub fn root_store(authorities: &[Certificate]) -> Result<Arc<RootCertStore>, PkiError> {
    let mut roots = RootCertStore::empty();
    for authority in authorities {
        roots
            .add(authority.to_rustls())
            .map_err(|e| PkiError::Certificate(e.to_string()))?;
    }
    Ok(Arc::new(roots))
}

/// The certificate a client showed in a TLS connection, read, or `None`
/// when it showed none or showed something unreadable. The connection's own
/// verifier has judged the chain by now; this reads what it concluded.
#[must_use]
pub fn presented_certificate(chain: Option<&[CertificateDer<'_>]>) -> Option<Certificate> {
    chain
        .and_then(<[CertificateDer<'_>]>::first)
        .and_then(|der| Certificate::from_der(der).ok())
}

/// Why a machine's proof of its certificate does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProofFault {
    /// The certificate or the signature cannot be read.
    #[error("the proof cannot be read")]
    Malformed,
    /// The certificate is not one the system issued, or is not good for a
    /// client.
    #[error("the certificate is not one of this system's")]
    Untrusted,
    /// The certificate ran out.
    #[error("the certificate has expired")]
    Expired,
    /// The certificate is not good yet.
    #[error("the certificate is not good yet")]
    NotYetValid,
    /// The signature is not by the certificate's key over what was asked.
    #[error("the signature does not hold")]
    BadSignature,
}

/// Checks a machine's proof that it holds the key of its certificate, as
/// a machine sends it inside a connection that carried no certificate: the
/// certificate has to be one the system issued and still good at `now` (a
/// session is never given to an expired one), and `signature`, the 64 bytes
/// `r ‖ s`, has to be its key's over `message`.
///
/// Returns the certificate, for whoever has to compare it with what the
/// machine claims to be.
pub fn verify_machine_proof(
    authority: &Certificate,
    certificate_pem: &str,
    signature: &[u8],
    message: &[u8],
    now: Time,
) -> Result<Certificate, ProofFault> {
    verify_proof_with_grace(
        authority,
        certificate_pem,
        signature,
        message,
        now,
        Duration::ZERO,
    )
}

/// As [`verify_machine_proof`], accepting a certificate that ran out less
/// than `grace` ago: for the one request that may be made with such a
/// certificate, the request to renew it.
pub fn verify_proof_with_grace(
    authority: &Certificate,
    certificate_pem: &str,
    signature: &[u8],
    message: &[u8],
    now: Time,
    grace: Duration,
) -> Result<Certificate, ProofFault> {
    let certificate = Certificate::from_pem(certificate_pem).map_err(|_| ProofFault::Malformed)?;
    let provider = crypto();
    let verifier =
        OrchClientVerifier::new(authority, grace, &provider).map_err(|_| ProofFault::Untrusted)?;
    verifier
        .check(&certificate.to_rustls(), &[], unix(now))
        .map_err(|error| match error {
            TlsError::InvalidCertificate(
                CertificateError::Expired | CertificateError::ExpiredContext { .. },
            ) => ProofFault::Expired,
            TlsError::InvalidCertificate(
                CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. },
            ) => ProofFault::NotYetValid,
            _ => ProofFault::Untrusted,
        })?;
    if !verify_signature(certificate.public_point(), message, signature) {
        return Err(ProofFault::BadSignature);
    }
    Ok(certificate)
}
