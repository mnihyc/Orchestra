//! Enrollment and renewal payloads.
//!
//! A machine becomes one of a system's by enrolling: it presents a token
//! that somebody made for it, and a request for a certificate over a key
//! that never leaves it. The token is good once and for a short while,
//! and it carries a pin: the hash of the public key of the system's own
//! certificate authority. At first contact the machine trusts nothing
//! but that pin, so that a token copied from a screen is enough to know
//! that the answer comes from the right system.
//!
//! The answer names the machine, gives it its certificate, and lists the
//! endpoints it may hold its session with. A certificate is renewed the
//! same way, over a session's identity, before it runs out.
//!
//! At a public edge renewal carries the current certificate's PEM as unpadded
//! base64url in `x-orch-renew-certificate`, a signature in
//! `x-orch-renew-signature`, and its RFC 3339 signing time in
//! `x-orch-renew-sent-at`. The machine's existing P-256 key signs five UTF-8
//! lines, joined by LF with no final LF: `orch-link-renew-v1`, the lowercase
//! SHA-256 of the exact `csr_pem` bytes, the machine UUID, the endpoint's
//! authority (including its port when present), and the signing time. The
//! signature is unpadded base64url of the 64 bytes `r || s`. The gateway
//! supplies the connected host to the verifier; arbitrary client forwarding
//! headers do not choose that host. Certificate identity, current/successor
//! fingerprint, expiry grace and the session clock allowance still apply.

use crate::session::{Platform, Role};
use crate::version::{PATH_PREFIX, SESSION_PATH};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The path a machine enrolls at, on the address it was given.
pub const ENROLL_PATH: &str = "/link/v1/enroll";
/// The path a machine renews its certificate at, on its endpoint.
pub const RENEW_PATH: &str = "/link/v1/renew";
/// The current certificate's UTF-8 PEM, encoded as unpadded base64url.
pub const RENEW_CERTIFICATE_HEADER: &str = "x-orch-renew-certificate";
/// The request-bound P-256 signature, encoded as unpadded base64url.
pub const RENEW_SIGNATURE_HEADER: &str = "x-orch-renew-signature";
/// The RFC 3339 moment at which the machine signed the request.
pub const RENEW_SENT_AT_HEADER: &str = "x-orch-renew-sent-at";
/// The first line of the renewal signing input, separating it from every other use of the key.
pub const RENEWAL_CONTEXT: &str = "orch-link-renew-v1";
/// How every enrollment token begins.
pub const TOKEN_PREFIX: &str = "oenr_";
/// How long an enrollment token is good for: long enough to carry it to
/// another machine by hand, short enough to be worthless if it is found
/// later.
pub const TOKEN_VALID_MINUTES: i64 = 15;
/// How long an enrollment or a renewal may take before it is reported
/// as failed, for a person to try again.
pub const REQUEST_TIMEOUT_SECS: u32 = 30;
/// The bytes of a pin: a SHA-256.
pub const PIN_BYTES: usize = 32;
/// How long a machine's or a service's certificate is valid, in days,
/// as a system ships.
pub const LEAF_VALIDITY_DAYS: i64 = 365;
/// For how many days after it ran out a machine's certificate is still
/// taken, by the renewal and by nothing else: a machine that was
/// switched off for a season comes back without being enrolled again.
pub const EXPIRED_RENEW_GRACE_DAYS: i64 = 90;
/// How a machine is named in its certificate, before its identifier.
pub const MACHINE_URI_PREFIX: &str = "orch://machine/";
/// How a service of the system is named in its certificate, before its
/// name.
pub const SERVICE_URI_PREFIX: &str = "orch://service/";

/// How a machine knows that an endpoint is its system's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    /// The endpoint shows a certificate of the system's own authority,
    /// and wants the machine's certificate in return. At first contact
    /// the machine checks that the authority's key has the token's pin.
    Pinned,
    /// The endpoint shows a certificate any browser would take, and
    /// sees no certificate of the machine. At first contact the machine
    /// checks it against the public authorities, and checks that the
    /// authority it is handed in the answer has the token's pin.
    Public,
}

/// An address a machine may hold its session with.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Endpoint {
    /// The address.
    pub url: String,
    /// How the machine knows that it is its system's.
    pub trust: Trust,
}

impl Endpoint {
    /// The scheme, host and port of the address, without any path.
    fn origin(&self) -> Option<&str> {
        let after_scheme = self.url.find("://")? + "://".len();
        let authority = self.url[after_scheme..]
            .split('/')
            .next()
            .filter(|authority| !authority.is_empty())?;
        Some(&self.url[..after_scheme + authority.len()])
    }

