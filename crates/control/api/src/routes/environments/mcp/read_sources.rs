//! Native enabled-server rows consumed by the shared selected-row read adapter.

use super::*;
use crate::key_operations::{native, targets};
use crate::routes::nodes::read_sources::Sources;
use crate::routes::resources::mcp::projection as definitions;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::environment_sources::{self as sources, McpSource as Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::FacetUsage;
use orch_authz::{Caller, engine::Graph};
use std::collections::{BTreeMap, HashSet};

pub(super) struct Servers;

fn collection(
    source: Source,
    groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>,
) -> native::Facts<Source> {
    native::Facts::collection(
        source,
        groups
            .into_iter()
            .filter(|(_, targets)| !targets.is_empty())
            .collect(),
    )
}

fn selected(
    plan: &crate::registry::OperationPlan,
    caller: &Caller,
    source: Source,
    family: KeyTargetKind,
    target: &KeyResolvedTarget,
) -> ApiResult<bool> {
    native::permits_usage(
        plan,
        &sources::MCP_GET,
        caller,
        &source,
        FacetUsage::ResponseContent,
        family,
        target,
    )
}

impl native::Selected for Servers {
    type Source = Source;
    type Query = ();
    type Value = PreparedGet;
    type Reply = EnvironmentMcp;
    const DECLARATION: &'static Declaration<Source> = &sources::MCP_GET;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(PreparedGet, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            let modern = native::modern(caller);
            let mut facts = if modern {
                vec![native::Facts::one(
                    Source::Environment,
                    KeyTargetKind::Object(ObjectKind::Environment),
                    targets::page_object(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Environment,
                            id: environment,
                        },
                    )
                    .await?,
                )]
            } else {
                Vec::new()
            };
            let plan = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            require_get(conn, &principal, environment).await?;
            let candidates = orch_engine::mcp::resolve(conn, environment, principal.now())
                .await
                .map_err(ApiError::internal)?;
            let identities = if modern {
                Sources::from_objects(
                    candidates
                        .iter()
                        .map(|server| definitions::subject(&server.row)),
                )
                .identity_targets(conn)
                .await?
            } else {
                HashMap::new()
            };
            let mut resolved = Vec::new();
            let mut servers = BTreeMap::new();
            for server in candidates {
                if modern {
                    let family = KeyTargetKind::Object(ObjectKind::McpServer);
                    let target = identities
                        .get(&definitions::subject(&server.row))
                        .ok_or_else(ApiError::not_found)?;
                    if !selected(&plan, caller, Source::Servers, family, target)? {
                        continue;
                    }
                    servers
                        .entry(family)
                        .or_insert_with(Vec::new)
                        .push(target.clone());
                }
                resolved.push(server);
            }
            let mut prepared = prepare_get(ctx, conn, &principal, environment, resolved).await?;
            if !modern {
                return Ok((prepared, facts));
            }
            facts.push(collection(Source::Servers, servers));

            let mut machines = BTreeMap::new();
            let mut allowed_machines = BTreeSet::new();
            let mut identities = targets::machine_identities(
                conn,
                &prepared
                    .machines
                    .iter()
                    .map(|row| row.id)
                    .collect::<Vec<_>>(),
            )
            .await?;
            for machine in &prepared.machines {
                let family = KeyTargetKind::Object(ObjectKind::Machine);
                let target = identities
                    .remove(&machine.id)
                    .ok_or_else(ApiError::not_found)?;
                if selected(&plan, caller, Source::Machines, family, &target)? {
                    allowed_machines.insert(machine.id);
                    machines.entry(family).or_insert_with(Vec::new).push(target);
                }
            }
            prepared
                .machines
                .retain(|row| allowed_machines.contains(&row.id));
            facts.push(collection(Source::Machines, machines));

            let mut names = BTreeMap::new();
            let mut allowed = HashSet::new();
            let current = Sources::from_objects(prepared.library_visibility.sources())
                .identity_targets(conn)
                .await?;
            for (object, target) in current {
                let family = KeyTargetKind::Object(object.kind);
                if selected(&plan, caller, Source::LibraryNames, family, &target)? {
                    allowed.insert((object.kind, object.id));
                    names.entry(family).or_insert_with(Vec::new).push(target);
                }
            }
            prepared.library_visibility.restrict_to(&allowed);
            facts.push(collection(Source::LibraryNames, names));

            let graph = Graph::load(conn, &prepared.node_ids.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(ApiError::internal)?;
            let (node_facts, nodes) = native::node_name_facts(
                &plan,
                Self::DECLARATION,
                caller,
                &graph,
                &prepared.node_ids,
                &[
                    (ObjectKind::Org, Source::Orgs),
                    (ObjectKind::Project, Source::Projects),
                    (ObjectKind::Topic, Source::Topics),
                    (ObjectKind::Thread, Source::Threads),
                ],
            )?;
            prepared.node_ids = nodes;
            prepared
                .node_disclosure
                .names
                .retain(|id| prepared.node_ids.contains(id));
            prepared
                .node_disclosure
                .displays
                .retain(|id| prepared.node_ids.contains(id));
            facts.extend(node_facts);
            Ok((prepared, facts))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require_get(conn, principal, ctx.id("id")?).await })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a PreparedGet,
    ) -> BoxFuture<'a, ApiResult<EnvironmentMcp>> {
        Box::pin(async move { value.render(ctx, conn, principal).await })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a PreparedGet,
        fresh: &'a PreparedGet,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let current: HashMap<_, _> = fresh
                .resolved
                .iter()
                .map(|server| (server.row.id, server))
                .collect();
            let machines: BTreeSet<_> = fresh.machines.iter().map(|machine| machine.id).collect();
            let names: HashSet<_> = fresh.library_visibility.sources().collect();
            Ok(opened.environment == fresh.environment
                && (!opened.import_allowed || fresh.import_allowed)
                && opened.resolved.iter().all(|old| {
                    current.get(&old.row.id).is_some_and(|now| {
                        old.row.environment_id == now.row.environment_id
                            && old.row.machine_id == now.row.machine_id
                            && old.from == now.from
                    })
                })
                && opened
                    .machines
                    .iter()
                    .all(|machine| machines.contains(&machine.id))
                && opened.library_subjects.is_subset(&fresh.library_subjects)
                && opened
                    .node_disclosure
                    .names
                    .is_subset(&fresh.node_disclosure.names)
                && opened
                    .node_disclosure
                    .displays
                    .is_subset(&fresh.node_disclosure.displays)
                && opened
                    .library_visibility
                    .sources()
                    .all(|object| names.contains(&object)))
        })
    }
}
