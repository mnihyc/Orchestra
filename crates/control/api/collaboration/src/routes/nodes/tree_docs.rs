//! Reading the document rows of many tree nodes with a fixed number of access statements.

use orch_access_contract::permissions::Permission;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_types::Id;

use std::collections::HashSet;
/// Nodes whose following documents the reader may read through the node's documents row.
pub(crate) async fn readable(
    conn: &mut Conn,
    principal: &Principal,
    nodes: &[Id],
) -> Result<HashSet<Id>, orch_authz::Error> {
    Ok(orch_authz::nodes::holding_ids(
        conn,
        principal,
        nodes,
        Permission::DocView,
        Act::Use,
        Policy::default(),
    )
    .await?
    .into_iter()
    .collect())
}
orch_api_base::__api_bind_f52fcb6b67!();
