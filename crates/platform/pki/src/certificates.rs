//! Local keys, CSRs and certificate issuance.
//!
//! A key is made where it is used and never leaves: a machine makes its own,
//! shows only a request for a certificate over it, and keeps the private
//! half in a file that nobody else reads. What comes back is a certificate,
//! which this module reads into the few facts that every decision about it
//! needs: whom it names, how long it is good for, which key it holds, and
//! what it is worth for.
//!
//! Every key here is ECDSA over P-256, and every signature is ECDSA with
//! SHA-256 written as the 64 bytes `r ‖ s`.

use crate::PkiError;
use chrono::DateTime;
use orch_link_contract::enrollment::{MACHINE_URI_PREFIX, SERVICE_URI_PREFIX};
use orch_types::{Id, Time};
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::pkcs8::DecodePrivateKey;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PublicKeyData};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use std::fmt;
use std::path::Path;
use std::str::FromStr;
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::prelude::{FromDer, X509Certificate};

/// The bytes of an uncompressed P-256 public key: a marker, then both
/// coordinates.
pub const P256_POINT_BYTES: usize = 65;
/// The marker that begins an uncompressed point.
const UNCOMPRESSED_MARKER: u8 = 0x04;
/// The bytes of a signature: both numbers, 32 bytes each.
pub const SIGNATURE_BYTES: usize = 64;
/// The subject every request carries. The system ignores it: it names the
/// holder itself, in the alternative names it puts in the certificate.
const REQUEST_SUBJECT: &str = "orch";

/// The services of the system that hold a certificate of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ServiceName {
    /// The API: it is the authority, and the other end of the backplane.
    Api,
    /// The gateway that machines connect to.
    Gateway,
    /// The worker.
    Worker,
    /// The web edge's HTTPS listener; its key never authenticates as an internal client.
    Frontend,
}

impl ServiceName {
    /// The name as it is written in a certificate.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Gateway => "gateway",
            Self::Worker => "worker",
            Self::Frontend => "frontend",
        }
    }
}

impl fmt::Display for ServiceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ServiceName {
    type Err = PkiError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "api" => Ok(Self::Api),
            "gateway" => Ok(Self::Gateway),
            "worker" => Ok(Self::Worker),
            "frontend" => Ok(Self::Frontend),
            other => Err(PkiError::Certificate(format!(
                "`{other}` is not a service of the system"
            ))),
        }
    }
}

/// Whom a certificate names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Identity {
    /// A machine, by its identifier.
    Machine(Id),
    /// A service of the system.
    Service(ServiceName),
}

impl Identity {
    /// The name as it stands in the certificate: a URI.
    #[must_use]
    pub fn uri(&self) -> String {
        match self {
            Self::Machine(id) => format!("{MACHINE_URI_PREFIX}{id}"),
            Self::Service(service) => format!("{SERVICE_URI_PREFIX}{service}"),
        }
    }

    /// The identity a URI names, when it names one.
    #[must_use]
    pub fn from_uri(uri: &str) -> Option<Self> {
        if let Some(id) = uri.strip_prefix(MACHINE_URI_PREFIX) {
            // Written the one way identifiers are written, so that a
            // machine has one name.
            let parsed: Id = id.parse().ok()?;
            return (parsed.to_string() == id).then_some(Self::Machine(parsed));
        }
        uri.strip_prefix(SERVICE_URI_PREFIX)
            .and_then(|name| name.parse().ok())
            .map(Self::Service)
    }
}

/// Sets the validity of `params` to the moments given. `rcgen` takes its own
/// date type and offers no way to build one at a given second except to
/// start from a date and add to it, so that is what is done.
pub(crate) fn set_validity(params: &mut CertificateParams, not_before: Time, not_after: Time) {
    let epoch = rcgen::date_time_ymd(1970, 1, 1);
    let at = |moment: Time| {
        let seconds = moment.unix_millis().div_euclid(MILLIS_PER_SECOND);
        epoch + std::time::Duration::from_secs(u64::try_from(seconds).unwrap_or(0))
    };
    params.not_before = at(not_before);
    params.not_after = at(not_after);
}

