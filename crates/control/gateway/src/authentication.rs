//! Certificate and signed-handshake checks with API authorization.
//!
//! Who a machine is gets settled in two steps that are kept apart. The
//! connection and the handshake say it (the certificate of the connection,
//! or, behind the public edge, a signature over what the gateway sent, both
//! checked against the system's authority by the link's session code) and the
//! system says whether that machine, with that certificate, may have a session
//! at all: it may not if it is unknown, was revoked, or shows a certificate
//! that is not the one it is known by. The gateway keeps no list of either.
//!
//! The requests a machine makes beside its session are told apart the same
//! way: on the pinned port by the certificate, which the connection has
//! checked, and behind the edge by the token its session was given.

use crate::Gateway;
use crate::backplane::BackplaneHandle;
use crate::listeners::{Kind, Tls};
use async_trait::async_trait;
use http::{HeaderMap, header};
use orch_link::session::SessionAuthority;
use orch_link_contract::backplane::{AuthQuery, AuthReply};
use orch_link_contract::content::session_token;
use orch_link_contract::version::RejectCode;
use orch_types::Id;

/// The system, as the gateway asks it whether a session may start: over the
/// backplane. While the backplane is down the answer is `busy`, which tells
/// the machine to try again in a few seconds.
pub struct BackplaneAuthority {
    backplane: BackplaneHandle,
}

impl BackplaneAuthority {
    /// An authority that asks over the gateway's backplane.
    #[must_use]
    pub fn new(gateway: &Gateway) -> Self {
        Self {
            backplane: gateway.inner.backplane.clone(),
        }
    }
}

#[async_trait]
impl SessionAuthority for BackplaneAuthority {
    async fn authorize(&self, query: AuthQuery) -> AuthReply {
        let n = query.n;
        match self.backplane.ask(query).await {
            Some(reply) => AuthReply { n, ..reply },
            None => AuthReply {
                endpoints: None,
                update_permit: None,
                n,
                ok: false,
                code: Some(RejectCode::Busy),
                cursors: None,
                placement: None,
            },
        }
    }
}

/// Who a request is from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identified {
    /// The machine.
    pub machine_id: Id,
    /// The SHA-256 of the certificate it holds its session with, or has shown,
    /// in hexadecimal.
    pub fingerprint: String,
}

/// Why a request is not from a machine that can be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unidentified {
    /// Nothing was shown that this kind of port takes.
    NothingShown,
    /// What was shown is not good: a certificate that is no machine's or has
    /// expired, a token no session has.
    NotGood,
}

/// Tells who makes a request, as the kind of port it came in on allows.
pub(crate) fn identify(
    gateway: &Gateway,
    kind: Kind,
    tls: &Tls,
    headers: &HeaderMap,
) -> Result<Identified, Unidentified> {
    match kind {
        Kind::Pinned => {
            let certificate = tls
                .client_certificate
                .as_ref()
                .ok_or(Unidentified::NothingShown)?;
            let machine_id = certificate.machine_id().ok_or(Unidentified::NotGood)?;
            if !certificate.valid_at(gateway.clock().time()) {
                return Err(Unidentified::NotGood);
            }
            Ok(Identified {
                machine_id,
                fingerprint: certificate.fingerprint(),
            })
        }
        Kind::Edge => {
            let authorization = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .ok_or(Unidentified::NothingShown)?;
            let token = session_token(authorization).ok_or(Unidentified::NotGood)?;
            let (machine_id, fingerprint) = gateway
                .inner
                .sessions
                .machine_of_token(token)
                .ok_or(Unidentified::NotGood)?;
            Ok(Identified {
                machine_id,
                fingerprint,
            })
        }
    }
}
