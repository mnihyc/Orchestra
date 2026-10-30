//! What one endpoint may configure beyond its address: how the key is presented, fixed and secret headers, query parameters.
//!
//! An API provider lists one or more endpoints of one upstream. Each has a protocol, a base address, and the
//! options its protocol declares ([`standard_options`] are the ones both built-in protocols declare):
//!
//! | Option | Meaning | When absent |
//! |--------|---------|-------------|
//! | `auth` | How the key is presented: `bearer`, `x-api-key`, `header` (the key as the value of the header named in `auth_header`) or `none` | The protocol's own way |
//! | `headers` | Headers sent with every request to that endpoint, as they are | None |
//! | `secret_headers` | Names of headers whose values are secrets of the provider, stored and rotated like its key and never returned | None |
//! | `query` | Query parameters appended to every request | None |
//!
//! A header the forwarder sets itself cannot be given: the one that carries the key, `Host`,
//! `Content-Length` and the hop-by-hop headers. [`EndpointOptions::check`] refuses them when a provider is
//! saved, and [`authorize`] presents the key and the options on a request the way the forwarder does.

use std::collections::BTreeMap;

use http::{HeaderMap, HeaderName, HeaderValue, header};
use orch_plugin_contract::Secret;
use orch_types::{Choice, Field, FieldType, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::credentials::Credential;

/// How the key is presented to an upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum AuthMode {
    /// `Authorization: Bearer <key>`.
    #[serde(rename = "bearer")]
    Bearer,
    /// `x-api-key: <key>`.
    #[serde(rename = "x-api-key")]
    XApiKey,
    /// The key as the value of the header named in `auth_header`.
    #[serde(rename = "header")]
    Header,
    /// The upstream takes no key.
    #[serde(rename = "none")]
    None,
}

impl AuthMode {
    /// The mode as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
            Self::XApiKey => "x-api-key",
            Self::Header => "header",
            Self::None => "none",
        }
    }
}

/// What one endpoint of an API upstream may configure beyond its base address. An option that is not here
/// is refused when the provider is saved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EndpointOptions {
    /// How the key is presented; absent means the protocol's own way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthMode>,
    /// With `auth: "header"`: the header that carries the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<String>,
    /// Headers sent with every request to that endpoint, as they are.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Names of headers whose values are secrets of the provider. The values are not here: each is stored
    /// and rotated like the provider's key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_headers: Vec<String>,
    /// Query parameters appended to every request.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, String>,
}

/// One endpoint of an API upstream: the protocol it speaks, where it is, and its options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Upstream {
    /// The wire protocol.
    pub protocol: String,
    /// The base address; requests are made below it.
    pub base_url: String,
    /// The options the protocol declares. For a protocol that a translator serves, whatever that
    /// translator's descriptor declares.
    #[serde(default)]
    pub options: Json,
}

impl EndpointOptions {
    /// Reads the options of an endpoint of a built-in protocol. An option that is not one of the five is
    /// refused.
    pub fn parse(options: &Json) -> Result<Self, serde_json::Error> {
        if options.is_null() {
            return Ok(Self::default());
        }
        serde_json::from_value(options.clone())
    }
}

/// What is wrong with an endpoint's options or address.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{option}: {message}")]
pub struct OptionFault {
    /// The option, or `base_url`.
    pub option: String,
    /// What is wrong with it.
    pub message: String,
}

impl OptionFault {
    fn new(option: &str, message: impl Into<String>) -> Self {
        Self {
            option: option.to_owned(),
            message: message.into(),
        }
    }
}

/// Headers that belong to one connection and are never passed on or given as an option.
pub const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Headers the forwarder always sets itself, apart from the one that carries the key.
pub const FORWARDER_HEADERS: [&str; 2] = ["host", "content-length"];

fn is_forbidden(name: &str, key_header: Option<&str>) -> bool {
    let lower = name.to_ascii_lowercase();
    FORWARDER_HEADERS.contains(&lower.as_str())
        || HOP_BY_HOP.contains(&lower.as_str())
        || key_header.is_some_and(|key| key.eq_ignore_ascii_case(&lower))
}

