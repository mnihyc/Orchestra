//! Native environment and heterogeneous placement preparation under their consuming topic operation.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use crate::routes::environments;

use crate::routes::environments::places as placement;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::topic_sources as sources;

use orch_api_contract::topic_sources::Source;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use std::collections::BTreeMap;

use std::collections::HashMap;
fn collection(
    source: Source,
    values: Vec<(KeyTargetKind, KeyResolvedTarget)>,
) -> native::Facts<Source> {
    let mut groups = BTreeMap::new();
    for (kind, target) in values {
        groups.entry(kind).or_insert_with(Vec::new).push(target);
    }
    native::Facts::collection(source, groups.into_iter().collect())
}

fn permits(
    ctx: &Ctx,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    source: Source,
    kind: KeyTargetKind,
    target: &KeyResolvedTarget,
) -> ApiResult<bool> {
    if !native::modern(caller) {
        return Ok(true);
    }
    let known = native::bind_known(ctx, declaration, &[])?;
    let usage = declaration
        .bindings
        .iter()
        .find(|b| b.source == source)
        .ok_or_else(|| ApiError::internal("the native topic source is undeclared"))?
        .usage;
    native::permits_usage(&known, declaration, caller, &source, usage, kind, target)
}

async fn resource_targets(
    conn: &mut Conn,
    kind: ResourceKind,
    ids: &[Id],
) -> ApiResult<HashMap<Id, KeyResolvedTarget>> {
    let mut graph = orch_authz::engine::Graph::default();
    let rows = orch_authz::resources::read_into(conn, &mut graph, ids)
        .await
        .map_err(ApiError::internal)?;
    Ok(rows
        .iter()
        .filter(|row| row.kind == kind)
        .filter_map(|row| {
            orch_engine::operation_scope::targets::from_resource_identity(&graph, row)
                .map(|target| (row.id, target))
        })
        .collect())
}

async fn names(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    mut wanted: NameSources,
) -> ApiResult<(NameBounds, native::Facts<Source>)> {
    let groups = crate::routes::access::operations::group_targets(
        conn,
        &wanted.groups.iter().copied().collect::<Vec<_>>(),
    )
    .await?;
    wanted.nodes.extend(
        groups
            .iter()
            .flat_map(|g| g.node_ancestors.iter().filter_map(|n| n.id)),
    );
    wanted.nodes.extend(
        orch_db::queries::access::holder_user_addresses(
            conn,
            &wanted.users.iter().copied().collect::<Vec<_>>(),
        )
        .await?
        .into_iter()
        .map(|(_, _, domain, _)| domain),
    );
    let mut facts = Vec::new();
    for target in groups {
        let family = KeyTargetKind::Identity(KeyIdentityKind::Group);
        if !permits(ctx, caller, declaration, Source::Names, family, &target)? {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "a native group reference is outside this selection",
            ));
        }
        facts.push((family, target));
    }
    let graph =
        orch_authz::engine::Graph::load(conn, &wanted.nodes.iter().copied().collect::<Vec<_>>())
            .await
            .map_err(ApiError::internal)?;
    let refs = node_view::refs(
        conn,
        principal,
        &graph.nodes().map(|n| n.id).collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let mut bounds = NameBounds {
        limited: native::modern(caller),
        ..NameBounds::default()
    };
    for node in graph.nodes() {
        if refs
            .get(&node.id)
            .is_some_and(|r| r.name.is_some() || r.thread_display.is_some())
            && let Some(target) =
                orch_engine::operation_scope::targets::from_node_identity(&graph, node.id)
            && permits(
                ctx,
                caller,
                declaration,
                Source::Names,
                target.object.kind,
                &target,
            )?
        {
            bounds.nodes.insert(node.id);
            facts.push((target.object.kind, target));
        }
    }
    if !wanted.containers.is_empty() {
        let objects = wanted
            .containers
            .iter()
            .map(|id| ObjectRef {
                kind: ObjectKind::DocumentContainer,
                id: *id,
            })
            .collect::<Vec<_>>();
        for (object, target) in crate::routes::nodes::read_sources::Sources::from_objects(objects)
            .identity_targets(conn)
            .await?
        {
            if permits(
                ctx,
                caller,
                declaration,
                Source::Names,
                KeyTargetKind::Object(object.kind),
                &target,
            )? {
                bounds.containers.insert(object.id);
                facts.push((KeyTargetKind::Object(object.kind), target));
            }
        }
    }
    Ok((bounds, collection(Source::Names, facts)))
}

