//! Issuer metadata and provider validation.
//!
//! An issuer describes itself at `<issuer>/.well-known/openid-configuration`: where a browser signs a
//! person in, where a code is exchanged, where its keys are and what it signs with. The description is
//! taken only where it names exactly the issuer it was asked for, offers the code flow, and gives addresses that are safe to
//! send a secret to: `https`, or `http` to this very machine, for a provider in development.
//!
//! What was read is kept by issuer for [`METADATA_TTL`], and the keys for [`KEYS_TTL`]; a token that
//! names a key the kept set does not have makes them be read again at once, once, so that a provider
//! that rotates its keys is followed without waiting. A cache's age is measured on the process's own
//! monotonic clock: it decides when to ask again, never whether an identity is good. **Test** reads
//! both anew and keeps what it read.

use crate::get_json;
use crate::jws::{Algorithm, KeySet};
use orch_identity_contract::provider::{
    Discovered, ProviderError, ProviderSettings, discovery_url,
};
use orch_types::Json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use url::{Host, Url};

/// How long what an issuer says of itself is kept: an hour, so that a moved endpoint is followed within
/// the hour and a morning of sign-ins asks the provider once.
pub const METADATA_TTL: Duration = Duration::from_secs(3_600);
/// How long an issuer's keys are kept: an hour as well. A key the kept set does not have is looked for
/// again at once, so a rotation does not wait for this.
pub const KEYS_TTL: Duration = Duration::from_secs(3_600);

/// What an issuer says of itself, as far as signing in needs it.
#[derive(Debug, Clone)]
pub struct Metadata {
    /// The issuer it names itself by, which every ID token it issues names.
    pub issuer: String,
    /// Where the browser signs the person in.
    pub authorization_endpoint: Url,
    /// Where a code is exchanged for the tokens.
    pub token_endpoint: Url,
    /// Where its keys are.
    pub jwks_uri: Url,
    /// Where what it knows of the person is read with an access token, where it says.
    pub userinfo_endpoint: Option<Url>,
    /// The algorithms it says it signs ID tokens with, of those that are taken; `None` where it does
    /// not say.
    pub signing_algorithms: Option<Vec<Algorithm>>,
    /// How a client may authenticate at the token endpoint; empty where it does not say, which means
    /// `client_secret_basic`.
    pub auth_methods: Vec<String>,
    /// The description as it was published.
    pub document: Json,
}

impl Metadata {
    /// Reads the description an issuer published when it was asked as `issuer`.
    pub fn read(issuer: &str, document: Json) -> Result<Self, ProviderError> {
        let named = document
            .get("issuer")
            .and_then(Json::as_str)
            .ok_or_else(|| ProviderError::new("the description names no issuer"))?
            .to_owned();
        if named != issuer {
            return Err(ProviderError::new(format!(
                "the description names the issuer {named}, not {issuer}"
            )));
        }
        if let Some(types) = document
            .get("response_types_supported")
            .and_then(Json::as_array)
            && !types.iter().any(|kind| kind.as_str() == Some("code"))
        {
            return Err(ProviderError::new(
                "the provider does not offer the authorization code flow",
            ));
        }
        let signing_algorithms = document
            .get("id_token_signing_alg_values_supported")
            .and_then(Json::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Json::as_str)
                    .filter_map(Algorithm::from_name)
                    .collect()
            });
        let auth_methods = document
            .get("token_endpoint_auth_methods_supported")
            .and_then(Json::as_array)
            .map(|methods| {
                methods
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let required = |name: &str| -> Result<Url, ProviderError> {
            endpoint(&document, name)?
                .ok_or_else(|| ProviderError::new(format!("the description gives no {name}")))
        };
        let authorization_endpoint = required("authorization_endpoint")?;
        let token_endpoint = required("token_endpoint")?;
        let jwks_uri = required("jwks_uri")?;
        let userinfo_endpoint = endpoint(&document, "userinfo_endpoint")?;
        Ok(Self {
            issuer: named,
            authorization_endpoint,
            token_endpoint,
            jwks_uri,
            userinfo_endpoint,
            signing_algorithms,
            auth_methods,
            document,
        })
    }

    /// Whether the issuer says it signs with `alg`, or says nothing of what it signs with.
    #[must_use]
    pub fn allows(&self, alg: Algorithm) -> bool {
        self.signing_algorithms
            .as_ref()
            .is_none_or(|listed| listed.contains(&alg))
    }
}

