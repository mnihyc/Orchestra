//! Individual Machine metadata and the actual native readiness sources behind it.

use super::{
    MachineRow, PreparedInventory, own_machine_on, prepare_inventory, queries, read,
    show_inventory_prepared,
};
use crate::key_operations::{native, publication};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::access::operations::projection_names;
use crate::routes::access::page::{NameBounds, NameSources};
use crate::routes::resources::view::environment_refs_from_rows_with_names;
use crate::sandbox_readiness;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::machine_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::FacetUsage;
use orch_api_contract::resources::Machine;
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_engine::operation_scope::targets::{self, TargetError};
use orch_types::{Id, Time};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const MACHINE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);

pub(super) struct MachineRead;
pub(super) struct Opened {
    native: Prepared,
    bounds: NameBounds,
    named_nodes: BTreeSet<Id>,
}
pub(super) struct Rendered {
    ctx: Ctx,
    accepted: Caller,
    opened: Arc<Opened>,
    machine: Machine,
}

/// The native individual factory, also used by the ordinary policy/readback path.
pub(super) struct Prepared {
    inventory: PreparedInventory,
    hosted: Vec<queries::EnvironmentRefRow>,
    readiness: Vec<(Id, sandbox_readiness::Prepared)>,
    names: NameSources,
}

impl Prepared {
    pub(super) async fn load(
        conn: &mut Conn,
        principal: &Principal,
        row: MachineRow,
    ) -> ApiResult<Self> {
        let id = row.id;
        let inventory = prepare_inventory(conn, principal, vec![row]).await?;
        let mut ids: BTreeSet<_> = inventory
            .registered
            .environments
            .iter()
            .map(|row| row.id)
            .collect();
        let hosted = orch_db::queries::environment_facts::hosted_resources(conn, id).await?;
        let mut hosted = queries::environments_among(conn, &hosted, &[]).await?;
        hosted.retain(|row| ids.insert(row.id));
        let mut readiness = Vec::new();
        for row in inventory.registered.environments.iter().chain(&hosted) {
            readiness.push((
                row.id,
                sandbox_readiness::prepare(conn, principal, row.id, None, None, None).await?,
            ));
        }
        let mut names = inventory.name_sources().clone();
        names.nodes.extend(
            orch_authz::resources::address_name_sources(
                conn,
                &hosted.iter().map(|row| row.id).collect::<Vec<_>>(),
            )
            .await
            .map_err(crate::middleware::authorization::restoration_error)?,
        );
        Ok(Self {
            inventory,
            hosted,
            readiness,
            names,
        })
    }

    fn objects(&self) -> Vec<ObjectRef> {
        let mut objects = self.inventory.reference_sources().to_vec();
        objects.extend(self.hosted.iter().map(|row| ObjectRef {
            kind: ObjectKind::Environment,
            id: row.id,
        }));
        objects.sort_unstable_by_key(|object| (object.kind, object.id));
        objects.dedup();
        objects
    }

