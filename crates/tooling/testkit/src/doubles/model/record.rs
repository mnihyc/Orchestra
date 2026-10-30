//! What the scripted model server remembers of each request, with every secret masked, and the check of the presented credential.
//!
//! A test asks the server what it was sent: the method, the path, the query, the headers, the body, which
//! script answered, and how the answer ended. The server is told what its credential is, so that it can check
//! the one that is presented, and it never writes that credential or any other secret into what it remembers.
//! A header is a secret when its name says so (it has `auth`, `key`, `token`, `secret`, `password`, `cookie` or
//! `credential` in it) or when the server was told it is one; a value is a secret wherever the credential or a
//! secret value the server was told of occurs in a header, the query or the body. A secret is written as
//! `[secret:` and the first eight digits of the BLAKE3 hash of its value, so that two secrets can be told apart
//! and a test can check that the right one arrived ([`fingerprint`]), while eight hexadecimal digits are far
//! too few to find a key by. A value with a scheme (`Bearer abc`) keeps the scheme.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use http::HeaderMap;
use serde::{Serialize, Serializer};
use serde_json::Value;

use super::request::Format;

/// Parts of a header name that make the header a secret.
const SECRET_NAME_PARTS: [&str; 7] = [
    "auth",
    "key",
    "token",
    "secret",
    "password",
    "cookie",
    "credential",
];

/// How many hexadecimal digits of a hash stand for a secret: 32 bits tell the few secrets of one test apart
/// and are of no use in finding a key.
const FINGERPRINT_DIGITS: usize = 8;

/// The scheme of the header that carries a bearer credential.
const BEARER: &str = "bearer";

/// Where a credential may be presented.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Presentation {
    /// `Authorization: Bearer <key>`.
    Bearer,
    /// `x-api-key: <key>`.
    XApiKey,
    /// The key as the value of the named header.
    Header(String),
}

impl fmt::Display for Presentation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bearer => f.write_str("bearer"),
            Self::XApiKey => f.write_str("x-api-key"),
            Self::Header(name) => write!(f, "header:{name}"),
        }
    }
}

impl FromStr for Presentation {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "bearer" => Ok(Self::Bearer),
            "x-api-key" => Ok(Self::XApiKey),
            other => match other.strip_prefix("header:") {
                Some(name) if !name.is_empty() => Ok(Self::Header(name.to_ascii_lowercase())),
                _ => Err(format!(
                    "{other:?} is none of bearer, x-api-key and header:<name>"
                )),
            },
        }
    }
}

impl Serialize for Presentation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// What came of checking the credential a request presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The server has no credential to check against; any request is let in.
    NotChecked,
    /// The right credential was presented in a way the server accepts.
    Accepted,
    /// Nothing that could be a credential was presented.
    Missing,
    /// Something was presented, but not the right credential in an accepted way.
    Wrong,
}

/// The result of checking the credential of one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CredentialCheck {
    /// The verdict.
    pub verdict: Verdict,
    /// Every way in which the right credential was presented, accepted or not.
    pub presented_by: Vec<Presentation>,
}

/// The value a request presents in the way `how`, if it presents one.
fn presented(headers: &HeaderMap, how: &Presentation) -> Option<String> {
    let value = |name: &str| {
        headers
            .get(name)?
            .to_str()
            .ok()
            .map(str::trim)
            .map(str::to_owned)
    };
    match how {
        Presentation::Bearer => {
            let authorization = value("authorization")?;
            let (scheme, key) = authorization.split_once(' ')?;
            scheme
                .eq_ignore_ascii_case(BEARER)
                .then(|| key.trim().to_owned())
        }
        Presentation::XApiKey => value("x-api-key"),
        Presentation::Header(name) => value(name),
    }
}

/// Checks the credential a request presents against `expected`, accepting it in the ways `accept`.
#[must_use]
pub fn check_credential(
    headers: &HeaderMap,
    expected: Option<&str>,
    accept: &[Presentation],
) -> CredentialCheck {
    let Some(expected) = expected else {
        return CredentialCheck {
            verdict: Verdict::NotChecked,
            presented_by: Vec::new(),
        };
    };
    let mut ways = vec![Presentation::Bearer, Presentation::XApiKey];
    for way in accept {
        if !ways.contains(way) {
            ways.push(way.clone());
        }
    }
    let presented_by: Vec<Presentation> = ways
        .iter()
        .filter(|way| presented(headers, way).as_deref() == Some(expected))
        .cloned()
        .collect();
    let verdict = if presented_by.iter().any(|way| accept.contains(way)) {
        Verdict::Accepted
    } else if headers.contains_key("authorization")
        || headers.contains_key("x-api-key")
        || accept.iter().any(
            |way| matches!(way, Presentation::Header(name) if headers.contains_key(name.as_str())),
        )
    {
        Verdict::Wrong
    } else {
        Verdict::Missing
    };
    CredentialCheck {
        verdict,
        presented_by,
    }
}

