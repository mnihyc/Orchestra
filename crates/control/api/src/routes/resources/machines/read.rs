//! Native physical Machine inventory through the shared selected-source read adapter.

use super::{
    Machine, MachineRow, PreparedInventory, prepare_list_inventory, queries,
    show_inventory_prepared,
};
use crate::key_operations::native;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, OperationPlan};
use crate::routes::access::operations::{NameRoles, projection_names};
use crate::routes::access::page::{NameBounds, NameSources};
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::machine_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::{Caller, Policy, Principal};
use orch_db::{Conn, DbError};
use orch_engine::operation_scope::targets::{self, TargetError};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet};

const MACHINE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);
pub(super) const NAMES: NameRoles = NameRoles {
    groups: Some(sources::GROUPS),
    nodes: [
        (ObjectKind::Org, sources::NODES[0]),
        (ObjectKind::Project, sources::NODES[1]),
        (ObjectKind::Topic, sources::NODES[2]),
        (ObjectKind::Thread, sources::NODES[3]),
    ],
    containers: None,
};

pub(super) struct Machines;
pub(super) struct Value {
    inventory: PreparedInventory,
    frozen: Frozen,
}
pub(super) struct Rendered {
    ctx: Ctx,
    accepted: Caller,
    rows: Vec<Machine>,
    frozen: Frozen,
}

struct Frozen {
    ids: Vec<Id>,
    registered: Vec<(Id, ObjectRef)>,
    names: NameSources,
    bounds: NameBounds,
    named_nodes: BTreeSet<Id>,
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the Machine inventory source is no longer admitted",
    )
}
fn target_error(error: TargetError) -> ApiError {
    match error {
        TargetError::Missing => ApiError::not_found(),
        TargetError::Db(error) => error.into(),
        TargetError::Authz(error) => ApiError::internal(error),
    }
}
fn collection(
    source: Source,
    kind: KeyTargetKind,
    targets: Vec<KeyResolvedTarget>,
) -> native::Facts<Source> {
    native::Facts::collection(
        source,
        if targets.is_empty() {
            Vec::new()
        } else {
            vec![(kind, targets)]
        },
    )
}

pub(super) async fn registered_facts(
    conn: &mut Conn,
    objects: &[ObjectRef],
) -> ApiResult<native::Facts<Source>> {
    let mut graph = orch_authz::engine::Graph::default();
    let ids: Vec<_> = objects.iter().map(|object| object.id).collect();
    let rows = orch_authz::resources::read_into(conn, &mut graph, &ids)
        .await
        .map_err(ApiError::internal)?;
    let rows: BTreeMap<_, _> = rows
        .into_iter()
        .map(|row| ((ObjectKind::from(row.kind), row.id), row))
        .collect();
    let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for object in objects {
        let row = rows
            .get(&(object.kind, object.id))
            .ok_or_else(ApiError::not_found)?;
        let target =
            targets::from_resource_identity(&graph, row).ok_or_else(ApiError::not_found)?;
        groups
            .entry(KeyTargetKind::Object(object.kind))
            .or_default()
            .push(target);
    }
    Ok(native::Facts::collection(
        Source::Registered,
        groups.into_iter().collect(),
    ))
}

/// The native names binder has already selected and bound its exact canonical roles.
fn declared(plan: &OperationPlan) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut facts: Vec<_> = sources::LIST
        .bindings
        .iter()
        .map(|binding| native::Facts::collection(binding.source, Vec::new()))
        .collect();
    for facet in plan
        .shared()
        .required_facets()
        .map_err(crate::key_operations::admission::binding_error)?
    {
        let source = match facet.resource_kind {
            KeyTargetKind::Object(ObjectKind::Machine) => Source::Governor,
            KeyTargetKind::Object(
                ObjectKind::Provider | ObjectKind::Agent | ObjectKind::Environment,
            ) => Source::Registered,
            KeyTargetKind::Identity(KeyIdentityKind::Group) => Source::Groups,
            KeyTargetKind::Object(ObjectKind::Org) => Source::Orgs,
            KeyTargetKind::Object(ObjectKind::Project) => Source::Projects,
            KeyTargetKind::Object(ObjectKind::Topic) => Source::Topics,
            KeyTargetKind::Object(ObjectKind::Thread) => Source::Threads,
            _ => {
                return Err(ApiError::internal(
                    "the native Machine list has an undeclared source",
                ));
            }
        };
        let fact = facts
            .iter_mut()
            .find(|fact| fact.source == source)
            .ok_or_else(refused)?;
        if let Some((_, targets)) = fact
            .groups
            .iter_mut()
            .find(|(kind, _)| *kind == facet.resource_kind)
        {
            targets.push(facet.target);
        } else {
            fact.groups.push((facet.resource_kind, vec![facet.target]));
        }
    }
    Ok(facts)
}

