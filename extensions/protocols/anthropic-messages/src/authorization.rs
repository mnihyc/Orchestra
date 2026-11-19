//! The key presented as the endpoint's options say, and shaped denials.
//!
//! The vendor of the Messages format documents `Authorization: Bearer <key>` as the header that carries a key
//! and `x-api-key: <key>` as a legacy fallback that is still supported (its API overview, read in October
//! 2026), so an endpoint whose options say nothing takes a bearer ([`DEFAULT_AUTH`]). Another vendor's
//! endpoint in the same format may document `x-api-key` as its one way, and its options then say `x-api-key`.
//! The request's `anthropic-version` and `anthropic-beta` headers pass on untouched. A request made by the
//! connector itself, such as model discovery, supplies the stable API version when none was supplied.
//! The rule that removes what the harness presented and adds the key, the fixed headers, the secret headers and
//! the query is the model contract's ([`orch_model_contract::endpoint::authorize`]), the same for every
//! protocol. What this module adds is the trace of what was done, which names headers and never shows a
//! value, and the shape of a refusal.
//!
//! A refusal is the vendor's own error document, so that a harness's client reads it like any error of the
//! upstream: `{"type": "error", "error": {"type", "message"}}`. The `type` follows the status as the vendor
//! documents (`invalid_request_error`, `authentication_error`, `permission_error`, `not_found_error`,
//! `request_too_large`, `rate_limit_error`, `api_error`, `overloaded_error`, and so on). The vendor's object
//! is allowed to grow, so the machine-readable `code` the forwarder gives (`model_not_allowed`,
//! `lease_closed`, `cap_reached`) is one more member of it.

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderValue, Response, StatusCode};
use orch_model_contract::credentials::Credential;
use orch_model_contract::endpoint::{AuthMode, AuthorizeError, EndpointOptions, authorize};
use orch_model_contract::protocol::SecretHeaders;
use serde_json::json;

/// How the key is presented when an endpoint's options say nothing about it.
pub const DEFAULT_AUTH: AuthMode = AuthMode::Bearer;

/// The stable Messages API version, also used by connector-originated discovery without a harness header.
const DEFAULT_VERSION: &str = "2023-06-01";

/// The status the vendor answers with while it is overloaded; the registry of HTTP statuses has no name for it.
const OVERLOADED_STATUS: u16 = 529;

/// The media type of a refusal.
const JSON: HeaderValue = HeaderValue::from_static("application/json");

/// Presents the credential on `headers` the way `options` say, with the headers and the query they
/// name, and returns the query parameters to append. See [`orch_model_contract::endpoint::authorize`].
///
/// Only the names of the headers that result are traced, at the finest level; no value is.
pub fn present(
    headers: &mut HeaderMap,
    credential: Option<&Credential>,
    options: &EndpointOptions,
    secrets: &SecretHeaders,
) -> Result<Vec<(String, String)>, AuthorizeError> {
    headers
        .entry("anthropic-version")
        .or_insert(HeaderValue::from_static(DEFAULT_VERSION));
    let query = authorize(headers, credential, options, secrets, DEFAULT_AUTH)?;
    tracing::trace!(
        protocol = orch_model_contract::protocol::ANTHROPIC_MESSAGES,
        auth = options.auth.unwrap_or(DEFAULT_AUTH).as_str(),
        headers = ?headers.keys().map(http::HeaderName::as_str).collect::<Vec<_>>(),
        query = ?query.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        "credential presented"
    );
    Ok(query)
}

/// The type the vendor names an error with, given its status.
fn error_type(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "invalid_request_error",
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::PAYMENT_REQUIRED => "billing_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::CONFLICT => "conflict_error",
        StatusCode::PAYLOAD_TOO_LARGE => "request_too_large",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        StatusCode::GATEWAY_TIMEOUT => "timeout_error",
        status if status.as_u16() == OVERLOADED_STATUS => "overloaded_error",
        status if status.is_server_error() => "api_error",
        _ => "invalid_request_error",
    }
}

