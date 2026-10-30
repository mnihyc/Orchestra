//! Static direct/group reading access from the current root inspection row, without an actual-read event.

use super::*;
use orch_access_contract::pages::Holder;
use orch_authz::engine::{Frame, Graph};

pub(super) async fn inspection(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
) -> ApiResult<Vec<orch_api_contract::access::Holder>> {
    let graph = Graph::load(conn, &[chat.org_id])
        .await
        .map_err(ApiError::internal)?;
    let site = graph.site_of(chat.org_id).ok_or_else(ApiError::not_found)?;
    let frame = Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        std::slice::from_ref(&site),
        &[Permission::DirectsView],
    )
    .await
    .map_err(ApiError::internal)?;
    let mut holders: Vec<Holder> = Vec::new();
    for named in frame
        .chain(&site, Permission::DirectsView)
        .holders(principal.now())
    {
        if !holders.contains(&named.entry.holder) {
            holders.push(named.entry.holder);
        }
    }
    crate::routes::access::page::Names::holders(
        conn,
        principal,
        Some(chat.org_id),
        Some(chat.org_id),
        &holders,
    )
    .await
}
