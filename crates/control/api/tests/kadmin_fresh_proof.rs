//! A person without a password proves their linked sign-in again for one act: the provider is asked to sign them in now, the callback opens no session and sends a one-use proof back in the fragment, and the proof makes one token in that session, once, while an old sign-in or another person's identity gives none.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use async_trait::async_trait;
use chrono::Duration;
use orch_api_contract::auth::{
    ApiTokenCreateBody, AuthOidcCallback, AuthOidcCallbackQuery, AuthOidcStart, AuthOidcStartQuery,
    MeTokenCreate, SensitiveProof, SensitivePurpose,
};
use orch_api_contract::common::ErrorCode;
use orch_api_core::middleware::authentication::{
    ACCESS_TOKEN_PREFIX, SESSION_PREFIX, new_token, token_hash,
};
use orch_api_core::routes::auth::oidc::install_kind;
use orch_identity_contract::claims::VerifiedIdentity;
use orch_identity_contract::provider::{
    CompleteRequest, Discovered, IdentityProviderKind, KIND_OIDC, PendingSignIn, ProviderError,
    ProviderSettings, SignInStart, StartRequest,
};
use orch_types::{Id, Time, new_id};
use std::sync::{Arc, Mutex};
use support::World;

/// A provider that signs whoever it is asked about in at the moment its code names (`at:<seconds>`), as
/// the identity `sub-<subject>` the code ends with, and remembers whether it was asked for a fresh sign-in.
struct Scripted {
    asked_fresh: Mutex<Vec<bool>>,
}

#[async_trait]
impl IdentityProviderKind for Scripted {
    fn kind(&self) -> &'static str {
        KIND_OIDC
    }

    async fn start(&self, request: &StartRequest) -> Result<SignInStart, ProviderError> {
        self.asked_fresh.lock().unwrap().push(request.fresh);
        let state = new_id().to_string();
        Ok(SignInStart {
            redirect_url: format!(
                "https://idp.example/authorize?state={state}&prompt=login&max_age=0"
            ),
            pending: PendingSignIn {
                state,
                nonce: new_id().to_string(),
                pkce_verifier: new_id().to_string(),
            },
        })
    }

    async fn complete(&self, request: &CompleteRequest) -> Result<VerifiedIdentity, ProviderError> {
        let (at, subject) = request
            .code
            .strip_prefix("at:")
            .and_then(|rest| rest.split_once(':'))
            .ok_or_else(|| ProviderError::new("the code is not one this provider gave"))?;
        let seconds: i64 = at.parse().map_err(|_| ProviderError::new("no time"))?;
        Ok(VerifiedIdentity {
            subject: subject.to_owned(),
            email: None,
            email_verified: false,
            name: None,
            username: None,
            auth_time: chrono::DateTime::from_timestamp(seconds, 0).map(Time::new),
        })
    }

    async fn validate(&self, settings: &ProviderSettings) -> Result<Discovered, ProviderError> {
        Ok(Discovered {
            issuer: settings.issuer.clone(),
            signing_keys: 1,
            metadata: serde_json::json!({}),
        })
    }
}

/// A person who signs in only through the provider `corp`, linked as `sub-ada`, with a browser session:
/// its cookie and its access token.
struct Passwordless {
    user: Id,
    cookie: String,
    access: String,
}

