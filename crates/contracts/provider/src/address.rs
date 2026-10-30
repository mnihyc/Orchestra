//! The address binding: a program's methods and JSON over HTTP; its envelope, its authorization, and how its reply is read.
//!
//! A source at an address is asked with `POST <address>` and the body `{"contract": 1, "method": "facts.read",
//! "params": {…}}` ([`AddressRequest`]), with `Content-Type` and `Accept` `application/json` and the
//! connector's `User-Agent`. It answers `200` with `{"result": …}` or `{"error": {"code", "message",
//! "retryable"}}` ([`AddressReply`]); any other status is an error, and a redirection is never followed
//! ([`read_reply`]). Its authorization is one of an endpoint's: none, a bearer token, or a named header with a
//! secret value ([`AddressAuth`]); the token goes to that address alone. The provider's key goes to it only
//! where the provider's managers turned that on for it (`send_credential`), and never a sign-in.
//!
//! `https://` always, `http://` only where `providers.allow_http_upstreams` allows it, by the same rule as the
//! endpoints the forwarder calls ([`SourceAddress::check`]). An address is asked from the machine that keeps
//! the credential, within that machine's network policy and, on a person's machine, its `provider.upstreams`.

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::limits;
use crate::source::{SourceError, SourceErrorCode};
use crate::wire;

/// The product name of the connector's `User-Agent` on a request to an address.
pub const USER_AGENT_PRODUCT: &str = "orch-connector";
/// The media type of a request to an address and of its reply.
pub const MEDIA_TYPE: &str = "application/json";
/// Headers the connector writes itself, which an address's authorization may not name.
const RESERVED_HEADERS: [&str; 8] = [
    "host",
    "content-length",
    "content-type",
    "accept",
    "user-agent",
    "connection",
    "transfer-encoding",
    "cookie",
];
/// The status of an answer.
const OK: u16 = 200;
/// The statuses of a refused authorization.
const REFUSED: [u16; 2] = [401, 403];
/// The status of a source that asks to be asked later.
const TOO_MANY: u16 = 429;

/// The `User-Agent` of a request to an address from a connector of `version`.
#[must_use]
pub fn user_agent(version: &str) -> String {
    format!("{USER_AGENT_PRODUCT}/{version}")
}

/// Checks an address an endpoint or a source is reached at: it is `https`, or `http` where the resolved policy
/// `providers.allow_http_upstreams` allows it (and what is sent travels in clear), and it names a host. The one
/// rule for both; what is wrong is said in a sentence.
pub fn check_url(base_url: &str, allow_http: bool) -> Result<Url, String> {
    let url = Url::parse(base_url).map_err(|error| error.to_string())?;
    match url.scheme() {
        "https" => {}
        "http" if allow_http => {}
        "http" => {
            return Err(
                "is http, which the policy does not allow: the key would travel in clear"
                    .to_owned(),
            );
        }
        other => return Err(format!("{other}: is neither https nor http")),
    }
    if url.host_str().is_none() {
        return Err("names no host".to_owned());
    }
    Ok(url)
}

/// How a request to an address is authorized: as an endpoint's key is presented.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AddressAuth {
    /// It is not.
    #[default]
    None,
    /// `Authorization: Bearer <token>`.
    Bearer,
    /// The token as the value of the header named in `header`.
    Header,
}

/// An address a source is reached at, with how it is authorized and whether the provider's key may be sent
/// to it. The authorization itself is a provider secret, never part of it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct SourceAddress {
    /// The address requests are posted to.
    pub url: String,
    /// How requests to it are authorized.
    #[serde(default)]
    pub auth: AddressAuth,
    /// For `auth` `header`, the header's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// Whether the provider's key is sent to it, as `params.credential.key`: only where its managers turned
    /// that on for this address, and only for a key.
    #[serde(default)]
    pub send_credential: bool,
}

/// Why an address cannot be kept.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddressFault {
    /// It is no address, names no host, or has a scheme the policy does not allow.
    #[error("the address {0}")]
    Url(String),
    /// It holds a user name or a password, which would be shown wherever it is.
    #[error("the address holds a user name or a password: give its authorization instead")]
    Credentials,
    /// `auth` is `header` and no header is named.
    #[error("auth is \"header\" and no header is named")]
    NoHeader,
    /// A header is named and `auth` is not `header`.
    #[error("a header is named only where auth is \"header\"")]
    HeaderUnused,
    /// The named header cannot carry an authorization.
    #[error("{0:?} cannot carry an authorization")]
    BadHeader(String),
}

/// Whether `name` is a header's name (a token of HTTP) that the connector does not write itself.
fn usable_header(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c))
        && !RESERVED_HEADERS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

