//! Resolve sessions and scoped tokens.
//!
//! A caller is nobody, a person in a browser session, or an API token
//! acting for its user. A browser sends the short-lived access token it
//! was given at sign-in; automation sends an API token. Both are looked up
//! by their SHA-256 on every request, with the user's row, so that a
//! revoked session or a suspended user is refused at once.

use super::requests::ApiError;

use super::requests::ApiResult;

use crate::state::AppState;

use chrono::DateTime;

use chrono::Utc;

use http::HeaderMap;

use http::header;

use orch_access_contract::key_scope::KeyScope;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::common::{SessionEnd, SessionEndCause};

use orch_db::queries::identity;

use orch_db::queries::identity::UserRow;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use sha2::Digest;

use sha2::Sha256;

use std::sync::Arc;

#[path = "authentication/api_key.rs"]
mod api_key;
pub use api_key::new_api_key;
pub use native_read::NativeReadIdentity;
/// What an access token begins with.
pub const ACCESS_TOKEN_PREFIX: &str = orch_api_contract::surface::BROWSER_TOKEN_PREFIX;

/// What a session cookie's value begins with.
pub const SESSION_PREFIX: &str = "ors_";

/// What an API token begins with.
pub const API_TOKEN_PREFIX: &str = orch_api_contract::surface::KEY_TOKEN_PREFIX;

/// How a caller proved who they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// A sealed native owner-only read; this is neither HTTP credential kind.
    OwnerRead { source: NativeReadIdentity },
    /// A browser session.
    Session {
        /// The session.
        session_id: Id,
    },
    /// An API token.
    Token {
        /// The token.
        token_id: Id,
        /// The permissions it may exercise; `*` is all its user holds.
        scopes: Vec<String>,
        /// The node it is confined to, with everything below.
        node_id: Option<Id>,
        /// Explicit operation policy, including retained issuance ceilings; None is legacy.
        operation_scope: Option<KeyScope>,
        /// Current deadline retained by accepted-work capture.
        expires_at: Option<Time>,
    },
}

/// Decodes only credential restriction data. Invalid stored policy is never a legacy fallback.
pub(crate) fn operation_scope(raw: Option<Json>) -> ApiResult<Option<KeyScope>> {
    let scope: Option<KeyScope> = raw
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| invalid_token())?;
    if scope
        .as_ref()
        .is_some_and(|scope| !scope.validate().is_empty())
    {
        return Err(invalid_token());
    }
    Ok(scope)
}

/// A signed-in caller.
#[derive(Debug, Clone, PartialEq)]
pub struct Authed {
    /// The exact browser session's explicit administrator mode; false for tokens.
    pub as_admin: bool,
    /// The user.
    pub user: UserRow,
    /// How they proved it.
    pub via: Via,
}

impl Authed {
    /// The user's identifier.
    #[must_use]
    pub fn id(&self) -> Id {
        self.user.id
    }

    /// Whether this is a browser session.
    #[must_use]
    pub fn is_session(&self) -> bool {
        matches!(self.via, Via::Session { .. })
    }

    /// Whether the caller acts as an admin of the system: what a user
    /// holds as one counts in a browser session only, never by a token.
    #[must_use]
    pub fn is_system_admin(&self) -> bool {
        self.user.is_system_admin && self.is_session() && self.as_admin
    }
}

/// Who makes a request.
#[derive(Debug, Clone, PartialEq)]
pub enum Caller {
    /// Nobody who is signed in.
    Anonymous,
    /// A signed-in caller.
    User(Arc<Authed>),
}

/// The SHA-256 of a token, which is what is stored of it.
#[must_use]
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Make a bearer for its credential family. Account-key issuers use `new_api_key` to retain its ID.
#[must_use]
pub fn new_token(prefix: &str) -> String {
    if prefix == API_TOKEN_PREFIX {
        return new_api_key().1;
    }
    use base64::Engine as _;
    let bytes: [u8; 32] = rand::random();
    format!(
        "{prefix}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

/// A new API key names its actual stored identity and carries 256 random secret bits.
#[must_use]
pub fn new_api_token(id: Id) -> String {
    orch_api_contract::api_keys::encode(id, &rand::random::<[u8; 32]>())
}

/// The bearer token of a request, where it carries one.
#[must_use]
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// The value of the cookie `name`, where the request carries it.
#[must_use]
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then_some(value))
}