    /// The address of the session socket on this endpoint.
    #[must_use]
    pub fn session_url(&self) -> Option<String> {
        self.origin().map(|origin| {
            let secure = origin
                .strip_prefix("https://")
                .map_or_else(|| origin.to_owned(), |rest| format!("wss://{rest}"));
            format!("{secure}{SESSION_PATH}")
        })
    }

    /// The address under which this endpoint serves content and renewal
    /// over HTTPS: the same host and port as the session.
    #[must_use]
    pub fn https_base(&self) -> Option<String> {
        let origin = self.origin()?;
        let (_scheme, rest) = origin.split_once("://")?;
        Some(format!("https://{rest}{PATH_PREFIX}"))
    }
}

/// The order in which a machine tries its endpoints: the one that
/// worked last first, then the others as enrollment gave them.
#[must_use]
pub fn endpoint_order<'a>(
    endpoints: &'a [Endpoint],
    last_worked: Option<&str>,
) -> Vec<&'a Endpoint> {
    let worked = |endpoint: &&Endpoint| Some(endpoint.url.as_str()) == last_worked;
    endpoints
        .iter()
        .filter(worked)
        .chain(endpoints.iter().filter(|endpoint| !worked(endpoint)))
        .collect()
}

/// A machine's request to be enrolled, sent without a certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnrollRequest {
    /// The retained machine when this data directory enrolls again; the token must name the same machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<Id>,
    /// The enrollment token.
    pub token: String,
    /// The request for a certificate. Whatever name it asks for is
    /// ignored: the system names the machine.
    pub csr_pem: String,
    /// What the machine is to be called.
    pub name: String,
    /// What it is built for.
    pub platform: Platform,
    /// The connector's own version.
    pub connector_version: String,
    /// The protocol versions it speaks.
    pub protocol_versions: Vec<u32>,
    /// The roles it has switched on.
    pub roles: Vec<Role>,
}

/// The system a machine was enrolled into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnrollReplySystem {
    /// Its name.
    pub name: String,
    /// The address people reach it at.
    pub public_url: String,
}

/// The system's answer to an enrollment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnrollReply {
    /// The machine's identifier from now on.
    pub machine_id: Id,
    /// Its certificate.
    pub certificate_pem: String,
    /// The certificate of the system's own authority.
    pub ca_pem: String,
    /// The addresses it may hold its session with, in the order to try
    /// them.
    pub endpoints: Vec<Endpoint>,
    /// The system it now belongs to.
    pub system: EnrollReplySystem,
    /// The protocol version the two will speak.
    pub protocol_version: u32,
}

/// A machine's request for a new certificate, over a new key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenewRequest {
    /// The request for a certificate.
    pub csr_pem: String,
}

/// The system's answer to a renewal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenewReply {
    /// The new certificate.
    pub certificate_pem: String,
    /// The certificate of the system's own authority.
    pub ca_pem: String,
}

/// Why an enrollment is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum EnrollRefusal {
    /// The token is past its time, or was used.
    #[error("the enrollment token is past its time or was used")]
    Gone,
    /// The request is not what an enrollment is.
    #[error("the request is not a valid enrollment")]
    Invalid,
    /// The machine and the system have no protocol version in common.
    #[error("the machine speaks no protocol version the system speaks")]
    ProtocolUnsupported,
}

impl EnrollRefusal {
    /// The HTTP status the refusal is answered with.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::Gone => 410,
            Self::Invalid => 400,
            Self::ProtocolUnsupported => 409,
        }
    }

    /// The code the refusal is answered with.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Gone => "gone",
            Self::Invalid => "request.invalid",
            Self::ProtocolUnsupported => "protocol_unsupported",
        }
    }
}

/// A text that is not an enrollment token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("an enrollment token is `oenr_`, a secret, a dot and a pin")]
pub struct NotAToken;

/// An enrollment token, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentToken {
    secret: String,
    pin: [u8; PIN_BYTES],
}

impl EnrollmentToken {
    /// A token with `secret`, random text that the system made, and the
    /// pin of the system's authority.
    #[must_use]
    pub fn new(secret: &str, pin: [u8; PIN_BYTES]) -> Self {
        Self {
            secret: secret.to_owned(),
            pin,
        }
    }

    /// The random part, which is what the system knows the token by.
    #[must_use]
    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// The pin: the SHA-256 of the public key of the system's authority.
    #[must_use]
    pub const fn pin(&self) -> &[u8; PIN_BYTES] {
        &self.pin
    }

