//! Validate composer provenance through ordinary native send gates. It grants no authority.

use crate::EngineError;
use orch_access_contract::{audit::LinkOrigin, permissions::Permission};
use orch_authz::{Act, Policy, Principal};
use orch_db::{Conn, audit::Origin};

#[derive(Debug, thiserror::Error)]
#[error("the sender cannot compose in this link's source context")]
/// A requested provenance context is unavailable for this actor.
pub struct Refused;

fn refused() -> EngineError {
    EngineError::BeforeEffect(Box::new(Refused))
}

/// Provenance describes the authenticated person's composing context, not a new message effect.
/// The original operation still admits the grant and every target; this check supplies no scope.
pub async fn check(
    conn: &mut Conn,
    principal: &Principal,
    source: Option<LinkOrigin>,
) -> Result<(), EngineError> {
    let Some(source) = source else {
        return Ok(());
    };
    let ordinary = principal.ordinary(conn).await?;
    match source {
        LinkOrigin::Chat { chat_id } => {
            let origin = Origin::default();
            let act = orch_chat::Acting {
                principal: &ordinary,
                now: principal.now().get(),
                origin: &origin,
            };
            let loaded = orch_chat::chats::load(conn, &act, chat_id, orch_chat::chats::Mode::Read)
                .await
                .map_err(chat_error)?;
            loaded.ensure_send().map_err(chat_error)?;
        }
        LinkOrigin::ThreadInput { thread_id } => {
            let thread = orch_db::queries::threads::thread(conn, thread_id)
                .await?
                .filter(|thread| thread.deleted_at.is_none() && thread.archived_at.is_none())
                .ok_or_else(refused)?;
            if !orch_authz::nodes::check(
                conn,
                &ordinary,
                thread.node_id,
                Permission::ThreadInteract,
                Act::Use,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(refused());
            }
        }
    }
    Ok(())
}

fn chat_error(error: orch_chat::Error) -> EngineError {
    match error {
        orch_chat::Error::Db(error) => error.into(),
        orch_chat::Error::Settings(error) => error.into(),
        _ => refused(),
    }
}