/// Rejects a foreign credential kind before any public/cookie shortcut or business effect.
/// Ticket bodies, enrollment proofs and the port-host protocol are not bearer credentials here.
pub fn on_surface(
    surface: orch_api_contract::surface::HttpSurface,
    headers: &HeaderMap,
) -> ApiResult<()> {
    checked_bearer(Some(surface), headers).map(|_| ())
}

fn checked_bearer(
    surface: Option<orch_api_contract::surface::HttpSurface>,
    headers: &HeaderMap,
) -> ApiResult<Option<(&str, Option<Id>)>> {
    use orch_api_contract::surface::HttpSurface;
    if surface == Some(HttpSurface::Key)
        && cookie(headers, orch_api_contract::common::SESSION_COOKIE).is_some()
    {
        return Err(invalid_token());
    }
    let Some(token) = bearer(headers) else {
        return Ok(None);
    };
    let account_key = token.starts_with(API_TOKEN_PREFIX);
    if surface.is_some_and(|plane| (plane == HttpSurface::Key) != account_key) {
        return Err(invalid_token());
    }
    let id = if account_key {
        Some(orch_api_contract::api_keys::key_id(token).ok_or_else(invalid_token)?)
    } else {
        None
    };
    Ok(Some((token, id)))
}

/// Resolve a credential on the surface selected by the server.
pub async fn authenticate_on(
    surface: orch_api_contract::surface::HttpSurface,
    app: &AppState,
    headers: &HeaderMap,
    now: DateTime<Utc>,
) -> ApiResult<Caller> {
    resolve(app, checked_bearer(Some(surface), headers)?, now).await
}

/// Resolve a bearer directly, applying the same offline format checks as the HTTP mounts.
pub async fn authenticate(
    app: &AppState,
    headers: &HeaderMap,
    now: DateTime<Utc>,
) -> ApiResult<Caller> {
    resolve(app, checked_bearer(None, headers)?, now).await
}

async fn resolve(
    app: &AppState,
    token: Option<(&str, Option<Id>)>,
    now: DateTime<Utc>,
) -> ApiResult<Caller> {
    let Some((token, key_id)) = token else {
        return Ok(Caller::Anonymous);
    };
    let hash = token_hash(token);
    let mut conn = app.db.acquire().await?;
    let (user_id, via, as_admin) = if let Some(key_id) = key_id {
        let row = identity::api_token_by_identity(&mut conn, key_id, &hash, now)
            .await?
            .ok_or_else(invalid_token)?;
        let scope = operation_scope(row.operation_scope)?;
        let expires_at = row.expires_at.map(Time::new);
        let _ = orch_authz::TokenGrant::from_stored_scope(
            &row.scopes,
            row.node_id,
            scope.clone(),
            expires_at,
        )
        .map_err(|_| invalid_token())?;
        (
            row.user_id,
            Via::Token {
                token_id: row.id,
                scopes: row.scopes,
                node_id: row.node_id,
                operation_scope: scope,
                expires_at,
            },
            false,
        )
    } else {
        let row = match identity::session_by_access_token(&mut conn, &hash, now).await? {
            Some(row) => row,
            None => {
                let mut error = invalid_token();
                // Only an exact retained bearer match may add a fact. Access-token expiry
                // alone still asks the live session to renew through its ordinary cookie.
                if let Some(proof) =
                    identity::session_proof_by_access_hash(&mut conn, &hash).await?
                {
                    if let Some(fact) = recorded_session_end(&proof, now) {
                        error = error.session_ended(fact);
                    } else if identity::user_by_id(&mut conn, proof.user_id)
                        .await?
                        .is_some_and(|user| user.suspended())
                    {
                        error = error.session_ended(unavailable_session());
                    }
                }
                return Err(error);
            }
        };
        (
            row.user_id,
            Via::Session {
                session_id: row.session_id,
            },
            row.as_admin,
        )
    };
    let user = identity::user_by_id(&mut conn, user_id)
        .await?
        .ok_or_else(invalid_token)?;
    if user.suspended() {
        let error = ApiError::new(ErrorCode::AuthSuspended, "this user is suspended");
        return Err(if matches!(&via, Via::Session { .. }) {
            error.session_ended(unavailable_session())
        } else {
            error
        });
    }
    // The handler can reuse this established connection immediately after authentication.
    conn.return_to_pool().await;
    Ok(Caller::User(Arc::new(Authed {
        user,
        via,
        as_admin,
    })))
}

