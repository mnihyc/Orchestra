//! Native placement choices with resource windowing and factual source publication.

use super::collection::{Collection, Query, Window};
use super::*;
use crate::key_operations::{native, targets};
use crate::routes::environments::places;
use crate::routes::nodes::read_sources::Sources;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::environment::{NodePlaceList, Place, PlaceListQuery};
use orch_api_contract::environment_sources::{self as sources, PlaceReadSource as Source};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::operations::FacetUsage;
use orch_authz::{Caller, engine::Graph, resources::Seen};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        NodePlaceList,
        native::selected::<Places>,
        native::selected_read::<Places>,
    );
}

struct Places;

#[derive(Default)]
struct Bounds {
    limited: bool,
    hosts: BTreeSet<Id>,
    machines: BTreeSet<Id>,
    mounts: BTreeSet<Id>,
    topics: BTreeSet<Id>,
    names: crate::routes::access::page::NameBounds,
}

struct Prepared {
    template: Id,
    hosts: Vec<orch_engine::places::HostPlace>,
    environments: places::PreparedEnvironments,
    bounds: Bounds,
    grouping: super::grouping::Prepared,
}

fn collection(
    source: Source,
    groups: BTreeMap<KeyTargetKind, Vec<KeyResolvedTarget>>,
) -> native::Facts<Source> {
    native::Facts::collection(
        source,
        groups
            .into_iter()
            .filter(|(_, rows)| !rows.is_empty())
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
                    .any(|held| held.permission == Permission::EnvironmentManage)
            })
            .collect())
    })
}

