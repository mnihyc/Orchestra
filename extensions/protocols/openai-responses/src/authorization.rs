//! The key presented as the endpoint's options say, and shaped denials.
//!
//! The vendor of the Responses format documents `Authorization: Bearer <key>` as its one way of presenting a
//! key, so that is the way an endpoint takes when its options say nothing ([`DEFAULT_AUTH`]). An endpoint of
//! another vendor in the same format may want the key elsewhere, and its options say where; the rule that
//! removes what the harness presented and adds the key, the fixed headers, the secret headers and the query is
//! the model contract's ([`orch_model_contract::endpoint::authorize`]), and it is the same for every
//! protocol. What this module adds is the trace of what was done, which names headers and never shows a
//! value, and the shape of a refusal.
//!
//! A refusal is the vendor's own error document, so that a harness's client reads it like any error of the
//! upstream: `{"error": {"message", "type", "param", "code"}}`. The vendor reports a rate limit as
//! `rate_limit_error`, an overloaded model as `service_unavailable_error`, a failure of its own as
//! `server_error` and every other refusal of a request as `invalid_request_error`; the machine-readable
//! `code` is the one the forwarder gives (`model_not_allowed`, `lease_closed`, `cap_reached`).

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderValue, Response, StatusCode};
use orch_model_contract::credentials::Credential;
use orch_model_contract::endpoint::{AuthMode, AuthorizeError, EndpointOptions, authorize};
use orch_model_contract::protocol::SecretHeaders;
use serde_json::json;

/// How the key is presented when an endpoint's options say nothing about it.
pub const DEFAULT_AUTH: AuthMode = AuthMode::Bearer;

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
    let query = authorize(headers, credential, options, secrets, DEFAULT_AUTH)?;
    tracing::trace!(
        protocol = orch_model_contract::protocol::OPENAI_RESPONSES,
        auth = options.auth.unwrap_or(DEFAULT_AUTH).as_str(),
        headers = ?headers.keys().map(http::HeaderName::as_str).collect::<Vec<_>>(),
        query = ?query.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        "credential presented"
    );
    Ok(query)
}

/// The type the vendor names a refusal with, given its status.
fn error_type(status: StatusCode) -> &'static str {
    match status {
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        StatusCode::SERVICE_UNAVAILABLE => "service_unavailable_error",
        status if status.is_server_error() => "server_error",
        _ => "invalid_request_error",
    }
}

/// An error in the vendor's shape, for a request the forwarder refuses.
#[must_use]
pub fn deny(status: StatusCode, code: &str, message: &str) -> Response<Bytes> {
    let body = json!({
        "error": { "message": message, "type": error_type(status), "param": null, "code": code }
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
        present(
            &mut headers,
            Some(&Credential::new("sk-real")),
            &EndpointOptions::default(),
            &SecretHeaders::new(),
        )
        .unwrap();
        assert_eq!(headers["authorization"], "Bearer sk-real");
        assert!(headers.get("x-api-key").is_none());
    }

    #[test]
    fn a_vendor_that_wants_the_key_in_a_header_of_its_own_gets_it_there_and_nowhere_else() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer harness"));
        present(
            &mut headers,
            Some(&Credential::new("sk-real")),
            &options(&json!({ "auth": "header", "auth_header": "api-key" })),
            &SecretHeaders::new(),
        )
        .unwrap();
        assert_eq!(headers["api-key"], "sk-real");
        assert!(headers.get("authorization").is_none());
    }

    #[test]
    fn an_upstream_that_takes_no_key_is_given_none_and_the_harnesss_is_dropped() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer harness"));
        present(
            &mut headers,
            None,
            &options(&json!({ "auth": "none" })),
            &SecretHeaders::new(),
        )
        .unwrap();
        assert!(headers.is_empty());
    }

    #[test]
    fn a_secret_header_without_a_value_stops_the_request() {
        let result = present(
            &mut HeaderMap::new(),
            Some(&Credential::new("sk-real")),
            &options(&json!({ "secret_headers": ["x-gateway-key"] })),
            &SecretHeaders::new(),
        );
        assert_eq!(
            result,
            Err(AuthorizeError::SecretMissing("x-gateway-key".into()))
        );
        let mut secrets = SecretHeaders::new();
        secrets.insert("x-gateway-key".into(), Secret::new("gw"));
        assert!(
            present(
                &mut HeaderMap::new(),
                Some(&Credential::new("sk-real")),
                &options(&json!({ "secret_headers": ["x-gateway-key"] })),
                &secrets,
            )
            .is_ok()
        );
    }

    #[test]
    fn a_refusal_is_the_vendors_error_document() {
        let refusal = deny(
            StatusCode::FORBIDDEN,
            "model_not_allowed",
            "gpt-6 is not on this lease",
        );
        assert_eq!(refusal.status(), StatusCode::FORBIDDEN);
        assert_eq!(refusal.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(
            body(&refusal),
            json!({ "error": {
                "message": "gpt-6 is not on this lease", "type": "invalid_request_error",
                "param": null, "code": "model_not_allowed"
            } })
        );
    }

    #[test]
    fn a_refusal_names_its_type_as_the_vendor_does() {
        let kind = |status| {
            body(&deny(status, "c", "m"))["error"]["type"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(kind(StatusCode::NOT_FOUND), "invalid_request_error");
        assert_eq!(kind(StatusCode::TOO_MANY_REQUESTS), "rate_limit_error");
        assert_eq!(
            kind(StatusCode::SERVICE_UNAVAILABLE),
            "service_unavailable_error"
        );
        assert_eq!(kind(StatusCode::BAD_GATEWAY), "server_error");
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