impl EndpointOptions {
    /// The header that carries the key under these options, given the protocol's own way `default_auth`.
    #[must_use]
    pub fn key_header(&self, default_auth: AuthMode) -> Option<&str> {
        match self.auth.unwrap_or(default_auth) {
            AuthMode::Bearer => Some("authorization"),
            AuthMode::XApiKey => Some("x-api-key"),
            AuthMode::Header => self.auth_header.as_deref(),
            AuthMode::None => None,
        }
    }

    /// Checks the options as a provider is saved: `auth_header` is given with `auth: "header"` and only
    /// then; every header name is a header name and every value a header value; no header is one the
    /// forwarder sets itself (the one that carries the key, `Host`, `Content-Length`, the hop-by-hop
    /// ones); a name is not given twice, as a fixed header and as a secret one; query names are not empty.
    /// Every fault is returned.
    pub fn check(&self, default_auth: AuthMode) -> Result<(), Vec<OptionFault>> {
        let mut faults = Vec::new();
        let mode = self.auth.unwrap_or(default_auth);
        match (mode, self.auth_header.as_deref()) {
            (AuthMode::Header, None) => {
                faults.push(OptionFault::new(
                    "auth_header",
                    "is needed when auth is \"header\"",
                ));
            }
            (AuthMode::Header, Some(name)) => {
                if HeaderName::from_bytes(name.as_bytes()).is_err() {
                    faults.push(OptionFault::new(
                        "auth_header",
                        format!("{name:?} is no header name"),
                    ));
                }
            }
            (_, Some(_)) => faults.push(OptionFault::new(
                "auth_header",
                "is for auth \"header\" only",
            )),
            (_, None) => {}
        }
        let key_header = self.key_header(default_auth);
        let mut seen: Vec<String> = Vec::new();
        for name in self.headers.keys().chain(self.secret_headers.iter()) {
            if HeaderName::from_bytes(name.as_bytes()).is_err() {
                faults.push(OptionFault::new(
                    "headers",
                    format!("{name:?} is no header name"),
                ));
                continue;
            }
            if is_forbidden(name, key_header) {
                faults.push(OptionFault::new(
                    "headers",
                    format!("{name:?} is a header the forwarder sets itself"),
                ));
            }
            let lower = name.to_ascii_lowercase();
            if seen.contains(&lower) {
                faults.push(OptionFault::new(
                    "headers",
                    format!("{name:?} is given twice"),
                ));
            }
            seen.push(lower);
        }
        for (name, value) in &self.headers {
            if HeaderValue::from_str(value).is_err() {
                faults.push(OptionFault::new(
                    "headers",
                    format!("the value of {name:?} is no header value"),
                ));
            }
        }
        if self.query.keys().any(String::is_empty) {
            faults.push(OptionFault::new("query", "a parameter needs a name"));
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }
}

/// Checks the address of an endpoint: it is `https`, or `http` where the resolved policy
/// `providers.allow_http_upstreams` allows it (and a key then travels in clear), and it names a host.
pub fn check_base_url(base_url: &str, allow_http: bool) -> Result<Url, OptionFault> {
    // The rule is the provider contract's, which a source's address follows too.
    orch_provider_contract::address::check_url(base_url, allow_http)
        .map_err(|message| OptionFault::new("base_url", message))
}

/// Why a request could not be authorized.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthorizeError {
    /// The endpoint takes a key and none is held.
    #[error("the endpoint takes a key and the provider has none")]
    NoCredential,
    /// `auth` is `header` and no header is named.
    #[error("auth is \"header\" and no auth_header is named")]
    NoAuthHeader,
    /// A secret header has no value.
    #[error("the secret header {0:?} has no value")]
    SecretMissing(String),
    /// A name or a value cannot be a header.
    #[error("{0:?} cannot be a header")]
    BadHeader(String),
}

