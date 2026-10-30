//! A signed OpenID Connect provider on the loopback, for the tests of signing in: discovery, keys that rotate, authorization with PKCE, the code exchanged for signed tokens, user information, and the faults a provider makes.
//!
//! [`OidcProvider`] answers as a real provider does, over HTTP on `127.0.0.1`, so that a sign-in is
//! tested whole with no network: its description at `/.well-known/openid-configuration`, its keys at
//! `/keys`, the browser's visit at `/authorize` (which signs in whoever [`OidcProvider::sign_in_as`]
//! names and sends the browser back with a code), the code exchanged at `/token` (one use, the
//! registered callback address, the client's secret by HTTP Basic or in the form, and the PKCE
//! verifier, whose SHA-256 has to be the challenge of the visit), and `/userinfo` for the bearer of an
//! access token it issued. ID tokens are signed with RS256 as shipped, or PS256, ES256, ES384 or EdDSA
//! ([`Alg`]); [`OidcProvider::rotate_keys`] signs with a new key from then on. [`Faults`] makes it
//! issue what a provider should not: another issuer or audience, another nonce, an ended or early
//! token, a token signed with a key it does not publish or not signed at all, the profile only at the
//! user information endpoint, and codes it refuses. Times come from the clock it is given, which is
//! the clock of the system under test.
//!
//! [`TestKey`] signs tokens for tests that need no provider. Its RSA keys are the two kept here, made
//! for these tests alone and used for nothing else; the others are made when asked for.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::extract::{Form, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use http::header::{AUTHORIZATION, LOCATION};
use http::{HeaderMap, HeaderValue, StatusCode};
use orch_types::Clock;
use ring::digest::{SHA256, digest};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, ECDSA_P384_SHA384_FIXED_SIGNING, EcdsaKeyPair, Ed25519KeyPair,
    KeyPair, RSA_PKCS1_SHA256, RSA_PSS_SHA256, RsaEncoding, RsaKeyPair, RsaPublicKeyComponents,
};
use serde_json::{Value as Json, json};

/// The client id the provider knows the system under test by, unless told another.
pub const CLIENT_ID: &str = "orch";
/// The client secret it holds for that client, unless told another.
pub const CLIENT_SECRET: &str = "s3cret";
/// How long an ID token it issues is good for: five minutes, what providers commonly give.
pub const TOKEN_LIFE_SECS: i64 = 300;
/// Where an issuer describes itself, after its address.
pub const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";

/// Two RSA keys of 2048 bits, PKCS #8 in base64, made for these tests alone: the provider signs with
/// one and rotates to the other, and a stray signature is made with the one it does not publish.
const RSA_TEST_KEYS: [&str; 2] = [
    "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",
];

/// The RSA key the next [`TestKey::new`] takes, in turn.
static NEXT_RSA: AtomicUsize = AtomicUsize::new(0);

/// An algorithm the provider signs ID tokens with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alg {
    /// RSA, PKCS #1 v1.5, SHA-256: the one every provider offers, and the one it signs with as shipped.
    Rs256,
    /// RSA, PSS, SHA-256.
    Ps256,
    /// ECDSA on P-256.
    Es256,
    /// ECDSA on P-384.
    Es384,
    /// Ed25519.
    EdDsa,
}

impl Alg {
    /// Its name in a token's header.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rs256 => "RS256",
            Self::Ps256 => "PS256",
            Self::Es256 => "ES256",
            Self::Es384 => "ES384",
            Self::EdDsa => "EdDSA",
        }
    }
}

/// What a key signs with.
#[derive(Debug)]
enum Signer {
    Rsa(RsaKeyPair),
    Ec(EcdsaKeyPair),
    Ed(Ed25519KeyPair),
}

/// A key that signs tokens, with the public half a provider publishes.
#[derive(Debug)]
pub struct TestKey {
    kid: String,
    alg: Alg,
    number: usize,
    signer: Signer,
}

/// A JSON value written as one part of a token.
fn encode_json(value: &Json) -> String {
    URL_SAFE_NO_PAD.encode(value.to_string())
}

/// The bytes of a number without the zeros in front.
fn trimmed(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len());
    bytes.get(start..).unwrap_or_default()
}

