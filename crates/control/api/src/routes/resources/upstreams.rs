//! The endpoints of an API provider: the protocols there are, the options each declares and what is refused.
//!
//! An API provider is one upstream that one key opens, reached at one or more endpoints, each in a wire
//! protocol and each with the options that protocol declares: how the key is presented, headers and query
//! parameters that go with every request, and the names of headers whose values are secrets. What a protocol
//! does not declare is refused when the provider is saved, and so is a header that the forwarder sets itself.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::nodes::invalid;
use orch_api_contract::resources::Upstream;
use orch_model_contract::protocol::{
    ANTHROPIC_MESSAGES, OPENAI_CHAT, OPENAI_EMBEDDINGS, OPENAI_RESPONSES,
};
use orch_types::Json;
use serde_json::{Map, json};
use std::collections::BTreeSet;

/// The wire protocols an upstream can speak, by name. A translator that is registered adds to them;
/// none is registered here. The Embeddings format is spoken by no agent: its endpoint makes the vectors of
/// search by meaning.
pub const PROTOCOLS: [&str; 4] = [
    OPENAI_RESPONSES,
    OPENAI_CHAT,
    ANTHROPIC_MESSAGES,
    OPENAI_EMBEDDINGS,
];

/// How the key may be presented.
const AUTH_MODES: [&str; 4] = ["bearer", "x-api-key", "header", "none"];

/// The members an endpoint's options may have: what the built-in protocols declare.
const OPTION_MEMBERS: [&str; 5] = ["auth", "auth_header", "headers", "query", "secret_headers"];

/// Headers the forwarder sets itself, whatever the endpoint says: the connection's own.
const FORWARDER_HEADERS: [&str; 10] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "upgrade",
];

/// What was found out about the endpoints of a provider, with the endpoints in the form they are kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    /// The endpoints as they are stored.
    pub upstreams: Vec<Upstream>,
    /// The protocols they speak, in the order given.
    pub protocols: Vec<String>,
    /// The names of the headers whose values are secrets, over all endpoints.
    pub secret_headers: BTreeSet<String>,
    /// Whether every endpoint takes no key.
    pub keyless: bool,
    /// Whether any endpoint is reached in clear text.
    pub clear_text: bool,
}

/// Whether `name` is a header name: letters, digits and hyphens.
fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|each| each.is_ascii_alphanumeric() || each == '-')
}

