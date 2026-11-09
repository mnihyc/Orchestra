//! Scope facts for the existing archive cascade, review and native response producers.

use super::{archive_plan, read_sources, view};
use crate::key_operations::{admission, resolver::Disclosure, targets};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, Guard};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use crate::routes::access::operations::{NameRoles, projection_names};
use crate::routes::access::page::{NameBounds, NameSources};
use crate::routes::documents::build::{ProjectedDocumentRef, WrittenLabelSource};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{Node, NodeArchiveBody};
use orch_api_contract::operations::TargetResolver;
use orch_authz::{Caller, Policy, Principal};
use orch_db::queries::{archive_plans, documents, hierarchy};
use orch_db::{Conn, Tx};
use orch_types::Id;
use std::collections::{BTreeSet, HashMap};

const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "archive_names_orgs"),
        (ObjectKind::Project, "archive_names_projects"),
        (ObjectKind::Topic, "archive_names_topics"),
        (ObjectKind::Thread, "archive_names_threads"),
    ],
    containers: None,
};
const FULL: &[(ObjectKind, &str)] = &[
    (ObjectKind::Org, "archive_full_orgs"),
    (ObjectKind::Project, "archive_full_projects"),
    (ObjectKind::Topic, "archive_full_topics"),
    (ObjectKind::Thread, "archive_full_threads"),
    (ObjectKind::Document, "archive_full_documents"),
    (ObjectKind::Provider, "archive_full_providers"),
    (ObjectKind::Agent, "archive_full_agents"),
    (ObjectKind::Environment, "archive_full_environments"),
    (ObjectKind::Machine, "archive_full_machines"),
];

#[derive(Clone)]
pub(super) struct Bounds {
    pub(super) names: NameBounds,
    pub(super) environments: BTreeSet<Id>,
    pub(super) environment_candidates: BTreeSet<Id>,
    readmes: BTreeSet<Id>,
    response: Option<OperationPlan>,
    pub(super) native: Option<NativePrefix>,
}

#[derive(Clone)]
pub(super) enum NativePrefix {
    Preview(Box<super::archive_plan::PreparedPlan>),
    Change {
        place: super::Placed,
        before: Box<Node>,
        review: Box<super::archive_plan::PreparedReview>,
    },
}

fn slot(plan: &OperationPlan, role: &str) -> ApiResult<usize> {
    plan.variant().facets.iter().position(|facet| matches!(facet.target,TargetResolver::Resolved(found)|TargetResolver::ResolvedCollection(found) if found==role))
        .ok_or_else(||ApiError::internal("an archive source role is undeclared"))
}
fn bind(
    plan: &mut OperationPlan,
    role: &str,
    kind: KeyTargetKind,
    rows: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let slot = slot(plan, role)?;
    if rows.is_empty() {
        plan.bind_empty(slot)
    } else {
        plan.bind(slot, kind, rows)
    }
}
fn act(ctx: &Ctx) -> bool {
    matches!(ctx.route.name, "NodeArchive" | "NodeUnarchive")
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<(OperationPlan, Bounds)> {
    let id = ctx.id("id")?;
    let root = targets::node(conn, id).await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    bind(
        &mut plan,
        "archive_root",
        root.object.kind,
        std::slice::from_ref(&root),
    )?;
    if act(ctx) {
        bind(
            &mut plan,
            "archive_result",
            root.object.kind,
            std::slice::from_ref(&root),
        )?;
    }
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    let set = hierarchy::deletion_set(conn, id, false).await?;
    let mut names = NameSources::default();
    names.nodes.extend(
        set.iter()
            .filter(|(kind, _, _)| kind != "document")
            .map(|(_, id, _)| *id),
    );
    names
        .nodes
        .extend(root.node_ancestors.iter().filter_map(|node| node.id));
    names.nodes.insert(id);
    // The existing README producer already decides ordinary document View. Its optional
    // metadata is only selected when this same operation may read the actual document.
    let mut readmes = BTreeSet::new();
    if act(ctx) {
        let rows = documents::readmes_of(conn, &[id]).await?;
        let ids: Vec<_> = rows.iter().map(|row| row.id).collect();
        let readable: HashMap<_, _> =
            orch_authz::documents::listed(conn, &principal, &ids, Policy::default())
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .filter(|row| row.holds(Permission::DocView))
                .map(|row| (row.document.id, row))
                .collect();
        let read_slot = slot(&plan, "archive_full_documents")?;
        for row in rows {
            let Some(listed) = readable.get(&row.id) else {
                continue;
            };
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Document,
                    id: row.id,
                },
            )
            .await?;
            if !plan.permits_binding(
                caller,
                read_slot,
                target.object.kind,
                std::slice::from_ref(&target),
            )? {
                continue;
            }
            readmes.insert(row.id);
            for source in ProjectedDocumentRef::candidates(conn, &row, listed).await? {
                if let WrittenLabelSource::Node(node) = source {
                    names.nodes.insert(node);
                } else {
                    return Err(ApiError::internal(
                        "an archive README has an undeclared named source",
                    ));
                }
            }
        }
    }
    let names = projection_names(conn, principal.caller(), &mut plan, names, &NAMES).await?;
    let env_slot = slot(&plan, "archive_environments")?;
    let mut environments = BTreeSet::new();
    let mut env_targets = Vec::new();
    let environment_rows = archive_plans::environments(conn, id).await?;
    let environment_candidates = environment_rows.iter().map(|(id, _)| *id).collect();
    let source_targets =
        read_sources::Sources::from_objects(environment_rows.iter().map(|(id, _)| ObjectRef {
            kind: ObjectKind::Environment,
            id: *id,
        }))
        .targets(conn)
        .await?;
    for (environment, _) in environment_rows {
        let object = ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        };
        let target = source_targets
            .get(&object)
            .ok_or_else(ApiError::not_found)?;
        if plan.permits_binding(
            caller,
            env_slot,
            target.object.kind,
            std::slice::from_ref(target),
        )? && archive_plan::environment_view(conn, &principal, environment).await?
        {
            environments.insert(environment);
            env_targets.push(target.clone());
        }
    }
    bind(
        &mut plan,
        "archive_environments",
        KeyTargetKind::Object(ObjectKind::Environment),
        &env_targets,
    )?;
    Ok((
        plan,
        Bounds {
            names,
            environments,
            environment_candidates,
            readmes,
            response: None,
            native: None,
        },
    ))
}

