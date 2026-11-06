//! Native resource preparations, factual source roles and their original renderers.
//!
//! Selection and ordinary authorization stay with the consuming operation. This handoff
//! carries only its already selected native rows and prepared values; it never infers a
//! source from a response field or lends another operation's Read authority. Consumers
//! retain their own final stages, including Machine readiness and Host ports probing.

use super::collection::Collection;
use super::mcp::projection as mcp;
use super::{agents, hosts, machines, mounts, providers, skills, templates};
use crate::key_operations::targets;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan};
use crate::routes::access::operations::{NameRoles, projection_names};
use crate::routes::access::page::{NameBounds, NameSources};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, ResourceKind};
use orch_api_contract::operations::TargetResolver;
use orch_api_contract::resource_reads;
use orch_authz::{Caller, Principal, resources::Seen};
use orch_db::Conn;
use orch_types::Id;

enum View {
    Agent(agents::PreparedListProjection),
    Provider(providers::PreparedListProjection),
    Template(templates::PreparedListProjection),
    Mcp(mcp::Prepared, NameBounds),
    Host(hosts::PreparedInventory),
    Environment(crate::routes::environments::PreparedInventory),
    Machine(machines::PreparedInventory),
    Mount(mounts::PreparedInventory),
    Skill(skills::PreparedInventory),
}

pub(crate) struct Prepared {
    view: View,
    names: NameSources,
    foreign: Vec<ObjectRef>,
}

/// Native selection facts, independent of an inventory row or transport response.
/// `ids` also retains selected physical identities whose governor is another kind.
pub(crate) struct Selection<'a> {
    pub kind: ResourceKind,
    pub ids: Vec<Id>,
    pub seen: &'a [Seen],
}

/// The original native outputs, without a consumer's collection or inventory wrapper.
pub(crate) enum Rendered {
    Agent(Vec<orch_api_contract::resources::Agent>),
    Provider(Vec<orch_api_contract::resources::Provider>),
    Template(Vec<orch_api_contract::resources::Template>),
    Mcp(Vec<orch_api_contract::resources::McpServer>),
    Host(Vec<orch_api_contract::resources::Host>),
    Environment(Vec<orch_api_contract::environment::Environment>),
    Machine(Vec<orch_api_contract::resources::Machine>),
    Mount(Vec<orch_api_contract::mounts::Mount>),
    Skill(Vec<orch_api_contract::skills::Skill>),
}

impl Prepared {
    fn kind(&self) -> ResourceKind {
        match &self.view {
            View::Agent(_) => ResourceKind::Agent,
            View::Provider(_) => ResourceKind::Provider,
            View::Template(_) => ResourceKind::Template,
            View::Mcp(..) => ResourceKind::McpServer,
            View::Host(_) => ResourceKind::Host,
            View::Environment(_) => ResourceKind::Environment,
            View::Machine(_) => ResourceKind::Machine,
            View::Mount(_) => ResourceKind::Mount,
            View::Skill(_) => ResourceKind::Skill,
        }
    }

    pub(crate) fn name_sources(&self) -> &NameSources {
        &self.names
    }

    pub(crate) fn foreign_sources(&self) -> &[ObjectRef] {
        &self.foreign
    }

    pub(crate) fn bound_names(&mut self, bounds: NameBounds) {
        match &mut self.view {
            View::Agent(prepared) => prepared.bound_names(bounds),
            View::Provider(prepared) => prepared.bound_names(bounds),
            View::Template(prepared) => prepared.bound_names(bounds),
            View::Mcp(_, current) => *current = bounds,
            View::Host(prepared) => prepared.bound_names(bounds),
            View::Environment(prepared) => prepared.bound_names(bounds),
            View::Machine(prepared) => prepared.bound_names(bounds),
            View::Mount(prepared) => prepared.bound_names(bounds),
            View::Skill(prepared) => prepared.bound_names(bounds),
        }
    }

