//! A document's personal discussion bell, independent of its text and read position.
//!
//! Every discussion adds currently readable subscribers to the ordinary
//! discussion recipients. Those I am in leaves that ordinary rule intact.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use orch_api_contract::documents::{
    DocumentDiscussionLevel, DocumentSubscriptionSet, DocumentSubscriptionSetBody,
};
use orch_db::queries::documents::people;
use orch_db::{DbError, sequences};
use orch_realtime_contract::feeds::UserEvent;

/// Binds the caller's discussion-bell request.
pub fn mount(registry: &mut Registry) {
    registry.on(DocumentSubscriptionSet, set);
}

async fn set(ctx: Ctx, (): (), body: DocumentSubscriptionSetBody) -> ApiResult<()> {
    let document = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    super::live(&mut tx, document).await?;
    let level = match body.level {
        DocumentDiscussionLevel::All => "all",
        DocumentDiscussionLevel::Involved => "involved",
    };
    people::set_level(
        &mut tx,
        guard.principal().user(),
        document,
        level,
        guard.now().get(),
    )
    .await?;
    let event = serde_json::to_value(UserEvent::DocumentState {
        document_id: document,
        discussion_level: body.level,
    })
    .map_err(ApiError::internal)?;
    sequences::notify_user_event(&mut tx, &[guard.principal().user()], &event).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
