//! Personalize harness request cards in a timeline batch, without a per-card HTTP read.

use crate::middleware::requests::ApiResult;
use orch_api_contract::threads::{Item, ItemPayload};
use orch_authz::Principal;
use orch_db::{Conn, queries::approvals};
use orch_types::Id;
use std::collections::HashMap;

/// Only a request of the same thread can be attached. Authority is the ordinary approval
/// projection's current authority, never inferred from an item's old pending status.
pub(crate) async fn project(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    items: &mut [Item],
    now: chrono::DateTime<chrono::Utc>,
    engine: &orch_engine::Engine,
) -> ApiResult<()> {
    let ids: Vec<Id> = items
        .iter()
        .filter_map(|item| match &item.payload {
            ItemPayload::Approval(payload) => Some(payload.approval_id),
            _ => None,
        })
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let found = approvals::harness_among(conn, thread, &ids).await?;
    let shown: HashMap<Id, _> =
        crate::routes::approvals::views(conn, principal, &found, now, engine)
            .await?
            .into_iter()
            .map(|approval| (approval.id, approval))
            .collect();
    for item in items {
        if let ItemPayload::Approval(payload) = &mut item.payload {
            // Erase any stored/stale projection even when its id was not in this thread.
            payload.approval = shown.get(&payload.approval_id).cloned().map(Box::new);
        }
    }
    Ok(())
}