async fn require(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    permission: Permission,
) -> ApiResult<()> {
    authorization::require_projected(
        conn,
        principal,
        ctx.id("id")?,
        authorization::exact(permission),
        orch_authz::Act::Use,
        Policy::default(),
        native::modern(principal.caller()),
    )
    .await
}

struct EnvironmentParts {
    prepared: environments::PreparedInventory,
    ids: Vec<Id>,
    targets: HashMap<Id, KeyResolvedTarget>,
    foreign: Vec<(KeyTargetKind, KeyResolvedTarget)>,
}

#[derive(Clone, Copy)]
enum Style {
    Environment,
    Place,
}

async fn prepare_environments(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    caller: &Caller,
    declaration: &'static Declaration<Source>,
    ids: &[Id],
    style: Style,
) -> ApiResult<EnvironmentParts> {
    let mut targets = resource_targets(conn, ResourceKind::Environment, ids).await?;
    let mut scoped = Vec::new();
    for id in ids {
        if let Some(target) = targets.get(id)
            && permits(
                ctx,
                caller,
                declaration,
                Source::Results,
                KeyTargetKind::Object(ObjectKind::Environment),
                target,
            )?
        {
            scoped.push(*id);
        }
    }
    let mut prepared = environments::prepare_viewed(ctx, conn, principal, &scoped).await?;
    let parts = prepared.content_sources(conn).await?;
    let mut selected = Vec::new();
    let mut used = Vec::new();
    for id in prepared.selected_ids() {
        let objects = parts
            .get(&id)
            .into_iter()
            .flatten()
            .filter(|object| match style {
                Style::Environment => true,
                Style::Place => matches!(object.kind, ObjectKind::Host | ObjectKind::Machine),
            })
            .copied()
            .collect::<Vec<_>>();
        let resolved = crate::routes::nodes::read_sources::Sources::from_objects(objects)
            .identity_targets(conn)
            .await?;
        let mut allowed = true;
        let mut sources = Vec::new();
        for (object, target) in resolved {
            let family = KeyTargetKind::Object(object.kind);
            allowed &= permits(ctx, caller, declaration, Source::Resources, family, &target)?;
            sources.push((family, target));
        }
        if allowed {
            selected.push(id);
            used.extend(sources);
        }
    }
    if selected.len() != prepared.selected_ids().len() {
        prepared = environments::prepare_viewed(ctx, conn, principal, &selected).await?;
    }
    targets.retain(|id, _| selected.contains(id));
    Ok(EnvironmentParts {
        prepared,
        ids: selected,
        targets,
        foreign: used,
    })
}

impl EnvironmentParts {
    fn results(&self) -> Vec<(KeyTargetKind, KeyResolvedTarget)> {
        self.ids
            .iter()
            .filter_map(|id| self.targets.get(id).cloned())
            .map(|t| (KeyTargetKind::Object(ObjectKind::Environment), t))
            .collect()
    }
}

pub(super) struct Environments;

pub(super) struct PreparedEnvironments {
    parts: EnvironmentParts,
    default: Option<Id>,
}

impl native::Selected for Environments {
    type Source = Source;
    type Query = ();
    type Value = PreparedEnvironments;
    type Reply = Vec<Environment>;
    const DECLARATION: &'static Declaration<Source> = &sources::ENVIRONMENTS;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(select_environments(ctx, conn, caller))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, Permission::TopicView))
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            let mut listed =
                environments::show_inventory_prepared(ctx, conn, principal, &value.parts.prepared)
                    .await?;
            listed.sort_by_key(|env| Some(env.base.id) != value.default);
            Ok(listed)
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
        Box::pin(async move {
            Ok(old.parts.ids.iter().all(|id| fresh.parts.ids.contains(id))
                && old.parts.prepared.preserves_sources(&fresh.parts.prepared))
        })
    }
}

