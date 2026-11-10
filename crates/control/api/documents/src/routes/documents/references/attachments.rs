//! The native attachment selector retains the actual file or carried-message association through IO.

use super::*;

use crate::middleware::transaction_guard::CurrentHttpIdentity;

use crate::routes::documents::files::FileWitness;
pub(super) struct Opening {
    pub(super) identity: CurrentHttpIdentity,
    pub(super) accepted: orch_authz::Caller,
}

impl Opening {
    pub(super) fn check(&self, ctx: &Ctx) -> ApiResult<()> {
        self.identity.check_deadline(&ctx.app)?;
        if !self.accepted.key_deadline_allows(ctx.app.clock.as_ref()) {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthInvalidToken,
                "the original attachment reader has ended",
            ));
        }
        Ok(())
    }
}

/// These are actual native source operands, never a target recovered from a formatted card.
#[derive(Clone, PartialEq)]
pub(super) enum Witness {
    Document {
        owner: Id,
        file: FileWitness,
    },
    Chat {
        reference: Id,
        chat: Id,
        message: Id,
        carried: i32,
        content: Hash,
        at: DateTime<Utc>,
    },
}

pub(super) fn message_content(content: &orch_types::Json) -> ApiResult<Hash> {
    Ok(Hash::of(
        &serde_json::to_vec(content).map_err(ApiError::internal)?,
    ))
}

impl Witness {
    /// Reopen only this selected native source; another file or custodian is never an alternative.
    pub(super) async fn check(&self, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
        match self {
            Self::Document { owner, file } => {
                file.current(conn).await?;
                allowed(
                    orch_authz::documents::check(
                        conn,
                        principal,
                        *owner,
                        Permission::DocView,
                        Policy::default(),
                    )
                    .await,
                )
            }
            Self::Chat {
                reference,
                chat,
                message,
                carried,
                content,
                at,
            } => {
                let current: Option<(Id, Id, i32)> = sqlx::query_as(
                    "SELECT chat_id,message_id,carried FROM chat_file_references WHERE id=$1",
                )
                .bind(reference)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
                if current != Some((*chat, *message, *carried)) {
                    return Err(ApiError::not_found());
                }
                let row = chat_rows::chat(conn, *chat)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                let access =
                    orch_authz::chats::listed_access(conn, principal, std::slice::from_ref(&row))
                        .await
                        .map_err(|_| ApiError::not_found())?;
                if !access.contains_key(chat) {
                    return Err(ApiError::not_found());
                }
                let row = chat_rows::message(conn, *chat, *message)
                    .await?
                    .filter(|row| !row.is_deleted())
                    .ok_or_else(ApiError::not_found)?;
                if message_content(&row.content)? != *content || row.created_at != *at {
                    return Err(ApiError::not_found());
                }
                Ok(())
            }
        }
    }
}
