//! The suite every wire protocol passes: allowed requests, the model's place, the key as an endpoint says, usage and errors.
//!
//! Fixtures describe the protocol's own request and usage shapes. The suite never calls a paid upstream.

use http::{HeaderMap, Method, StatusCode};
use orch_model_contract::credentials::Credential;
use orch_model_contract::endpoint::EndpointOptions;
use orch_model_contract::protocol::{SecretHeaders, WireProtocol};
use orch_model_contract::usage::ObservedUsage;

use crate::report::Report;

/// A representative allowed request and response in an implementation's own format.
pub struct Fixture<'a> {
    /// The request method the format permits.
    pub method: Method,
    /// Its path below the endpoint base.
    pub path: &'a str,
    /// Its body, naming the model below.
    pub body: &'a [u8],
    /// The exact routing name in the body.
    pub model: &'a str,
    /// A complete response that reports usage, including its stream framing.
    pub response: &'a [u8],
    /// The exact usage the response reports.
    pub usage: ObservedUsage,
}

/// Checks allowlisting, model extraction, error shape, credential options and chunk-independent usage.
pub fn run(protocol: &dyn WireProtocol, fixture: &Fixture<'_>, report: &mut Report) {
    report.checked(
        "protocol.allowed",
        "Its request passes and an unrelated operation is refused",
        {
            if protocol.allowed(&fixture.method, fixture.path)
                && !protocol.allowed(&Method::DELETE, "/conformance/not-a-model-operation")
            {
                Ok(())
            } else {
                Err(
                    "the protocol accepted an unrelated operation or refused its own request"
                        .into(),
                )
            }
        },
    );
    report.checked(
        "protocol.model",
        "The routing model is read exactly from the request",
        {
            if protocol.model_of(fixture.path, fixture.body).as_deref() == Some(fixture.model)
                && protocol.model_of(fixture.path, b"not JSON").is_none()
            {
                Ok(())
            } else {
                Err("the extracted model differs from the requested routing name".into())
            }
        },
    );
    report.checked(
        "protocol.error",
        "A refused request has its status and a readable structured error",
        {
            let denied = protocol.deny(
                StatusCode::FORBIDDEN,
                "conformance_denied",
                "The request is forbidden",
            );
            let body: Result<serde_json::Value, _> = serde_json::from_slice(denied.body());
            if denied.status() == StatusCode::FORBIDDEN
                && body.is_ok()
                && String::from_utf8_lossy(denied.body()).contains("The request is forbidden")
            {
                Ok(())
            } else {
                Err("the refusal lost its status, error text or JSON shape".into())
            }
        },
    );
    report.checked(
        "protocol.authorization",
        "Endpoint options replace the harness credential",
        {
            let checked = || {
                let options: EndpointOptions =
                    serde_json::from_value(serde_json::json!({"auth":"bearer"}))
                        .map_err(|error| error.to_string())?;
                let mut headers = HeaderMap::new();
                headers.insert(
                    http::header::AUTHORIZATION,
                    http::HeaderValue::from_static("Bearer harness-key"),
                );
                protocol
                    .authorize(
                        &mut headers,
                        Some(&Credential::new("conformance-key")),
                        &options,
                        &SecretHeaders::new(),
                    )
                    .map_err(|error| error.to_string())?;
                if headers
                    .get(http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    != Some("Bearer conformance-key")
                {
                    return Err(
                        "the upstream credential did not replace the harness credential".to_owned(),
                    );
                }
                Ok(())
            };
            checked().map_err(Into::into)
        },
    );
    report.checked(
        "protocol.usage",
        "Usage survives every byte boundary of a streamed response",
        {
            let mut observer = protocol.usage_observer();
            observer.observe(fixture.response);
            let whole = observer.usage();
            let mut fragmented = protocol.usage_observer();
            for byte in fixture.response {
                fragmented.observe(std::slice::from_ref(byte));
            }
            if whole.as_ref() == Some(&fixture.usage) && fragmented.usage() == whole {
                Ok(())
            } else {
                Err("whole and fragmented usage differ from the response's actual counts".into())
            }
        },
    );
}
