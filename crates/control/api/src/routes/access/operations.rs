//! Existing permission readers: current selector, actual source facts, and bounded ordinary projection.
use super::*;
use crate::key_operations::targets;
use crate::registry::{OperationFuture, OperationPlan};
use orch_access_contract::decisions::{ObjectRef, Through};
use orch_access_contract::key_scope::{
    KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::pages as stored;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operations::TargetResolver;
use orch_authz::Caller;
use orch_authz::engine::{Graph, Site};
use orch_authz::things::{self, Thing};
use page::{NameBounds, NameSources};
use std::collections::{BTreeSet, HashSet};

const RESOURCES: [(ObjectKind, &str); 5] = [
    (ObjectKind::Provider, "providers"),
    (ObjectKind::Agent, "agents"),
    (ObjectKind::Environment, "environments"),
    (ObjectKind::Host, "hosts"),
    (ObjectKind::Template, "templates"),
];

pub(super) struct PreparedPage {
    pub view: pages::PageView,
    pub root: Option<Id>,
    pub names: NameBounds,
    pub dependencies: dependencies::PreparedDependencies,
    pub environment: Option<thread_environment::PreparedThreadEnvironment>,
    pub covers: Option<api::PageCovers>,
}
pub(super) struct PreparedEffective {
    pub held: things::Effective,
    pub entries: Vec<stored::Entry>,
    pub paths: Vec<(stored::Holder, Vec<stored::Holder>)>,
    pub role_place: Option<Id>,
    pub names: NameBounds,
}
fn index(plan: &OperationPlan, role: &str) -> ApiResult<usize> {
    plan.variant().facets.iter().position(|facet|matches!(facet.target,TargetResolver::Resolved(name)|TargetResolver::ResolvedCollection(name) if name==role)).ok_or_else(||ApiError::internal("the inspection source is not declared"))
}
fn bind(
    plan: &mut OperationPlan,
    role: &str,
    kind: KeyTargetKind,
    targets: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let index = index(plan, role)?;
    if targets.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, kind, targets)
    }
}
fn base(ctx: &Ctx, selection: &str, target: &KeyResolvedTarget) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(ctx.route, selection)?;
    let index = index(&plan, "page")?;
    plan.bind(index, target.object.kind, std::slice::from_ref(target))?;
    Ok(plan)
}
async fn found(
    ctx: &Ctx,
    conn: &mut Conn,
    defaults: bool,
) -> ApiResult<(Thing, Graph, Site, Policy)> {
    let thing = if defaults {
        Thing::Node(ctx.id("id")?)
    } else {
        subject::named(ctx)?.thing
    };
    let (graph, site) = things::site(conn, thing).await.map_err(|e| match e {
        orch_authz::Error::NotFound(_) => ApiError::not_found(),
        other => ApiError::internal(other),
    })?;
    let policy = subject::policy_of(conn, thing, &site).await?;
    Ok((thing, graph, site, policy))
}
/// The original ordinary page access alternatives, not additional public operations.
#[derive(Clone, Copy)]
enum PagePath {
    View,
    Inspect,
    Defaults,
}
fn permission_refusal(error: &ApiError) -> bool {
    matches!(
        error.problem.code,
        ErrorCode::AuthzDenied | ErrorCode::NotFound
    )
}
fn scope_refusal() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the key does not allow this permission reader",
    )
}
/// Admit only a path that both this credential and the existing ordinary check permit.
/// Authentication, suspension and second-step failures are terminal, never fallback signals.
async fn page_caller(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    thing: Thing,
    target: &KeyResolvedTarget,
    policy: Policy,
    path: PagePath,
) -> ApiResult<Option<(OperationPlan, Principal)>> {
    let selection = match path {
        PagePath::Defaults => ctx.route.name.to_owned(),
        PagePath::View => format!("{}.view", ctx.route.name),
        PagePath::Inspect => format!("{}.inspect", ctx.route.name),
    };
    if !caller.permits_operation(&selection) {
        return Ok(None);
    }
    let plan = base(ctx, &selection, target)?;
    if !plan.permits_binding(
        caller,
        index(&plan, "page")?,
        target.object.kind,
        std::slice::from_ref(target),
    )? {
        return Ok(None);
    }
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    match path {
        PagePath::Inspect => subject::require_manage(conn, &principal, thing, policy).await?,
        _ => subject::require_view(conn, &principal, thing, policy).await?,
    }
    Ok(Some((plan, principal)))
}