async fn select_environments(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<(PreparedEnvironments, Vec<native::Facts<Source>>)> {
    let topic = ctx.id("id")?;
    let default = super::default_of(conn, topic).await?;
    let mut ids = rows::environments_under(conn, topic).await?;
    if let Some(id) = default
        && !ids.contains(&id)
    {
        ids.push(id);
    }
    let targets = resource_targets(conn, ResourceKind::Environment, &ids).await?;
    let mut selected = Vec::new();
    for target in targets.into_values() {
        if permits(
            ctx,
            caller,
            &sources::ENVIRONMENTS,
            Source::Results,
            KeyTargetKind::Object(ObjectKind::Environment),
            &target,
        )? {
            selected.push((KeyTargetKind::Object(ObjectKind::Environment), target));
        }
    }
    let known = native::bind_known(
        ctx,
        &sources::ENVIRONMENTS,
        &[collection(Source::Results, selected)],
    )?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    require(ctx, conn, &principal, Permission::TopicView).await?;
    let mut parts = prepare_environments(
        ctx,
        conn,
        &principal,
        caller,
        &sources::ENVIRONMENTS,
        &ids,
        Style::Environment,
    )
    .await?;
    let (bounds, names) = names(
        ctx,
        conn,
        &principal,
        caller,
        &sources::ENVIRONMENTS,
        parts.prepared.name_sources().clone(),
    )
    .await?;
    parts.prepared.bound_names(bounds);
    let facts = vec![
        collection(Source::Results, parts.results()),
        collection(Source::Resources, parts.foreign.clone()),
        names,
    ];
    Ok((PreparedEnvironments { parts, default }, facts))
}

pub(super) struct Places;

pub(super) struct PreparedPlaces {
    input: placement::HostChoiceInput,
    hosts: Vec<orch_engine::places::HostPlace>,
    environments: EnvironmentParts,
    names: NameBounds,
    grouping: crate::routes::resources::grouping::Prepared,
    origin_machines: std::collections::BTreeSet<Id>,
}

impl native::Selected for Places {
    type Source = Source;
    type Query = PlaceListQuery;
    type Value = PreparedPlaces;
    type Reply = Vec<Place>;
    const DECLARATION: &'static Declaration<Source> = &sources::PLACES;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(select_places(ctx, conn, caller, query))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, Permission::TopicInteract))
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            // Name the actual native host rows without repeating host planning or scheduler selection.
            let mut hosts = value.hosts.clone();
            placement::group_host_choices(
                &ctx.app,
                conn,
                principal,
                &mut hosts,
                Some(&value.names),
                native::modern(principal.caller()).then_some(&value.origin_machines),
            )
            .await?;
            let mut places = hosts
                .into_iter()
                .map(|host| Place::Host(host.place))
                .collect::<Vec<_>>();
            let environments = environments::show_inventory_prepared(
                ctx,
                conn,
                principal,
                &value.environments.prepared,
            )
            .await?;
            places.extend(
                environments
                    .into_iter()
                    .map(placement::place_of)
                    .map(Place::Environment),
            );
            let grouped = value
                .grouping
                .render(
                    &ctx.app,
                    conn,
                    principal,
                    Some(&value.names),
                    native::modern(principal.caller()).then_some(&value.origin_machines),
                )
                .await?;
            for place in &mut places {
                if let Place::Environment(environment) = place
                    && let Some(metadata) =
                        grouped.get(&("environment".to_owned(), environment.environment.id))
                {
                    environment.grouping.home = metadata.home.clone();
                    environment.grouping.home_path = metadata.home_path.clone();
                }
            }
            Ok(places)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.grouping.preserves_sources(&fresh.grouping)
                && old.origin_machines.is_subset(&fresh.origin_machines)
                && old.input.wanted.template.id == fresh.input.wanted.template.id
                && old.hosts.iter().all(|host| {
                    fresh.hosts.iter().any(|new| {
                        host.host.id == new.host.id && host.host.machine_id == new.host.machine_id
                    })
                })
                && old
                    .environments
                    .ids
                    .iter()
                    .all(|id| fresh.environments.ids.contains(id))
                && old
                    .environments
                    .prepared
                    .preserves_sources(&fresh.environments.prepared)
                && (!old.names.limited || old.names.nodes.is_subset(&fresh.names.nodes)))
        })
    }
}

