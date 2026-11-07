//! Resolve optional copied reference labels through the same current operation boundary.

use super::build::{WrittenLabelSource, WrittenLabels};
use crate::key_operations::targets;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::DocumentOpsCreateBody;
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::Caller;
use orch_db::Conn;
use orch_types::Id;

// These are the typed collection roles of this operation's canonical declaration. Separate kinds
// preserve each source's actual family without treating a creation parent as the business resource.
const SOURCES: &[(KeyTargetKind, &str)] = &[
    (KeyTargetKind::Object(ObjectKind::Org), "reference_orgs"),
    (
        KeyTargetKind::Object(ObjectKind::Project),
        "reference_projects",
    ),
    (KeyTargetKind::Object(ObjectKind::Topic), "reference_topics"),
    (
        KeyTargetKind::Object(ObjectKind::Thread),
        "reference_threads",
    ),
    (
        KeyTargetKind::Object(ObjectKind::Document),
        "reference_documents",
    ),
    (
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        "reference_chats",
    ),
];

fn collection(plan: &OperationPlan, kind: KeyTargetKind) -> ApiResult<usize> {
    let role = SOURCES
        .iter()
        .find_map(|(wanted, role)| (*wanted == kind).then_some(*role))
        .ok_or_else(|| ApiError::internal("a reference source has no declared family"))?;
    plan.variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
        .ok_or_else(|| ApiError::internal("the reference source collection is not declared"))
}

fn destination(ctx: &Ctx, target: &KeyResolvedTarget) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    let index = plan
        .variant()
        .facets
        .iter()
        .position(|facet| {
            facet.usage == FacetUsage::Required
                && facet.action == KeyAction::Write
                && facet.target == TargetResolver::PermissionObject
        })
        .ok_or_else(|| ApiError::internal("the document destination is not declared"))?;
    plan.bind(
        index,
        KeyTargetKind::Object(ObjectKind::Document),
        std::slice::from_ref(target),
    )?;
    let disclosure = plan
        .variant()
        .facets
        .iter()
        .position(|facet| facet.usage == FacetUsage::ResponseContent)
        .ok_or_else(|| ApiError::internal("the document disclosure is not declared"))?;
    plan.bind(
        disclosure,
        KeyTargetKind::Object(ObjectKind::Document),
        std::slice::from_ref(target),
    )?;
    Ok(plan)
}

pub(super) fn bind_sources(
    plan: &mut OperationPlan,
    sources: &[(WrittenLabelSource, KeyResolvedTarget)],
) -> ApiResult<()> {
    for (kind, _) in SOURCES {
        let index = collection(plan, *kind)?;
        let targets: Vec<_> = sources
            .iter()
            .filter(|(_, target)| target.object.kind == *kind)
            .map(|(_, target)| target.clone())
            .collect();
        if targets.is_empty() {
            plan.bind_empty(index)?;
        } else {
            plan.bind(index, *kind, &targets)?;
        }
    }
    Ok(())
}

pub(super) async fn source_target(
    conn: &mut Conn,
    source: WrittenLabelSource,
) -> ApiResult<KeyResolvedTarget> {
    match source {
        // Reference identity may be retained; ordinary projection still decides whether any name is visible.
        WrittenLabelSource::Node(id) => targets::node_identity(conn, id).await,
        WrittenLabelSource::Document(id) => {
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Document,
                    id,
                },
            )
            .await
        }
        WrittenLabelSource::Chat(id) => targets::chat(conn, id).await,
    }
}

/// Body/source planning only. The registry admits the completed plan, and the ordinary writer
/// retains all its permission, validation, locking and atomic-batch checks.
pub(super) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: DocumentOpsCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let target = targets::permission_object(ctx, conn).await?;
        let id = ctx.id("id")?;
        let client: Id = body
            .client_id
            .parse()
            .map_err(|_| crate::routes::nodes::invalid("client_id", "must be a UUID"))?;
        let repeated =
            orch_db::queries::documents::batch::repeated_seq(conn, id, caller.user(), client)
                .await?
                .is_some();
        let (labels, allowed) = prepare_written(
            ctx,
            conn,
            caller,
            destination(ctx, &target)?,
            (!repeated).then_some(&body.ops),
        )
        .await?;
        let mut plan = destination(ctx, &target)?;
        bind_sources(&mut plan, &allowed)?;
        Ok(plan.with_data(labels))
    })
}

/// The one copy-label preparation for existing document and comment writes. A retained
/// acceptance has no replacement-input labels to acquire; ordinary writers still own replay.
pub(super) async fn prepare_written<T: serde::Serialize>(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    mut preparation: OperationPlan,
    content: Option<&T>,
) -> ApiResult<(WrittenLabels, Vec<(WrittenLabelSource, KeyResolvedTarget)>)> {
    // The actual destination is admitted before any optional source identity or name lookup.
    let _ = preparation.preparation_principal(ctx, conn, caller).await?;
    let mut allowed = match content {
        Some(content) => {
            let candidates = WrittenLabels::candidates(conn, content).await?;
            allowed_sources(conn, caller, &preparation, candidates).await?
        }
        None => Vec::new(),
    };
    bind_sources(&mut preparation, &allowed)?;
    let principal = preparation.preparation_principal(ctx, conn, caller).await?;
    let labels = match content {
        Some(content) => {
            let sources: Vec<_> = allowed.iter().map(|(source, _)| *source).collect();
            WrittenLabels::capture_allowed(conn, &principal, content, &sources).await?
        }
        None => WrittenLabels::default(),
    };
    allowed.retain(|(source, _)| labels.sources().contains(source));
    Ok((labels, allowed))
}

