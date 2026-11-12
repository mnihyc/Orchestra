//! Native discussion/comment acceptance and current Read-only creation replay.

use super::*;

use crate::key_operations::publication;

use crate::middleware::idempotency::Creation;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use futures::future::BoxFuture;

use orch_api_contract::documents::DocumentCommentCreateResponse;

use orch_api_contract::documents::DocumentCommentReceipt;

use orch_api_contract::documents::DocumentDiscussionCreateResponse;

use orch_api_contract::documents::DocumentDiscussionReceipt;

use orch_authz::Caller;

use orch_db::idempotency::creation::Created;
pub(super) struct Ready {
    pub labels: build::WrittenLabels,
    pub repeated: bool,
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    content: Vec<Inline>,
    written: Id,
    discussion: Option<Id>,
) -> ApiResult<OperationPlan> {
    let document = ctx.id("id")?;
    let target = super::super::read_operation::identity(conn, document).await?;
    let known = writers::target_plan(ctx, ctx.route.name, &target)?;
    let _ = known.preparation_principal(ctx, conn, caller).await?;
    let repeated = discussions::comment(conn, document, written)
        .await?
        .is_some_and(|row| {
            row.author_id == caller.user() && discussion.is_none_or(|id| row.discussion_id == id)
        });
    let (labels, sources) = super::super::operation::prepare_written(
        ctx,
        conn,
        caller,
        known,
        (!repeated).then_some(&content),
    )
    .await?;
    let mut plan = writers::target_plan(ctx, ctx.route.name, &target)?;
    super::super::operation::bind_sources(&mut plan, &sources)?;
    Ok(plan.with_data(Ready { labels, repeated }))
}

pub(super) fn opening<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: DocumentDiscussionCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let written = client_id(&body.client_id)?;
        let content = comments::check_content(&body.content).map_err(invalid_faults)?;
        comments::clean_quote(body.quote.as_deref())
            .map_err(|fault| invalid(&fault.path, &fault.message))?;
        prepare(ctx, conn, caller, content, written, None).await
    })
}

pub(super) fn answering<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: DocumentCommentCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let written = client_id(&body.client_id)?;
        let content = comments::check_content(&body.content).map_err(invalid_faults)?;
        prepare(
            ctx,
            conn,
            caller,
            content,
            written,
            Some(ctx.id("discussion_id")?),
        )
        .await
    })
}

fn discussion_receipt(
    document: Id,
    id: Id,
    seq: orch_types::Seq,
) -> DocumentDiscussionCreateResponse {
    DocumentDiscussionCreateResponse::Receipt(DocumentDiscussionReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        document_id: document,
        discussion_id: id,
        seq,
    })
}

fn comment_receipt(document: Id, id: Id, seq: orch_types::Seq) -> DocumentCommentCreateResponse {
    DocumentCommentCreateResponse::Receipt(DocumentCommentReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        document_id: document,
        comment_id: id,
        seq,
    })
}

pub(super) async fn accepted_discussion(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    full: &discussions::Full,
) -> ApiResult<DocumentDiscussionCreateResponse> {
    orch_db::idempotency::creation::observe_with(
        conn,
        Some(principal.user()),
        "document_discussion",
        full.discussion.id,
        json!({"document":full.discussion.document_id,"seq":full.discussion.seq_updated}),
    )
    .await?;
    let safe = discussion_receipt(
        full.discussion.document_id,
        full.discussion.id,
        full.discussion.seq_updated,
    );
    publication::receipt(ctx, &safe)?;
    Ok(
        match writers::discussion(ctx, conn, principal, full).await? {
            Some(value) => DocumentDiscussionCreateResponse::full(value),
            None => safe,
        },
    )
}

pub(super) async fn accepted_comment(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &discussions::CommentRow,
) -> ApiResult<DocumentCommentCreateResponse> {
    orch_db::idempotency::creation::observe_with(
        conn,
        Some(principal.user()),
        "document_comment",
        row.id,
        json!({"document":row.document_id,"discussion":row.discussion_id,"seq":row.seq_updated}),
    )
    .await?;
    let safe = comment_receipt(row.document_id, row.id, row.seq_updated);
    publication::receipt(ctx, &safe)?;
    Ok(match writers::comment(ctx, conn, principal, row).await? {
        Some(value) => DocumentCommentCreateResponse::full(value),
        None => safe,
    })
}

pub(super) fn discussion_binding()
-> Creation<(), DocumentDiscussionCreateBody, DocumentDiscussionCreateResponse> {
    Creation {
        kind: "document_discussion",
        replay: replay_discussion,
        identity: |value| {
            Ok(Created {
                kind: "document_discussion".into(),
                id: match value {
                    DocumentDiscussionCreateResponse::Full(value) => value.id,
                    DocumentDiscussionCreateResponse::Receipt(value) => value.discussion_id,
                },
                data: serde_json::Value::Null,
            })
        },
    }
}

pub(super) fn comment_binding()
-> Creation<(), DocumentCommentCreateBody, DocumentCommentCreateResponse> {
    Creation {
        kind: "document_comment",
        replay: replay_comment,
        identity: |value| {
            Ok(Created {
                kind: "document_comment".into(),
                id: match value {
                    DocumentCommentCreateResponse::Full(value) => value.id,
                    DocumentCommentCreateResponse::Receipt(value) => value.comment_id,
                },
                data: serde_json::Value::Null,
            })
        },
    }
}

fn replay_discussion(
    ctx: Ctx,
    (): (),
    _: DocumentDiscussionCreateBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<DocumentDiscussionCreateResponse>> {
    Box::pin(async move {
        let document =
            serde_json::from_value(created.data["document"].clone()).map_err(ApiError::internal)?;
        let seq =
            serde_json::from_value(created.data["seq"].clone()).map_err(ApiError::internal)?;
        if ctx.id("id")? != document {
            return Err(ApiError::not_found());
        }
        let safe = discussion_receipt(document, created.id, seq);
        publication::receipt(&ctx, &safe)?;
        match writers::replay_discussion(&ctx, document, created.id).await {
            Ok(Some(value)) => Ok(DocumentDiscussionCreateResponse::full(value)),
            Ok(None) => Ok(safe),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(safe)
            }
            Err(error) => Err(error),
        }
    })
}

fn replay_comment(
    ctx: Ctx,
    (): (),
    _: DocumentCommentCreateBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<DocumentCommentCreateResponse>> {
    Box::pin(async move {
        let document =
            serde_json::from_value(created.data["document"].clone()).map_err(ApiError::internal)?;
        let discussion: Id = serde_json::from_value(created.data["discussion"].clone())
            .map_err(ApiError::internal)?;
        let seq =
            serde_json::from_value(created.data["seq"].clone()).map_err(ApiError::internal)?;
        if ctx.id("id")? != document || ctx.id("discussion_id")? != discussion {
            return Err(ApiError::not_found());
        }
        let safe = comment_receipt(document, created.id, seq);
        publication::receipt(&ctx, &safe)?;
        match writers::replay_comment(&ctx, document, created.id).await {
            Ok(Some(value)) => Ok(DocumentCommentCreateResponse::full(value)),
            Ok(None) => Ok(safe),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(safe)
            }
            Err(error) => Err(error),
        }
    })
}
