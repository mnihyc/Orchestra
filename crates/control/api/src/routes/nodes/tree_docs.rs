//! Reading the document rows of many tree nodes with a fixed number of access statements.

use orch_access_contract::permissions::{Permission, PermissionName};
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_types::Id;
use std::collections::HashSet;

/// Nodes whose following documents the reader may read through the node's documents row.
pub(crate) async fn readable(
    conn: &mut Conn,
    principal: &Principal,
    nodes: &[Id],
) -> Result<HashSet<Id>, orch_authz::Error> {
    Ok(orch_authz::nodes::holding(
        conn,
        principal,
        nodes,
        PermissionName::Exact(Permission::DocView),
        Act::Use,
        Policy::default(),
    )
    .await?
    .into_iter()
    .map(|node| node.id)
    .collect())
}