/// Whether an address's host is the loopback of where its requests are made: the provider's runtime, whose
/// connector makes them. A request there crosses no network, so it needs no TLS.
fn is_loopback(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// A base URL: `https`, or `http` where the policy allows it or the host is the runtime's own loopback, with a
/// host and nothing after the path that a request could not carry (no credentials, no query, no fragment).
pub(crate) fn clean_base_url(
    path: &str,
    base_url: &str,
    http_allowed: bool,
) -> ApiResult<(String, bool)> {
    let trimmed = base_url.trim();
    let (rest, clear) = if let Some(rest) = trimmed.strip_prefix("https://") {
        (rest, false)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if is_loopback(authority) {
            // Clear text on the runtime's own loopback is no clear text on a network.
            return clean_rest(path, trimmed, rest, false);
        }
        if !http_allowed {
            return Err(invalid(
                path,
                "the address is https; http is allowed only where the policy providers.allow_http_upstreams says so",
            ));
        }
        (rest, true)
    } else {
        return Err(invalid(path, "the address is an https address"));
    };
    clean_rest(path, trimmed, rest, clear)
}

/// The rest of [`clean_base_url`]: a host, and no credentials, query or fragment.
fn clean_rest(path: &str, trimmed: &str, rest: &str, clear: bool) -> ApiResult<(String, bool)> {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let bad = authority.is_empty()
        || authority.contains('@')
        || trimmed.contains(char::is_whitespace)
        || trimmed.contains(['?', '#']);
    if bad {
        return Err(invalid(
            path,
            "the address needs a host, and has no credentials, query or fragment in it",
        ));
    }
    Ok((trimmed.trim_end_matches('/').to_owned(), clear))
}

/// A map of text to text, as `headers` and `query` are.
fn text_map(path: &str, value: &Json, header_names: bool) -> ApiResult<Map<String, Json>> {
    let Some(members) = value.as_object() else {
        return Err(invalid(path, "this is a map of text to text"));
    };
    for (name, text) in members {
        if !text.is_string() {
            return Err(invalid(&format!("{path}.{name}"), "the value is text"));
        }
        if header_names && !is_header_name(name) {
            return Err(invalid(&format!("{path}.{name}"), "this is no header name"));
        }
    }
    Ok(members.clone())
}

/// Checks the endpoints of a provider and puts them in the form they are kept: at least one, at most
/// one for each protocol, each with the options its protocol declares.
pub fn check(upstreams: &[Upstream], http_allowed: bool) -> ApiResult<Checked> {
    if upstreams.is_empty() {
        return Err(invalid(
            "upstreams",
            "an API provider has at least one endpoint",
        ));
    }
    let mut kept: Vec<Upstream> = Vec::new();
    let mut protocols: Vec<String> = Vec::new();
    let mut secret_headers: BTreeSet<String> = BTreeSet::new();
    let mut keyless = true;
    let mut clear_text = false;
    for (index, upstream) in upstreams.iter().enumerate() {
        let at = format!("upstreams[{index}]");
        if !PROTOCOLS.contains(&upstream.protocol.as_str()) {
            return Err(invalid(
                &format!("{at}.protocol"),
                &format!(
                    "this is no protocol; the protocols are {}",
                    PROTOCOLS.join(", ")
                ),
            ));
        }
        if protocols.contains(&upstream.protocol) {
            return Err(invalid(
                &format!("{at}.protocol"),
                "an upstream has one endpoint for each protocol",
            ));
        }
        let (base_url, clear) =
            clean_base_url(&format!("{at}.base_url"), &upstream.base_url, http_allowed)?;
        clear_text |= clear;
        let given = match &upstream.options {
            Json::Null => Map::new(),
            Json::Object(members) => members.clone(),
            _ => {
                return Err(invalid(
                    &format!("{at}.options"),
                    "the options are an object",
                ));
            }
        };
        if let Some(unknown) = given
            .keys()
            .find(|name| !OPTION_MEMBERS.contains(&name.as_str()))
        {
            return Err(invalid(
                &format!("{at}.options.{unknown}"),
                "this protocol declares no such option",
            ));
        }
        let mut options = Map::new();
        let auth = match given.get("auth") {
            None | Some(Json::Null) => "bearer",
            Some(Json::String(mode)) if AUTH_MODES.contains(&mode.as_str()) => mode.as_str(),
            Some(_) => {
                return Err(invalid(
                    &format!("{at}.options.auth"),
                    &format!("the key is presented as {}", AUTH_MODES.join(", ")),
                ));
            }
        };
        if let Some(mode) = given.get("auth").filter(|mode| !mode.is_null()) {
            options.insert("auth".to_owned(), mode.clone());
        }
        keyless &= auth == "none";
        let auth_header = match (auth, given.get("auth_header")) {
            ("header", Some(Json::String(name))) if is_header_name(name) => Some(name.clone()),
            ("header", _) => {
                return Err(invalid(
                    &format!("{at}.options.auth_header"),
                    "the header that carries the key needs a header name",
                ));
            }
            (_, Some(Json::Null) | None) => None,
            (_, Some(_)) => {
                return Err(invalid(
                    &format!("{at}.options.auth_header"),
                    "only the way of presenting the key as a header of your choice takes a header name",
                ));
            }
        };
        if let Some(name) = &auth_header {
            options.insert("auth_header".to_owned(), json!(name));
        }
        let mut forbidden: BTreeSet<String> = FORWARDER_HEADERS
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        match auth {
            "bearer" => {
                forbidden.insert("authorization".to_owned());
            }
            "x-api-key" => {
                forbidden.insert("x-api-key".to_owned());
            }
            "header" => {
                forbidden.extend(auth_header.iter().map(|name| name.to_ascii_lowercase()));
            }
            _ => {}
        }
        let headers = match given.get("headers") {
            None | Some(Json::Null) => Map::new(),
            Some(value) => text_map(&format!("{at}.options.headers"), value, true)?,
        };
        let secret_names: Vec<String> = match given.get("secret_headers") {
            None | Some(Json::Null) => Vec::new(),
            Some(Json::Array(names)) => names
                .iter()
                .map(|name| match name.as_str() {
                    Some(text) if is_header_name(text) => Ok(text.to_owned()),
                    _ => Err(invalid(
                        &format!("{at}.options.secret_headers"),
                        "these are header names",
                    )),
                })
                .collect::<ApiResult<_>>()?,
            Some(_) => {
                return Err(invalid(
                    &format!("{at}.options.secret_headers"),
                    "these are a list of header names",
                ));
            }
        };
        for name in headers
            .keys()
            .map(String::as_str)
            .chain(secret_names.iter().map(String::as_str))
        {
            if forbidden.contains(&name.to_ascii_lowercase()) {
                return Err(invalid(
                    &format!("{at}.options"),
                    &format!("{name} is set by the forwarder itself"),
                ));
            }
        }
        if secret_names.iter().any(|name| {
            headers
                .keys()
                .any(|header| header.eq_ignore_ascii_case(name))
        }) {
            return Err(invalid(
                &format!("{at}.options.secret_headers"),
                "a header is either given or secret, not both",
            ));
        }
        if !headers.is_empty() {
            options.insert("headers".to_owned(), Json::Object(headers));
        }
        match given.get("query") {
            None | Some(Json::Null) => {}
            Some(value) => {
                let query = text_map(&format!("{at}.options.query"), value, false)?;
                if !query.is_empty() {
                    options.insert("query".to_owned(), Json::Object(query));
                }
            }
        }
        if !secret_names.is_empty() {
            secret_headers.extend(secret_names.iter().cloned());
            options.insert("secret_headers".to_owned(), json!(secret_names));
        }
        protocols.push(upstream.protocol.clone());
        kept.push(Upstream {
            protocol: upstream.protocol.clone(),
            base_url,
            options: Json::Object(options),
        });
    }
    Ok(Checked {
        upstreams: kept,
        protocols,
        secret_headers,
        keyless,
        clear_text,
    })
}

/// Reads the endpoints of a stored provider.
pub fn stored(upstreams: &Json) -> ApiResult<Vec<Upstream>> {
    serde_json::from_value(upstreams.clone())
        .map_err(|error| ApiError::internal(format!("stored endpoints do not read: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(protocol: &str, base_url: &str, options: Json) -> Upstream {
        Upstream {
            protocol: protocol.to_owned(),
            base_url: base_url.to_owned(),
            options,
        }
    }

    #[test]
    fn an_endpoint_on_the_runtimes_own_loopback_needs_no_tls_whatever_the_policy() {
        for local in [
            "http://127.0.0.1:8090/v1",
            "http://localhost:8090/v1",
            "http://[::1]:8090/v1",
        ] {
            let checked = check(
                &[endpoint(
                    "openai-embeddings",
                    local,
                    json!({ "auth": "none" }),
                )],
                false,
            )
            .unwrap();
            assert!(!checked.clear_text, "{local}");
            assert!(checked.keyless);
        }
        for away in [
            "http://10.0.0.5:8090/v1",
            "http://127.0.0.1.example.com/v1",
            "http://user@127.0.0.1:8090/v1",
        ] {
            assert!(
                check(&[endpoint("openai-embeddings", away, json!({}))], false).is_err(),
                "{away}"
            );
        }
    }

    #[test]
    fn an_endpoint_is_https_and_an_upstream_has_one_for_each_protocol() {
        let one = endpoint("openai-responses", "https://api.example.com/v1/", json!({}));
        let checked = check(std::slice::from_ref(&one), false).unwrap();
        assert_eq!(checked.upstreams[0].base_url, "https://api.example.com/v1");
        assert_eq!(checked.protocols, ["openai-responses"]);
        assert!(!checked.keyless && !checked.clear_text);
        assert!(check(&[], false).is_err(), "at least one endpoint");
        assert!(
            check(&[one.clone(), one], false).is_err(),
            "one for each protocol"
        );
        let http = endpoint("openai-responses", "http://models.internal:8080", json!({}));
        assert!(check(std::slice::from_ref(&http), false).is_err());
        assert!(check(&[http], true).unwrap().clear_text);
        for bad in [
            "ftp://x.example",
            "https://",
            "https://u:p@x.example",
            "https://x.example?a=b",
            "x.example",
        ] {
            let upstream = endpoint("anthropic-messages", bad, json!({}));
            assert!(check(&[upstream], true).is_err(), "{bad}");
        }
        let unknown = endpoint("gopher", "https://x.example", json!({}));
        assert!(check(&[unknown], false).is_err());
    }

    #[test]
    fn only_the_options_a_protocol_declares_are_taken_and_the_key_header_is_the_forwarders() {
        let ok = |options: Json| {
            check(
                &[endpoint("anthropic-messages", "https://x.example", options)],
                false,
            )
        };
        assert!(
            ok(
                json!({ "auth": "x-api-key", "headers": { "x-route": "a" }, "query": { "v": "1" } })
            )
            .is_ok()
        );
        assert!(
            ok(json!({ "colour": 1 })).is_err(),
            "an option no protocol declares"
        );
        assert!(ok(json!({ "auth": "digest" })).is_err());
        assert!(
            ok(json!({ "auth": "header" })).is_err(),
            "it needs the header's name"
        );
        assert!(ok(json!({ "auth": "header", "auth_header": "x-gateway" })).is_ok());
        assert!(
            ok(json!({ "auth_header": "x-gateway" })).is_err(),
            "only the choice of a header takes one"
        );
        assert!(
            ok(json!({ "headers": { "Authorization": "Bearer x" } })).is_err(),
            "the key header"
        );
        assert!(
            ok(json!({ "auth": "x-api-key", "headers": { "Authorization": "Bearer x" } })).is_ok()
        );
        assert!(ok(json!({ "auth": "x-api-key", "headers": { "X-Api-Key": "x" } })).is_err());
        assert!(
            ok(json!({ "headers": { "Host": "x" } })).is_err(),
            "the forwarder sets it"
        );
        assert!(ok(json!({ "headers": { "x-a": "1" }, "secret_headers": ["X-A"] })).is_err());
    }

    #[test]
    fn an_upstream_that_takes_no_key_on_any_endpoint_is_keyless_and_secret_headers_are_collected() {
        let checked = check(
            &[
                endpoint(
                    "openai-responses",
                    "https://a.example",
                    json!({ "auth": "none" }),
                ),
                endpoint(
                    "anthropic-messages",
                    "https://a.example/anthropic",
                    json!({ "auth": "none", "secret_headers": ["x-gateway-key"] }),
                ),
            ],
            false,
        )
        .unwrap();
        assert!(checked.keyless);
        assert_eq!(
            checked.secret_headers.into_iter().collect::<Vec<_>>(),
            ["x-gateway-key"]
        );
    }
}
