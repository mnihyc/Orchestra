//! Stored native deletion reviews use one guarded save and independent completed Read.

use super::*;
use crate::key_operations::native::{effects, reply};
use crate::key_operations::{native, publication, targets};
use crate::middleware::transaction_guard;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyResolvedTarget;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::ownership::{NodeDeletePreviewReceipt, NodeDeletePreviewResponse};
use orch_api_contract::ownership_sources::{self as sources, Source};
use orch_api_contract::routes::Endpoint;
use orch_authz::{Caller, Principal};
use std::collections::BTreeSet;
use std::sync::Arc;

pub(super) struct Preview;

/// A saved review keeps its own real operation/key; current deletion is admitted separately.
pub(super) async fn require_origins(
    ctx: &Ctx,
    conn: &mut Conn,
    confirmer: &Principal,
    id: Id,
    origins: &[orch_authz::CapturedOperation],
) -> ApiResult<()> {
    for origin in origins {
        require_origin(ctx, conn, confirmer, id, origin).await?;
    }
    Ok(())
}

async fn require_origin(
    ctx: &Ctx,
    conn: &mut Conn,
    confirmer: &Principal,
    id: Id,
    origin: &orch_authz::CapturedOperation,
) -> ApiResult<()> {
    let refused = || {
        ApiError::new(
            ErrorCode::AuthzDenied,
            "the original deletion review is no longer allowed",
        )
    };
    if origin.operation() != sources::DELETE_PREVIEW.operation || origin.user() != confirmer.user()
    {
        return Err(refused());
    }
    let original = origin
        .restore_current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(authorization::restoration_error)?
        .ok_or_else(refused)?;
    let plan = native::bind_required_route(
        NodeDeletePreview::ROUTE,
        &sources::DELETE_PREVIEW,
        &facts(conn, id).await?,
    )?;
    let accepted = plan
        .shared()
        .admit(&original, false)
        .map_err(crate::key_operations::admission::binding_error)?;
    let principal = authorization::load_principal(conn, accepted, ctx.app.clock.time()).await?;
    let user = orch_db::queries::identity::user_by_id(conn, origin.user())
        .await?
        .ok_or_else(refused)?;
    let principal = crate::sign_in::second_step::hold_key_origin(conn, &user, principal).await?;
    allowed(conn, &principal, id)
        .await
        .map_err(authorization::problem_without_content)?;
    if !principal
        .caller()
        .key_deadline_allows(ctx.app.clock.as_ref())
    {
        return Err(refused());
    }
    Ok(())
}

async fn facts(conn: &mut Conn, id: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    let node = targets::node(conn, id).await?;
    Ok(vec![
        native::Facts::one(Source::DeleteSubject, node.object.kind, node.clone()),
        native::Facts::one(Source::DeleteReviewNode, node.object.kind, node),
    ])
}

async fn allowed(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<()> {
    authorization::require_projected(
        conn,
        principal,
        id,
        MANAGE,
        Act::Delete,
        Policy::default(),
        native::modern(principal.caller()),
    )
    .await
}

impl effects::Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = Arc<DeleteReview>;
    type Output = SavedReview;

    fn declaration(_review: &Arc<DeleteReview>) -> &'static Declaration<Source> {
        &sources::DELETE_PREVIEW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Arc<DeleteReview>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let known = native::bind_known(ctx, &sources::DELETE_PREVIEW, &facts(conn, id).await?)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            allowed(conn, &principal, id).await?;
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let (_, kind) = placed(conn, id).await?;
            if kind == NodeKind::Org
                && hierarchy::org(conn, id)
                    .await?
                    .is_some_and(|org| org.org_kind == "personal")
            {
                return Err(invalid("id", "a personal org is never deleted"));
            }
            let set = hierarchy::deletion_set(conn, id, false).await?;
            let prepared = Arc::new(prepare_delete_review(ctx, conn, id, &set).await?);
            let facts = facts(conn, id).await?;
            Ok((prepared, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        review: &'a Arc<DeleteReview>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(allowed(conn, principal, review.node.id))
    }
}

impl effects::Apply for Preview {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        review: &'a Arc<DeleteReview>,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<SavedReview>>> {
        Box::pin(async move {
            Ok(effects::Outcome::Applied(
                save_delete_review(
                    ctx,
                    conn,
                    principal,
                    Arc::clone(review),
                    move || before.check(),
                    &[],
                )
                .await?,
            ))
        })
    }
}

