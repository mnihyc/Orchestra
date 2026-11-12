//! Scope/source adapters for the existing comment author, discussion editor and moderator writers.

use super::super::build;

use super::super::operation;

use super::super::read_operation;

use super::edit_replay;

use crate::key_operations::resolver::Disclosure;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use orch_access_contract::key_scope::KeyAction;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::documents::DocumentComment;

use orch_api_contract::documents::DocumentCommentReceipt;

use orch_api_contract::documents::DocumentCommentUpdateBody;

use orch_api_contract::documents::DocumentCommentUpdateResponse;

use orch_api_contract::documents::DocumentDiscussion;

use orch_api_contract::documents::DocumentDiscussionReceipt;

use orch_api_contract::documents::DocumentDiscussionResolveBody;

use orch_api_contract::documents::DocumentDiscussionResolveResponse;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::TargetResolver;

use orch_authz::Caller;

use orch_authz::CapturedCaller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::documents::discussions;

use orch_db::queries::documents::discussions::CommentRow;

use orch_db::queries::documents::discussions::Full;
const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);

pub(super) enum Native {
    Resolve(discussions::DiscussionRow),
    Edit(super::EditPrefix),
    Delete(super::DeletePrefix),
}

pub(super) struct PreparedWrite {
    pub labels: build::WrittenLabels,
    pub native: Native,
}

async fn current(ctx: &Ctx, conn: &mut Conn, caller: &Caller) -> ApiResult<Caller> {
    CapturedCaller::capture(caller)
        .restore(conn, ctx.app.clock.time())
        .await
        .map_err(authorization::restoration_error)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the credential is no longer valid",
            )
        })
}

pub(super) fn target_plan(
    ctx: &Ctx,
    selection: &str,
    target: &KeyResolvedTarget,
) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(ctx.route, selection)?;
    let mut required = 0;
    for index in 0..plan.variant().facets.len() {
        let facet = &plan.variant().facets[index];
        if facet.target != TargetResolver::Path("id") {
            continue;
        }
        if facet.resources != [DOCUMENT] {
            return Err(ApiError::internal(
                "the comment writer's Document target is not declared",
            ));
        }
        if facet.usage == FacetUsage::Required {
            if !matches!(facet.action, KeyAction::Write | KeyAction::Manage) {
                return Err(ApiError::internal(
                    "the comment writer's action is not declared",
                ));
            }
            required += 1;
        } else if facet.action != KeyAction::Read {
            return Err(ApiError::internal(
                "the comment writer's response is not declared",
            ));
        }
        plan.bind(index, DOCUMENT, std::slice::from_ref(target))?;
    }
    if required != 1 {
        return Err(ApiError::internal(
            "the comment writer needs its one actual Document effect",
        ));
    }
    Ok(plan)
}

pub(super) fn edit<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: DocumentCommentUpdateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let document = ctx.id("id")?;
        let comment = ctx.id("comment_id")?;
        // Preserve the native edit's validation-before-object-lookup order. These
        // supplied fields disclose no stored object or source information.
        let content = orch_documents::comments::check_content(&body.content)
            .map_err(super::invalid_faults)?;
        let attempt = edit_replay::attempt(&body)?;
        let caller = current(ctx, conn, caller).await?;
        let target = read_operation::identity(conn, document).await?;
        let plan = target_plan(ctx, ctx.route.name, &target)?;
        // No source identity or private replay record is read before the actual destination scope.
        let _ = plan.preparation_principal(ctx, conn, &caller).await?;
        let repeated = match (
            &attempt,
            discussions::comment(conn, document, comment).await?,
        ) {
            (Some(attempt), Some(row)) if row.author_id == caller.user() => {
                edit_replay::recorded(ctx, conn, &row, attempt).await?
            }
            _ => false,
        };
        let (labels, sources) =
            operation::prepare_written(ctx, conn, &caller, plan, (!repeated).then_some(&content))
                .await?;
        let mut plan = target_plan(ctx, ctx.route.name, &target)?;
        operation::bind_sources(&mut plan, &sources)?;
        let principal = plan.preparation_principal(ctx, conn, &caller).await?;
        let native =
            super::edit_prefix(ctx, conn, &principal, document, comment, attempt.as_ref()).await?;
        Ok(plan.with_data(PreparedWrite {
            labels,
            native: Native::Edit(native),
        }))
    })
}