async fn passwordless(world: &World) -> Passwordless {
    install_kind(Arc::new(Scripted {
        asked_fresh: Mutex::new(Vec::new()),
    }));
    let person = world.person("ada").await;
    let provider = new_id();
    let session = new_id();
    let (cookie, access) = (new_token(SESSION_PREFIX), new_token(ACCESS_TOKEN_PREFIX));
    let pool = &world.db.pool;
    sqlx::query("UPDATE users SET password_hash = NULL WHERE id = $1")
        .bind(person.id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO identity_providers (id, key, name, issuer, client_id) VALUES ($1, 'corp', 'Corp', 'https://idp.example', 'orch')",
    )
    .bind(provider)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_identities (identity_provider_id, subject, user_id) VALUES ($1, 'sub-ada', $2)")
        .bind(provider)
        .bind(person.id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, refresh_hash, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(session)
    .bind(person.id)
    .bind(token_hash(&cookie))
    .bind(world.now() + Duration::days(30))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_tokens (token_hash, session_id, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(token_hash(&access))
    .bind(session)
    .bind(world.now() + Duration::days(30))
    .execute(pool)
    .await
    .unwrap();
    Passwordless {
        user: person.id,
        cookie,
        access,
    }
}

/// Starts a fresh proof for making a token from the form at `/settings/tokens`; the state it began, and
/// the mark it gave the browser, which alone may end it.
async fn start(world: &World, ada: &Passwordless) -> (String, String) {
    let reply = world
        .app
        .call(AuthOidcStart, &[&"corp"])
        .query(&AuthOidcStartQuery {
            accept_invitation: false,
            redirect: Some("/settings/tokens".to_owned()),
            sensitive_purpose: Some(SensitivePurpose::TokenCreate),
            ..AuthOidcStartQuery::default()
        })
        .header("cookie", &format!("orch_session={}", ada.cookie))
        .send_bytes()
        .await;
    assert_eq!(reply.status, 302);
    let to = reply.headers["location"].to_str().unwrap().to_owned();
    assert!(to.starts_with("https://idp.example/authorize?"), "{to}");
    let mark = support::cookie(&reply.headers, "orch_oidc").unwrap().value;
    let state = url::Url::parse(&to)
        .unwrap()
        .query_pairs()
        .find(|(name, _)| name == "state")
        .map(|(_, value)| value.into_owned())
        .unwrap();
    (state, mark)
}

/// Comes back from the provider with `code`, in the browser marked `mark`; where the browser is sent.
async fn come_back(world: &World, state: &str, mark: &str, code: &str) -> String {
    let reply = world
        .app
        .call(AuthOidcCallback, &[&"corp"])
        .query(&AuthOidcCallbackQuery {
            code: Some(code.to_owned()),
            state: Some(state.to_owned()),
            ..AuthOidcCallbackQuery::default()
        })
        .header("cookie", &format!("orch_oidc={mark}"))
        .send_bytes()
        .await;
    assert_eq!(reply.status, 302);
    assert!(
        reply.headers.get("set-cookie").is_none(),
        "no session is opened"
    );
    reply.headers["location"].to_str().unwrap().to_owned()
}

fn token_with(proof: &str) -> ApiTokenCreateBody {
    ApiTokenCreateBody {
        name: "deploy".to_owned(),
        scopes: Some(vec!["thread.view".to_owned()]),
        node_id: None,
        expires_at: None,
        proof: SensitiveProof {
            identity_proof: Some(proof.to_owned()),
            ..SensitiveProof::default()
        },
    }
}

#[tokio::test]
async fn a_fresh_sign_in_at_the_linked_provider_gives_one_proof_that_makes_one_token() {
    let world = World::new().await;
    let ada = passwordless(&world).await;
    let (state, mark) = start(&world, &ada).await;
    let now = world.now().timestamp();
    let back = come_back(&world, &state, &mark, &format!("at:{now}:sub-ada")).await;
    let proof = back
        .strip_prefix("/settings/tokens#identity_proof=")
        .unwrap_or_else(|| panic!("{back}"))
        .to_owned();
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sensitive_proofs WHERE proof_hash = $1")
            .bind(token_hash(&proof))
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(stored, 1, "only its hash is kept");

    let made = world
        .app
        .call(MeTokenCreate, &[])
        .token(&ada.access)
        .body(&token_with(&proof))
        .send()
        .await
        .ok();
    assert!(made.token.is_some());
    // Spent: it makes no second token, and the callback's state served once.
    let again = world
        .app
        .call(MeTokenCreate, &[])
        .token(&ada.access)
        .body(&token_with(&proof))
        .send()
        .await
        .problem();
    assert_eq!(again.code, ErrorCode::AuthReauthenticationRequired);
    let replayed = come_back(&world, &state, &mark, &format!("at:{now}:sub-ada")).await;
    assert_eq!(replayed, "/login?error=state_expired");
    let _ = ada.user;
}

#[tokio::test]
async fn an_older_sign_in_or_another_persons_identity_gives_no_proof() {
    let world = World::new().await;
    let ada = passwordless(&world).await;
    let (state, mark) = start(&world, &ada).await;
    let before = (world.now() - Duration::minutes(10)).timestamp();
    let stale = come_back(&world, &state, &mark, &format!("at:{before}:sub-ada")).await;
    assert_eq!(stale, "/settings/tokens#identity_proof_error=not_fresh");

    let (state, mark) = start(&world, &ada).await;
    let now = world.now().timestamp();
    let other = come_back(&world, &state, &mark, &format!("at:{now}:sub-bo")).await;
    assert_eq!(other, "/settings/tokens#identity_proof_error=denied");
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM sensitive_proofs")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(kept, 0);
}