/// These native label roles are shared by inventory and individual projection declarations.
pub(super) fn projection_facts(plan: &OperationPlan) -> ApiResult<Vec<native::Facts<Source>>> {
    let facets = plan
        .shared()
        .required_facets()
        .map_err(crate::key_operations::admission::binding_error)?;
    Ok([
        (
            Source::Groups,
            KeyTargetKind::Identity(KeyIdentityKind::Group),
        ),
        (Source::Orgs, KeyTargetKind::Object(ObjectKind::Org)),
        (Source::Projects, KeyTargetKind::Object(ObjectKind::Project)),
        (Source::Topics, KeyTargetKind::Object(ObjectKind::Topic)),
        (Source::Threads, KeyTargetKind::Object(ObjectKind::Thread)),
    ]
    .into_iter()
    .map(|(source, kind)| {
        collection(
            source,
            kind,
            facets
                .iter()
                .filter(|facet| facet.resource_kind == kind)
                .map(|facet| facet.target.clone())
                .collect(),
        )
    })
    .collect())
}

fn registrations(prepared: &PreparedInventory) -> Vec<(Id, ObjectRef)> {
    prepared
        .registered
        .providers
        .iter()
        .filter_map(|row| {
            row.machine_id.map(|machine| {
                (
                    machine,
                    ObjectRef {
                        kind: ObjectKind::Provider,
                        id: row.id,
                    },
                )
            })
        })
        .chain(prepared.registered.agents.iter().filter_map(|row| {
            row.machine_id.map(|machine| {
                (
                    machine,
                    ObjectRef {
                        kind: ObjectKind::Agent,
                        id: row.id,
                    },
                )
            })
        }))
        .chain(prepared.registered.environments.iter().filter_map(|row| {
            row.machine_id.map(|machine| {
                (
                    machine,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: row.id,
                    },
                )
            })
        }))
        .collect()
}

