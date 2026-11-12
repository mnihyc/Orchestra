//! One effective organization text for public preview and every invitation redemption path.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::users::InvitationTerms;

use orch_db::Conn;

use orch_db::queries::chats;

use orch_db::queries::hierarchy;

use orch_db::queries::identity::InvitationRow;

use orch_settings::Place;

use orch_settings::get;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use sha2::Digest;

use sha2::Sha256;
pub(crate) async fn target_org(
    conn: &mut Conn,
    invitation: &InvitationRow,
) -> ApiResult<Option<Id>> {
    let context = match invitation.kind.as_str() {
        "system" => return Ok(None),
        "org" => invitation.org_id,
        "node" => invitation.node_id,
        "chat" => match invitation.chat_id {
            Some(id) => chats::chat(conn, id).await?.map(|chat| chat.home()),
            None => None,
        },
        _ => None,
    }
    .ok_or_else(super::gone)?;
    let rows = hierarchy::with_ancestors(conn, &[context]).await?;
    let target = rows
        .iter()
        .find(|row| row.id == context && row.deleted_at.is_none())
        .ok_or_else(super::gone)?;
    target
        .path
        .iter()
        .rev()
        .find_map(|id| {
            rows.iter()
                .find(|row| row.id == *id && row.kind == "org")
                .map(|row| Some(row.id))
        })
        .ok_or_else(super::gone)
}

fn text_terms(text: String) -> Option<InvitationTerms> {
    if text.is_empty() {
        return None;
    }
    let hash = hex::encode(Sha256::digest(text.as_bytes()));
    Some(InvitationTerms { text, hash })
}

pub(crate) async fn effective(
    conn: &mut Conn,
    invitation: &InvitationRow,
) -> ApiResult<Option<InvitationTerms>> {
    let Some(org) = target_org(conn, invitation).await? else {
        return Ok(None);
    };
    let text = get::<String>(conn, Place::Node(org), keys::ORG_TERMS)
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_default();
    Ok(text_terms(text))
}

/// Pin all actual organization text sources, including absent rows, through the joining transaction.
/// Invitation redemption already owns its native target/ancestry serialization footprint.
pub(crate) async fn require(
    conn: &mut Conn,
    invitation: &InvitationRow,
    accepted_hash: Option<&str>,
    clock: &dyn orch_types::Clock,
) -> ApiResult<Option<InvitationTerms>> {
    if let Some(org) = target_org(conn, invitation).await? {
        let rows = hierarchy::with_ancestors(conn, &[org]).await?;
        let path = rows
            .iter()
            .find(|row| row.id == org)
            .ok_or_else(super::gone)?
            .path
            .clone();
        for id in path {
            if rows.iter().any(|row| row.id == id && row.kind == "org") {
                orch_db::queries::settings::lock_org_terms(conn, Some(id), false).await?;
            }
        }
    }
    let current = effective(conn, invitation).await?;
    if let Some(terms) = &current
        && accepted_hash != Some(terms.hash.as_str())
    {
        // A source wait must not publish an offer after its delegation or lifetime closes.
        super::preview_issuer(conn, invitation, clock).await?;
        if invitation.expires_at <= clock.now() {
            return Err(super::gone());
        }
        return Err(ApiError::new(
            ErrorCode::InvitationTermsNotAccepted,
            "accept the current organization terms before joining",
        )
        .with("terms", terms));
    }
    Ok(current)
}

/// An anonymous identity-provider start/callback can review only a currently delegated offer.
pub(crate) async fn require_offer(
    conn: &mut Conn,
    invitation: &InvitationRow,
    accepted_hash: Option<&str>,
    clock: &dyn orch_types::Clock,
) -> ApiResult<Option<InvitationTerms>> {
    super::preview_issuer(conn, invitation, clock).await?;
    let current = require(conn, invitation, accepted_hash, clock).await?;
    if invitation.expires_at <= clock.now() {
        return Err(super::gone());
    }
    Ok(current)
}

