//! Signed tunnel ticket mechanisms.
//!
//! Who may open a tunnel to whom is decided by the system alone, and is
//! written down as a ticket: a few claims, signed with a key that only the
//! system holds. A gateway that holds the matching public key can tell a
//! real ticket from a forged one without asking anybody, and refuses a
//! ticket that names somebody else than the machine that presents it.
//!
//! A ticket is `v1.<claims>.<signature>`, the claims as base64url JSON and
//! the signature Ed25519 over the first two parts.

use crate::PkiError;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use orch_link_contract::tunnels::{OpenErrorCode, Service, Ticket, TicketClaims, TicketSource};
use orch_types::{Id, Time, new_id};

/// The bytes of an Ed25519 secret key.
const SECRET_BYTES: usize = 32;
/// Milliseconds in a second.
const SECOND_MILLIS: i64 = 1_000;

/// What the system fills in when it makes a ticket; the rest is the
/// ticket's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRequest {
    /// Who opens the tunnel.
    pub src: TicketSource,
    /// The machine it ends at.
    pub dst: Id,
    /// The service it ends at there.
    pub service: Service,
    /// The thread it serves, where it serves one.
    pub thread_id: Option<Id>,
    /// The tool set the executor lists on every channel of the ticket.
    pub tool_set: Option<String>,
    /// Until when channels may be opened with it.
    pub expires_at: Time,
}

/// The system's signing key for tickets.
pub struct TicketSigner {
    key: SigningKey,
}

impl std::fmt::Debug for TicketSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TicketSigner(..)")
    }
}

impl TicketSigner {
    /// Makes a key. The system does this once each time it starts and
    /// gives the public half to its gateways.
    #[must_use]
    pub fn generate() -> Self {
        let mut secret = [0u8; SECRET_BYTES];
        rand::fill(&mut secret[..]);
        Self::from_secret(secret)
    }

    /// The key whose secret is `secret`.
    #[must_use]
    pub fn from_secret(secret: [u8; SECRET_BYTES]) -> Self {
        Self {
            key: SigningKey::from_bytes(&secret),
        }
    }

    /// The public half, for the gateways.
    #[must_use]
    pub fn verifier(&self) -> TicketVerifier {
        TicketVerifier {
            key: self.key.verifying_key(),
        }
    }

    /// The public half as the system hands it to a gateway: unpadded
    /// base64url of its 32 bytes.
    #[must_use]
    pub fn public_key_text(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.verifying_key().to_bytes())
    }

    /// Signs `claims`.
    pub fn sign(&self, claims: &TicketClaims) -> Result<String, PkiError> {
        let signing_input = Ticket::signing_input(claims)
            .map_err(|e| PkiError::Signature(format!("the claims cannot be written: {e}")))?;
        let signature = self.key.sign(signing_input.as_bytes());
        Ok(Ticket::assemble(&signing_input, &signature.to_bytes()))
    }

    /// Makes and signs the ticket that `request` asks for.
    pub fn issue(&self, request: &TicketRequest) -> Result<String, PkiError> {
        let claims = TicketClaims {
            jti: new_id(),
            kind: request.service.kind(),
            src: request.src,
            dst: request.dst,
            service: request.service.to_string(),
            thread_id: request.thread_id,
            tool_set: request.tool_set.clone(),
            port: None,
            exp: request.expires_at.unix_millis().div_euclid(SECOND_MILLIS),
        };
        self.sign(&claims)
    }
}

/// The public half of the system's ticket key: what a gateway checks with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketVerifier {
    key: VerifyingKey,
}

impl TicketVerifier {
    /// Reads the key as [`TicketSigner::public_key_text`] writes it.
    pub fn from_text(text: &str) -> Result<Self, PkiError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(text)
            .map_err(|_| PkiError::Signature("the ticket key is not base64url".to_owned()))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| PkiError::Signature("a ticket key is 32 bytes".to_owned()))?;
        VerifyingKey::from_bytes(&bytes)
            .map(|key| Self { key })
            .map_err(|e| PkiError::Signature(format!("not an Ed25519 key: {e}")))
    }

    /// The claims of `ticket`, if the system signed it, `presenter` is who
    /// it names, and it is not past its time at `now`. A ticket that is
    /// not the system's, or not its presenter's, is a bad ticket whatever
    /// else is wrong with it; only a genuine one can be told to be late.
    pub fn verify(
        &self,
        ticket: &str,
        presenter: TicketSource,
        now: Time,
    ) -> Result<TicketClaims, OpenErrorCode> {
        let parsed = Ticket::parse(ticket).map_err(|_| OpenErrorCode::BadTicket)?;
        let signature: [u8; 64] = parsed
            .signature()
            .try_into()
            .map_err(|_| OpenErrorCode::BadTicket)?;
        self.key
            .verify(parsed.signed(), &Signature::from_bytes(&signature))
            .map_err(|_| OpenErrorCode::BadTicket)?;
        let claims = parsed.claims();
        // A presenter who is not the ticket's is refused as a bad ticket even
        // when the ticket is also late: late is only told to its owner.
        if claims.src != presenter {
            return Err(OpenErrorCode::BadTicket);
        }
        claims.check(presenter, now)?;
        Ok(claims.clone())
    }
}
