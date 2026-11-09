//! A native compound move review captures its audience once, then independently publishes Read.

use super::*;
use crate::key_operations::native::{self, effects, reply};
use crate::routes::document_tree::metadata;
use futures::future::BoxFuture;
use orch_api_contract::access::{PREVIEW_MAX_PEOPLE, PagePreview, PreviewPerson};
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::document_sources::{self as sources, MoveSource as Source};
use orch_api_contract::documents::{DocumentMovePreviewReceipt, DocumentMovePreviewResponse};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Caller;
use std::collections::BTreeMap;
use std::sync::Arc;
pub(super) struct Reviews;
#[derive(Clone)]
pub(super) struct Review {
    source: DocumentRow,
    directory: document_tree_domain::Directory,
    to: Id,
    plan: documents::MovePlan,
    cuts: Vec<metadata::Cut>,
}
pub(super) struct Proposal {
    source: DocumentRow,
    directory: document_tree_domain::Directory,
    to: Id,
    changes: bool,
    cuts: Vec<metadata::Cut>,
}
pub(super) struct Subject {
    row: DocumentRow,
    directory: document_tree_domain::Directory,
}

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the native move review is no longer readable",
    )
}
async fn current(
    conn: &mut Conn,
    principal: &Principal,
    review: &Review,
    row: &DocumentRow,
    directory: document_tree_domain::Directory,
) -> ApiResult<()> {
    if row.id != review.source.id
        || row.node_id != review.source.node_id
        || row.org_id != review.source.org_id
        || directory != review.directory
    {
        return Err(refused());
    }
    for cut in &review.cuts {
        match metadata::unchanged(conn, principal, cut, principal.now()).await {
            Ok(()) => {}
            Err(error) if error.code() == ErrorCode::ConflictStaleRevision => return Err(refused()),
            Err(error) => return Err(error),
        }
    }
    may_move(conn, principal, Policy::default(), row, review.to).await?;
    if native::modern(principal.caller()) {
        authorization::require_document(
            conn,
            principal,
            row.id,
            Permission::DocView,
            Policy::default(),
        )
        .await?;
    }
    Ok(())
}
impl effects::Effect for Reviews {
    type Source = Source;
    type Query = ();
    type Body = DocumentMoveBody;
    type Value = Proposal;
    type Output = Arc<Review>;
    fn declaration(_value: &Proposal) -> &'static Declaration<Source> {
        &sources::MOVE_PREVIEW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: DocumentMoveBody,
    ) -> BoxFuture<'a, ApiResult<(Proposal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let native = super::move_operands::prepare(
                ctx,
                conn,
                caller,
                body.node_id,
                &sources::MOVE_PREVIEW,
            )
            .await?;
            let row = native.source;
            let directory = native.directory;
            let principal = native.principal;
            let changes = native.changes;
            let bound = native.facts;
            let mut cuts = Vec::new();
            for node in [row.node_id, body.node_id] {
                if !cuts.iter().any(|cut: &metadata::Cut| cut.node == node) {
                    cuts.push(metadata::cut(conn, &principal, node, principal.now()).await?);
                }
            }
            Ok((
                Proposal {
                    source: row,
                    directory,
                    to: body.node_id,
                    changes,
                    cuts,
                },
                bound,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            may_move(conn, principal, Policy::default(), &value.source, value.to).await?;
            Ok(())
        })
    }
}
impl effects::Apply for Reviews {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Proposal,
        _before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Arc<Review>>>> {
        Box::pin(async move {
            if !principal
                .caller()
                .key_deadline_allows(ctx.app.clock.as_ref())
            {
                return Err(ApiError::new(
                    ErrorCode::AuthInvalidToken,
                    "the review key has ended",
                ));
            }
            // This is the only audience planner call in this request. It has no first business effect.
            let plan = if value.changes {
                documents::plan_move(
                    conn,
                    principal,
                    value.source.id,
                    value.to,
                    Policy::default(),
                )
                .await
                .map_err(ApiError::internal)?
            } else {
                documents::MovePlan::default()
            };
            if !principal
                .caller()
                .key_deadline_allows(ctx.app.clock.as_ref())
            {
                return Err(ApiError::new(
                    ErrorCode::AuthInvalidToken,
                    "the review key has ended",
                ));
            }
            Ok(effects::Outcome::Unchanged(Arc::new(Review {
                source: value.source.clone(),
                directory: value.directory,
                to: value.to,
                plan,
                cuts: value.cuts.clone(),
            })))
        })
    }
}
impl reply::Projection for Reviews {
    type Source = Source;
    type Input = Arc<Review>;
    type Subject = Subject;
    type Value = ();
    type Reply = PagePreview;
    const READ_SOURCE: Source = Source::Document;
    fn declaration(_input: &Arc<Review>) -> &'static Declaration<Source> {
        &sources::MOVE_PREVIEW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Arc<Review>,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = live(conn, input.source.id).await?;
            let directory = stored_directory(conn, &row).await?;
            let bound = super::move_operands::facts(conn, &row, input.to).await?;
            Ok((Subject { row, directory }, bound))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Arc<Review>,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(current(
            conn,
            principal,
            input,
            &subject.row,
            subject.directory,
        ))
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Arc<Review>,
        _subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Arc<Review>,
        _prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<PagePreview>> {
        Box::pin(render(conn, &input.plan))
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Arc<Review>,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            if opened.subject.row.node_id != fresh.subject.row.node_id
                || opened.subject.directory != fresh.subject.directory
            {
                return Ok(false);
            }
            current(
                conn,
                principal,
                input,
                &fresh.subject.row,
                fresh.subject.directory,
            )
            .await?;
            Ok(true)
        })
    }
}
async fn render(conn: &mut Conn, plan: &documents::MovePlan) -> ApiResult<PagePreview> {
    let people: Vec<Id> = plan
        .gaining
        .keys()
        .chain(plan.losing.keys())
        .copied()
        .collect();
    let known = refusals::user_refs(conn, &people).await?;
    let shown = |side: &BTreeMap<Id, Vec<Permission>>| -> (Vec<PreviewPerson>, i64) {
        let most = usize::try_from(PREVIEW_MAX_PEOPLE).unwrap_or(usize::MAX);
        let mut listed = Vec::new();
        let mut more = 0_i64;
        for (person, permissions) in side {
            match known.get(person) {
                Some(user) if listed.len() < most => listed.push(PreviewPerson {
                    user: user.clone(),
                    permissions: permissions
                        .iter()
                        .map(|permission| permission.as_str().to_owned())
                        .collect(),
                    pages: 1,
                }),
                _ => more += 1,
            }
        }
        (listed, more)
    };
    let (gaining, more_gaining) = shown(&plan.gaining);
    let (losing, more_losing) = shown(&plan.losing);
    Ok(PagePreview {
        owner_reads: None,
        pages: 1,
        orgs: None,
        gaining,
        losing,
        more: more_gaining + more_losing,
    })
}
fn receipt(input: &Review) -> DocumentMovePreviewResponse {
    DocumentMovePreviewResponse::Receipt(DocumentMovePreviewReceipt {
        projection: ReceiptProjection::Receipt,
        document_id: input.source.id,
        node_id: input.to,
    })
}
pub(super) async fn preview(
    ctx: Ctx,
    (): (),
    _body: DocumentMoveBody,
) -> ApiResult<DocumentMovePreviewResponse> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Reviews>(&ctx, &mut tx).await?;
    initial.check_deadline(&completed.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    let safe = receipt(&completed.output);
    crate::key_operations::publication::receipt(&completed.ctx, &safe)?;
    Ok(
        match reply::render::<Reviews>(&completed.ctx, &completed.accepted, completed.output)
            .await?
        {
            Some(value) => DocumentMovePreviewResponse::Full(Box::new(value)),
            None => safe,
        },
    )
}