impl TestKey {
    /// A key for `alg` called `kid`. RSA keys are taken in turn from the two kept; the others are made
    /// now.
    #[must_use]
    pub fn new(alg: Alg, kid: &str) -> Self {
        Self::numbered(alg, kid, NEXT_RSA.fetch_add(1, Ordering::Relaxed))
    }

    /// A key for `alg` called `kid`, the `number`th: for RSA the kept key of that number (counted round),
    /// so that two numbers in a row are two keys.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "test support: the kept keys and the ones made here are good keys, and a key that cannot be had leaves no test to run"
    )]
    pub fn numbered(alg: Alg, kid: &str, number: usize) -> Self {
        let random = SystemRandom::new();
        let signer = match alg {
            Alg::Rs256 | Alg::Ps256 => {
                let kept = RSA_TEST_KEYS
                    .get(number % RSA_TEST_KEYS.len())
                    .copied()
                    .unwrap_or_default();
                let der = STANDARD.decode(kept).expect("a kept key is base64");
                Signer::Rsa(RsaKeyPair::from_pkcs8(&der).expect("a kept key is an RSA key"))
            }
            Alg::Es256 | Alg::Es384 => {
                let curve = if alg == Alg::Es256 {
                    &ECDSA_P256_SHA256_FIXED_SIGNING
                } else {
                    &ECDSA_P384_SHA384_FIXED_SIGNING
                };
                let made = EcdsaKeyPair::generate_pkcs8(curve, &random).expect("a key is made");
                Signer::Ec(
                    EcdsaKeyPair::from_pkcs8(curve, made.as_ref(), &random)
                        .expect("a key made here is read"),
                )
            }
            Alg::EdDsa => {
                let made = Ed25519KeyPair::generate_pkcs8(&random).expect("a key is made");
                Signer::Ed(
                    Ed25519KeyPair::from_pkcs8(made.as_ref()).expect("a key made here is read"),
                )
            }
        };
        Self {
            kid: kid.to_owned(),
            alg,
            number,
            signer,
        }
    }

    /// What it is called.
    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// What it signs with.
    #[must_use]
    pub fn alg(&self) -> Alg {
        self.alg
    }

    /// Its public half, as a provider publishes it: a JWK with its name, its algorithm and `use: sig`.
    #[must_use]
    pub fn jwk(&self) -> Json {
        let encoded = |bytes: &[u8]| URL_SAFE_NO_PAD.encode(bytes);
        let mut jwk = match &self.signer {
            Signer::Rsa(pair) => {
                let public = RsaPublicKeyComponents::<Vec<u8>>::from(pair.public());
                json!({ "kty": "RSA", "n": encoded(trimmed(&public.n)), "e": encoded(trimmed(&public.e)) })
            }
            Signer::Ec(pair) => {
                // The point written whole: a first byte, then the two coordinates of equal size.
                let point = pair.public_key().as_ref();
                let coordinates = point.get(1..).unwrap_or_default();
                let (x, y) = coordinates.split_at(coordinates.len() / 2);
                let curve = if self.alg == Alg::Es256 {
                    "P-256"
                } else {
                    "P-384"
                };
                json!({ "kty": "EC", "crv": curve, "x": encoded(x), "y": encoded(y) })
            }
            Signer::Ed(pair) => {
                json!({ "kty": "OKP", "crv": "Ed25519", "x": encoded(pair.public_key().as_ref()) })
            }
        };
        jwk["kid"] = json!(self.kid);
        jwk["alg"] = json!(self.alg.name());
        jwk["use"] = json!("sig");
        jwk
    }

    /// A token of `claims` in the compact form, signed, whose header names this key.
    #[must_use]
    pub fn sign(&self, claims: &Json) -> String {
        self.sign_with_header(
            &json!({ "alg": self.alg.name(), "kid": self.kid, "typ": "JWT" }),
            claims,
        )
    }

    /// A token of `claims` under `header`, signed with this key whatever the header says.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "test support: a good key signs what it is given"
    )]
    pub fn sign_with_header(&self, header: &Json, claims: &Json) -> String {
        let input = format!("{}.{}", encode_json(header), encode_json(claims));
        let random = SystemRandom::new();
        let signature: Vec<u8> = match &self.signer {
            Signer::Rsa(pair) => {
                let padding: &'static dyn RsaEncoding = if self.alg == Alg::Ps256 {
                    &RSA_PSS_SHA256
                } else {
                    &RSA_PKCS1_SHA256
                };
                let mut signature = vec![0; pair.public().modulus_len()];
                pair.sign(padding, &random, input.as_bytes(), &mut signature)
                    .expect("an RSA key signs");
                signature
            }
            Signer::Ec(pair) => pair
                .sign(&random, input.as_bytes())
                .expect("an ECDSA key signs")
                .as_ref()
                .to_vec(),
            Signer::Ed(pair) => pair.sign(input.as_bytes()).as_ref().to_vec(),
        };
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
    }
}

