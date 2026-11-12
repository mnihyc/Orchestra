//! Actual physical-machine snapshots and selected native Mount/Environment sources.

use super::machine_projection as projection;

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use crate::routes::access::page::NameBounds;

use crate::routes::resources::collection::Collection;

use crate::routes::resources::view::addresses_with_names;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::mount_sources as sources;

use orch_api_contract::mount_sources::MachineSource as Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::engine::Graph;

use std::collections::HashMap;

use std::collections::HashSet;
pub(super) struct Mounts;

pub(super) struct Directories;

pub(super) struct MountsReady {
    machine: Id,
    native: projection::Mounts,
    grouping: crate::routes::resources::grouping::Prepared,
    names: NameBounds,
}

pub(super) struct DataReady {
    machine: Id,
    native: projection::Directories,
}

async fn primary(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
) -> ApiResult<(Id, Principal, OperationPlan, Vec<native::Facts<Source>>)> {
    let machine = ctx.id("id")?;
    let target = targets::machine_identities(conn, &[machine])
        .await?
        .remove(&machine)
        .ok_or_else(ApiError::not_found)?;
    let facts = vec![native::Facts::one(
        Source::Machine,
        KeyTargetKind::Object(ObjectKind::Machine),
        target,
    )];
    let plan = native::bind_known(ctx, declaration, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    Ok((machine, principal, plan, facts))
}

async fn authority(
    conn: &mut Conn,
    principal: &Principal,
    machine: Id,
    owner: Option<Id>,
    host: Option<Id>,
    holdings: &BTreeMap<Id, resources::Effective>,
    allow_use: bool,
) -> ApiResult<bool> {
    let owns = owner == Some(principal.user()) && principal.caller().reaches(None);
    let manages = matches!(
        resources::may_change_policy_key(
            conn,
            principal,
            machine,
            resources::KeyAbout::TheMachine,
            true,
            Policy::default()
        )
        .await
        .map_err(crate::middleware::authorization::restoration_error)?,
        resources::Keying::Allowed
    );
    let host = host.and_then(|id| holdings.get(&id));
    if !owns
        && !manages
        && !has(host, Permission::HostView)
        && !(allow_use && has(host, Permission::HostUse))
    {
        return Err(ApiError::not_found());
    }
    Ok(manages)
}

async fn holdings(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<BTreeMap<Id, resources::Effective>> {
    Ok(
        resources::effective_among(conn, principal, ids, Policy::default())
            .await
            .map_err(crate::middleware::authorization::restoration_error)?
            .into_iter()
            .map(|held| (held.object.id, held))
            .collect(),
    )
}

async fn candidates(
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    declaration: &'static Declaration<Source>,
    source: Source,
    kind: ResourceKind,
    ids: &[Id],
) -> ApiResult<(HashSet<Id>, HashMap<Id, KeyResolvedTarget>)> {
    let targets = Collection::new(kind).targets(conn, ids).await?;
    let business = KeyTargetKind::Object(ObjectKind::from(kind));
    let mut selected = HashSet::new();
    for id in ids {
        let Some(target) = targets.get(id) else {
            continue;
        };
        if !native::modern(caller)
            || native::permits(plan, declaration, caller, &source, business, target)?
        {
            selected.insert(*id);
        }
    }
    Ok((selected, targets))
}

fn selected(
    source: Source,
    kind: ObjectKind,
    ids: BTreeSet<Id>,
    mut targets: HashMap<Id, KeyResolvedTarget>,
) -> ApiResult<native::Facts<Source>> {
    let values = ids
        .into_iter()
        .map(|id| targets.remove(&id).ok_or_else(ApiError::not_found))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(native::Facts::collection(
        source,
        if values.is_empty() {
            Vec::new()
        } else {
            vec![(KeyTargetKind::Object(kind), values)]
        },
    ))
}

impl native::Selected for Mounts {
    type Source = Source;
    type Query = ();
    type Value = MountsReady;
    type Reply = api::MachineMounts;
    const DECLARATION: &'static Declaration<Source> = &sources::MACHINE_MOUNT_READ;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(MountsReady, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (machine, principal, plan, mut facts) =
                primary(ctx, conn, caller, Self::DECLARATION).await?;
            let snapshot = rows::on_machine(conn, machine)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let mounts: Vec<_> = snapshot.mounts.0.iter().map(|row| row.id).collect();
            let environments: Vec<_> = snapshot.environments.0.iter().map(|row| row.id).collect();
            let ids: Vec<_> = mounts
                .iter()
                .chain(environments.iter())
                .copied()
                .chain(snapshot.host_id)
                .collect();
            let held = holdings(conn, &principal, &ids).await?;
            let manages = authority(
                conn,
                &principal,
                machine,
                snapshot.owner_id,
                snapshot.host_id,
                &held,
                false,
            )
            .await?;
            let (allowed_mounts, mount_targets) = candidates(
                conn,
                caller,
                &plan,
                Self::DECLARATION,
                Source::Mounts,
                ResourceKind::Mount,
                &mounts,
            )
            .await?;
            let (allowed_environments, environment_targets) = candidates(
                conn,
                caller,
                &plan,
                Self::DECLARATION,
                Source::Environments,
                ResourceKind::Environment,
                &environments,
            )
            .await?;
            let native = projection::mounts(
                snapshot,
                &held,
                manages,
                &allowed_mounts,
                &allowed_environments,
            )?;
            facts.push(selected(
                Source::Mounts,
                ObjectKind::Mount,
                native.mounts(),
                mount_targets,
            )?);
            facts.push(selected(
                Source::Environments,
                ObjectKind::Environment,
                native.environments(),
                environment_targets,
            )?);
            let subjects = native
                .addresses
                .iter()
                .map(|&id| ("mount".to_owned(), id))
                .collect::<Vec<_>>();
            let grouping =
                crate::routes::resources::grouping::Prepared::select(conn, &subjects).await?;
            let selection = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let mut nodes: BTreeSet<_> = resources::address_name_sources(
                conn,
                &native.addresses.iter().copied().collect::<Vec<_>>(),
            )
            .await
            .map_err(crate::middleware::authorization::restoration_error)?
            .into_iter()
            .collect();
            nodes.extend(grouping.homes());
            let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(crate::middleware::authorization::restoration_error)?;
            let (names, allowed) = native::node_name_facts(
                &selection,
                Self::DECLARATION,
                caller,
                &graph,
                &nodes,
                &[
                    (ObjectKind::Org, Source::AddressOrgs),
                    (ObjectKind::Project, Source::AddressProjects),
                    (ObjectKind::Topic, Source::AddressTopics),
                    (ObjectKind::Thread, Source::AddressThreads),
                ],
            )?;
            facts.extend(names);
            Ok((
                MountsReady {
                    machine,
                    native,
                    grouping,
                    names: NameBounds {
                        limited: native::modern(caller),
                        nodes: allowed,
                        ..Default::default()
                    },
                },
                facts,
            ))
        })
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
        value: &'a MountsReady,
    ) -> BoxFuture<'a, ApiResult<api::MachineMounts>> {
        Box::pin(async move {
            let addresses = addresses_with_names(
                conn,
                &value.native.addresses.iter().copied().collect::<Vec<_>>(),
                Some(&value.names),
            )
            .await?;
            let mut reply = value.native.render(&addresses)?;
            reply.grouping = value
                .grouping
                .choices(&ctx.app, conn, principal, Some(&value.names), None)
                .await?;
            Ok(reply)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a (),
        old: &'a MountsReady,
        fresh: &'a MountsReady,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.machine == fresh.machine
                && old.grouping.preserves_sources(&fresh.grouping)
                && old.native.preserved_by(&fresh.native)
                && old.names.nodes.is_subset(&fresh.names.nodes))
        })
    }
}

