//! Conversation-private blob custody, using the ordinary chat attachment machinery.

use super::*;

use crate::key_operations::blob_sources;

use crate::key_operations::targets;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_types::Hash;
pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a orch_authz::Caller,
    (): (),
    body: api::AskAttachmentBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.bind(
            0,
            KeyTargetKind::Object(ObjectKind::Thread),
            &[targets::node(conn, ctx.id("id")?).await?],
        )?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let (_, inspecting, _) = opening_for(
            ctx,
            conn,
            &principal,
            ctx.id("id")?,
            false,
            &[Permission::ThreadInteract],
        )
        .await?;
        if inspecting {
            return Err(denied("Only the person may attach a file to Ask"));
        }
        let prepared = blob_sources::prepare(ctx, conn, caller, &plan, &[body.blob], &[]).await?;
        blob_sources::bind(&mut plan, &prepared.bindings)?;
        Ok(plan)
    })
}

fn reference(bytes: &[u8]) -> Option<(Hash, String)> {
    serde_json::from_slice::<api::AskAttachmentBody>(bytes)
        .ok()
        .map(|body| (body.blob, body.name))
}

pub(super) async fn create(
    ctx: Ctx,
    (): (),
    body: api::AskAttachmentBody,
) -> ApiResult<api::AskAttachmentReply> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let (row, inspecting, _) = opening_for(
        &ctx,
        &mut conn,
        &principal,
        ctx.id("id")?,
        false,
        &[Permission::ThreadInteract],
    )
    .await?;
    if inspecting {
        return Err(denied(
            "Inspecting administrators may read attachments, but cannot add them",
        ));
    }
    let limit = setting::<i64>(
        &mut conn,
        Place::Node(row.root_org_id),
        "chat.attachment_max_bytes",
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(i64::MAX);
    drop(conn);
    let mut file_ctx = ctx.clone();
    file_ctx
        .headers
        .remove(orch_api_contract::threads::FILENAME_HEADER);
    let taken = crate::attachments::take(
        &file_ctx,
        axum::body::Body::from(serde_json::to_vec(&body).map_err(ApiError::internal)?),
        limit,
        reference,
    )
    .await
    .map_err(|error| match error {
        crate::attachments::Refused::Other(error) => error,
        crate::attachments::Refused::TooLarge { limit, .. } => ApiError::new(
            ErrorCode::RequestTooLarge,
            "The file exceeds this conversation's ordinary attachment limit",
        )
        .with("limit", limit),
    })?;
    let described = crate::attachments::describe(&ctx, &taken.meta, &taken.name).await?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let (_, inspecting, _) = opening_for(
        &ctx,
        &mut tx,
        guard.principal(),
        row.id,
        true,
        &[Permission::ThreadInteract],
    )
    .await?;
    if inspecting {
        return Err(denied("Only the person may attach a file"));
    }
    guard.check_deadline(&ctx)?;
    crate::attachments::retain(&ctx, &mut tx, guard.principal(), &taken).await?;
    rows::attach(
        &mut tx,
        row.id,
        &taken.meta.hash,
        &taken.name,
        &described.media_type,
        taken.meta.size,
        ctx.app.clock.now(),
    )
    .await?;
    let (current, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    opening_for(
        &current,
        &mut tx,
        guard.principal(),
        row.id,
        false,
        &[Permission::ThreadInteract],
    )
    .await?;
    guard.check_deadline(&current)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(api::AskAttachmentReply {
        blob: taken.meta.hash,
        name: taken.name,
        size: taken.meta.size,
        media_type: described.media_type,
    })
}

pub(super) async fn get(
    ctx: Ctx,
    query: orch_api_contract::chats::ChatBlobGetQuery,
    _body: axum::body::Body,
) -> ApiResult<axum::response::Response> {
    let hash = ctx
        .param("hash")?
        .parse::<Hash>()
        .map_err(|_| ApiError::not_found())?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    opening(&ctx, &mut conn, &principal, ctx.id("id")?, false).await?;
    let file = rows::file(&mut conn, ctx.id("id")?, &hash)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if query.preview == Some(true) {
        return crate::previews::serve(&ctx, &hash, &file.name, &file.media_type).await;
    }
    crate::attachments::serve_kept(
        &ctx,
        &hash,
        &file.name,
        &file.media_type,
        &["image"],
        query.download.unwrap_or(false),
    )
    .await
}

pub(crate) fn url(thread: Id, hash: &Hash, temporary: bool) -> String {
    if temporary {
        format!("/api/v1/ask/conversations/{thread}/attachments/{hash}")
    } else {
        format!("/api/v1/threads/{thread}/blobs/{hash}")
    }
}