/// A token of `claims` that is not signed at all (`alg: none`).
#[must_use]
pub fn unsigned(claims: &Json) -> String {
    format!(
        "{}.{}.",
        encode_json(&json!({ "alg": "none", "typ": "JWT" })),
        encode_json(claims)
    )
}

/// The PKCE challenge of a verifier: its SHA-256 in base64url.
#[must_use]
pub fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(digest(&SHA256, verifier.as_bytes()))
}

/// Who signs in at the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    /// Who they are at the provider (`sub`).
    pub subject: String,
    /// Their address.
    pub email: Option<String>,
    /// Whether the provider says it confirmed the address.
    pub email_verified: bool,
    /// The name to show.
    pub name: Option<String>,
    /// The name to sign in with (`preferred_username`).
    pub username: Option<String>,
}

impl Person {
    /// Somebody known only by `subject`.
    #[must_use]
    pub fn new(subject: &str) -> Self {
        Self {
            subject: subject.to_owned(),
            email: None,
            email_verified: false,
            name: None,
            username: None,
        }
    }

    /// The same person with an address, confirmed by the provider or not.
    #[must_use]
    pub fn with_email(mut self, email: &str, verified: bool) -> Self {
        self.email = Some(email.to_owned());
        self.email_verified = verified;
        self
    }

    /// The same person with a name to show.
    #[must_use]
    pub fn named(mut self, name: &str) -> Self {
        self.name = Some(name.to_owned());
        self
    }

    /// The same person with a username.
    #[must_use]
    pub fn with_username(mut self, username: &str) -> Self {
        self.username = Some(username.to_owned());
        self
    }
}

/// What the provider does wrong on purpose in what it issues from now on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Faults {
    /// The issuer its ID tokens name instead of its own.
    pub issuer: Option<String>,
    /// The audience its ID tokens name instead of the client.
    pub audience: Option<Json>,
    /// The nonce its ID tokens carry instead of the one the visit brought.
    pub nonce: Option<String>,
    /// How many seconds before now its ID tokens say they were issued.
    pub issued_ago: i64,
    /// How many seconds after they were issued its ID tokens end, in place of [`TOKEN_LIFE_SECS`].
    pub lifetime: Option<i64>,
    /// How many seconds before the visit its ID tokens say the person last signed in.
    pub auth_time_ago: i64,
    /// Its ID tokens say nothing of when the person signed in.
    pub no_auth_time: bool,
    /// Its ID tokens are signed with a key it does not publish, under the name of the one it does.
    pub unpublished_key: bool,
    /// Its ID tokens are not signed.
    pub unsigned: bool,
    /// Its ID tokens carry no address, name or username: those are told at the user information
    /// endpoint alone.
    pub profile_at_user_info: bool,
    /// Its token endpoint refuses every code.
    pub refuse_codes: bool,
}

/// How the provider is set up.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// The one client it knows.
    pub client_id: String,
    /// That client's secret; none for a public client, which names itself in the form.
    pub client_secret: Option<String>,
    /// The one callback address it sends the browser back to.
    pub redirect_uri: String,
    /// What it signs ID tokens with.
    pub alg: Alg,
    /// Where its time comes from: the clock of the system under test.
    pub clock: Arc<dyn Clock>,
}