/// Run the unchanged ordinary root rule and native read-only operands in the final
/// guarded producer; no pre-wait/admin-context DTO is borrowed by the handler.
async fn native_prefix(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: Option<&NodeArchiveBody>,
) -> ApiResult<OperationPlan> {
    let (plan, mut bounds) = prepare(ctx, conn, caller).await?;
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    let id = ctx.id("id")?;
    let archiving = matches!(ctx.route.name, "NodeArchive" | "NodeArchivePreview");
    let (place, kind) = super::placed(conn, id).await?;
    let own = hierarchy::with_ancestors(conn, &[id])
        .await?
        .iter()
        .any(|row| row.id == id && row.archived_at.is_some());
    super::may_archive(
        conn,
        &principal,
        id,
        if archiving && !own {
            orch_authz::Act::Use
        } else {
            orch_authz::Act::Unarchive
        },
    )
    .await?;
    bounds.native = Some(match body {
        Some(body) => {
            // An action on a side thread has always failed here; a preview keeps
            // its existing blocker instead of acquiring a new action requirement.
            if kind == orch_api_contract::nodes::NodeKind::Thread
                && orch_db::queries::threads::forking::side_parent(conn, id)
                    .await?
                    .is_some()
            {
                return Err(ApiError::new(
                    ErrorCode::ThreadSide,
                    "a side thread is closed and not archived",
                ));
            }
            let before = super::shown(ctx, conn, &principal, id).await?;
            let review = super::archive_plan::prepare_verify(
                ctx,
                conn,
                &principal,
                id,
                archiving,
                body,
                Some(&bounds),
            )
            .await?;
            NativePrefix::Change {
                place,
                before: Box::new(before),
                review: Box::new(review),
            }
        }
        None => NativePrefix::Preview(Box::new(
            super::archive_plan::prepare_make(ctx, conn, &principal, id, archiving, Some(&bounds))
                .await?,
        )),
    });
    if body.is_some() {
        bounds.response = Some(plan.clone());
    }
    Ok(plan.with_data(bounds))
}

pub(super) fn review<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move { native_prefix(ctx, conn, caller, None).await })
}
pub(super) fn change<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: NodeArchiveBody,
) -> OperationFuture<'a> {
    Box::pin(async move { native_prefix(ctx, conn, caller, Some(&body)).await })
}

pub(super) async fn action(ctx: &Ctx) -> ApiResult<(Ctx, Tx, Guard)> {
    let (tx, guard) = transaction_guard::begin(ctx).await?;
    let current = ctx
        .for_operation_checkpoint(guard.user().clone(), guard.now().get())
        .with_admitted_caller(guard.principal().caller().clone())?;
    Ok((current, tx, guard))
}

/// One native pre-effect archive/preview serialization prefix. The original
/// ordinary root authority is repeated at the late current checkpoint; descendants
/// still follow the native cascade without another per-child Manage requirement.
pub(super) async fn effect(ctx: &Ctx, conn: &mut Conn, id: Id) -> ApiResult<(Ctx, Guard, Bounds)> {
    ctx.app
        .engine
        .lock_archive_in(conn, id)
        .await
        .map_err(ApiError::engine)?;
    hierarchy::deletion_set(conn, id, true).await?;
    let (current, guard) = transaction_guard::after_wait(ctx, conn).await?;
    let bounds = guard.operation_data::<Bounds>()?.as_ref().clone();
    Ok((current, guard, bounds))
}

