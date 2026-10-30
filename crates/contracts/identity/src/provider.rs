//! Sign-in start, callback and provider validation boundary.
//!
//! A provider lets people sign in with an account they have somewhere else. What it does is the same for every
//! kind, and the kind ([`IdentityProviderKind`]) does only what differs between protocols; the API owns the rest.
//!
//! 1. **Start.** The API checks that the provider is switched on, cleans the in-app address to return to
//!    ([`clean_redirect`]), and asks the kind to [`start`](IdentityProviderKind::start): the kind makes the
//!    request it needs to recognise the answer again ([`PendingSignIn`]) and the address the browser is sent to.
//!    The API keeps both with the rest, for ten minutes ([`SignInRecord`]), and sends the browser.
//! 2. **Callback.** The API reads the record by the `state` the browser brings and deletes it, so that it serves
//!    once; refuses with `state_expired` where there is none, where it is no longer live or where it was made
//!    for another provider ([`SignInRecord::check`]); reads what the provider sent
//!    ([`CallbackParams::authorization_code`]); asks
//!    the kind to [`complete`](IdentityProviderKind::complete) the sign-in, which gives the identity the
//!    provider vouches for; looks the identity up; and [`decide`](crate::claims::decide)s.
//! 3. **Validate.** When a provider is saved the API checks its settings ([`ProviderSettings::check`], and the
//!    kind's own [`check`](IdentityProviderKind::check)); its **Test** asks the kind to
//!    [`validate`](IdentityProviderKind::validate) them against the provider itself.
//!
//! The client secret is sealed in the database. The API unseals it for the length of one call and gives it to the
//! kind inside the request; the kind never reads the database or any other secret.