pub(super) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    _body: DocumentDiscussionResolveBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let caller = current(ctx, conn, caller).await?;
        let target = read_operation::identity(conn, ctx.id("id")?).await?;
        let mut plan = target_plan(ctx, ctx.route.name, &target)?;
        // This act has no copied input labels. Its selected stored reply is bound after the writer.
        operation::bind_sources(&mut plan, &[])?;
        let principal = plan.preparation_principal(ctx, conn, &caller).await?;
        let native =
            super::resolve_prefix(conn, &principal, ctx.id("id")?, ctx.id("discussion_id")?)
                .await?;
        Ok(plan.with_data(PreparedWrite {
            labels: build::WrittenLabels::default(),
            native: Native::Resolve(native),
        }))
    })
}

pub(super) fn delete<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let caller = current(ctx, conn, caller).await?;
        let document = ctx.id("id")?;
        let target = read_operation::identity(conn, document).await?;
        let Some(row) = discussions::comment(conn, document, ctx.id("comment_id")?).await? else {
            // No author means no actual final variant. A permitted declared candidate may
            // enter only the same ordinary View-before-404 prefix as the native writer.
            for selection in [
                "DocumentCommentDelete.own",
                "DocumentCommentDelete.moderate",
            ] {
                let plan = target_plan(ctx, selection, &target)?;
                if !orch_engine::operation_scope::compatible_selection(&caller, plan.variant()) {
                    continue;
                }
                let index = plan
                    .variant()
                    .facets
                    .iter()
                    .position(|facet| facet.usage == FacetUsage::Required)
                    .ok_or_else(|| {
                        ApiError::internal("the comment delete action is not declared")
                    })?;
                if !plan.permits_binding(&caller, index, DOCUMENT, std::slice::from_ref(&target))? {
                    continue;
                }
                let principal = plan.preparation_principal(ctx, conn, &caller).await?;
                super::super::live(conn, document).await?;
                authorization::require_document(
                    conn,
                    &principal,
                    document,
                    Permission::DocView,
                    Policy::default(),
                )
                .await?;
                return Err(ApiError::not_found());
            }
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "the key does not allow this operation",
            ));
        };
        let selection = if row.author_id == caller.user() {
            "DocumentCommentDelete.own"
        } else {
            "DocumentCommentDelete.moderate"
        };
        let plan = target_plan(ctx, selection, &target)?;
        let principal = plan.preparation_principal(ctx, conn, &caller).await?;
        let native = super::delete_prefix(conn, &principal, document, row.id).await?;
        // Finish the existing empty projection before the final guarded preparation
        // returns. No handler-side SELECT remains before the first tombstone write.
        empty(ctx, conn, &principal).await?;
        Ok(plan.with_data(PreparedWrite {
            labels: build::WrittenLabels::default(),
            native: Native::Delete(native),
        }))
    })
}

/// Unknown native errors have no completed optional-content proof. Only an actual response
/// checkpoint may complete Full again; the existing writer and error status stay authoritative.
pub(super) fn pending(ctx: &Ctx, principal: &Principal) {
    if principal
        .caller()
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
    {
        ctx.checkpoint_disclosure(Disclosure::Pending);
    }
}

#[derive(Clone)]
enum Content {
    Comment(std::sync::Arc<CommentRow>),
    Discussion(std::sync::Arc<Full>),
}

enum Shown {
    Comment(DocumentComment),
    Discussion(DocumentDiscussion),
}

impl Content {
    async fn candidates(&self, conn: &mut Conn) -> ApiResult<Vec<build::WrittenLabelSource>> {
        match self {
            Self::Comment(row) => {
                build::comment_sources(conn, std::slice::from_ref(row.as_ref())).await
            }
            Self::Discussion(full) => {
                build::discussion_sources(conn, std::slice::from_ref(full.as_ref())).await
            }
        }
    }