impl ProviderConfig {
    /// A provider that knows the client [`CLIENT_ID`] with [`CLIENT_SECRET`], sends the browser back to
    /// `redirect_uri`, signs with RS256 and tells the time by `clock`.
    #[must_use]
    pub fn new(redirect_uri: &str, clock: Arc<dyn Clock>) -> Self {
        Self {
            client_id: CLIENT_ID.to_owned(),
            client_secret: Some(CLIENT_SECRET.to_owned()),
            redirect_uri: redirect_uri.to_owned(),
            alg: Alg::Rs256,
            clock,
        }
    }
}

/// What a visit to `/authorize` asked, by parameter.
pub type Asked = BTreeMap<String, String>;

/// A code that was given and not yet exchanged.
#[derive(Debug, Clone)]
struct Grant {
    person: Person,
    nonce: Option<String>,
    challenge: String,
    redirect_uri: String,
    /// When the person signed in, in whole seconds.
    at: i64,
}

/// What the provider changes as it runs.
#[derive(Debug)]
struct Inner {
    published: Vec<Arc<TestKey>>,
    signing: Arc<TestKey>,
    keys_made: usize,
    person: Person,
    refuse_next: bool,
    faults: Faults,
    grants: HashMap<String, Grant>,
    access: HashMap<String, Person>,
    asked: Vec<Asked>,
    key_reads: usize,
    tokens_issued: usize,
}

#[derive(Debug)]
struct Shared {
    issuer: String,
    config: ProviderConfig,
    inner: Mutex<Inner>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A running provider; it stops when this is dropped.
#[derive(Debug)]
pub struct OidcProvider {
    address: SocketAddr,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl OidcProvider {
    /// Starts a provider on a free port of the loopback. It signs in `Person::new("subject-1")` until
    /// told whom.
    pub async fn start(config: ProviderConfig) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let address = listener.local_addr()?;
        let first = Arc::new(TestKey::numbered(config.alg, "key-1", 0));
        let shared = Arc::new(Shared {
            issuer: format!("http://{address}"),
            inner: Mutex::new(Inner {
                published: vec![Arc::clone(&first)],
                signing: first,
                keys_made: 1,
                person: Person::new("subject-1"),
                refuse_next: false,
                faults: Faults::default(),
                grants: HashMap::new(),
                access: HashMap::new(),
                asked: Vec::new(),
                key_reads: 0,
                tokens_issued: 0,
            }),
            config,
        });
        let app = axum::Router::new()
            .route(DISCOVERY_PATH, get(discovery))
            .route("/keys", get(keys))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route("/userinfo", get(user_info))
            .with_state(Arc::clone(&shared));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            address,
            shared,
            task,
        })
    }

    /// Its issuer: its address, such as `http://127.0.0.1:41500`.
    #[must_use]
    pub fn issuer(&self) -> String {
        self.shared.issuer.clone()
    }

    /// The address it listens on.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.address
    }

    /// Signs `person` in at every visit from now on.
    pub fn sign_in_as(&self, person: Person) {
        self.shared.lock().person = person;
    }

    /// Refuses the next visit as a person who did not agree would (`access_denied`).
    pub fn refuse_next(&self) {
        self.shared.lock().refuse_next = true;
    }

    /// Makes the provider do `faults` from now on.
    pub fn set_faults(&self, faults: Faults) {
        self.shared.lock().faults = faults;
    }

    /// Signs with a new key from now on, which it publishes; the old ones stay published where
    /// `keep_old`. The new key's name is answered.
    pub fn rotate_keys(&self, keep_old: bool) -> String {
        let mut inner = self.shared.lock();
        inner.keys_made += 1;
        let number = inner.keys_made;
        let kid = format!("key-{number}");
        let key = Arc::new(TestKey::numbered(self.shared.config.alg, &kid, number - 1));
        if !keep_old {
            inner.published.clear();
        }
        inner.published.push(Arc::clone(&key));
        inner.signing = key;
        kid
    }

    /// How many times its keys were read.
    #[must_use]
    pub fn key_reads(&self) -> usize {
        self.shared.lock().key_reads
    }

    /// How many ID tokens it issued.
    #[must_use]
    pub fn tokens_issued(&self) -> usize {
        self.shared.lock().tokens_issued
    }

    /// What each visit to `/authorize` asked, in order.
    #[must_use]
    pub fn asked(&self) -> Vec<Asked> {
        self.shared.lock().asked.clone()
    }

    /// Goes where a browser is sent to sign in (`address`, at this provider) and answers the address the
    /// provider sends it back to: the callback, with a code or an error.
    pub async fn authorize(&self, address: &str) -> Result<String, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        let response = client
            .get(address)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if response.status() != StatusCode::FOUND {
            return Err(format!("the provider answered {}", response.status()));
        }
        response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| "the provider sent the browser nowhere".to_owned())
    }
}