    /// Bind this native preparation under its consuming family's own operation. Inventory
    /// consumes the very same `foreign_sources` and names through its heterogeneous result role.
    pub(super) async fn bind_family_sources(
        &mut self,
        conn: &mut Conn,
        caller: &Caller,
        plan: &mut OperationPlan,
        family: &resource_reads::Family,
    ) -> ApiResult<()> {
        if self.kind() != family.kind
            || self.foreign.iter().any(|object| {
                !family
                    .sources
                    .iter()
                    .any(|role| ObjectKind::from(role.kind) == object.kind)
            })
        {
            return Err(ApiError::internal("the native family source is undeclared"));
        }
        for role in family.sources {
            let ids: Vec<_> = self
                .foreign
                .iter()
                .filter(|object| object.kind == ObjectKind::from(role.kind))
                .map(|object| object.id)
                .collect();
            let source = if role.kind == ResourceKind::Machine {
                Source::Machines {
                    role: role.role,
                    ids: &ids,
                }
            } else {
                Source::Resources {
                    role: role.role,
                    kind: role.kind,
                    ids: &ids,
                }
            };
            bind_source(conn, plan, &source).await?;
        }
        let names = name_roles(family);
        self.bound_names(projection_names(conn, caller, plan, self.names.clone(), &names).await?);
        Ok(())
    }

    pub(crate) async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<Rendered> {
        Ok(match &self.view {
            View::Agent(prepared) => {
                Rendered::Agent(agents::show_list_prepared(ctx, conn, principal, prepared).await?)
            }
            View::Provider(prepared) => Rendered::Provider(
                providers::show_list_prepared(ctx, conn, principal, prepared).await?,
            ),
            View::Template(prepared) => {
                Rendered::Template(templates::show_list_prepared(conn, principal, prepared).await?)
            }
            View::Machine(prepared) => Rendered::Machine(
                machines::show_inventory_prepared(ctx, conn, principal, prepared).await?,
            ),
            View::Mount(prepared) => {
                Rendered::Mount(mounts::show_inventory_prepared(conn, principal, prepared).await?)
            }
            View::Skill(prepared) => {
                Rendered::Skill(skills::show_inventory_prepared(conn, principal, prepared).await?)
            }
            View::Host(prepared) => Rendered::Host(
                hosts::show_inventory_prepared(ctx, conn, principal, prepared).await?,
            ),
            View::Environment(prepared) => Rendered::Environment(
                crate::routes::environments::show_inventory_prepared(
                    ctx, conn, principal, prepared,
                )
                .await?,
            ),
            View::Mcp(prepared, bounds) => {
                Rendered::Mcp(mcp::render(ctx, conn, principal, prepared, Some(bounds)).await?)
            }
        })
    }
}

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