pub(super) async fn named_nodes(
    conn: &mut Conn,
    principal: &Principal,
    ids: &BTreeSet<Id>,
) -> ApiResult<BTreeSet<Id>> {
    Ok(crate::routes::nodes::view::refs(
        conn,
        principal,
        &ids.iter().copied().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?
    .into_iter()
    .filter_map(|(id, node)| (node.name.is_some() || node.thread_display.is_some()).then_some(id))
    .collect())
}

impl native::Selected for Machines {
    type Source = Source;
    type Query = ();
    type Value = Value;
    type Reply = Rendered;
    const DECLARATION: &'static Declaration<Source> = &sources::LIST;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let found = queries::physical_machines(conn).await?;
            let ids: Vec<_> = found.iter().map(|row| row.id).collect();
            let targets = targets::machine_identities(conn, &ids)
                .await
                .map_err(target_error)?;
            let selection = native::bind_known(ctx, Self::DECLARATION, &[])?;
            let mut selected = Vec::new();
            let mut primary = Vec::new();
            for row in found {
                let target = targets.get(&row.id).ok_or_else(ApiError::not_found)?;
                if native::modern(caller)
                    && !native::permits(
                        &selection,
                        Self::DECLARATION,
                        caller,
                        &Source::Governor,
                        MACHINE,
                        target,
                    )?
                {
                    continue;
                }
                primary.push(target.clone());
                selected.push(row);
            }
            let known = if primary.is_empty() {
                native::bind(
                    ctx,
                    Self::DECLARATION,
                    &Self::DECLARATION
                        .bindings
                        .iter()
                        .map(|binding| native::Facts::collection(binding.source, Vec::new()))
                        .collect::<Vec<_>>(),
                )?
            } else {
                native::bind_known(
                    ctx,
                    Self::DECLARATION,
                    &[collection(Source::Governor, MACHINE, primary)],
                )?
            };
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let mut inventory = prepare_list_inventory(conn, &principal, selected).await?;
            let ids: Vec<_> = inventory.found.iter().map(|row| row.id).collect();
            let primary = ids
                .iter()
                .map(|id| targets.get(id).cloned().ok_or_else(ApiError::not_found))
                .collect::<ApiResult<Vec<_>>>()?;
            let mut plan = native::bind_known(
                ctx,
                Self::DECLARATION,
                &[
                    collection(Source::Governor, MACHINE, primary),
                    registered_facts(conn, inventory.reference_sources()).await?,
                ],
            )?;
            let names = inventory.name_sources().clone();
            let bounds = projection_names(conn, caller, &mut plan, names.clone(), &NAMES).await?;
            inventory.bound_names(bounds.clone());
            let named_nodes = named_nodes(conn, &principal, &bounds.nodes).await?;
            let frozen = Frozen {
                ids,
                registered: registrations(&inventory),
                names,
                bounds,
                named_nodes,
            };
            Ok((Value { inventory, frozen }, declared(&plan)?))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let _ = ctx.user()?;
            Ok(())
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<Rendered>> {
        Box::pin(async move {
            let frozen = Frozen {
                ids: value.frozen.ids.clone(),
                registered: value.frozen.registered.clone(),
                names: value.frozen.names.clone(),
                bounds: value.frozen.bounds.clone(),
                named_nodes: value.frozen.named_nodes.clone(),
            };
            Ok(Rendered {
                ctx: ctx.clone(),
                accepted: principal.caller().clone(),
                rows: show_inventory_prepared(ctx, conn, principal, &value.inventory).await?,
                frozen,
            })
        })
    }
}

async fn check(ctx: &Ctx, conn: &mut Conn, reader: &Principal, frozen: &Frozen) -> ApiResult<()> {
    let rows: Vec<MachineRow> = queries::physical_machines_among(conn, &frozen.ids).await?;
    if rows.len() != frozen.ids.len() {
        return Err(refused());
    }
    let targets = targets::machine_identities(conn, &frozen.ids)
        .await
        .map_err(target_error)?;
    let primary = frozen
        .ids
        .iter()
        .map(|id| targets.get(id).cloned().ok_or_else(refused))
        .collect::<ApiResult<Vec<_>>>()?;
    let mut objects: Vec<_> = frozen
        .registered
        .iter()
        .map(|(_, object)| *object)
        .collect();
    objects.sort_unstable_by_key(|object| (object.kind, object.id));
    objects.dedup();
    let mut plan = native::bind_known(
        ctx,
        &sources::LIST,
        &[
            collection(Source::Governor, MACHINE, primary),
            registered_facts(conn, &objects).await?,
        ],
    )?;
    let priming = if frozen.ids.is_empty() {
        native::bind(
            ctx,
            &sources::LIST,
            &sources::LIST
                .bindings
                .iter()
                .map(|binding| native::Facts::collection(binding.source, Vec::new()))
                .collect::<Vec<_>>(),
        )?
    } else {
        plan.clone()
    };
    let principal = native::preparation_principal(ctx, conn, reader.caller(), &priming).await?;
    let current = prepare_list_inventory(conn, &principal, rows).await?;
    if current.found.len() != frozen.ids.len() {
        return Err(refused());
    }
    let registered: BTreeSet<_> = registrations(&current)
        .into_iter()
        .map(|(machine, object)| (machine, object.kind, object.id))
        .collect();
    if frozen
        .registered
        .iter()
        .any(|(machine, object)| !registered.contains(&(*machine, object.kind, object.id)))
    {
        return Err(refused());
    }
    let names = projection_names(
        conn,
        reader.caller(),
        &mut plan,
        frozen.names.clone(),
        &NAMES,
    )
    .await?;
    if !frozen.bounds.nodes.is_subset(&names.nodes) {
        return Err(refused());
    }
    if !frozen
        .named_nodes
        .is_subset(&named_nodes(conn, &principal, &frozen.named_nodes).await?)
    {
        return Err(refused());
    }
    if native::modern(reader.caller()) {
        native::bind(ctx, &sources::LIST, &declared(&plan)?)?.admit(reader.caller())?;
    }
    Ok(())
}

pub(super) async fn list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Machine>> {
    let value = native::selected_read::<Machines>(ctx, (), ()).await?;
    let (mut tx, guard) = transaction_guard::begin(&value.ctx).await?;
    check(&value.ctx, &mut tx, guard.principal(), &value.frozen).await?;
    guard.check_deadline(&value.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    guard.check_deadline(&value.ctx)?;
    crate::key_operations::publication::retain(
        &value.ctx,
        &value.accepted,
        value.frozen,
        |ctx, conn, reader, frozen| Box::pin(check(ctx, conn, reader, frozen)),
    )?;
    Ok(value.rows)
}