/// Presents the key and the options of an endpoint on the headers of a request that is about to be
/// forwarded, the way the forwarder does, and returns the query parameters to append.
///
/// First, `Authorization`, `x-api-key`, `Cookie`, the hop-by-hop headers and every header the options name
/// are removed, so that what the harness presented never reaches the upstream. Then the key is added the
/// way `auth` says, or the protocol's own way `default_auth` when it is absent; nothing for `none`. Then
/// the fixed headers, then the secret ones, whose values come from `secrets` by name. The key and the
/// secret values are marked sensitive, so that they are not printed with the headers.
pub fn authorize(
    headers: &mut HeaderMap,
    credential: Option<&Credential>,
    options: &EndpointOptions,
    secrets: &BTreeMap<String, Secret>,
    default_auth: AuthMode,
) -> Result<Vec<(String, String)>, AuthorizeError> {
    let bad = |name: &str| AuthorizeError::BadHeader(name.to_owned());
    for name in ["authorization", "x-api-key", "cookie"]
        .into_iter()
        .chain(HOP_BY_HOP)
    {
        headers.remove(name);
    }
    for name in options
        .headers
        .keys()
        .chain(options.secret_headers.iter())
        .chain(options.auth_header.iter())
    {
        headers.remove(HeaderName::from_bytes(name.as_bytes()).map_err(|_| bad(name))?);
    }
    let key = || {
        credential
            .map(Credential::expose)
            .ok_or(AuthorizeError::NoCredential)
    };
    let presented: Option<(HeaderName, String)> = match options.auth.unwrap_or(default_auth) {
        AuthMode::None => None,
        AuthMode::Bearer => Some((header::AUTHORIZATION, format!("Bearer {}", key()?))),
        AuthMode::XApiKey => Some((HeaderName::from_static("x-api-key"), key()?.to_owned())),
        AuthMode::Header => {
            let name = options
                .auth_header
                .as_deref()
                .ok_or(AuthorizeError::NoAuthHeader)?;
            Some((
                HeaderName::from_bytes(name.as_bytes()).map_err(|_| bad(name))?,
                key()?.to_owned(),
            ))
        }
    };
    if let Some((name, value)) = presented {
        let mut value = HeaderValue::from_str(&value).map_err(|_| bad(name.as_str()))?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    for (name, value) in &options.headers {
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| bad(name))?;
        headers.insert(
            header_name,
            HeaderValue::from_str(value).map_err(|_| bad(name))?,
        );
    }
    for name in &options.secret_headers {
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| bad(name))?;
        let secret = secrets
            .get(name)
            .or_else(|| {
                secrets
                    .iter()
                    .find(|(known, _)| known.eq_ignore_ascii_case(name))
                    .map(|(_, secret)| secret)
            })
            .ok_or_else(|| AuthorizeError::SecretMissing(name.clone()))?;
        let mut value = HeaderValue::from_str(secret.expose()).map_err(|_| bad(name))?;
        value.set_sensitive(true);
        headers.insert(header_name, value);
    }
    Ok(options
        .query
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect())
}