/// Milliseconds in a second, for turning a moment into whole seconds.
const MILLIS_PER_SECOND: i64 = 1_000;

/// An ECDSA P-256 key made for a certificate, with its private half.
pub struct LeafKey {
    pair: KeyPair,
    signing: SigningKey,
}

impl fmt::Debug for LeafKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LeafKey(..)")
    }
}

impl LeafKey {
    /// Makes a key.
    pub fn generate() -> Result<Self, PkiError> {
        let pair = KeyPair::generate().map_err(|e| PkiError::Key(e.to_string()))?;
        Self::from_pair(pair)
    }

    /// Reads a key from its PKCS #8 PEM text.
    pub fn from_pem(pem: &str) -> Result<Self, PkiError> {
        let pair = KeyPair::from_pem(pem).map_err(|e| PkiError::Key(e.to_string()))?;
        Self::from_pair(pair)
    }

    fn from_pair(pair: KeyPair) -> Result<Self, PkiError> {
        let signing = SigningKey::from_pkcs8_der(&pair.serialize_der())
            .map_err(|e| PkiError::Key(format!("not an ECDSA P-256 key: {e}")))?;
        Ok(Self { pair, signing })
    }

    /// A second handle on the same key.
    pub fn duplicate(&self) -> Result<Self, PkiError> {
        Self::from_pem(&self.to_pem())
    }

    /// The key as PKCS #8 PEM text. This is the secret: it belongs in a file
    /// that only its owner reads.
    #[must_use]
    pub fn to_pem(&self) -> String {
        self.pair.serialize_pem()
    }

    /// The key as the TLS library takes it.
    #[must_use]
    pub fn to_rustls(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.pair.serialize_der()))
    }

    /// The public half as an uncompressed point.
    #[must_use]
    pub fn public_point(&self) -> [u8; P256_POINT_BYTES] {
        let mut point = [0u8; P256_POINT_BYTES];
        let encoded = self.signing.verifying_key().to_sec1_point(false);
        point.copy_from_slice(encoded.as_bytes());
        point
    }

    /// The private half as 32 bytes, for the one use that needs the number
    /// itself: opening what was sealed to the key.
    pub(crate) fn secret_scalar(&self) -> [u8; 32] {
        let mut scalar = [0u8; 32];
        scalar.copy_from_slice(&self.signing.to_bytes());
        scalar
    }

    pub(crate) fn rcgen_pair(&self) -> &KeyPair {
        &self.pair
    }

    /// The request for a certificate over this key, as PEM text. Its
    /// subject says nothing: the system names the holder itself.
    pub fn request_pem(&self) -> Result<String, PkiError> {
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, REQUEST_SUBJECT);
        params
            .serialize_request(&self.pair)
            .and_then(|request| request.pem())
            .map_err(|e| PkiError::Request(e.to_string()))
    }

    /// Signs `message`: ECDSA with SHA-256, as the 64 bytes `r ‖ s`.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> [u8; SIGNATURE_BYTES] {
        let signature: Signature = self.signing.sign(message);
        let mut bytes = [0u8; SIGNATURE_BYTES];
        bytes.copy_from_slice(&signature.to_bytes());
        bytes
    }
}

/// Whether `signature` (the 64 bytes `r ‖ s`) is `public_point`'s ECDSA with
/// SHA-256 over `message`.
#[must_use]
pub fn verify_signature(public_point: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Ok(key) = VerifyingKey::from_sec1_bytes(public_point) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    key.verify(message, &signature).is_ok()
}

/// What an extension of a certificate says that matters here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Facts {
    identities: Vec<Identity>,
    dns_names: Vec<String>,
    client_auth: bool,
    server_auth: bool,
    ca: bool,
    path_len: Option<u32>,
    key_cert_sign: bool,
    crl_sign: bool,
}