pub(crate) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    selection: Selection<'_>,
) -> ApiResult<Option<Prepared>> {
    let Selection { kind, ids, seen } = selection;
    if ids.is_empty() {
        return Ok(None);
    }
    let mut foreign = Vec::new();
    let (view, names) = match kind {
        ResourceKind::Agent => {
            let prepared = agents::prepare_list_projection(conn, principal, seen).await?;
            foreign.extend(prepared.provider_ids().iter().map(|id| ObjectRef {
                kind: ObjectKind::Provider,
                id: *id,
            }));
            foreign.extend(prepared.machine_ids().iter().map(|id| ObjectRef {
                kind: ObjectKind::Machine,
                id: *id,
            }));
            let names = prepared.name_sources().clone();
            (View::Agent(prepared), names)
        }
        ResourceKind::Provider => {
            let prepared = providers::prepare_list_projection(ctx, conn, principal, seen).await?;
            let names = prepared.name_sources().clone();
            (View::Provider(prepared), names)
        }
        ResourceKind::Template => {
            let prepared = templates::prepare_list_projection(conn, seen).await?;
            let names = prepared.name_sources().clone();
            (View::Template(prepared), names)
        }
        ResourceKind::Machine => {
            let ids: Vec<_> = ids.iter().copied().collect();
            let found =
                orch_db::queries::resources::routes::physical_machines_among(conn, &ids).await?;
            let prepared = machines::prepare_inventory(conn, principal, found).await?;
            let names = prepared.name_sources().clone();
            (View::Machine(prepared), names)
        }
        ResourceKind::Mount => {
            let ids: Vec<_> = ids.iter().copied().collect();
            let prepared = mounts::prepare_inventory(ctx, conn, principal, &ids).await?;
            native_sources(
                &mut foreign,
                &resource_reads::MOUNT.sources[0],
                prepared.machine_ids(),
            );
            let names = prepared.name_sources().clone();
            (View::Mount(prepared), names)
        }
        ResourceKind::Skill => {
            let prepared = skills::prepare_inventory(conn, seen).await?;
            let names = prepared.name_sources().clone();
            (View::Skill(prepared), names)
        }
        ResourceKind::Host => {
            let prepared = hosts::prepare_inventory(ctx, conn, principal, seen).await?;
            // The Host's own physical Machine/facade is intrinsic to this same subject.
            // Only actually emitted retained Environment names/refs are distinct sources.
            foreign.extend(prepared.retained_name_sources().iter().map(|id| ObjectRef {
                kind: ObjectKind::Environment,
                id: *id,
            }));
            let names = prepared.name_sources().clone();
            (View::Host(prepared), names)
        }
        ResourceKind::Environment => {
            let prepared =
                crate::routes::environments::prepare_inventory(ctx, conn, principal, seen).await?;
            foreign.extend(prepared.template_ids().iter().map(|id| ObjectRef {
                kind: ObjectKind::Template,
                id: *id,
            }));
            foreign.extend(prepared.host_ids().iter().map(|id| ObjectRef {
                kind: ObjectKind::Host,
                id: *id,
            }));
            // Physical MachineRef is distinct content. Connector/runtime update and program
            // observations remain intrinsic to the Environment, not new Machine privileges.
            let physical = orch_db::queries::resources::routes::live_machine_ids(
                conn,
                prepared.physical_machine_ids(),
            )
            .await?;
            foreign.extend(physical.into_iter().map(|id| ObjectRef {
                kind: ObjectKind::Machine,
                id,
            }));
            let names = prepared.name_sources().clone();
            (View::Environment(prepared), names)
        }
        ResourceKind::McpServer => {
            // Keep the native Manage checks of this preparation style.
            let ids: Vec<_> = ids.into_iter().collect();
            let prepared =
                mcp::prepare(conn, principal, &ids, Some(seen), mcp::Style::Presets).await?;
            foreign.extend(prepared.machines.iter().map(|id| ObjectRef {
                kind: ObjectKind::Machine,
                id: *id,
            }));
            foreign.extend(prepared.environments.iter().map(|id| ObjectRef {
                kind: ObjectKind::Environment,
                id: *id,
            }));
            let names = prepared.names.clone();
            (View::Mcp(prepared, NameBounds::default()), names)
        }
    };
    foreign.sort_unstable_by_key(|object| (object.kind, object.id));
    foreign.dedup();
    Ok(Some(Prepared {
        view,
        names,
        foreign,
    }))
}

/// The native producer supplies identities for this declared factual role. Both inventory
/// and typed reads consume these same facts; the role never selects identities from a DTO.
fn native_sources(out: &mut Vec<ObjectRef>, role: &resource_reads::SourceRole, ids: &[Id]) {
    out.extend(ids.iter().map(|id| ObjectRef {
        kind: role.kind.into(),
        id: *id,
    }));
}

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

/// Stored sources selected by the native preparation, not identities inferred from a DTO.
pub(super) enum Source<'a> {
    Resources {
        role: &'static str,
        kind: ResourceKind,
        ids: &'a [Id],
    },
    /// Physical Machine content keeps its actual Host/Machine governor and Machine family.
    Machines { role: &'static str, ids: &'a [Id] },
}

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

async fn bind_source(
    conn: &mut Conn,
    plan: &mut OperationPlan,
    source: &Source<'_>,
) -> ApiResult<()> {
    let (role, kind, ids, mut found) = match source {
        Source::Resources { role, kind, ids } => (
            *role,
            KeyTargetKind::Object((*kind).into()),
            *ids,
            Collection::new(*kind).targets(conn, ids).await?,
        ),
        Source::Machines { role, ids } => (
            *role,
            KeyTargetKind::Object(ObjectKind::Machine),
            *ids,
            targets::machine_subjects(conn, ids).await?,
        ),
    };
    let facts = ids
        .iter()
        .map(|id| found.remove(id).ok_or_else(ApiError::not_found))
        .collect::<ApiResult<Vec<_>>>()?;
    bind_collection_source(plan, role, kind, &facts)
}