    async fn project(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        allowed: &[build::WrittenLabelSource],
    ) -> ApiResult<(Shown, Vec<build::WrittenLabelSource>)> {
        match self {
            Self::Comment(row) => {
                let (mut comments, sources) = build::comments_projection(
                    conn,
                    principal,
                    std::slice::from_ref(row.as_ref()),
                    Policy::default(),
                    Some(allowed),
                )
                .await?;
                let comment = comments
                    .pop()
                    .ok_or_else(|| ApiError::internal("the author of a comment is not there"))?;
                Ok((Shown::Comment(comment), sources))
            }
            Self::Discussion(full) => {
                let (mut discussions, sources) = build::discussions_projection(
                    conn,
                    principal,
                    full.discussion.document_id,
                    std::slice::from_ref(full.as_ref()),
                    Policy::default(),
                    Some(allowed),
                )
                .await?;
                let discussion = discussions
                    .pop()
                    .ok_or_else(|| ApiError::internal("a discussion could not be shown"))?;
                Ok((Shown::Discussion(discussion), sources))
            }
        }
    }
}

async fn response(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    content: Content,
) -> ApiResult<Option<Shown>> {
    if ctx.disclosure() == Disclosure::Denied {
        return Ok(None);
    }
    let selection = principal
        .caller()
        .admitted_operation()
        .ok_or_else(|| ApiError::internal("the comment writer has no admitted operation"))?;
    let Some(caller) = crate::key_operations::admission::response_caller(
        ctx,
        conn,
        principal.caller(),
        selection,
        ctx.app.clock.time(),
    )
    .await?
    else {
        return Ok(None);
    };
    let caller = caller.caller;
    let document = ctx.id("id")?;
    let target = read_operation::identity(conn, document).await?;
    let mut preparation = target_plan(ctx, selection, &target)?;
    let candidates = content.candidates(conn).await?;
    let proposed = operation::allowed_sources(conn, &caller, &preparation, candidates).await?;
    operation::bind_sources(&mut preparation, &proposed)?;
    let reader = preparation
        .preparation_principal(ctx, conn, &caller)
        .await?;
    let allowed: Vec<_> = proposed.into_iter().map(|(source, _)| source).collect();
    let (_, actual) = content.project(conn, &reader, &allowed).await?;
    // Re-resolve just the names actually materialized, including real file owners and
    // node-derived chat names. Unreadable/unreturned candidates never become required reads.
    let mut plan = target_plan(
        ctx,
        selection,
        &read_operation::identity(conn, document).await?,
    )?;
    let actual = operation::allowed_sources(conn, &caller, &plan, actual).await?;
    operation::bind_sources(&mut plan, &actual)?;
    let allowed: Vec<_> = actual.into_iter().map(|(source, _)| source).collect();
    plan.read_response(ctx, conn, principal.caller(), move |conn, reader| {
        Box::pin(async move { Ok(content.project(conn, reader, &allowed).await?.0) })
    })
    .await
}

pub(super) async fn comment(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &CommentRow,
) -> ApiResult<Option<DocumentComment>> {
    match response(
        ctx,
        conn,
        principal,
        Content::Comment(std::sync::Arc::new(row.clone())),
    )
    .await?
    {
        Some(Shown::Comment(comment)) => Ok(Some(comment)),
        None => Ok(None),
        Some(Shown::Discussion(_)) => {
            Err(ApiError::internal("the comment projection changed kind"))
        }
    }
}

pub(super) async fn discussion(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    full: &Full,
) -> ApiResult<Option<DocumentDiscussion>> {
    match response(
        ctx,
        conn,
        principal,
        Content::Discussion(std::sync::Arc::new(full.clone())),
    )
    .await?
    {
        Some(Shown::Discussion(discussion)) => Ok(Some(discussion)),
        None => Ok(None),
        Some(Shown::Comment(_)) => {
            Err(ApiError::internal("the discussion projection changed kind"))
        }
    }
}

/// Empty Delete has no public content source. Use the same existing completion seam,
/// under the same current guarded identity, rather than inventing a Read requirement.
pub(super) async fn empty(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
    let selection = principal
        .caller()
        .admitted_operation()
        .ok_or_else(|| ApiError::internal("the comment deletion has no admitted operation"))?;
    let mut plan = target_plan(
        ctx,
        selection,
        &read_operation::identity(conn, ctx.id("id")?).await?,
    )?;
    if ctx.route.name != "DocumentCommentDelete" {
        // A source-free static error still needs its declared current Document Read;
        // it contains no comment, foreign labels or parent metadata to bind.
        operation::bind_sources(&mut plan, &[])?;
    }
    plan.read_response(ctx, conn, principal.caller(), |_, _| {
        Box::pin(async { Ok(()) })
    })
    .await?;
    Ok(())
}