/// A certificate, read once into the facts that decisions need.
#[derive(Clone, PartialEq, Eq)]
pub struct Certificate {
    der: Vec<u8>,
    not_before: Time,
    not_after: Time,
    subject_cn: Option<String>,
    serial: Vec<u8>,
    point: [u8; P256_POINT_BYTES],
    spki_sha256: [u8; 32],
    facts: Facts,
}

impl fmt::Debug for Certificate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Certificate")
            .field("identities", &self.facts.identities)
            .field("not_before", &self.not_before)
            .field("not_after", &self.not_after)
            .finish_non_exhaustive()
    }
}

fn moment(seconds: i64) -> Result<Time, PkiError> {
    DateTime::from_timestamp(seconds, 0)
        .map(Time::new)
        .ok_or_else(|| PkiError::Certificate("a date out of range".to_owned()))
}

impl Certificate {
    /// Reads a certificate from its DER bytes.
    pub fn from_der(der: &[u8]) -> Result<Self, PkiError> {
        let bad = |what: &str| PkiError::Certificate(what.to_owned());
        let (rest, parsed) = X509Certificate::from_der(der)
            .map_err(|e| PkiError::Certificate(format!("not a certificate: {e}")))?;
        if !rest.is_empty() {
            return Err(bad("bytes after the certificate"));
        }
        let key = &parsed.tbs_certificate.subject_pki;
        let point: [u8; P256_POINT_BYTES] = key
            .subject_public_key
            .data
            .as_ref()
            .try_into()
            .map_err(|_| bad("the key is not an ECDSA P-256 key"))?;
        if point[0] != UNCOMPRESSED_MARKER {
            return Err(bad("the key is not an uncompressed point"));
        }
        let mut facts = Facts::default();
        for extension in parsed.extensions() {
            match extension.parsed_extension() {
                ParsedExtension::SubjectAlternativeName(names) => {
                    for name in &names.general_names {
                        match name {
                            GeneralName::URI(uri) => {
                                facts.identities.extend(Identity::from_uri(uri));
                            }
                            GeneralName::DNSName(dns) => facts.dns_names.push((*dns).to_owned()),
                            _ => {}
                        }
                    }
                }
                ParsedExtension::ExtendedKeyUsage(usage) => {
                    facts.client_auth = usage.client_auth || usage.any;
                    facts.server_auth = usage.server_auth || usage.any;
                }
                ParsedExtension::BasicConstraints(constraints) => {
                    facts.ca = constraints.ca;
                    facts.path_len = constraints.path_len_constraint;
                }
                ParsedExtension::KeyUsage(usage) => {
                    facts.key_cert_sign = usage.key_cert_sign();
                    facts.crl_sign = usage.crl_sign();
                }
                _ => {}
            }
        }
        let subject_cn = parsed
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .map(str::to_owned);
        Ok(Self {
            der: der.to_vec(),
            not_before: moment(parsed.validity().not_before.timestamp())?,
            not_after: moment(parsed.validity().not_after.timestamp())?,
            subject_cn,
            serial: parsed.raw_serial().to_vec(),
            point,
            spki_sha256: Sha256::digest(key.raw).into(),
            facts,
        })
    }

    /// Reads a certificate from PEM text that holds exactly one.
    pub fn from_pem(pem: &str) -> Result<Self, PkiError> {
        let mut certificates = Self::all_from_pem(pem)?;
        match (certificates.pop(), certificates.is_empty()) {
            (Some(one), true) => Ok(one),
            _ => Err(PkiError::Certificate(
                "expected exactly one certificate".to_owned(),
            )),
        }
    }

    /// Reads every certificate in PEM text, in order.
    pub fn all_from_pem(pem: &str) -> Result<Vec<Self>, PkiError> {
        let mut found = Vec::new();
        for block in x509_parser::pem::Pem::iter_from_buffer(pem.as_bytes()) {
            let block = block.map_err(|e| PkiError::Certificate(format!("bad PEM: {e}")))?;
            if block.label == "CERTIFICATE" {
                found.push(Self::from_der(&block.contents)?);
            }
        }
        if found.is_empty() {
            return Err(PkiError::Certificate(
                "no certificate in the text".to_owned(),
            ));
        }
        Ok(found)
    }

