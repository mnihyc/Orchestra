//! Native mixed resource fleet consumption; resource candidate/window policy stays shared.

use super::collection::Collection;

use super::collection::Query;

use super::collection::Window;

use super::*;

use crate::key_operations::native;

use crate::routes::environments::hosted;

use crate::routes::nodes::read_sources::Sources;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::environment::HostLoad;

use orch_api_contract::environment::NodeEnvironmentList;

use orch_api_contract::environment::NodeEnvironmentListQuery;

use orch_api_contract::environment::NodeEnvironmentListReply;

use orch_api_contract::environment_sources as sources;

use orch_api_contract::environment_sources::FleetReadSource as Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_authz::Caller;

use orch_authz::engine::Graph;

use orch_authz::resources::Seen;

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use std::collections::HashMap;
pub(crate) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        NodeEnvironmentList,
        native::selected::<Fleet>,
        native::selected_read::<Fleet>,
    );
}
orch_api_base::__api_bind_da9ef22327!();

struct Fleet;

struct Prepared {
    environments: hosted::PreparedFleet,
    hosts: Vec<HostLoad>,
}

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

fn managed<'a>(
    _conn: &'a mut Conn,
    _principal: &'a Principal,
    rows: Vec<Seen>,
    _query: &'a (),
) -> BoxFuture<'a, ApiResult<Vec<Seen>>> {
    Box::pin(async move {
        Ok(rows
            .into_iter()
            .filter(|row| {
                row.held
                    .iter()
                    .any(|held| held.permission == Permission::HostManage)
            })
            .collect())
    })
}

async fn references(
    conn: &mut Conn,
    plan: &crate::registry::OperationPlan,
    caller: &Caller,
    source: Source,
    kind: ObjectKind,
    ids: BTreeSet<Id>,
) -> ApiResult<(native::Facts<Source>, BTreeSet<Id>)> {
    let current = Sources::from_objects(ids.into_iter().map(|id| ObjectRef { kind, id }))
        .identity_targets(conn)
        .await?;
    let mut selected = Vec::new();
    let mut names = BTreeSet::new();
    for (object, target) in current {
        if native::permits_usage(
            plan,
            &sources::FLEET,
            caller,
            &source,
            FacetUsage::ResponseContent,
            KeyTargetKind::Object(kind),
            &target,
        )? {
            names.insert(object.id);
            selected.push(target);
        }
    }
    let groups = if selected.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([(KeyTargetKind::Object(kind), selected)])
    };
    Ok((collection(source, groups), names))
}