pub(super) fn finish_comment(
    ctx: &Ctx,
    row: &CommentRow,
    full: Option<DocumentComment>,
) -> DocumentCommentUpdateResponse {
    match (full, ctx.key_receipt_required()) {
        (Some(full), false) => DocumentCommentUpdateResponse::full(full),
        _ => DocumentCommentUpdateResponse::Receipt(DocumentCommentReceipt {
            projection: ReceiptProjection::Receipt,
            document_id: row.document_id,
            comment_id: row.id,
            seq: row.seq_updated,
        }),
    }
}

pub(super) fn finish_discussion(
    ctx: &Ctx,
    full: &Full,
    shown: Option<DocumentDiscussion>,
) -> DocumentDiscussionResolveResponse {
    match (shown, ctx.key_receipt_required()) {
        (Some(shown), false) => DocumentDiscussionResolveResponse::full(shown),
        _ => DocumentDiscussionResolveResponse::Receipt(DocumentDiscussionReceipt {
            projection: ReceiptProjection::Receipt,
            document_id: full.discussion.document_id,
            discussion_id: full.discussion.id,
            seq: full.discussion.seq_updated,
        }),
    }
}

struct ReplaySource {
    document: orch_types::Id,
    id: orch_types::Id,
    discussion: bool,
    versions: Vec<(orch_types::Id, orch_types::Seq)>,
    labels: Vec<build::WrittenLabelSource>,
}

impl Content {
    fn document(&self) -> orch_types::Id {
        match self {
            Self::Comment(row) => row.document_id,
            Self::Discussion(full) => full.discussion.document_id,
        }
    }
}

async fn replay_response(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    content: Content,
) -> ApiResult<Option<(Shown, Vec<build::WrittenLabelSource>)>> {
    use crate::key_operations::admission::ResponseProjection;
    let document = content.document();
    let selection = ctx.route.name;
    let target = read_operation::identity(conn, document).await?;
    let mut proposed_plan = target_plan(ctx, selection, &target)?;
    let candidates = content.candidates(conn).await?;
    let proposed =
        operation::allowed_sources(conn, principal.caller(), &proposed_plan, candidates).await?;
    operation::bind_sources(&mut proposed_plan, &proposed)?;
    let proposed = proposed
        .into_iter()
        .map(|(source, _)| source)
        .collect::<Vec<_>>();
    let indices = proposed_plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(index, facet)| (facet.usage == FacetUsage::ResponseContent).then_some(index))
        .collect();
    let discovery = ctx.for_effect_discovery(ctx.user()?.as_ref().clone(), ctx.app.clock.now());
    let probe = content.clone();
    let observing = ctx.clone();
    let actual = proposed_plan
        .read_response_projection(
            &discovery,
            conn,
            principal.caller(),
            ResponseProjection::Replay(indices),
            move |conn, reader| {
                Box::pin(async move {
                    authorization::require_document(
                        conn,
                        reader,
                        document,
                        Permission::DocView,
                        Policy::default(),
                    )
                    .await?;
                    observing.publication.retain_ceiling(reader.caller())?;
                    Ok(probe.project(conn, reader, &proposed).await?.1)
                })
            },
        )
        .await?;
    let Some(actual) = actual else {
        return Ok(None);
    };
    let mut plan = target_plan(
        ctx,
        selection,
        &read_operation::identity(conn, document).await?,
    )?;
    let actual = operation::allowed_sources(conn, principal.caller(), &plan, actual).await?;
    operation::bind_sources(&mut plan, &actual)?;
    let actual = actual
        .into_iter()
        .map(|(source, _)| source)
        .collect::<Vec<_>>();
    let frozen = actual.clone();
    let indices = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(index, facet)| (facet.usage == FacetUsage::ResponseContent).then_some(index))
        .collect();
    let observing = ctx.clone();
    let value = plan
        .read_response_projection(
            ctx,
            conn,
            principal.caller(),
            ResponseProjection::Replay(indices),
            move |conn, reader| {
                Box::pin(async move {
                    authorization::require_document(
                        conn,
                        reader,
                        document,
                        Permission::DocView,
                        Policy::default(),
                    )
                    .await?;
                    observing.publication.retain_ceiling(reader.caller())?;
                    Ok(content.project(conn, reader, &actual).await?.0)
                })
            },
        )
        .await?;
    Ok(value.map(|value| (value, frozen)))
}

