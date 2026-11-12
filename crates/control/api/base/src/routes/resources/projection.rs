//! Native resource preparations, factual source roles and their original renderers.
//!
//! Selection and ordinary authorization stay with the consuming operation. This handoff
//! carries only its already selected native rows and prepared values; it never infers a
//! source from a response field or lends another operation's Read authority. Consumers
//! retain their own final stages, including Machine readiness and Host ports probing.

pub use super::mcp::projection as mcp;

pub use super::agents;

pub use super::hosts;

pub use super::machines;

pub use super::mounts;

pub use super::providers;

pub use super::skills;

pub use super::templates;

pub use crate::key_operations::targets;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use crate::registry::OperationPlan;

pub use crate::routes::access::operations::NameRoles;

pub use crate::routes::access::operations::projection_names;

pub use crate::routes::access::page::NameBounds;

pub use crate::routes::access::page::NameSources;

pub use orch_access_contract::decisions::ObjectRef;

pub use orch_access_contract::key_scope::KeyTargetKind;

pub use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::ResourceKind;

pub use orch_api_contract::resource_reads;

pub use orch_authz::Caller;

pub use orch_authz::Principal;

pub use orch_authz::resources::Seen;

pub use orch_db::Conn;

pub use orch_types::Id;

use std::collections::BTreeSet;
pub enum View {
    Agent(agents::PreparedListProjection),
    Provider(providers::PreparedListProjection),
    IndividualProvider(Box<providers::PreparedProjection>),
    Template(templates::PreparedListProjection),
    Mcp(mcp::Prepared, NameBounds),
    Host(hosts::PreparedInventory),
    Environment(crate::routes::environments::PreparedInventory),
    IndividualEnvironment(crate::routes::environments::PreparedInventory, Id),
    Machine(machines::PreparedInventory),
    Mount(mounts::PreparedInventory),
    Skill(skills::PreparedInventory),
}

pub struct Prepared {
    pub view: View,
    pub names: NameSources,
    pub foreign: Vec<ObjectRef>,
    pub retained: BTreeSet<(ObjectKind, Id)>,
}

/// Native selection facts, independent of an inventory row or transport response.
/// `ids` also retains selected physical identities whose governor is another kind.
pub struct Selection<'a> {
    pub kind: ResourceKind,
    pub ids: Vec<Id>,
    pub seen: &'a [Seen],
}

/// The original native outputs, without a consumer's collection or inventory wrapper.
pub enum Rendered {
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
    /// The same native preparation must retain the original protected source associations.
    /// Mutable display values may change; a different native source cannot validate old bytes.
    pub fn preserves_sources(&self, fresh: &Self) -> bool {
        let sources = fresh
            .foreign
            .iter()
            .map(|object| (object.kind, object.id))
            .collect::<BTreeSet<_>>();
        if !self
            .foreign
            .iter()
            .all(|object| sources.contains(&(object.kind, object.id)))
        {
            return false;
        }
        match (&self.view, &fresh.view) {
            (View::Environment(old), View::Environment(current)) => old.preserves_sources(current),
            (View::IndividualEnvironment(old, id), View::IndividualEnvironment(current, next)) => {
                id == next && old.preserves_sources(current)
            }
            (View::Host(old), View::Host(current)) => old.preserves_sources(current),
            (View::Mount(old), View::Mount(current)) => old.preserves_sources(current),
            (View::Machine(old), View::Machine(current)) => old.preserves_sources(current),
            (View::Agent(_), View::Agent(_))
            | (View::Provider(_), View::Provider(_))
            | (View::IndividualProvider(_), View::IndividualProvider(_))
            | (View::Template(_), View::Template(_))
            | (View::Mcp(..), View::Mcp(..))
            | (View::Skill(_), View::Skill(_)) => true,
            _ => false,
        }
    }

    pub fn kind(&self) -> ResourceKind {
        match &self.view {
            View::Agent(_) => ResourceKind::Agent,
            View::Provider(_) | View::IndividualProvider(_) => ResourceKind::Provider,
            View::Template(_) => ResourceKind::Template,
            View::Mcp(..) => ResourceKind::McpServer,
            View::Host(_) => ResourceKind::Host,
            View::Environment(_) | View::IndividualEnvironment(..) => ResourceKind::Environment,
            View::Machine(_) => ResourceKind::Machine,
            View::Mount(_) => ResourceKind::Mount,
            View::Skill(_) => ResourceKind::Skill,
        }
    }

    pub fn name_sources(&self) -> &NameSources {
        &self.names
    }

    pub fn foreign_sources(&self) -> &[ObjectRef] {
        &self.foreign
    }