use async_trait::async_trait;
use chrono::Duration;
use orch_plugin_contract::Secret;
use orch_types::fields::FieldError;
use orch_types::{Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::claims::{ClaimNames, Flow, SignInError, VerifiedIdentity};

/// The kind that signs people in with OpenID Connect: the value of `kind` that selects it.
pub const KIND_OIDC: &str = "oidc";
/// The scope without which a provider answers with no identity at all.
pub const REQUIRED_SCOPE: &str = "openid";
/// What a provider asks for unless it is set to ask for something else.
pub const DEFAULT_SCOPES: [&str; 3] = ["openid", "profile", "email"];
/// The fewest characters of a key.
pub const KEY_MIN_CHARS: u32 = 2;
/// The most characters of a key.
pub const KEY_MAX_CHARS: u32 = 32;
/// How long a sign-in may take between its start and its callback: long enough to read a consent screen or
/// fetch a code from an authenticator, short enough that an abandoned one is not a way in.
pub const SIGN_IN_LIFETIME_SECS: i64 = 10 * 60;
/// Where the browser returns when the person gave no address to return to.
pub const DEFAULT_REDIRECT: &str = "/";
/// The longest address to return to: what the servers and proxies that carry a redirect commonly accept.
pub const REDIRECT_MAX_BYTES: u32 = 2048;
/// What is appended to an issuer to find its description of itself.
pub const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";
/// The path under which the API answers, before the paths of its sign-in.
pub const API_BASE_PATH: &str = "/api/v1";
/// The error a provider reports when the person, or the provider for them, refused.
pub const ACCESS_DENIED: &str = "access_denied";

fn default_scopes() -> Vec<String> {
    DEFAULT_SCOPES
        .iter()
        .map(|scope| (*scope).to_owned())
        .collect()
}

const fn default_enabled() -> bool {
    true
}

/// A provider as the kind and the rules need it: the part of the row of `identity_providers` that is not the
/// secret. Where a new user lands (the org and the role the provider names) is the API's, not the kind's, and is
/// not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSettings {
    /// The key that appears in the callback address.
    pub key: String,
    /// The label of its button.
    pub name: String,
    /// The issuer; for OpenID Connect its description is found at [`discovery_url`].
    pub issuer: String,
    /// The client id the provider knows this system by.
    pub client_id: String,
    /// What the sign-in asks the provider for; it holds [`REQUIRED_SCOPE`].
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
    /// The names of the claims that say who somebody is.
    #[serde(flatten)]
    pub claims: ClaimNames,
    /// The domains whose addresses may sign in; empty admits any.
    #[serde(default)]
    pub allowed_email_domains: Vec<String>,
    /// Whether a first sign-in makes a user.
    #[serde(default)]
    pub auto_register: bool,
    /// Whether people may sign in through it.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl ProviderSettings {
    /// What is wrong with the settings, member by member, before they are saved; nothing is checked against the
    /// provider (that is [`IdentityProviderKind::validate`]).
    pub fn check(&self) -> Result<(), Vec<FieldError>> {
        let mut faults = Vec::new();
        let mut refuse = |path: &str, message: &str| {
            faults.push(FieldError {
                path: path.to_owned(),
                message: message.to_owned(),
            });
        };
        if !is_valid_key(&self.key) {
            refuse(
                "key",
                "must begin with a letter and hold 2 to 32 lower case letters, digits or dashes",
            );
        }
        if self.name.trim().is_empty() {
            refuse("name", "must not be empty");
        }
        if let Some(fault) = issuer_fault(&self.issuer) {
            refuse("issuer", &fault);
        }
        if self.client_id.trim().is_empty() {
            refuse("client_id", "must not be empty");
        }
        if !self.scopes.iter().any(|scope| scope == REQUIRED_SCOPE) {
            refuse("scopes", "must hold openid");
        }
        if !self.scopes.iter().all(|scope| is_scope_token(scope)) {
            refuse("scopes", "must each be one word without spaces or quotes");
        }
        for (path, claim) in [
            ("claim_subject", &self.claims.subject),
            ("claim_email", &self.claims.email),
            ("claim_name", &self.claims.name),
            ("claim_username", &self.claims.username),
        ] {
            if claim.trim().is_empty() {
                refuse(path, "must not be empty");
            }
        }
        for domain in &self.allowed_email_domains {
            if let Some(fault) = domain_fault(domain) {
                refuse("allowed_email_domains", &format!("`{domain}` {fault}"));
            }
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }
}

/// Whether `key` may name a provider in an address: a lower case letter, then lower case letters, digits and
/// dashes, 2 to 32 characters in all.
#[must_use]
pub fn is_valid_key(key: &str) -> bool {
    let length = u32::try_from(key.len()).unwrap_or(u32::MAX);
    let mut characters = key.chars();
    (KEY_MIN_CHARS..=KEY_MAX_CHARS).contains(&length)
        && characters.next().is_some_and(|c| c.is_ascii_lowercase())
        && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether a scope may be written in the one space-separated list of a request: visible characters, not the
/// double quote or the backslash, and at least one.
fn is_scope_token(scope: &str) -> bool {
    !scope.is_empty()
        && scope
            .chars()
            .all(|c| c.is_ascii_graphic() && c != '"' && c != '\\')
}

fn domain_fault(domain: &str) -> Option<&'static str> {
    if domain.is_empty() {
        Some("must not be empty")
    } else if domain.starts_with('.') || domain.ends_with('.') {
        Some("must not begin or end with a dot")
    } else if !domain
        .chars()
        .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
    {
        Some("must be a domain name, written without an @")
    } else {
        None
    }
}

/// What is wrong with an issuer, if anything. An issuer is an address that names a place on the web, and a
/// safe one: `https`, or `http` for a provider on this very machine (a development server); no user name,
/// query or fragment.
#[must_use]
pub fn issuer_fault(issuer: &str) -> Option<String> {
    let Ok(url) = Url::parse(issuer) else {
        return Some("must be an address".to_owned());
    };
    if url.query().is_some() || url.fragment().is_some() {
        return Some("must not have a query or a fragment".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Some("must not carry a user name".to_owned());
    }
    match (url.scheme(), url.host()) {
        ("https", Some(_)) => None,
        ("http", Some(host)) if host_is_loopback(&host) => None,
        ("http", Some(_)) => {
            Some("must use https; http is for a provider on this machine".to_owned())
        }
        _ => Some("must be an https address".to_owned()),
    }
}

fn host_is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
    }
}

/// Where an issuer describes itself: the issuer with any closing slash taken off and [`DISCOVERY_PATH`] after
/// it.
#[must_use]
pub fn discovery_url(issuer: &str) -> String {
    format!("{}{DISCOVERY_PATH}", issuer.trim_end_matches('/'))
}

/// The address a provider is told to send the browser back to, which is the one to give it when a client is
/// made there: the address of the system, the base path of the API, and the callback of the provider's key.
#[must_use]
pub fn callback_url(public_url: &str, key: &str) -> String {
    format!(
        "{}{API_BASE_PATH}/auth/oidc/{key}/callback",
        public_url.trim_end_matches('/')
    )
}