    /// The certificate as DER bytes.
    #[must_use]
    pub fn der(&self) -> &[u8] {
        &self.der
    }

    /// The certificate as the TLS library takes it.
    #[must_use]
    pub fn to_rustls(&self) -> CertificateDer<'static> {
        CertificateDer::from(self.der.clone())
    }

    /// The certificate as PEM text.
    #[must_use]
    pub fn pem(&self) -> String {
        pem_encode("CERTIFICATE", &self.der)
    }

    /// The SHA-256 of the certificate, in lowercase hexadecimal: what the
    /// system knows a machine's certificate by.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        hex::encode(Sha256::digest(&self.der))
    }

    /// The SHA-256 of the certificate's public key as it is encoded in the
    /// certificate: the pin of an authority.
    #[must_use]
    pub const fn spki_sha256(&self) -> [u8; 32] {
        self.spki_sha256
    }

    /// The public key as an uncompressed point.
    #[must_use]
    pub const fn public_point(&self) -> &[u8; P256_POINT_BYTES] {
        &self.point
    }

    /// The first moment the certificate is good.
    #[must_use]
    pub const fn not_before(&self) -> Time {
        self.not_before
    }

    /// The last moment the certificate is good.
    #[must_use]
    pub const fn not_after(&self) -> Time {
        self.not_after
    }

    /// The serial number.
    #[must_use]
    pub fn serial(&self) -> &[u8] {
        &self.serial
    }

    /// The common name of the subject, where there is one.
    #[must_use]
    pub fn subject_common_name(&self) -> Option<&str> {
        self.subject_cn.as_deref()
    }

    /// Whom the certificate names.
    #[must_use]
    pub fn identities(&self) -> &[Identity] {
        &self.facts.identities
    }

    /// The machine the certificate names, when it names exactly one
    /// identity and that is a machine.
    #[must_use]
    pub fn machine_id(&self) -> Option<Id> {
        match self.facts.identities.as_slice() {
            [Identity::Machine(id)] => Some(*id),
            _ => None,
        }
    }

    /// The service the certificate names, when it names exactly one
    /// identity and that is a service.
    #[must_use]
    pub fn service(&self) -> Option<ServiceName> {
        match self.facts.identities.as_slice() {
            [Identity::Service(service)] => Some(*service),
            _ => None,
        }
    }

    /// The DNS names the certificate is good for.
    #[must_use]
    pub fn dns_names(&self) -> &[String] {
        &self.facts.dns_names
    }

    /// Whether the certificate may be used to prove a client.
    #[must_use]
    pub const fn allows_client_auth(&self) -> bool {
        self.facts.client_auth
    }

    /// Whether the certificate may be used to prove a server.
    #[must_use]
    pub const fn allows_server_auth(&self) -> bool {
        self.facts.server_auth
    }

    /// Whether the certificate is one of an authority.
    #[must_use]
    pub const fn is_ca(&self) -> bool {
        self.facts.ca
    }

    /// How many authorities may stand between this one and a certificate it
    /// issues, where it is an authority and says.
    #[must_use]
    pub const fn path_len(&self) -> Option<u32> {
        self.facts.path_len
    }

    /// Whether the certificate may sign certificates and lists of
    /// revocations, as an authority's key does.
    #[must_use]
    pub const fn signs_certificates(&self) -> bool {
        self.facts.key_cert_sign && self.facts.crl_sign
    }

    /// Whether the moment `now` lies within the certificate's validity.
    #[must_use]
    pub fn valid_at(&self, now: Time) -> bool {
        self.not_before <= now && now <= self.not_after
    }

    /// Whether the certificate holds the key that `key` is.
    #[must_use]
    pub fn holds(&self, key: &LeafKey) -> bool {
        self.point == key.public_point()
    }
}

/// A request for a certificate, read and checked: its own signature holds,
/// which proves that whoever sent it holds the private half of its key, and
/// the key is ECDSA over P-256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateRequest {
    pem: String,
    point: [u8; P256_POINT_BYTES],
    public_key: rcgen::PublicKey,
}