    pub fn bound_names(&mut self, bounds: NameBounds) {
        match &mut self.view {
            View::Agent(prepared) => prepared.bound_names(bounds),
            View::Provider(prepared) => prepared.bound_names(bounds),
            View::IndividualProvider(prepared) => prepared.bound_names(bounds),
            View::Template(prepared) => prepared.bound_names(bounds),
            View::Mcp(_, current) => *current = bounds,
            View::Host(prepared) => prepared.bound_names(bounds),
            View::Environment(prepared) | View::IndividualEnvironment(prepared, _) => {
                prepared.bound_names(bounds)
            }
            View::Machine(prepared) => prepared.bound_names(bounds),
            View::Mount(prepared) => prepared.bound_names(bounds),
            View::Skill(prepared) => prepared.bound_names(bounds),
        }
    }

    /// Bind this native preparation under its consuming family's own operation. Inventory
    /// consumes the very same `foreign_sources` and names through its heterogeneous result role.
    pub async fn bind_family_sources(
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
            let source = if role.kind == ResourceKind::Machine
                && !ids.is_empty()
                && ids
                    .iter()
                    .all(|id| self.retained.contains(&(ObjectKind::Machine, *id)))
            {
                Source::MachineReferences {
                    role: role.role,
                    ids: &ids,
                }
            } else if role.kind == ResourceKind::Machine {
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

    pub async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<Rendered> {
        Ok(match &self.view {
            View::Agent(prepared) => {
                Rendered::Agent(agents::show_list_prepared(ctx, conn, principal, prepared).await?)
            }
            View::IndividualProvider(prepared) => Rendered::Provider(vec![
                providers::show_prepared(ctx, conn, principal, prepared.as_ref().clone()).await?,
            ]),
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
            View::IndividualEnvironment(prepared, id) => {
                let mut rows = crate::routes::environments::show_inventory_prepared(
                    ctx, conn, principal, prepared,
                )
                .await?;
                let view = rows.pop().ok_or_else(ApiError::not_found)?;
                Rendered::Environment(vec![
                    crate::routes::environments::finish_shown(ctx, conn, principal, *id, view)
                        .await?,
                ])
            }
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

orch_api_base::api_family_callback! {
    bind __api_bind_6110f20d78, __api_install_6110f20d78, __api_callback_6110f20d78;
    slot __ApiCallback6110f20d78, __API_CALLBACK_6110F20D78, __api_set_6110f20d78;
    base orch_api_base::routes::resources::projection;

    fn name_roles<'__api>(arg0: &'__api orch_api_contract::resource_reads::Family) -> orch_api_base::routes::access::operations::NameRoles;
}

pub async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    selection: Selection<'_>,
) -> ApiResult<Option<Prepared>> {
    let Selection { kind, ids, seen } = selection;
    if ids.is_empty() {
        return Ok(None);
    }
    let view = match kind {
        ResourceKind::Agent => {
            View::Agent(agents::prepare_list_projection(conn, principal, seen).await?)
        }
        ResourceKind::Provider => {
            View::Provider(providers::prepare_list_projection(ctx, conn, principal, seen).await?)
        }
        ResourceKind::Template => {
            View::Template(templates::prepare_list_projection(conn, seen).await?)
        }
        ResourceKind::Machine => {
            let found =
                orch_db::queries::resources::routes::physical_machines_among(conn, &ids).await?;
            View::Machine(machines::prepare_inventory(conn, principal, found).await?)
        }
        ResourceKind::Mount => {
            View::Mount(mounts::prepare_inventory(ctx, conn, principal, &ids).await?)
        }
        ResourceKind::Skill => View::Skill(skills::prepare_inventory(conn, seen).await?),
        ResourceKind::Host => {
            View::Host(hosts::prepare_inventory(ctx, conn, principal, seen).await?)
        }
        ResourceKind::Environment => View::Environment(
            crate::routes::environments::prepare_inventory(ctx, conn, principal, seen).await?,
        ),
        ResourceKind::McpServer => View::Mcp(
            mcp::prepare(conn, principal, &ids, Some(seen), mcp::Style::Presets).await?,
            NameBounds::default(),
        ),
    };
    Ok(Some(Prepared::from_view(conn, view).await?))
}

impl Prepared {
    /// Both native cardinalities materialize their factual source roles here, once.
    pub async fn from_view(conn: &mut Conn, view: View) -> ApiResult<Self> {
        let mut foreign = Vec::new();
        let mut retained = Vec::new();
        let names = match &view {
            View::Agent(prepared) => {
                foreign.extend(prepared.provider_ids().iter().map(|id| ObjectRef {
                    kind: ObjectKind::Provider,
                    id: *id,
                }));
                foreign.extend(prepared.machine_ids().iter().map(|id| ObjectRef {
                    kind: ObjectKind::Machine,
                    id: *id,
                }));
                prepared.name_sources().clone()
            }
            View::Provider(prepared) => prepared.name_sources().clone(),
            View::IndividualProvider(prepared) => prepared.name_sources(),
            View::Template(prepared) => prepared.name_sources().clone(),
            View::Machine(prepared) => {
                foreign.extend(prepared.reference_sources().iter().copied());
                prepared.name_sources().clone()
            }
            View::Mount(prepared) => {
                native_sources(
                    &mut foreign,
                    &resource_reads::MOUNT.sources[0],
                    prepared.machine_ids(),
                );
                prepared.name_sources().clone()
            }
            View::Skill(prepared) => prepared.name_sources().clone(),
            View::Host(prepared) => {
                retained.extend(prepared.retained_name_sources().iter().map(|id| ObjectRef {
                    kind: ObjectKind::Environment,
                    id: *id,
                }));
                foreign.extend(retained.iter().copied());
                prepared.name_sources().clone()
            }
            View::Environment(prepared) | View::IndividualEnvironment(prepared, _) => {
                for object in prepared
                    .content_sources(conn)
                    .await?
                    .into_values()
                    .flatten()
                {
                    if matches!(
                        object.kind,
                        ObjectKind::Template | ObjectKind::Host | ObjectKind::Machine
                    ) {
                        retained.push(object);
                    }
                    foreign.push(object);
                }
                prepared.name_sources().clone()
            }
            View::Mcp(prepared, _) => {
                foreign.extend(prepared.machines.iter().map(|id| ObjectRef {
                    kind: ObjectKind::Machine,
                    id: *id,
                }));
                foreign.extend(prepared.environments.iter().map(|id| ObjectRef {
                    kind: ObjectKind::Environment,
                    id: *id,
                }));
                prepared.names.clone()
            }
        };
        foreign.sort_unstable_by_key(|object| (object.kind, object.id));
        foreign.dedup();
        Ok(Self {
            view,
            names,
            foreign,
            retained: retained
                .into_iter()
                .map(|object| (object.kind, object.id))
                .collect(),
        })
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_f101738fc4, __api_install_f101738fc4, __api_callback_f101738fc4;
    slot __ApiCallbackf101738fc4, __API_CALLBACK_F101738FC4, __api_set_f101738fc4;
    base orch_api_base::routes::resources::projection;

    fn native_sources<'__api>(arg0: &'__api mut Vec<orch_access_contract::decisions::ObjectRef>, arg1: &'__api orch_api_contract::resource_reads::SourceRole, arg2: &'__api [orch_types::Id]) -> ();
}

orch_api_base::api_family_callback! {
    bind __api_bind_19ca020a5c, __api_install_19ca020a5c, __api_callback_19ca020a5c;
    slot __ApiCallback19ca020a5c, __API_CALLBACK_19CA020A5C, __api_set_19ca020a5c;
    base orch_api_base::routes::resources::projection;

    fn bind_collection_source<'__api>(arg0: &'__api mut orch_api_base::key_operations::admission::Prepared, arg1: &'static str, arg2: orch_access_contract::key_scope::KeyTargetKind, arg3: &'__api [orch_access_contract::key_scope::KeyResolvedTarget]) -> orch_api_base::middleware::requests::ApiResult<()>;
}

/// Stored sources selected by the native preparation, not identities inferred from a DTO.
pub enum Source<'a> {
    Resources {
        role: &'static str,
        kind: ResourceKind,
        ids: &'a [Id],
    },
    /// Physical Machine content keeps its actual Host/Machine governor and Machine family.
    Machines { role: &'static str, ids: &'a [Id] },
    /// Native kept physical references retain that same governor without opening its live page.
    MachineReferences { role: &'static str, ids: &'a [Id] },
}

pub async fn bind_source(
    conn: &mut Conn,
    plan: &mut OperationPlan,
    source: &Source<'_>,
) -> ApiResult<()> {
    let (role, kind, ids, mut found) = match source {
        Source::Resources { role, kind, ids } => {
            let sources =
                crate::routes::nodes::read_sources::Sources::from_objects(ids.iter().map(|id| {
                    ObjectRef {
                        kind: (*kind).into(),
                        id: *id,
                    }
                }));
            let found = sources
                .identity_targets(conn)
                .await?
                .into_iter()
                .map(|(object, target)| (object.id, target))
                .collect();
            (*role, KeyTargetKind::Object((*kind).into()), *ids, found)
        }
        Source::Machines { role, ids } => (
            *role,
            KeyTargetKind::Object(ObjectKind::Machine),
            *ids,
            targets::machine_subjects(conn, ids).await?,
        ),
        Source::MachineReferences { role, ids } => (
            *role,
            KeyTargetKind::Object(ObjectKind::Machine),
            *ids,
            targets::machine_identities(conn, ids).await?,
        ),
    };
    let facts = ids
        .iter()
        .map(|id| found.remove(id).ok_or_else(ApiError::not_found))
        .collect::<ApiResult<Vec<_>>>()?;
    bind_collection_source(plan, role, kind, &facts)
}

#[path = "projection/single.rs"]
pub mod single;