    fn machines(&self) -> Vec<Id> {
        self.readiness
            .iter()
            .filter_map(|(_, ready)| ready.machine())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Narrow only native references; the primary Machine and its intrinsic metadata stay.
    async fn restrict(
        &mut self,
        conn: &mut Conn,
        objects: &BTreeSet<(ObjectKind, Id)>,
        machines: &BTreeSet<Id>,
    ) -> ApiResult<()> {
        self.inventory
            .registered
            .providers
            .retain(|row| objects.contains(&(ObjectKind::Provider, row.id)));
        self.inventory
            .registered
            .agents
            .retain(|row| objects.contains(&(ObjectKind::Agent, row.id)));
        self.inventory
            .registered
            .environments
            .retain(|row| objects.contains(&(ObjectKind::Environment, row.id)));
        self.inventory
            .registered
            .sources
            .retain(|source| objects.contains(&(source.kind, source.id)));
        self.readiness.retain(|(id, ready)| {
            objects.contains(&(ObjectKind::Environment, *id))
                && ready.machine().is_none_or(|id| machines.contains(&id))
        });
        let ready: BTreeSet<_> = self.readiness.iter().map(|(id, _)| *id).collect();
        self.hosted.retain(|row| ready.contains(&row.id));
        self.names = self.inventory.access.names.clone();
        let objects: Vec<_> = self.objects().iter().map(|object| object.id).collect();
        let nodes = orch_authz::resources::address_name_sources(conn, &objects)
            .await
            .map_err(crate::middleware::authorization::restoration_error)?;
        self.names.nodes.extend(nodes);
        self.inventory.sources = self.names.clone();
        Ok(())
    }

    fn preserved_by(&self, fresh: &Self) -> bool {
        let readiness: BTreeMap<_, _> = fresh
            .readiness
            .iter()
            .map(|(id, value)| (*id, value))
            .collect();
        self.inventory.preserves_sources(&fresh.inventory)
            && self.readiness.iter().all(|(id, old)| {
                readiness
                    .get(id)
                    .is_some_and(|current| old.preserved_by(current))
            })
    }

    pub(super) async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<Machine> {
        let mut machine = show_inventory_prepared(ctx, conn, principal, &self.inventory)
            .await?
            .pop()
            .ok_or_else(|| ApiError::internal("the individual machine was not projected"))?;
        let mut environments: BTreeMap<_, _> = machine
            .environments
            .iter()
            .map(|row| (row.id, row.clone()))
            .collect();
        let names = self
            .inventory
            .bounds
            .limited
            .then_some(&self.inventory.bounds);
        environments.extend(
            environment_refs_from_rows_with_names(
                conn,
                self.hosted.clone(),
                Time::new(ctx.now),
                names,
            )
            .await?
            .into_iter()
            .map(|(_, row)| (row.id, row)),
        );
        for (id, prepared) in &self.readiness {
            let reference = environments.remove(id).ok_or_else(|| {
                ApiError::internal("the prepared readiness environment was not projected")
            })?;
            machine
                .environment_sandboxes
                .push(prepared.render(reference)?);
        }
        Ok(machine)
    }
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the individual Machine source is no longer admitted",
    )
}
fn target_error(error: TargetError) -> ApiError {
    match error {
        TargetError::Missing => ApiError::not_found(),
        TargetError::Db(error) => error.into(),
        TargetError::Authz(error) => ApiError::internal(error),
    }
}