/// An error in the vendor's shape, for a request the forwarder refuses.
#[must_use]
pub fn deny(status: StatusCode, code: &str, message: &str) -> Response<Bytes> {
    let body = json!({
        "type": "error",
        "error": { "type": error_type(status), "message": message, "code": code }
    });
    let mut response = Response::new(Bytes::from(body.to_string()));
    *response.status_mut() = status;
    response.headers_mut().insert(CONTENT_TYPE, JSON);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_plugin_contract::Secret;
    use serde_json::Value;

    fn options(value: &Value) -> EndpointOptions {
        EndpointOptions::parse(value).unwrap()
    }

    fn body(response: &Response<Bytes>) -> Value {
        serde_json::from_slice(response.body()).unwrap()
    }

    #[test]
    fn the_key_goes_as_a_bearer_unless_the_options_say_otherwise() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer harness"));
        headers.insert("x-api-key", HeaderValue::from_static("harness"));
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        present(
            &mut headers,
            Some(&Credential::new("sk-real")),
            &EndpointOptions::default(),
            &SecretHeaders::new(),
        )
        .unwrap();
        assert_eq!(headers["authorization"], "Bearer sk-real");
        assert!(headers.get("x-api-key").is_none());
        assert_eq!(
            headers["anthropic-version"], "2023-06-01",
            "the harness's own version header passes on"
        );
    }

    #[test]
    fn a_vendor_that_documents_x_api_key_gets_the_key_there_and_not_as_a_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer harness"));
        present(
            &mut headers,
            Some(&Credential::new("sk-real")),
            &options(&json!({ "auth": "x-api-key" })),
            &SecretHeaders::new(),
        )
        .unwrap();
        assert_eq!(headers["x-api-key"], "sk-real");
        assert!(headers.get("authorization").is_none());
    }

    #[test]
    fn discovery_has_a_version_without_a_harness_and_an_endpoints_explicit_version_wins() {
        let mut headers = HeaderMap::new();
        present(
            &mut headers,
            None,
            &options(&json!({"auth":"none"})),
            &SecretHeaders::new(),
        )
        .unwrap();
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static("harness-version"),
        );
        present(
            &mut headers,
            None,
            &options(&json!({"auth":"none"})),
            &SecretHeaders::new(),
        )
        .unwrap();
        assert_eq!(headers["anthropic-version"], "harness-version");
        present(
            &mut headers,
            None,
            &options(&json!({"auth":"none","headers":{"anthropic-version":"endpoint-version"}})),
            &SecretHeaders::new(),
        )
        .unwrap();
        assert_eq!(headers["anthropic-version"], "endpoint-version");
    }

    #[test]
    fn a_key_in_a_header_of_the_endpoints_own_naming_is_sent_there_alone() {
        let mut headers = HeaderMap::new();
        present(
            &mut headers,
            Some(&Credential::new("sk-real")),
            &options(&json!({ "auth": "header", "auth_header": "api-key" })),
            &SecretHeaders::new(),
        )
        .unwrap();
        assert_eq!(headers["api-key"], "sk-real");
        assert_eq!(headers.len(), 2);
        assert_eq!(headers["anthropic-version"], "2023-06-01");
    }

    #[test]
    fn a_secret_header_is_sent_with_its_value_and_a_missing_one_stops_the_request() {
        let wants = options(&json!({ "secret_headers": ["x-gateway-key"] }));
        let mut secrets = SecretHeaders::new();
        secrets.insert("x-gateway-key".into(), Secret::new("gw-secret"));
        let mut headers = HeaderMap::new();
        present(&mut headers, Some(&Credential::new("k")), &wants, &secrets).unwrap();
        assert_eq!(headers["x-gateway-key"], "gw-secret");
        assert_eq!(
            present(
                &mut HeaderMap::new(),
                Some(&Credential::new("k")),
                &wants,
                &SecretHeaders::new()
            ),
            Err(AuthorizeError::SecretMissing("x-gateway-key".into()))
        );
    }

    #[test]
    fn a_refusal_is_the_vendors_error_document() {
        let refusal = deny(
            StatusCode::FORBIDDEN,
            "model_not_allowed",
            "claude-opus-5 is not on this lease",
        );
        assert_eq!(refusal.status(), StatusCode::FORBIDDEN);
        assert_eq!(refusal.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(
            body(&refusal),
            json!({ "type": "error", "error": {
                "type": "permission_error", "message": "claude-opus-5 is not on this lease",
                "code": "model_not_allowed"
            } })
        );
    }

    #[test]
    fn a_refusal_names_its_type_as_the_vendor_documents_for_the_status() {
        let kind = |status: u16| {
            let status = StatusCode::from_u16(status).unwrap();
            body(&deny(status, "c", "m"))["error"]["type"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        for (status, expected) in [
            (400, "invalid_request_error"),
            (401, "authentication_error"),
            (403, "permission_error"),
            (404, "not_found_error"),
            (413, "request_too_large"),
            (429, "rate_limit_error"),
            (500, "api_error"),
            (504, "timeout_error"),
            (529, "overloaded_error"),
            (422, "invalid_request_error"),
            (502, "api_error"),
        ] {
            assert_eq!(kind(status), expected, "{status}");
        }
    }

    #[test]
    fn a_message_with_quotes_and_newlines_stays_one_valid_document() {
        let refusal = deny(StatusCode::FORBIDDEN, "x", "say \"no\"\nthen \\ stop");
        assert_eq!(
            body(&refusal)["error"]["message"],
            "say \"no\"\nthen \\ stop"
        );
    }
}