impl CertificateRequest {
    /// Reads a request from PEM text.
    pub fn from_pem(pem: &str) -> Result<Self, PkiError> {
        let parsed = rcgen::CertificateSigningRequestParams::from_pem(pem)
            .map_err(|e| PkiError::Request(e.to_string()))?;
        let point: [u8; P256_POINT_BYTES] = parsed
            .public_key
            .der_bytes()
            .try_into()
            .map_err(|_| PkiError::Request("the key is not an ECDSA P-256 key".to_owned()))?;
        if point[0] != UNCOMPRESSED_MARKER
            || parsed.public_key.algorithm() != &rcgen::PKCS_ECDSA_P256_SHA256
        {
            return Err(PkiError::Request(
                "the key is not an ECDSA P-256 key".to_owned(),
            ));
        }
        Ok(Self {
            pem: pem.to_owned(),
            point,
            public_key: parsed.public_key,
        })
    }

    /// The request as it was read.
    #[must_use]
    pub fn pem(&self) -> &str {
        &self.pem
    }

    /// The key the request asks a certificate for, as an uncompressed point.
    #[must_use]
    pub const fn public_point(&self) -> &[u8; P256_POINT_BYTES] {
        &self.point
    }

    pub(crate) const fn public_key(&self) -> &rcgen::PublicKey {
        &self.public_key
    }
}

/// `der` as PEM text under `label`.
#[must_use]
pub fn pem_encode(label: &str, der: &[u8]) -> String {
    use base64::Engine;
    const LINE: usize = 64;
    let body = base64::engine::general_purpose::STANDARD.encode(der);
    let mut text = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(LINE) {
        text.push_str(&String::from_utf8_lossy(line));
        text.push('\n');
    }
    text.push_str(&format!("-----END {label}-----\n"));
    text
}

/// Writes `text` to `path` so that a reader sees the whole old file or the
/// whole new one, and so that nobody but the owner can read it: the mode is
/// `0600` where the system has modes, and an access list for the user alone
/// where it has not.
pub fn write_private_file(path: &Path, text: &str) -> Result<(), PkiError> {
    write_atomically(path, text, Access::OwnerOnly)
}

/// Writes `text` to `path` so that a reader sees the whole old file or the
/// whole new one. The file is readable by all.
pub fn write_file(path: &Path, text: &str) -> Result<(), PkiError> {
    write_atomically(path, text, Access::Everyone)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    OwnerOnly,
    Everyone,
}

fn write_atomically(path: &Path, text: &str, access: Access) -> Result<(), PkiError> {
    use std::io::Write;
    let file_error = |source: std::io::Error| PkiError::File {
        path: path.display().to_string(),
        source,
    };
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(directory).map_err(file_error)?;
    // A temporary file is made readable by its owner alone, so that a secret
    // is never visible to others even for the moment before it is renamed.
    let mut staged = tempfile::NamedTempFile::new_in(directory).map_err(file_error)?;
    staged.write_all(text.as_bytes()).map_err(file_error)?;
    staged.as_file().sync_all().map_err(file_error)?;
    #[cfg(unix)]
    if access == Access::Everyone {
        use std::os::unix::fs::PermissionsExt;
        /// Owner reads and writes; everybody else reads.
        const WORLD_READABLE: u32 = 0o644;
        staged
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(WORLD_READABLE))
            .map_err(file_error)?;
    }
    staged.persist(path).map_err(|e| file_error(e.error))?;
    #[cfg(not(unix))]
    if access == Access::OwnerOnly {
        restrict_to_user(path).map_err(file_error)?;
    }
    Ok(())
}

/// Takes every access but the user's own from a file, on a system whose
/// files have access lists and no modes.
#[cfg(not(unix))]
fn restrict_to_user(path: &Path) -> std::io::Result<()> {
    let user = std::env::var("USERNAME").map_err(std::io::Error::other)?;
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("{user}:F"))
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("icacls refused to restrict the file"))
    }
}