    /// Whether an authority whose public key hashes to `spki_sha256` is
    /// the one this token was made by.
    #[must_use]
    pub fn pins(&self, spki_sha256: &[u8; PIN_BYTES]) -> bool {
        &self.pin == spki_sha256
    }
}

impl std::fmt::Display for EnrollmentToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{TOKEN_PREFIX}{}.{}",
            self.secret,
            URL_SAFE_NO_PAD.encode(self.pin)
        )
    }
}

impl std::str::FromStr for EnrollmentToken {
    type Err = NotAToken;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (secret, pin) = text
            .strip_prefix(TOKEN_PREFIX)
            .and_then(|rest| rest.rsplit_once('.'))
            .ok_or(NotAToken)?;
        if secret.is_empty() {
            return Err(NotAToken);
        }
        let pin = URL_SAFE_NO_PAD
            .decode(pin)
            .ok()
            .and_then(|pin| <[u8; PIN_BYTES]>::try_from(pin).ok());
        Ok(Self {
            secret: secret.to_owned(),
            pin: pin.ok_or(NotAToken)?,
        })
    }
}

/// Whether a certificate that is valid from `not_before` to `not_after`
/// is due to be renewed at `now`: two thirds of its time have passed, so
/// that a third is left in which a renewal that fails can be tried
/// again.
#[must_use]
pub fn renewal_due(not_before: Time, not_after: Time, now: Time) -> bool {
    // Two thirds, without fractions: elapsed / whole >= 2 / 3.
    const PARTS_PASSED: i128 = 2;
    const PARTS_IN_ALL: i128 = 3;
    let whole = i128::from(not_after.unix_millis() - not_before.unix_millis());
    let elapsed = i128::from(now.unix_millis() - not_before.unix_millis());
    elapsed * PARTS_IN_ALL >= whole * PARTS_PASSED
}