impl native::Selected for Fleet {
    type Source = Source;
    type Query = NodeEnvironmentListQuery;
    type Value = Prepared;
    type Reply = NodeEnvironmentListReply;
    const DECLARATION: &'static Declaration<Source> = &sources::FLEET;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let under =
                orch_db::queries::environments::hosting::environments_under(conn, ctx.id("id")?)
                    .await?;
            let modern = native::modern(caller);
            let plan = native::bind_known(ctx, Self::DECLARATION, &[])?;
            let environment = Collection::new(ResourceKind::Environment);
            let hosts = Collection::new(ResourceKind::Host);
            let mut targets = if modern {
                environment.targets(conn, &under).await?
            } else {
                HashMap::new()
            };
            let mut selected_under = Vec::new();
            for id in under {
                if !modern {
                    selected_under.push(id);
                } else if let Some(target) = targets.get(&id)
                    && native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::Subjects,
                        KeyTargetKind::Object(ObjectKind::Environment),
                        target,
                    )?
                {
                    selected_under.push(id);
                }
            }
            let principal = if !modern {
                native::preparation_principal(ctx, conn, caller, &plan).await?
            } else if let Some(target) = selected_under.first().and_then(|id| targets.get(id)) {
                let known = native::bind_known(
                    ctx,
                    Self::DECLARATION,
                    &[collection(
                        Source::Subjects,
                        BTreeMap::from([(
                            KeyTargetKind::Object(ObjectKind::Environment),
                            vec![target.clone()],
                        )]),
                    )],
                )?;
                native::preparation_principal(ctx, conn, caller, &known).await?
            } else if let Some(principal) = hosts.principal_for(ctx, conn, caller, &plan).await? {
                principal
            } else {
                // All native under-Environment identities and Host Looker candidates were exhausted.
                let empty = native::bind_known(
                    ctx,
                    Self::DECLARATION,
                    &[collection(Source::Subjects, BTreeMap::new())],
                )?;
                native::preparation_principal(ctx, conn, caller, &empty).await?
            };
            authorization::require_on_path_in(ctx, conn, &principal, "node.view").await?;
            let mut environments =
                hosted::prepare_fleet(ctx, conn, &principal, &selected_under, query).await?;
            let seen = hosts
                .selected(
                    conn,
                    &principal,
                    caller,
                    &plan,
                    Query {
                        window: Window {
                            after: None,
                            limit: LIST_LIMIT,
                        },
                        value: &(),
                        filter: managed,
                    },
                )
                .await?
                .items;
            let selected_hosts = seen.iter().map(|row| row.resource.id).collect::<Vec<_>>();
            let host_loads = hosted::host_loads(conn, &selected_hosts).await?;
            if !modern {
                return Ok((
                    Prepared {
                        environments,
                        hosts: host_loads,
                    },
                    Vec::new(),
                ));
            }
            let mut subjects = BTreeMap::new();
            let environments_targets = environments
                .sources
                .iter()
                .map(|source| {
                    targets
                        .remove(&source.environment)
                        .ok_or_else(ApiError::not_found)
                })
                .collect::<ApiResult<Vec<_>>>()?;
            if !environments_targets.is_empty() {
                subjects.insert(
                    KeyTargetKind::Object(ObjectKind::Environment),
                    environments_targets,
                );
            }
            let mut host_targets = hosts.targets(conn, &selected_hosts).await?;
            let selected = selected_hosts
                .iter()
                .map(|id| host_targets.remove(id).ok_or_else(ApiError::not_found))
                .collect::<ApiResult<Vec<_>>>()?;
            if !selected.is_empty() {
                subjects.insert(KeyTargetKind::Object(ObjectKind::Host), selected);
            }
            let mut facts = vec![collection(Source::Subjects, subjects)];
            let (host_facts, host_names) = references(
                conn,
                &plan,
                caller,
                Source::Hosts,
                ObjectKind::Host,
                environments
                    .sources
                    .iter()
                    .filter_map(|source| source.host)
                    .collect(),
            )
            .await?;
            let (template_facts, templates) = references(
                conn,
                &plan,
                caller,
                Source::Templates,
                ObjectKind::Template,
                environments
                    .sources
                    .iter()
                    .filter_map(|source| source.template)
                    .collect(),
            )
            .await?;
            let (provider_facts, providers) = references(
                conn,
                &plan,
                caller,
                Source::Providers,
                ObjectKind::Provider,
                environments
                    .sources
                    .iter()
                    .flat_map(|source| source.providers.iter().copied())
                    .collect(),
            )
            .await?;
            facts.extend([host_facts, template_facts, provider_facts]);
            let homes: BTreeSet<_> = environments
                .sources
                .iter()
                .map(|source| source.home)
                .collect();
            let graph = Graph::load(conn, &homes.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(ApiError::internal)?;
            let (named, homes) = native::node_name_facts(
                &plan,
                Self::DECLARATION,
                caller,
                &graph,
                &homes,
                &[
                    (ObjectKind::Org, Source::Orgs),
                    (ObjectKind::Project, Source::Projects),
                    (ObjectKind::Topic, Source::Topics),
                    (ObjectKind::Thread, Source::Threads),
                ],
            )?;
            facts.extend(named);
            environments
                .disclosure
                .names
                .retain(|id| homes.contains(id));
            environments
                .disclosure
                .displays
                .retain(|id| homes.contains(id));
            environments.bounds = hosted::FleetBounds {
                limited: true,
                homes,
                hosts: host_names,
                templates,
                providers,
            };
            Ok((
                Prepared {
                    environments,
                    hosts: host_loads,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require_on_path_in(ctx, conn, principal, "node.view").await
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<NodeEnvironmentListReply>> {
        Box::pin(async move {
            Ok(NodeEnvironmentListReply {
                entries: value.environments.render(conn, principal).await?,
                hosts: value.hosts.clone(),
            })
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Prepared,
        fresh: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.environments.preserves(&fresh.environments)
                && opened
                    .hosts
                    .iter()
                    .all(|old| fresh.hosts.iter().any(|now| now.host_id == old.host_id)))
        })
    }
}
