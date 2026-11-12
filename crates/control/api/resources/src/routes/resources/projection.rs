//! Native resource preparations, factual source roles and their original renderers.
//!
//! Selection and ordinary authorization stay with the consuming operation. This handoff
//! carries only its already selected native rows and prepared values; it never infers a
//! source from a response field or lends another operation's Read authority. Consumers
//! retain their own final stages, including Machine readiness and Host ports probing.

use super::mcp::projection as mcp;

use super::agents;

use super::hosts;

use super::machines;

use super::mounts;

use super::providers;

use super::skills;

use super::templates;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operations::TargetResolver;

use orch_api_contract::resource_reads;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_authz::resources::Seen;

use orch_db::Conn;

use orch_types::Id;

pub use orch_api_base::routes::resources::projection::View;

pub use orch_api_base::routes::resources::projection::Prepared;

pub use orch_api_base::routes::resources::projection::Selection;

pub use orch_api_base::routes::resources::projection::Rendered;

pub use orch_api_base::routes::resources::projection::prepare;

pub use orch_api_base::routes::resources::projection::Source;

pub use orch_api_base::routes::resources::projection::bind_source;
pub(super) fn name_roles(family: &resource_reads::Family) -> NameRoles {
    NameRoles {
        groups: family.names.groups,
        nodes: [
            (ObjectKind::Org, family.names.nodes[0]),
            (ObjectKind::Project, family.names.nodes[1]),
            (ObjectKind::Topic, family.names.nodes[2]),
            (ObjectKind::Thread, family.names.nodes[3]),
        ],
        containers: family.names.containers,
    }
}
orch_api_base::__api_bind_6110f20d78!();

/// The native producer supplies identities for this declared factual role. Both inventory
/// and typed reads consume these same facts; the role never selects identities from a DTO.
fn native_sources(out: &mut Vec<ObjectRef>, role: &resource_reads::SourceRole, ids: &[Id]) {
    out.extend(ids.iter().map(|id| ObjectRef {
        kind: role.kind.into(),
        id: *id,
    }));
}
orch_api_base::__api_bind_f101738fc4!();

/// Bind actual collection facts; only the shared plan may accept checked emptiness.
pub(super) fn bind(
    plan: &mut OperationPlan,
    slot: usize,
    kind: KeyTargetKind,
    facts: &[KeyResolvedTarget],
) -> ApiResult<()> {
    if facts.is_empty() {
        plan.bind_empty(slot)
    } else {
        plan.bind(slot, kind, facts)
    }
}

/// A named native source must be an explicitly declared collection of this operation.
pub(super) fn bind_collection_source(
    plan: &mut OperationPlan,
    role: &'static str,
    kind: KeyTargetKind,
    facts: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let slot = plan
        .variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
        .ok_or_else(|| ApiError::internal("the native content source is undeclared"))?;
    bind(plan, slot, kind, facts)
}
orch_api_base::__api_bind_19ca020a5c!();

/// Resolve and bind actual native sources in producer order, then bound their names.
/// Ordinary source permissions and final publication remain with the original producer.
pub(super) async fn bind_sources(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut OperationPlan,
    sources: &[Source<'_>],
    names: NameSources,
    roles: &NameRoles,
) -> ApiResult<NameBounds> {
    for source in sources {
        bind_source(conn, plan, source).await?;
    }
    projection_names(conn, caller, plan, names, roles).await
}

#[path = "projection/single.rs"]
pub mod single;
