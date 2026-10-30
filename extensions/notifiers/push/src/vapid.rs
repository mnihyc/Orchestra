//! Configured VAPID identity.
//!
//! Web Push tells a push service who is sending: the sender signs a short
//! token with a key of its own (VAPID, RFC 8292) and gives its public half, so
//! that the service can tie a flood of messages to one sender, and so that a
//! browser's subscription, which was made for that public key, accepts nothing
//! from any other. The system has one such key (a file of the deployment's
//! secrets), and its public half is what a browser is given to subscribe with.
//!
//! The key is a P-256 private key, written as PEM (`PRIVATE KEY` or
//! `EC PRIVATE KEY`) or as the 32 bytes of the private value in URL-safe
//! base64 without padding, which is what most key generators print.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_types::Clock;
use p256::SecretKey;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;
use serde_json::json;
use url::Url;

/// The bytes of the private value of a P-256 key.
const PRIVATE_VALUE_BYTES: usize = 32;
/// A token lasts twelve hours, within RFC 8292's maximum of twenty-four hours.
const TOKEN_LIFETIME_SECS: i64 = 12 * 60 * 60;
/// VAPID requires the ES256 algorithm and a JWT header.
const TOKEN_HEADER: &[u8] = br#"{"typ":"JWT","alg":"ES256"}"#;

/// Why a key could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VapidError {
    /// The text is neither a PEM key nor the base64 of a private value.
    #[error("the VAPID key is neither a PEM private key nor a base64 private value")]
    NotAKey,
    /// The sender is named by nothing a push service accepts.
    #[error("the VAPID contact is an address that starts with https:// or mailto:")]
    NoContact,
    /// A token could not be encoded or signed.
    #[error("the VAPID token could not be signed")]
    CannotSign,
}

/// The system's VAPID identity.
#[derive(Clone)]
pub struct Vapid {
    key: SigningKey,
    public_key: String,
    contact: String,
}

impl std::fmt::Debug for Vapid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vapid")
            .field("public_key", &self.public_key)
            .field("contact", &self.contact)
            .finish_non_exhaustive()
    }
}