/// The address the description gives under `name`, if it gives one; an address that is not safe to
/// send a secret to is refused.
fn endpoint(document: &Json, name: &str) -> Result<Option<Url>, ProviderError> {
    let Some(text) = document.get(name).and_then(Json::as_str) else {
        return Ok(None);
    };
    let url =
        Url::parse(text).map_err(|_| ProviderError::new(format!("{name} is not an address")))?;
    match address_fault(&url) {
        Some(fault) => Err(ProviderError::new(format!("{name} {fault}"))),
        None => Ok(Some(url)),
    }
}

/// What is wrong with an address a secret may be sent to: it is `https`, or `http` to this machine.
fn address_fault(url: &Url) -> Option<&'static str> {
    match (url.scheme(), url.host()) {
        ("https", Some(_)) => None,
        ("http", Some(host)) if is_loopback(&host) => None,
        ("http", Some(_)) => Some("must use https"),
        _ => Some("must be an https address"),
    }
}

fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    }
}

/// What is kept of one issuer.
#[derive(Debug, Clone)]
struct Kept {
    metadata: Arc<Metadata>,
    read_at: Instant,
    keys: Option<(Arc<KeySet>, Instant)>,
}

/// What issuers said of themselves and the keys they publish, kept by the issuer they were asked as.
#[derive(Debug, Default)]
pub struct Cache {
    issuers: Mutex<HashMap<String, Kept>>,
}

impl Cache {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Kept>> {
        self.issuers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn kept(&self, issuer: &str) -> Option<Kept> {
        self.lock().get(issuer).cloned()
    }

    /// What `issuer` says of itself: as kept, or read now where nothing is kept or it is old.
    pub async fn metadata(
        &self,
        client: &reqwest::Client,
        issuer: &str,
    ) -> Result<Arc<Metadata>, ProviderError> {
        if let Some(kept) = self.kept(issuer)
            && kept.read_at.elapsed() < METADATA_TTL
        {
            return Ok(kept.metadata);
        }
        let metadata = Arc::new(read_metadata(client, issuer).await?);
        self.keep(issuer, Arc::clone(&metadata), None);
        Ok(metadata)
    }

    /// The keys of `issuer`, whose description is `metadata`: as kept, or read now where `again` is
    /// asked, nothing is kept, or they are old. The answer says whether they were read now.
    pub async fn keys(
        &self,
        client: &reqwest::Client,
        issuer: &str,
        metadata: &Metadata,
        again: bool,
    ) -> Result<(Arc<KeySet>, bool), ProviderError> {
        if !again
            && let Some((keys, read_at)) = self.kept(issuer).and_then(|kept| kept.keys)
            && read_at.elapsed() < KEYS_TTL
        {
            return Ok((keys, false));
        }
        let keys = Arc::new(read_keys(client, &metadata.jwks_uri).await?);
        if let Some(kept) = self.lock().get_mut(issuer)
            && kept.metadata.jwks_uri == metadata.jwks_uri
        {
            kept.keys = Some((Arc::clone(&keys), Instant::now()));
        }
        Ok((keys, true))
    }