impl native::Selected for Directories {
    type Source = Source;
    type Query = ();
    type Value = DataReady;
    type Reply = api::MachineEnvironmentData;
    const DECLARATION: &'static Declaration<Source> = &sources::MACHINE_DATA_READ;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(DataReady, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (machine, principal, plan, mut facts) =
                primary(ctx, conn, caller, Self::DECLARATION).await?;
            let snapshot = rows::environment_data(conn, machine)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let environments: Vec<_> = snapshot.environments.0.iter().map(|row| row.id).collect();
            let ids: Vec<_> = environments
                .iter()
                .copied()
                .chain(snapshot.host_id)
                .collect();
            let held = holdings(conn, &principal, &ids).await?;
            authority(
                conn,
                &principal,
                machine,
                snapshot.owner_id,
                snapshot.host_id,
                &held,
                true,
            )
            .await?;
            let (allowed, targets) = candidates(
                conn,
                caller,
                &plan,
                Self::DECLARATION,
                Source::Environments,
                ResourceKind::Environment,
                &environments,
            )
            .await?;
            let native = projection::directories(machine, snapshot, &held, &allowed)?;
            facts.push(selected(
                Source::Environments,
                ObjectKind::Environment,
                native.environments(),
                targets,
            )?);
            Ok((DataReady { machine, native }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a DataReady,
    ) -> BoxFuture<'a, ApiResult<api::MachineEnvironmentData>> {
        Box::pin(async move { value.native.render() })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a (),
        old: &'a DataReady,
        fresh: &'a DataReady,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.machine == fresh.machine && old.native.preserved_by(&fresh.native))
        })
    }
}