/// Typed response pieces; no existing-content fields are fabricated for a receipt.
pub(super) struct Readback {
    pub blocks: Vec<orch_api_contract::documents::DocumentBlock>,
    pub document: Option<orch_api_contract::documents::DocumentRef>,
}

/// Preserve stored D content while bounding every dynamically resolved protected reference name.
/// A fresh shared response checkpoint revalidates both D and the materialized source set.
pub(super) async fn read_back(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &orch_authz::Principal,
    rows: &[orch_db::queries::documents::BlockRow],
    policy: orch_authz::Policy,
    document: bool,
) -> ApiResult<Option<Readback>> {
    use super::build::{ProjectedBlocks, ProjectedDocumentRef};
    use orch_access_contract::permissions::Permission;
    if ctx.key_receipt_required() {
        return Ok(None);
    }
    let id = ctx.id("id")?;
    let listed = super::build::standing(conn, current, id, policy).await?;
    if !listed.holds(Permission::DocView) {
        return Ok(None);
    }
    let row = if document {
        Some(super::live(conn, id).await?)
    } else {
        None
    };
    let target = targets::permission_object(ctx, conn).await?;
    let mut preparation = destination(ctx, &target)?;
    let mut candidates = ProjectedBlocks::candidates(conn, rows).await?;
    if let Some(row) = &row {
        candidates.extend(ProjectedDocumentRef::candidates(conn, row, &listed).await?);
    }
    candidates.sort_unstable();
    candidates.dedup();
    let proposed = allowed_sources(conn, current.caller(), &preparation, candidates).await?;
    bind_sources(&mut preparation, &proposed)?;
    let reader = preparation
        .preparation_principal(ctx, conn, current.caller())
        .await?;
    let listed = super::build::standing(conn, &reader, id, policy).await?;
    if !listed.holds(Permission::DocView) {
        return Ok(None);
    }
    let sources: Vec<_> = proposed.iter().map(|(source, _)| *source).collect();
    let observed = ProjectedBlocks::capture_allowed(conn, &reader, rows, policy, &sources).await?;
    let mut materialized = observed.sources().to_vec();
    if let Some(row) = &row {
        let observed = ProjectedDocumentRef::capture_allowed(
            conn,
            &reader,
            row,
            &listed,
            reader.now().get(),
            policy,
            &sources,
        )
        .await?;
        materialized.extend_from_slice(observed.sources());
    }
    materialized.sort_unstable();
    materialized.dedup();
    // Re-resolve actual materialized source ancestry. Recapture only this admitted source set:
    // a new visible name or changed file owner cannot silently widen the response.
    let mut response = destination(ctx, &targets::permission_object(ctx, conn).await?)?;
    let actual = allowed_sources(conn, current.caller(), &response, materialized).await?;
    bind_sources(&mut response, &actual)?;
    let allowed: Vec<_> = actual.into_iter().map(|(source, _)| source).collect();
    let rows = rows.to_vec();
    let answer = response
        .read_response(ctx, conn, current.caller(), move |conn, reader| {
            Box::pin(async move {
                let listed = super::build::standing(conn, reader, id, policy).await?;
                if !listed.holds(Permission::DocView) {
                    return Ok(None);
                }
                let projected =
                    ProjectedBlocks::capture_allowed(conn, reader, &rows, policy, &allowed).await?;
                let document = match row {
                    Some(row) => Some(
                        ProjectedDocumentRef::capture_allowed(
                            conn,
                            reader,
                            &row,
                            &listed,
                            reader.now().get(),
                            policy,
                            &allowed,
                        )
                        .await?
                        .document,
                    ),
                    None => None,
                };
                Ok(Some(Readback {
                    blocks: projected.blocks,
                    document,
                }))
            })
        })
        .await?;
    Ok(answer.flatten())
}

pub(super) async fn allowed_sources(
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    candidates: Vec<WrittenLabelSource>,
) -> ApiResult<Vec<(WrittenLabelSource, KeyResolvedTarget)>> {
    let mut allowed = Vec::new();
    for source in candidates {
        let target = match source_target(conn, source).await {
            Ok(target) => target,
            Err(error)
                if matches!(
                    error.code(),
                    ErrorCode::NotFound
                        | ErrorCode::NodeArchived
                        | ErrorCode::AuthSuspended
                        | ErrorCode::OrgSuspended
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let index = collection(plan, target.object.kind)?;
        if plan.permits_binding(
            caller,
            index,
            target.object.kind,
            std::slice::from_ref(&target),
        )? {
            allowed.push((source, target));
        }
    }
    Ok(allowed)
}