    /// Keeps what `issuer` said of itself, and its keys where they were read with it. Keys kept from
    /// another address than the one described now are forgotten.
    pub fn keep(&self, issuer: &str, metadata: Arc<Metadata>, keys: Option<Arc<KeySet>>) {
        let now = Instant::now();
        let mut issuers = self.lock();
        let kept = issuers.entry(issuer.to_owned()).or_insert_with(|| Kept {
            metadata: Arc::clone(&metadata),
            read_at: now,
            keys: None,
        });
        if kept.metadata.jwks_uri != metadata.jwks_uri {
            kept.keys = None;
        }
        kept.metadata = metadata;
        kept.read_at = now;
        if let Some(keys) = keys {
            kept.keys = Some((keys, now));
        }
    }
}

/// Reads what `issuer` says of itself, now.
pub async fn read_metadata(
    client: &reqwest::Client,
    issuer: &str,
) -> Result<Metadata, ProviderError> {
    let document = get_json(client, &discovery_url(issuer), None).await?;
    Metadata::read(issuer, document)
}

/// Reads the keys published at `jwks_uri`, now.
pub async fn read_keys(client: &reqwest::Client, jwks_uri: &Url) -> Result<KeySet, ProviderError> {
    let document = get_json(client, jwks_uri.as_str(), None).await?;
    KeySet::read(&document)
}

/// Looks at the provider for **Test**: reads its description and its keys anew, keeps them, and says
/// what was found, or what went wrong in the words an administrator reads.
pub async fn validate(
    client: &reqwest::Client,
    cache: &Cache,
    settings: &ProviderSettings,
) -> Result<Discovered, ProviderError> {
    let metadata = read_metadata(client, &settings.issuer)
        .await
        .map_err(|error| {
            ProviderError::new(format!(
                "Could not read the issuer's discovery document: {error}"
            ))
        })?;
    let keys = read_keys(client, &metadata.jwks_uri)
        .await
        .map_err(|error| {
            ProviderError::new(format!("Could not read the issuer's signing keys: {error}"))
        })?;
    if keys.keys.is_empty() {
        return Err(ProviderError::new(
            "The issuer publishes no signing key this system can use",
        ));
    }
    let (metadata, keys) = (Arc::new(metadata), Arc::new(keys));
    cache.keep(
        &settings.issuer,
        Arc::clone(&metadata),
        Some(Arc::clone(&keys)),
    );
    Ok(Discovered {
        issuer: metadata.issuer.clone(),
        signing_keys: i64::try_from(keys.keys.len()).unwrap_or(i64::MAX),
        metadata: metadata.document.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn described() -> Json {
        json!({
            "issuer": "https://login.acme.example/realms/main",
            "authorization_endpoint": "https://login.acme.example/realms/main/auth",
            "token_endpoint": "https://login.acme.example/realms/main/token",
            "jwks_uri": "https://keys.acme.example/certs",
            "userinfo_endpoint": "https://login.acme.example/realms/main/userinfo",
            "response_types_supported": ["code", "id_token"],
            "id_token_signing_alg_values_supported": ["RS256", "HS256", "ES256"],
            "token_endpoint_auth_methods_supported": ["client_secret_post"]
        })
    }

    #[test]
    fn a_description_of_the_issuer_asked_for_is_read_with_its_endpoints_and_algorithms() {
        let mut with_slash = described();
        with_slash["issuer"] = json!("https://login.acme.example/realms/main/");
        let read = Metadata::read("https://login.acme.example/realms/main/", with_slash).unwrap();
        assert_eq!(read.issuer, "https://login.acme.example/realms/main/");
        assert_eq!(read.jwks_uri.as_str(), "https://keys.acme.example/certs");
        assert_eq!(
            read.signing_algorithms,
            Some(vec![Algorithm::Rs256, Algorithm::Es256]),
            "a shared secret is no algorithm that is taken"
        );
        assert!(read.allows(Algorithm::Es256));
        assert!(!read.allows(Algorithm::EdDsa));
        assert_eq!(read.auth_methods, ["client_secret_post"]);
        let mut silent = described();
        silent
            .as_object_mut()
            .unwrap()
            .remove("id_token_signing_alg_values_supported");
        let read = Metadata::read("https://login.acme.example/realms/main", silent).unwrap();
        assert!(
            read.allows(Algorithm::EdDsa),
            "nothing said is nothing refused"
        );
    }

    #[test]
    fn a_description_of_another_issuer_or_without_the_code_flow_is_refused() {
        assert!(Metadata::read("https://login.acme.example/realms/other", described()).is_err());
        assert!(Metadata::read("https://login.acme.example/realms/main/", described()).is_err());
        let mut with_slash = described();
        with_slash["issuer"] = json!("https://login.acme.example/realms/main/");
        assert!(Metadata::read("https://login.acme.example/realms/main", with_slash).is_err());
        let mut implicit = described();
        implicit["response_types_supported"] = json!(["id_token"]);
        assert!(Metadata::read("https://login.acme.example/realms/main", implicit).is_err());
        let mut keyless = described();
        keyless.as_object_mut().unwrap().remove("jwks_uri");
        assert!(Metadata::read("https://login.acme.example/realms/main", keyless).is_err());
    }

    #[test]
    fn an_endpoint_is_https_or_on_this_machine() {
        for (address, good) in [
            ("https://login.acme.example/token", true),
            ("http://127.0.0.1:41500/token", true),
            ("http://localhost:41500/token", true),
            ("http://[::1]:41500/token", true),
            ("http://login.acme.example/token", false),
            ("ftp://login.acme.example/token", false),
        ] {
            let mut document = described();
            document["token_endpoint"] = json!(address);
            assert_eq!(
                Metadata::read("https://login.acme.example/realms/main", document).is_ok(),
                good,
                "{address}"
            );
        }
    }
}