/// Whether `path` is a place inside the application and nothing else: it begins with one `/` and not `//` or
/// `/\` (which a browser reads as another site), holds no control character (which a browser strips before it
/// reads the rest), and is not longer than [`REDIRECT_MAX_BYTES`].
#[must_use]
pub fn is_in_app_path(path: &str) -> bool {
    let mut characters = path.chars();
    characters.next() == Some('/')
        && !matches!(characters.next(), Some('/' | '\\'))
        && !path.chars().any(char::is_control)
        && u32::try_from(path.len()).is_ok_and(|length| length <= REDIRECT_MAX_BYTES)
}

/// The address to return to: the one asked for where it is a place inside the application, and
/// [`DEFAULT_REDIRECT`] otherwise.
#[must_use]
pub fn clean_redirect(requested: Option<&str>) -> String {
    match requested {
        Some(path) if is_in_app_path(path) => path.to_owned(),
        _ => DEFAULT_REDIRECT.to_owned(),
    }
}

/// What the kind needs again at the callback, made at the start: the three secrets of the request. They are the
/// members `state`, `nonce` and `pkce_verifier` of the stored sign-in; a kind that has no use for one writes an
/// unguessable value there all the same.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PendingSignIn {
    /// Names the sign-in: sent to the provider and brought back by the browser, and the key of the record.
    pub state: String,
    /// Ties the provider's answer to this request; the identity the provider vouches for carries it.
    pub nonce: String,
    /// The secret half of the proof key: the provider is told its hash at the start and the secret itself when
    /// the code is exchanged, so that a stolen code is no use.
    pub pkce_verifier: String,
}

/// What a start gives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignInStart {
    /// Where the browser is sent: the provider's address for signing in, with the request in its query.
    pub redirect_url: String,
    /// What the callback needs again.
    pub pending: PendingSignIn,
}

/// What the API keeps between a start and its callback: a row of `oidc_states`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignInRecord {
    /// The provider it was made for, and the only one whose callback may use it: a record that is brought to
    /// the callback of another provider is refused, so that an answer cannot be replayed where it was not asked.
    pub identity_provider_id: Id,
    /// What the kind made.
    #[serde(flatten)]
    pub pending: PendingSignIn,
    /// Where the browser returns after a sign-in; always a place inside the application.
    pub redirect_path: String,
    /// The signed-in user the identity is to be linked to; none for a sign-in.
    pub link_user_id: Option<Id>,
    /// The invitation the sign-in came with, as the person gave it.
    pub invitation: Option<String>,
    /// When it ends.
    pub expires_at: Time,
}

impl SignInRecord {
    /// A record for a sign-in with the provider `identity_provider_id` that begins `now`. The address to return
    /// to is cleaned ([`clean_redirect`]); an invitation that is only spaces is none.
    #[must_use]
    pub fn begin(
        identity_provider_id: Id,
        pending: PendingSignIn,
        redirect: Option<&str>,
        link_user_id: Option<Id>,
        invitation: Option<&str>,
        now: Time,
    ) -> Self {
        Self {
            identity_provider_id,
            pending,
            redirect_path: clean_redirect(redirect),
            link_user_id,
            invitation: invitation
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(str::to_owned),
            expires_at: Time::new(now.get() + Duration::seconds(SIGN_IN_LIFETIME_SECS)),
        }
    }

    /// Whether the callback may still use it: at the moment it ends it no longer may.
    #[must_use]
    pub fn is_live(&self, now: Time) -> bool {
        now < self.expires_at
    }

    /// Whether the callback of `provider` may use the record at `now`: it is live, and it was made for that
    /// provider. Otherwise the sign-in ends with `state_expired`.
    pub fn check(&self, provider: Id, now: Time) -> Result<(), SignInError> {
        if self.identity_provider_id == provider && self.is_live(now) {
            Ok(())
        } else {
            Err(SignInError::StateExpired)
        }
    }

    /// How the callback ends: a link for a user who was signed in at the start, a sign-in otherwise.
    /// `good_invitation` is the API's verdict on [`invitation`](Self::invitation).
    #[must_use]
    pub fn flow(&self, good_invitation: bool) -> Flow {
        match self.link_user_id {
            Some(user) => Flow::Link { user },
            None => Flow::SignIn { good_invitation },
        }
    }
}