impl SourceAddress {
    /// Checks the address against the scheme rule of the endpoints (`allow_http` is what
    /// `providers.allow_http_upstreams` resolves to), and that its authorization is whole: a header named
    /// where, and only where, `auth` is `header`.
    pub fn check(&self, allow_http: bool) -> Result<Url, AddressFault> {
        let url = check_url(&self.url, allow_http).map_err(AddressFault::Url)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(AddressFault::Credentials);
        }
        match (self.auth, self.header.as_deref()) {
            (AddressAuth::Header, None) => Err(AddressFault::NoHeader),
            (AddressAuth::Header, Some(name)) if !usable_header(name) => {
                Err(AddressFault::BadHeader(name.to_owned()))
            }
            (AddressAuth::None | AddressAuth::Bearer, Some(_)) => Err(AddressFault::HeaderUnused),
            _ => Ok(url),
        }
    }

    /// Whether requests to it travel unencrypted (`http://`): the page marks it so.
    #[must_use]
    pub fn unencrypted(&self) -> bool {
        Url::parse(&self.url).is_ok_and(|url| url.scheme() == "http")
    }

    /// The host it names, as an audit row and a confirmation name it.
    #[must_use]
    pub fn host(&self) -> Option<String> {
        Url::parse(&self.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
    }

    /// The header that authorizes a request with `token`: its name and its value. `None` where the address is
    /// not authorized.
    #[must_use]
    pub fn authorization(&self, token: &str) -> Option<(String, String)> {
        match self.auth {
            AddressAuth::None => None,
            AddressAuth::Bearer => Some(("Authorization".to_owned(), format!("Bearer {token}"))),
            AddressAuth::Header => self
                .header
                .as_ref()
                .map(|name| (name.clone(), token.to_owned())),
        }
    }
}

/// The body of a request to an address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AddressRequest {
    /// The version of the contract the request is written in.
    pub contract: u32,
    /// The method ([`wire`]).
    pub method: String,
    /// Its parameters: `{}` for `describe`, a `SourceAsk` (with `reset_id` and `idempotency_key` for
    /// `resets.spend`), or a `PresentAsk`.
    #[serde(default)]
    pub params: Json,
}

impl AddressRequest {
    /// A request of `method` with `params`, in this contract's version.
    #[must_use]
    pub fn new(method: &str, params: Json) -> Self {
        Self {
            contract: wire::CONTRACT,
            method: method.to_owned(),
            params,
        }
    }
}

/// The body of an address's answer: its result, or its error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AddressReply {
    /// What the method answers.
    Result(Json),
    /// Why it did not.
    Error(SourceError),
}