/// What the server holds secret: names of headers it was told of, and values.
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    names: Vec<String>,
    values: Vec<String>,
}

impl Secrets {
    /// Secrets that are the named headers and the given values.
    #[must_use]
    pub fn new(names: &[String], values: impl IntoIterator<Item = String>) -> Self {
        Self {
            names: names.iter().map(|name| name.to_ascii_lowercase()).collect(),
            values: values
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
        }
    }

    fn is_secret_name(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.names.contains(&lower) || SECRET_NAME_PARTS.iter().any(|part| lower.contains(part))
    }

    /// The text with every secret value in it replaced by its fingerprint.
    #[must_use]
    pub fn mask_text(&self, text: &str) -> String {
        let mut masked = text.to_owned();
        for value in &self.values {
            masked = masked.replace(value.as_str(), &fingerprint(value));
        }
        masked
    }

    /// The headers as a test sees them: lower-case names, one value each (several are joined with a comma),
    /// secrets masked.
    #[must_use]
    pub fn mask_headers(&self, headers: &HeaderMap) -> BTreeMap<String, String> {
        let mut masked: BTreeMap<String, String> = BTreeMap::new();
        for (name, value) in headers {
            let text = String::from_utf8_lossy(value.as_bytes()).into_owned();
            let text = if self.is_secret_name(name.as_str()) {
                match text.split_once(' ') {
                    Some((scheme, rest)) if scheme.chars().all(|c| c.is_ascii_alphabetic()) => {
                        format!("{scheme} {}", fingerprint(rest.trim()))
                    }
                    _ => fingerprint(&text),
                }
            } else {
                self.mask_text(&text)
            };
            masked
                .entry(name.as_str().to_owned())
                .and_modify(|known| {
                    known.push_str(", ");
                    known.push_str(&text);
                })
                .or_insert(text);
        }
        masked
    }
}

/// How a secret is written in what the server remembers: `[secret:` and eight digits of its hash.
#[must_use]
pub fn fingerprint(value: &str) -> String {
    let hash = blake3::hash(value.as_bytes()).to_hex();
    format!("[secret:{}]", &hash.as_str()[..FINGERPRINT_DIGITS])
}

/// Where the script that answered came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptSource {
    /// A directive in the message of the person.
    Directive,
    /// A script set ahead and used once.
    Queue,
    /// The script set to answer anything else.
    Default,
    /// No script: the server said what it was told.
    Echo,
}

/// How an answer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Still being sent.
    Pending,
    /// All of it was sent.
    Completed,
    /// It was an error, before any stream began.
    Error,
    /// The connection was aborted by the script.
    Cut,
    /// The body was ended early by the script.
    Truncated,
    /// The client went away before the end.
    ClientGone,
}

/// What the server remembers of one request.
#[derive(Debug, Clone, Serialize)]
pub struct RecordedRequest {
    /// The number of the request, counted from 1.
    pub seq: u64,
    /// The method.
    pub method: String,
    /// The path, without its query.
    pub path: String,
    /// The query as it was sent, secrets masked.
    pub query: Option<String>,
    /// The headers, secrets masked.
    pub headers: BTreeMap<String, String>,
    /// The body: the JSON it was, or else its text.
    pub body: Value,
    /// The format it was in, if it was a request to one of the two.
    pub format: Option<Format>,
    /// The model it named.
    pub model: Option<String>,
    /// Whether it asked for a stream.
    pub stream: bool,
    /// The check of its credential.
    pub credential: CredentialCheck,
    /// Where the script that answered came from.
    pub script: Option<ScriptSource>,
    /// The round of tool calls it was in.
    pub round: usize,
    /// The status it was answered with.
    pub status: Option<u16>,
    /// How the answer ended.
    pub outcome: Outcome,
}

impl RecordedRequest {
    /// The value of a header, masked as it was written, by name in any case.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    const BOTH: [Presentation; 2] = [Presentation::Bearer, Presentation::XApiKey];

    #[test]
    fn the_right_key_is_accepted_as_a_bearer_or_as_x_api_key_and_the_way_is_noted() {
        let by_bearer = check_credential(
            &headers(&[("authorization", "Bearer sk-1")]),
            Some("sk-1"),
            &BOTH,
        );
        assert_eq!(by_bearer.verdict, Verdict::Accepted);
        assert_eq!(by_bearer.presented_by, [Presentation::Bearer]);
        let by_header = check_credential(&headers(&[("x-api-key", "sk-1")]), Some("sk-1"), &BOTH);
        assert_eq!(by_header.verdict, Verdict::Accepted);
        assert_eq!(by_header.presented_by, [Presentation::XApiKey]);
        let scheme = check_credential(
            &headers(&[("authorization", "bEaReR sk-1")]),
            Some("sk-1"),
            &BOTH,
        );
        assert_eq!(scheme.verdict, Verdict::Accepted, "the scheme has no case");
    }

