//! What the tests of the authority have in common: a clock that stands still, an authority, identities, and a TLS
//! handshake between two configurations in memory.

#![allow(dead_code, reason = "each test file uses some of these")]

use chrono::{TimeZone, Utc};
use orch_pki::ca::{CertificateAuthority, Holder};
use orch_pki::certificates::{Certificate, LeafKey, ServiceName};
use orch_pki::verification::TlsIdentity;
use orch_types::{Id, ManualClock, new_id};
use rustls::pki_types::ServerName;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// The moment every test starts at.
pub fn clock() -> ManualClock {
    ManualClock::new(
        Utc.with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    )
}

/// A new authority for a system called Test, good for the shipped ten years.
pub fn authority(clock: &ManualClock) -> CertificateAuthority {
    CertificateAuthority::create("Test", 3_650, clock).unwrap()
}

/// A machine's key and certificate, issued by `authority` for 365 days.
pub fn machine(authority: &CertificateAuthority, clock: &ManualClock) -> (Id, TlsIdentity) {
    let id = new_id();
    let key = LeafKey::generate().unwrap();
    let request =
        orch_pki::certificates::CertificateRequest::from_pem(&key.request_pem().unwrap()).unwrap();
    let certificate = authority
        .issue(&request, &Holder::Machine(id), 365, clock)
        .unwrap();
    (
        id,
        TlsIdentity {
            chain: vec![certificate],
            key,
        },
    )
}

/// A service's key and certificate for the names given, with the authority's own certificate after it in the
/// chain, as a gateway sends it.
pub fn service(
    authority: &CertificateAuthority,
    clock: &ManualClock,
    service: ServiceName,
    names: &[&str],
) -> TlsIdentity {
    let holder = Holder::Service {
        service,
        names: names.iter().map(|name| (*name).to_owned()).collect(),
    };
    let (key, certificate) = authority.issue_with_new_key(&holder, 365, clock).unwrap();
    TlsIdentity {
        chain: vec![certificate, authority.certificate().clone()],
        key,
    }
}

/// What a handshake came to for each side.
pub struct Outcome {
    /// The certificate the server saw the client show, if any.
    pub client_certificate: Option<Certificate>,
}

/// Runs a TLS handshake between a client and a server configuration, and one byte each way so that an alert
/// the server raised after the client was done reaches the client. `Err` carries what either side said.
pub async fn handshake(
    client: Arc<rustls::ClientConfig>,
    server: Arc<rustls::ServerConfig>,
    name: &'static str,
) -> Result<Outcome, String> {
    let (near, far) = duplex(64 * 1024);
    let acceptor = TlsAcceptor::from(server);
    let connector = TlsConnector::from(client);
    let serve = async move {
        let mut stream = acceptor
            .accept(far)
            .await
            .map_err(|e| format!("server: {e}"))?;
        let seen =
            orch_pki::verification::presented_certificate(stream.get_ref().1.peer_certificates());
        let mut byte = [0u8; 1];
        stream
            .read_exact(&mut byte)
            .await
            .map_err(|e| format!("server read: {e}"))?;
        stream
            .write_all(&byte)
            .await
            .map_err(|e| format!("server write: {e}"))?;
        stream
            .flush()
            .await
            .map_err(|e| format!("server flush: {e}"))?;
        Ok::<_, String>(seen)
    };
    let call = async move {
        let name = ServerName::try_from(name).unwrap();
        let mut stream = connector
            .connect(name, near)
            .await
            .map_err(|e| format!("client: {e}"))?;
        stream
            .write_all(&[7])
            .await
            .map_err(|e| format!("client write: {e}"))?;
        stream
            .flush()
            .await
            .map_err(|e| format!("client flush: {e}"))?;
        let mut byte = [0u8; 1];
        stream
            .read_exact(&mut byte)
            .await
            .map_err(|e| format!("client read: {e}"))?;
        Ok::<_, String>(())
    };
    let (served, called) = tokio::join!(serve, call);
    called?;
    Ok(Outcome {
        client_certificate: served?,
    })
}
