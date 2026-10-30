//! Port caller assertions: the short-lived signed statement of who a port's caller is.
//!
//! A program inside an environment can read plain headers that name its caller, but any other
//! program beside it can send the same headers. What it trusts instead is an assertion: a compact
//! JWS signed with the installation's dedicated Ed25519 key, bound to one environment, one port,
//! one connection and its generation, and valid for thirty seconds. The program fetches the public
//! key from the environment's own ports endpoint or from the installation's public address and
//! checks the signature, the algorithm, the issuer, the audience of its own target and the time.
//!
//! The signing key belongs to the installation's protected secrets and never enters a unit; only
//! its public half crosses the link. An assertion says who is calling; it is no permission to open
//! another port.

use crate::PkiError;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use orch_types::{Id, Time, new_id};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// How long an assertion holds: the local hop that verifies it needs a moment, and a copied
/// assertion is worthless soon after.
pub const ASSERTION_SECONDS: i64 = 30;
/// The clock skew a verifier allows on either side of an assertion's times.
pub const ASSERTION_SKEW_SECONDS: i64 = 5;
/// The JOSE algorithm an assertion is signed with: the fully specified identifier of Ed25519.
pub const ASSERTION_ALGORITHM: &str = "Ed25519";
/// The curve of the key, as its public key names it.
pub const ASSERTION_CURVE: &str = "Ed25519";
/// The type an assertion's header names.
pub const ASSERTION_TYPE: &str = "orch-port+jwt";
/// The bytes of the signing key's secret.
pub const SECRET_BYTES: usize = 32;
/// The file of the installation's secrets that holds the key's secret, in hexadecimal.
pub const KEY_FILE: &str = "orch_ports_assertion_key";
/// The hexadecimal digits of the public key's digest that name it.
const KID_DIGITS: usize = 16;

/// The audience of an assertion: one environment's one port. A program expands the template the
/// public keys come with to its own target and compares.
#[must_use]
pub fn audience(environment: Id, port: u16) -> String {
    format!("orch-port:{environment}:{port}")
}

/// The audience template the public keys are given with.
pub const AUDIENCE_TEMPLATE: &str = "orch-port:{environment_id}:{port}";

/// Who the assertion says is calling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertedCaller {
    /// A raw host peer, proved only by its observed outside address; no person/link is invented.
    Address,
    /// A signed-in person of the installation.
    Person {
        /// Their identifier.
        id: Id,
        /// Their username.
        username: String,
        /// The name they are shown by.
        display_name: String,
    },
    /// Whoever holds a link.
    Link {
        /// The link's opening.
        id: Id,
        /// The link's label.
        label: String,
    },
}

/// What one assertion states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortAssertion {
    /// The installation's public address.
    pub issuer: String,
    /// The environment of the target.
    pub environment_id: Id,
    /// The port of the target.
    pub port: u16,
    /// `tcp` or `udp`.
    pub transport: &'static str,
    /// The opening that admitted the caller; none for a holder's own use.
    pub opening_id: Option<Id>,
    /// The connection the caller holds.
    pub connection_id: Id,
    /// The generation of the environment's runtime the connection was made to.
    pub generation: i64,
    /// The address the caller came from, as Orch accepted it.
    pub outside_address: String,
    /// Its port.
    pub outside_port: u16,
    /// Who the caller is.
    pub caller: AssertedCaller,
}

/// The installation's port assertion key.
pub struct PortAssertionSigner {
    key: SigningKey,
    kid: String,
}

impl std::fmt::Debug for PortAssertionSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortAssertionSigner")
            .field("kid", &self.kid)
            .finish_non_exhaustive()
    }
}

fn kid_of(key: &VerifyingKey) -> String {
    let digest = hex::encode(Sha256::digest(key.to_bytes()));
    digest.chars().take(KID_DIGITS).collect()
}

