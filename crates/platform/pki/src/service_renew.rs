//! A service's own certificate, kept current while it runs.
//!
//! The API, the gateway and the worker start with the certificates the deployment made for them, and each renews
//! its own once two thirds of its life have passed ([`renewal::due`]): the API from the authority beside it, the
//! gateway and the worker by asking the API, over their current mutual-TLS identity, to certify a new key. A
//! [`LiveIdentity`] is what their listeners and clients show: a renewed certificate is shown at every handshake
//! after it, and a connection made before keeps the one it was made with, so that nothing standing is dropped.
//! Nothing here keeps time or opens a socket: the service looks with its own clock and asks in its own way.

use std::future::Future;
use std::sync::{Arc, PoisonError, RwLock};

use orch_types::Time;
use rustls::SignatureScheme;
use rustls::client::ResolvesClientCert;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use crate::PkiError;
use crate::certificates::{Certificate, LeafKey};
use crate::renewal;
use crate::verification::{TlsIdentity, crypto};

/// What is shown now: the chain, the holder's first, and the key as TLS uses it.
struct Shown {
    chain: Vec<Certificate>,
    key: Arc<CertifiedKey>,
}

/// The identity a service shows now, which a renewal replaces in place.
pub struct LiveIdentity {
    shown: RwLock<Shown>,
}

impl std::fmt::Debug for LiveIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveIdentity").finish_non_exhaustive()
    }
}

/// `identity` as TLS shows it.
fn certified(identity: &TlsIdentity) -> Result<Arc<CertifiedKey>, PkiError> {
    let chain = identity.chain.iter().map(Certificate::to_rustls).collect();
    let key = crypto()
        .key_provider
        .load_private_key(identity.key.to_rustls())
        .map_err(|error| PkiError::Key(error.to_string()))?;
    Ok(Arc::new(CertifiedKey::new(chain, key)))
}

impl LiveIdentity {
    /// Shows `identity` until it is replaced.
    pub fn new(identity: &TlsIdentity) -> Result<Arc<Self>, PkiError> {
        Ok(Arc::new(Self {
            shown: RwLock::new(Shown {
                chain: identity.chain.clone(),
                key: certified(identity)?,
            }),
        }))
    }

    /// The holder's certificate shown now.
    #[must_use]
    pub fn certificate(&self) -> Option<Certificate> {
        self.shown
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .chain
            .first()
            .cloned()
    }

    /// Shows `identity` from the next handshake on; connections made before keep what they were made with.
    pub fn replace(&self, identity: &TlsIdentity) -> Result<(), PkiError> {
        let key = certified(identity)?;
        *self.shown.write().unwrap_or_else(PoisonError::into_inner) = Shown {
            chain: identity.chain.clone(),
            key,
        };
        Ok(())
    }

    /// The certificates after the holder's in the chain shown now: the authority's, where it is sent along.
    fn rest(&self) -> Vec<Certificate> {
        self.shown
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .chain
            .iter()
            .skip(1)
            .cloned()
            .collect()
    }

    fn key(&self) -> Arc<CertifiedKey> {
        Arc::clone(
            &self
                .shown
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .key,
        )
    }
}

impl ResolvesServerCert for LiveIdentity {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.key())
    }
}

impl ResolvesClientCert for LiveIdentity {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        Some(self.key())
    }

    fn has_certs(&self) -> bool {
        true
    }
}

/// Why a renewal did not take place.
#[derive(Debug, thiserror::Error)]
pub enum RenewFault {
    /// A key, a request or the answer could not be made or read.
    #[error(transparent)]
    Pki(#[from] PkiError),
    /// Whoever was asked refused, or could not be reached.
    #[error("the renewal was not given: {0}")]
    Refused(String),
    /// The answer is not a certificate the service can show.
    #[error("{0}")]
    Wrong(&'static str),
}

/// Renews `identity` where it is due at `now`: a new key, a request for it that `renew` has certified (it answers
/// the certificate as PEM, or why not), and the answer adopted once it is checked. Returns whether it renewed; a
/// service looks again later after a failure, while its certificate still holds.
pub async fn renew_if_due<F, Fut>(
    identity: &LiveIdentity,
    now: Time,
    renew: F,
) -> Result<bool, RenewFault>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    let current = identity
        .certificate()
        .ok_or(RenewFault::Wrong("the service shows no certificate"))?;
    if !renewal::due(&current, now) {
        return Ok(false);
    }
    let (key, csr_pem) = renewal::start()?;
    let pem = renew(csr_pem).await.map_err(RenewFault::Refused)?;
    adopt(identity, &current, key, &pem)?;
    Ok(true)
}

/// Shows the certificate `pem`, made for `key`, in place of `current`: only one that holds that key and names
/// the same holder, so that a service never shows a certificate of somebody else's.
pub fn adopt(
    identity: &LiveIdentity,
    current: &Certificate,
    key: LeafKey,
    pem: &str,
) -> Result<(), RenewFault> {
    let renewed = Certificate::from_pem(pem)?;
    if !renewed.holds(&key) {
        return Err(RenewFault::Wrong(
            "the renewed certificate is not for the new key",
        ));
    }
    if renewed.identities() != current.identities() {
        return Err(RenewFault::Wrong(
            "the renewed certificate names another holder",
        ));
    }
    let chain = std::iter::once(renewed).chain(identity.rest()).collect();
    identity.replace(&TlsIdentity { chain, key })?;
    Ok(())
}
