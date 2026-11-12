//! Static direct/group reading access from the current root inspection row, without an actual-read event.

use super::*;

use orch_access_contract::pages::Holder;

use orch_authz::engine::Frame;

use orch_authz::engine::Graph;
pub(super) async fn inspection(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
) -> ApiResult<Vec<orch_api_contract::access::Holder>> {
    let holders = holders(conn, principal, chat).await?;
    project(conn, principal, chat, &holders, None).await
}

/// The actual inspection holder identities, before any full Group or protected place is named.
pub(super) async fn holders(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
) -> ApiResult<Vec<Holder>> {
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
    Ok(holders)
}

/// The same holder projection after its full Group sources have been admitted.
pub(super) async fn project(
    conn: &mut Conn,
    principal: &Principal,
    chat: &ChatRow,
    holders: &[Holder],
    bounds: Option<&crate::routes::access::page::NameBounds>,
) -> ApiResult<Vec<orch_api_contract::access::Holder>> {
    match bounds {
        Some(bounds) => {
            crate::routes::access::page::Names::holders_bounded(
                conn,
                principal,
                Some(chat.org_id),
                Some(chat.org_id),
                holders,
                bounds,
            )
            .await
        }
        None => {
            crate::routes::access::page::Names::holders(
                conn,
                principal,
                Some(chat.org_id),
                Some(chat.org_id),
                holders,
            )
            .await
        }
    }
}