async fn replay_result(
    ctx: &Ctx,
    document: orch_types::Id,
    id: orch_types::Id,
    discussion: bool,
) -> ApiResult<Option<Shown>> {
    use crate::key_operations::{native, publication};
    let ctx = ctx.for_completed_replay_projection();
    let accepted = authorization::caller_of(ctx.user()?)?;
    let result = native::completed(&ctx, &accepted, move |ctx, conn, reader| {
        Box::pin(async move {
            let content = if discussion {
                Content::Discussion(std::sync::Arc::new(
                    discussions::get(conn, document, id)
                        .await?
                        .ok_or_else(ApiError::not_found)?,
                ))
            } else {
                Content::Comment(std::sync::Arc::new(
                    discussions::comment(conn, document, id)
                        .await?
                        .ok_or_else(ApiError::not_found)?,
                ))
            };
            let versions = match &content {
                Content::Comment(row) => vec![(row.id, row.seq_updated)],
                Content::Discussion(full) => {
                    std::iter::once((full.discussion.id, full.discussion.seq_updated))
                        .chain(full.comments.iter().map(|row| (row.id, row.seq_updated)))
                        .collect()
                }
            };
            Ok(replay_response(ctx, conn, reader, content)
                .await?
                .map(|(value, labels)| {
                    (
                        value,
                        ReplaySource {
                            document,
                            id,
                            discussion,
                            versions,
                            labels,
                        },
                    )
                }))
        })
    })
    .await?;
    let Some((value, source)) = result else {
        return Ok(None);
    };
    publication::retain(
        &ctx,
        &accepted,
        std::sync::Arc::new(source),
        |ctx, conn, reader, source| Box::pin(check_replay(ctx, conn, reader, source)),
    )?;
    Ok(Some(value))
}

async fn check_replay(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    source: &ReplaySource,
) -> ApiResult<()> {
    let ctx = ctx.for_completed_replay_projection();
    let versions =
        discussions::source_versions(conn, source.document, source.id, source.discussion).await?;
    if versions != source.versions {
        return Err(ApiError::not_found());
    }
    let document = source.document;
    let mut plan = target_plan(
        &ctx,
        ctx.route.name,
        &read_operation::identity(conn, document).await?,
    )?;
    let actual =
        operation::allowed_sources(conn, reader.caller(), &plan, source.labels.clone()).await?;
    if !source
        .labels
        .iter()
        .all(|source| actual.iter().any(|(current, _)| source == current))
    {
        return Err(ApiError::not_found());
    }
    operation::bind_sources(&mut plan, &actual)?;
    let indices = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter_map(|(index, facet)| (facet.usage == FacetUsage::ResponseContent).then_some(index))
        .collect();
    let checking = ctx.clone();
    let labels = source.labels.clone();
    plan.read_response_projection(
        &ctx,
        conn,
        reader.caller(),
        crate::key_operations::admission::ResponseProjection::Replay(indices),
        move |conn, reader| {
            Box::pin(async move {
                authorization::require_document(
                    conn,
                    reader,
                    document,
                    Permission::DocView,
                    Policy::default(),
                )
                .await?;
                checking.publication.retain_ceiling(reader.caller())?;
                if !build::label_sources_visible(&checking, conn, reader, &labels).await? {
                    return Err(ApiError::not_found());
                }
                Ok(())
            })
        },
    )
    .await?
    .ok_or_else(ApiError::not_found)
}

pub(super) async fn replay_comment(
    ctx: &Ctx,
    document: orch_types::Id,
    id: orch_types::Id,
) -> ApiResult<Option<DocumentComment>> {
    match replay_result(ctx, document, id, false).await? {
        Some(Shown::Comment(value)) => Ok(Some(value)),
        None => Ok(None),
        Some(Shown::Discussion(_)) => Err(ApiError::internal(
            "the comment replay returned another kind",
        )),
    }
}

pub(super) async fn replay_discussion(
    ctx: &Ctx,
    document: orch_types::Id,
    id: orch_types::Id,
) -> ApiResult<Option<DocumentDiscussion>> {
    match replay_result(ctx, document, id, true).await? {
        Some(Shown::Discussion(value)) => Ok(Some(value)),
        None => Ok(None),
        Some(Shown::Comment(_)) => Err(ApiError::internal(
            "the discussion replay returned another kind",
        )),
    }
}
