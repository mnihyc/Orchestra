//! Project names only when the current reader can view the resource at its actual home.
use crate::EngineError;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_authz::Principal;
use orch_db::{Conn, queries::resources};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub(super) fn may_read(principal: &Principal) -> bool {
    !principal
        .caller()
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}

pub(super) async fn names(
    conn: &mut Conn,
    principal: &Principal,
    ids: BTreeSet<Id>,
) -> Result<BTreeMap<(ObjectKind, Id), String>, EngineError> {
    if ids.is_empty() || !may_read(principal) {
        return Ok(BTreeMap::new());
    }
    let facts = resources::resource_facts(conn, &ids.into_iter().collect::<Vec<_>>()).await?;
    let policies = crate::access::resource_policies_for_facts(conn, &facts).await?;
    let mut groups = HashMap::<orch_authz::Policy, Vec<Id>>::new();
    let mut names = BTreeMap::new();
    for fact in facts {
        groups
            .entry(policies.sharing_at(fact.home_id))
            .or_default()
            .push(fact.id);
        names.insert(fact.id, fact.name);
    }
    let mut visible = BTreeMap::new();
    for (policy, ids) in groups {
        for access in orch_authz::resources::effective_among(conn, principal, &ids, policy).await? {
            let permission = match access.object.kind {
                ObjectKind::Pool => Permission::PoolView,
                ObjectKind::Provider => Permission::ProviderView,
                ObjectKind::Host => Permission::HostView,
                _ => continue,
            };
            if access.held.iter().any(|held| held.permission == permission)
                && let Some(name) = names.remove(&access.object.id)
            {
                visible.insert((access.object.kind, access.object.id), name);
            }
        }
    }
    Ok(visible)
}