/// Reads an address's reply of HTTP `status` with `body`: its result, or the error it is. A redirection is
/// `redirected` (never followed); 401 and 403 are `unauthorized`; 429 and 5xx are `unavailable`; any other
/// status, or a body that is not the envelope, is `malformed`; a body over [`limits::REPORT_MAX_BYTES`] is
/// `too_large`; an error the address wrote is that error.
pub fn read_reply(status: u16, body: &[u8]) -> Result<Json, SourceError> {
    use SourceErrorCode as Code;
    if body.len() > limits::REPORT_MAX_BYTES {
        return Err(SourceError::new(
            Code::TooLarge,
            format!("the reply is over {} bytes", limits::REPORT_MAX_BYTES),
        ));
    }
    match status {
        OK => {}
        300..=399 => {
            return Err(SourceError::new(
                Code::Redirected,
                format!("it answered HTTP {status}: give its final address"),
            ));
        }
        status if REFUSED.contains(&status) => {
            return Err(SourceError::new(
                Code::Unauthorized,
                format!("it refused its authorization (HTTP {status})"),
            ));
        }
        status if status == TOO_MANY || (500..=599).contains(&status) => {
            return Err(SourceError::new(
                Code::Unavailable,
                format!("it answered HTTP {status}"),
            ));
        }
        status => {
            return Err(SourceError::new(
                Code::Malformed,
                format!("it answered HTTP {status}, not 200"),
            ));
        }
    }
    match serde_json::from_slice::<AddressReply>(body) {
        Ok(AddressReply::Result(value)) => Ok(value),
        Ok(AddressReply::Error(error)) => Err(error),
        Err(_) => Err(SourceError::new(
            Code::Malformed,
            "the reply is neither {\"result\": …} nor {\"error\": …}",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn address(url: &str, auth: AddressAuth, header: Option<&str>) -> SourceAddress {
        SourceAddress {
            url: url.to_owned(),
            auth,
            header: header.map(str::to_owned),
            send_credential: false,
        }
    }

    #[test]
    fn a_request_is_written_in_the_envelope_with_the_contract_and_the_method() {
        let request =
            AddressRequest::new(wire::FACTS_READ, json!({ "provider": { "name": "Acme" } }));
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({ "contract": 1, "method": "facts.read", "params": { "provider": { "name": "Acme" } } })
        );
        assert_eq!(user_agent("0.1.0"), "orch-connector/0.1.0");
    }

    #[test]
    fn a_reply_is_its_result_or_its_error_and_any_other_status_an_error() {
        let ok = read_reply(200, br#"{"result": {"plan": "pro"}}"#).unwrap();
        assert_eq!(ok, json!({ "plan": "pro" }));
        let told = read_reply(
            200,
            br#"{"error": {"code": "unsupported", "message": "no models here", "retryable": false}}"#,
        )
        .unwrap_err();
        assert_eq!(told.code, SourceErrorCode::Unsupported);
        assert_eq!(told.message, "no models here");
        let code = |status: u16| read_reply(status, b"{}").unwrap_err().code;
        assert_eq!(code(301), SourceErrorCode::Redirected);
        assert_eq!(code(308), SourceErrorCode::Redirected);
        assert_eq!(code(401), SourceErrorCode::Unauthorized);
        assert_eq!(code(403), SourceErrorCode::Unauthorized);
        assert_eq!(code(429), SourceErrorCode::Unavailable);
        assert_eq!(code(503), SourceErrorCode::Unavailable);
        assert_eq!(code(404), SourceErrorCode::Malformed);
        assert_eq!(code(204), SourceErrorCode::Malformed);
        assert!(read_reply(503, b"").unwrap_err().retryable);
        assert_eq!(
            read_reply(200, b"<html>").unwrap_err().code,
            SourceErrorCode::Malformed
        );
        assert_eq!(
            read_reply(200, br#"{"result": 1, "error": {}}"#)
                .unwrap_err()
                .code,
            SourceErrorCode::Malformed
        );
        let large = vec![b' '; limits::REPORT_MAX_BYTES + 1];
        assert_eq!(
            read_reply(200, &large).unwrap_err().code,
            SourceErrorCode::TooLarge
        );
    }

    #[test]
    fn an_address_is_https_unless_the_policy_allows_http_and_is_marked_unencrypted() {
        let secure = address("https://plan.acme.example/orch", AddressAuth::Bearer, None);
        assert!(secure.check(false).is_ok());
        assert!(!secure.unencrypted());
        assert_eq!(secure.host().as_deref(), Some("plan.acme.example"));
        let plain = address("http://10.0.0.7:8080/facts", AddressAuth::None, None);
        assert!(matches!(plain.check(false), Err(AddressFault::Url(_))));
        assert!(plain.check(true).is_ok());
        assert!(plain.unencrypted());
        assert!(matches!(
            address("ftp://plan.acme.example", AddressAuth::None, None).check(true),
            Err(AddressFault::Url(_))
        ));
        assert_eq!(
            address("https://me:pw@plan.acme.example", AddressAuth::None, None).check(false),
            Err(AddressFault::Credentials)
        );
    }

    #[test]
    fn an_authorization_is_a_bearer_token_or_a_named_header_the_connector_does_not_write_itself() {
        let bearer = address("https://plan.acme.example", AddressAuth::Bearer, None);
        assert_eq!(
            bearer.authorization("t0k3n"),
            Some(("Authorization".to_owned(), "Bearer t0k3n".to_owned()))
        );
        let header = address(
            "https://plan.acme.example",
            AddressAuth::Header,
            Some("X-Acme-Key"),
        );
        assert!(header.check(false).is_ok());
        assert_eq!(
            header.authorization("t0k3n"),
            Some(("X-Acme-Key".to_owned(), "t0k3n".to_owned()))
        );
        assert_eq!(
            address("https://plan.acme.example", AddressAuth::None, None).authorization("t0k3n"),
            None
        );
        assert_eq!(
            address("https://plan.acme.example", AddressAuth::Header, None).check(false),
            Err(AddressFault::NoHeader)
        );
        assert_eq!(
            address(
                "https://plan.acme.example",
                AddressAuth::Header,
                Some("Cookie")
            )
            .check(false),
            Err(AddressFault::BadHeader("Cookie".into()))
        );
        assert_eq!(
            address(
                "https://plan.acme.example",
                AddressAuth::Header,
                Some("X Key")
            )
            .check(false),
            Err(AddressFault::BadHeader("X Key".into()))
        );
        assert_eq!(
            address(
                "https://plan.acme.example",
                AddressAuth::Bearer,
                Some("X-Acme-Key")
            )
            .check(false),
            Err(AddressFault::HeaderUnused)
        );
        let read: SourceAddress =
            serde_json::from_value(json!({ "url": "https://plan.acme.example" })).unwrap();
        assert_eq!(read.auth, AddressAuth::None);
        assert!(
            !read.send_credential,
            "the key is sent only where it was turned on"
        );
    }
}