/// Records a successful acknowledgement on the native row the invitation just supplied.
/// The immutable audit retains every acceptance even when a human grant is coalesced.
pub(crate) async fn record(
    ctx: &crate::registry::Ctx,
    conn: &mut Conn,
    invitation: &InvitationRow,
    user: &orch_db::queries::identity::UserRow,
    terms: &InvitationTerms,
) -> ApiResult<()> {
    use orch_access_contract::audit::AuditAction;
    use orch_db::{DbError, audit};
    use serde_json::json;

    let org = target_org(conn, invitation)
        .await?
        .ok_or_else(super::gone)?;
    let place = hierarchy::with_ancestors(conn, &[org])
        .await?
        .into_iter()
        .find(|row| row.id == org)
        .ok_or_else(super::gone)?;
    let at = ctx.app.clock.now();
    let changed = match invitation.kind.as_str() {
        "org" => sqlx::query(
            "UPDATE org_members SET terms_hash=$3, terms_accepted_at=$4
             WHERE org_id=$1 AND user_id=$2 AND domain_id=$5",
        )
        .bind(place.org_id)
        .bind(user.id)
        .bind(&terms.hash)
        .bind(at)
        .bind(org)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?,
        "node" if invitation.role.is_some() => sqlx::query(
            "UPDATE node_roles SET terms_hash=$3, terms_accepted_at=$4
             WHERE node_id=$1 AND subject_kind='user' AND subject_id=$2
               AND subject_role IS NULL AND role=$5 AND removed_at IS NULL",
        )
        .bind(invitation.node_id)
        .bind(user.id)
        .bind(&terms.hash)
        .bind(at)
        .bind(&invitation.role)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?,
        "node" => sqlx::query(
            "UPDATE permission_entries SET terms_hash=$3, terms_accepted_at=$4
             WHERE object_kind='node' AND object_id=$1 AND page='live' AND profile='any'
               AND holder_kind='user' AND holder_id=$2 AND holder_role IS NULL
               AND share_inside IS NULL AND permission=ANY($5) AND removed_at IS NULL",
        )
        .bind(invitation.node_id)
        .bind(user.id)
        .bind(&terms.hash)
        .bind(at)
        .bind(&invitation.permissions)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?,
        "chat" => sqlx::query(
            "UPDATE chat_members SET terms_hash=$3, terms_accepted_at=$4
             WHERE chat_id=$1 AND user_id=$2 AND left_at IS NULL",
        )
        .bind(invitation.chat_id)
        .bind(user.id)
        .bind(&terms.hash)
        .bind(at)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?,
        _ => return Err(super::gone()),
    };
    if changed.rows_affected() == 0 {
        return Err(ApiError::internal(
            "accepted invitation has no native recipient row",
        ));
    }
    let action = audit::action!(AuditAction::MemberTermsAccepted);
    let mut entry = audit::Entry::new(
        audit::Actor::User(user.id),
        action.as_str(),
        "member",
        Some(user.id),
        "accepted the organization's joining terms",
    );
    entry.history = action.is_history_line();
    entry.org_id = Some(place.org_id);
    entry.node_id = Some(org);
    entry.details = json!({
        "org_id": org,
        "user_id": user.id,
        "terms_hash": terms.hash,
        "invitation_id": invitation.id,
    });
    audit::append(conn, &entry, &crate::sign_in::origin(ctx), at).await?;
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    #[allow(clippy::expect_used, reason = "a test fails by panicking")]
    fn the_public_hash_is_sha256_of_the_exact_resolved_utf8_text() {
        let terms = text_terms("abc".into()).expect("nonempty terms");
        assert_eq!(
            terms.hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(terms.text, "abc");
        assert!(text_terms(String::new()).is_none());
        assert_ne!(
            text_terms("abc\n".into()).expect("exact text").hash,
            terms.hash
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_the_public_hash_is_sha256_of_the_exact_resolved_utf8_text() {
        self::the_public_hash_is_sha256_of_the_exact_resolved_utf8_text()
    }
}
