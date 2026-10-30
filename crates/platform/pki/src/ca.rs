//! CA initialization and custody.
//!
//! The authority is a P-256 key and a self-signed certificate that may sign
//! certificates and nothing below them: an authority that stood between it
//! and a leaf would be one the system never made. Its key is the one secret
//! whose loss lets somebody make a machine out of nothing, so it is made
//! once, written to a file that only its owner reads, and loaded by the one
//! service that issues certificates.
//!
//! Issuing is the other half. A request is a key that somebody holds; what
//! goes into the certificate is not the request's but the system's: the
//! name is made from the machine's identifier, the usages from the kind of
//! holder, the validity from the clock it is given.

use crate::PkiError;
use crate::certificates::{
    Certificate, CertificateRequest, Identity, LeafKey, ServiceName, set_validity,
};
use orch_link_contract::session::CLOCK_SKEW_SECS;
use orch_types::{Clock, Id, Time};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PublicKeyData, SanType, SerialNumber,
};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// The file the authority's private key is kept in.
pub const CA_KEY_FILE: &str = "orch_ca_key.pem";
/// The file the authority's certificate is kept in.
pub const CA_CERT_FILE: &str = "orch_ca_cert.pem";
/// What an authority's name begins with, before the name of the system.
pub const CA_NAME_PREFIX: &str = "Orch CA ";
/// How long a new authority is good for, in days, as a system ships: ten
/// years, long enough that nobody has to think about it and short enough
/// that nobody forgets it.
pub const CA_VALIDITY_DAYS: u64 = 3_650;
/// How many authorities may stand between this one and a leaf: none.
pub const CA_PATH_LEN: u8 = 0;
/// The bytes of a serial number: 128 bits, drawn at random.
pub const SERIAL_BYTES: usize = 16;
/// How far before the moment of issue a certificate begins, as long as a
/// proof may be out of step with the gateway's clock, so that a machine
/// whose clock runs a little late is not told its fresh certificate is not
/// yet good.
pub const BACKDATE_SECS: i64 = CLOCK_SKEW_SECS;
/// Milliseconds in a day.
const DAY_MILLIS: i64 = 86_400_000;
/// Milliseconds in a second.
const SECOND_MILLIS: i64 = 1_000;

/// What a certificate is for, which decides what it names and allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder {
    /// A machine: it proves itself to a gateway.
    Machine(Id),
    /// A service of the system: it is proved to others, and proves itself
    /// to the other services. `names` are the DNS names and addresses it is
    /// reached by.
    Service {
        /// Which service.
        service: ServiceName,
        /// The names it answers to.
        names: Vec<String>,
    },
}

impl Holder {
    fn identity(&self) -> Identity {
        match self {
            Self::Machine(id) => Identity::Machine(*id),
            Self::Service { service, .. } => Identity::Service(*service),
        }
    }
}

/// The authority: its certificate and its key.
pub struct CertificateAuthority {
    certificate: Certificate,
    key: LeafKey,
    issuer: Issuer<'static, KeyPair>,
}

impl std::fmt::Debug for CertificateAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateAuthority")
            .field("pin", &hex::encode(self.pin()))
            .finish_non_exhaustive()
    }
}

/// A random serial number of 128 bits.
fn random_serial() -> SerialNumber {
    let mut bytes = [0u8; SERIAL_BYTES];
    rand::fill(&mut bytes[..]);
    // DER removes redundant leading zeroes; reject those draws to keep the full serial width.
    while bytes[0] == 0 {
        rand::fill(&mut bytes[..]);
    }
    SerialNumber::from_slice(&bytes)
}

fn from_ip_or_dns(name: &str) -> Result<SanType, PkiError> {
    if let Ok(address) = name.parse::<IpAddr>() {
        return Ok(SanType::IpAddress(address));
    }
    let dns = name
        .to_owned()
        .try_into()
        .map_err(|_| PkiError::Issue(format!("`{name}` is not a DNS name")))?;
    Ok(SanType::DnsName(dns))
}

fn uri_name(uri: String) -> Result<SanType, PkiError> {
    let uri = uri
        .clone()
        .try_into()
        .map_err(|_| PkiError::Issue(format!("`{uri}` is not a URI name")))?;
    Ok(SanType::URI(uri))
}

fn days_to_millis(days: u64) -> i64 {
    i64::try_from(days)
        .unwrap_or(i64::MAX / DAY_MILLIS)
        .saturating_mul(DAY_MILLIS)
}

fn shift(at: Time, millis: i64) -> Time {
    Time::new(at.get() + chrono::Duration::milliseconds(millis))
}

