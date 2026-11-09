//! Protected node names selected by the update family's native reference producer.

use super::*;
use crate::routes::access::page::NameBounds;
use crate::routes::nodes::view::{self, ReferenceDisclosure};
use std::collections::BTreeSet;

pub(super) struct Names {
    pub bounds: NameBounds,
    pub disclosure: ReferenceDisclosure,
    pub groups: BTreeMap<KeyTargetKind, Vec<orch_access_contract::key_scope::KeyResolvedTarget>>,
}
pub(super) async fn prepare(
    conn: &mut Conn,
    principal: &Principal,
    known: &crate::registry::OperationPlan,
    declaration: &'static Declaration<Source>,
    wanted: BTreeSet<Id>,
) -> ApiResult<Names> {
    let graph = orch_authz::engine::Graph::load(conn, &wanted.into_iter().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let candidates = graph.nodes().map(|node| node.id).collect::<Vec<_>>();
    let disclosure =
        view::reference_disclosure(conn, principal, &candidates, orch_authz::Policy::default())
            .await?;
    let mut names = BTreeSet::new();
    let mut groups = BTreeMap::new();
    for id in &disclosure.names {
        let target = targets::node_identity(conn, *id).await?;
        let family = target.object.kind;
        if native::permits_usage(
            known,
            declaration,
            principal.caller(),
            &Source::Context,
            FacetUsage::ResponseContent,
            family,
            &target,
        )? {
            names.insert(*id);
            groups.entry(family).or_insert_with(Vec::new).push(target);
        }
    }
    Ok(Names {
        bounds: NameBounds {
            limited: native::modern(principal.caller()),
            nodes: names,
            ..Default::default()
        },
        disclosure,
        groups,
    })
}