fn usable<'a>(
    _conn: &'a mut Conn,
    _principal: &'a Principal,
    rows: Vec<Seen>,
    _query: &'a (),
) -> BoxFuture<'a, ApiResult<Vec<Seen>>> {
    Box::pin(async move {
        Ok(rows
            .into_iter()
            .filter(|row| {
                row.held.iter().any(|held| {
                    matches!(
                        held.permission,
                        Permission::HostUse | Permission::HostManage
                    )
                })
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
    let actual = Sources::from_objects(ids.into_iter().map(|id| ObjectRef { kind, id }))
        .identity_targets(conn)
        .await?;
    let mut allowed = BTreeSet::new();
    let mut selected = Vec::new();
    for (object, target) in actual {
        if native::permits_usage(
            plan,
            &sources::PLACE_LIST,
            caller,
            &source,
            FacetUsage::ResponseContent,
            KeyTargetKind::Object(kind),
            &target,
        )? {
            allowed.insert(object.id);
            selected.push(target);
        }
    }
    Ok((
        collection(
            source,
            BTreeMap::from([(KeyTargetKind::Object(kind), selected)]),
        ),
        allowed,
    ))
}

impl native::Selected for Places {
    type Source = Source;
    type Query = PlaceListQuery;
    type Value = Prepared;
    type Reply = Vec<Place>;
    const DECLARATION: &'static Declaration<Source> = &sources::PLACE_LIST;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PlaceListQuery,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let home = ctx.id("id")?;
            let modern = native::modern(caller);
            let mut facts = Vec::new();
            if modern {
                let home = targets::node(conn, home).await?;
                facts.push(native::Facts::one(Source::Home, home.object.kind, home));
            }
            let initial = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &initial).await?;
            authorization::require_on_path_in(ctx, conn, &principal, "node.interact").await?;
            let request = places::prepare_request(conn, home, query).await?;
            if modern {
                let template = targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Template,
                        id: request.template.id,
                    },
                )
                .await?;
                facts.push(native::Facts::one(
                    Source::Template,
                    KeyTargetKind::Object(ObjectKind::Template),
                    template,
                ));
            }
            let plan = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = if modern {
                native::preparation_principal(ctx, conn, caller, &plan).await?
            } else {
                principal
            };
            let hosts = Collection::new(ResourceKind::Host);
            let environments = Collection::new(ResourceKind::Environment);
            let host_choices = if modern {
                let selected = hosts
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
                            filter: usable,
                        },
                    )
                    .await?
                    .items;
                let ids = selected
                    .iter()
                    .map(|row| row.resource.id)
                    .collect::<Vec<_>>();
                places::prepare_hosts_among(conn, &principal, home, &request, ctx.now.into(), &ids)
                    .await?
            } else {
                places::prepare_hosts(conn, &principal, home, &request, ctx.now.into()).await?
            };
            let seen = environments
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
            let environment_choices =
                places::prepare_environments(ctx, conn, &principal, &seen).await?;
            let grouped_subjects = host_choices
                .iter()
                .map(|host| ("host".to_owned(), host.host.id))
                .chain(
                    environment_choices
                        .sources
                        .iter()
                        .map(|row| ("environment".to_owned(), row.environment)),
                )
                .collect::<Vec<_>>();
            let grouping = super::grouping::Prepared::select(conn, &grouped_subjects).await?;
            let mut value = Prepared {
                grouping,
                template: request.template.id,
                hosts: host_choices,
                environments: environment_choices,
                bounds: Bounds::default(),
            };
            if !modern {
                return Ok((value, facts));
            }
            let mut subjects = BTreeMap::new();
            for (kind, selected) in [
                (
                    ResourceKind::Host,
                    value
                        .hosts
                        .iter()
                        .map(|host| host.host.id)
                        .collect::<Vec<_>>(),
                ),
                (
                    ResourceKind::Environment,
                    value
                        .environments
                        .sources
                        .iter()
                        .map(|row| row.environment)
                        .collect(),
                ),
            ] {
                let targets = Collection::new(kind).targets(conn, &selected).await?;
                let selected = selected
                    .iter()
                    .map(|id| targets.get(id).cloned().ok_or_else(ApiError::not_found))
                    .collect::<ApiResult<Vec<_>>>()?;
                subjects.insert(KeyTargetKind::Object(kind.into()), selected);
            }
            facts.push(collection(Source::Subjects, subjects));
            let (host_facts, host_names) = references(
                conn,
                &plan,
                caller,
                Source::Hosts,
                ObjectKind::Host,
                value
                    .environments
                    .sources
                    .iter()
                    .filter_map(|row| row.host)
                    .collect(),
            )
            .await?;
            let origin_ids = value.grouping.origins().into_iter().collect::<Vec<_>>();
            let origin_ids =
                orch_db::queries::resources::routes::physical_machines_among(conn, &origin_ids)
                    .await?
                    .into_iter()
                    .map(|row| row.id)
                    .collect::<Vec<_>>();
            let (machine_facts, machine_names) = references(
                conn,
                &plan,
                caller,
                Source::Machines,
                ObjectKind::Machine,
                value
                    .environments
                    .sources
                    .iter()
                    .filter_map(|row| row.machine)
                    .chain(origin_ids)
                    .collect(),
            )
            .await?;
            let (mount_facts, mount_names) = references(
                conn,
                &plan,
                caller,
                Source::Mounts,
                ObjectKind::Mount,
                value
                    .hosts
                    .iter()
                    .flat_map(|host| host.mount_sources.iter().copied())
                    .collect(),
            )
            .await?;
            facts.extend([host_facts, machine_facts, mount_facts]);
            let mut name_ids: BTreeSet<_> = value
                .environments
                .sources
                .iter()
                .flat_map(|row| row.topics.iter().copied())
                .collect();
            name_ids.extend(value.grouping.homes());
            let graph = Graph::load(conn, &name_ids.iter().copied().collect::<Vec<_>>())
                .await
                .map_err(ApiError::internal)?;
            name_ids.extend(graph.nodes().map(|node| node.id));
            let (topic_facts, names) = native::node_name_facts(
                &plan,
                Self::DECLARATION,
                caller,
                &graph,
                &name_ids,
                &[
                    (ObjectKind::Org, Source::Orgs),
                    (ObjectKind::Project, Source::Projects),
                    (ObjectKind::Topic, Source::Topics),
                    (ObjectKind::Thread, Source::Threads),
                ],
            )?;
            let topics = names
                .iter()
                .copied()
                .filter(|id| {
                    graph
                        .get(*id)
                        .is_some_and(|node| node.kind.as_str() == "topic")
                })
                .collect::<BTreeSet<_>>();
            facts.extend(topic_facts);
            value
                .environments
                .disclosure
                .names
                .retain(|id| topics.contains(id));
            value
                .environments
                .disclosure
                .displays
                .retain(|id| topics.contains(id));
            value.bounds = Bounds {
                limited: true,
                hosts: host_names,
                machines: machine_names,
                mounts: mount_names,
                topics,
                names: crate::routes::access::page::NameBounds {
                    limited: true,
                    nodes: names,
                    ..Default::default()
                },
            };
            Ok((value, facts))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require_on_path_in(ctx, conn, principal, "node.interact").await
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<Vec<Place>>> {
        Box::pin(async move {
            let mut hosts = value.hosts.clone();
            if value.bounds.limited {
                for host in &mut hosts {
                    host.place
                        .mounts
                        .retain(|mount| value.bounds.mounts.contains(&mount.mount.id));
                }
            }
            places::group_host_choices(
                &ctx.app,
                conn,
                principal,
                &mut hosts,
                Some(&value.bounds.names),
                value.bounds.limited.then_some(&value.bounds.machines),
            )
            .await?;
            places::name_host_choices(conn, &mut hosts, Some(&value.bounds.names)).await?;
            let mut result = hosts
                .into_iter()
                .map(|host| Place::Host(host.place))
                .collect::<Vec<_>>();
            result.extend(
                value
                    .environments
                    .render(
                        ctx,
                        conn,
                        principal,
                        places::EnvironmentNames {
                            limited: value.bounds.limited,
                            hosts: &value.bounds.hosts,
                            machines: &value.bounds.machines,
                            topics: &value.bounds.topics,
                        },
                    )
                    .await?
                    .into_iter()
                    .map(Place::Environment),
            );
            let grouped = value
                .grouping
                .render(
                    &ctx.app,
                    conn,
                    principal,
                    Some(&value.bounds.names),
                    value.bounds.limited.then_some(&value.bounds.machines),
                )
                .await?;
            for place in &mut result {
                if let Place::Environment(environment) = place
                    && let Some(metadata) =
                        grouped.get(&("environment".to_owned(), environment.environment.id))
                {
                    let availability = std::mem::take(&mut environment.grouping.availability);
                    environment.grouping = metadata.clone();
                    environment.grouping.availability = availability;
                }
            }
            Ok(result)
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a PlaceListQuery,
        opened: &'a Prepared,
        fresh: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.grouping.preserves_sources(&fresh.grouping)
                && opened.template == fresh.template
                && opened.environments.preserves(
                    &fresh.environments,
                    &opened.bounds.hosts,
                    &opened.bounds.machines,
                )
                && opened
                    .environments
                    .disclosure
                    .names
                    .is_subset(&fresh.environments.disclosure.names)
                && opened
                    .environments
                    .disclosure
                    .displays
                    .is_subset(&fresh.environments.disclosure.displays)
                && opened.hosts.iter().all(|old| {
                    fresh
                        .hosts
                        .iter()
                        .find(|now| now.host.id == old.host.id)
                        .is_some_and(|now| {
                            old.host.machine_id == now.host.machine_id
                                && old
                                    .mount_sources
                                    .iter()
                                    .filter(|id| {
                                        !opened.bounds.limited || opened.bounds.mounts.contains(id)
                                    })
                                    .all(|id| now.mount_sources.contains(id))
                        })
                }))
        })
    }
}
