//! A reset link made by hand, for a person who cannot sign in and whom no mail reaches: made by a system admin, or by the operator on the host.
//!
//! It is the link that mailed recovery sends, made without the mail: a secret
//! in an address on the system's own origin, of which only the hash is kept,
//! good for one use and for half an hour. Using it (`POST /auth/recovery/confirm`)
//! sets a new password, ends every session of the person and, if the link was
//! made for somebody who lost their second step, switches that off too. The
//! making is audited, by the actor who made it, so that the trail says who
//! opened a way into an account.

use super::mail::{EMAIL_TOKEN_PREFIX, Purpose, link};
use crate::middleware::authentication::{new_token, token_hash};
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_config::ServerConfig;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::identity::{self, NewEmailToken, UserRow};
use orch_db::{Conn, DbError};
use serde_json::json;

/// The page of the system's origin that takes the secret of a reset link.
const RESET_PAGE: &str = "reset";

/// A link that was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    /// The address to open, with the secret in it. It is shown once: only its
    /// hash is kept.
    pub url: String,
    /// When it stops being good.
    pub expires_at: DateTime<Utc>,
}

/// Makes a reset link for `user`, in the transaction of the caller, and
/// writes the line that says who made it. With `clear_second_step`, using it
/// also switches off the second step of the person, an authenticator and a
/// mailed code alike.
///
/// The line holds no secret; it is written as a password reset because the
/// audit catalogue has no other word for a way into an account, and tells
/// the two apart by its `stage` (`link_made`; the line of using it, written
/// by the person, has none).
pub async fn make(
    conn: &mut Conn,
    config: &ServerConfig,
    user: &UserRow,
    clear_second_step: bool,
    by: Actor,
    origin: &Origin,
    now: DateTime<Utc>,
) -> Result<Made, DbError> {
    // Order the recipient's current address against address changes before issuing the link.
    let user = orch_db::queries::transaction_guard::lock_person(conn, user.id)
        .await?
        .ok_or_else(|| DbError::Reference {
            constraint: "users".to_owned(),
        })?;
    let secret = new_token(EMAIL_TOKEN_PREFIX);
    let expires_at = now + Purpose::Reset.life();
    identity::create_email_token(
        conn,
        &NewEmailToken {
            id: orch_types::new_id(),
            user_id: user.id,
            purpose: Purpose::Reset.stored(),
            token_hash: &token_hash(&secret),
            // Nothing is sent: the column names the address a link went to,
            // and this one went to whoever is given it.
            email: user.email.as_deref().unwrap_or_default(),
            clear_second_step,
            at: now,
            expires_at,
        },
    )
    .await?;
    let action = AuditAction::AuthPasswordReset;
    let summary = format!("a reset link was made for {}", user.username);
    let mut entry = Entry::new(
        by,
        action.as_str(),
        action.object_kind().unwrap_or("auth"),
        Some(user.id),
        &summary,
    );
    entry.history = action.is_history_line();
    entry.details = json!({
        "stage": "link_made",
        "clear_second_step": clear_second_step,
        "expires_at": orch_types::Time::new(expires_at),
    });
    audit::append(conn, &entry, origin, now).await?;
    Ok(Made {
        url: link(config, RESET_PAGE, &secret),
        expires_at,
    })
}