pub(super) async fn run(ctx: Ctx) -> ApiResult<NodeDeletePreviewResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Preview>(&ctx, &mut tx).await?;
    let identity = transaction_guard::current_http_identity(
        &completed.ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    identity.check_deadline(&completed.ctx.app)?;
    tx.commit().await.map_err(DbError::from)?;
    let output = completed.output;
    let receipt = NodeDeletePreviewReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        preflight_id: output.preflight.id,
        expires_at: Time::new(output.preflight.expires_at),
        node_id: output.review.node.id,
        revision: output.review.node.revision,
        counts: counts_of(&output.review.set),
        purge_at: Time::new(output.review.purge_at),
    };
    publication::receipt(&completed.ctx, &receipt)?;
    let full = reply::render::<Stored>(&completed.ctx, &completed.accepted, output).await?;
    Ok(full.map_or_else(
        || NodeDeletePreviewResponse::Receipt(receipt),
        NodeDeletePreviewResponse::full,
    ))
}

struct Stored;
struct Subject {
    node: hierarchy::NodeRow,
    row: orch_db::queries::jobs::forks::PreflightRow,
}
struct Content {
    restoration: KeyResolvedTarget,
    visible: BTreeSet<Id>,
    displays: BTreeSet<Id>,
}

impl reply::Projection for Stored {
    type Source = Source;
    type Input = SavedReview;
    type Subject = Subject;
    type Value = Content;
    type Reply = NodeDeletePlan;
    const READ_SOURCE: Source = Source::DeleteReviewNode;

    fn declaration(_input: &SavedReview) -> &'static Declaration<Source> {
        &sources::DELETE_PREVIEW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a SavedReview,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = orch_db::queries::jobs::forks::preflight(conn, input.preflight.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let id = input.review.node.id;
            let node = hierarchy::with_ancestors(conn, &[id])
                .await?
                .into_iter()
                .find(|node| node.id == id)
                .ok_or_else(ApiError::not_found)?;
            let facts = facts(conn, id).await?;
            Ok((Subject { node, row }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a SavedReview,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let original = &input.preflight;
            let actual = &subject.row;
            if actual.kind != DELETE_PLAN_KIND
                || actual.node_id != Some(input.review.node.id)
                || actual.created_by != principal.user()
                || actual.created_by != original.created_by
                || actual.plan != original.plan
                || actual.expires_at != original.expires_at
                || actual.expires_at <= ctx.now
                || subject.node.parent_id != input.review.node.parent_id
                || subject.node.path != input.review.node.path
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the stored review source changed",
                ));
            }
            allowed(conn, principal, subject.node.id).await
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a SavedReview,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(Content, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let restore = subject.node.parent_id.unwrap_or(subject.node.id);
            let restoration = targets::node(conn, restore).await?;
            let visible = view::reference_visibility(
                conn,
                principal,
                &[subject.node.id, restore],
                Policy::default(),
            )
            .await?;
            let displays = view::reference_thread_visibility(
                conn,
                principal,
                &[subject.node.id, restore],
                Policy::default(),
            )
            .await?;
            let facts = vec![native::Facts::collection(
                Source::DeleteReviewPlaces,
                vec![(restoration.object.kind, vec![restoration.clone()])],
            )];
            Ok((
                Content {
                    restoration,
                    visible,
                    displays,
                },
                facts,
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a SavedReview,
        _prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<NodeDeletePlan>> {
        Box::pin(render_delete_review(conn, principal, input))
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a SavedReview,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject.node.id == fresh.subject.node.id
                && opened.subject.node.parent_id == fresh.subject.node.parent_id
                && opened.subject.node.path == fresh.subject.node.path
                && opened.subject.node.name == fresh.subject.node.name
                && opened.value.restoration == fresh.value.restoration
                && opened.value.visible.is_subset(&fresh.value.visible)
                && opened.value.displays.is_subset(&fresh.value.displays))
        })
    }
}