impl PortAssertionSigner {
    /// Makes a fresh key: for an installation whose secrets hold none, which then cannot vouch
    /// for an assertion made before it restarts.
    #[must_use]
    pub fn generate() -> Self {
        let mut secret = [0u8; SECRET_BYTES];
        rand::fill(&mut secret[..]);
        Self::from_secret(secret)
    }

    /// The key whose secret is `secret`, as the installation's secrets keep it.
    #[must_use]
    pub fn from_secret(secret: [u8; SECRET_BYTES]) -> Self {
        let key = SigningKey::from_bytes(&secret);
        let kid = kid_of(&key.verifying_key());
        Self { key, kid }
    }

    /// The key's reference, which an assertion's header names.
    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The public half: unpadded base64url of its 32 bytes, the `x` of its JSON Web Key.
    #[must_use]
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.verifying_key().to_bytes())
    }

    /// Signs what `assertion` states, issued at `now`, as a compact JWS.
    pub fn sign(&self, assertion: &PortAssertion, now: Time) -> Result<String, PkiError> {
        let issued = now.get().timestamp();
        let mut claims = json!({
            "iss": assertion.issuer,
            "aud": audience(assertion.environment_id, assertion.port),
            "opening_id": assertion.opening_id,
            "environment_id": assertion.environment_id,
            "port": assertion.port,
            "transport": assertion.transport,
            "connection_id": assertion.connection_id,
            "generation": assertion.generation,
            "outside": { "address": assertion.outside_address, "port": assertion.outside_port },
            "iat": issued,
            "exp": issued + ASSERTION_SECONDS,
            "jti": new_id(),
        });
        match &assertion.caller {
            AssertedCaller::Address => {}
            AssertedCaller::Person {
                id,
                username,
                display_name,
            } => {
                claims["person"] =
                    json!({ "id": id, "username": username, "display_name": display_name });
            }
            AssertedCaller::Link { id, label } => {
                claims["link"] = json!({ "id": id, "label": label });
            }
        }
        let header = json!({ "alg": ASSERTION_ALGORITHM, "kid": self.kid, "typ": ASSERTION_TYPE });
        let part = |value: &Value| {
            serde_json::to_vec(value)
                .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
                .map_err(|error| {
                    PkiError::Signature(format!("an assertion cannot be written: {error}"))
                })
        };
        let input = format!("{}.{}", part(&header)?, part(&claims)?);
        let signature = self.key.sign(input.as_bytes());
        Ok(format!(
            "{input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }
}

/// Checks an assertion as a program inside should: the public key `public_key` (base64url), the
/// algorithm, the issuer, the audience of its own `environment` and `port`, and the time at `now`
/// with the allowed skew. Answers its claims.
pub fn verify(
    assertion: &str,
    public_key: &str,
    issuer: &str,
    environment: Id,
    port: u16,
    now: Time,
) -> Result<Value, PkiError> {
    let refused = |why: &str| PkiError::Signature(format!("the assertion is refused: {why}"));
    let mut parts = assertion.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(refused("it is not a compact JWS"));
    };
    let decode = |part: &str| {
        URL_SAFE_NO_PAD
            .decode(part)
            .map_err(|_| refused("a part is not base64url"))
    };
    let header: Value =
        serde_json::from_slice(&decode(header)?).map_err(|_| refused("the header is not JSON"))?;
    if header["alg"] != ASSERTION_ALGORITHM || header["typ"] != ASSERTION_TYPE {
        return Err(refused("the algorithm or type is not the assertion's"));
    }
    let key_bytes: [u8; 32] = decode(public_key)?
        .try_into()
        .map_err(|_| refused("the public key is not 32 bytes"))?;
    let key =
        VerifyingKey::from_bytes(&key_bytes).map_err(|_| refused("the public key is not a key"))?;
    let signature_bytes: [u8; 64] = decode(signature)?
        .try_into()
        .map_err(|_| refused("the signature is not 64 bytes"))?;
    let input = &assertion[..assertion.rfind('.').unwrap_or(0)];
    key.verify(input.as_bytes(), &Signature::from_bytes(&signature_bytes))
        .map_err(|_| refused("the signature does not hold"))?;
    let claims: Value = serde_json::from_slice(&decode(payload)?)
        .map_err(|_| refused("the claims are not JSON"))?;
    if claims["iss"] != issuer {
        return Err(refused("another installation issued it"));
    }
    if claims["aud"] != audience(environment, port).as_str() {
        return Err(refused("it is for another target"));
    }
    let at = now.get().timestamp();
    let (Some(issued), Some(expires)) = (claims["iat"].as_i64(), claims["exp"].as_i64()) else {
        return Err(refused("it has no times"));
    };
    if issued > at + ASSERTION_SKEW_SECONDS || expires < at - ASSERTION_SKEW_SECONDS {
        return Err(refused("it is not valid now"));
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};

    fn assertion(environment: Id) -> PortAssertion {
        PortAssertion {
            issuer: "https://orch.example.com".into(),
            environment_id: environment,
            port: 8080,
            transport: "tcp",
            opening_id: None,
            connection_id: new_id(),
            generation: 3,
            outside_address: "198.51.100.7".into(),
            outside_port: 51_000,
            caller: AssertedCaller::Person {
                id: new_id(),
                username: "ada".into(),
                display_name: "Ada Lovelace".into(),
            },
        }
    }

    #[test]
    fn a_raw_host_peer_asserts_only_its_observed_address_and_never_a_person_or_link() {
        let signer = PortAssertionSigner::generate();
        let environment = new_id();
        let now = Time::new(Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap());
        let mut facts = assertion(environment);
        facts.caller = AssertedCaller::Address;
        let signed = signer.sign(&facts, now).unwrap();
        let claims = verify(
            &signed,
            &signer.public_key(),
            &facts.issuer,
            environment,
            facts.port,
            now,
        )
        .unwrap();
        assert!(claims.get("person").is_none());
        assert!(claims.get("link").is_none());
        assert_eq!(claims["outside"]["address"], facts.outside_address);
        assert_eq!(claims["connection_id"], facts.connection_id.to_string());
    }

    #[test]
    fn a_program_verifies_its_own_target_and_refuses_another_target_another_key_or_a_late_copy() {
        let signer = PortAssertionSigner::generate();
        let environment = new_id();
        let now = Time::new(Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap());
        let signed = signer.sign(&assertion(environment), now).unwrap();
        let claims = verify(
            &signed,
            &signer.public_key(),
            "https://orch.example.com",
            environment,
            8080,
            now,
        )
        .unwrap();
        assert_eq!(claims["person"]["username"], "ada");
        assert!(claims.get("link").is_none());
        assert!(
            verify(
                &signed,
                &signer.public_key(),
                "https://orch.example.com",
                environment,
                8081,
                now
            )
            .is_err()
        );
        let other = PortAssertionSigner::generate();
        assert!(
            verify(
                &signed,
                &other.public_key(),
                "https://orch.example.com",
                environment,
                8080,
                now
            )
            .is_err()
        );
        let late = Time::new(
            now.get() + Duration::seconds(ASSERTION_SECONDS + ASSERTION_SKEW_SECONDS + 1),
        );
        assert!(
            verify(
                &signed,
                &signer.public_key(),
                "https://orch.example.com",
                environment,
                8080,
                late
            )
            .is_err()
        );
    }

    #[test]
    fn the_same_secret_keeps_its_key_reference_and_its_public_half() {
        let secret = [7u8; SECRET_BYTES];
        let first = PortAssertionSigner::from_secret(secret);
        let again = PortAssertionSigner::from_secret(secret);
        assert_eq!(first.kid(), again.kid());
        assert_eq!(first.kid().len(), KID_DIGITS);
        assert_eq!(first.public_key(), again.public_key());
    }
}