/// What the browser brings to the callback. Providers add members of their own (`iss`, `session_state`, `scope`
/// and others), which are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CallbackParams {
    /// The code to be exchanged for the identity.
    pub code: Option<String>,
    /// Names the sign-in this answers.
    pub state: Option<String>,
    /// What the provider reports when it did not sign the person in.
    pub error: Option<String>,
    /// The provider's words for it; for the log, never for the person.
    pub error_description: Option<String>,
}

impl CallbackParams {
    /// The state the callback names, if it names one.
    #[must_use]
    pub fn state(&self) -> Option<&str> {
        self.state.as_deref().filter(|state| !state.is_empty())
    }

    /// The code to exchange, or the error the sign-in ends with: what the provider reports is `denied` when it is
    /// [`ACCESS_DENIED`] (the person did not agree, or the provider refused them) and `idp_error` when it is
    /// anything else; no code and no report is `idp_error` too. Look at the state first: an answer to no sign-in
    /// that is here is `state_expired` whatever else it says.
    pub fn authorization_code(&self) -> Result<&str, SignInError> {
        if let Some(error) = self.error.as_deref().filter(|error| !error.is_empty()) {
            return Err(if error == ACCESS_DENIED {
                SignInError::Denied
            } else {
                SignInError::IdpError
            });
        }
        self.code
            .as_deref()
            .filter(|code| !code.is_empty())
            .ok_or(SignInError::IdpError)
    }
}

/// What a start is given.
#[derive(Debug, Clone)]
pub struct StartRequest {
    /// The provider.
    pub settings: ProviderSettings,
    /// Where the provider is to send the browser back to ([`callback_url`]).
    pub callback_url: String,
    /// The person is to be signed in again now, whatever session they have at the provider, for one
    /// sensitive act: for OpenID Connect `prompt=login` and `max_age=0`, so that the identity comes back
    /// with the time of that sign-in ([`VerifiedIdentity::auth_time`](crate::claims::VerifiedIdentity)).
    pub fresh: bool,
}

/// What the completion of a sign-in is given.
#[derive(Debug, Clone)]
pub struct CompleteRequest {
    /// The provider.
    pub settings: ProviderSettings,
    /// The client secret, unsealed for this call; none for a client that has none.
    pub client_secret: Option<Secret>,
    /// The address the start gave the provider; a code is good for the address it was issued to.
    pub callback_url: String,
    /// What the start made.
    pub pending: PendingSignIn,
    /// The code the callback brought ([`CallbackParams::authorization_code`]).
    pub code: String,
    /// The moment of the completion, by the system's clock: an identity that ended before it, or that was
    /// issued after it, is not taken.
    pub now: Time,
}

/// Why a kind could not do what it was asked: the provider was not reached, answered with an error, sent an
/// identity that does not pass, or is not set up as it should be. The words are for an administrator reading
/// the answer of **Test**, and for the log; the person is sent `idp_error` and nothing more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{detail}")]
pub struct ProviderError {
    /// What went wrong, in a sentence.
    pub detail: String,
}

impl ProviderError {
    /// An error that says `detail`.
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl From<ProviderError> for SignInError {
    fn from(_: ProviderError) -> Self {
        Self::IdpError
    }
}

/// What a provider says of itself when it is looked at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Discovered {
    /// The issuer it names itself by.
    pub issuer: String,
    /// How many keys it signs with.
    pub signing_keys: i64,
    /// Its description of itself, as it published it.
    pub metadata: Json,
}

impl Discovered {
    /// The `issuer_metadata` of the answer of **Test**: the description, with the number of signing keys beside
    /// it as `signing_keys`, so that the page can say what was found.
    #[must_use]
    pub fn issuer_metadata(&self) -> Json {
        let mut metadata = match &self.metadata {
            Json::Object(members) => members.clone(),
            _ => serde_json::Map::new(),
        };
        metadata.insert("signing_keys".to_owned(), Json::from(self.signing_keys));
        Json::Object(metadata)
    }
}