impl Drop for OidcProvider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A random value for a code or a token.
fn random_value() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

/// A redirect to `location`.
fn found(location: &str) -> Response {
    match HeaderValue::from_str(location) {
        Ok(value) => (StatusCode::FOUND, [(LOCATION, value)]).into_response(),
        Err(_) => (StatusCode::BAD_REQUEST, "an address that cannot be sent").into_response(),
    }
}

/// An error of the token or user information endpoint, as OAuth writes it.
fn oauth_error(status: StatusCode, error: &str) -> Response {
    (status, axum::Json(json!({ "error": error }))).into_response()
}

/// What the provider tells of `person`: their address with whether it is confirmed, their name and
/// their username, where it has them.
fn profile(person: &Person, claims: &mut Json) {
    if let Some(email) = &person.email {
        claims["email"] = json!(email);
        claims["email_verified"] = json!(person.email_verified);
    }
    if let Some(name) = &person.name {
        claims["name"] = json!(name);
    }
    if let Some(username) = &person.username {
        claims["preferred_username"] = json!(username);
    }
}

async fn discovery(State(shared): State<Arc<Shared>>) -> axum::Json<Json> {
    let issuer = &shared.issuer;
    axum::Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/keys"),
        "userinfo_endpoint": format!("{issuer}/userinfo"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": [shared.config.alg.name()],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": ["openid", "profile", "email"],
        "claims_supported": ["sub", "email", "email_verified", "name", "preferred_username", "auth_time", "nonce"]
    }))
}

async fn keys(State(shared): State<Arc<Shared>>) -> axum::Json<Json> {
    let mut inner = shared.lock();
    inner.key_reads += 1;
    let published: Vec<Json> = inner.published.iter().map(|key| key.jwk()).collect();
    axum::Json(json!({ "keys": published }))
}

async fn authorize(
    State(shared): State<Arc<Shared>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let config = &shared.config;
    let mut inner = shared.lock();
    inner.asked.push(query.clone().into_iter().collect());
    let asked = |name: &str| query.get(name).map(String::as_str);
    // Nobody is sent back to an address the client did not register.
    if asked("client_id") != Some(config.client_id.as_str())
        || asked("redirect_uri") != Some(config.redirect_uri.as_str())
    {
        return (
            StatusCode::BAD_REQUEST,
            "unknown client or callback address",
        )
            .into_response();
    }
    let state = asked("state").unwrap_or_default().to_owned();
    let back = |pairs: &[(&str, &str)]| {
        let mut query = form_urlencoded::Serializer::new(String::new());
        query.extend_pairs(pairs);
        let joint = if config.redirect_uri.contains('?') {
            '&'
        } else {
            '?'
        };
        found(&format!("{}{joint}{}", config.redirect_uri, query.finish()))
    };
    let opens = asked("scope").is_some_and(|scope| scope.split(' ').any(|each| each == "openid"));
    let challenge =
        asked("code_challenge").filter(|_| asked("code_challenge_method") == Some("S256"));
    let refused = if asked("response_type") != Some("code") {
        Some("unsupported_response_type")
    } else if !opens {
        Some("invalid_scope")
    } else if challenge.is_none() {
        Some("invalid_request")
    } else if inner.refuse_next {
        inner.refuse_next = false;
        Some("access_denied")
    } else {
        None
    };
    if let Some(error) = refused {
        return back(&[("error", error), ("state", state.as_str())]);
    }
    let code = random_value();
    let grant = Grant {
        person: inner.person.clone(),
        nonce: asked("nonce").map(str::to_owned),
        challenge: challenge.unwrap_or_default().to_owned(),
        redirect_uri: config.redirect_uri.clone(),
        at: config.clock.now().timestamp(),
    };
    inner.grants.insert(code.clone(), grant);
    back(&[("code", code.as_str()), ("state", state.as_str())])
}