    #[test]
    fn a_wrong_key_or_nothing_at_all_is_not_accepted_and_the_two_are_told_apart() {
        let wrong = check_credential(
            &headers(&[("authorization", "Bearer sk-2")]),
            Some("sk-1"),
            &BOTH,
        );
        assert_eq!(
            (wrong.verdict, wrong.presented_by.len()),
            (Verdict::Wrong, 0)
        );
        let basic = check_credential(
            &headers(&[("authorization", "Basic c2stMQ==")]),
            Some("sk-1"),
            &BOTH,
        );
        assert_eq!(basic.verdict, Verdict::Wrong);
        let none = check_credential(&headers(&[("accept", "*/*")]), Some("sk-1"), &BOTH);
        assert_eq!(none.verdict, Verdict::Missing);
    }

    #[test]
    fn a_server_can_insist_on_one_way_and_the_right_key_the_wrong_way_is_wrong() {
        let only_header = [Presentation::Header("api-key".into())];
        let right_way =
            check_credential(&headers(&[("api-key", "sk-1")]), Some("sk-1"), &only_header);
        assert_eq!(right_way.verdict, Verdict::Accepted);
        assert_eq!(
            right_way.presented_by,
            [Presentation::Header("api-key".into())]
        );
        let wrong_way = check_credential(
            &headers(&[("authorization", "Bearer sk-1")]),
            Some("sk-1"),
            &only_header,
        );
        assert_eq!(wrong_way.verdict, Verdict::Wrong);
        assert_eq!(
            wrong_way.presented_by,
            [Presentation::Bearer],
            "the key itself was right"
        );
    }

    #[test]
    fn a_server_with_no_credential_lets_everything_in() {
        let check = check_credential(&headers(&[]), None, &BOTH);
        assert_eq!(check.verdict, Verdict::NotChecked);
    }

    #[test]
    fn a_secret_header_is_masked_by_its_name_and_keeps_its_scheme() {
        let secrets = Secrets::new(&[], []);
        let masked = secrets.mask_headers(&headers(&[
            ("authorization", "Bearer sk-secret"),
            ("x-api-key", "sk-secret"),
            ("x-gateway-key", "gw-secret"),
            ("x-route", "team-a"),
            ("anthropic-version", "2023-06-01"),
        ]));
        assert_eq!(
            masked["authorization"],
            format!("Bearer {}", fingerprint("sk-secret"))
        );
        assert_eq!(masked["x-api-key"], fingerprint("sk-secret"));
        assert_eq!(masked["x-gateway-key"], fingerprint("gw-secret"));
        assert_eq!(masked["x-route"], "team-a");
        assert_eq!(masked["anthropic-version"], "2023-06-01");
        assert!(!format!("{masked:?}").contains("sk-secret"));
    }

    #[test]
    fn a_header_the_server_was_told_of_is_secret_and_a_secret_value_is_masked_wherever_it_is() {
        let secrets = Secrets::new(&["X-Org".to_owned()], ["leaky-value".to_owned()]);
        let masked = secrets.mask_headers(&headers(&[
            ("x-org", "acme"),
            ("x-forwarded-for", "carries leaky-value by mistake"),
        ]));
        assert_eq!(masked["x-org"], fingerprint("acme"));
        assert_eq!(
            masked["x-forwarded-for"],
            format!("carries {} by mistake", fingerprint("leaky-value"))
        );
        assert_eq!(
            secrets.mask_text("?token=leaky-value&b=1"),
            format!("?token={}&b=1", fingerprint("leaky-value"))
        );
    }

    #[test]
    fn two_values_of_one_header_are_joined() {
        let masked = Secrets::default().mask_headers(&headers(&[("x-tag", "a"), ("x-tag", "b")]));
        assert_eq!(masked["x-tag"], "a, b");
    }

    #[test]
    fn a_fingerprint_tells_values_apart_and_is_stable() {
        assert_eq!(fingerprint("sk-1"), fingerprint("sk-1"));
        assert_ne!(fingerprint("sk-1"), fingerprint("sk-2"));
        let text = fingerprint("sk-1");
        assert!(text.starts_with("[secret:") && text.ends_with(']'));
        assert_eq!(text.len(), "[secret:".len() + FINGERPRINT_DIGITS + 1);
    }

    #[test]
    fn a_presentation_reads_and_writes_the_words_of_the_manual() {
        for text in ["bearer", "x-api-key", "header:api-key"] {
            assert_eq!(text.parse::<Presentation>().unwrap().to_string(), text);
        }
        assert_eq!(
            "header:API-Key".parse::<Presentation>().unwrap(),
            Presentation::Header("api-key".into())
        );
        assert!("basic".parse::<Presentation>().is_err());
        assert!("header:".parse::<Presentation>().is_err());
    }
}
