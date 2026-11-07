//! The caller's private release namespace and its unchanged native custody writer.

use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::threads::UploadBlobDeleteQuery;
use orch_api_contract::upload_sources::{self as sources, Source};
use orch_authz::Principal;
use orch_db::Conn;
use orch_types::{Hash, Id};

pub(super) struct Custody;
pub(super) struct Release {
    pub(super) hash: Hash,
    account: Id,
    thread: Option<Id>,
    path: Option<String>,
}

pub(super) fn input(ctx: &Ctx, query: UploadBlobDeleteQuery) -> ApiResult<Release> {
    let hash = ctx
        .param("hash")?
        .parse::<Hash>()
        .map_err(|_| ApiError::not_found())?;
    if query.thread.is_some() != query.path.is_some() {
        return Err(ApiError::invalid(
            "path",
            "an attachment release names both its thread and path",
        ));
    }
    Ok(Release {
        hash,
        account: ctx.user()?.id(),
        thread: query.thread,
        path: query.path,
    })
}

async fn load(
    ctx: &Ctx,
    conn: &mut Conn,
    query: UploadBlobDeleteQuery,
) -> ApiResult<(Release, Vec<native::Facts<Source>>)> {
    let value = input(ctx, query)?;
    // The account is the native SQL owner namespace even for an idempotent absent hash.
    // A selected thread/path is a private reservation key, not a grant to that thread.
    let target = targets::account(conn, value.account).await?;
    Ok((
        value,
        vec![native::Facts::one(
            Source::CustodyAccount,
            KeyTargetKind::Identity(KeyIdentityKind::Upload),
            target,
        )],
    ))
}

impl native::Stored for Custody {
    type Source = Source;
    type Value = Release;
    const ROWS: &'static [Declaration<Source>] = &[sources::RELEASE];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Release, Vec<native::Facts<Source>>)>> {
        Box::pin(load(ctx, conn, UploadBlobDeleteQuery::default()))
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Release,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require(ctx, principal, value) })
    }
}

impl native::StoredQuery for Custody {
    type Query = UploadBlobDeleteQuery;

    fn load_query<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
        query: Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Release, Vec<native::Facts<Source>>)>> {
        Box::pin(load(ctx, conn, query))
    }
}

pub(super) fn require(ctx: &Ctx, principal: &Principal, value: &Release) -> ApiResult<()> {
    if principal.user() != value.account || ctx.user()?.id() != value.account {
        return Err(ApiError::internal("the upload custody changed account"));
    }
    Ok(())
}

/// Both deletes remain account-filtered; other private copies and draft pins keep their bytes.
pub(super) async fn release(conn: &mut Conn, value: &Release) -> ApiResult<()> {
    let content = serde_json::json!([{ "blob": value.hash.to_string() }]);
    let images = serde_json::json!([{ "images": [{ "blob": value.hash.to_string() }] }]);
    let attachments = serde_json::json!([{ "attachments": [{ "blob": value.hash.to_string() }] }]);
    sqlx::query("DELETE FROM thread_attachment_uploads WHERE created_by=$1 AND blob=$2 AND thread_id=$3 AND path=$4")
        .bind(value.account).bind(value.hash.as_bytes().as_slice()).bind(value.thread).bind(&value.path)
        .execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
    sqlx::query(
        "DELETE FROM completed_uploads WHERE user_id=$1 AND hash=$2
      AND NOT EXISTS(SELECT 1 FROM thread_attachment_uploads WHERE created_by=$1 AND blob=$2)
      AND NOT EXISTS(SELECT 1 FROM drafts WHERE user_id=$1 AND (content @> $3 OR content @> $4 OR content @> $5))
      AND NOT EXISTS(SELECT 1 FROM chat_states WHERE user_id=$1 AND (draft @> $3 OR draft @> $4 OR draft @> $5))",
    )
    .bind(value.account)
    .bind(value.hash.as_bytes().as_slice())
    .bind(&content)
    .bind(&images)
    .bind(&attachments)
    .execute(conn)
    .await
    .map_err(orch_db::DbError::from)?;
    Ok(())
}