/// One kind of identity provider, used by the API: `oidc` is the one built in, and no other is loaded in
/// `v0.1.0`; the trait is the seam where another would join.
#[async_trait]
pub trait IdentityProviderKind: Send + Sync + 'static {
    /// The value of `kind` that selects it.
    fn kind(&self) -> &'static str;

    /// What is wrong with the settings of a provider of this kind, before they are saved. It begins with the
    /// rules every kind shares ([`ProviderSettings::check`]) and may add its own.
    fn check(&self, settings: &ProviderSettings) -> Result<(), Vec<FieldError>> {
        settings.check()
    }

    /// Begins a sign-in: makes the request the callback will recognise the answer by, and the address to send
    /// the browser to. Nothing is stored here.
    async fn start(&self, request: &StartRequest) -> Result<SignInStart, ProviderError>;

    /// Ends a sign-in with the code the provider sent, and answers with the identity it vouches for, after the
    /// kind has checked everything that makes the answer the provider's word. For OpenID Connect that is: the
    /// signature, against the keys the provider publishes; the issuer and the audience; that the identity has
    /// not ended; and that its nonce is the pending one. The claims are read with
    /// [`VerifiedIdentity::from_claims`] under the names in the settings.
    async fn complete(&self, request: &CompleteRequest) -> Result<VerifiedIdentity, ProviderError>;

    /// Looks at the provider itself, for **Test**: is it there, does it describe itself, does it name the
    /// issuer it was set up with, what does it sign with. Signs nobody in.
    async fn validate(&self, settings: &ProviderSettings) -> Result<Discovered, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::{Account, Decision, Lookup, RegistrationMode, decide};
    use futures::executor::block_on;
    use serde_json::json;

    fn row() -> Json {
        json!({
            "key": "acme-sso", "name": "Acme SSO", "kind": "oidc",
            "issuer": "https://login.acme.example/realms/main", "client_id": "orch",
            "scopes": ["openid", "profile", "email"],
            "claim_subject": "sub", "claim_email": "email", "claim_name": "name",
            "claim_username": "preferred_username",
            "allowed_email_domains": ["acme.example"], "auto_register": true, "enabled": true
        })
    }

    fn settings() -> ProviderSettings {
        serde_json::from_value(row()).unwrap()
    }

    fn faults(settings: &ProviderSettings) -> Vec<(String, String)> {
        settings
            .check()
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|fault| (fault.path, fault.message))
            .collect()
    }

    #[test]
    fn a_provider_row_is_read_with_the_defaults_of_the_table_for_what_it_leaves_out() {
        let bare: ProviderSettings = serde_json::from_value(json!({
            "key": "g", "name": "G", "issuer": "https://accounts.example", "client_id": "c"
        }))
        .unwrap();
        assert_eq!(bare.scopes, ["openid", "profile", "email"]);
        assert_eq!(bare.claims, ClaimNames::default());
        assert!(bare.allowed_email_domains.is_empty());
        assert!(
            !bare.auto_register,
            "a first sign-in makes nobody unless it is said to"
        );
        assert!(bare.enabled);
        let read = settings();
        assert_eq!(read.key, "acme-sso");
        assert_eq!(read.allowed_email_domains, ["acme.example"]);
        let written = serde_json::to_value(&read).unwrap();
        for member in [
            "key",
            "name",
            "issuer",
            "client_id",
            "scopes",
            "claim_subject",
            "claim_email",
            "claim_name",
            "claim_username",
            "allowed_email_domains",
            "auto_register",
            "enabled",
        ] {
            assert_eq!(written[member], row()[member], "{member}");
        }
    }

    #[test]
    fn settings_that_are_right_pass_and_each_wrong_member_is_named() {
        assert_eq!(settings().check(), Ok(()));
        let mut wrong = settings();
        wrong.key = "Acme".into();
        wrong.name = "  ".into();
        wrong.client_id = String::new();
        wrong.scopes = vec!["profile".into(), "bad scope".into()];
        wrong.claims.email = String::new();
        wrong.allowed_email_domains =
            vec!["@acme.example".into(), "ok.example".into(), String::new()];
        let paths: Vec<String> = faults(&wrong).into_iter().map(|(path, _)| path).collect();
        assert_eq!(
            paths,
            [
                "key",
                "name",
                "client_id",
                "scopes",
                "scopes",
                "claim_email",
                "allowed_email_domains",
                "allowed_email_domains"
            ]
        );
    }

    #[test]
    fn a_key_is_a_letter_then_letters_digits_and_dashes_in_two_to_thirty_two_characters() {
        for key in [
            "ab",
            "acme-sso",
            "a1",
            "a-",
            &format!("a{}", "b".repeat(31)),
        ] {
            assert!(is_valid_key(key), "{key}");
        }
        for key in [
            "",
            "a",
            "1a",
            "-a",
            "Ab",
            "a_b",
            "a.b",
            "ab c",
            "é1",
            &format!("a{}", "b".repeat(32)),
        ] {
            assert!(!is_valid_key(key), "{key}");
        }
    }

    #[test]
    fn an_issuer_is_https_or_a_provider_on_this_machine_and_has_no_query_fragment_or_user() {
        for issuer in [
            "https://login.acme.example",
            "https://login.acme.example/realms/main",
            "https://login.acme.example:8443/",
            "http://localhost:5556/dex",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ] {
            assert_eq!(issuer_fault(issuer), None, "{issuer}");
        }
        for issuer in [
            "",
            "login.acme.example",
            "http://login.acme.example",
            "ftp://login.acme.example",
            "https://login.acme.example?x=1",
            "https://login.acme.example#top",
            "https://me:pw@login.acme.example",
            "https://",
        ] {
            assert!(issuer_fault(issuer).is_some(), "{issuer}");
        }
    }

    #[test]
    fn the_description_of_an_issuer_is_found_after_its_path_without_a_closing_slash() {
        assert_eq!(
            discovery_url("https://login.acme.example"),
            "https://login.acme.example/.well-known/openid-configuration"
        );
        assert_eq!(
            discovery_url("https://login.acme.example/realms/main/"),
            "https://login.acme.example/realms/main/.well-known/openid-configuration"
        );
    }

    #[test]
    fn the_callback_address_is_the_system_the_api_and_the_key() {
        assert_eq!(
            callback_url("https://orch.example.com", "acme-sso"),
            "https://orch.example.com/api/v1/auth/oidc/acme-sso/callback"
        );
        assert_eq!(
            callback_url("https://orch.example.com/", "acme-sso"),
            "https://orch.example.com/api/v1/auth/oidc/acme-sso/callback"
        );
    }

    #[test]
    fn the_address_to_return_to_is_a_place_inside_the_application_or_the_front_page() {
        for good in [
            "/",
            "/projects/7?tab=files#top",
            "/a b",
            "/%2F/evil.example",
            "/x\\y",
        ] {
            assert!(is_in_app_path(good), "{good}");
            assert_eq!(clean_redirect(Some(good)), good);
        }
        let long = format!("/{}", "a".repeat(2047));
        assert!(is_in_app_path(&long), "2048 bytes are the most");
        for bad in [
            "",
            "projects",
            "//evil.example",
            "/\\evil.example",
            "https://evil.example",
            "javascript:alert(1)",
            " /x",
            "/x\ny",
            "/\t/evil.example",
            "/x\u{7f}",
        ] {
            assert!(!is_in_app_path(bad), "{bad:?}");
            assert_eq!(clean_redirect(Some(bad)), "/", "{bad:?}");
        }
        assert!(!is_in_app_path(&format!("{long}a")));
        assert_eq!(clean_redirect(None), "/");
    }

    fn pending() -> PendingSignIn {
        PendingSignIn {
            state: "s-1".into(),
            nonce: "n-1".into(),
            pkce_verifier: "v-1".into(),
        }
    }

    fn at(text: &str) -> Time {
        text.parse().unwrap()
    }

    fn provider() -> Id {
        Id::from_u128(5)
    }

    #[test]
    fn a_sign_in_lives_ten_minutes_and_ends_at_the_moment_it_is_due() {
        let started = at("2026-10-08T17:04:05.123Z");
        let record = SignInRecord::begin(
            provider(),
            pending(),
            Some("/projects"),
            None,
            Some("  "),
            started,
        );
        assert_eq!(record.expires_at, at("2026-10-08T17:14:05.123Z"));
        assert!(record.is_live(started));
        assert!(record.is_live(at("2026-10-08T17:14:05.122Z")));
        assert!(!record.is_live(at("2026-10-08T17:14:05.123Z")));
        assert!(!record.is_live(at("2026-10-08T18:00:00.000Z")));
        assert_eq!(record.redirect_path, "/projects");
        assert_eq!(record.invitation, None, "spaces are no invitation");
        let with_invitation = SignInRecord::begin(
            provider(),
            pending(),
            Some("https://evil.example"),
            None,
            Some(" tok-1 "),
            started,
        );
        assert_eq!(with_invitation.invitation.as_deref(), Some("tok-1"));
        assert_eq!(with_invitation.redirect_path, "/");
    }

    #[test]
    fn a_stored_sign_in_serves_the_callback_of_its_own_provider_while_it_lives_and_no_other() {
        let started = at("2026-10-08T17:04:05.123Z");
        let record = SignInRecord::begin(provider(), pending(), None, None, None, started);
        assert_eq!(
            record.check(provider(), at("2026-10-08T17:10:00.000Z")),
            Ok(())
        );
        assert_eq!(
            record.check(Id::from_u128(6), at("2026-10-08T17:10:00.000Z")),
            Err(SignInError::StateExpired),
            "an answer is not replayed at the callback of another provider"
        );
        assert_eq!(
            record.check(provider(), at("2026-10-08T17:14:05.123Z")),
            Err(SignInError::StateExpired)
        );
    }

    #[test]
    fn a_stored_sign_in_is_the_row_of_the_table_and_says_how_the_callback_ends() {
        let user = Id::from_u128(7);
        let record = SignInRecord::begin(
            provider(),
            pending(),
            None,
            Some(user),
            None,
            at("2026-10-08T17:04:05.123Z"),
        );
        assert_eq!(
            serde_json::to_value(&record).unwrap(),
            json!({
                "identity_provider_id": "00000000-0000-0000-0000-000000000005",
                "state": "s-1", "nonce": "n-1", "pkce_verifier": "v-1", "redirect_path": "/",
                "link_user_id": "00000000-0000-0000-0000-000000000007", "invitation": null,
                "expires_at": "2026-10-08T17:14:05.123Z"
            })
        );
        assert_eq!(record.flow(true), Flow::Link { user });
        let plain = SignInRecord::begin(
            provider(),
            pending(),
            None,
            None,
            Some("tok"),
            at("2026-10-08T17:04:05.123Z"),
        );
        assert_eq!(
            plain.flow(true),
            Flow::SignIn {
                good_invitation: true
            }
        );
        assert_eq!(
            plain.flow(false),
            Flow::SignIn {
                good_invitation: false
            }
        );
    }

    #[test]
    fn what_the_provider_sends_back_is_a_code_or_the_error_the_sign_in_ends_with() {
        let read = |query: Json| -> CallbackParams { serde_json::from_value(query).unwrap() };
        let good = read(
            json!({ "code": "c0de", "state": "s-1", "iss": "https://login.acme.example",
            "session_state": "x" }),
        );
        assert_eq!(good.authorization_code(), Ok("c0de"));
        assert_eq!(good.state(), Some("s-1"));
        let declined =
            read(json!({ "error": "access_denied", "error_description": "no", "state": "s-1" }));
        assert_eq!(declined.authorization_code(), Err(SignInError::Denied));
        for other in [
            json!({ "error": "server_error", "state": "s-1" }),
            json!({ "error": "invalid_scope", "code": "c0de" }),
            json!({ "state": "s-1" }),
            json!({ "code": "", "state": "s-1" }),
        ] {
            assert_eq!(
                read(other.clone()).authorization_code(),
                Err(SignInError::IdpError),
                "{other}"
            );
        }
        assert_eq!(read(json!({ "code": "c" })).state(), None);
        assert_eq!(read(json!({ "code": "c", "state": "" })).state(), None);
    }

    #[test]
    fn what_a_provider_said_when_it_was_looked_at_is_shown_with_the_number_of_its_keys() {
        let found = Discovered {
            issuer: "https://login.acme.example".into(),
            signing_keys: 2,
            metadata: json!({ "issuer": "https://login.acme.example", "jwks_uri": "https://login.acme.example/keys" }),
        };
        assert_eq!(
            found.issuer_metadata(),
            json!({ "issuer": "https://login.acme.example", "jwks_uri": "https://login.acme.example/keys",
                "signing_keys": 2 })
        );
        let bare = Discovered {
            issuer: "i".into(),
            signing_keys: 0,
            metadata: Json::Null,
        };
        assert_eq!(bare.issuer_metadata(), json!({ "signing_keys": 0 }));
    }

    #[test]
    fn a_failure_of_the_provider_is_idp_error_for_the_person_and_words_for_the_administrator() {
        let error = ProviderError::new(
            "Could not read the issuer's discovery document: connection refused",
        );
        assert_eq!(
            error.to_string(),
            "Could not read the issuer's discovery document: connection refused"
        );
        assert_eq!(SignInError::from(error), SignInError::IdpError);
    }

    /// A kind that asks nothing of the world, to walk a sign-in through the trait from its start to its decision.
    struct Scripted;

    #[async_trait]
    impl IdentityProviderKind for Scripted {
        fn kind(&self) -> &'static str {
            KIND_OIDC
        }

        fn check(&self, settings: &ProviderSettings) -> Result<(), Vec<FieldError>> {
            settings.check()
        }

        async fn start(&self, request: &StartRequest) -> Result<SignInStart, ProviderError> {
            let pending = pending();
            let mut redirect = Url::parse("https://login.acme.example/authorize")
                .map_err(|error| ProviderError::new(error.to_string()))?;
            redirect
                .query_pairs_mut()
                .append_pair("client_id", &request.settings.client_id)
                .append_pair("redirect_uri", &request.callback_url)
                .append_pair("scope", &request.settings.scopes.join(" "))
                .append_pair("state", &pending.state)
                .append_pair("nonce", &pending.nonce);
            Ok(SignInStart {
                redirect_url: redirect.into(),
                pending,
            })
        }

        async fn complete(
            &self,
            request: &CompleteRequest,
        ) -> Result<VerifiedIdentity, ProviderError> {
            if request.code != "c0de"
                || request.client_secret.as_ref().map(Secret::expose) != Some("s3cret")
            {
                return Err(ProviderError::new("the code was not accepted"));
            }
            let claims = json!({ "sub": "248289761001", "nonce": request.pending.nonce,
                "email": "dana@acme.example", "email_verified": true, "name": "Dana Whit" });
            VerifiedIdentity::from_claims(&request.settings.claims, &claims)
                .map_err(|error| ProviderError::new(error.to_string()))
        }

        async fn validate(&self, settings: &ProviderSettings) -> Result<Discovered, ProviderError> {
            Ok(Discovered {
                issuer: settings.issuer.clone(),
                signing_keys: 2,
                metadata: json!({ "issuer": settings.issuer }),
            })
        }
    }

    #[test]
    fn a_sign_in_goes_from_its_start_through_the_callback_to_a_decision() {
        let kind: Box<dyn IdentityProviderKind> = Box::new(Scripted);
        let settings = settings();
        assert_eq!(kind.kind(), "oidc");
        assert_eq!(kind.check(&settings), Ok(()));
        let callback = callback_url("https://orch.example.com", &settings.key);

        let started = block_on(kind.start(&StartRequest {
            settings: settings.clone(),
            callback_url: callback.clone(),
            fresh: false,
        }))
        .unwrap();
        let sent = Url::parse(&started.redirect_url).unwrap();
        let query: Vec<(String, String)> = sent
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        assert!(query.contains(&("state".to_owned(), started.pending.state.clone())));
        assert!(query.contains(&("scope".to_owned(), "openid profile email".to_owned())));
        assert!(query.contains(&("redirect_uri".to_owned(), callback.clone())));

        let now = at("2026-10-08T17:04:05.123Z");
        let record = SignInRecord::begin(Id::from_u128(5), started.pending, None, None, None, now);
        let came_back: CallbackParams =
            serde_json::from_value(json!({ "code": "c0de", "state": record.pending.state }))
                .unwrap();
        assert_eq!(came_back.state(), Some(record.pending.state.as_str()));
        assert_eq!(
            record.check(Id::from_u128(5), at("2026-10-08T17:06:00.000Z")),
            Ok(())
        );
        let code = came_back.authorization_code().unwrap();

        let request = CompleteRequest {
            settings: settings.clone(),
            client_secret: Some(Secret::new("s3cret")),
            callback_url: callback,
            pending: record.pending.clone(),
            code: code.to_owned(),
            now: at("2026-10-08T17:06:01.000Z"),
        };
        assert!(
            !format!("{request:?}").contains("s3cret"),
            "a secret is never shown"
        );
        let identity = block_on(kind.complete(&request)).unwrap();
        assert_eq!(identity.subject, "248289761001");
        let wrong = CompleteRequest {
            code: "other".into(),
            ..request.clone()
        };
        assert_eq!(
            block_on(kind.complete(&wrong)).unwrap_err(),
            ProviderError::new("the code was not accepted")
        );

        let nobody = Lookup::default();
        assert_eq!(
            decide(
                &settings,
                RegistrationMode::Open,
                record.flow(false),
                &identity,
                &nobody
            ),
            Ok(Decision::Register)
        );
        let known = Lookup {
            linked: Some(Account {
                user: Id::from_u128(3),
                suspended: false,
            }),
            same_email: None,
        };
        assert_eq!(
            decide(
                &settings,
                RegistrationMode::Closed,
                record.flow(false),
                &identity,
                &known
            ),
            Ok(Decision::SignIn {
                user: Id::from_u128(3)
            })
        );

        let tested = block_on(kind.validate(&settings)).unwrap();
        assert_eq!(tested.signing_keys, 2);
        assert_eq!(tested.issuer_metadata()["signing_keys"], 2);
    }
}