async fn select_places(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &PlaceListQuery,
) -> ApiResult<(PreparedPlaces, Vec<native::Facts<Source>>)> {
    let topic = ctx.id("id")?;
    let input = placement::prepare_host_choices(conn, topic, query).await?;
    let template = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Template,
            id: input.wanted.template.id,
        },
    )
    .await?;
    let template_fact = native::Facts::one(
        Source::Template,
        KeyTargetKind::Object(ObjectKind::Template),
        template,
    );
    let known = native::bind_known(ctx, &sources::PLACES, std::slice::from_ref(&template_fact))?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    require(ctx, conn, &principal, Permission::TopicInteract).await?;
    let raw_hosts = orch_db::queries::resources::management::host_ids(conn).await?;
    let raw_environments: Vec<Id> =
        sqlx::query_scalar("SELECT id FROM environments WHERE deleted_at IS NULL")
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let mut host_ids = Vec::new();
    for (id, target) in resource_targets(conn, ResourceKind::Host, &raw_hosts).await? {
        if permits(
            ctx,
            caller,
            &sources::PLACES,
            Source::Results,
            KeyTargetKind::Object(ObjectKind::Host),
            &target,
        )? {
            host_ids.push(id);
        }
    }
    let mounts = crate::routes::nodes::read_sources::Sources::from_objects(
        input.wanted.mounts.iter().map(|id| ObjectRef {
            kind: ObjectKind::Mount,
            id: *id,
        }),
    )
    .identity_targets(conn)
    .await?;
    for (object, target) in &mounts {
        if !permits(
            ctx,
            caller,
            &sources::PLACES,
            Source::Resources,
            KeyTargetKind::Object(object.kind),
            target,
        )? {
            host_ids.clear();
            break;
        }
    }
    let mut hosts = placement::select_host_choices(
        conn,
        &principal,
        topic,
        &input,
        ctx.now.into(),
        native::modern(caller).then_some(host_ids.as_slice()),
    )
    .await?;
    let mut environment_ids = Vec::new();
    for (id, target) in resource_targets(conn, ResourceKind::Environment, &raw_environments).await?
    {
        if permits(
            ctx,
            caller,
            &sources::PLACES,
            Source::Results,
            KeyTargetKind::Object(ObjectKind::Environment),
            &target,
        )? {
            environment_ids.push(id);
        }
    }
    let manageable = placement::manageable_environment_ids(
        conn,
        &principal,
        native::modern(caller).then_some(environment_ids.as_slice()),
    )
    .await?;
    let mut environments = prepare_environments(
        ctx,
        conn,
        &principal,
        caller,
        &sources::PLACES,
        &manageable,
        Style::Place,
    )
    .await?;
    let grouped_subjects = hosts
        .iter()
        .map(|host| ("host".to_owned(), host.host.id))
        .chain(manageable.iter().map(|&id| ("environment".to_owned(), id)))
        .collect::<Vec<_>>();
    let grouping =
        crate::routes::resources::grouping::Prepared::select(conn, &grouped_subjects).await?;
    let origin_ids = grouping.origins().into_iter().collect::<Vec<_>>();
    let physical_origins =
        orch_db::queries::resources::routes::physical_machines_among(conn, &origin_ids).await?;
    let mut origin_machines = std::collections::BTreeSet::new();
    let mut origin_facts = Vec::new();
    for machine in physical_origins {
        let target = targets::machine_identities(conn, &[machine.id])
            .await?
            .remove(&machine.id)
            .ok_or_else(ApiError::not_found)?;
        if permits(
            ctx,
            caller,
            &sources::PLACES,
            Source::Resources,
            KeyTargetKind::Object(ObjectKind::Machine),
            &target,
        )? {
            origin_machines.insert(machine.id);
            origin_facts.push((KeyTargetKind::Object(ObjectKind::Machine), target));
        }
    }
    let mut wanted = environments.prepared.place_names(conn).await?;
    wanted.nodes.extend(grouping.homes());
    wanted.nodes.extend(
        orch_authz::resources::address_name_sources(
            conn,
            &hosts.iter().map(|host| host.host.id).collect::<Vec<_>>(),
        )
        .await
        .map_err(ApiError::internal)?,
    );
    let (names, name_facts) =
        names(ctx, conn, &principal, caller, &sources::PLACES, wanted).await?;
    environments.prepared.bound_names(names.clone());
    placement::name_host_choices(conn, &mut hosts, Some(&names)).await?;
    let mut results = environments.results();
    for host in &hosts {
        results.push((
            KeyTargetKind::Object(ObjectKind::Host),
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: host.host.id,
                },
            )
            .await?,
        ));
    }
    let mut foreign = environments.foreign.clone();
    foreign.extend(origin_facts);
    if !hosts.is_empty() {
        foreign.extend(
            mounts
                .into_iter()
                .map(|(object, target)| (KeyTargetKind::Object(object.kind), target)),
        );
    }
    let facts = vec![
        collection(Source::Results, results),
        collection(Source::Resources, foreign),
        name_facts,
        template_fact,
    ];
    Ok((
        PreparedPlaces {
            input,
            grouping,
            origin_machines,
            hosts,
            environments,
            names,
        },
        facts,
    ))
}