pub(super) fn page<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare_page(ctx, conn, caller, false))
}
pub(super) fn defaults<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare_page(ctx, conn, caller, true))
}
async fn prepare_page(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    defaults: bool,
) -> ApiResult<OperationPlan> {
    let (thing, graph, site, policy) = found(ctx, conn, defaults).await?;
    let target = targets::from_site(&graph, &site)?;
    let paths: &[PagePath] = if defaults {
        &[PagePath::Defaults]
    } else {
        &[PagePath::View, PagePath::Inspect]
    };
    let mut ordinary_refusal = None;
    let mut incomplete_scope = false;
    for path in paths {
        let prepared = page_caller(ctx, conn, caller, thing, &target, policy, *path).await;
        let (plan, principal) = match prepared {
            Ok(Some(ready)) => ready,
            Ok(None) => continue,
            Err(error) if permission_refusal(&error) => {
                ordinary_refusal.get_or_insert(error);
                continue;
            }
            Err(error) => return Err(error),
        };
        // Keep the existing getters' kind validation after their ordinary read admission.
        if defaults {
            check_which(
                Which::Defaults,
                refusals::node_kind(site.kind.as_str()).ok_or_else(ApiError::not_found)?,
            )?;
        }
        if let Thing::Other(object) = thing {
            if object.kind != site.kind {
                return Err(ApiError::not_found());
            }
        }
        let plan =
            page_candidate(ctx, conn, &principal, thing, &site, policy, defaults, plan).await?;
        let facets = plan
            .shared()
            .required_facets()
            .map_err(ApiError::internal)?;
        if principal.caller().permits_facets(&facets) {
            return Ok(plan);
        }
        // A primary View grant is insufficient if its required external source is missing.
        // Try the existing Manage inspection alternative with its own exact complete scope.
        incomplete_scope = true;
    }
    if incomplete_scope {
        Err(scope_refusal())
    } else {
        Err(ordinary_refusal.unwrap_or_else(scope_refusal))
    }
}
#[allow(
    clippy::too_many_arguments,
    reason = "the same resolved page, ordinary caller and its current projection scope"
)]
async fn page_candidate(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    thing: Thing,
    site: &Site,
    policy: Policy,
    defaults: bool,
    mut plan: OperationPlan,
) -> ApiResult<OperationPlan> {
    let at = if defaults {
        Target::Defaults(PlaceRef::Node(site.id))
    } else {
        thing.target()
    };
    let view = pages::read(conn, principal, at, policy)
        .await
        .map_err(ApiError::internal)?;
    let mut names = page::page_name_sources(&view);
    let dependencies = dependencies::prepare_page(conn, principal, &view).await?;
    names.nodes.extend(dependencies.nodes());
    let environment = if !defaults && site.kind == ObjectKind::Thread {
        thread_environment::prepare_thread(conn, principal, site.id, policy).await?
    } else {
        None
    };
    let mut objects: Vec<_> = dependencies.objects().collect();
    if let Some(environment) = &environment {
        objects.extend(environment.objects());
        names.nodes.extend(environment.nodes());
    }
    let bounds =
        projection_sources(ctx, conn, principal.caller(), &mut plan, names, &objects).await?;
    let covers = if !defaults && matches!(thing, Thing::Node(_)) {
        covers::of_place(conn, principal, site.id, policy)
            .await
            .map_err(ApiError::internal)?
            .map(|counted| api::PageCovers {
                places: counted.follows.places,
                threads: counted.follows.threads,
                documents: counted.follows.documents,
                apart: api::PageCounts {
                    places: counted.apart.places,
                    threads: counted.apart.threads,
                    documents: counted.apart.documents,
                },
            })
    } else {
        None
    };
    Ok(plan.with_data(PreparedPage {
        view,
        root: site.root,
        names: bounds,
        dependencies,
        environment,
        covers,
    }))
}