async fn load(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<(Arc<Opened>, Vec<native::Facts<Source>>)> {
    prepare_for(ctx, conn, caller, ctx.id("id")?, &sources::GET, Vec::new()).await
}

/// The same actual Machine source under the consuming read's canonical declaration.
pub(super) async fn prepare_for(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    id: Id,
    declaration: &'static Declaration<Source>,
    mut facts: Vec<native::Facts<Source>>,
) -> ApiResult<(Arc<Opened>, Vec<native::Facts<Source>>)> {
    let mut targets = targets::machine_identities(conn, &[id])
        .await
        .map_err(target_error)?;
    let primary = targets.remove(&id).ok_or_else(ApiError::not_found)?;
    let governor = declaration
        .bindings
        .iter()
        .find(|binding| {
            binding.source == Source::Governor
                && binding.usage == orch_api_contract::operations::FacetUsage::Required
        })
        .ok_or_else(|| ApiError::internal("the native Machine reader has no governor role"))?;
    facts.push(match governor.cardinality {
        orch_api_contract::operation_sources::Cardinality::One => {
            native::Facts::one(Source::Governor, MACHINE, primary)
        }
        orch_api_contract::operation_sources::Cardinality::Collection => {
            native::Facts::collection(Source::Governor, vec![(MACHINE, vec![primary])])
        }
    });
    let known = native::bind_known(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    let row = own_machine_on(conn, &principal, id).await?;
    let prepared = Prepared::load(conn, &principal, row).await?;
    bound(
        ctx,
        conn,
        caller,
        &principal,
        prepared,
        Scope {
            declaration,
            facts,
            usage: FacetUsage::Required,
        },
    )
    .await
}

/// The same native individual renderer after its producer's own primary response Read.
/// This path never re-admits a completed management action.
pub(super) async fn prepare_response(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: MachineRow,
    declaration: &'static Declaration<Source>,
) -> ApiResult<(Arc<Opened>, Vec<native::Facts<Source>>)> {
    let id = row.id;
    let mut identities = targets::machine_identities(conn, &[id])
        .await
        .map_err(target_error)?;
    let target = identities.remove(&id).ok_or_else(ApiError::not_found)?;
    let facts = vec![native::Facts::one(Source::Governor, MACHINE, target)];
    let prepared = Prepared::load(conn, principal, row).await?;
    bound(
        ctx,
        conn,
        principal.caller(),
        principal,
        prepared,
        Scope {
            declaration,
            facts,
            usage: FacetUsage::ResponseContent,
        },
    )
    .await
}

struct Scope {
    declaration: &'static Declaration<Source>,
    facts: Vec<native::Facts<Source>>,
    usage: FacetUsage,
}

async fn bound(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    principal: &Principal,
    mut prepared: Prepared,
    scope: Scope,
) -> ApiResult<(Arc<Opened>, Vec<native::Facts<Source>>)> {
    let Scope {
        declaration,
        mut facts,
        usage,
    } = scope;
    let known = native::bind_known(ctx, declaration, &facts)?;
    let candidates = read::registered_facts(conn, &prepared.objects()).await?;
    let machines = prepared.machines();
    let mut physical = targets::machine_identities(conn, &machines)
        .await
        .map_err(target_error)?;
    if native::modern(caller) {
        let mut allowed = BTreeSet::new();
        for (kind, targets) in &candidates.groups {
            for target in targets {
                if native::permits_usage(
                    &known,
                    declaration,
                    caller,
                    &Source::Registered,
                    usage,
                    *kind,
                    target,
                )? {
                    let KeyTargetKind::Object(kind) = kind else {
                        return Err(ApiError::internal(
                            "a Machine registration has an identity source",
                        ));
                    };
                    let id = target.object.id.ok_or_else(ApiError::not_found)?;
                    allowed.insert((*kind, id));
                }
            }
        }
        let mut allowed_machines = BTreeSet::new();
        for id in &machines {
            let target = physical.get(id).ok_or_else(ApiError::not_found)?;
            if native::permits_usage(
                &known,
                declaration,
                caller,
                &Source::ReadinessMachines,
                usage,
                MACHINE,
                target,
            )? {
                allowed_machines.insert(*id);
            }
        }
        prepared.restrict(conn, &allowed, &allowed_machines).await?;
    }
    facts.push(read::registered_facts(conn, &prepared.objects()).await?);
    let machines = prepared.machines();
    let physical = machines
        .iter()
        .map(|id| physical.remove(id).ok_or_else(ApiError::not_found))
        .collect::<ApiResult<Vec<_>>>()?;
    facts.push(native::Facts::collection(
        Source::ReadinessMachines,
        if physical.is_empty() {
            Vec::new()
        } else {
            vec![(MACHINE, physical)]
        },
    ));
    let mut plan = native::bind_known(ctx, declaration, &facts)?;
    let bounds = projection_names(
        conn,
        caller,
        &mut plan,
        prepared.names.clone(),
        &read::NAMES,
    )
    .await?;
    let named_nodes = read::named_nodes(conn, principal, &bounds.nodes).await?;
    facts.extend(read::projection_facts_for(&plan, usage)?);
    prepared.inventory.bound_names(bounds.clone());
    Ok((
        Arc::new(Opened {
            native: prepared,
            bounds,
            named_nodes,
        }),
        facts,
    ))
}

impl Opened {
    pub(super) fn preserved_by(&self, fresh: &Self) -> bool {
        self.native.preserved_by(&fresh.native)
            && self.bounds.nodes.is_subset(&fresh.bounds.nodes)
            && self.named_nodes.is_subset(&fresh.named_nodes)
    }
    pub(super) async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<Machine> {
        self.native.render(ctx, conn, principal).await
    }
}

impl native::Selected for MachineRead {
    type Source = Source;
    type Query = ();
    type Value = Arc<Opened>;
    type Reply = Rendered;
    const DECLARATION: &'static Declaration<Source> = &sources::GET;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(load(ctx, conn, caller))
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Rendered>> {
        Box::pin(async move {
            Ok(Rendered {
                ctx: ctx.clone(),
                accepted: principal.caller().clone(),
                opened: Arc::clone(value),
                machine: value.render(ctx, conn, principal).await?,
            })
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a (),
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.preserved_by(fresh)) })
    }
}

/// Retain the actual native individual source for ordinary and admitted HTTP publication.
pub(super) async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Machine> {
    let rendered = native::selected_read::<MachineRead>(ctx, (), ()).await?;
    publication::retain(
        &rendered.ctx,
        &rendered.accepted,
        rendered.opened,
        |ctx, conn, reader, old| {
            Box::pin(async move {
                let (fresh, facts) = load(ctx, conn, reader.caller()).await?;
                if !old.preserved_by(&fresh) {
                    return Err(refused());
                }
                if native::modern(reader.caller()) {
                    native::bind(ctx, &sources::GET, &facts)?.admit(reader.caller())?;
                }
                Ok(())
            })
        },
    )?;
    Ok(rendered.machine)
}
