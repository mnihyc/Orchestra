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

use orch_db::queries::identity;

use orch_db::queries::identity::UserRow;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use sha2::Digest;

use sha2::Sha256;

use std::sync::Arc;

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

/// A new token: `prefix` and 32 random bytes in base64url.
#[must_use]
pub fn new_token(prefix: &str) -> String {
    use base64::Engine as _;
    let bytes: [u8; 32] = rand::random();
    format!(
        "{prefix}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
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
    use orch_api_contract::surface::HttpSurface;
    // Keep the existing resolver's classification: opat_ names a key; other bearers
    // must still resolve as actual session access tokens. This is not a new token-format rule.
    if bearer(headers)
        .is_some_and(|token| (surface == HttpSurface::Key) != token.starts_with(API_TOKEN_PREFIX))
        || (surface == HttpSurface::Key
            && cookie(headers, orch_api_contract::common::SESSION_COOKIE).is_some())
    {
        return Err(invalid_token());
    }
    Ok(())
}

/// The same credential resolver, restricted to the surface selected by the server's mount.
pub async fn authenticate_on(
    surface: orch_api_contract::surface::HttpSurface,
    app: &AppState,
    headers: &HeaderMap,
    now: DateTime<Utc>,
) -> ApiResult<Caller> {
    on_surface(surface, headers)?;
    authenticate(app, headers, now).await
}

/// Finds out who makes a request from its headers. No token is nobody; a
/// token that is unknown, expired or revoked is refused, and so is a user
/// who is suspended.
pub async fn authenticate(
    app: &AppState,
    headers: &HeaderMap,
    now: DateTime<Utc>,
) -> ApiResult<Caller> {
    let Some(token) = bearer(headers) else {
        return Ok(Caller::Anonymous);
    };
    let hash = token_hash(token);
    let mut conn = app.db.acquire().await?;
    let (user_id, via, as_admin) = if token.starts_with(API_TOKEN_PREFIX) {
        let row = identity::api_token_by_hash(&mut conn, &hash, now)
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
        let row = identity::session_by_access_token(&mut conn, &hash, now)
            .await?
            .ok_or_else(invalid_token)?;
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
        return Err(ApiError::new(
            ErrorCode::AuthSuspended,
            "this user is suspended",
        ));
    }
    Ok(Caller::User(Arc::new(Authed {
        user,
        via,
        as_admin,
    })))
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
        for (token, origin, foreign) in [
            ("oat_synthetic", HttpSurface::Browser, HttpSurface::Key),
            ("opat_synthetic", HttpSurface::Key, HttpSurface::Browser),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                format!("Bearer {token}").parse().unwrap(),
            );
            assert!(on_surface(origin, &headers).is_ok());
            assert!(on_surface(foreign, &headers).is_err());
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