pub(super) fn people<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: api::PermissionPeopleListQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (thing, graph, site, policy) = found(ctx, conn, false).await?;
        let target = targets::from_site(&graph, &site)?;
        let mut refusal = None;
        for path in [PagePath::View, PagePath::Inspect] {
            match page_caller(ctx, conn, caller, thing, &target, policy, path).await {
                Ok(Some((plan, _))) => return Ok(plan),
                Ok(None) => {}
                Err(error) if permission_refusal(&error) => {
                    refusal.get_or_insert(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(refusal.unwrap_or_else(scope_refusal))
    })
}

pub(super) fn effective<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: api::EffectiveAccessGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (thing, graph, site, policy) = found(ctx, conn, false).await?;
        let target = targets::from_site(&graph, &site)?;
        let own = query.user_id == caller.user();
        let selection = if own {
            "EffectiveAccessGet.self"
        } else {
            "EffectiveAccessGet.inspect"
        };
        let mut plan = base(ctx, selection, &target)?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        if own {
            subject::require_page(conn, &principal, thing, policy).await?;
        } else {
            subject::require_manage(conn, &principal, thing, policy).await?;
        }
        // This other principal is only the ordinary subject whose rights are inspected. It is
        // never substituted for the authenticated caller or used to admit/project the response.
        let person = if own {
            principal.clone()
        } else {
            Principal::load(conn, Caller::session(query.user_id), principal.now())
                .await
                .map_err(|_| ApiError::not_found())?
        };
        let held = things::effective_at(conn, &person, graph, &site, policy)
            .await
            .map_err(ApiError::internal)?;
        let ids: Vec<_> = held
            .held
            .iter()
            .flat_map(|p| &p.through)
            .filter_map(|t| match t {
                Through::Entry(via) => Some(via.entry),
                _ => None,
            })
            .collect();
        let rows = access::entries_by_id(conn, &ids).await?;
        let entries: Vec<_> = rows
            .iter()
            .filter_map(|row| orch_authz::engine::entry_of(row).ok())
            .collect();
        let mut paths = Vec::new();
        for entry in &entries {
            if paths.iter().any(|(holder, _)| *holder == entry.holder) {
                continue;
            }
            let chain = holders::chain(conn, &person, entry.holder, site.role_place)
                .await
                .map_err(ApiError::internal)?;
            paths.push((entry.holder, chain));
        }
        let mut names = page::entry_name_sources(&entries, None);
        let lists: Vec<_> = paths.iter().map(|(_, chain)| chain.clone()).collect();
        merge_names(
            &mut names,
            page::holder_list_name_sources(&lists, site.role_place),
        );
        if let Some(role) = held.role {
            names.nodes.extend(role.at.node());
        }
        for through in held.held.iter().flat_map(|p| &p.through) {
            if let Through::Entry(via) = through {
                match via.from {
                    stored::Origin::Node(id) | stored::Origin::Defaults(PlaceRef::Node(id)) => {
                        names.nodes.insert(id);
                    }
                    stored::Origin::DocumentContainer(id) => {
                        names.containers.insert(id);
                    }
                    _ => {}
                }
            }
        }
        let bounds =
            projection_sources(ctx, conn, principal.caller(), &mut plan, names, &[]).await?;
        Ok(plan.with_data(PreparedEffective {
            held,
            entries,
            paths,
            role_place: site.role_place,
            names: bounds,
        }))
    })
}
fn merge_names(into: &mut NameSources, other: NameSources) {
    into.users.extend(other.users);
    into.groups.extend(other.groups);
    into.nodes.extend(other.nodes);
    into.containers.extend(other.containers);
}

