//! Small gateway-local probes that never authenticate a machine or contact the API.

use crate::{
    Gateway,
    listeners::{Body, full, host_of, problem},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http::{Method, Request, Response, StatusCode, header};
use hyper::body::Incoming;
use orch_link_contract::gateways::{GatewayProbeReply, PROBE_NONCE_HEADER, probe_signing_input};

pub(crate) fn reply(gateway: &Gateway, request: &Request<Incoming>) -> Response<Body> {
    if request.method() != Method::GET {
        return problem(
            StatusCode::METHOD_NOT_ALLOWED,
            "request.invalid",
            "a gateway probe is a GET",
        );
    }
    let Some(nonce) = request
        .headers()
        .get(PROBE_NONCE_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|nonce| {
            nonce.len() == 43
                && URL_SAFE_NO_PAD
                    .decode(nonce)
                    .is_ok_and(|bytes| bytes.len() == 32)
        })
    else {
        return problem(
            StatusCode::BAD_REQUEST,
            "request.invalid",
            "a gateway probe needs a fresh 32-byte nonce",
        );
    };
    let Some(host) = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return problem(
            StatusCode::BAD_REQUEST,
            "request.invalid",
            "a gateway probe needs its host",
        );
    };
    let input = probe_signing_input(nonce, &gateway.inner.settings.gateway_id, &host_of(host));
    let Ok((certificate, signature)) = gateway.inner.identity.proof(input.as_bytes()) else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "source.offline",
            "the gateway's current identity is unavailable",
        );
    };
    let reply = GatewayProbeReply {
        gateway_id: gateway.inner.settings.gateway_id.clone(),
        nonce: nonce.to_owned(),
        certificate_pem: certificate.pem(),
        signature: URL_SAFE_NO_PAD.encode(signature),
    };
    let Ok(body) = serde_json::to_vec(&reply) else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "source.offline",
            "the gateway probe could not be written",
        );
    };
    let mut response = Response::new(full(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    response
}