/// Whether a machine's certificate that ran out at `not_after` is still
/// taken for a renewal at `now`, `grace_days` being how long that is
/// allowed.
#[must_use]
pub fn renewable_after_expiry(not_after: Time, now: Time, grace_days: i64) -> bool {
    const DAY_MS: i64 = 86_400_000;
    now.unix_millis() - not_after.unix_millis() <= grace_days.saturating_mul(DAY_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn time(text: &str) -> Time {
        text.parse().unwrap()
    }

    fn endpoints() -> Vec<Endpoint> {
        vec![
            Endpoint {
                url: "wss://gw.orch.internal:9443".into(),
                trust: Trust::Pinned,
            },
            Endpoint {
                url: "wss://orch.example.com/link".into(),
                trust: Trust::Public,
            },
        ]
    }

    #[test]
    fn an_enrollment_reads_and_writes_as_the_protocol_has_it() {
        let token = EnrollmentToken::new("c2VjcmV0", [7; PIN_BYTES]).to_string();
        let request = json!({
            "token": token, "csr_pem": "-----BEGIN CERTIFICATE REQUEST-----…",
            "name": "alice-laptop", "platform": { "os": "linux", "arch": "x86_64" },
            "connector_version": "0.1.0", "protocol_versions": [1], "roles": ["agent", "environment"]
        });
        let read: EnrollRequest = serde_json::from_value(request.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), request);

        let reply = json!({
            "machine_id": "0199c1a0-0000-7000-8000-00000000000a", "certificate_pem": "…", "ca_pem": "…",
            "endpoints": [ { "url": "wss://gw.orch.internal:9443", "trust": "pinned" },
                           { "url": "wss://orch.example.com/link", "trust": "public" } ],
            "system": { "name": "Orch", "public_url": "https://orch.example.com" },
            "protocol_version": 1
        });
        let read: EnrollReply = serde_json::from_value(reply.clone()).unwrap();
        assert_eq!(read.endpoints, endpoints());
        assert_eq!(serde_json::to_value(&read).unwrap(), reply);
    }

    #[test]
    fn a_renewal_reads_and_writes_as_the_protocol_has_it() {
        let request: RenewRequest = serde_json::from_value(json!({ "csr_pem": "…" })).unwrap();
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({ "csr_pem": "…" })
        );
        let reply = json!({ "certificate_pem": "…", "ca_pem": "…" });
        let read: RenewReply = serde_json::from_value(reply.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), reply);
        assert!(
            serde_json::from_value::<RenewRequest>(json!({ "csr_pem": "…", "name": "x" })).is_err()
        );
    }

    #[test]
    fn a_token_carries_a_secret_and_the_pin_of_its_system() {
        let token = EnrollmentToken::new("c2VjcmV0", [7; PIN_BYTES]);
        let text = token.to_string();
        assert!(text.starts_with("oenr_c2VjcmV0."));
        let read: EnrollmentToken = text.parse().unwrap();
        assert_eq!(read, token);
        assert!(read.pins(&[7; PIN_BYTES]));
        assert!(!read.pins(&[8; PIN_BYTES]));
    }

    #[test]
    fn a_text_that_is_no_token_is_refused() {
        let good = EnrollmentToken::new("c2VjcmV0", [7; PIN_BYTES]).to_string();
        let no_prefix = good.replacen("oenr_", "", 1);
        let short_pin = format!("oenr_c2VjcmV0.{}", URL_SAFE_NO_PAD.encode([7u8; 16]));
        for text in [
            "",
            "oenr_",
            "oenr_c2VjcmV0",
            "oenr_.AAAA",
            &no_prefix,
            &short_pin,
        ] {
            assert_eq!(text.parse::<EnrollmentToken>(), Err(NotAToken), "{text}");
        }
    }

    #[test]
    fn each_refusal_has_its_status_and_its_code() {
        assert_eq!(
            (EnrollRefusal::Gone.status(), EnrollRefusal::Gone.code()),
            (410, "gone")
        );
        assert_eq!(
            (
                EnrollRefusal::Invalid.status(),
                EnrollRefusal::Invalid.code()
            ),
            (400, "request.invalid")
        );
        let unsupported = EnrollRefusal::ProtocolUnsupported;
        assert_eq!(
            (unsupported.status(), unsupported.code()),
            (409, "protocol_unsupported")
        );
    }

    #[test]
    fn the_session_and_the_content_are_on_the_host_and_port_of_the_endpoint() {
        let [pinned, public] = <[Endpoint; 2]>::try_from(endpoints()).unwrap();
        assert_eq!(
            pinned.session_url().unwrap(),
            "wss://gw.orch.internal:9443/link/v1/session"
        );
        assert_eq!(
            public.session_url().unwrap(),
            "wss://orch.example.com/link/v1/session"
        );
        assert_eq!(
            pinned.https_base().unwrap(),
            "https://gw.orch.internal:9443/link/v1"
        );
        assert_eq!(
            public.https_base().unwrap(),
            "https://orch.example.com/link/v1"
        );
        let broken = Endpoint {
            url: "orch.example.com".into(),
            trust: Trust::Public,
        };
        assert_eq!(broken.session_url(), None);
    }

    #[test]
    fn a_machine_tries_the_endpoint_that_worked_last_first() {
        let endpoints = endpoints();
        let urls = |order: Vec<&Endpoint>| {
            order
                .into_iter()
                .map(|endpoint| endpoint.url.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            urls(endpoint_order(&endpoints, None)),
            ["wss://gw.orch.internal:9443", "wss://orch.example.com/link"]
        );
        assert_eq!(
            urls(endpoint_order(
                &endpoints,
                Some("wss://orch.example.com/link")
            )),
            ["wss://orch.example.com/link", "wss://gw.orch.internal:9443"]
        );
        assert_eq!(
            urls(endpoint_order(&endpoints, Some("wss://gone.example.com"))).len(),
            2
        );
    }

    #[test]
    fn a_certificate_is_renewed_when_two_thirds_of_its_time_have_passed() {
        let from = time("2026-01-01T00:00:00.000Z");
        let to = time("2026-12-27T00:00:00.000Z"); // 360 days
        assert!(!renewal_due(from, to, time("2026-08-28T23:59:59.999Z")));
        assert!(renewal_due(from, to, time("2026-08-29T00:00:00.000Z")));
        assert!(renewal_due(from, to, time("2027-03-01T00:00:00.000Z")));
    }

    #[test]
    fn a_certificate_that_ran_out_is_still_renewed_for_ninety_days_and_not_after() {
        let ran_out = time("2026-01-01T00:00:00.000Z");
        assert!(renewable_after_expiry(
            ran_out,
            time("2026-04-01T00:00:00.000Z"),
            EXPIRED_RENEW_GRACE_DAYS
        ));
        assert!(!renewable_after_expiry(
            ran_out,
            time("2026-04-01T00:00:00.001Z"),
            EXPIRED_RENEW_GRACE_DAYS
        ));
        assert!(renewable_after_expiry(
            ran_out,
            time("2025-12-01T00:00:00.000Z"),
            EXPIRED_RENEW_GRACE_DAYS
        ));
    }
}