/// Existing protected projection sources, with role names supplied by the consuming operation.
/// This reuses the same factual Group/Node readers and scope binder; it grants no ordinary rights.
pub(crate) struct NameRoles {
    pub(crate) groups: Option<&'static str>,
    pub(crate) nodes: [(ObjectKind, &'static str); 4],
    pub(crate) containers: Option<&'static str>,
}

const INSPECTION_NAMES: NameRoles = NameRoles {
    groups: Some("inspection_groups"),
    nodes: [
        (ObjectKind::Org, "inspection_names_orgs"),
        (ObjectKind::Project, "inspection_names_projects"),
        (ObjectKind::Topic, "inspection_names_topics"),
        (ObjectKind::Thread, "inspection_names_threads"),
    ],
    containers: Some("inspection_names_containers"),
};

async fn projection_sources(
    _ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut OperationPlan,
    names: NameSources,
    objects: &[ObjectRef],
) -> ApiResult<NameBounds> {
    let mut resource_graph = Graph::default();
    let ids: Vec<_> = objects
        .iter()
        .map(|o| o.id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let resource_rows = orch_authz::resources::read_into(conn, &mut resource_graph, &ids)
        .await
        .map_err(ApiError::internal)?;
    for (kind, suffix) in RESOURCES {
        let mut targets = Vec::new();
        for object in objects.iter().filter(|o| o.kind == kind) {
            if targets
                .iter()
                .any(|t: &KeyResolvedTarget| t.object.id == Some(object.id))
            {
                continue;
            }
            let row = resource_rows
                .iter()
                .find(|row| {
                    row.id == object.id && ObjectKind::from(row.kind) == kind && !row.deleted
                })
                .ok_or_else(ApiError::not_found)?;
            let site = orch_authz::resources::site_of(&resource_graph, row)
                .ok_or_else(ApiError::not_found)?;
            targets.push(crate::key_operations::targets::from_site(
                &resource_graph,
                &site,
            )?);
        }
        bind(
            plan,
            &format!("inspection_{suffix}"),
            KeyTargetKind::Object(kind),
            &targets,
        )?;
    }
    // Reject an undeclared actual dependency kind rather than silently dropping its source.
    if objects
        .iter()
        .any(|object| !RESOURCES.iter().any(|(kind, _)| *kind == object.kind))
    {
        return Err(ApiError::internal(
            "an inspection dependency kind is undeclared",
        ));
    }
    projection_names(conn, caller, plan, names, &INSPECTION_NAMES).await
}

/// Existing Group identities and retained owning-org paths, loaded in one batch.
/// This supplies facts only; the operation binder and ordinary projection still decide authority.
pub(crate) async fn group_targets(
    conn: &mut Conn,
    ids: &[Id],
) -> ApiResult<Vec<KeyResolvedTarget>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let groups: Vec<(Id, Id)> = sqlx::query_as(
        "SELECT g.id,g.org_id FROM groups g JOIN nodes n ON n.id=g.org_id WHERE g.id=ANY($1)",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let group_homes: Vec<_> = groups.iter().map(|(_, org)| *org).collect();
    let group_graph = Graph::load(conn, &group_homes)
        .await
        .map_err(ApiError::internal)?;
    let mut group_targets = Vec::new();
    for (group, org) in groups {
        // This is only the retained identity/placement of the group that the ordinary page
        // still names; deleting an org does not turn page metadata into a different grant.
        if !group_graph
            .get(org)
            .is_some_and(|node| ObjectKind::from(node.kind) == ObjectKind::Org)
        {
            return Err(ApiError::not_found());
        }
        let mut target =
            orch_engine::operation_scope::targets::from_node_identity(&group_graph, org)
                .ok_or_else(ApiError::not_found)?;
        target.object = KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::Group),
            id: Some(group),
        };
        group_targets.push(target);
    }
    Ok(group_targets)
}

/// The existing optional protected-node name binder for the consuming operation's declared roles.
/// The ordinary reference projector still decides visibility and redaction.
pub(crate) fn projection_node_names(
    caller: &Caller,
    plan: &mut OperationPlan,
    graph: &Graph,
    roles: &[(ObjectKind, &str)],
) -> ApiResult<BTreeSet<Id>> {
    let mut allowed = BTreeSet::new();
    for &(kind, role) in roles {
        let mut targets = Vec::new();
        let slot = index(plan, role)?;
        for node in graph.nodes().filter(|n| ObjectKind::from(n.kind) == kind) {
            let Some(target) =
                orch_engine::operation_scope::targets::from_node_identity(graph, node.id)
            else {
                continue;
            };
            if plan.permits_binding(
                caller,
                slot,
                KeyTargetKind::Object(kind),
                std::slice::from_ref(&target),
            )? {
                allowed.insert(node.id);
                targets.push(target);
            }
        }
        bind(plan, role, KeyTargetKind::Object(kind), &targets)?;
    }
    Ok(allowed)
}

/// Binds actual full Group payloads and optionally admissible protected names in batches.
/// Mandatory groups are never dropped; ordinary Node/Container projection retains liveness and redaction.
pub(crate) async fn projection_names(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut OperationPlan,
    mut names: NameSources,
    roles: &NameRoles,
) -> ApiResult<NameBounds> {
    let mut bounds = NameBounds {
        limited: caller
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some()),
        ..NameBounds::default()
    };
    let group_targets =
        group_targets(conn, &names.groups.iter().copied().collect::<Vec<_>>()).await?;
    names.nodes.extend(
        group_targets
            .iter()
            .flat_map(|target| target.node_ancestors.iter().filter_map(|node| node.id)),
    );
    if let Some(role) = roles.groups {
        bind(
            plan,
            role,
            KeyTargetKind::Identity(KeyIdentityKind::Group),
            &group_targets,
        )?;
    } else if !group_targets.is_empty() {
        return Err(ApiError::internal(
            "the projection has an undeclared Group source",
        ));
    }
    let users: Vec<_> = names.users.into_iter().collect();
    names.nodes.extend(
        access::holder_user_addresses(conn, &users)
            .await?
            .into_iter()
            .map(|(_, _, domain, _)| domain),
    );
    let graph = Graph::load(conn, &names.nodes.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    bounds
        .nodes
        .extend(projection_node_names(caller, plan, &graph, &roles.nodes)?);
    if let Some(role) = roles.containers {
        let mut graph = Graph::default();
        let containers = orch_authz::documents::read_into(
            conn,
            &mut graph,
            &names.containers.into_iter().collect::<Vec<_>>(),
        )
        .await
        .map_err(ApiError::internal)?;
        let mut targets = Vec::new();
        let slot = index(plan, role)?;
        for row in containers
            .into_iter()
            .filter(|row| row.kind == ObjectKind::DocumentContainer)
        {
            let Some(site) = orch_authz::documents::site_of(&graph, &row) else {
                continue;
            };
            let target = crate::key_operations::targets::from_site(&graph, &site)?;
            if plan.permits_binding(
                caller,
                slot,
                KeyTargetKind::Object(ObjectKind::DocumentContainer),
                std::slice::from_ref(&target),
            )? {
                bounds.containers.insert(row.id);
                targets.push(target);
            }
        }
        bind(
            plan,
            role,
            KeyTargetKind::Object(ObjectKind::DocumentContainer),
            &targets,
        )?;
    } else if !names.containers.is_empty() {
        return Err(ApiError::internal(
            "the projection has an undeclared container-name source",
        ));
    }
    Ok(bounds)
}

/// One actual read snapshot carries the freshly admitted operands through the existing projection.
pub(crate) async fn read_page(ctx: &Ctx) -> ApiResult<api::PermissionPage> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<PreparedPage>()?;
    let mut shown = page::permission_page_bounded(
        &mut tx,
        &principal,
        &prepared.view,
        prepared.root,
        principal.now(),
        &prepared.names,
        &prepared.dependencies,
    )
    .await?;
    shown.covers = prepared.covers.clone();
    if let Some(environment) = &prepared.environment {
        thread_environment::project_thread(&mut tx, &principal, environment, &mut shown).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}