/// The options both built-in protocols declare, in the one shape in which a person is asked for a value.
#[must_use]
pub fn standard_options() -> Vec<Field> {
    let mut auth = Field::new("auth", "How the key is presented", FieldType::Choice);
    auth.required = false;
    auth.choices = Some(
        [
            ("bearer", "Authorization: Bearer <key>"),
            ("x-api-key", "x-api-key: <key>"),
            ("header", "A header of your choice"),
            ("none", "No key"),
        ]
        .into_iter()
        .map(|(value, label)| Choice {
            value: value.to_owned(),
            label: label.to_owned(),
        })
        .collect(),
    );
    auth.help = Some(
        "How the key is sent to this endpoint. Left out, the protocol's own way is used. Choose \"none\" for a model \
         server that takes no key."
            .to_owned(),
    );
    let mut auth_header = Field::new(
        "auth_header",
        "Header that carries the key",
        FieldType::Text,
    );
    auth_header.required = false;
    auth_header.help = Some(
        "Only with \"A header of your choice\": the name of the header that holds the key."
            .to_owned(),
    );
    let mut headers = Field::new("headers", "Headers", FieldType::Map);
    headers.required = false;
    headers.help = Some(
        "Headers sent with every request to this endpoint, as they are: a routing header for a gateway, a version \
         header an upstream insists on."
            .to_owned(),
    );
    let mut secret_headers = Field::new("secret_headers", "Secret headers", FieldType::SecretMap);
    secret_headers.required = false;
    secret_headers.help = Some(
        "Headers whose values are secrets, such as a gateway's own key beside the upstream's. They are stored like the \
         key and never shown again."
            .to_owned(),
    );
    let mut query = Field::new("query", "Query parameters", FieldType::Map);
    query.required = false;
    query.help = Some(
        "Parameters added to the address of every request, such as an API version.".to_owned(),
    );
    vec![auth, auth_header, headers, secret_headers, query]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn options(value: Json) -> EndpointOptions {
        EndpointOptions::parse(&value).unwrap()
    }

    fn secrets(pairs: &[(&str, &str)]) -> BTreeMap<String, Secret> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), Secret::new(*value)))
            .collect()
    }

    #[test]
    fn the_options_of_an_endpoint_read_as_the_spec_writes_them() {
        let read = options(json!({
            "auth": "x-api-key", "headers": { "x-route": "team-a" }, "query": {}, "secret_headers": ["x-gateway-key"]
        }));
        assert_eq!(read.auth, Some(AuthMode::XApiKey));
        assert_eq!(read.secret_headers, ["x-gateway-key"]);
        assert_eq!(read.check(AuthMode::Bearer), Ok(()));
        assert_eq!(AuthMode::XApiKey.as_str(), "x-api-key");
        assert_eq!(
            EndpointOptions::parse(&Json::Null).unwrap(),
            EndpointOptions::default()
        );
    }

    #[test]
    fn an_option_the_protocol_does_not_declare_is_refused() {
        assert!(EndpointOptions::parse(&json!({ "timeout": 5 })).is_err());
        assert!(EndpointOptions::parse(&json!({ "auth": "digest" })).is_err());
    }

    #[test]
    fn a_header_the_forwarder_sets_itself_cannot_be_given() {
        let faults = options(json!({
            "headers": { "Host": "x", "content-length": "3", "Transfer-Encoding": "chunked", "Authorization": "Bearer y" },
            "secret_headers": ["connection"]
        }))
        .check(AuthMode::Bearer)
        .unwrap_err();
        assert_eq!(faults.len(), 5, "{faults:?}");
        // With `auth: none` there is no key header, so the upstream can be given its own authorization.
        assert_eq!(
            options(json!({ "auth": "none", "headers": { "Authorization": "Basic abc" } }))
                .check(AuthMode::Bearer),
            Ok(())
        );
        // The header that carries the key is the one named, when a header is named.
        let named = options(
            json!({ "auth": "header", "auth_header": "x-gateway-key", "headers": { "X-Gateway-Key": "k" } }),
        );
        assert_eq!(named.check(AuthMode::Bearer).unwrap_err().len(), 1);
    }

    #[test]
    fn auth_header_goes_with_auth_header_and_with_nothing_else() {
        assert_eq!(
            options(json!({ "auth": "header" }))
                .check(AuthMode::Bearer)
                .unwrap_err()[0]
                .option,
            "auth_header"
        );
        assert_eq!(
            options(json!({ "auth": "bearer", "auth_header": "x-key" }))
                .check(AuthMode::Bearer)
                .unwrap_err()[0]
                .message,
            "is for auth \"header\" only"
        );
        assert_eq!(
            options(json!({ "auth": "header", "auth_header": "x-key" })).check(AuthMode::Bearer),
            Ok(())
        );
        let twice = options(json!({ "headers": { "X-A": "1" }, "secret_headers": ["x-a"] }))
            .check(AuthMode::Bearer);
        assert!(
            twice
                .unwrap_err()
                .iter()
                .any(|fault| fault.message.contains("twice"))
        );
        assert!(
            options(json!({ "query": { "": "x" } }))
                .check(AuthMode::Bearer)
                .is_err()
        );
        assert!(
            options(json!({ "headers": { "bad name": "x" } }))
                .check(AuthMode::Bearer)
                .is_err()
        );
    }

    #[test]
    fn an_address_is_https_unless_the_policy_allows_http() {
        assert!(check_base_url("https://llm.example.com/anthropic", false).is_ok());
        assert!(check_base_url("http://10.0.0.5:8000", true).is_ok());
        let refused = check_base_url("http://10.0.0.5:8000", false).unwrap_err();
        assert!(refused.message.contains("in clear"));
        assert!(check_base_url("ftp://x.example", true).is_err());
        assert!(check_base_url("not a url", true).is_err());
    }

    #[test]
    fn the_key_is_presented_the_way_the_options_say_and_what_the_harness_sent_is_removed() {
        let mut incoming = HeaderMap::new();
        incoming.insert(
            "authorization",
            HeaderValue::from_static("Bearer harness-token"),
        );
        incoming.insert("x-api-key", HeaderValue::from_static("harness-token"));
        incoming.insert("cookie", HeaderValue::from_static("session=1"));
        incoming.insert("connection", HeaderValue::from_static("keep-alive"));
        incoming.insert("x-route", HeaderValue::from_static("from-harness"));
        incoming.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        let key = Credential::new("sk-real");
        let opts = options(json!({
            "auth": "x-api-key", "headers": { "x-route": "team-a" }, "secret_headers": ["x-gateway-key"],
            "query": { "api-version": "2025-01-01" }
        }));
        let query = authorize(
            &mut incoming,
            Some(&key),
            &opts,
            &secrets(&[("x-gateway-key", "gw-secret")]),
            AuthMode::Bearer,
        )
        .unwrap();
        assert!(
            incoming.get("authorization").is_none(),
            "what the harness presented never reaches the upstream"
        );
        assert!(incoming.get("cookie").is_none() && incoming.get("connection").is_none());
        assert_eq!(incoming["x-api-key"], "sk-real");
        assert_eq!(
            incoming["x-route"], "team-a",
            "an option wins over what the harness sent"
        );
        assert_eq!(incoming["x-gateway-key"], "gw-secret");
        assert_eq!(
            incoming["anthropic-version"], "2023-06-01",
            "what is not named is passed on"
        );
        assert!(incoming["x-api-key"].is_sensitive() && incoming["x-gateway-key"].is_sensitive());
        assert!(!format!("{incoming:?}").contains("sk-real"));
        assert_eq!(query, [("api-version".to_owned(), "2025-01-01".to_owned())]);
    }

    #[test]
    fn the_protocols_own_way_is_used_when_the_options_say_nothing() {
        let key = Credential::new("sk-real");
        let mut headers = HeaderMap::new();
        authorize(
            &mut headers,
            Some(&key),
            &EndpointOptions::default(),
            &BTreeMap::new(),
            AuthMode::Bearer,
        )
        .unwrap();
        assert_eq!(headers["authorization"], "Bearer sk-real");
        let mut custom = HeaderMap::new();
        let opts = options(json!({ "auth": "header", "auth_header": "api-key" }));
        authorize(
            &mut custom,
            Some(&key),
            &opts,
            &BTreeMap::new(),
            AuthMode::Bearer,
        )
        .unwrap();
        assert_eq!(custom["api-key"], "sk-real");
        assert!(custom.get("authorization").is_none());
    }

    #[test]
    fn an_endpoint_that_takes_no_key_gets_none_and_needs_none() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer harness-token"),
        );
        authorize(
            &mut headers,
            None,
            &options(json!({ "auth": "none" })),
            &BTreeMap::new(),
            AuthMode::Bearer,
        )
        .unwrap();
        assert!(headers.is_empty());
        assert_eq!(
            authorize(
                &mut HeaderMap::new(),
                None,
                &EndpointOptions::default(),
                &BTreeMap::new(),
                AuthMode::Bearer
            ),
            Err(AuthorizeError::NoCredential)
        );
        assert_eq!(
            authorize(
                &mut HeaderMap::new(),
                Some(&Credential::new("k")),
                &options(json!({ "secret_headers": ["x-g"] })),
                &BTreeMap::new(),
                AuthMode::Bearer
            ),
            Err(AuthorizeError::SecretMissing("x-g".into()))
        );
        assert_eq!(
            authorize(
                &mut HeaderMap::new(),
                Some(&Credential::new("k")),
                &options(json!({ "auth": "header" })),
                &BTreeMap::new(),
                AuthMode::Bearer
            ),
            Err(AuthorizeError::NoAuthHeader)
        );
    }

    #[test]
    fn a_form_asks_for_the_five_options_and_none_is_required() {
        let fields = standard_options();
        let names: Vec<_> = fields.iter().map(|field| field.name.as_str()).collect();
        assert_eq!(
            names,
            ["auth", "auth_header", "headers", "secret_headers", "query"]
        );
        assert!(
            fields
                .iter()
                .all(|field| !field.required && field.help.is_some())
        );
        assert_eq!(fields[0].choices.as_ref().unwrap().len(), 4);
        // What a form can give is accepted by the options.
        assert_eq!(
            orch_types::fields::validate(
                &fields,
                &json!({ "auth": "none", "query": { "v": "1" } })
            ),
            Ok(())
        );
    }
}