/// Native locks can change the current review set. Freshly bind this operation's own
/// sources on that connection; no earlier preview or another endpoint is authority.
pub(super) async fn refresh(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<(Ctx, Principal, Bounds)> {
    let (plan, mut bounds) = prepare(ctx, conn, principal.caller()).await?;
    if act(ctx) {
        bounds.response = Some(plan.clone());
    }
    let admitted = plan.with_data(bounds.clone()).admit(principal.caller())?;
    let current = admission::install(ctx.clone(), admitted)?;
    let reader = authorization::load_principal(
        conn,
        current
            .admitted_caller()
            .ok_or_else(|| ApiError::internal("an archive lost its current admission"))?
            .clone(),
        principal.now(),
    )
    .await?;
    Ok((current, reader, bounds))
}

/// An effect already admitted and applied by the ordinary writer does not become
/// unauthorized merely because its optional later description can no longer be read.
/// The retained inverse is only caller-bound review data; every later act admits anew.
pub(super) async fn response_bounds(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<(Ctx, Principal, Bounds)> {
    match refresh(ctx, conn, principal).await {
        Err(error) if scoped(ctx) && matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok((
                ctx.clone(),
                principal.clone(),
                Bounds {
                    names: NameBounds {
                        limited: true,
                        ..NameBounds::default()
                    },
                    environments: BTreeSet::new(),
                    environment_candidates: BTreeSet::new(),
                    readmes: BTreeSet::new(),
                    response: None,
                    native: None,
                },
            ))
        }
        other => other,
    }
}

pub(super) async fn read_node(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    bounds: &Bounds,
) -> ApiResult<Option<Node>> {
    if principal
        .caller()
        .grant()
        .is_none_or(|grant| grant.operation_scope().is_none())
    {
        return super::shown(ctx, conn, principal, id).await.map(Some);
    }
    if ctx.disclosure() == Disclosure::Denied {
        return Ok(None);
    }
    let result = async {
        // Keep the existing rich native node/Thread producer and every ordinary decision.
        // Its actual typed references supply facts; missing mandatory source Read selects
        // a receipt rather than fabricating or clearing the binding/environment DTOs.
        let node = view::one_with_read_sources(
            conn,
            principal,
            id,
            principal.now().get(),
            Policy::default(),
            &bounds.names.nodes,
            None,
            &bounds.readmes,
        )
        .await?
        .ok_or_else(ApiError::not_found)?;
        let sources = read_sources::Sources::of(&node)?;
        let resolved_sources = sources.targets(conn).await?;
        let mut plan = bounds
            .response
            .as_ref()
            .ok_or_else(|| ApiError::internal("the archive response lost its current preparation"))?
            .clone();
        for &(kind, role) in FULL {
            let mut targets_for_kind = Vec::new();
            for object in sources.objects().filter(|object| object.kind == kind) {
                targets_for_kind.push(
                    resolved_sources
                        .get(&object)
                        .cloned()
                        .ok_or_else(ApiError::not_found)?,
                );
            }
            bind(
                &mut plan,
                role,
                KeyTargetKind::Object(kind),
                &targets_for_kind,
            )?;
        }
        if sources
            .objects()
            .any(|object| !FULL.iter().any(|(kind, _)| *kind == object.kind))
        {
            return Err(ApiError::internal(
                "the native node projection has an undeclared archive source",
            ));
        }
        plan.read_response(ctx, conn, principal.caller(), move |_conn, _reader| {
            Box::pin(async move { Ok(node) })
        })
        .await
    }
    .await;
    match result {
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}

pub(super) fn scoped(ctx: &Ctx) -> bool {
    ctx.admitted_caller()
        .and_then(Caller::grant)
        .is_some_and(|grant| grant.operation_scope().is_some())
        || ctx
            .operation_data::<Bounds>()
            .is_ok_and(|bounds| bounds.names.limited)
}

pub(super) fn problem(ctx: &Ctx, error: ApiError) -> ApiError {
    if scoped(ctx) && error.code() != orch_api_contract::common::ErrorCode::ConflictStaleRevision {
        authorization::problem_without_content(error)
    } else {
        error
    }
}

pub(super) fn safe_body(plan: &orch_api_contract::nodes::NodeArchivePlan) -> NodeArchiveBody {
    NodeArchiveBody {
        preflight_id: plan.preflight_id,
        stamp: plan.stamp.clone(),
        expected_revision: plan.revision,
    }
}
