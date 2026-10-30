//! Resolve sessions and scoped tokens.
//!
//! A caller is nobody, a person in a browser session, or an API token
//! acting for its user. A browser sends the short-lived access token it
//! was given at sign-in; automation sends an API token. Both are looked up
//! by their SHA-256 on every request, with the user's row, so that a
//! revoked session or a suspended user is refused at once.

use super::requests::{ApiError, ApiResult};
use crate::state::AppState;
use chrono::{DateTime, Utc};
use http::{HeaderMap, header};
use orch_api_contract::common::ErrorCode;
use orch_db::queries::identity::{self, UserRow};
use orch_types::Id;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// What an access token begins with.
pub const ACCESS_TOKEN_PREFIX: &str = "oat_";
/// What a session cookie's value begins with.
pub const SESSION_PREFIX: &str = "ors_";
/// What an API token begins with.
pub const API_TOKEN_PREFIX: &str = "opat_";

/// How a caller proved who they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
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
    },
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
        (
            row.user_id,
            Via::Token {
                token_id: row.id,
                scopes: row.scopes,
                node_id: row.node_id,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cookie_is_found_among_others() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "theme=dark; orch_session=ors_abc; x=1".parse().unwrap(),
        );
        assert_eq!(cookie(&headers, "orch_session"), Some("ors_abc"));
        assert_eq!(cookie(&headers, "orch_device"), None);
    }

    #[test]
    fn tokens_are_long_random_and_told_apart_by_their_prefix() {
        let first = new_token(ACCESS_TOKEN_PREFIX);
        let second = new_token(ACCESS_TOKEN_PREFIX);
        assert!(first.starts_with("oat_") && first.len() == 4 + 43);
        assert_ne!(first, second);
        assert_ne!(token_hash(&first), token_hash(&second));
    }
}