/// Who the client of a token request says it is: by HTTP Basic, whose two parts are each written as a
/// form writes a value, or in the form.
fn client_of(
    headers: &HeaderMap,
    form: &HashMap<String, String>,
) -> Option<(String, Option<String>)> {
    let decoded = |text: &str| {
        form_urlencoded::parse(format!("v={text}").as_bytes())
            .next()
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default()
    };
    if let Some(basic) = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Basic "))
    {
        let text = String::from_utf8(STANDARD.decode(basic).ok()?).ok()?;
        let (id, secret) = text.split_once(':')?;
        return Some((decoded(id), Some(decoded(secret))));
    }
    Some((
        form.get("client_id")?.clone(),
        form.get("client_secret").cloned(),
    ))
}

async fn token(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let config = &shared.config;
    let known = client_of(&headers, &form)
        .is_some_and(|(id, secret)| id == config.client_id && secret == config.client_secret);
    if !known {
        return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client");
    }
    if form.get("grant_type").map(String::as_str) != Some("authorization_code") {
        return oauth_error(StatusCode::BAD_REQUEST, "unsupported_grant_type");
    }
    let mut inner = shared.lock();
    // A code is good once, whatever comes of its exchange.
    let Some(grant) = form.get("code").and_then(|code| inner.grants.remove(code)) else {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant");
    };
    let proven = form
        .get("code_verifier")
        .is_some_and(|verifier| challenge_of(verifier) == grant.challenge);
    if inner.faults.refuse_codes || form.get("redirect_uri") != Some(&grant.redirect_uri) || !proven
    {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant");
    }
    let faults = inner.faults.clone();
    let issued = config.clock.now().timestamp() - faults.issued_ago;
    let mut claims = json!({
        "iss": faults.issuer.clone().unwrap_or_else(|| shared.issuer.clone()),
        "sub": grant.person.subject,
        "aud": faults.audience.clone().unwrap_or_else(|| json!(config.client_id)),
        "iat": issued,
        "exp": issued + faults.lifetime.unwrap_or(TOKEN_LIFE_SECS)
    });
    if let Some(nonce) = faults.nonce.clone().or_else(|| grant.nonce.clone()) {
        claims["nonce"] = json!(nonce);
    }
    if !faults.no_auth_time {
        claims["auth_time"] = json!(grant.at - faults.auth_time_ago);
    }
    if !faults.profile_at_user_info {
        profile(&grant.person, &mut claims);
    }
    let id_token = if faults.unsigned {
        unsigned(&claims)
    } else if faults.unpublished_key {
        // Another key, under the name of the one it publishes.
        let signing = &inner.signing;
        let stray = TestKey::numbered(config.alg, signing.kid(), signing.number + 1);
        stray.sign(&claims)
    } else {
        inner.signing.sign(&claims)
    };
    let access = random_value();
    inner.access.insert(access.clone(), grant.person.clone());
    inner.tokens_issued += 1;
    axum::Json(json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": TOKEN_LIFE_SECS,
        "id_token": id_token,
        "scope": "openid profile email"
    }))
    .into_response()
}

async fn user_info(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);
    let inner = shared.lock();
    let Some(person) = bearer.and_then(|token| inner.access.get(&token).cloned()) else {
        return oauth_error(StatusCode::UNAUTHORIZED, "invalid_token");
    };
    let mut claims = json!({ "sub": person.subject });
    profile(&person, &mut claims);
    axum::Json(claims).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_kept_rsa_keys_are_two_keys_and_sign() {
        let first = TestKey::numbered(Alg::Rs256, "a", 0);
        let second = TestKey::numbered(Alg::Rs256, "b", 1);
        assert_ne!(first.jwk()["n"], second.jwk()["n"]);
        assert_eq!(first.jwk()["e"], "AQAB");
        let token = first.sign(&json!({ "sub": "x" }));
        assert_eq!(token.split('.').count(), 3);
    }

    #[test]
    fn a_challenge_is_the_sha256_of_the_verifier() {
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