impl CertificateAuthority {
    /// Makes a new authority for the system called `system_name`, good for
    /// `validity_days` from now.
    pub fn create(
        system_name: &str,
        validity_days: u64,
        clock: &dyn Clock,
    ) -> Result<Self, PkiError> {
        let key = LeafKey::generate()?;
        let now = clock.time();
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, format!("{CA_NAME_PREFIX}{system_name}"));
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(CA_PATH_LEN));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.serial_number = Some(random_serial());
        set_validity(
            &mut params,
            shift(now, -BACKDATE_SECS * SECOND_MILLIS),
            shift(now, days_to_millis(validity_days)),
        );
        let made = params.self_signed(key.rcgen_pair())?;
        Self::assemble(Certificate::from_der(made.der())?, key)
    }

    /// Reads an authority from its certificate and its key, both PEM text.
    /// The key has to be the certificate's, and the certificate has to be
    /// that of an authority that signs leaves and nothing else.
    pub fn from_pem(certificate_pem: &str, key_pem: &str) -> Result<Self, PkiError> {
        Self::assemble(
            Certificate::from_pem(certificate_pem)?,
            LeafKey::from_pem(key_pem)?,
        )
    }

    fn assemble(certificate: Certificate, key: LeafKey) -> Result<Self, PkiError> {
        let bad = |what: &str| PkiError::Certificate(format!("the authority: {what}"));
        if !certificate.holds(&key) {
            return Err(bad("the key is not the certificate's"));
        }
        if !certificate.is_ca() || !certificate.signs_certificates() {
            return Err(bad("the certificate is no authority's"));
        }
        if certificate.path_len() != Some(u32::from(CA_PATH_LEN)) {
            return Err(bad("the certificate lets another authority stand below it"));
        }
        let pair = KeyPair::from_pem(&key.to_pem())?;
        let issuer = Issuer::from_ca_cert_der(&certificate.to_rustls(), pair)?;
        Ok(Self {
            certificate,
            key,
            issuer,
        })
    }

    /// The authority's certificate.
    #[must_use]
    pub const fn certificate(&self) -> &Certificate {
        &self.certificate
    }

    /// The authority's certificate as PEM text.
    #[must_use]
    pub fn certificate_pem(&self) -> String {
        self.certificate.pem()
    }

    /// The authority's private key as PEM text. It goes into a file that
    /// only its owner reads, and nowhere else.
    #[must_use]
    pub fn key_pem(&self) -> String {
        self.key.to_pem()
    }

    /// The pin of the authority: the SHA-256 of its public key.
    #[must_use]
    pub const fn pin(&self) -> [u8; 32] {
        self.certificate.spki_sha256()
    }

    /// Issues a certificate for the key of `request` to `holder`. What the
    /// request says about its holder is not read: the certificate names
    /// whom the system says it names.
    pub fn issue(
        &self,
        request: &CertificateRequest,
        holder: &Holder,
        validity_days: u64,
        clock: &dyn Clock,
    ) -> Result<Certificate, PkiError> {
        self.issue_for_key(request.public_key(), holder, validity_days, clock)
    }

    /// Makes a key and a certificate for a service, in one step: for a
    /// service that is its own authority's neighbour and has nobody to send
    /// a request to.
    pub fn issue_with_new_key(
        &self,
        holder: &Holder,
        validity_days: u64,
        clock: &dyn Clock,
    ) -> Result<(LeafKey, Certificate), PkiError> {
        let key = LeafKey::generate()?;
        let certificate = self.issue_for_key(key.rcgen_pair(), holder, validity_days, clock)?;
        Ok((key, certificate))
    }

    fn issue_for_key(
        &self,
        public_key: &impl PublicKeyData,
        holder: &Holder,
        validity_days: u64,
        clock: &dyn Clock,
    ) -> Result<Certificate, PkiError> {
        let now = clock.time();
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        let identity = holder.identity();
        params.subject_alt_names.push(uri_name(identity.uri())?);
        params
            .distinguished_name
            .push(DnType::CommonName, identity.uri());
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = match holder {
            Holder::Machine(_) => vec![ExtendedKeyUsagePurpose::ClientAuth],
            Holder::Service { service, names } => {
                for name in names {
                    params.subject_alt_names.push(from_ip_or_dns(name)?);
                }
                if *service == ServiceName::Frontend {
                    vec![ExtendedKeyUsagePurpose::ServerAuth]
                } else {
                    vec![
                        ExtendedKeyUsagePurpose::ServerAuth,
                        ExtendedKeyUsagePurpose::ClientAuth,
                    ]
                }
            }
        };
        params.serial_number = Some(random_serial());
        params.use_authority_key_identifier_extension = true;
        set_validity(
            &mut params,
            shift(now, -BACKDATE_SECS * SECOND_MILLIS),
            shift(now, days_to_millis(validity_days)),
        );
        let made = params.signed_by(public_key, &self.issuer)?;
        Certificate::from_der(made.der())
    }
}

/// Where an authority is kept: two files in a directory.
#[derive(Debug, Clone)]
pub struct CaFiles {
    key: PathBuf,
    certificate: PathBuf,
}

impl CaFiles {
    /// The files of the authority kept in `dir`.
    #[must_use]
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            key: dir.join(CA_KEY_FILE),
            certificate: dir.join(CA_CERT_FILE),
        }
    }

    /// The path of the key.
    #[must_use]
    pub fn key_path(&self) -> &Path {
        &self.key
    }

    /// The path of the certificate.
    #[must_use]
    pub fn certificate_path(&self) -> &Path {
        &self.certificate
    }

    /// Whether the authority has been made.
    #[must_use]
    pub fn exist(&self) -> bool {
        self.key.exists() && self.certificate.exists()
    }

    /// Loads the authority.
    pub fn load(&self) -> Result<CertificateAuthority, PkiError> {
        let read = |path: &Path| {
            std::fs::read_to_string(path).map_err(|source| PkiError::File {
                path: path.display().to_string(),
                source,
            })
        };
        CertificateAuthority::from_pem(&read(&self.certificate)?, &read(&self.key)?)
    }

    /// Makes the authority and writes it, unless it is there already: an
    /// authority is made once, and a second one would silently cut off every
    /// machine of the first. Returns whether it was made now.
    pub fn create_if_missing(
        &self,
        system_name: &str,
        validity_days: u64,
        clock: &dyn Clock,
    ) -> Result<(CertificateAuthority, bool), PkiError> {
        if self.exist() {
            return Ok((self.load()?, false));
        }
        let authority = CertificateAuthority::create(system_name, validity_days, clock)?;
        crate::certificates::write_private_file(&self.key, &authority.key_pem())?;
        // The certificate is public, but it is written the same way so that
        // a half-written file is never seen.
        crate::certificates::write_file(&self.certificate, &authority.certificate_pem())?;
        Ok((authority, true))
    }
}