/// The first retained session ending wins; cleanup or a later observation supplies no time.
pub fn recorded_session_end(
    proof: &identity::SessionProof,
    now: DateTime<Utc>,
) -> Option<SessionEnd> {
    let expired = (proof.expires_at <= now).then_some(proof.expires_at);
    if let Some(revoked) = proof.revoked_at
        && expired.is_none_or(|end| revoked < end)
    {
        return Some(SessionEnd {
            cause: SessionEndCause::Revoked,
            at_if_known: Some(Time::new(revoked)),
        });
    }
    expired.map(|end| SessionEnd {
        cause: SessionEndCause::Expired,
        at_if_known: Some(Time::new(end)),
    })
}

/// Account standing has no captured change instant, so it cannot supply one.
pub fn unavailable_session() -> SessionEnd {
    SessionEnd {
        cause: SessionEndCause::AccountUnavailable,
        at_if_known: None,
    }
}

fn invalid_token() -> ApiError {
    ApiError::new(
        ErrorCode::AuthInvalidToken,
        "the token is expired or unknown",
    )
}

#[path = "authentication/native_read.rs"]
pub mod native_read;

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn surface_headers_reject_foreign_kinds_before_public_and_cookie_shortcuts() {
        use orch_api_contract::surface::HttpSurface;
        let api_key = new_api_token(orch_types::new_id());
        for (token, origin, foreign) in [
            ("oat_synthetic", HttpSurface::Browser, HttpSurface::Key),
            (api_key.as_str(), HttpSurface::Key, HttpSurface::Browser),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                format!("Bearer {token}").parse().unwrap(),
            );
            assert!(on_surface(origin, &headers).is_ok());
            assert!(on_surface(foreign, &headers).is_err());
        }
        for malformed in ["opat_synthetic".to_owned(), format!("{api_key}x")] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                format!("Bearer {malformed}").parse().unwrap(),
            );
            assert!(on_surface(HttpSurface::Key, &headers).is_err());
            assert!(on_surface(HttpSurface::Browser, &headers).is_err());
        }
        let mut cookie = HeaderMap::new();
        cookie.insert(
            header::COOKIE,
            "orch_session=ors_synthetic".parse().unwrap(),
        );
        assert!(on_surface(HttpSurface::Browser, &cookie).is_ok());
        assert!(on_surface(HttpSurface::Key, &cookie).is_err());
        // No bearer is needed for the already authorized ticket/public protocols.
        assert!(on_surface(HttpSurface::Key, &HeaderMap::new()).is_ok());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_surface_headers_reject_foreign_kinds_before_public_and_cookie_shortcuts() {
        self::surface_headers_reject_foreign_kinds_before_public_and_cookie_shortcuts()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_cookie_is_found_among_others() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "theme=dark; orch_session=ors_abc; x=1".parse().unwrap(),
        );
        assert_eq!(cookie(&headers, "orch_session"), Some("ors_abc"));
        assert_eq!(cookie(&headers, "orch_device"), None);
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_cookie_is_found_among_others() {
        self::a_cookie_is_found_among_others()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn tokens_are_long_random_and_told_apart_by_their_prefix() {
        let first = new_token(ACCESS_TOKEN_PREFIX);
        let second = new_token(ACCESS_TOKEN_PREFIX);
        assert!(first.starts_with("oat_") && first.len() == 4 + 43);
        assert_ne!(first, second);
        assert_ne!(token_hash(&first), token_hash(&second));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_tokens_are_long_random_and_told_apart_by_their_prefix() {
        self::tokens_are_long_random_and_told_apart_by_their_prefix()
    }
}