impl Vapid {
    /// Reads the key from `secret` and names the sender by `contact`, an
    /// `https://` address or a `mailto:` one, which a push service may use to
    /// reach whoever runs this system.
    pub fn read(secret: &str, contact: &str) -> Result<Self, VapidError> {
        let contact = contact.trim();
        if !(contact.starts_with("https://") || contact.starts_with("mailto:")) {
            return Err(VapidError::NoContact);
        }
        let text = secret.trim();
        let secret = if text.contains("-----BEGIN") {
            SecretKey::from_pkcs8_pem(text)
                .or_else(|_| SecretKey::from_sec1_pem(text))
                .map_err(|_| VapidError::NotAKey)?
        } else {
            let private_value = URL_SAFE_NO_PAD
                .decode(text)
                .map_err(|_| VapidError::NotAKey)?;
            if private_value.len() != PRIVATE_VALUE_BYTES {
                return Err(VapidError::NotAKey);
            }
            SecretKey::from_slice(&private_value).map_err(|_| VapidError::NotAKey)?
        };
        let key = SigningKey::from(secret);
        Ok(Self {
            public_key: URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes()),
            key,
            contact: contact.to_owned(),
        })
    }

    /// The public half of the key, in URL-safe base64 without padding: what a
    /// browser is given as the application server key to subscribe with.
    #[must_use]
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// The authorization for this endpoint's origin, signed with the sender's key.
    pub(crate) fn sign_for(&self, endpoint: &Url, clock: &dyn Clock) -> Result<String, VapidError> {
        let expires = clock
            .now()
            .timestamp()
            .checked_add(TOKEN_LIFETIME_SECS)
            .ok_or(VapidError::CannotSign)?;
        let claims = json!({
            "aud": endpoint.origin().ascii_serialization(),
            "exp": expires,
            "sub": self.contact,
        });
        let claims = serde_json::to_vec(&claims).map_err(|_| VapidError::CannotSign)?;
        let unsigned = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(TOKEN_HEADER),
            URL_SAFE_NO_PAD.encode(claims)
        );
        let signature: Signature = self
            .key
            .try_sign(unsigned.as_bytes())
            .map_err(|_| VapidError::CannotSign)?;
        Ok(format!(
            "vapid t={unsigned}.{}, k={}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes()),
            self.public_key
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A published VAPID test key, whose public value browsers subscribe with.
    const PRIVATE_BASE64: &str = "IQ9Ur0ykXoHS9gzfYX0aBjy9lvdrjx_PFUXmie9YRcY";
    const PUBLIC_BASE64: &str =
        "BMjQIp55pdbU8pfCBKyXcZjlmER_mXt5LqNrN1hrXbdBS5EnhIbMu3Au-RV53iIpztzNXkGI56BFB1udQ8Bq_H4";

    #[test]
    fn a_key_written_as_base64_gives_the_public_key_a_browser_subscribes_with() {
        let vapid = Vapid::read(PRIVATE_BASE64, "https://orch.example").unwrap();
        assert_eq!(vapid.public_key(), PUBLIC_BASE64);
        assert_eq!(
            Vapid::read(&format!("{PRIVATE_BASE64}\n"), "mailto:ops@orch.example")
                .unwrap()
                .public_key(),
            PUBLIC_BASE64,
            "the end of a line in a file is not part of the key"
        );
    }

    #[test]
    fn what_is_not_a_key_or_names_no_contact_is_refused() {
        for bytes in [[0; PRIVATE_VALUE_BYTES], [u8::MAX; PRIVATE_VALUE_BYTES]] {
            assert_eq!(
                Vapid::read(&URL_SAFE_NO_PAD.encode(bytes), "https://orch.example").unwrap_err(),
                VapidError::NotAKey,
                "a scalar outside the curve's range is refused without panicking"
            );
        }
        assert_eq!(
            Vapid::read("not a key", "https://orch.example").unwrap_err(),
            VapidError::NotAKey
        );
        assert_eq!(
            Vapid::read("", "https://orch.example").unwrap_err(),
            VapidError::NotAKey
        );
        assert_eq!(
            Vapid::read("AAAA", "https://orch.example").unwrap_err(),
            VapidError::NotAKey,
            "too short"
        );
        assert_eq!(
            Vapid::read(
                "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----",
                "https://orch.example"
            )
            .unwrap_err(),
            VapidError::NotAKey
        );
        for contact in ["", "ops@orch.example", "http://orch.example", "ftp://x"] {
            assert_eq!(
                Vapid::read(PRIVATE_BASE64, contact).unwrap_err(),
                VapidError::NoContact,
                "{contact:?}"
            );
        }
    }

    #[test]
    fn the_debug_form_shows_the_public_key_and_never_the_private_one() {
        let shown = format!(
            "{:?}",
            Vapid::read(PRIVATE_BASE64, "https://orch.example").unwrap()
        );
        assert!(shown.contains(PUBLIC_BASE64) && !shown.contains(PRIVATE_BASE64));
    }

    #[test]
    fn both_private_key_pem_forms_keep_the_same_browser_identity() {
        use p256::pkcs8::{EncodePrivateKey, LineEnding};

        let secret =
            SecretKey::from_slice(&URL_SAFE_NO_PAD.decode(PRIVATE_BASE64).unwrap()).unwrap();
        for pem in [
            secret.to_pkcs8_pem(LineEnding::LF).unwrap(),
            secret.to_sec1_pem(LineEnding::LF).unwrap(),
        ] {
            assert_eq!(
                Vapid::read(&pem, "mailto:ops@orch.example")
                    .unwrap()
                    .public_key(),
                PUBLIC_BASE64
            );
        }
    }

    #[test]
    fn a_signed_token_names_the_exact_origin_and_expires_by_the_supplied_clock() {
        use orch_types::{ManualClock, Time};
        use p256::ecdsa::signature::Verifier;

        let at = "2026-10-14T01:00:00.000Z".parse::<Time>().unwrap().get();
        let clock = ManualClock::new(at);
        let vapid = Vapid::read(PRIVATE_BASE64, "mailto:ops@orch.example").unwrap();
        for origin in [
            "https://push.example",
            "https://push.example:8443",
            "https://[2001:db8::1]:8443",
        ] {
            let endpoint = Url::parse(&format!("{origin}/push/device?secret=example")).unwrap();
            let authorization = vapid.sign_for(&endpoint, &clock).unwrap();
            let token = authorization
                .strip_prefix("vapid t=")
                .unwrap()
                .split(", ")
                .next()
                .unwrap();
            let parts: Vec<_> = token.split('.').collect();
            let header: serde_json::Value =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
            assert_eq!(header, json!({"typ":"JWT", "alg":"ES256"}));
            let claims: serde_json::Value =
                serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
            assert_eq!(
                claims,
                json!({"aud":origin, "exp":at.timestamp() + TOKEN_LIFETIME_SECS, "sub":"mailto:ops@orch.example"})
            );
            let signature =
                Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
            let unsigned = format!("{}.{}", parts[0], parts[1]);
            vapid
                .key
                .verifying_key()
                .verify(unsigned.as_bytes(), &signature)
                .unwrap();
            assert!(
                vapid
                    .key
                    .verifying_key()
                    .verify(b"changed token", &signature)
                    .is_err()
            );
        }
    }
}
